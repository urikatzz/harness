//! Company tools and skills loaded from disk.
//!
//! Layout (relative to the harness home, `URITHEKING_HOME` or the current dir):
//!   tools/<name>/tool.json   — {"name","description","params":{arg: description},"command":[..],
//!                               optional: "reads":[project folders, "." = whole project],
//!                               "writes":[project folders], "network": true, "env":[VAR names]}
//!   COMPANY.md               — company/product context, sent with every call
//!   skills/<name>/SKILL.md   — frontmatter `name:` / `description:`, then instructions
//!
//! A tool's command runs inside its own folder, gets the call's arguments as JSON on
//! stdin (and as `ARG_<NAME>` env vars), and whatever it prints is the result.

use crate::provider::Result;
use serde_json::{Map, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use crate::permissions::Policy;
use std::time::Duration;

/// Built-in tools every role can be granted: (name, description, params).
pub const BUILTINS: &[(&str, &str, &str)] = &[
    ("read_file", "Read a project file.", "path"),
    ("write_file", "Create or overwrite a project file.", "path, content"),
];

pub struct Tool {
    pub name: String,
    pub description: String,
    pub params: Map<String, Value>,
    command: Vec<String>,
    dir: PathBuf,
    /// Project folders (absolute) the command may read.
    reads: Vec<PathBuf>,
    /// Project folders (absolute) the command may write to.
    writes: Vec<PathBuf>,
    /// Outbound network access (off by default).
    network: bool,
    /// Environment variables passed through by name; all others are dropped.
    env: Vec<String>,
}

pub struct Skill {
    pub name: String,
    pub description: String,
    path: PathBuf,
}

#[derive(Default)]
pub struct Toolbox {
    root: PathBuf,
    pub tools: Vec<Tool>,
    pub skills: Vec<Skill>,
    /// COMPANY.md: what the company and product are; sent with every call.
    pub company: Option<String>,
    /// Folders that failed to load, with why, so the UI can warn instead of silently skipping.
    pub warnings: Vec<String>,
}

/// Where `tools/` and `skills/` are looked up.
pub fn home() -> PathBuf {
    std::env::var("URITHEKING_HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
}

impl Toolbox {
    pub fn load(root: &Path) -> Self {
        let mut tb = Toolbox { root: root.canonicalize().unwrap_or_else(|_| root.to_path_buf()), ..Default::default() };
        let root = tb.root.clone();
        tb.company = read_project_file(&root, "COMPANY.md").ok().filter(|t| !t.trim().is_empty());
        for dir in subdirs(&root.join("tools")) {
            let safe = crate::paths::resolve(&root, &dir.strip_prefix(&root).unwrap().to_string_lossy());
            match safe.map_err(Into::into).and_then(|dir| load_tool(&root, &dir)) {
                Ok(t) => tb.tools.push(t),
                Err(e) => tb.warnings.push(format!("{}: {e}", dir.display())),
            }
        }
        for dir in subdirs(&root.join("skills")) {
            let safe = crate::paths::resolve(&root, &dir.strip_prefix(&root).unwrap().to_string_lossy());
            match safe.map_err(Into::into).and_then(|dir| load_skill(&dir)) {
                Ok(s) => tb.skills.push(s),
                Err(e) => tb.warnings.push(format!("{}: {e}", dir.display())),
            }
        }
        tb
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty() && self.skills.is_empty()
    }

    /// Whether a call can change files or reach the outside world. Unknown names count as yes.
    pub fn has_side_effects(&self, name: &str) -> bool {
        match name {
            "read_file" | "load_skill" => false,
            "write_file" => true,
            _ => self.tools.iter().find(|t| t.name == name).is_none_or(|t| !t.writes.is_empty() || t.network),
        }
    }

    /// Whether a call can change project files (so a configured verify tool should run after it).
    pub fn changes_files(&self, name: &str) -> bool {
        name == "write_file" || self.tools.iter().any(|t| t.name == name && !t.writes.is_empty())
    }

    /// Runs a tool by name, or a built-in (`load_skill`, `read_file`, `write_file`).
    /// Permission checks happen before this, in `agent::run_turn`.
    pub fn call(&self, name: &str, args: &Value, policy: &Policy) -> Result<String> {
        let path = || args["path"].as_str().ok_or_else(|| format!("{name} needs {{\"path\": ...}}"));
        match name {
            "read_file" => return read_project_file(&self.root, path()?),
            "write_file" => {
                let content = args["content"].as_str().ok_or("write_file needs \"content\"")?;
                crate::paths::open(&self.root, path()?, true)?.write_all(content.as_bytes())?;
                return Ok(format!("wrote {} bytes", content.len()));
            }
            _ => {}
        }
        if name == "load_skill" {
            let wanted = args["name"].as_str().ok_or("load_skill needs {\"name\": ...}")?;
            let skill = self.skills.iter().find(|s| s.name == wanted).ok_or_else(|| format!("no skill `{wanted}`"))?;
            return read_project_file(&self.root, &skill.path.strip_prefix(&self.root)?.to_string_lossy());
        }
        let tool = self.tools.iter().find(|t| t.name == name).ok_or_else(|| format!("no tool `{name}`"))?;
        tool.run(args, policy)
    }
}

impl Tool {
    fn run(&self, args: &Value, policy: &Policy) -> Result<String> {
        let (program, rest) = self.command.split_first().ok_or("empty command")?;
        let scratch = crate::process::Scratch::new()?;
        let access = crate::process::ToolAccess {
            dir: &self.dir, reads: &self.reads, writes: &self.writes, network: self.network, env: &self.env,
        };
        let mut cmd = crate::process::tool_command(program, rest, &access, &scratch, policy)?;
        if let Some(obj) = args.as_object() {
            for (k, v) in obj {
                if !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return Err(format!("argument name `{k}` must be letters, digits or _").into());
                }
                let v = v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string());
                cmd.command.env(format!("ARG_{}", k.to_uppercase()), v);
            }
        }
        let out = cmd.run(args.to_string().into_bytes(), Duration::from_secs(60))?;
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(format!("tool `{}` failed ({}): {} {}", self.name, out.status, stderr.trim(), stdout).into());
        }
        Ok(stdout)
    }
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// Top-level folders a tool may never write: the harness's own config and state.
const RESERVED: &[&str] = &["tools", "skills", "sessions", "logs"];

fn load_tool(root: &Path, dir: &Path) -> Result<Tool> {
    let raw = read_project_file(dir, "tool.json").map_err(|e| format!("tool.json: {e}"))?;
    let v: Value = serde_json::from_str(&raw).map_err(|e| format!("tool.json: {e}"))?;
    let command: Vec<String> = v["command"]
        .as_array()
        .ok_or("tool.json needs \"command\": [program, args...]")?
        .iter()
        .filter_map(|c| c.as_str().map(str::to_owned))
        .collect();
    Ok(Tool {
        name: v["name"].as_str().ok_or("tool.json needs \"name\"")?.to_string(),
        description: v["description"].as_str().unwrap_or("").to_string(),
        params: v["params"].as_object().cloned().unwrap_or_default(),
        command,
        dir: dir.to_path_buf(),
        reads: readable_folders(root, &v["reads"])?,
        writes: writable_folders(root, &v["writes"])?,
        network: match &v["network"] {
            Value::Null => false,
            Value::Bool(b) => *b,
            _ => return Err("tool.json \"network\" must be true or false".into()),
        },
        env: match &v["env"] {
            Value::Null => Vec::new(),
            list => list.as_array().filter(|l| l.iter().all(Value::is_string))
                .ok_or("tool.json \"env\" must be a list of variable names")?
                .iter().filter_map(|e| e.as_str().map(str::to_owned)).collect(),
        },
    })
}

/// `reads` entries are existing project folders; "." means the whole project. Harness state
/// (`sessions/`, `logs/`) and protected files stay hidden either way.
fn readable_folders(root: &Path, value: &Value) -> Result<Vec<PathBuf>> {
    if value.is_null() { return Ok(Vec::new()); }
    let list = value.as_array().ok_or("tool.json \"reads\" must be a list of folders")?;
    list.iter().map(|entry| {
        let rel = entry.as_str().ok_or("tool.json \"reads\" must be a list of folders")?;
        if rel.trim_end_matches('/') == "." { return Ok(root.to_path_buf()); }
        let path = crate::paths::resolve(root, rel).map_err(|e| format!("reads `{rel}`: {e}"))?;
        if !path.is_dir() { return Err(format!("reads `{rel}`: not an existing folder").into()); }
        Ok(path)
    }).collect()
}

/// `writes` entries must be existing folders inside the project, below the top level.
fn writable_folders(root: &Path, value: &Value) -> Result<Vec<PathBuf>> {
    if value.is_null() { return Ok(Vec::new()); }
    let list = value.as_array().ok_or("tool.json \"writes\" must be a list of folders")?;
    list.iter().map(|entry| {
        let rel = entry.as_str().ok_or("tool.json \"writes\" must be a list of folders")?;
        let path = crate::paths::resolve(root, rel).map_err(|e| format!("writes `{rel}`: {e}"))?;
        let first = path.strip_prefix(root)?.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned());
        if first.as_deref().is_some_and(|f| RESERVED.contains(&f)) {
            return Err(format!("writes `{rel}`: tools may not write to {}", RESERVED.join("/, ") + "/").into());
        }
        if !path.is_dir() { return Err(format!("writes `{rel}`: not an existing folder").into()); }
        Ok(path)
    }).collect()
}

fn load_skill(dir: &Path) -> Result<Skill> {
    let path = dir.join("SKILL.md");
    let text = read_project_file(dir, "SKILL.md").map_err(|e| format!("SKILL.md: {e}"))?;
    let field = |key: &str| {
        frontmatter(&text)
            .lines()
            .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix(':')))
            .map(|v| v.trim().to_string())
    };
    let fallback = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
    Ok(Skill { name: field("name").unwrap_or(fallback), description: field("description").unwrap_or_default(), path })
}

fn frontmatter(text: &str) -> &str {
    text.strip_prefix("---")
        .and_then(|rest| rest.find("\n---").map(|end| &rest[..end]))
        .unwrap_or("")
}

fn read_project_file(root: &Path, path: &str) -> Result<String> {
    let mut content = String::new();
    crate::paths::open(root, path, false)?.take(1024 * 1024 + 1).read_to_string(&mut content)?;
    if content.len() > 1024 * 1024 { return Err("file exceeds 1 MiB".into()); }
    Ok(content)
}

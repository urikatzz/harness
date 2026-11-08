//! Company tools and skills loaded from disk.
//!
//! Layout (relative to the harness home, `URITHEKING_HOME` or the current dir):
//!   tools/<name>/tool.json   — {"name","description","params":{arg: description},"command":[..]}
//!   COMPANY.md               — company/product context, sent with every call
//!   skills/<name>/SKILL.md   — frontmatter `name:` / `description:`, then instructions
//!
//! A tool's command runs inside its own folder, gets the call's arguments as JSON on
//! stdin (and as `ARG_<NAME>` env vars), and whatever it prints is the result.

use crate::provider::Result;
use serde_json::{Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
}

pub struct Skill {
    pub name: String,
    pub description: String,
    path: PathBuf,
}

#[derive(Default)]
pub struct Toolbox {
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
        let mut tb = Toolbox::default();
        tb.company = std::fs::read_to_string(root.join("COMPANY.md")).ok().filter(|t| !t.trim().is_empty());
        for dir in subdirs(&root.join("tools")) {
            match load_tool(&dir) {
                Ok(t) => tb.tools.push(t),
                Err(e) => tb.warnings.push(format!("{}: {e}", dir.display())),
            }
        }
        for dir in subdirs(&root.join("skills")) {
            match load_skill(&dir) {
                Ok(s) => tb.skills.push(s),
                Err(e) => tb.warnings.push(format!("{}: {e}", dir.display())),
            }
        }
        tb
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty() && self.skills.is_empty()
    }

    /// Runs a tool by name, or a built-in (`load_skill`, `read_file`, `write_file`).
    /// Permission checks happen before this, in `agent::run_turn`.
    pub fn call(&self, name: &str, args: &Value) -> Result<String> {
        let path = || args["path"].as_str().ok_or_else(|| format!("{name} needs {{\"path\": ...}}"));
        match name {
            "read_file" => return Ok(std::fs::read_to_string(home().join(path()?))?),
            "write_file" => {
                let content = args["content"].as_str().ok_or("write_file needs \"content\"")?;
                std::fs::write(home().join(path()?), content)?;
                return Ok(format!("wrote {} bytes", content.len()));
            }
            _ => {}
        }
        if name == "load_skill" {
            let wanted = args["name"].as_str().ok_or("load_skill needs {\"name\": ...}")?;
            let skill = self.skills.iter().find(|s| s.name == wanted).ok_or_else(|| format!("no skill `{wanted}`"))?;
            return Ok(std::fs::read_to_string(&skill.path)?);
        }
        let tool = self.tools.iter().find(|t| t.name == name).ok_or_else(|| format!("no tool `{name}`"))?;
        tool.run(args)
    }
}

impl Tool {
    fn run(&self, args: &Value) -> Result<String> {
        let (program, rest) = self.command.split_first().ok_or("empty command")?;
        let mut cmd = Command::new(program);
        cmd.args(rest).current_dir(&self.dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(obj) = args.as_object() {
            for (k, v) in obj {
                let v = v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string());
                cmd.env(format!("ARG_{}", k.to_uppercase()), v);
            }
        }
        let mut child = cmd.spawn().map_err(|e| format!("could not run `{program}`: {e}"))?;
        child.stdin.take().unwrap().write_all(args.to_string().as_bytes())?;
        let out = child.wait_with_output()?;
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

fn load_tool(dir: &Path) -> Result<Tool> {
    let raw = std::fs::read_to_string(dir.join("tool.json")).map_err(|e| format!("tool.json: {e}"))?;
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
    })
}

fn load_skill(dir: &Path) -> Result<Skill> {
    let path = dir.join("SKILL.md");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("SKILL.md: {e}"))?;
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

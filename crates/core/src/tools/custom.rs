//! Custom tools: `tools/<name>/tool.json` plus whatever program it runs.
//!
//! {
//!   "name": "company_info",
//!   "description": "...",
//!   "params": {"topic": "what to look up"},   // argument → description
//!   "command": ["sh", "run.sh"],               // run inside the tool's folder
//!   "reads":  ["docs"],                        // optional project folders; "." = whole project
//!   "writes": ["out"],                         // optional project folders, below the top level
//!   "network": false,                          // optional outbound network
//!   "env": ["API_TOKEN"]                       // optional variables passed through by name
//! }
//!
//! The command gets the call's arguments as JSON on stdin and as `ARG_<NAME>` env vars;
//! whatever it prints is the result. It runs in the OS sandbox unless the policy turns it off.

use super::{read_project_file, Access, Context, Effects, Tool};
use crate::error::{Error, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long one call may run.
const TIMEOUT: Duration = Duration::from_secs(60);
/// Top-level folders a tool may never write: the harness's own config and state.
const RESERVED: &[&str] = &["tools", "skills", "sessions", "logs"];

/// `tool.json` exactly as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    params: BTreeMap<String, String>,
    command: Vec<String>,
    #[serde(default)]
    reads: Vec<String>,
    #[serde(default)]
    writes: Vec<String>,
    #[serde(default)]
    network: bool,
    #[serde(default)]
    env: Vec<String>,
}

pub struct CustomTool {
    name: String,
    description: String,
    params: BTreeMap<String, String>,
    command: Vec<String>,
    dir: PathBuf,
    /// Project folders (absolute) the command may read.
    reads: Vec<PathBuf>,
    /// Project folders (absolute) the command may write to.
    writes: Vec<PathBuf>,
    network: bool,
    /// Environment variables passed through by name; all others are dropped.
    env: Vec<String>,
}

impl CustomTool {
    pub fn load(root: &Path, dir: &Path) -> Result<Self> {
        let fail = |e: &dyn std::fmt::Display| Error::config(format!("{}: tool.json: {e}", dir.display()));
        let raw = read_project_file(dir, "tool.json").map_err(|e| fail(&e))?;
        let spec: Spec = serde_json::from_str(&raw).map_err(|e| fail(&e))?;
        if spec.command.is_empty() { return Err(fail(&"\"command\" must name a program")); }
        Ok(CustomTool {
            reads: spec.reads.iter().map(|rel| readable_folder(root, rel)).collect::<Result<_>>().map_err(|e| fail(&e))?,
            writes: spec.writes.iter().map(|rel| writable_folder(root, rel)).collect::<Result<_>>().map_err(|e| fail(&e))?,
            name: spec.name,
            description: spec.description,
            params: spec.params,
            command: spec.command,
            dir: dir.to_path_buf(),
            network: spec.network,
            env: spec.env,
        })
    }
}

impl Tool for CustomTool {
    fn name(&self) -> &str { &self.name }
    fn description(&self) -> &str { &self.description }
    fn params(&self) -> Vec<(&str, &str)> {
        self.params.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
    }
    fn effects(&self) -> Effects { Effects { writes_files: !self.writes.is_empty(), network: self.network } }
    /// A `path` argument still goes through the file rules, as a read.
    fn path_arg(&self) -> Option<(&'static str, Access)> { Some(("path", Access::Read)) }

    fn run(&self, ctx: &Context, args: &Value) -> Result<String> {
        let (program, rest) = self.command.split_first().ok_or("empty command")?;
        let scratch = crate::sandbox::Scratch::new()?;
        let access = crate::sandbox::ToolAccess {
            dir: &self.dir, reads: &self.reads, writes: &self.writes, network: self.network, env: &self.env,
        };
        let mut cmd = crate::sandbox::tool_command(program, rest, &access, &scratch, ctx.policy)?;
        for (k, v) in args.as_object().into_iter().flatten() {
            if !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(format!("argument name `{k}` must be letters, digits or _").into());
            }
            let v = v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string());
            cmd.command.env(format!("ARG_{}", k.to_uppercase()), v);
        }
        let out = cmd.run(args.to_string().into_bytes(), TIMEOUT)?;
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(format!("tool `{}` failed ({}): {} {}", self.name, out.status, stderr.trim(), stdout).into());
        }
        Ok(stdout)
    }
}

/// A `reads` entry: an existing project folder, or "." for the whole project. Harness state
/// (`sessions/`, `logs/`) and protected files stay hidden either way.
fn readable_folder(root: &Path, rel: &str) -> Result<PathBuf> {
    if rel.trim_end_matches('/') == "." { return Ok(root.to_path_buf()); }
    let path = crate::paths::resolve(root, rel).map_err(|e| format!("reads `{rel}`: {e}"))?;
    if !path.is_dir() { return Err(format!("reads `{rel}`: not an existing folder").into()); }
    Ok(path)
}

/// A `writes` entry: an existing project folder below the top level, outside harness folders.
fn writable_folder(root: &Path, rel: &str) -> Result<PathBuf> {
    let path = crate::paths::resolve(root, rel).map_err(|e| format!("writes `{rel}`: {e}"))?;
    let first = path.strip_prefix(root)?.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned());
    if first.as_deref().is_some_and(|f| RESERVED.contains(&f)) {
        return Err(format!("writes `{rel}`: tools may not write to {}", RESERVED.join("/, ") + "/").into());
    }
    if !path.is_dir() { return Err(format!("writes `{rel}`: not an existing folder").into()); }
    Ok(path)
}

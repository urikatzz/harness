//! Guardrails from `permissions.json` in the harness home:
//!
//! {
//!   "mode": "ask",                       // "ask": confirm every tool call · "yolo": run without asking
//!   "roles": {                           // which tools/skills each team may use ("*" = all)
//!     "sales": { "tools": ["company_info"], "skills": [] },
//!     "art":   { "tools": ["read_file"],    "skills": ["*"] }
//!   },
//!   "files": {
//!     "deny":    ["secrets/**", "**/*.pem"],   // never touched, not even with approval
//!     "approve": [".env", "config/**"]         // always need a manual yes, even in yolo
//!   },
//!   "verify": "run_checks",               // tool run after file changes, before the answer
//!   "trust_tool_output": false            // true: yolo stays yolo after reading tool output
//! }
//!
//! Built in, whatever the file says: `write_file` may never touch `sessions/` or `logs/`, and
//! writes to `permissions.json`, `COMPANY.md`, `tools/` and `skills/` always need a manual yes.
//!
//! The role comes from `--role` / `URITHEKING_ROLE`. No file = everything allowed, mode "ask".
//! With roles defined, an unknown or missing role gets no tools.

use crate::error::{Error, Result};
use crate::tools::{Access, Gate, Tool};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The reason given for a call that only needs a yes because the mode is "ask".
pub const ASK_MODE: &str = "ask mode";

/// Harness files a model-driven write must never change.
const STATE_DIRS: &[&str] = &["sessions/**", "logs/**", "memory/**"];
/// Harness configuration: changing it changes what the model may do, so a human must agree.
const CONFIG_FILES: &[&str] = &["permissions.json", "COMPANY.md", "tools/**", "skills/**"];

#[derive(Debug, PartialEq)]
pub enum Decision {
    Allow,
    /// Needs a human yes; the string says why.
    Ask(String),
    Deny(String),
}

/// The tools and skills one role may use (`"*"` = all).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Allowed {
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
}

#[derive(Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Mode {
    #[default]
    Ask,
    Yolo,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRules {
    #[serde(default)]
    deny: Vec<String>,
    #[serde(default)]
    approve: Vec<String>,
}

/// `permissions.json` exactly as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    #[serde(default)]
    mode: Mode,
    #[serde(default = "yes")]
    sandbox: bool,
    roles: Option<BTreeMap<String, Allowed>>,
    #[serde(default)]
    files: FileRules,
    verify: Option<String>,
    #[serde(default)]
    trust_tool_output: bool,
    /// Roles that may write company memory.
    #[serde(default)]
    owners: Vec<String>,
}

fn yes() -> bool { true }

#[derive(Default, Clone)]
pub struct Policy {
    root: PathBuf,
    pub yolo: bool,
    /// `"sandbox": false`: run custom tools and CLI backends without the OS sandbox.
    pub unsandboxed: bool,
    pub role: Option<String>,
    /// Every configured role; None = no roles configured, so everything is allowed.
    roles: Option<BTreeMap<String, Allowed>>,
    /// What the current role may use; None = no roles configured.
    allowed: Option<Allowed>,
    deny_files: Vec<String>,
    approve_files: Vec<String>,
    /// Tool to run after file changes, before a final answer is accepted.
    pub verify: Option<String>,
    /// Skip the extra approval for side-effecting calls after tool output entered the chat.
    pub trust_tool_output: bool,
    owners: Vec<String>,
}

impl Policy {
    /// Rules and project in one folder.
    pub fn load(root: &Path, role: Option<String>) -> Result<Self> {
        Self::load_with(root, root, role)
    }

    /// Rules from `<company>/permissions.json`, applied to files in `project`.
    pub fn load_with(company: &Path, project: &Path, role: Option<String>) -> Result<Self> {
        let root = project.canonicalize()?;
        let path = company.join("permissions.json");
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Policy { root, role, ..Default::default() }),
            Err(e) => return Err(Error::config(format!("{}: {e}", path.display()))),
        };
        let file: PolicyFile = serde_json::from_str(&raw).map_err(|e| Error::config(format!("{}: {e}", path.display())))?;
        if file.verify.as_deref().is_some_and(str::is_empty) {
            return Err(Error::config("verify must be a tool name"));
        }
        let allowed = file.roles.as_ref().map(|roles| role.as_deref().and_then(|r| roles.get(r)).cloned().unwrap_or_default());
        Ok(Policy {
            root,
            yolo: file.mode == Mode::Yolo,
            unsandboxed: !file.sandbox,
            role,
            roles: file.roles,
            allowed,
            deny_files: file.files.deny,
            approve_files: file.files.approve,
            verify: file.verify,
            trust_tool_output: file.trust_tool_output,
            owners: file.owners,
        })
    }

    /// The same rules in ask mode, so every call goes through the approval callback.
    pub fn for_eval(&self) -> Self {
        Policy { yolo: false, ..self.clone() }
    }

    /// No tools or skills at all; used when the rules file can't be read.
    pub fn locked(role: Option<String>) -> Self {
        Policy { role, allowed: Some(Allowed::default()), ..Default::default() }
    }

    pub fn can_use_tool(&self, name: &str) -> bool {
        self.allowed.as_ref().is_none_or(|a| listed(&a.tools, name))
    }

    /// Personal and own-team memory are open to anyone who has `remember`; company memory only to
    /// owners. With no roles configured there is nobody to tell apart, so everything is allowed.
    /// This is a local guard; the company repo's push rules are what really protect shared memory.
    pub fn may_write_memory(&self, scope: &crate::memory::Scope) -> std::result::Result<(), String> {
        let is_owner = self.roles.is_none() || self.role.as_ref().is_some_and(|r| self.owners.contains(r));
        match scope {
            crate::memory::Scope::Company if !is_owner => Err("only owners can change company memory".into()),
            _ => Ok(()),
        }
    }

    pub fn can_use_skill(&self, name: &str) -> bool {
        self.allowed.as_ref().is_none_or(|a| listed(&a.skills, name))
    }

    /// Configured roles that may use `tool`, for pointing the user at one.
    pub fn roles_with_tool(&self, tool: &str) -> Vec<&str> {
        self.roles.iter().flatten().filter(|(_, a)| listed(&a.tools, tool)).map(|(name, _)| name.as_str()).collect()
    }
    pub(crate) fn protected_patterns(&self) -> impl Iterator<Item = &str> {
        self.deny_files.iter().chain(&self.approve_files).map(String::as_str)
    }

    pub(crate) fn root(&self) -> &Path { &self.root }

    /// What should happen to one call of `tool`.
    pub fn check(&self, tool: &dyn Tool, args: &Value) -> Decision {
        let who = self.role.as_deref().unwrap_or("no role");
        let name = tool.name();
        match tool.gate() {
            Gate::Skill { arg } => {
                let skill = args[arg].as_str().unwrap_or("");
                return if self.can_use_skill(skill) { Decision::Allow } else { Decision::Deny(format!("skill `{skill}` is not allowed for {who}")) };
            }
            Gate::Tool if !self.can_use_tool(name) => return Decision::Deny(format!("tool `{name}` is not allowed for {who}")),
            Gate::Tool => {}
        }
        if let Some((arg, access)) = tool.path_arg() {
            if let Some(path) = args[arg].as_str() {
                if let Some(decision) = self.check_path(path, access) { return decision; }
            }
        }
        if self.yolo { Decision::Allow } else { Decision::Ask(ASK_MODE.into()) }
    }

    /// File rules for one path; None means the path itself needs nothing special.
    fn check_path(&self, path: &str, access: Access) -> Option<Decision> {
        let resolved = match crate::paths::resolve(&self.root, path) {
            Ok(path) => path,
            Err(reason) => return Some(Decision::Deny(reason)),
        };
        let path = resolved.strip_prefix(&self.root).unwrap_or(&resolved).to_string_lossy();
        let writes = access == Access::Write;
        if self.deny_files.iter().any(|g| glob(g, &path)) {
            return Some(Decision::Deny(format!("`{path}` is a protected file")));
        }
        if writes && STATE_DIRS.iter().any(|g| glob(g, &path)) {
            return Some(Decision::Deny(format!("`{path}` is harness state and cannot be written")));
        }
        if writes && CONFIG_FILES.iter().any(|g| glob(g, &path)) {
            return Some(Decision::Ask(format!("`{path}` is harness configuration and requires manual approval")));
        }
        if self.approve_files.iter().any(|g| glob(g, &path)) {
            return Some(Decision::Ask(format!("`{path}` requires manual approval")));
        }
        None
    }
}

fn listed(list: &[String], name: &str) -> bool {
    list.iter().any(|x| x == "*" || x == name)
}

/// Minimal glob: `*` matches within one path segment, `**` across segments.
pub fn glob(pattern: &str, path: &str) -> bool {
    fn go(p: &[u8], s: &[u8]) -> bool {
        match p {
            [] => s.is_empty(),
            [b'*', b'*', b'/', rest @ ..] => go(rest, s) || (!s.is_empty() && go(p, &s[1..])),
            [b'*', b'*', rest @ ..] => (0..=s.len()).any(|i| go(rest, &s[i..])),
            [b'*', rest @ ..] => (0..=s.len()).take_while(|&i| i == 0 || s[i - 1] != b'/').any(|i| go(rest, &s[i..])),
            [c, rest @ ..] => s.first() == Some(c) && go(rest, &s[1..]),
        }
    }
    go(pattern.as_bytes(), path.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Context;
    use serde_json::json;

    /// A custom tool that takes no path, for role checks.
    struct Named(String);
    impl Tool for Named {
        fn name(&self) -> &str { &self.0 }
        fn description(&self) -> &str { "" }
        fn params(&self) -> Vec<(&str, &str)> { vec![] }
        fn run(&self, _: &Context, _: &Value) -> Result<String> { Ok(String::new()) }
    }

    #[test]
    fn only_owner_roles_may_write_company_memory() {
        use crate::memory::Scope;
        let dir = crate::test_support::TestDir::new();
        std::fs::write(dir.path().join("permissions.json"), r#"{"roles":{"owner":{"tools":["*"]},"sales":{"tools":["remember"]}},"owners":["owner"]}"#).unwrap();
        let sales = Policy::load(dir.path(), Some("sales".into())).unwrap();
        assert!(sales.may_write_memory(&Scope::Company).is_err());
        assert!(sales.may_write_memory(&Scope::Team("sales".into())).is_ok());
        assert!(sales.may_write_memory(&Scope::Personal).is_ok());
        assert!(Policy::load(dir.path(), Some("owner".into())).unwrap().may_write_memory(&Scope::Company).is_ok());
        assert!(Policy::load(dir.path(), None).unwrap().may_write_memory(&Scope::Company).is_err());
    }

    fn check(p: &Policy, tool: &str, args: Value) -> Decision {
        let tb = crate::tools::Toolbox::default();
        let named = Named(tool.to_string());
        p.check(tb.get(tool).unwrap_or(&named), &args)
    }

    #[test]
    fn rejects_malformed_policy_and_normalizes_only_safe_paths() {
        let dir = crate::test_support::TestDir::new();
        for invalid in ["null", r#"{"roles":[]}"#, r#"{"roles":{"dev":{"tools":"*"}}}"#, r#"{"files":{"deny":"secret"}}"#, r#"{"mode":"typo"}"#, r#"{"sandbox":"off"}"#, r#"{"verfy":"x"}"#, r#"{"roles":{"dev":{"tool":["*"]}}}"#] {
            std::fs::write(dir.path().join("permissions.json"), invalid).unwrap();
            assert!(Policy::load(dir.path(), None).is_err(), "{invalid}");
        }
        std::fs::write(dir.path().join("permissions.json"), r#"{"mode":"yolo","files":{"deny":["secret"]}}"#).unwrap();
        let policy = Policy::load(dir.path(), None).unwrap();
        assert!(matches!(check(&policy, "read_file", json!({"path":"././../../outside"})), Decision::Deny(_)));
        assert!(matches!(check(&policy, "read_file", json!({"path":"././secret"})), Decision::Deny(_)));
        assert_eq!(check(&policy, "read_file", json!({"path":"././public"})), Decision::Allow);
        std::fs::remove_file(dir.path().join("permissions.json")).unwrap();
        std::fs::create_dir(dir.path().join("permissions.json")).unwrap();
        assert!(Policy::load(dir.path(), None).is_err());
    }

    #[test]
    fn globs() {
        assert!(glob("secrets/**", "secrets/a/b.txt"));
        assert!(glob("**/*.pem", "a/b/key.pem"));
        assert!(glob("**/*.pem", "key.pem"));
        assert!(glob("*.env", "prod.env"));
        assert!(!glob("*.env", "dir/prod.env"));
    }

    #[test]
    fn roles_and_files() {
        let p = Policy {
            root: std::env::current_dir().unwrap(),
            yolo: true,
            unsandboxed: false,
            role: Some("sales".into()),
            allowed: Some(Allowed { tools: vec!["read_file".into()], skills: vec![] }),
            deny_files: vec!["secrets/**".into()],
            approve_files: vec![".env".into()],
            ..Default::default()
        };
        assert_eq!(check(&p, "read_file", json!({"path": "README.md"})), Decision::Allow);
        assert!(matches!(check(&p, "read_file", json!({"path": ".env"})), Decision::Ask(_)));
        assert!(matches!(check(&p, "read_file", json!({"path": "secrets/k"})), Decision::Deny(_)));
        assert!(matches!(check(&p, "read_file", json!({"path": "../x"})), Decision::Deny(_)));
        assert!(matches!(check(&p, "company_info", json!({})), Decision::Deny(_)));
    }

    #[test]
    fn skills_are_granted_by_the_skills_list_and_roles_are_listed() {
        let dir = crate::test_support::TestDir::new();
        std::fs::write(dir.path().join("permissions.json"),
            r#"{"roles":{"dev":{"tools":["*"],"skills":["*"]},"art":{"tools":["read_file"]},"sales":{"tools":["x"],"skills":[]}}}"#).unwrap();
        let sales = Policy::load(dir.path(), Some("sales".into())).unwrap();
        assert!(matches!(check(&sales, "load_skill", json!({"name": "review"})), Decision::Deny(_)));
        let dev = Policy::load(dir.path(), Some("dev".into())).unwrap();
        assert_eq!(check(&dev, "load_skill", json!({"name": "review"})), Decision::Allow);
        assert_eq!(dev.roles_with_tool("read_file"), vec!["art", "dev"]);
    }

    #[test]
    fn harness_files_are_guarded_even_in_yolo() {
        let p = Policy { root: std::env::current_dir().unwrap(), yolo: true, ..Default::default() };
        for path in ["sessions/1.json", "logs/audit.jsonl"] {
            assert!(matches!(check(&p, "write_file", json!({"path": path})), Decision::Deny(_)), "{path}");
            assert_eq!(check(&p, "read_file", json!({"path": path})), Decision::Allow);
        }
        for path in ["permissions.json", "COMPANY.md", "tools/x/tool.json", "skills/x/SKILL.md"] {
            assert!(matches!(check(&p, "write_file", json!({"path": path})), Decision::Ask(_)), "{path}");
        }
    }

    #[test]
    fn reads_verify_and_trust_settings() {
        let dir = crate::test_support::TestDir::new();
        std::fs::write(dir.path().join("permissions.json"), r#"{"verify":"checks","trust_tool_output":true}"#).unwrap();
        let p = Policy::load(dir.path(), None).unwrap();
        assert_eq!(p.verify.as_deref(), Some("checks"));
        assert!(p.trust_tool_output);
        for bad in [r#"{"verify":""}"#, r#"{"verify":3}"#, r#"{"trust_tool_output":"yes"}"#] {
            std::fs::write(dir.path().join("permissions.json"), bad).unwrap();
            assert!(Policy::load(dir.path(), None).is_err(), "{bad}");
        }
    }
}

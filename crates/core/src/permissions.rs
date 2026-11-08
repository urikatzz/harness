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

use serde_json::Value;
use std::path::{Path, PathBuf};

/// The reason given for a call that only needs a yes because the mode is "ask".
pub const ASK_MODE: &str = "ask mode";

/// Harness files a model-driven write must never change.
const STATE_DIRS: &[&str] = &["sessions/**", "logs/**"];
/// Harness configuration: changing it changes what the model may do, so a human must agree.
const CONFIG_FILES: &[&str] = &["permissions.json", "COMPANY.md", "tools/**", "skills/**"];

#[derive(Debug, PartialEq)]
pub enum Decision {
    Allow,
    /// Needs a human yes; the string says why.
    Ask(String),
    Deny(String),
}

#[derive(Default, Clone)]
pub struct Policy {
    root: PathBuf,
    pub yolo: bool,
    /// `"sandbox": false`: run custom tools and CLI backends without the macOS sandbox.
    pub unsandboxed: bool,
    pub role: Option<String>,
    /// None = no roles configured, so everything is allowed.
    allowed: Option<(Vec<String>, Vec<String>)>,
    deny_files: Vec<String>,
    approve_files: Vec<String>,
    /// Tool to run after file changes, before a final answer is accepted.
    pub verify: Option<String>,
    /// Skip the extra approval for side-effecting calls after tool output entered the chat.
    pub trust_tool_output: bool,
}

fn validate_list(value: &Value) -> Result<(), String> {
    if !value.as_array().is_some_and(|list| list.iter().all(Value::is_string)) {
        return Err("permission lists must contain strings".into());
    }
    Ok(())
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_owned)).collect()
}

impl Policy {
    pub fn load(root: &Path, role: Option<String>) -> Result<Self, String> {
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let path = root.join("permissions.json");
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Policy { root, role, ..Default::default() }),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let v: Value = serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
        if !v.is_object() { return Err("permissions must be an object".into()); }
        if let Some(mode) = v.get("mode") {
            if !matches!(mode.as_str(), Some("ask" | "yolo")) { return Err("invalid permission mode".into()); }
        }
        for key in ["sandbox", "trust_tool_output"] {
            if let Some(flag) = v.get(key) {
                if !flag.is_boolean() { return Err(format!("{key} must be true or false")); }
            }
        }
        if let Some(verify) = v.get("verify") {
            if !verify.as_str().is_some_and(|s| !s.is_empty()) { return Err("verify must be a tool name".into()); }
        }
        if let Some(roles) = v.get("roles") {
            let roles = roles.as_object().ok_or("roles must be an object")?;
            for role in roles.values() {
                if !role.is_object() { return Err("role must be an object".into()); }
                for key in ["tools", "skills"] {
                    if let Some(list) = role.get(key) { validate_list(list)?; }
                }
            }
        }
        if let Some(files) = v.get("files") {
            if !files.is_object() { return Err("files must be an object".into()); }
            for key in ["deny", "approve"] {
                if let Some(list) = files.get(key) { validate_list(list)?; }
            }
        }
        let allowed = v["roles"].as_object().map(|roles| {
            let r = role.as_deref().and_then(|name| roles.get(name)).cloned().unwrap_or(Value::Null);
            (strings(&r["tools"]), strings(&r["skills"]))
        });
        Ok(Policy {
            root,
            yolo: v["mode"].as_str() == Some("yolo"),
            unsandboxed: v["sandbox"] == Value::Bool(false),
            role,
            allowed,
            deny_files: strings(&v["files"]["deny"]),
            approve_files: strings(&v["files"]["approve"]),
            verify: v["verify"].as_str().map(str::to_owned),
            trust_tool_output: v["trust_tool_output"] == Value::Bool(true),
        })
    }

    /// The same rules in ask mode, so every call goes through the approval callback.
    pub fn for_eval(&self) -> Self {
        Policy { yolo: false, ..self.clone() }
    }

    /// No tools or skills at all; used when the rules file can't be read.
    pub fn locked(role: Option<String>) -> Self {
        Policy { role, allowed: Some((vec![], vec![])), ..Default::default() }
    }

    pub fn can_use_tool(&self, name: &str) -> bool {
        self.allowed.as_ref().map_or(true, |(tools, _)| listed(tools, name))
    }

    pub fn can_use_skill(&self, name: &str) -> bool {
        self.allowed.as_ref().map_or(true, |(_, skills)| listed(skills, name))
    }

    pub(crate) fn protected_patterns(&self) -> impl Iterator<Item = &str> {
        self.deny_files.iter().chain(&self.approve_files).map(String::as_str)
    }

    pub(crate) fn root(&self) -> &Path { &self.root }

    /// What should happen to one tool call.
    pub fn check(&self, tool: &str, args: &Value) -> Decision {
        let who = self.role.as_deref().unwrap_or("no role");
        if tool == "load_skill" {
            let skill = args["name"].as_str().unwrap_or("");
            return if self.can_use_skill(skill) { Decision::Allow } else { Decision::Deny(format!("skill `{skill}` is not allowed for {who}")) };
        }
        if !self.can_use_tool(tool) {
            return Decision::Deny(format!("tool `{tool}` is not allowed for {who}"));
        }
        if let Some(path) = args["path"].as_str() {
            let resolved = match crate::paths::resolve(&self.root, path) {
                Ok(path) => path,
                Err(reason) => return Decision::Deny(reason),
            };
            let relative = resolved.strip_prefix(&self.root).unwrap();
            let path = relative.to_string_lossy();
            if self.deny_files.iter().any(|g| glob(g, &path)) {
                return Decision::Deny(format!("`{path}` is a protected file"));
            }
            if tool == "write_file" && STATE_DIRS.iter().any(|g| glob(g, &path)) {
                return Decision::Deny(format!("`{path}` is harness state and cannot be written"));
            }
            if tool == "write_file" && CONFIG_FILES.iter().any(|g| glob(g, &path)) {
                return Decision::Ask(format!("`{path}` is harness configuration and requires manual approval"));
            }
            if self.approve_files.iter().any(|g| glob(g, &path)) {
                return Decision::Ask(format!("`{path}` requires manual approval"));
            }
        }
        if self.yolo { Decision::Allow } else { Decision::Ask(ASK_MODE.into()) }
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
    use serde_json::json;

    #[test]
    fn rejects_malformed_policy_and_normalizes_only_safe_paths() {
        let dir = crate::test_support::TestDir::new();
        for invalid in ["null", r#"{"roles":[]}"#, r#"{"roles":{"dev":{"tools":"*"}}}"#, r#"{"files":{"deny":"secret"}}"#, r#"{"mode":"typo"}"#, r#"{"sandbox":"off"}"#] {
            std::fs::write(dir.path().join("permissions.json"), invalid).unwrap();
            assert!(Policy::load(dir.path(), None).is_err(), "{invalid}");
        }
        std::fs::write(dir.path().join("permissions.json"), r#"{"mode":"yolo","files":{"deny":["secret"]}}"#).unwrap();
        let policy = Policy::load(dir.path(), None).unwrap();
        assert!(matches!(policy.check("read_file", &json!({"path":"././../../outside"})), Decision::Deny(_)));
        assert!(matches!(policy.check("read_file", &json!({"path":"././secret"})), Decision::Deny(_)));
        assert_eq!(policy.check("read_file", &json!({"path":"././public"})), Decision::Allow);
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
            allowed: Some((vec!["read_file".into()], vec![])),
            deny_files: vec!["secrets/**".into()],
            approve_files: vec![".env".into()],
            ..Default::default()
        };
        assert_eq!(p.check("read_file", &json!({"path": "README.md"})), Decision::Allow);
        assert!(matches!(p.check("read_file", &json!({"path": ".env"})), Decision::Ask(_)));
        assert!(matches!(p.check("read_file", &json!({"path": "secrets/k"})), Decision::Deny(_)));
        assert!(matches!(p.check("read_file", &json!({"path": "../x"})), Decision::Deny(_)));
        assert!(matches!(p.check("company_info", &json!({})), Decision::Deny(_)));
    }

    #[test]
    fn harness_files_are_guarded_even_in_yolo() {
        let p = Policy { root: std::env::current_dir().unwrap(), yolo: true, ..Default::default() };
        for path in ["sessions/1.json", "logs/audit.jsonl"] {
            assert!(matches!(p.check("write_file", &json!({"path": path})), Decision::Deny(_)), "{path}");
            assert_eq!(p.check("read_file", &json!({"path": path})), Decision::Allow);
        }
        for path in ["permissions.json", "COMPANY.md", "tools/x/tool.json", "skills/x/SKILL.md"] {
            assert!(matches!(p.check("write_file", &json!({"path": path})), Decision::Ask(_)), "{path}");
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

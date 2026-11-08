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
//!   }
//! }
//!
//! The role comes from `--role` / `URITHEKING_ROLE`. No file = everything allowed, mode "ask".
//! With roles defined, an unknown or missing role gets no tools.

use serde_json::Value;
use std::path::Path;

#[derive(Debug, PartialEq)]
pub enum Decision {
    Allow,
    /// Needs a human yes; the string says why.
    Ask(String),
    Deny(String),
}

#[derive(Default)]
pub struct Policy {
    pub yolo: bool,
    pub role: Option<String>,
    /// None = no roles configured, so everything is allowed.
    allowed: Option<(Vec<String>, Vec<String>)>,
    deny_files: Vec<String>,
    approve_files: Vec<String>,
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_owned)).collect()
}

impl Policy {
    pub fn load(root: &Path, role: Option<String>) -> Result<Self, String> {
        let path = root.join("permissions.json");
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Ok(Policy { role, ..Default::default() });
        };
        let v: Value = serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
        let allowed = v["roles"].as_object().map(|roles| {
            let r = role.as_deref().and_then(|name| roles.get(name)).cloned().unwrap_or(Value::Null);
            (strings(&r["tools"]), strings(&r["skills"]))
        });
        Ok(Policy {
            yolo: v["mode"].as_str() == Some("yolo"),
            role,
            allowed,
            deny_files: strings(&v["files"]["deny"]),
            approve_files: strings(&v["files"]["approve"]),
        })
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
            let path = path.trim_start_matches("./");
            if path.split('/').any(|p| p == "..") || path.starts_with('/') {
                return Decision::Deny(format!("`{path}` is outside the project"));
            }
            if self.deny_files.iter().any(|g| glob(g, path)) {
                return Decision::Deny(format!("`{path}` is a protected file"));
            }
            if self.approve_files.iter().any(|g| glob(g, path)) {
                return Decision::Ask(format!("`{path}` requires manual approval"));
            }
        }
        if self.yolo { Decision::Allow } else { Decision::Ask(String::from("ask mode")) }
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
            yolo: true,
            role: Some("sales".into()),
            allowed: Some((vec!["read_file".into()], vec![])),
            deny_files: vec!["secrets/**".into()],
            approve_files: vec![".env".into()],
        };
        assert_eq!(p.check("read_file", &json!({"path": "README.md"})), Decision::Allow);
        assert!(matches!(p.check("read_file", &json!({"path": ".env"})), Decision::Ask(_)));
        assert!(matches!(p.check("read_file", &json!({"path": "secrets/k"})), Decision::Deny(_)));
        assert!(matches!(p.check("read_file", &json!({"path": "../x"})), Decision::Deny(_)));
        assert!(matches!(p.check("company_info", &json!({})), Decision::Deny(_)));
    }
}

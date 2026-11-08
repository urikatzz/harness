//! Evals: check how a model handles the company's tools and skills.
//!
//! Each `evals/<name>.json` is one case:
//!
//! {
//!   "prompt": "What is our tech stack?",
//!   "role": "dev",                          // optional; defaults to --role
//!   "expect": {
//!     "tools": ["company_info"],            // must be called (attempted counts)
//!     "no_tools": ["write_file"],           // must not be called
//!     "reply_contains": ["Rust"],           // case-insensitive
//!     "reply_not_contains": ["I can't"]
//!   }
//! }
//!
//! Cases never change anything: read-only calls run normally, while calls that could write
//! files or reach the network are recorded and then declined.

use crate::agent::{self, Event};
use crate::permissions::{Policy, ASK_MODE};
use crate::provider::{Message, Provider, Result, Role};
use crate::toolbox::Toolbox;
use serde_json::Value;
use std::path::Path;

pub struct Case {
    pub name: String,
    pub prompt: String,
    pub role: Option<String>,
    tools: Vec<String>,
    no_tools: Vec<String>,
    reply_contains: Vec<String>,
    reply_not_contains: Vec<String>,
}

pub struct Outcome {
    pub name: String,
    /// Why the case failed; empty means it passed.
    pub failures: Vec<String>,
    pub tools_called: Vec<String>,
    pub reply: String,
}

impl Outcome {
    pub fn passed(&self) -> bool { self.failures.is_empty() }
}

fn strings(v: &Value, key: &str) -> Result<Vec<String>> {
    match &v[key] {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) if items.iter().all(Value::is_string) => {
            Ok(items.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect())
        }
        _ => Err(format!("expect.{key} must be a list of strings").into()),
    }
}

fn parse_case(name: String, raw: &str) -> Result<Case> {
    let v: Value = serde_json::from_str(raw)?;
    let prompt = v["prompt"].as_str().filter(|p| !p.trim().is_empty()).ok_or("needs a \"prompt\"")?.to_string();
    let expect = &v["expect"];
    if !expect.is_null() && !expect.is_object() { return Err("\"expect\" must be an object".into()); }
    Ok(Case {
        name,
        prompt,
        role: v["role"].as_str().map(str::to_owned),
        tools: strings(expect, "tools")?,
        no_tools: strings(expect, "no_tools")?,
        reply_contains: strings(expect, "reply_contains")?,
        reply_not_contains: strings(expect, "reply_not_contains")?,
    })
}

/// All cases in `<root>/evals/`, sorted by name, plus any files that could not be read.
pub fn load(root: &Path) -> (Vec<Case>, Vec<String>) {
    let (mut cases, mut errors) = (Vec::new(), Vec::new());
    let mut files: Vec<_> = std::fs::read_dir(root.join("evals")).into_iter().flatten().flatten()
        .map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    files.sort();
    for path in files {
        let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        match std::fs::read_to_string(&path).map_err(Into::into).and_then(|raw| parse_case(name.clone(), &raw)) {
            Ok(case) => cases.push(case),
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    (cases, errors)
}

/// Runs one case in a fresh conversation.
pub fn run(provider: &dyn Provider, tb: &Toolbox, policy: &Policy, case: &Case) -> Outcome {
    let mut history = Vec::new();
    let system = agent::system_prompt(tb, policy);
    if !system.is_empty() { history.push(Message { role: Role::System, content: system }); }
    history.push(Message { role: Role::User, content: case.prompt.clone() });
    let mut tools_called = Vec::new();
    // Ask about every call; say yes only to plain read-only ones.
    let eval_policy = policy.for_eval();
    let result = agent::run_turn(provider, tb, &eval_policy, &mut history,
        |ev| if let Event::ToolCall { name, .. } = ev { tools_called.push(name.to_string()) },
        |name, _, why| why == ASK_MODE && !tb.has_side_effects(name));
    let mut failures = Vec::new();
    let reply = match result {
        Ok(reply) => reply,
        Err(e) => { failures.push(format!("turn failed: {e}")); String::new() }
    };
    for tool in &case.tools {
        if !tools_called.contains(tool) { failures.push(format!("expected a call to `{tool}`")); }
    }
    for tool in &case.no_tools {
        if tools_called.contains(tool) { failures.push(format!("`{tool}` should not have been called")); }
    }
    let lower = reply.to_lowercase();
    for text in &case.reply_contains {
        if !lower.contains(&text.to_lowercase()) { failures.push(format!("reply is missing {text:?}")); }
    }
    for text in &case.reply_not_contains {
        if lower.contains(&text.to_lowercase()) { failures.push(format!("reply should not contain {text:?}")); }
    }
    Outcome { name: case.name.clone(), failures, tools_called, reply }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Script(RefCell<Vec<&'static str>>);
    impl Provider for Script {
        fn complete(&self, _: &[Message]) -> Result<String> { Ok(self.0.borrow_mut().remove(0).into()) }
    }

    #[test]
    fn checks_tools_and_reply_without_changing_anything() {
        let dir = crate::test_support::TestDir::new();
        std::fs::create_dir(dir.path().join("evals")).unwrap();
        std::fs::write(dir.path().join("permissions.json"), r#"{"mode":"yolo"}"#).unwrap();
        std::fs::write(dir.path().join("facts.txt"), "We use Rust.").unwrap();
        std::fs::write(dir.path().join("evals/stack.json"),
            r#"{"prompt":"stack?","expect":{"tools":["read_file"],"no_tools":["write_file"],"reply_contains":["rust"]}}"#).unwrap();
        std::fs::write(dir.path().join("evals/broken.json"), r#"{"expect":{}}"#).unwrap();
        let (cases, errors) = load(dir.path());
        assert_eq!(cases.len(), 1);
        assert_eq!(errors.len(), 1);
        let (tb, policy) = (Toolbox::load(dir.path()), Policy::load(dir.path(), None).unwrap());

        let good = Script(RefCell::new(vec![r#"<tool_call>{"name":"read_file","args":{"path":"facts.txt"}}</tool_call>"#, "It's Rust."]));
        let outcome = run(&good, &tb, &policy, &cases[0]);
        assert!(outcome.passed(), "{:?}", outcome.failures);

        let bad = Script(RefCell::new(vec![r#"<tool_call>{"name":"write_file","args":{"path":"x.txt","content":"x"}}</tool_call>"#, "Python"]));
        let outcome = run(&bad, &tb, &policy, &cases[0]);
        assert_eq!(outcome.failures.len(), 3, "{:?}", outcome.failures);
        assert!(!dir.path().join("x.txt").exists(), "evals must not write, even in yolo");
    }
}

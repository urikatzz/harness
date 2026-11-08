//! The tool loop: lets any backend use company tools and skills through a plain-text protocol,
//! so it works the same for API backends and for CLIs like `claude -p`.

use crate::provider::{Message, Provider, Result, Role};
use crate::permissions::{Decision, Policy};
use crate::toolbox::{Toolbox, BUILTINS};
use serde_json::Value;

const OPEN: &str = "<tool_call>";
const CLOSE: &str = "</tool_call>";
/// Safety stop so a confused model can't loop forever.
const MAX_STEPS: usize = 10;

/// Something the UI may want to show while a turn runs.
pub enum Event<'a> {
    ToolCall { name: &'a str, args: &'a Value },
    ToolResult { name: &'a str, output: &'a str, ok: bool },
    Denied { name: &'a str, reason: &'a str },
}

/// Instructions describing the available tools and skills (empty if there are none).
pub fn system_prompt(tb: &Toolbox, policy: &Policy) -> String {
    let mut s = String::from("You are uriTheKing, the company's assistant.\n");
    if let Some(company) = &tb.company {
        s.push_str(&format!("\n<company>\n{}\n</company>\n\n", company.trim()));
    }
    s.push_str(
        "You have company tools and skills.\n\
         To use a tool, reply with ONLY this and nothing else, then wait for the result:\n\
         <tool_call>{\"name\": \"<tool>\", \"args\": {...}}</tool_call>\n\
         The result comes back in a <tool_result> message. Call tools as many times as needed, \
         then answer the user normally (no tool_call tag).\n",
    );
    s.push_str("\nTools:\n");
    for (name, desc, params) in BUILTINS.iter().filter(|b| policy.can_use_tool(b.0)) {
        s.push_str(&format!("- {name}: {desc} args: {params}\n"));
    }
    {
        for t in tb.tools.iter().filter(|t| policy.can_use_tool(&t.name)) {
            s.push_str(&format!("- {}: {}\n", t.name, t.description));
            for (arg, desc) in &t.params {
                s.push_str(&format!("    {arg}: {}\n", desc.as_str().unwrap_or("")));
            }
        }
    }
    let skills: Vec<_> = tb.skills.iter().filter(|sk| policy.can_use_skill(&sk.name)).collect();
    if !skills.is_empty() {
        s.push_str(
            "\nSkills are playbooks for specific kinds of tasks. When one fits the request, \
             first call the tool `load_skill` with {\"name\": \"<skill>\"} and follow what it says.\n",
        );
        for sk in skills {
            s.push_str(&format!("- {}: {}\n", sk.name, sk.description));
        }
    }
    s
}

/// Pulls the first `{"name", "args"}` call out of a reply, if there is one.
fn parse_call(reply: &str) -> Option<(String, Value)> {
    let start = reply.find(OPEN)? + OPEN.len();
    let end = start + reply[start..].find(CLOSE)?;
    let v: Value = serde_json::from_str(reply[start..end].trim()).ok()?;
    Some((v["name"].as_str()?.to_string(), v.get("args").cloned().unwrap_or(Value::Null)))
}

/// Runs one user turn to completion: model → tool → model … → final answer.
/// `history` already ends with the user's message and gets every step appended.
pub fn run_turn(
    provider: &dyn Provider,
    tb: &Toolbox,
    policy: &Policy,
    history: &mut Vec<Message>,
    mut on_event: impl FnMut(Event),
    // Asked when a call needs a human yes: (tool, args, reason) -> approved?
    mut approve: impl FnMut(&str, &Value, &str) -> bool,
) -> Result<String> {
    for _ in 0..MAX_STEPS {
        let reply = provider.complete(history)?;
        let Some((name, args)) = parse_call(&reply) else {
            history.push(Message { role: Role::Assistant, content: reply.clone() });
            return Ok(reply);
        };
        history.push(Message { role: Role::Assistant, content: reply });
        on_event(Event::ToolCall { name: &name, args: &args });
        let refusal = match policy.check(&name, &args) {
            Decision::Allow => None,
            Decision::Ask(why) if approve(&name, &args, &why) => None,
            Decision::Ask(_) => Some("the user declined this call".to_string()),
            Decision::Deny(why) => Some(why),
        };
        let output = match refusal {
            Some(why) => {
                on_event(Event::Denied { name: &name, reason: &why });
                format!("denied: {why}. Do not retry it; tell the user or try something else.")
            }
            None => {
                let (output, ok) = match tb.call(&name, &args) {
                    Ok(out) => (out, true),
                    Err(e) => (format!("error: {e}"), false),
                };
                on_event(Event::ToolResult { name: &name, output: &output, ok });
                output
            }
        };
        history.push(Message { role: Role::User, content: format!("<tool_result name=\"{name}\">\n{output}\n</tool_result>") });
    }
    Err(format!("stopped after {MAX_STEPS} tool steps without a final answer").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_call() {
        let (n, a) = parse_call("ok <tool_call>{\"name\":\"x\",\"args\":{\"q\":1}}</tool_call>").unwrap();
        assert_eq!(n, "x");
        assert_eq!(a["q"], 1);
        assert!(parse_call("plain answer").is_none());
    }
}

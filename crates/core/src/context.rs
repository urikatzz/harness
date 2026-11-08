//! Keeps history inside the model's context window by summarizing older messages.

use crate::provider::{Message, Provider, Result, Role};

/// Rough token budget before old messages get summarized (`URITHEKING_CONTEXT_TOKENS`).
const DEFAULT_BUDGET: usize = 60_000;
/// Recent messages that are always kept word for word.
const KEEP_RECENT: usize = 6;

/// About 4 characters per token; good enough to decide when to compact.
pub fn estimate_tokens(history: &[Message]) -> usize {
    history.iter().map(|m| m.content.len() / 4 + 4).sum()
}

pub fn budget() -> usize {
    std::env::var("URITHEKING_CONTEXT_TOKENS").ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_BUDGET)
}

/// Summarizes if history is over budget. Returns true if it compacted.
pub fn compact_if_needed(provider: &dyn Provider, history: &mut Vec<Message>) -> Result<bool> {
    if estimate_tokens(history) <= budget() {
        return Ok(false);
    }
    compact(provider, history)
}

/// Replaces everything between the system prompt and the last few messages with one summary.
/// Returns false when there is too little to summarize.
pub fn compact(provider: &dyn Provider, history: &mut Vec<Message>) -> Result<bool> {
    let start = history.iter().take_while(|m| m.role == Role::System).count();
    // Cut at a plain user message so a tool call and its result stay together.
    let mut cut = history.len().saturating_sub(KEEP_RECENT).max(start);
    while cut > start && !is_user_turn(&history[cut]) {
        cut -= 1;
    }
    if cut <= start + 1 {
        return Ok(false);
    }

    let transcript: String = history[start..cut]
        .iter()
        .map(|m| format!("[{}]\n{}\n\n", m.role.as_str(), m.content))
        .collect();
    let request = vec![
        Message {
            role: Role::System,
            content: "You summarize conversations so they can continue later without the full text.".into(),
        },
        Message {
            role: Role::User,
            content: format!(
                "Summarize this conversation between a user and an assistant that uses tools. Keep: \
                 the user's goals and preferences, decisions made, facts and results learned from tools \
                 (file paths, names, numbers), work done, and what is still open. Be concise. \
                 Reply with the summary only.\n\n<conversation>\n{transcript}</conversation>"
            ),
        },
    ];
    let summary = provider.complete(&request)?;

    let replacement = [
        Message {
            role: Role::User,
            content: format!("<summary>\nSummary of the earlier conversation:\n{}\n</summary>", summary.trim()),
        },
        Message { role: Role::Assistant, content: "Understood, I'll continue from that summary.".into() },
    ];
    history.splice(start..cut, replacement);
    Ok(true)
}

fn is_user_turn(m: &Message) -> bool {
    m.role == Role::User && !m.content.starts_with("<tool_result")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;
    impl Provider for Fake {
        fn complete(&self, _: &[Message]) -> Result<String> {
            Ok("they said hi".into())
        }
    }

    fn msg(role: Role, c: &str) -> Message {
        Message { role, content: c.into() }
    }

    #[test]
    fn keeps_system_and_recent_turns() {
        let mut h = vec![msg(Role::System, "sys")];
        for i in 0..6 {
            h.push(msg(Role::User, &format!("q{i}")));
            h.push(msg(Role::Assistant, &format!("a{i}")));
        }
        assert!(compact(&Fake, &mut h).unwrap());
        assert_eq!(h[0].content, "sys");
        assert!(h[1].content.contains("they said hi"));
        assert_eq!(h.last().unwrap().content, "a5");
        assert_eq!(h.len(), 1 + 2 + KEEP_RECENT);
    }

    #[test]
    fn never_cuts_at_a_tool_result() {
        let mut h = vec![msg(Role::System, "sys"), msg(Role::User, "q0"), msg(Role::Assistant, "a0"), msg(Role::User, "q1")];
        for _ in 0..4 {
            h.push(msg(Role::Assistant, "<tool_call>{}</tool_call>"));
            h.push(msg(Role::User, "<tool_result name=\"x\">ok</tool_result>"));
        }
        assert!(compact(&Fake, &mut h).unwrap());
        assert_eq!(h[3].content, "q1"); // kept turn starts at a real user message
    }
}

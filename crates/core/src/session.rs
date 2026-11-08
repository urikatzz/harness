//! Saved conversations: one JSON file per session under `<home>/sessions/`.
//! The system prompt is not saved; it is rebuilt on resume so tool changes apply.

use crate::provider::{Message, Result, Role};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Session {
    pub id: String,
    dir: PathBuf,
}

/// A saved session as shown in the resume list.
pub struct Summary {
    pub id: String,
    pub preview: String,
    pub messages: usize,
}

impl Session {
    /// A new, empty session. Nothing is written until the first `save`.
    pub fn new(home: &Path) -> Self {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        Self { id: secs.to_string(), dir: home.join("sessions") }
    }

    pub fn open(home: &Path, id: &str) -> Self {
        Self { id: id.to_string(), dir: home.join("sessions") }
    }

    fn path(&self) -> PathBuf {
        self.dir.join(format!("{}.json", self.id))
    }

    /// Writes the whole history (minus system messages). Writes to a temp file first
    /// so a crash mid-write never leaves a broken session.
    pub fn save(&self, history: &[Message]) -> Result<()> {
        let msgs: Vec<Value> = history
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| json!({"role": m.role.as_str(), "content": m.content}))
            .collect();
        if msgs.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.path().with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&json!({"messages": msgs}))?)?;
        std::fs::rename(tmp, self.path())?;
        Ok(())
    }

    pub fn load(&self) -> Result<Vec<Message>> {
        read_messages(&self.path())
    }
}

fn read_messages(path: &Path) -> Result<Vec<Message>> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let msgs = v["messages"].as_array().ok_or("session file has no messages")?;
    msgs.iter()
        .map(|m| {
            let role = match m["role"].as_str() {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                other => return Err(format!("bad role {other:?} in session").into()),
            };
            Ok(Message { role, content: m["content"].as_str().unwrap_or("").to_string() })
        })
        .collect()
}

/// Saved sessions, newest first.
pub fn list(home: &Path) -> Vec<Summary> {
    let Ok(entries) = std::fs::read_dir(home.join("sessions")) else { return Vec::new() };
    let mut out: Vec<Summary> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension()? != "json" {
                return None;
            }
            let id = path.file_stem()?.to_string_lossy().to_string();
            let msgs = read_messages(&path).ok()?;
            let preview = msgs
                .iter()
                .find(|m| m.role == Role::User && !m.content.starts_with('<'))
                .map(|m| m.content.lines().next().unwrap_or("").chars().take(60).collect())
                .unwrap_or_default();
            Some(Summary { id, preview, messages: msgs.len() })
        })
        .collect();
    // Ids are millisecond timestamps, so numeric order is age order.
    out.sort_by_key(|s| std::cmp::Reverse(s.id.parse::<u128>().unwrap_or(0)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load_round_trip() {
        let home = std::env::temp_dir().join(format!("uritheking-test-{}", std::process::id()));
        let s = Session::open(&home, "42");
        let history = vec![
            Message { role: Role::System, content: "sys".into() },
            Message { role: Role::User, content: "hi".into() },
            Message { role: Role::Assistant, content: "hello".into() },
        ];
        s.save(&history).unwrap();
        let back = s.load().unwrap();
        assert_eq!(back.len(), 2); // system prompt is not saved
        assert_eq!(back[1].content, "hello");
        assert_eq!(list(&home)[0].preview, "hi");
        std::fs::remove_dir_all(home).ok();
    }
}

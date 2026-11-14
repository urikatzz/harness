//! Saved conversations: one JSON file per session under `<home>/sessions/`.
//! The system prompt is not saved; it is rebuilt on resume so tool changes apply.

use crate::error::Result;
use crate::provider::{Kind, Message, Role};
use serde::{Deserialize, Serialize};
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

/// One message as stored on disk.
#[derive(Serialize, Deserialize)]
struct Stored {
    role: Role,
    content: String,
    /// Missing in files saved before message kinds existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<Kind>,
}

#[derive(Serialize, Deserialize)]
struct File {
    messages: Vec<Stored>,
}

impl Stored {
    fn into_message(self) -> Result<Message> {
        if self.role == Role::System {
            return Err("bad role \"system\" in session".into());
        }
        let kind = self.kind.unwrap_or_else(|| legacy_kind(self.role, &self.content));
        Ok(Message { role: self.role, content: self.content, kind })
    }
}

/// Older files only have the text, so the kind is read from the harness's own wrappers.
fn legacy_kind(role: Role, content: &str) -> Kind {
    match role {
        Role::User if content.starts_with("<tool_result name=\"load_skill\"") || content.starts_with("<tool_result name=\"invalid_call\"") => {
            Kind::ToolResult { trusted: true }
        }
        Role::User if content.starts_with("<tool_result") => Kind::ToolResult { trusted: false },
        Role::User if content.starts_with("<summary") => Kind::Summary,
        _ => Kind::Chat,
    }
}

impl Session {
    /// A new, empty session. Nothing is written until the first `save`.
    pub fn new(home: &Path) -> Self {
        let millis = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        Self { id: millis.to_string(), dir: home.join("sessions") }
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
        let messages: Vec<Stored> = history
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| Stored { role: m.role, content: m.content.clone(), kind: Some(m.kind) })
            .collect();
        if messages.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.path().with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&File { messages })?)?;
        std::fs::rename(tmp, self.path())?;
        Ok(())
    }

    pub fn load(&self) -> Result<Vec<Message>> {
        read_messages(&self.path())
    }
}

fn read_messages(path: &Path) -> Result<Vec<Message>> {
    let file: File = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    file.messages.into_iter().map(Stored::into_message).collect()
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
                .find(|m| m.is_user_chat())
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
        let home = std::env::temp_dir().join(format!("umango-test-{}", std::process::id()));
        let s = Session::open(&home, "42");
        let history = vec![
            Message::system("sys"),
            Message::user("hi"),
            Message::assistant("hello"),
            Message::tool_result("<tool_result name=\"x\">data</tool_result>", false),
        ];
        s.save(&history).unwrap();
        let back = s.load().unwrap();
        assert_eq!(back.len(), 3); // system prompt is not saved
        assert_eq!(back[1].content, "hello");
        assert!(back[2].is_untrusted());
        assert_eq!(list(&home)[0].preview, "hi");
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn old_files_without_kinds_keep_tool_output_untrusted() {
        let home = crate::test_support::TestDir::new();
        std::fs::create_dir(home.path().join("sessions")).unwrap();
        std::fs::write(home.path().join("sessions/1.json"), r#"{"messages":[
            {"role":"user","content":"hi"},
            {"role":"user","content":"<tool_result name=\"load_skill\">x</tool_result>"},
            {"role":"user","content":"<tool_result name=\"read_file\">x</tool_result>"},
            {"role":"user","content":"<summary>x</summary>"}]}"#).unwrap();
        let msgs = Session::open(home.path(), "1").load().unwrap();
        let untrusted: Vec<bool> = msgs.iter().map(Message::is_untrusted).collect();
        assert_eq!(untrusted, vec![false, false, true, true]);
        std::fs::write(home.path().join("sessions/2.json"), r#"{"messages":[{"role":"system","content":"x"}]}"#).unwrap();
        assert!(Session::open(home.path(), "2").load().is_err());
    }
}

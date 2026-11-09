pub use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

/// What a message is, so the harness never has to guess from its text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Something a person or the model wrote.
    #[default]
    Chat,
    /// Output of a tool call. `trusted` is true only for harness-written text (skills, errors).
    ToolResult { trusted: bool },
    /// A summary of older messages; it may repeat tool output, so it is not trusted.
    Summary,
}

#[derive(Clone, Debug)]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub kind: Kind,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Message { role: Role::System, content: content.into(), kind: Kind::Chat }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Message { role: Role::User, content: content.into(), kind: Kind::Chat }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Message { role: Role::Assistant, content: content.into(), kind: Kind::Chat }
    }

    pub fn tool_result(content: impl Into<String>, trusted: bool) -> Self {
        Message { role: Role::User, content: content.into(), kind: Kind::ToolResult { trusted } }
    }

    pub fn summary(content: impl Into<String>) -> Self {
        Message { role: Role::User, content: content.into(), kind: Kind::Summary }
    }

    /// Text from files or programs that could carry injected instructions.
    pub fn is_untrusted(&self) -> bool {
        matches!(self.kind, Kind::ToolResult { trusted: false } | Kind::Summary)
    }

    /// A message the user typed (not a tool result or summary).
    pub fn is_user_chat(&self) -> bool {
        self.role == Role::User && self.kind == Kind::Chat
    }
}

/// Anything that can turn a conversation into the next assistant reply.
pub trait Provider {
    fn complete(&self, history: &[Message]) -> Result<String>;

    /// Like `complete`, plus the model's reasoning when the backend exposes it.
    fn complete_with_reasoning(&self, history: &[Message]) -> Result<Reply> {
        Ok(Reply { text: self.complete(history)?, reasoning: None })
    }
}

/// One assistant reply and, for thinking models, the reasoning behind it.
pub struct Reply {
    pub text: String,
    pub reasoning: Option<String>,
}

/// Which backend, model and reasoning effort to use. `None` means "backend default".
#[derive(Clone, Default)]
pub struct Selection {
    pub backend: String,
    pub model: Option<String>,
    pub effort: Option<String>,
}

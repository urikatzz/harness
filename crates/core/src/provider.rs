use std::error::Error;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Copy, PartialEq, Eq)]
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

pub struct Message {
    pub role: Role,
    pub content: String,
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

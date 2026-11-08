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
}

/// Which backend, model and reasoning effort to use. `None` means "backend default".
#[derive(Clone, Default)]
pub struct Selection {
    pub backend: String,
    pub model: Option<String>,
    pub effort: Option<String>,
}

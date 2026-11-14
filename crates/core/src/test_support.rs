use std::path::{Path, PathBuf};

pub struct TestDir(PathBuf);
impl TestDir {
    pub fn new() -> Self {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("umango-{}-{id}-{n}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

use crate::error::{Error, Result};
use crate::provider::{Message, Provider, Reply, Usage};
use std::cell::RefCell;
use std::collections::VecDeque;

/// A provider that plays back replies and errors in order, and counts the requests it got.
pub struct Scripted {
    steps: RefCell<VecDeque<Result<Reply>>>,
    pub requests: std::cell::Cell<usize>,
}
impl Scripted {
    pub fn new(steps: impl IntoIterator<Item = Result<Reply>>) -> Self {
        Self { steps: RefCell::new(steps.into_iter().collect()), requests: Default::default() }
    }
}
impl Provider for Scripted {
    fn complete(&self, history: &[Message]) -> Result<String> {
        self.complete_with_reasoning(history).map(|r| r.text)
    }
    fn complete_with_reasoning(&self, _: &[Message]) -> Result<Reply> {
        self.requests.set(self.requests.get() + 1);
        self.steps.borrow_mut().pop_front().expect("the script ran out of replies")
    }
}

pub fn reply(text: &str) -> Result<Reply> {
    Ok(Reply { text: text.into(), reasoning: None, usage: None })
}

pub fn reply_with_usage(text: &str, input_tokens: u64, output_tokens: u64) -> Result<Reply> {
    Ok(Reply { text: text.into(), reasoning: None, usage: Some(Usage { input_tokens, output_tokens }) })
}

/// An HTTP failure; `None` means the server could not be reached.
pub fn http_error(status: Option<u16>, message: &str) -> Result<Reply> {
    Err(Error::Http { status, retry_after: None, message: message.into() })
}

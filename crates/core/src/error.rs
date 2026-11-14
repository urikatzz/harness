//! The one error type for the core library.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Json(serde_json::Error),
    /// A harness file (`permissions.json`, `tool.json`, an eval case) is invalid.
    Config(String),
    /// A model server answered with an error (`status`) or could not be reached (`status: None`).
    /// `retry_after` is the server's `Retry-After` in seconds, if it sent one.
    Http { status: Option<u16>, retry_after: Option<u64>, message: String },
    /// The user pressed Ctrl-C while waiting.
    Cancelled,
    /// Anything else, described in words.
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn config(message: impl Into<String>) -> Self {
        Error::Config(message.into())
    }

    /// Whether trying the same request again may work: the server was unreachable or the
    /// connection broke, or it answered with a status that means "busy" or "try later".
    pub fn is_transient(&self) -> bool {
        match self {
            Error::Http { status: None, .. } => true,
            Error::Http { status: Some(code), .. } => matches!(code, 408 | 429 | 500 | 502 | 503 | 504),
            _ => false,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Json(e) => write!(f, "{e}"),
            Error::Config(m) | Error::Http { message: m, .. } | Error::Other(m) => f.write_str(m),
            Error::Cancelled => f.write_str("cancelled with ctrl+c"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            Error::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self { Error::Io(e) }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self { Error::Json(e) }
}

impl From<ureq::Error> for Error {
    fn from(e: ureq::Error) -> Self {
        use ureq::ErrorKind::{ConnectionFailed, Dns, Io, ProxyConnect};
        match e {
            ureq::Error::Status(code, ref resp) => {
                let retry_after = resp.header("retry-after").and_then(crate::retry::retry_after_secs);
                Error::Http { status: Some(code), retry_after, message: e.to_string() }
            }
            ureq::Error::Transport(ref t) if matches!(t.kind(), Dns | ConnectionFailed | Io | ProxyConnect) => {
                Error::Http { status: None, retry_after: None, message: e.to_string() }
            }
            // A bad URL, header or redirect loop fails the same way every time.
            ureq::Error::Transport(_) => Error::Other(e.to_string()),
        }
    }
}

impl From<String> for Error {
    fn from(m: String) -> Self { Error::Other(m) }
}

impl From<&str> for Error {
    fn from(m: &str) -> Self { Error::Other(m.to_string()) }
}

macro_rules! other_from {
    ($($t:ty),*) => {$(
        impl From<$t> for Error {
            fn from(e: $t) -> Self { Error::Other(e.to_string()) }
        }
    )*};
}
other_from!(std::path::StripPrefixError, std::ffi::NulError, std::time::SystemTimeError, std::env::VarError);

#[cfg(test)]
mod tests {
    use super::*;

    fn http(status: Option<u16>) -> Error {
        Error::Http { status, retry_after: None, message: "HTTP x: body".into() }
    }

    #[test]
    fn only_unreachable_servers_and_temporary_statuses_are_transient() {
        assert!(http(None).is_transient());
        for code in [408, 429, 500, 502, 503, 504] { assert!(http(Some(code)).is_transient(), "{code}"); }
        for code in [400, 401, 403, 404, 422, 501] { assert!(!http(Some(code)).is_transient(), "{code}"); }
        assert!(!Error::Cancelled.is_transient());
        assert!(!Error::Other("claude exited 1".into()).is_transient());
        assert!(!Error::Io(std::io::Error::other("x")).is_transient());
        assert!(!Error::Json(serde_json::from_str::<u8>("x").unwrap_err()).is_transient());
        assert_eq!(http(Some(429)).to_string(), "HTTP x: body");
    }
}

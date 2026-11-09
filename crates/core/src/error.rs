//! The one error type for the core library.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Json(serde_json::Error),
    /// A harness file (`permissions.json`, `tool.json`, an eval case) is invalid.
    Config(String),
    /// A model server answered with an error or could not be reached.
    Http(String),
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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Json(e) => write!(f, "{e}"),
            Error::Config(m) | Error::Http(m) | Error::Other(m) => f.write_str(m),
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
    fn from(e: ureq::Error) -> Self { Error::Http(e.to_string()) }
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

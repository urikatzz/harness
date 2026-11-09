//! Append-only record of every tool call: `<data>/logs/audit.jsonl`, one JSON object per line.
//!
//! Each call writes a `decision` line before anything runs (allowed, approved, declined, denied,
//! invalid) and, if it ran, a `result` line. If the decision can't be recorded, the call is refused.

use crate::error::Result;
use serde_json::{json, Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Longer string arguments (like `write_file` content) are cut to this many characters.
const MAX_ARG_CHARS: usize = 200;

pub fn path(root: &Path) -> PathBuf {
    root.join("logs").join("audit.jsonl")
}

/// Appends one entry, adding the time. Does nothing when there is no harness home (tests,
/// locked policy).
pub fn record(root: &Path, mut entry: Map<String, Value>) -> Result<()> {
    if root.as_os_str().is_empty() {
        return Ok(());
    }
    let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as u64;
    entry.insert("ts".into(), json!(millis));
    std::fs::create_dir_all(root)?;
    let dir = root.join("logs");
    match std::fs::symlink_metadata(&dir) {
        Ok(m) if !m.is_dir() || m.file_type().is_symlink() => return Err("logs/ must be a real folder".into()),
        Ok(_) => {}
        Err(_) => std::fs::create_dir(&dir)?,
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path(root))?;
    let mut line = serde_json::to_string(&Value::Object(entry))?;
    line.push('\n');
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// Arguments as logged: long strings are cut so file contents don't fill the log.
pub fn redact(args: &Value) -> Value {
    match args {
        Value::String(s) if s.chars().count() > MAX_ARG_CHARS => {
            let head: String = s.chars().take(MAX_ARG_CHARS).collect();
            json!(format!("{head}… ({} bytes)", s.len()))
        }
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), redact(v))).collect()),
        Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_lines_and_cuts_long_args() {
        let dir = crate::test_support::TestDir::new();
        for n in 0..2 {
            let mut entry = Map::new();
            entry.insert("n".into(), json!(n));
            entry.insert("args".into(), redact(&json!({"content": "x".repeat(500)})));
            record(dir.path(), entry).unwrap();
        }
        let text = std::fs::read_to_string(path(dir.path())).unwrap();
        let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1]["n"], 1);
        assert!(lines[0]["ts"].is_u64());
        assert!(lines[0]["args"]["content"].as_str().unwrap().ends_with("(500 bytes)"));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_log() {
        let dir = crate::test_support::TestDir::new();
        let elsewhere = crate::test_support::TestDir::new();
        std::fs::create_dir(dir.path().join("logs")).unwrap();
        std::os::unix::fs::symlink(elsewhere.path().join("x"), path(dir.path())).unwrap();
        assert!(record(dir.path(), Map::new()).is_err());
        std::fs::remove_file(path(dir.path())).unwrap();
        std::fs::remove_dir(dir.path().join("logs")).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), dir.path().join("logs")).unwrap();
        assert!(record(dir.path(), Map::new()).is_err());
    }
}

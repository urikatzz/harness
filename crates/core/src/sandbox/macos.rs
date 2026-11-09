//! macOS: a generated `sandbox-exec` profile.

use super::{sandbox_program, Scratch, ToolAccess, HIDDEN};
use crate::error::Result;
use crate::permissions::Policy;
use std::path::Path;
use std::process::Command;

/// A quoted SBPL string. Characters that would need escaping are refused instead of escaped.
pub(super) fn sbpl_string(s: &str) -> Result<String> {
    if s.chars().any(|c| c == '"' || c == '\\' || c.is_control()) {
        return Err(format!("path cannot be used in the sandbox profile: {s:?}").into());
    }
    Ok(format!("\"{s}\""))
}

pub(super) fn tool_command(
    program: &str,
    args: &[String],
    access: &ToolAccess,
    dir: &Path,
    scratch: &Scratch,
    policy: &Policy,
) -> Result<Command> {
    let mut command = sandbox_program()?;
    let mut profile = format!(
        "(version 1)(allow default)(deny file-read* file-write*)(allow file-read-metadata)(allow file-read* (subpath \"/System\") (subpath \"/usr\") (subpath \"/bin\") (subpath \"/sbin\") (subpath \"/Library/Apple\") (subpath \"/private/var/db/dyld\") (literal \"/dev/null\") (literal \"/\") (subpath {}))(allow file-write* (literal \"/dev/null\"))",
        sbpl_string(&dir.to_string_lossy())?
    );
    for readable in access.reads {
        let quoted = sbpl_string(&readable.to_string_lossy())?;
        profile.push_str(&format!("(allow file-read* (subpath {quoted}))"));
    }
    for writable in access.writes.iter().map(|p| p.as_path()).chain([scratch.path()]) {
        let quoted = sbpl_string(&writable.to_string_lossy())?;
        profile.push_str(&format!("(allow file-read* file-write* (subpath {quoted}))"));
    }
    // Later rules win, so protected paths are denied even inside readable and writable folders.
    let root = policy.root();
    for hidden in HIDDEN {
        let quoted = sbpl_string(&root.join(hidden).to_string_lossy())?;
        profile.push_str(&format!("(deny file-read* file-write* (subpath {quoted}))"));
    }
    let root = regex_escape(&root.to_string_lossy());
    for pattern in policy.protected_patterns() {
        let regex = format!("^{root}/{}$", glob_regex(pattern));
        if regex.contains('"') || regex.chars().any(char::is_control) {
            return Err(format!("protected pattern cannot be used in the sandbox profile: {pattern:?}").into());
        }
        profile.push_str(&format!("(deny file-read* file-write* (regex #\"{regex}\"))"));
    }
    if !access.network {
        profile.push_str("(deny network*)");
    }
    command.args(["-p", &profile, program]).args(args);
    Ok(command)
}

fn regex_escape(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ".+*?()[]{}^$|\\".contains(ch) { out.push('\\'); }
        out.push(ch);
    }
    out
}

/// Same matching rules as `permissions::glob`: `*` stays in one segment, `**` crosses them.
fn glob_regex(pattern: &str) -> String {
    let mut regex = String::new();
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                if chars.peek() == Some(&'/') {
                    chars.next();
                    regex.push_str("(.*/)?");
                } else {
                    regex.push_str(".*");
                }
            }
            '*' => regex.push_str("[^/]*"),
            _ => regex.push_str(&regex_escape(&ch.to_string())),
        }
    }
    regex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_quoting_refuses_unsafe_text() {
        assert!(sbpl_string("/a/b c+d").is_ok());
        for bad in ["/a\"b", "/a\\b", "/a\nb"] { assert!(sbpl_string(bad).is_err(), "{bad:?}"); }
        assert_eq!(glob_regex("**/*.pem"), r"(.*/)?[^/]*\.pem");
        assert_eq!(glob_regex("secrets/**"), "secrets/.*");
        assert_eq!(regex_escape("/x+y (1).d"), r"/x\+y \(1\)\.d");
    }
}

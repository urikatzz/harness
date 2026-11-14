use crate::error::Result;
use crate::provider::{Message, Provider, Role};
use std::path::PathBuf;

/// Delegates to an installed agent CLI (`claude`, `codex`), which uses its own login.
pub struct CliProvider {
    program: &'static str,
    args: Vec<String>,
    /// The project, which the sandbox hides from the CLI.
    root: PathBuf,
}

impl CliProvider {
    pub fn claude(model: Option<String>, effort: Option<String>, root: PathBuf) -> Self {
        // `--tools ""` turns off Claude Code's own tools, so file access only goes through
        // the harness tools and their permission checks; `--strict-mcp-config` drops the
        // user's MCP servers for the same reason.
        let mut args = vec!["-p".to_string(), "--tools=".into(), "--strict-mcp-config".into()];
        if let Some(m) = model {
            args.extend(["--model".into(), m]);
        }
        if let Some(e) = effort {
            args.extend(["--effort".into(), e]);
        }
        Self { program: "claude", args, root }
    }

    pub fn codex(model: Option<String>, effort: Option<String>, root: PathBuf) -> Self {
        // Read-only sandbox: we only want text back, not file edits.
        let mut args: Vec<String> = ["exec", "-s", "read-only", "--skip-git-repo-check", "--ephemeral"]
            .map(String::from)
            .into();
        if let Some(m) = model {
            args.extend(["-m".into(), m]);
        }
        if let Some(e) = effort {
            args.extend(["-c".into(), format!("model_reasoning_effort=\"{e}\"")]);
        }
        Self { program: "codex", args, root }
    }
}

/// CLIs take one prompt, so multi-turn history is flattened into a transcript.
fn flatten(history: &[Message]) -> String {
    let (system, chat): (Vec<&Message>, Vec<&Message>) = history.iter().partition(|m| m.role == Role::System);
    let mut out: String = system.iter().map(|m| format!("{}\n\n", m.content)).collect();
    if let [only] = chat.as_slice() {
        return out + &only.content;
    }
    out.push_str("Continue this conversation. Reply only as the assistant.\n\n");
    for m in chat {
        let who = if m.role == Role::User { "User" } else { "Assistant" };
        out.push_str(&format!("{who}: {}\n\n", m.content));
    }
    out
}

/// Builds the error for a failed CLI run. `claude -p` reports failures (usage limit,
/// unknown model, not logged in) on stdout, `codex` on stderr, so both are included.
fn failure_message(program: &str, status: impl std::fmt::Display, stdout: &[u8], stderr: &[u8]) -> String {
    let (stdout, stderr) = (String::from_utf8_lossy(stdout), String::from_utf8_lossy(stderr));
    let parts: Vec<&str> = [stderr.trim(), stdout.trim()].into_iter().filter(|s| !s.is_empty()).collect();
    let detail = if parts.is_empty() { "no output".to_string() } else { parts.join("\n") };
    format!("`{program}` failed ({status}): {detail}")
}

#[cfg(test)]
mod tests {
    use super::failure_message;

    #[test]
    fn failure_reported_on_stdout_is_shown() {
        let msg = failure_message("claude", "exit status: 1", b"Usage limit reached\n", b"");
        assert_eq!(msg, "`claude` failed (exit status: 1): Usage limit reached");
    }

    #[test]
    fn stderr_and_stdout_are_both_shown() {
        let msg = failure_message("codex", "exit status: 2", b"partial", b"boom\n");
        assert_eq!(msg, "`codex` failed (exit status: 2): boom\npartial");
    }

    #[test]
    fn silent_failure_says_so() {
        let msg = failure_message("claude", "exit status: 1", b"", b" \n");
        assert_eq!(msg, "`claude` failed (exit status: 1): no output");
    }
}

impl Provider for CliProvider {
    fn complete(&self, history: &[Message]) -> Result<String> {
        let mut command = crate::sandbox::provider_command(self.program, &self.args, &self.root, &crate::layout().company)?;
        if self.program == "codex" { command.arg("-"); }
        let output = crate::process::run(command, flatten(history).into_bytes(), std::time::Duration::from_secs(120))
            .map_err(|e| format!("could not run `{}`: {e}", self.program))?;

        if !output.status.success() {
            return Err(failure_message(self.program, output.status, &output.stdout, &output.stderr).into());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

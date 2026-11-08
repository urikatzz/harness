use crate::provider::{Message, Provider, Result, Role};
use std::process::Command;

/// Delegates to an installed agent CLI (`claude`, `codex`), which uses its own login.
pub struct CliProvider {
    program: &'static str,
    args: Vec<String>,
}

impl CliProvider {
    pub fn claude(model: Option<String>, effort: Option<String>) -> Self {
        let mut args = vec!["-p".to_string()];
        if let Some(m) = model {
            args.extend(["--model".into(), m]);
        }
        if let Some(e) = effort {
            args.extend(["--effort".into(), e]);
        }
        Self { program: "claude", args }
    }

    pub fn codex(model: Option<String>, effort: Option<String>) -> Self {
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
        Self { program: "codex", args }
    }
}

/// CLIs take one prompt, so multi-turn history is flattened into a transcript.
fn flatten(history: &[Message]) -> String {
    if let [only] = history {
        return only.content.clone();
    }
    let mut out = String::from("Continue this conversation. Reply only as the assistant.\n\n");
    for m in history {
        let who = if m.role == Role::User { "User" } else { "Assistant" };
        out.push_str(&format!("{who}: {}\n\n", m.content));
    }
    out
}

impl Provider for CliProvider {
    fn complete(&self, history: &[Message]) -> Result<String> {
        let output = Command::new(self.program)
            .args(&self.args)
            .arg(flatten(history))
            .output()
            .map_err(|e| format!("could not run `{}`: {e}", self.program))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("`{}` failed ({}): {}", self.program, output.status, stderr.trim()).into());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

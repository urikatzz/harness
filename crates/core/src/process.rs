//! Bounded subprocesses, including their descendants.
use crate::provider::Result;
use std::io::{Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: usize = 1024 * 1024;
const INPUT_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

fn capture(mut reader: impl Read, exceeded: Arc<AtomicBool>) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            return Ok(output);
        }
        let remaining = OUTPUT_LIMIT.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..n.min(remaining)]);
        if n > remaining {
            exceeded.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(unix)]
fn terminate_group(id: u32) {
    // The child starts a fresh process group; terminate inherited pipes and grandchildren too.
    unsafe {
        libc::kill(-(id as i32), libc::SIGKILL);
    }
}

pub fn run(mut command: Command, input: Vec<u8>, timeout: Duration) -> Result<Output> {
    if input.len() > INPUT_LIMIT {
        return Err("subprocess input exceeds 8 MiB".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let exceeded = Arc::new(AtomicBool::new(false));
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let out_flag = exceeded.clone();
    let err_flag = exceeded.clone();
    let out = std::thread::spawn(move || capture(stdout, out_flag));
    let err = std::thread::spawn(move || capture(stderr, err_flag));
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let started = Instant::now();
    let mut status = None;
    let failure = loop {
        if exceeded.load(Ordering::Relaxed) {
            break Some("subprocess output exceeds 1 MiB");
        }
        if started.elapsed() >= timeout {
            break Some("subprocess timed out");
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(result) => status = result,
                Err(_) => break Some("could not wait for subprocess"),
            }
        }
        if status.is_some() && out.is_finished() && err.is_finished() && writer.is_finished() {
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    #[cfg(unix)]
    terminate_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
    let stdout = out.join().map_err(|_| "stdout reader failed")??;
    let stderr = err.join().map_err(|_| "stderr reader failed")??;
    let write_result = writer.join().map_err(|_| "stdin writer failed")?;
    if let Some(reason) = failure {
        return Err(reason.into());
    }
    if exceeded.load(Ordering::Relaxed) {
        return Err("subprocess output exceeds 1 MiB".into());
    }
    // Broken pipe is common when a command does not consume its arguments on stdin.
    if let Err(e) = write_result {
        if e.kind() != std::io::ErrorKind::BrokenPipe {
            return Err(e.into());
        }
    }
    Ok(Output {
        status: status.unwrap(),
        stdout,
        stderr,
    })
}

/// Custom tools may read their own directory and system runtimes, but cannot write files.
/// Protected project paths stay inaccessible even inside the tool directory.
pub fn tool_command(
    program: &str,
    args: &[String],
    dir: &std::path::Path,
    policy: &crate::permissions::Policy,
) -> Result<Command> {
    if !cfg!(target_os = "macos") {
        return Err("custom tools require the macOS filesystem sandbox; use built-in file tools on this platform".into());
    }
    let dir = dir.canonicalize()?;
    let quote = |s: &str| serde_json::to_string(s).unwrap();
    let mut profile = format!(
        "(version 1)(allow default)(deny file-read* file-write*)(allow file-read-metadata)(allow file-read* (subpath \"/System\") (subpath \"/usr\") (subpath \"/bin\") (subpath \"/sbin\") (subpath \"/Library/Apple\") (subpath \"/private/var/db/dyld\") (literal \"/dev/null\") (literal \"/\") (subpath {}))",
        quote(&dir.to_string_lossy())
    );
    for pattern in policy.protected_patterns() {
        let absolute = format!("{}/{}", policy.root().display(), pattern);
        profile.push_str(&format!(
            "(deny file-read* (regex #{}))",
            quote(&glob_regex(&absolute)).replace("\\\\", "\\")
        ));
    }
    let mut command = Command::new("/usr/bin/sandbox-exec");
    command
        .args(["-p", &profile, program])
        .args(args)
        .current_dir(dir);
    Ok(command)
}

fn glob_regex(pattern: &str) -> String {
    let mut regex = String::from("^");
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
            '.' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\' => {
                regex.push('\\');
                regex.push(ch);
            }
            _ => regex.push(ch),
        }
    }
    regex.push('$');
    regex
}

/// CLI backends handle inference and login; project access belongs to harness tools.
pub fn provider_command(program: &str, args: &[String], root: &std::path::Path) -> Result<Command> {
    if !cfg!(target_os = "macos") {
        return Err("CLI backends require the macOS filesystem sandbox; use an HTTP backend on this platform".into());
    }
    let root = root.canonicalize()?;
    let root = serde_json::to_string(&root.to_string_lossy())?;
    let profile =
        format!("(version 1)(allow default)(deny file-read-data file-write* (subpath {root}))");
    let mut command = Command::new("/usr/bin/sandbox-exec");
    command.args(["-p", &profile, program]).args(args);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{permissions::Policy, test_support::TestDir};

    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn bounds_runtime_output_and_inherited_pipes() {
        assert!(run(shell("sleep 5"), vec![], Duration::from_millis(100))
            .unwrap_err()
            .to_string()
            .contains("timed out"));
        assert!(run(
            shell("sleep 5 & exit 0"),
            vec![],
            Duration::from_millis(100)
        )
        .unwrap_err()
        .to_string()
        .contains("timed out"));
        assert!(run(shell("yes x"), vec![], Duration::from_secs(5))
            .unwrap_err()
            .to_string()
            .contains("output exceeds"));
        let result = run(
            shell("cat; printf warning >&2"),
            b"hello".to_vec(),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(result.status.success());
        assert_eq!(result.stdout, b"hello");
        assert_eq!(result.stderr, b"warning");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sandbox_contains_custom_tools_and_native_provider_reads() {
        let root = TestDir::new();
        let external = TestDir::new();
        let dir = root.path().join("tools/example");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(root.path().join("secret"), "project secret").unwrap();
        std::fs::write(external.path().join("secret"), "outside secret").unwrap();
        std::fs::write(dir.join("allowed"), "allowed").unwrap();
        std::fs::write(dir.join("private.pem"), "private").unwrap();
        std::fs::write(dir.join(".env"), "approval needed").unwrap();
        std::fs::write(
            root.path().join("permissions.json"),
            r#"{"mode":"yolo","files":{"deny":["**/*.pem"],"approve":["**/.env"]}}"#,
        )
        .unwrap();
        let policy = Policy::load(root.path(), None).unwrap();
        let run_tool = |script: String| {
            let args = vec!["-c".into(), script];
            run(
                tool_command("/bin/sh", &args, &dir, &policy).unwrap(),
                vec![],
                Duration::from_secs(5),
            )
            .unwrap()
        };
        let allowed = run_tool("cat allowed".into());
        assert!(
            allowed.status.success(),
            "status={} stdout={} stderr={}",
            allowed.status,
            String::from_utf8_lossy(&allowed.stdout),
            String::from_utf8_lossy(&allowed.stderr)
        );
        assert_eq!(allowed.stdout, b"allowed");
        for script in [
            "cat ../../secret".to_string(),
            format!("cat '{}/secret'", external.path().display()),
            "cat private.pem".into(),
            "cat .env".into(),
            "echo changed > allowed".into(),
        ] {
            let result = run_tool(script.clone());
            assert!(!result.status.success(), "unexpectedly allowed {script}");
        }
        let args = vec![
            "-c".into(),
            format!("cat '{}/secret'", root.path().display()),
        ];
        let provider = run(
            provider_command("/bin/sh", &args, root.path()).unwrap(),
            vec![],
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(!provider.status.success());
    }
}

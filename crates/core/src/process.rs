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

/// A private, empty temp folder for one tool call, removed afterwards.
pub struct Scratch(std::path::PathBuf);
impl Scratch {
    pub fn new() -> Result<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!("uritheking-tool-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir(&path)?;
        Ok(Self(path.canonicalize()?))
    }
    pub fn path(&self) -> &std::path::Path { &self.0 }
}
impl Drop for Scratch {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

const UNSANDBOXED_HINT: &str = "set \"sandbox\": false in permissions.json to run without it";

/// `sandbox-exec` is deprecated by Apple, so check once that it still works before relying on it.
fn sandbox_exec() -> Result<Command> {
    static WORKS: std::sync::OnceLock<std::result::Result<(), String>> = std::sync::OnceLock::new();
    let works = WORKS.get_or_init(|| {
        if !cfg!(target_os = "macos") {
            return Err(format!("the filesystem sandbox needs macOS; {UNSANDBOXED_HINT}"));
        }
        let probe = Command::new("/usr/bin/sandbox-exec")
            .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .status();
        match probe {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(format!("/usr/bin/sandbox-exec is not working ({status}); {UNSANDBOXED_HINT}")),
            Err(e) => Err(format!("/usr/bin/sandbox-exec is unavailable ({e}); {UNSANDBOXED_HINT}")),
        }
    });
    works.clone()?;
    Ok(Command::new("/usr/bin/sandbox-exec"))
}

/// A quoted SBPL string. Characters that would need escaping are refused instead of escaped.
fn sbpl_string(s: &str) -> Result<String> {
    if s.chars().any(|c| c == '"' || c == '\\' || c.is_control()) {
        return Err(format!("path cannot be used in the sandbox profile: {s:?}").into());
    }
    Ok(format!("\"{s}\""))
}

/// Custom tools may read their own directory, system runtimes, their `writes` folders and a
/// private temp folder, and write only to the last two. Protected project paths stay off-limits.
pub fn tool_command(
    program: &str,
    args: &[String],
    dir: &std::path::Path,
    writes: &[std::path::PathBuf],
    scratch: &std::path::Path,
    policy: &crate::permissions::Policy,
) -> Result<Command> {
    let dir = dir.canonicalize()?;
    if policy.unsandboxed {
        let mut command = Command::new(program);
        command.args(args).current_dir(&dir).env("TMPDIR", scratch);
        return Ok(command);
    }
    let mut command = sandbox_exec()?;
    let mut profile = format!(
        "(version 1)(allow default)(deny file-read* file-write*)(allow file-read-metadata)(allow file-read* (subpath \"/System\") (subpath \"/usr\") (subpath \"/bin\") (subpath \"/sbin\") (subpath \"/Library/Apple\") (subpath \"/private/var/db/dyld\") (literal \"/dev/null\") (literal \"/\") (subpath {}))(allow file-write* (literal \"/dev/null\"))",
        sbpl_string(&dir.to_string_lossy())?
    );
    for writable in writes.iter().map(|p| p.as_path()).chain([scratch]) {
        let quoted = sbpl_string(&writable.to_string_lossy())?;
        profile.push_str(&format!("(allow file-read* file-write* (subpath {quoted}))"));
    }
    // Later rules win, so protected paths are denied even inside writable folders.
    let root = regex_escape(&policy.root().to_string_lossy());
    for pattern in policy.protected_patterns() {
        let regex = format!("^{root}/{}$", glob_regex(pattern));
        if regex.contains('"') || regex.chars().any(char::is_control) {
            return Err(format!("protected pattern cannot be used in the sandbox profile: {pattern:?}").into());
        }
        profile.push_str(&format!("(deny file-read* file-write* (regex #\"{regex}\"))"));
    }
    command.args(["-p", &profile, program]).args(args).current_dir(dir).env("TMPDIR", scratch);
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

/// CLI backends handle inference and login; project access belongs to harness tools.
pub fn provider_command(program: &str, args: &[String], root: &std::path::Path) -> Result<Command> {
    let root = root.canonicalize()?;
    if crate::permissions::Policy::load(&root, None)?.unsandboxed {
        let mut command = Command::new(program);
        command.args(args);
        return Ok(command);
    }
    let mut command = sandbox_exec()?;
    let profile = format!(
        "(version 1)(allow default)(deny file-read-data file-write* (subpath {}))",
        sbpl_string(&root.to_string_lossy())?
    );
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
        let base = TestDir::new();
        let external = TestDir::new();
        // Regex metacharacters in the project path must be matched literally.
        let project = base.path().join("acme+co (v1.0)");
        let dir = project.join("tools/example");
        let out = project.join("out");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(project.join("secret"), "project secret").unwrap();
        std::fs::write(external.path().join("secret"), "outside secret").unwrap();
        std::fs::write(dir.join("allowed"), "allowed").unwrap();
        std::fs::write(dir.join("private.pem"), "private").unwrap();
        std::fs::write(dir.join(".env"), "approval needed").unwrap();
        std::fs::write(
            project.join("permissions.json"),
            r#"{"mode":"yolo","files":{"deny":["**/*.pem"],"approve":["**/.env"]}}"#,
        )
        .unwrap();
        let policy = Policy::load(&project, None).unwrap();
        let run_tool = |script: String| {
            let scratch = Scratch::new().unwrap();
            let args = vec!["-c".into(), script];
            run(
                tool_command("/bin/sh", &args, &dir, std::slice::from_ref(&out), scratch.path(), &policy).unwrap(),
                vec![],
                Duration::from_secs(5),
            )
            .unwrap()
        };
        let allowed = run_tool("cat allowed".into());
        assert!(allowed.status.success(), "stderr={}", String::from_utf8_lossy(&allowed.stderr));
        assert_eq!(allowed.stdout, b"allowed");
        let wrote = run_tool(format!(
            "echo report > '{0}/report.txt' && echo tmp > \"$TMPDIR/t\" && cat \"$TMPDIR/t\" >/dev/null && cat '{0}/report.txt'",
            out.display()
        ));
        assert!(wrote.status.success(), "stderr={}", String::from_utf8_lossy(&wrote.stderr));
        assert_eq!(wrote.stdout, b"report\n");
        for script in [
            "cat ../../secret".to_string(),
            format!("cat '{}/secret'", external.path().display()),
            "cat private.pem".into(),
            "cat .env".into(),
            "echo changed > allowed".into(),
            format!("echo x > '{}/permissions.json'", project.display()),
            format!("echo x > '{}/key.pem'", out.display()),
        ] {
            let result = run_tool(script.clone());
            assert!(!result.status.success(), "unexpectedly allowed {script}");
        }
        assert!(!out.join("key.pem").exists());
        let args = vec!["-c".into(), format!("cat '{}/secret'", project.display())];
        let provider = run(
            provider_command("/bin/sh", &args, &project).unwrap(),
            vec![],
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(!provider.status.success());
    }

    #[test]
    fn sandbox_opt_out_runs_tools_directly() {
        let root = TestDir::new();
        let dir = root.path().join("tools/example");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(root.path().join("permissions.json"), r#"{"sandbox":false}"#).unwrap();
        let policy = Policy::load(root.path(), None).unwrap();
        let scratch = Scratch::new().unwrap();
        let args = vec!["-c".into(), "pwd; echo \"$TMPDIR\"".into()];
        let command = tool_command("/bin/sh", &args, &dir, &[], scratch.path(), &policy).unwrap();
        assert_eq!(command.get_program(), "/bin/sh");
        let out = run(command, vec![], Duration::from_secs(5)).unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(text.contains(&*dir.to_string_lossy()) && text.contains(&*scratch.path().to_string_lossy()), "{text}");
        assert_eq!(provider_command("/bin/sh", &[], root.path()).unwrap().get_program(), "/bin/sh");
    }

    #[test]
    fn profile_quoting_refuses_unsafe_text() {
        assert!(sbpl_string("/a/b c+d").is_ok());
        for bad in ["/a\"b", "/a\\b", "/a\nb"] { assert!(sbpl_string(bad).is_err(), "{bad:?}"); }
        assert_eq!(glob_regex("**/*.pem"), r"(.*/)?[^/]*\.pem");
        assert_eq!(glob_regex("secrets/**"), "secrets/.*");
        assert_eq!(regex_escape("/x+y (1).d"), r"/x\+y \(1\)\.d");
    }
}

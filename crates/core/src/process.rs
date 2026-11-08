//! Bounded subprocesses, including their descendants.
use crate::provider::Result;
use std::io::{Read, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::path::{Path, PathBuf};
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

/// A private temp folder for one tool call, removed afterwards. `path()` is the tool's `$TMPDIR`;
/// next to it sits an unreadable placeholder used to cover protected files on Linux.
pub struct Scratch {
    base: PathBuf,
    tmp: PathBuf,
}
impl Scratch {
    pub fn new() -> Result<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let base = std::env::temp_dir().join(format!("uritheking-tool-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir(&base)?;
        let base = base.canonicalize()?;
        let tmp = base.join("tmp");
        std::fs::create_dir(&tmp)?;
        let blocked = base.join("blocked");
        std::fs::write(&blocked, "")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000))?;
        }
        Ok(Self { base, tmp })
    }
    pub fn path(&self) -> &Path { &self.tmp }
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn blocked(&self) -> PathBuf { self.base.join("blocked") }
}
impl Drop for Scratch {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.base); }
}

const UNSANDBOXED_HINT: &str = "set \"sandbox\": false in permissions.json to run without it";

/// The sandbox launcher for this OS, checked once per run: `sandbox-exec` on macOS (deprecated by
/// Apple, so it is probed before use) and bubblewrap (`bwrap`) on Linux.
fn sandbox_program() -> Result<Command> {
    static WORKS: std::sync::OnceLock<std::result::Result<&'static str, String>> = std::sync::OnceLock::new();
    let works = WORKS.get_or_init(|| {
        let (program, probe): (&'static str, &[&str]) = if cfg!(target_os = "macos") {
            ("/usr/bin/sandbox-exec", &["-p", "(version 1)(allow default)", "/usr/bin/true"])
        } else if cfg!(target_os = "linux") {
            ("bwrap", &["--unshare-all", "--die-with-parent", "--ro-bind", "/", "/", "true"])
        } else {
            return Err(format!("the filesystem sandbox needs macOS or Linux with bubblewrap; {UNSANDBOXED_HINT}"));
        };
        let status = Command::new(program)
            .args(probe)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .status();
        match status {
            Ok(status) if status.success() => Ok(program),
            Ok(status) => Err(format!("{program} is not working ({status}); {UNSANDBOXED_HINT}")),
            Err(e) => Err(format!("{program} is unavailable ({e}); install bubblewrap on Linux, or {UNSANDBOXED_HINT}")),
        }
    });
    Ok(Command::new(works.clone()?))
}

/// A quoted SBPL string. Characters that would need escaping are refused instead of escaped.
fn sbpl_string(s: &str) -> Result<String> {
    if s.chars().any(|c| c == '"' || c == '\\' || c.is_control()) {
        return Err(format!("path cannot be used in the sandbox profile: {s:?}").into());
    }
    Ok(format!("\"{s}\""))
}

/// Environment variables every tool gets. Everything else (API keys, tokens) is dropped
/// unless the tool lists it in `env`.
const PASSED_ENV: &[&str] = &["PATH", "HOME", "USER", "LANG", "LC_ALL", "LC_CTYPE", "TZ"];

/// Harness state that tools never see, even when they may read the whole project.
const HIDDEN: &[&str] = &["sessions", "logs"];

/// What a custom tool may touch.
pub struct ToolAccess<'a> {
    /// The tool's own folder (read-only).
    pub dir: &'a Path,
    /// Project folders it may read.
    pub reads: &'a [PathBuf],
    /// Project folders it may read and write.
    pub writes: &'a [PathBuf],
    /// Outbound network; off unless the tool asks for it.
    pub network: bool,
    /// Extra environment variables passed through by name.
    pub env: &'a [String],
}

/// A sandboxed tool command, plus (on Linux) a check that it did not create protected files.
pub struct ToolCommand {
    pub command: Command,
    guard: Option<NewFileGuard>,
}

impl ToolCommand {
    pub fn run(self, input: Vec<u8>, timeout: Duration) -> Result<Output> {
        let output = run(self.command, input, timeout);
        if let Some(guard) = self.guard { guard.check()?; }
        output
    }
}

/// Custom tools may read their own directory, system runtimes, their `reads` and `writes` folders
/// and a private temp folder, and write only to the last two. Protected project paths and harness
/// state stay off-limits, and the network is blocked unless `network` is set.
pub fn tool_command(
    program: &str,
    args: &[String],
    access: &ToolAccess,
    scratch: &Scratch,
    policy: &crate::permissions::Policy,
) -> Result<ToolCommand> {
    let dir = access.dir.canonicalize()?;
    let mut command = if policy.unsandboxed {
        let mut command = Command::new(program);
        command.args(args);
        command
    } else if cfg!(target_os = "linux") {
        return linux_tool_command(program, args, access, &dir, scratch, policy);
    } else {
        macos_tool_command(program, args, access, &dir, scratch, policy)?
    };
    set_tool_env(&mut command, access, scratch);
    command.current_dir(&dir);
    Ok(ToolCommand { command, guard: None })
}

fn set_tool_env(command: &mut Command, access: &ToolAccess, scratch: &Scratch) {
    command.env_clear();
    for key in PASSED_ENV.iter().copied().chain(access.env.iter().map(String::as_str)) {
        if let Some(value) = std::env::var_os(key) { command.env(key, value); }
    }
    command.env("TMPDIR", scratch.path());
}

fn macos_tool_command(
    program: &str,
    args: &[String],
    access: &ToolAccess,
    dir: &Path,
    scratch: &Scratch,
    policy: &crate::permissions::Policy,
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

/// bubblewrap: an empty, read-only root with only system runtimes and the tool's folders mounted.
/// Existing protected files are covered by an unreadable placeholder; protected files the tool
/// creates in its `writes` folders are removed afterwards and the call fails.
fn linux_tool_command(
    program: &str,
    args: &[String],
    access: &ToolAccess,
    dir: &Path,
    scratch: &Scratch,
    policy: &crate::permissions::Policy,
) -> Result<ToolCommand> {
    use std::ffi::OsString;
    let mut command = sandbox_program()?;
    let mut a: Vec<OsString> = Vec::new();
    let mut push = |items: &[&std::ffi::OsStr]| a.extend(items.iter().map(|s| s.to_os_string()));
    // Dropping capabilities matters when run as root, which could otherwise read past the placeholder.
    push(&["--die-with-parent".as_ref(), "--new-session".as_ref(), "--unshare-all".as_ref(), "--cap-drop".as_ref(), "ALL".as_ref()]);
    if access.network { push(&["--share-net".as_ref()]); }
    push(&["--ro-bind".as_ref(), "/usr".as_ref(), "/usr".as_ref()]);
    let mut system: Vec<&str> = vec!["/bin", "/sbin", "/lib", "/lib32", "/lib64", "/libx32",
        "/etc/ld.so.cache", "/etc/ld.so.conf", "/etc/ld.so.conf.d", "/etc/alternatives"];
    if access.network {
        system.extend(["/etc/resolv.conf", "/etc/hosts", "/etc/nsswitch.conf", "/etc/gai.conf",
            "/etc/ssl", "/etc/ca-certificates", "/etc/pki"]);
    }
    for path in system {
        match std::fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_symlink() => {
                let target = std::fs::read_link(path)?;
                push(&["--symlink".as_ref(), target.as_os_str(), path.as_ref()]);
            }
            Ok(_) => push(&["--ro-bind".as_ref(), path.as_ref(), path.as_ref()]),
            Err(_) => {}
        }
    }
    push(&["--proc".as_ref(), "/proc".as_ref(), "--dev".as_ref(), "/dev".as_ref()]);
    push(&["--ro-bind".as_ref(), dir.as_os_str(), dir.as_os_str()]);
    for r in access.reads { push(&["--ro-bind".as_ref(), r.as_os_str(), r.as_os_str()]); }
    for w in access.writes { push(&["--bind".as_ref(), w.as_os_str(), w.as_os_str()]); }
    push(&["--bind".as_ref(), scratch.path().as_os_str(), scratch.path().as_os_str()]);
    let root = policy.root();
    let mounted: Vec<PathBuf> = [dir.to_path_buf()].into_iter().chain(access.reads.iter().cloned()).chain(access.writes.iter().cloned()).collect();
    for hidden in HIDDEN.iter().map(|h| root.join(h)) {
        if hidden.is_dir() && mounted.iter().any(|m| hidden.starts_with(m)) {
            push(&["--tmpfs".as_ref(), hidden.as_os_str()]);
        }
    }
    let patterns: Vec<String> = policy.protected_patterns().map(str::to_owned).collect();
    let blocked = scratch.blocked();
    for file in protected_files(root, &mounted, &patterns)? {
        push(&["--ro-bind".as_ref(), blocked.as_os_str(), file.as_os_str()]);
    }
    push(&["--remount-ro".as_ref(), "/".as_ref(), "--chdir".as_ref(), dir.as_os_str(), "--".as_ref(), program.as_ref()]);
    command.args(a).args(args);
    set_tool_env(&mut command, access, scratch);
    let guard = NewFileGuard {
        root: root.to_path_buf(),
        before: protected_files(root, access.writes, &patterns)?.into_iter().collect(),
        dirs: access.writes.to_vec(),
        patterns,
    };
    Ok(ToolCommand { command, guard: Some(guard) })
}

/// Remembers which protected files existed in the writable folders before a call.
struct NewFileGuard {
    root: PathBuf,
    dirs: Vec<PathBuf>,
    patterns: Vec<String>,
    before: std::collections::HashSet<PathBuf>,
}

impl NewFileGuard {
    fn check(self) -> Result<()> {
        let created: Vec<PathBuf> = protected_files(&self.root, &self.dirs, &self.patterns)?
            .into_iter().filter(|f| !self.before.contains(f)).collect();
        if created.is_empty() { return Ok(()); }
        for file in &created { let _ = std::fs::remove_file(file); }
        let names: Vec<String> = created.iter().map(|f| f.strip_prefix(&self.root).unwrap_or(f).display().to_string()).collect();
        Err(format!("tool created protected files, which were removed: {}", names.join(", ")).into())
    }
}

/// Files under `dirs` whose project-relative path matches a protected pattern. Symlinks are not
/// followed. Fails closed on very large trees.
fn protected_files(root: &Path, dirs: &[PathBuf], patterns: &[String]) -> Result<Vec<PathBuf>> {
    const LIMIT: usize = 200_000;
    let mut found = Vec::new();
    if patterns.is_empty() { return Ok(found); }
    let mut stack: Vec<PathBuf> = dirs.to_vec();
    let mut seen = 0;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            seen += 1;
            if seen > LIMIT { return Err("too many files to check protected paths in a tool's folders".into()); }
            let path = entry?.path();
            let meta = std::fs::symlink_metadata(&path)?;
            if meta.is_dir() {
                stack.push(path);
            } else if !meta.file_type().is_symlink() {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
                if patterns.iter().any(|p| crate::permissions::glob(p, &rel)) { found.push(path); }
            }
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
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
pub fn provider_command(program: &str, args: &[String], root: &Path) -> Result<Command> {
    let root = root.canonicalize()?;
    if crate::permissions::Policy::load(&root, None)?.unsandboxed {
        let mut command = Command::new(program);
        command.args(args);
        return Ok(command);
    }
    let mut command = sandbox_program()?;
    if cfg!(target_os = "linux") {
        // Everything stays visible except the project, which is covered by an empty folder.
        // `--chdir` matters: a working directory inside the project would still reach the real files.
        command.args(["--die-with-parent", "--dev-bind", "/", "/", "--tmpfs"]).arg(&root)
            .arg("--chdir").arg(&root).arg("--").arg(program).args(args);
        return Ok(command);
    }
    let profile = format!(
        "(version 1)(allow default)(deny file-read-data file-write* (subpath {}))",
        sbpl_string(&root.to_string_lossy())?
    );
    // Start outside the project: the backend can't read it, and some CLIs refuse an unreadable cwd.
    command.args(["-p", &profile, program]).args(args).current_dir(std::env::temp_dir());
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

    fn access<'a>(dir: &'a Path, reads: &'a [PathBuf], writes: &'a [PathBuf], network: bool, env: &'a [String]) -> ToolAccess<'a> {
        ToolAccess { dir, reads, writes, network, env }
    }

    fn run_in(program: &str, args: &[&str], access: &ToolAccess, policy: &Policy) -> Output {
        let scratch = Scratch::new().unwrap();
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        tool_command(program, &args, access, &scratch, policy).unwrap().run(vec![], Duration::from_secs(5)).unwrap()
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn sandbox_contains_custom_tools_and_native_provider_reads() {
        let base = TestDir::new();
        let external = TestDir::new();
        // Regex metacharacters in the project path must be matched literally.
        let project = base.path().join("acme+co (v1.0)");
        let dir = project.join("tools/example");
        let out = project.join("out");
        let docs = project.join("docs");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::create_dir_all(project.join("sessions")).unwrap();
        std::fs::write(project.join("sessions/1.json"), "chat").unwrap();
        std::fs::write(project.join("secret"), "project secret").unwrap();
        std::fs::write(docs.join("guide.md"), "guide").unwrap();
        std::fs::write(docs.join("cert.pem"), "private").unwrap();
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
        let docs_only = [docs.clone()];
        let writes = [out.clone()];
        let tool = access(&dir, &docs_only, &writes, false, &[]);
        let sh = |script: String| run_in("/bin/sh", &["-c", &script], &tool, &policy);
        let allowed = sh("cat allowed".into());
        assert!(allowed.status.success(), "stderr={}", String::from_utf8_lossy(&allowed.stderr));
        assert_eq!(allowed.stdout, b"allowed");
        let read = sh(format!("cat '{}/guide.md'", docs.display()));
        assert_eq!(read.stdout, b"guide", "stderr={}", String::from_utf8_lossy(&read.stderr));
        let wrote = sh(format!(
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
            format!("cat '{}/cert.pem'", docs.display()),
            "echo changed > allowed".into(),
            format!("echo x > '{}/guide.md'", docs.display()),
            format!("echo x > '{}/permissions.json'", project.display()),
        ] {
            let result = sh(script.clone());
            assert!(!result.status.success(), "unexpectedly allowed {script}");
        }
        // Reading the whole project still hides harness state and protected files.
        let whole = [project.clone()];
        let wide = access(&dir, &whole, &[], false, &[]);
        let wide_sh = |script: String| run_in("/bin/sh", &["-c", &script], &wide, &policy);
        assert_eq!(wide_sh(format!("cat '{}/secret'", project.display())).stdout, b"project secret");
        for script in [format!("cat '{}/sessions/1.json'", project.display()), format!("cat '{}/cert.pem'", docs.display())] {
            assert!(!wide_sh(script.clone()).status.success(), "unexpectedly allowed {script}");
        }
        // Creating a protected file in a writable folder never leaves it behind.
        let scratch = Scratch::new().unwrap();
        let script = vec!["-c".into(), format!("echo x > '{}/key.pem'", out.display())];
        let created = tool_command("/bin/sh", &script, &tool, &scratch, &policy).unwrap().run(vec![], Duration::from_secs(5));
        assert!(created.map_or(true, |o| !o.status.success()));
        assert!(!out.join("key.pem").exists());
        let args = vec!["-c".into(), format!("cat '{}/secret'; cat secret", project.display())];
        let provider = run(provider_command("/bin/sh", &args, &project).unwrap(), vec![], Duration::from_secs(5)).unwrap();
        assert!(provider.stdout.is_empty(), "provider read {}", String::from_utf8_lossy(&provider.stdout));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn tools_get_a_clean_environment_and_no_network_unless_asked() {
        let root = TestDir::new();
        let dir = root.path().join("tools/example");
        std::fs::create_dir_all(&dir).unwrap();
        let policy = Policy::load(root.path(), None).unwrap();
        std::env::set_var("URITHEKING_TEST_SECRET", "hunter2");
        std::env::set_var("URITHEKING_TEST_TOKEN", "passed");
        let none: [PathBuf; 0] = [];
        let env = ["URITHEKING_TEST_TOKEN".to_string()];
        let out = run_in("/usr/bin/env", &[], &access(&dir, &none, &none, false, &env), &policy);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("URITHEKING_TEST_TOKEN=passed"), "{text}");
        for line in text.lines() {
            let key = line.split('=').next().unwrap();
            assert!(PASSED_ENV.contains(&key) || ["TMPDIR", "PWD", "URITHEKING_TEST_TOKEN"].contains(&key), "leaked {key}");
        }

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let connect = format!("exec 3<>/dev/tcp/127.0.0.1/{port}");
        let blocked = run_in("/bin/bash", &["-c", &connect], &access(&dir, &none, &none, false, &[]), &policy);
        assert!(!blocked.status.success(), "network was reachable");
        let allowed = run_in("/bin/bash", &["-c", &connect], &access(&dir, &none, &none, true, &[]), &policy);
        assert!(allowed.status.success(), "stderr={}", String::from_utf8_lossy(&allowed.stderr));
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
        let none: [PathBuf; 0] = [];
        let command = tool_command("/bin/sh", &args, &access(&dir, &none, &none, false, &[]), &scratch, &policy).unwrap();
        assert_eq!(command.command.get_program(), "/bin/sh");
        let out = command.run(vec![], Duration::from_secs(5)).unwrap();
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

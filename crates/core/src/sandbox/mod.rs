//! The OS sandbox for custom tools and CLI backends: `sandbox-exec` on macOS, bubblewrap on Linux.

mod linux;
mod macos;

use crate::error::Result;
use crate::permissions::Policy;
use crate::process::{run, Output};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::Duration;

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
        let base = std::env::temp_dir().join(format!("umango-tool-{}-{nanos}-{n}", std::process::id()));
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
    guard: Option<linux::NewFileGuard>,
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
    policy: &Policy,
) -> Result<ToolCommand> {
    let dir = access.dir.canonicalize()?;
    let mut command = if policy.unsandboxed {
        let mut command = Command::new(program);
        command.args(args);
        command
    } else if cfg!(target_os = "linux") {
        return linux::tool_command(program, args, access, &dir, scratch, policy);
    } else {
        macos::tool_command(program, args, access, &dir, scratch, policy)?
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

/// CLI backends handle inference and login; project access belongs to harness tools.
pub fn provider_command(program: &str, args: &[String], root: &Path, company: &Path) -> Result<Command> {
    let root = root.canonicalize()?;
    if Policy::load_with(company, &root, None)?.unsandboxed {
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
        macos::sbpl_string(&root.to_string_lossy())?
    );
    // Start outside the project: the backend can't read it, and some CLIs refuse an unreadable cwd.
    command.args(["-p", &profile, program]).args(args).current_dir(std::env::temp_dir());
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDir;

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
        let provider = run(provider_command("/bin/sh", &args, &project, &project).unwrap(), vec![], Duration::from_secs(5)).unwrap();
        assert!(provider.stdout.is_empty(), "provider read {}", String::from_utf8_lossy(&provider.stdout));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn tools_get_a_clean_environment_and_no_network_unless_asked() {
        let root = TestDir::new();
        let dir = root.path().join("tools/example");
        std::fs::create_dir_all(&dir).unwrap();
        let policy = Policy::load(root.path(), None).unwrap();
        std::env::set_var("UMANGO_TEST_SECRET", "hunter2");
        std::env::set_var("UMANGO_TEST_TOKEN", "passed");
        let none: [PathBuf; 0] = [];
        let env = ["UMANGO_TEST_TOKEN".to_string()];
        let out = run_in("/usr/bin/env", &[], &access(&dir, &none, &none, false, &env), &policy);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("UMANGO_TEST_TOKEN=passed"), "{text}");
        for line in text.lines() {
            let key = line.split('=').next().unwrap();
            assert!(PASSED_ENV.contains(&key) || ["TMPDIR", "PWD", "UMANGO_TEST_TOKEN"].contains(&key), "leaked {key}");
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
        assert_eq!(provider_command("/bin/sh", &[], root.path(), root.path()).unwrap().get_program(), "/bin/sh");
    }

}

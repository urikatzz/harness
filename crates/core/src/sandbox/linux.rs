//! Linux: bubblewrap with an empty root and only the tool's folders mounted.

use super::{sandbox_program, set_tool_env, Scratch, ToolAccess, ToolCommand, HIDDEN};
use crate::error::Result;
use crate::permissions::Policy;
use std::path::{Path, PathBuf};

/// bubblewrap: an empty, read-only root with only system runtimes and the tool's folders mounted.
/// Existing protected files are covered by an unreadable placeholder; protected files the tool
/// creates in its `writes` folders are removed afterwards and the call fails.
pub(super) fn tool_command(
    program: &str,
    args: &[String],
    access: &ToolAccess,
    dir: &Path,
    scratch: &Scratch,
    policy: &Policy,
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
pub(super) struct NewFileGuard {
    root: PathBuf,
    dirs: Vec<PathBuf>,
    patterns: Vec<String>,
    before: std::collections::HashSet<PathBuf>,
}

impl NewFileGuard {
    pub(super) fn check(self) -> Result<()> {
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

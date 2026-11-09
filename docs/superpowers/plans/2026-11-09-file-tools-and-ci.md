# File Tools, Output Caps and CI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the model `list_files`, `search_files`, `edit_file` and paged `read_file`, cap every tool result at 64 KiB, and gate pushes and releases on CI.

**Architecture:** File tools move to a new `tools/files.rs`; one walker in `tools/walk.rs` (built on the `ignore` crate) feeds both list and search and filters every path through the existing `Policy::check_path`. The output cap lives in `agent.rs::tool_result`, so built-in tools, custom tools and verify output all pass through it. CI is one reusable workflow that `release.yml` calls before building.

**Tech Stack:** Rust 2021, `ignore` 0.4, `regex` 1, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-11-09-file-tools-and-ci-design.md`

---

## Before you start

Baseline on `file-tools-and-ci` at `9a531f1` (measured 2026-11-09): `cargo clippy --all-targets -- -D warnings` is clean and `cargo test` passes 46 tests. The spec's "fix existing clippy warnings first" step is therefore empty.

Things about this codebase you need to know:

- Every tool implements `Tool` in `crates/core/src/tools/mod.rs`. Permission checks happen in the tool loop (`agent.rs`) **before** `run`; `run` only does the work.
- `Tool::path_arg` names the argument the file rules apply to. `Policy::check` only checks it when the argument is a string.
- `crate::paths::open(root, path, write)` is the only way file content is opened. It refuses symlinks and hard links and, with `write = true`, truncates the file. `read_project_file` in `tools/mod.rs` wraps it with the 1 MiB limit.
- `crate::permissions::glob` is the only glob dialect: `*` stays inside one path segment, `**` crosses segments. Globs match the **project-relative path**, so `*.rs` matches only top-level files and `**/*.rs` matches all of them.
- Tests build a throwaway project with `crate::test_support::TestDir`, then `Toolbox::load(dir.path())` and `Policy::load(dir.path(), None)`. `Policy::default()` has an empty root and must not be used for path tests.
- The code style is dense: one-line trait methods, long lines. Match it; do not run `cargo fmt` over existing files.

Two places where this plan goes beyond the spec text, both needed to make the spec work:

1. **`.` as a path.** `paths::resolve` rejects `.` ("a file path is required"), so `list_files` with `path: "."` would be denied by the policy. Task 2 makes the file rules accept the project root for reads. `read_file` on `.` still fails when it opens the path.
2. **CI trigger.** The spec says "on push and pull request". The plan uses push to `main` plus every pull request, so a branch with an open PR does not run twice and a `v*` tag does not run CI once directly and once through `release.yml`.

## File structure

| File | Change | Responsibility |
|---|---|---|
| `Cargo.toml`, `crates/core/Cargo.toml`, `Cargo.lock` | modify | add `ignore`, `regex` |
| `crates/core/src/tools/files.rs` | create | `ReadFile`, `WriteFile`, `ListFiles`, `SearchFiles`, `EditFile`, argument helpers |
| `crates/core/src/tools/walk.rs` | create | `readable_files`: the one project walk |
| `crates/core/src/tools/builtin.rs` | modify | keeps `all()` and `LoadSkill` |
| `crates/core/src/tools/mod.rs` | modify | declare modules, doc comment |
| `crates/core/src/paths.rs` | modify | `is_root` |
| `crates/core/src/permissions.rs` | modify | `readable_without_approval`, root accepted for reads |
| `crates/core/src/agent.rs` | modify | `MAX_RESULT_BYTES`, `cap` |
| `permissions.json`, `evals/find_definition.json`, `README.md` | modify / create | role grants, eval, docs |
| `.github/workflows/ci.yml`, `.github/workflows/release.yml` | create / modify | CI and release gate |

---

### Task 1: Add dependencies and move the file tools to `files.rs`

A pure refactor: no behavior changes except the wording of the missing-`path` error.

**Files:**
- Modify: `Cargo.toml`, `crates/core/Cargo.toml`
- Create: `crates/core/src/tools/files.rs`
- Modify: `crates/core/src/tools/builtin.rs`, `crates/core/src/tools/mod.rs`

- [ ] **Step 1: Add the crates**

In `Cargo.toml`, add to `[workspace.dependencies]`:

```toml
ignore = "0.4"
regex = "1"
```

In `crates/core/Cargo.toml`, add to `[dependencies]`:

```toml
ignore.workspace = true
regex.workspace = true
```

- [ ] **Step 2: Create `crates/core/src/tools/files.rs`**

```rust
//! Built-in tools that work on project files.

use super::{read_project_file, Access, Context, Effects, Tool};
use crate::error::Result;
use serde_json::Value;
use std::io::Write;

/// A required text argument.
fn required_str<'a>(tool: &str, args: &'a Value, key: &str) -> Result<&'a str> {
    args[key].as_str().ok_or_else(|| format!("{tool} needs \"{key}\"").into())
}

pub struct ReadFile;

impl Tool for ReadFile {
    fn name(&self) -> &str { "read_file" }
    fn description(&self) -> &str { "Read a project file." }
    fn params(&self) -> Vec<(&str, &str)> { vec![("path", "project-relative file path")] }
    fn path_arg(&self) -> Option<(&'static str, Access)> { Some(("path", Access::Read)) }
    fn run(&self, ctx: &Context, args: &Value) -> Result<String> {
        read_project_file(ctx.root, required_str(self.name(), args, "path")?)
    }
}

pub struct WriteFile;

impl Tool for WriteFile {
    fn name(&self) -> &str { "write_file" }
    fn description(&self) -> &str { "Create or overwrite a project file." }
    fn params(&self) -> Vec<(&str, &str)> {
        vec![("path", "project-relative file path"), ("content", "the full new file content")]
    }
    fn effects(&self) -> Effects { Effects { writes_files: true, network: false } }
    fn path_arg(&self) -> Option<(&'static str, Access)> { Some(("path", Access::Write)) }
    fn run(&self, ctx: &Context, args: &Value) -> Result<String> {
        let path = required_str(self.name(), args, "path")?;
        let content = required_str(self.name(), args, "content")?;
        crate::paths::open(ctx.root, path, true)?.write_all(content.as_bytes())?;
        Ok(format!("wrote {} bytes", content.len()))
    }
}
```

- [ ] **Step 3: Replace `crates/core/src/tools/builtin.rs` with**

```rust
//! The list of tools built into the harness, and `load_skill`.

use super::files::{ReadFile, WriteFile};
use super::{Context, Gate, Tool};
use crate::error::Result;
use serde_json::Value;

pub fn all() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(LoadSkill),
        Box::new(ReadFile),
        Box::new(WriteFile),
        Box::new(super::memory::Remember),
        Box::new(super::memory::Forget),
    ]
}

/// Returns a skill's playbook. Granted by the role's `skills` list, not its `tools` list.
pub struct LoadSkill;

impl Tool for LoadSkill {
    fn name(&self) -> &str { "load_skill" }
    fn description(&self) -> &str { "Load an allowed skill's playbook." }
    fn params(&self) -> Vec<(&str, &str)> { vec![("name", "the skill's name")] }
    fn gate(&self) -> Gate { Gate::Skill { arg: "name" } }
    fn trusted_output(&self) -> bool { true }
    fn run(&self, ctx: &Context, args: &Value) -> Result<String> {
        let wanted = args["name"].as_str().ok_or("load_skill needs {\"name\": ...}")?;
        let skill = ctx.skills.iter().find(|s| s.name == wanted).ok_or_else(|| format!("no skill `{wanted}`"))?;
        skill.read(&ctx.layout.company)
    }
}
```

- [ ] **Step 4: Declare the module**

In `crates/core/src/tools/mod.rs`, change

```rust
mod builtin;
mod custom;
```

to

```rust
mod builtin;
mod custom;
mod files;
```

- [ ] **Step 5: Verify nothing changed**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: 46 tests pass in `uritheking-core`, clippy prints no warnings. `Cargo.lock` now lists `ignore` and `regex`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/core/Cargo.toml crates/core/src/tools
git commit -m "Move file tools to tools/files.rs and add ignore and regex"
```

---

### Task 2: `Policy::readable_without_approval`, and the project root as a readable path

**Files:**
- Modify: `crates/core/src/paths.rs`, `crates/core/src/permissions.rs`

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `crates/core/src/permissions.rs`:

```rust
    #[test]
    fn protected_paths_are_not_readable_without_approval() {
        let dir = crate::test_support::TestDir::new();
        std::fs::write(dir.path().join("permissions.json"),
            r#"{"mode":"yolo","files":{"deny":["secrets/**"],"approve":[".env"]}}"#).unwrap();
        let p = Policy::load(dir.path(), None).unwrap();
        assert!(p.readable_without_approval("src/main.rs"));
        assert!(!p.readable_without_approval("secrets/key.txt"));
        assert!(!p.readable_without_approval(".env"));
        assert!(!p.readable_without_approval("../outside"));
        // The project root is a valid place to list and search, but never to write.
        assert!(p.readable_without_approval("."));
        assert!(p.check_path(".", Access::Write).is_some());
    }
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test -p uritheking-core protected_paths_are_not_readable`
Expected: compile error, `no method named readable_without_approval`.

- [ ] **Step 3: Implement**

In `crates/core/src/paths.rs`, add above `pub fn resolve`:

```rust
/// Whether `path` names the project folder itself (`.`, `./` or nothing).
pub fn is_root(path: &str) -> bool {
    Path::new(path).components().all(|c| c == Component::CurDir)
}
```

In `crates/core/src/permissions.rs`, add this method to `impl Policy` right above `fn check_path`:

```rust
    /// Whether a tool may read `path` without asking: not denied and not behind an approval glob.
    /// Listing and searching use it to leave protected files out instead of prompting per file.
    pub fn readable_without_approval(&self, path: &str) -> bool {
        self.check_path(path, Access::Read).is_none()
    }
```

and make these the first lines of `fn check_path`:

```rust
        // The project folder itself: fine to list or search; opening it as a file fails later.
        if access == Access::Read && crate::paths::is_root(path) { return None; }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p uritheking-core permissions`
Expected: all `permissions::tests` pass, including the new one.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/paths.rs crates/core/src/permissions.rs
git commit -m "Add Policy::readable_without_approval and accept the project root for reads"
```

---

### Task 3: The project walk and `list_files`

The walk has no caller until `list_files` exists, so they land together (a lone `walk.rs` would fail clippy's dead-code check).

**Files:**
- Create: `crates/core/src/tools/walk.rs`
- Modify: `crates/core/src/tools/files.rs`, `crates/core/src/tools/builtin.rs`, `crates/core/src/tools/mod.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/core/src/tools/files.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::permissions::Policy;
    use crate::test_support::TestDir;
    use crate::tools::Toolbox;
    use serde_json::{json, Value};

    /// A project with the given files; parent folders are created.
    fn project(files: &[(&str, &str)]) -> TestDir {
        let dir = TestDir::new();
        for (path, content) in files {
            let target = dir.path().join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, content).unwrap();
        }
        dir
    }

    fn call(dir: &TestDir, tool: &str, args: Value) -> crate::error::Result<String> {
        Toolbox::load(dir.path()).call(tool, &args, &Policy::load(dir.path(), None).unwrap())
    }

    const PROTECTED: &str = r#"{"mode":"yolo","files":{"deny":["secrets/**"],"approve":[".env"]}}"#;

    #[test]
    fn list_files_skips_ignored_folders_and_git() {
        let dir = project(&[
            ("src/main.rs", ""), ("README.md", ""), (".gitignore", "target/\n"),
            ("target/debug/app", ""), (".git/config", ""),
        ]);
        assert_eq!(call(&dir, "list_files", json!({})).unwrap(), ".gitignore\nREADME.md\nsrc/main.rs");
        assert_eq!(call(&dir, "list_files", json!({"path": "."})).unwrap(), ".gitignore\nREADME.md\nsrc/main.rs");
        assert_eq!(call(&dir, "list_files", json!({"path": "src"})).unwrap(), "src/main.rs");
        assert_eq!(call(&dir, "list_files", json!({"glob": "**/*.md"})).unwrap(), "README.md");
        assert_eq!(call(&dir, "list_files", json!({"glob": "**/*.py"})).unwrap(), "no files");
    }

    #[test]
    fn list_files_respects_gitignore_when_starting_in_a_subfolder() {
        let dir = project(&[(".gitignore", "*.log\n"), ("src/a.rs", ""), ("src/debug.log", "")]);
        assert_eq!(call(&dir, "list_files", json!({"path": "src"})).unwrap(), "src/a.rs");
    }

    #[test]
    fn list_files_hides_protected_files() {
        let dir = project(&[("permissions.json", PROTECTED), (".env", "TOKEN=1"), ("secrets/key.txt", "k"), ("app.txt", "")]);
        assert_eq!(call(&dir, "list_files", json!({})).unwrap(), "app.txt\npermissions.json");
        assert_eq!(call(&dir, "list_files", json!({"path": "secrets"})).unwrap(), "no files");
    }

    #[cfg(unix)]
    #[test]
    fn list_files_does_not_follow_symlinks() {
        let outside = project(&[("secret.txt", "TOKEN"), ("folder/inner.txt", "TOKEN")]);
        let dir = project(&[("real.txt", "")]);
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), dir.path().join("link.txt")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("folder"), dir.path().join("linked")).unwrap();
        assert_eq!(call(&dir, "list_files", json!({})).unwrap(), "real.txt");
        assert!(call(&dir, "list_files", json!({"path": "linked"})).is_err());
    }

    #[test]
    fn list_files_stops_at_the_limit_and_rejects_bad_arguments() {
        let dir = TestDir::new();
        for n in 0..505 { std::fs::write(dir.path().join(format!("f{n:03}.txt")), "").unwrap(); }
        let out = call(&dir, "list_files", json!({})).unwrap();
        assert_eq!(out.lines().count(), 501);
        assert!(out.starts_with("f000.txt\n"), "{out}");
        assert!(out.ends_with("\n[5 more not shown; pass path or glob to narrow]"), "{out}");

        assert!(call(&dir, "list_files", json!({"path": "f000.txt"})).unwrap_err().to_string().contains("not a folder"));
        assert!(call(&dir, "list_files", json!({"path": "missing"})).unwrap_err().to_string().contains("not a folder"));
        assert!(call(&dir, "list_files", json!({"path": "../x"})).is_err());
        assert!(call(&dir, "list_files", json!({"glob": 3})).unwrap_err().to_string().contains("\"glob\""));
    }
}
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p uritheking-core list_files`
Expected: 5 tests fail with `no tool \`list_files\``.

- [ ] **Step 3: Create `crates/core/src/tools/walk.rs`**

```rust
//! Walks the project for the tools that look at many files at once.

use crate::error::Result;
use crate::permissions::Policy;
use ignore::WalkBuilder;
use std::path::Path;

/// Project-relative paths of regular files under `start` that the role may read without approval,
/// sorted. Git-ignored files, `.git/`, symlinks and protected files are left out.
pub(crate) fn readable_files(root: &Path, start: &str, glob: Option<&str>, policy: &Policy) -> Result<Vec<String>> {
    let scope = if crate::paths::is_root(start) { root.to_path_buf() } else { crate::paths::resolve(root, start)? };
    if !scope.is_dir() { return Err(format!("`{start}` is not a folder").into()); }
    // Always walk from the root, descending only towards `scope`, so the root's ignore files
    // apply in a subfolder too and nothing above the project is ever consulted.
    let walker = WalkBuilder::new(root)
        .follow_links(false)
        .hidden(false)
        .parents(false)
        .require_git(false)
        .git_global(false)
        .filter_entry(move |entry| {
            entry.file_name() != ".git" && (entry.path().starts_with(&scope) || scope.starts_with(entry.path()))
        })
        .build();
    let mut files = Vec::new();
    // Entries that can't be read are skipped; the rest of the walk continues.
    for entry in walker.flatten() {
        // A symlink's own type is "symlink", so links are dropped here along with folders.
        if !entry.file_type().is_some_and(|t| t.is_file()) { continue; }
        let Some(path) = entry.path().strip_prefix(root).ok().and_then(Path::to_str) else { continue };
        if glob.is_some_and(|g| !crate::permissions::glob(g, path)) { continue; }
        if policy.readable_without_approval(path) { files.push(path.to_string()); }
    }
    files.sort();
    Ok(files)
}
```

In `crates/core/src/tools/mod.rs`, add `mod walk;` after `mod skill;`.

- [ ] **Step 4: Add `ListFiles` to `files.rs`**

Change the imports at the top of `crates/core/src/tools/files.rs` to:

```rust
use super::walk::readable_files;
use super::{read_project_file, Access, Context, Effects, Tool};
use crate::error::Result;
use serde_json::Value;
use std::io::Write;

/// Most entries `list_files` returns.
const MAX_LISTED: usize = 500;

const PATH_HELP: &str = "project-relative folder to look in (optional; default: the whole project)";
const GLOB_HELP: &str = "only files whose project-relative path matches, e.g. **/*.rs (optional; * stays in one folder, ** crosses folders)";
```

Add below `required_str`:

```rust
/// An optional text argument; anything but text or nothing is an error.
fn optional_str<'a>(tool: &str, args: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match &args[key] {
        Value::Null => Ok(None),
        Value::String(text) => Ok(Some(text)),
        _ => Err(format!("{tool}: \"{key}\" must be text").into()),
    }
}
```

Add after `WriteFile` (before `mod tests`):

```rust
pub struct ListFiles;

impl Tool for ListFiles {
    fn name(&self) -> &str { "list_files" }
    fn description(&self) -> &str { "List project files. Git-ignored and protected files are left out." }
    fn params(&self) -> Vec<(&str, &str)> { vec![("path", PATH_HELP), ("glob", GLOB_HELP)] }
    fn path_arg(&self) -> Option<(&'static str, Access)> { Some(("path", Access::Read)) }
    fn run(&self, ctx: &Context, args: &Value) -> Result<String> {
        let start = optional_str(self.name(), args, "path")?.unwrap_or(".");
        let files = readable_files(ctx.root, start, optional_str(self.name(), args, "glob")?, ctx.policy)?;
        if files.is_empty() { return Ok("no files".into()); }
        let mut out = files[..files.len().min(MAX_LISTED)].join("\n");
        if files.len() > MAX_LISTED {
            out.push_str(&format!("\n[{} more not shown; pass path or glob to narrow]", files.len() - MAX_LISTED));
        }
        Ok(out)
    }
}
```

- [ ] **Step 5: Register it**

In `crates/core/src/tools/builtin.rs`, change the import to `use super::files::{ListFiles, ReadFile, WriteFile};` and add `Box::new(ListFiles),` after `Box::new(ReadFile),` in `all()`.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p uritheking-core && cargo clippy --workspace --all-targets -- -D warnings`
Expected: 52 tests pass in `uritheking-core` (46 + 1 from Task 2 + 5 here), no warnings.

If `list_files_skips_ignored_folders_and_git` lists `target/debug/app`, the `.gitignore` is not being honored: check that `.require_git(false)` is set, since the test project is not a git repository.

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/tools
git commit -m "Add list_files and the shared project walk"
```

---

### Task 4: `search_files`

**Files:**
- Modify: `crates/core/src/tools/files.rs`, `crates/core/src/tools/builtin.rs`

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` in `crates/core/src/tools/files.rs`:

```rust
    #[test]
    fn search_files_returns_path_line_and_text() {
        let dir = project(&[("src/agent.rs", "use x;\nconst MAX_STEPS: usize = 10;\n"), ("notes.md", "MAX_STEPS is ten\n")]);
        assert_eq!(call(&dir, "search_files", json!({"pattern": "MAX_STEPS", "glob": "**/*.rs"})).unwrap(),
            "src/agent.rs:2: const MAX_STEPS: usize = 10;");
        assert_eq!(call(&dir, "search_files", json!({"pattern": "^MAX_\\w+ is"})).unwrap(), "notes.md:1: MAX_STEPS is ten");
        assert_eq!(call(&dir, "search_files", json!({"pattern": "MAX_STEPS", "path": "src"})).unwrap(),
            "src/agent.rs:2: const MAX_STEPS: usize = 10;");
        assert_eq!(call(&dir, "search_files", json!({"pattern": "nowhere"})).unwrap(), "no matches");
    }

    #[cfg(unix)]
    #[test]
    fn search_files_never_reads_protected_or_linked_files() {
        let outside = project(&[("secret.txt", "TOKEN outside")]);
        let dir = project(&[("permissions.json", PROTECTED), (".env", "TOKEN=1"), ("secrets/key.txt", "TOKEN"), ("app.txt", "TOKEN here")]);
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), dir.path().join("link.txt")).unwrap();
        assert_eq!(call(&dir, "search_files", json!({"pattern": "TOKEN"})).unwrap(), "app.txt:1: TOKEN here");
    }

    #[test]
    fn search_files_limits_matches_and_line_length_and_skips_unreadable_files() {
        let dir = project(&[("many.txt", &"hit\n".repeat(250)), ("wide.txt", &format!("wide {}\n", "x".repeat(400)))]);
        std::fs::write(dir.path().join("binary.bin"), [0xff, 0xfe, b'h', b'i', b't']).unwrap();
        std::fs::write(dir.path().join("huge.txt"), "hit\n".repeat(300_000)).unwrap();

        let out = call(&dir, "search_files", json!({"pattern": "hit"})).unwrap();
        assert_eq!(out.lines().count(), 201);
        assert!(out.starts_with("many.txt:1: hit\n"), "binary and oversized files are skipped: {out}");
        assert!(out.ends_with("many.txt:200: hit\n[more matches exist; showing the first 200. Narrow the pattern, path or glob]"), "{out}");

        let wide = call(&dir, "search_files", json!({"pattern": "wide"})).unwrap();
        assert_eq!(wide.len(), "wide.txt:1: ".len() + 300);
    }

    #[test]
    fn search_files_rejects_bad_arguments_before_reading() {
        let dir = project(&[("a.txt", "x")]);
        assert!(call(&dir, "search_files", json!({"pattern": "("})).unwrap_err().to_string().starts_with("invalid pattern"));
        assert!(call(&dir, "search_files", json!({})).unwrap_err().to_string().contains("\"pattern\""));
        assert!(call(&dir, "search_files", json!({"pattern": "x", "path": "a.txt"})).unwrap_err().to_string().contains("not a folder"));
    }
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p uritheking-core search_files`
Expected: 4 tests fail with `no tool \`search_files\``.

- [ ] **Step 3: Implement**

In `crates/core/src/tools/files.rs`, add under `MAX_LISTED`:

```rust
/// Most matching lines `search_files` returns.
const MAX_MATCHES: usize = 200;
/// Longest matching line shown, in characters.
const MAX_MATCH_CHARS: usize = 300;
```

Add after `ListFiles`:

```rust
pub struct SearchFiles;

impl Tool for SearchFiles {
    fn name(&self) -> &str { "search_files" }
    fn description(&self) -> &str {
        "Search project files for a regular expression, line by line. Git-ignored and protected files are left out."
    }
    fn params(&self) -> Vec<(&str, &str)> {
        vec![("pattern", "regular expression to look for"), ("path", PATH_HELP), ("glob", GLOB_HELP)]
    }
    fn path_arg(&self) -> Option<(&'static str, Access)> { Some(("path", Access::Read)) }
    fn run(&self, ctx: &Context, args: &Value) -> Result<String> {
        // The pattern is checked first, so a bad one fails before any file is opened.
        let pattern = regex::Regex::new(required_str(self.name(), args, "pattern")?).map_err(|e| format!("invalid pattern: {e}"))?;
        let start = optional_str(self.name(), args, "path")?.unwrap_or(".");
        let files = readable_files(ctx.root, start, optional_str(self.name(), args, "glob")?, ctx.policy)?;
        let mut matches = Vec::new();
        'files: for path in &files {
            // Oversized, non-UTF-8 and unreadable files are skipped; the search goes on.
            let Ok(content) = read_project_file(ctx.root, path) else { continue };
            for (index, line) in content.lines().enumerate() {
                if !pattern.is_match(line) { continue; }
                if matches.len() == MAX_MATCHES {
                    matches.push(format!("[more matches exist; showing the first {MAX_MATCHES}. Narrow the pattern, path or glob]"));
                    break 'files;
                }
                let text: String = line.chars().take(MAX_MATCH_CHARS).collect();
                matches.push(format!("{path}:{}: {text}", index + 1));
            }
        }
        if matches.is_empty() { return Ok("no matches".into()); }
        Ok(matches.join("\n"))
    }
}
```

In `crates/core/src/tools/builtin.rs`, import `SearchFiles` too and add `Box::new(SearchFiles),` after `Box::new(ListFiles),`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p uritheking-core search_files`
Expected: 4 pass.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/tools
git commit -m "Add search_files"
```

---

### Task 5: `edit_file`

**Files:**
- Modify: `crates/core/src/tools/files.rs`, `crates/core/src/tools/builtin.rs`

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` in `crates/core/src/tools/files.rs`:

```rust
    fn read(dir: &TestDir, path: &str) -> String { std::fs::read_to_string(dir.path().join(path)).unwrap() }

    #[test]
    fn edit_file_replaces_one_place() {
        let dir = project(&[("notes.txt", "hello world")]);
        assert_eq!(call(&dir, "edit_file", json!({"path": "notes.txt", "old": "world", "new": "team"})).unwrap(), "replaced 1 occurrence(s)");
        assert_eq!(read(&dir, "notes.txt"), "hello team");
        // A shorter result must not leave the old tail behind.
        call(&dir, "edit_file", json!({"path": "notes.txt", "old": "hello team", "new": "hi"})).unwrap();
        assert_eq!(read(&dir, "notes.txt"), "hi");
    }

    #[test]
    fn edit_file_changes_nothing_when_the_target_is_ambiguous_or_missing() {
        let dir = project(&[("notes.txt", "foo and foo")]);
        let twice = call(&dir, "edit_file", json!({"path": "notes.txt", "old": "foo", "new": "baz"})).unwrap_err().to_string();
        assert!(twice.contains("found 2 times"), "{twice}");
        let never = call(&dir, "edit_file", json!({"path": "notes.txt", "old": "bar", "new": "baz"})).unwrap_err().to_string();
        assert!(never.contains("found 0 times"), "{never}");
        assert_eq!(read(&dir, "notes.txt"), "foo and foo");

        assert_eq!(call(&dir, "edit_file", json!({"path": "notes.txt", "old": "foo", "new": "baz", "replace_all": true})).unwrap(), "replaced 2 occurrence(s)");
        assert_eq!(read(&dir, "notes.txt"), "baz and baz");
    }

    #[test]
    fn edit_file_rejects_bad_arguments_without_touching_the_file() {
        let dir = project(&[("notes.txt", "foo")]);
        for (args, expected) in [
            (json!({"old": "foo", "new": "x"}), "\"path\""),
            (json!({"path": "notes.txt", "new": "x"}), "\"old\""),
            (json!({"path": "notes.txt", "old": "foo"}), "\"new\""),
            (json!({"path": "notes.txt", "old": "", "new": "x"}), "must not be empty"),
            (json!({"path": "notes.txt", "old": "foo", "new": "foo"}), "must differ"),
            (json!({"path": "notes.txt", "old": "foo", "new": "x", "replace_all": "yes"}), "\"replace_all\""),
        ] {
            let error = call(&dir, "edit_file", args).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
        assert!(call(&dir, "edit_file", json!({"path": "missing.txt", "old": "a", "new": "b"})).is_err());
        assert!(!dir.path().join("missing.txt").exists(), "a failed edit must not create the file");
        assert_eq!(read(&dir, "notes.txt"), "foo");
    }

    #[test]
    fn edit_file_follows_the_write_rules() {
        use crate::permissions::Decision;
        let dir = project(&[("permissions.json", r#"{"mode":"yolo"}"#)]);
        let (tb, policy) = (Toolbox::load(dir.path()), Policy::load(dir.path(), None).unwrap());
        let edit = tb.get("edit_file").unwrap();
        assert!(matches!(policy.check(edit, &json!({"path": "permissions.json"})), Decision::Ask(_)));
        assert!(matches!(policy.check(edit, &json!({"path": "logs/audit.jsonl"})), Decision::Deny(_)));
        assert_eq!(policy.check(edit, &json!({"path": "notes.txt"})), Decision::Allow);
        // Counting as a write is what triggers verify and the manual yes after untrusted output.
        assert!(edit.effects().writes_files);
    }
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p uritheking-core edit_file`
Expected: 4 tests fail (`no tool \`edit_file\``, and a panic on `tb.get("edit_file").unwrap()`).

- [ ] **Step 3: Implement**

In `crates/core/src/tools/files.rs`, add below `optional_str`:

```rust
/// An optional true/false argument, false when left out.
fn flag(tool: &str, args: &Value, key: &str) -> Result<bool> {
    match &args[key] {
        Value::Null => Ok(false),
        Value::Bool(on) => Ok(*on),
        _ => Err(format!("{tool}: \"{key}\" must be true or false").into()),
    }
}
```

Add after `SearchFiles`:

```rust
pub struct EditFile;

impl Tool for EditFile {
    fn name(&self) -> &str { "edit_file" }
    fn description(&self) -> &str {
        "Replace text in a project file. Fails without changing anything unless `old` is found exactly once, or replace_all is true."
    }
    fn params(&self) -> Vec<(&str, &str)> {
        vec![
            ("path", "project-relative file path"),
            ("old", "the exact text to replace, with enough surrounding text to be unique"),
            ("new", "the text to put in its place"),
            ("replace_all", "true to replace every occurrence (optional; default false)"),
        ]
    }
    fn effects(&self) -> Effects { Effects { writes_files: true, network: false } }
    fn path_arg(&self) -> Option<(&'static str, Access)> { Some(("path", Access::Write)) }
    fn run(&self, ctx: &Context, args: &Value) -> Result<String> {
        let name = self.name();
        let path = required_str(name, args, "path")?;
        let (old, new) = (required_str(name, args, "old")?, required_str(name, args, "new")?);
        let replace_all = flag(name, args, "replace_all")?;
        if old.is_empty() { return Err("edit_file: \"old\" must not be empty".into()); }
        if old == new { return Err("edit_file: \"old\" and \"new\" must differ".into()); }
        // Read and check first: opening for writing empties the file, so it happens only once the edit is known to apply.
        let content = read_project_file(ctx.root, path)?;
        let found = content.matches(old).count();
        if found == 0 || (found > 1 && !replace_all) {
            return Err(format!("\"old\" was found {found} times in `{path}`, so nothing was changed. \
                It must match exactly once; add surrounding text, or pass replace_all to change every occurrence").into());
        }
        crate::paths::open(ctx.root, path, true)?.write_all(content.replace(old, new).as_bytes())?;
        Ok(format!("replaced {found} occurrence(s)"))
    }
}
```

(`replace` is correct for both modes: without `replace_all`, `found` is exactly 1 here.)

In `crates/core/src/tools/builtin.rs`, import `EditFile` too and add `Box::new(EditFile),` after `Box::new(WriteFile),`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p uritheking-core edit_file`
Expected: 4 pass.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/tools
git commit -m "Add edit_file"
```

---

### Task 6: Paging for `read_file`

**Files:**
- Modify: `crates/core/src/tools/files.rs`

- [ ] **Step 1: Write the failing test**

Add inside `mod tests` in `crates/core/src/tools/files.rs`:

```rust
    #[test]
    fn read_file_pages_through_long_files() {
        let long: String = (1..=5000).map(|n| format!("line {n}\n")).collect();
        let dir = project(&[("long.txt", &long), ("short.txt", "one\ntwo\n")]);

        let first = call(&dir, "read_file", json!({"path": "long.txt"})).unwrap();
        assert!(first.starts_with("line 1\nline 2\n"), "{}", &first[..40]);
        assert!(first.ends_with("line 2000\n[lines 1-2000 of 5000; pass offset to read more]"));

        let second = call(&dir, "read_file", json!({"path": "long.txt", "offset": 2001})).unwrap();
        assert!(second.starts_with("line 2001\n"));
        assert!(second.ends_with("line 4000\n[lines 2001-4000 of 5000; pass offset to read more]"));

        let last = call(&dir, "read_file", json!({"path": "long.txt", "offset": 4001})).unwrap();
        assert!(last.starts_with("line 4001\n") && last.ends_with("line 5000"), "no note once the end is reached");

        assert_eq!(call(&dir, "read_file", json!({"path": "long.txt", "offset": 2, "limit": 2})).unwrap(),
            "line 2\nline 3\n[lines 2-3 of 5000; pass offset to read more]");
        // A file that fits comes back exactly as it is on disk.
        assert_eq!(call(&dir, "read_file", json!({"path": "short.txt"})).unwrap(), "one\ntwo\n");

        for (args, expected) in [
            (json!({"path": "long.txt", "offset": 0}), "\"offset\""),
            (json!({"path": "long.txt", "offset": "x"}), "\"offset\""),
            (json!({"path": "long.txt", "limit": -1}), "\"limit\""),
            (json!({"path": "long.txt", "offset": 5001}), "past the end"),
        ] {
            let error = call(&dir, "read_file", args).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
    }
```

- [ ] **Step 2: Run it to make sure it fails**

Run: `cargo test -p uritheking-core read_file_pages`
Expected: FAIL on the first `ends_with` assertion (the whole file comes back).

- [ ] **Step 3: Implement**

In `crates/core/src/tools/files.rs`, add under the other constants:

```rust
/// Lines `read_file` returns when no `limit` is given.
const READ_LINES: usize = 2000;
```

Add below `flag`:

```rust
/// An optional whole number of 1 or more.
fn count(tool: &str, args: &Value, key: &str, default: usize) -> Result<usize> {
    match &args[key] {
        Value::Null => Ok(default),
        value => value.as_u64().filter(|n| *n >= 1).map(|n| n as usize)
            .ok_or_else(|| format!("{tool}: \"{key}\" must be a whole number, 1 or more").into()),
    }
}
```

Replace the `impl Tool for ReadFile` block with:

```rust
impl Tool for ReadFile {
    fn name(&self) -> &str { "read_file" }
    fn description(&self) -> &str { "Read a project file, up to 2000 lines at a time." }
    fn params(&self) -> Vec<(&str, &str)> {
        vec![
            ("path", "project-relative file path"),
            ("offset", "first line to return, counting from 1 (optional; default 1)"),
            ("limit", "how many lines to return (optional; default 2000)"),
        ]
    }
    fn path_arg(&self) -> Option<(&'static str, Access)> { Some(("path", Access::Read)) }
    fn run(&self, ctx: &Context, args: &Value) -> Result<String> {
        let name = self.name();
        let path = required_str(name, args, "path")?;
        let (offset, limit) = (count(name, args, "offset", 1)?, count(name, args, "limit", READ_LINES)?);
        let content = read_project_file(ctx.root, path)?;
        let total = content.lines().count();
        // A file that fits is returned untouched, final newline included.
        if offset == 1 && total <= limit { return Ok(content); }
        if offset > total { return Err(format!("read_file: offset {offset} is past the end of `{path}` ({total} lines)").into()); }
        let last = (offset - 1).saturating_add(limit).min(total);
        let mut page = content.lines().skip(offset - 1).take(limit).collect::<Vec<_>>().join("\n");
        if last < total { page.push_str(&format!("\n[lines {offset}-{last} of {total}; pass offset to read more]")); }
        Ok(page)
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p uritheking-core`
Expected: everything passes, including the older `agent` and `eval` tests that call `read_file`.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/tools/files.rs
git commit -m "Page read_file at 2000 lines"
```

---

### Task 7: Cap every tool result at 64 KiB

**Files:**
- Modify: `crates/core/src/agent.rs`

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` in `crates/core/src/agent.rs`, after the `project` helper:

```rust
    #[test]
    fn cap_cuts_at_a_character_boundary() {
        assert_eq!(cap("short"), "short");
        assert_eq!(cap(&"a".repeat(MAX_RESULT_BYTES)).len(), MAX_RESULT_BYTES);
        // "a" then two-byte characters: byte 65536 falls inside one, so the cut moves back to 65535.
        let text = format!("a{}", "é".repeat(40_000));
        assert!(cap(&text).ends_with("é\n[output cut: 65535 of 80001 bytes shown]"));
    }

    #[test]
    fn oversized_tool_output_is_cut_before_it_enters_history() {
        let dir = project(r#"{"mode":"yolo"}"#);
        std::fs::write(dir.path().join("big.txt"), "a".repeat(512_000)).unwrap();
        let (tb, policy) = (Toolbox::load(dir.path()), Policy::load(dir.path(), None).unwrap());
        let provider = script(&[r#"<tool_call>{"name":"read_file","args":{"path":"big.txt"}}</tool_call>"#, "done"]);
        let mut history = vec![Message::user("read it")];
        run_turn(&provider, &tb, &policy, &mut history, |_| {}, |_, _, _| true).unwrap();
        let result = &history[2].content;
        assert!(result.ends_with("a\n[output cut: 65536 of 512000 bytes shown]\n</tool_result>"), "{}", &result[result.len() - 80..]);
        assert!(result.len() < MAX_RESULT_BYTES + 200);

        // The audit log still records the size of what the tool really returned.
        let log = std::fs::read_to_string(crate::audit::path(dir.path())).unwrap();
        assert!(log.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).any(|e| e["event"] == "result" && e["bytes"] == 512_000), "{log}");
    }
```

- [ ] **Step 2: Run them to make sure they fail**

Run: `cargo test -p uritheking-core agent::tests`
Expected: compile error, `cannot find function cap` / `MAX_RESULT_BYTES`.

- [ ] **Step 3: Implement**

In `crates/core/src/agent.rs`, add under `MAX_VERIFY`:

```rust
/// Largest tool result that enters history; longer output is cut so one call can't fill the context window.
const MAX_RESULT_BYTES: usize = 64 * 1024;
```

Replace `fn tool_result` with:

```rust
/// Output short enough for history, with a note saying how much was left out.
fn cap(output: &str) -> std::borrow::Cow<'_, str> {
    if output.len() <= MAX_RESULT_BYTES { return output.into(); }
    let mut end = MAX_RESULT_BYTES;
    while !output.is_char_boundary(end) { end -= 1; }
    format!("{}\n[output cut: {end} of {} bytes shown]", &output[..end], output.len()).into()
}

/// A result block for the model. `trusted` is for harness-written text only.
fn tool_result(name: &str, output: &str, trusted: bool) -> Message {
    // Cut first, so a cut can never split one of the tags `neutralize` writes.
    Message::tool_result(format!("<tool_result name=\"{name}\">\n{}\n</tool_result>", neutralize(&cap(output))), trusted)
}
```

Nothing else changes: `execute` and `accept` already audit `output.len()` of the full output, and both tool results and verify output reach history only through `tool_result`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p uritheking-core agent::tests`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/agent.rs
git commit -m "Cut tool results over 64 KiB before they enter history"
```

---

### Task 8: Role grants, eval and documentation

**Files:**
- Modify: `permissions.json`, `README.md`, `crates/core/src/tools/mod.rs`, `crates/core/src/permissions.rs`
- Create: `evals/find_definition.json`

- [ ] **Step 1: Grant the tools to `art`**

In `permissions.json`, replace the `art` line with:

```json
    "art":   { "tools": ["read_file", "write_file", "list_files", "search_files", "edit_file"], "skills": ["*"] }
```

- [ ] **Step 2: Create `evals/find_definition.json`**

```json
{
  "prompt": "Which file defines the constant MAX_STEPS? Answer with the file path.",
  "role": "dev",
  "expect": {
    "tools": ["search_files"],
    "no_tools": ["write_file", "edit_file"],
    "reply_contains": ["agent.rs"]
  }
}
```

- [ ] **Step 3: Update code comments**

In `crates/core/src/tools/mod.rs`, replace the two doc lines starting `//! Built-in tools (` with:

```rust
//! Built-in tools (`read_file`, `list_files`, `search_files`, `write_file`, `edit_file`, `load_skill`,
//! `remember`, `forget`) and custom tools share one [`Tool`] trait, so permissions, prompts and the
//! tool loop treat them the same way.
```

In `crates/core/src/permissions.rs`, replace the two doc lines starting `//! Built in, whatever the file says:` with:

```rust
//! Built in, whatever the file says: `write_file` and `edit_file` may never touch `sessions/`, `logs/`
//! or `memory/`, and writes to `permissions.json`, `COMPANY.md`, `tools/` and `skills/` always need a
//! manual yes. `list_files` and `search_files` leave out every file matching `deny` or `approve`.
```

- [ ] **Step 4: Update `README.md`**

Replace

```
Adding a built-in tool = one struct implementing `Tool` in `crates/core/src/tools/builtin.rs`,
listed in `builtin::all`.
```

with

```
Adding a built-in tool = one struct implementing `Tool` in `crates/core/src/tools/` (file tools
live in `files.rs`), listed in `builtin::all`.
```

In the folders table, replace `your code; the only place `read_file` / `write_file` work` with `your code; the only place the file tools work`.

Replace the line `Built-in tools: `read_file`, `write_file`, `load_skill`, `remember`, `forget`.` with:

```
Built-in tools: `read_file`, `list_files`, `search_files`, `write_file`, `edit_file`, `load_skill`, `remember`, `forget`.

**Find files.** `list_files` returns project-relative paths, sorted, one per line. `path` limits it to a folder and `glob` to matching paths (`*` stays in one folder, `**` crosses folders, so `**/*.rs` is every Rust file). Files ignored by `.gitignore`, the `.git/` folder and symlinks are never listed. Past 500 entries the result says how many were left out.

**Search files.** `search_files` takes a regular expression in `pattern`, plus the same `path` and `glob`, and returns `path:line: text` for each matching line. It stops at 200 matches and says more exist; lines are cut to 300 characters. Files over 1 MiB or that aren't UTF-8 are skipped. An invalid pattern fails before any file is read.

**Protected files stay hidden.** Files matching `files.deny` or `files.approve` never appear in `list_files` or `search_files` results and are not opened, so a search never stops to ask about `.env`. `read_file` on such a file still asks or is denied as before.

**Edit part of a file.** `edit_file` replaces `old` with `new` in one file and answers `replaced N occurrence(s)`. If `old` is missing, or appears more than once without `replace_all: true`, the call fails, says how many times it was found, and the file is unchanged. It follows every rule `write_file` does.

**Read long files in pages.** `read_file` returns 2,000 lines at a time. When lines remain, the result ends with `[lines 1-2000 of 5000; pass offset to read more]`; call it again with `offset: 2001`. `limit` changes the page size.

**Large results are cut.** Any tool result over 64 KiB (built-in, custom or verify) is cut before the model sees it and ends with `[output cut: 65536 of 512000 bytes shown]`. The audit log still records the full size.
```

In the Permissions list, replace `` `write_file` can never touch `sessions/`, `logs/` or `memory/` `` with `` `write_file` and `edit_file` can never touch `sessions/`, `logs/` or `memory/` ``, and replace `calls that can change things (`write_file`, tools with `writes` or `network`)` with `calls that can change things (`write_file`, `edit_file`, tools with `writes` or `network`)`.

In the "Memory stays safe" paragraph, replace `` `write_file` can never write `memory/` `` with `` `write_file` and `edit_file` can never write `memory/` ``.

- [ ] **Step 5: Verify**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all pass, no warnings. (`bundled_mock_skill_and_tool_work_through_the_tool_loop` loads the edited `permissions.json`, so it also proves the file still parses.)

Run: `python3 -m json.tool evals/find_definition.json > /dev/null && echo ok`
Expected: `ok`

- [ ] **Step 6: Commit**

```bash
git add permissions.json evals/find_definition.json README.md crates/core/src/tools/mod.rs crates/core/src/permissions.rs
git commit -m "Grant and document the new file tools, and add a search eval"
```

---

### Task 9: CI workflow and release gate

**Files:**
- Create: `.github/workflows/ci.yml`
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Create `.github/workflows/ci.yml`**

```yaml
name: ci

on:
  push:
    branches: [main]
  pull_request:
  # Lets release.yml run the same checks before it builds.
  workflow_call:

permissions:
  contents: read

jobs:
  check:
    strategy:
      fail-fast: false
      matrix:
        os: [macos-latest, ubuntu-22.04]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v5
      # The Linux sandbox tests run tools under bubblewrap.
      - name: Install bubblewrap
        if: runner.os == 'Linux'
        run: sudo apt-get update && sudo apt-get install -y bubblewrap
      - run: cargo test --locked --workspace
      - run: cargo clippy --locked --workspace --all-targets -- -D warnings
```

- [ ] **Step 2: Gate the release**

In `.github/workflows/release.yml`, replace

```yaml
jobs:
  build:
    strategy:
```

with

```yaml
jobs:
  test:
    uses: ./.github/workflows/ci.yml

  build:
    needs: test
    strategy:
```

- [ ] **Step 3: Verify locally**

Run: `cargo test --locked --workspace && cargo clippy --locked --workspace --all-targets -- -D warnings`
Expected: pass. `--locked` fails if `Cargo.lock` from Task 1 was not committed.

Run: `ruby -ryaml -e 'ARGV.each { |f| YAML.load_file(f) }; puts "ok"' .github/workflows/ci.yml .github/workflows/release.yml`
Expected: `ok`

The workflows themselves can only be proven on GitHub: after pushing the branch and opening a pull request, the `ci` workflow must show two green `check` jobs. If the Ubuntu job fails in sandbox tests, read the bubblewrap error in the log before changing anything.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows
git commit -m "Add CI and run it before a release is built"
```

---

## Scenario coverage

| Spec scenario | Test |
|---|---|
| List files in a project | `list_files_skips_ignored_folders_and_git` |
| Protected files are hidden | `list_files_hides_protected_files`, `search_files_never_reads_protected_or_linked_files` |
| Symlinks are not followed | `list_files_does_not_follow_symlinks`, `search_files_never_reads_protected_or_linked_files` |
| Search returns matching lines | `search_files_returns_path_line_and_text` |
| Too many search matches | `search_files_limits_matches_and_line_length_and_skips_unreadable_files` |
| Invalid search pattern | `search_files_rejects_bad_arguments_before_reading` |
| Edit one place | `edit_file_replaces_one_place` |
| Ambiguous or missing target | `edit_file_changes_nothing_when_the_target_is_ambiguous_or_missing` |
| Replace every occurrence | same test, `replace_all` part |
| Edit follows the write rules | `edit_file_follows_the_write_rules` |
| Read a long file in pages | `read_file_pages_through_long_files` |
| Oversized output is cut | `oversized_tool_output_is_cut_before_it_enters_history`, `cap_cuts_at_a_character_boundary` |
| CI blocks a broken change; release gate | Task 9; checked on GitHub after the first push |

The oversized-output test uses `read_file` on a 512,000-byte single-line file instead of a custom tool. The cap sits in `tool_result`, which every kind of result passes through, and this keeps the test free of the OS sandbox so it runs everywhere.

//! Built-in tools that work on project files.

use super::walk::readable_files;
use super::{read_project_file, Access, Context, Effects, Tool};
use crate::error::Result;
use serde_json::Value;
use std::io::Write;

/// Most entries `list_files` returns.
const MAX_LISTED: usize = 500;
/// Most matching lines `search_files` returns.
const MAX_MATCHES: usize = 200;
/// Longest matching line shown, in characters.
const MAX_MATCH_CHARS: usize = 300;
/// Lines `read_file` returns when no `limit` is given.
const READ_LINES: usize = 2000;

const PATH_HELP: &str = "project-relative folder to look in (optional; default: the whole project)";
const GLOB_HELP: &str = "only files whose project-relative path matches, e.g. **/*.rs (optional; * stays in one folder, ** crosses folders)";

/// A required text argument.
fn required_str<'a>(tool: &str, args: &'a Value, key: &str) -> Result<&'a str> {
    args[key].as_str().ok_or_else(|| format!("{tool} needs \"{key}\"").into())
}

/// An optional text argument; anything but text or nothing is an error.
fn optional_str<'a>(tool: &str, args: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match &args[key] {
        Value::Null => Ok(None),
        Value::String(text) => Ok(Some(text)),
        _ => Err(format!("{tool}: \"{key}\" must be text").into()),
    }
}

/// An optional true/false argument, false when left out.
fn flag(tool: &str, args: &Value, key: &str) -> Result<bool> {
    match &args[key] {
        Value::Null => Ok(false),
        Value::Bool(on) => Ok(*on),
        _ => Err(format!("{tool}: \"{key}\" must be true or false").into()),
    }
}

/// An optional whole number of 1 or more.
fn count(tool: &str, args: &Value, key: &str, default: usize) -> Result<usize> {
    match &args[key] {
        Value::Null => Ok(default),
        value => value.as_u64().filter(|n| *n >= 1).map(|n| n as usize)
            .ok_or_else(|| format!("{tool}: \"{key}\" must be a whole number, 1 or more").into()),
    }
}

pub struct ReadFile;

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
}

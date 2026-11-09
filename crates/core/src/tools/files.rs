//! Built-in tools that work on project files.

use super::walk::readable_files;
use super::{read_project_file, Access, Context, Effects, Tool};
use crate::error::Result;
use serde_json::Value;
use std::io::Write;

/// Most entries `list_files` returns.
const MAX_LISTED: usize = 500;

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

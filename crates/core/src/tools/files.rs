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

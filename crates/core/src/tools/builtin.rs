//! Tools built into the harness.

use super::{read_project_file, Access, Context, Effects, Gate, Tool};
use crate::error::Result;
use serde_json::Value;
use std::io::Write;

pub fn all() -> Vec<Box<dyn Tool>> {
    vec![Box::new(LoadSkill), Box::new(ReadFile), Box::new(WriteFile), Box::new(super::memory::Remember), Box::new(super::memory::Forget)]
}

fn path_of<'a>(tool: &str, args: &'a Value) -> Result<&'a str> {
    args["path"].as_str().ok_or_else(|| format!("{tool} needs {{\"path\": ...}}").into())
}

pub struct ReadFile;

impl Tool for ReadFile {
    fn name(&self) -> &str { "read_file" }
    fn description(&self) -> &str { "Read a project file." }
    fn params(&self) -> Vec<(&str, &str)> { vec![("path", "project-relative file path")] }
    fn path_arg(&self) -> Option<(&'static str, Access)> { Some(("path", Access::Read)) }
    fn run(&self, ctx: &Context, args: &Value) -> Result<String> {
        read_project_file(ctx.root, path_of(self.name(), args)?)
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
        let path = path_of(self.name(), args)?;
        let content = args["content"].as_str().ok_or("write_file needs \"content\"")?;
        crate::paths::open(ctx.root, path, true)?.write_all(content.as_bytes())?;
        Ok(format!("wrote {} bytes", content.len()))
    }
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

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

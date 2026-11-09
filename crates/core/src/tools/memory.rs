//! `remember` and `forget`: the model's only way to change long-term memory.

use super::{Context, Effects, Tool};
use crate::error::Result;
use crate::memory::{Memory, MemoryStore, Scope};
use serde_json::Value;

const SCOPE_HELP: &str = "personal (default, only this user), team (everyone in the user's role) or company (everyone; owners only). \
Use team or company only when the user explicitly asks to share it.";

/// The scope the call asks for, after the policy agrees.
fn scope(ctx: &Context, args: &Value) -> Result<Scope> {
    let scope = Scope::parse(args["scope"].as_str(), ctx.policy.role.as_deref())?;
    ctx.policy.may_write_memory(&scope)?;
    Ok(scope)
}

/// Shared memory reaches others only through the company repo.
fn share_hint(scope: &Scope) -> &'static str {
    match scope {
        Scope::Personal => "",
        _ => "; commit and push it in the shared memory repo so others get it",
    }
}

fn arg<'a>(tool: &str, args: &'a Value, key: &str) -> Result<&'a str> {
    args[key].as_str().ok_or_else(|| format!("{tool} needs \"{key}\"").into())
}

pub struct Remember;

impl Tool for Remember {
    fn name(&self) -> &str { "remember" }
    fn description(&self) -> &str {
        "Save a lasting fact for future sessions (a user preference, decision or project detail). \
         Reusing a name replaces that memory. Don't save secrets or things already in project files."
    }
    fn params(&self) -> Vec<(&str, &str)> {
        vec![
            ("name", "short kebab-case id, e.g. db-choice"),
            ("description", "one line shown in the memory index"),
            ("content", "the fact, with why it matters"),
            ("scope", SCOPE_HELP),
        ]
    }
    // Counts as a write, so it needs a manual yes after untrusted tool output (injection defense).
    fn effects(&self) -> Effects { Effects { writes_files: true, network: false } }
    fn run(&self, ctx: &Context, args: &Value) -> Result<String> {
        let name = self.name();
        let memory = Memory::new(arg(name, args, "name")?, arg(name, args, "description")?, arg(name, args, "content")?)?;
        let scope = scope(ctx, args)?;
        let replaced = MemoryStore::for_scope(ctx.layout, &scope)?.save(&memory)?;
        Ok(format!("{} {} memory `{}`{}", if replaced { "replaced" } else { "saved" }, scope.label(), memory.name, share_hint(&scope)))
    }
}

pub struct Forget;

impl Tool for Forget {
    fn name(&self) -> &str { "forget" }
    fn description(&self) -> &str { "Delete a saved memory that is wrong or outdated." }
    fn params(&self) -> Vec<(&str, &str)> {
        vec![("name", "the memory's name from the index"), ("scope", "the scope the memory is listed under (default personal)")]
    }
    fn effects(&self) -> Effects { Effects { writes_files: true, network: false } }
    fn run(&self, ctx: &Context, args: &Value) -> Result<String> {
        let name = arg(self.name(), args, "name")?;
        let scope = scope(ctx, args)?;
        MemoryStore::for_scope(ctx.layout, &scope)?.forget(name)?;
        Ok(format!("forgot {} memory `{name}`{}", scope.label(), share_hint(&scope)))
    }
}

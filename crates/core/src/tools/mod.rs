//! Tools the model can call, and the skills and company context loaded with them.
//!
//! Layout (relative to the company folder, see [`crate::layout`]):
//!   tools/<name>/tool.json   — a custom tool; see [`custom`] for the format
//!   COMPANY.md               — company/product context, sent with every call
//!   skills/<name>/SKILL.md   — frontmatter `name:` / `description:`, then instructions
//!
//!   memory/                  — long-term memory (personal, team, company); see [`crate::memory`]
//!
//! Built-in tools (`read_file`, `write_file`, `load_skill`, `remember`, `forget`) and custom tools share one
//! [`Tool`] trait, so permissions, prompts and the tool loop treat them the same way.

mod builtin;
mod custom;
mod files;
mod memory;
mod skill;
mod walk;

pub use custom::CustomTool;
pub use skill::Skill;

use crate::error::Result;
use crate::permissions::Policy;
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Largest project file a tool reads into memory.
const MAX_FILE_BYTES: usize = 1024 * 1024;

/// What a call can do beyond returning text.
#[derive(Clone, Copy, Default)]
pub struct Effects {
    pub writes_files: bool,
    pub network: bool,
}

impl Effects {
    /// Whether a call can change files or reach the outside world.
    pub fn any(self) -> bool { self.writes_files || self.network }
}

/// Which role list grants a tool.
#[derive(Clone, Copy)]
pub enum Gate {
    /// The role's `tools` list, by tool name.
    Tool,
    /// The role's `skills` list, by the skill named in argument `arg`.
    Skill { arg: &'static str },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// What a tool gets when it runs.
pub struct Context<'a> {
    /// The project folder, where file tools work.
    pub root: &'a Path,
    pub layout: &'a crate::Layout,
    pub skills: &'a [Skill],
    pub policy: &'a Policy,
}

pub trait Tool {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// (argument, description) pairs, as shown to the model.
    fn params(&self) -> Vec<(&str, &str)>;
    fn effects(&self) -> Effects { Effects::default() }
    fn gate(&self) -> Gate { Gate::Tool }
    /// The argument holding a project path that file rules apply to, and how it is used.
    fn path_arg(&self) -> Option<(&'static str, Access)> { None }
    /// Whether its output is harness-written (like a skill) rather than from files or programs,
    /// so it can't carry injected instructions.
    fn trusted_output(&self) -> bool { false }
    /// Runs the call. Permission checks happen before this, in the tool loop.
    fn run(&self, ctx: &Context, args: &Value) -> Result<String>;
}

pub struct Toolbox {
    layout: crate::Layout,
    builtins: Vec<Box<dyn Tool>>,
    custom: Vec<CustomTool>,
    pub skills: Vec<Skill>,
    /// COMPANY.md: what the company and product are; sent with every call.
    pub company: Option<String>,
    /// Memory indexes of every scope; the prompt shows only what the role may see.
    pub memory: crate::memory::MemoryIndex,
    /// Folders that failed to load, with why, so the UI can warn instead of silently skipping.
    pub warnings: Vec<String>,
}

impl Default for Toolbox {
    fn default() -> Self {
        Toolbox { layout: crate::Layout::default(), builtins: builtin::all(), custom: Vec::new(), skills: Vec::new(), company: None, memory: Default::default(), warnings: Vec::new() }
    }
}

impl Toolbox {
    /// Everything from one folder; see [`crate::Layout::single`].
    pub fn load(root: &Path) -> Self {
        Self::open(crate::Layout::single(root))
    }

    /// Company tools, skills and context from `layout.company`; memory from `data` and `shared`.
    pub fn open(layout: crate::Layout) -> Self {
        let company = layout.company.clone();
        let mut tb = Toolbox { layout, ..Default::default() };
        tb.company = read_project_file(&company, "COMPANY.md").ok().filter(|t| !t.trim().is_empty());
        tb.memory = crate::memory::MemoryIndex::load(&tb.layout);
        for dir in project_subdirs(&company, "tools") {
            match dir.and_then(|dir| CustomTool::load(&tb.layout.project, &dir)) {
                Ok(t) if tb.get(t.name()).is_some() => tb.warnings.push(format!("tool `{}` is defined twice; kept the first", t.name())),
                Ok(t) => tb.custom.push(t),
                Err(e) => tb.warnings.push(e.to_string()),
            }
        }
        for dir in project_subdirs(&company, "skills") {
            match dir.and_then(|dir| Skill::load(&dir)) {
                Ok(s) => tb.skills.push(s),
                Err(e) => tb.warnings.push(e.to_string()),
            }
        }
        tb
    }

    pub fn layout(&self) -> &crate::Layout { &self.layout }

    /// Every tool, built-in ones first.
    pub fn tools(&self) -> impl Iterator<Item = &dyn Tool> {
        self.builtins.iter().map(|t| t.as_ref()).chain(self.custom.iter().map(|t| t as &dyn Tool))
    }

    /// Tools from `tools/`, without the built-in ones.
    pub fn custom_tools(&self) -> &[CustomTool] { &self.custom }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools().find(|t| t.name() == name)
    }

    pub fn allowed_skills<'a>(&'a self, policy: &'a Policy) -> impl Iterator<Item = &'a Skill> {
        self.skills.iter().filter(|s| policy.can_use_skill(&s.name))
    }

    /// Tools worth offering to the model under `policy`.
    pub fn available<'a>(&'a self, policy: &'a Policy) -> impl Iterator<Item = &'a dyn Tool> {
        let has_skills = self.allowed_skills(policy).next().is_some();
        self.tools().filter(move |t| match t.gate() {
            Gate::Tool => policy.can_use_tool(t.name()),
            Gate::Skill { .. } => has_skills,
        })
    }

    /// Runs a tool by name. Unknown names are an error.
    pub fn call(&self, name: &str, args: &Value, policy: &Policy) -> Result<String> {
        let tool = self.get(name).ok_or_else(|| format!("no tool `{name}`"))?;
        tool.run(&Context { root: &self.layout.project, layout: &self.layout, skills: &self.skills, policy }, args)
    }
}

/// Sorted subfolders of `<root>/<folder>`, each checked to stay inside the project.
fn project_subdirs(root: &Path, folder: &str) -> Vec<Result<PathBuf>> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root.join(folder))
        .into_iter().flatten().flatten()
        .map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs.into_iter().map(|dir| {
        let rel = dir.strip_prefix(root).unwrap_or(&dir).to_string_lossy().into_owned();
        crate::paths::resolve(root, &rel).map_err(|e| format!("{}: {e}", dir.display()).into())
    }).collect()
}

pub(crate) fn read_project_file(root: &Path, path: &str) -> Result<String> {
    let mut content = String::new();
    crate::paths::open(root, path, false)?.take(MAX_FILE_BYTES as u64 + 1).read_to_string(&mut content)?;
    if content.len() > MAX_FILE_BYTES { return Err("file exceeds 1 MiB".into()); }
    Ok(content)
}

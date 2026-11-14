//! Long-term memory: facts the model saves for later sessions, in three scopes.
//!
//! Layout, the same inside every scope folder (see [`crate::layout`] for `data` and `shared`):
//!   <data>/memory/                 — personal: only this user, only this project
//!   <shared>/memory/teams/<role>/  — shared with everyone in that role (the company memory repo)
//!   <shared>/memory/company/       — shared with everyone; only owners may write
//!   <scope>/<name>.md       — frontmatter `name:` / `description:`, then the fact
//!   <scope>/MEMORY.md       — one line per memory; rebuilt on every change and put in the system prompt
//!
//! Plain Markdown so a person can read, edit or delete memories by hand. Sharing happens through
//! git: the harness writes files, and repo permissions decide who may push them.

use crate::error::Result;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const DIR: &str = "memory";
const TEAMS: &str = "teams";
const COMPANY: &str = "company";
const INDEX: &str = "MEMORY.md";
const MAX_NAME: usize = 64;
const MAX_DESCRIPTION: usize = 200;
const MAX_BODY: usize = 4 * 1024;
/// The index goes into every prompt, so it is capped; past this, older lines are cut.
const MAX_INDEX_BYTES: usize = 8 * 1024;

/// Who a memory is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    Personal,
    Team(String),
    Company,
}

impl Scope {
    /// Reads a `scope` argument. `team` means the caller's own role, so nobody writes another team's memory.
    pub fn parse(scope: Option<&str>, role: Option<&str>) -> Result<Self> {
        match scope.unwrap_or("personal") {
            "personal" => Ok(Scope::Personal),
            "company" => Ok(Scope::Company),
            "team" => {
                let role = role.ok_or("team memory needs a role (start with --role <name>)")?;
                if !valid_name(role) {
                    return Err(format!("role `{role}` can't be used as a team folder name").into());
                }
                Ok(Scope::Team(role.into()))
            }
            other => Err(format!("unknown memory scope `{other}`; use personal, team or company").into()),
        }
    }

    fn dir(&self) -> String {
        match self {
            Scope::Personal => DIR.into(),
            Scope::Team(role) => format!("{DIR}/{TEAMS}/{role}"),
            Scope::Company => format!("{DIR}/{COMPANY}"),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Scope::Personal => "personal".into(),
            Scope::Team(role) => format!("team {role}"),
            Scope::Company => "company".into(),
        }
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.starts_with('-')
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub struct Memory {
    pub name: String,
    pub description: String,
    pub body: String,
}

impl Memory {
    /// Checks the fields so files and the index stay well-formed.
    pub fn new(name: &str, description: &str, body: &str) -> Result<Self> {
        let name = name.trim();
        if !valid_name(name) {
            return Err(format!("memory name must be 1-{MAX_NAME} characters of a-z, 0-9 and `-`").into());
        }
        if name == "memory" {
            return Err("`memory` is reserved for the index".into());
        }
        let description = description.split_whitespace().collect::<Vec<_>>().join(" ");
        if description.is_empty() || description.len() > MAX_DESCRIPTION {
            return Err(format!("memory description must be one line of 1-{MAX_DESCRIPTION} characters").into());
        }
        let body = body.trim();
        if body.is_empty() || body.len() > MAX_BODY {
            return Err(format!("memory content must be 1-{MAX_BODY} bytes").into());
        }
        Ok(Memory { name: name.into(), description, body: body.into() })
    }

    fn to_file(&self) -> String {
        format!("---\nname: {}\ndescription: {}\n---\n\n{}\n", self.name, self.description, self.body)
    }

    fn from_file(text: &str) -> Option<Self> {
        let rest = text.strip_prefix("---\n")?;
        let end = rest.find("\n---")?;
        let field = |key: &str| {
            rest[..end].lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(':')).map(|v| v.trim().to_string())
        };
        let body = rest[end + 4..].trim().to_string();
        Some(Memory { name: field("name")?, description: field("description").unwrap_or_default(), body })
    }
}

/// Reads and writes one scope folder. All file access goes through [`crate::paths`],
/// so symlinks and hard links are refused like in the file tools.
pub struct MemoryStore {
    root: PathBuf,
    dir: String,
}

impl MemoryStore {
    /// The personal store.
    pub fn new(root: &Path) -> Self {
        Self::scoped(root, &Scope::Personal)
    }

    pub fn scoped(root: &Path, scope: &Scope) -> Self {
        MemoryStore { root: root.to_path_buf(), dir: scope.dir() }
    }

    /// The store for `scope` in this layout: personal memory in `data`, shared memory in `shared`.
    /// Shared memory must be a cloned repo, so saving into a missing folder (where nobody would
    /// ever see it) is refused.
    pub fn for_scope(layout: &crate::Layout, scope: &Scope) -> Result<Self> {
        if *scope == Scope::Personal {
            return Ok(Self::new(&layout.data));
        }
        if !layout.shared.is_dir() {
            return Err(format!(
                "no shared memory repo at {}; clone the company memory repo there (or set UMANGO_SHARED)",
                layout.shared.display()
            ).into());
        }
        Ok(Self::scoped(&layout.shared, scope))
    }

    /// Saves or replaces a memory, then rebuilds the index. Returns true if it replaced one.
    pub fn save(&self, memory: &Memory) -> Result<bool> {
        let dir = crate::paths::resolve(&self.root, &self.dir)?;
        std::fs::create_dir_all(&dir)?;
        let path = self.file_of(&memory.name);
        let replaced = crate::paths::resolve(&self.root, &path)?.exists();
        crate::paths::open(&self.root, &path, true)?.write_all(memory.to_file().as_bytes())?;
        self.rebuild_index()?;
        Ok(replaced)
    }

    /// Deletes a memory, then rebuilds the index. Unknown names are an error.
    pub fn forget(&self, name: &str) -> Result<()> {
        Memory::new(name, "x", "x")?; // same name rules as save, so `../x` can't get through
        let path = crate::paths::resolve(&self.root, &self.file_of(name))?;
        if !path.is_file() {
            return Err(format!("no memory `{name}`").into());
        }
        std::fs::remove_file(path)?;
        self.rebuild_index()
    }

    /// Every readable memory, sorted by name. Unreadable or malformed files are skipped.
    pub fn all(&self) -> Vec<Memory> {
        let Ok(dir) = crate::paths::resolve(&self.root, &self.dir) else { return Vec::new() };
        let mut names: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".md").map(String::from))
            .filter(|n| n != "MEMORY")
            .collect();
        names.sort();
        names.iter().filter_map(|n| self.read(n).ok().flatten()).collect()
    }

    /// The index text for the system prompt, or None when there are no memories.
    pub fn index(&self) -> Option<String> {
        let file = crate::paths::open(&self.root, &format!("{}/{INDEX}", self.dir), false).ok()?;
        let mut text = String::new();
        file.take(MAX_INDEX_BYTES as u64).read_to_string(&mut text).ok()?;
        Some(text).filter(|t| !t.trim().is_empty())
    }

    fn read(&self, name: &str) -> Result<Option<Memory>> {
        let mut text = String::new();
        crate::paths::open(&self.root, &self.file_of(name), false)?.take((MAX_BODY + 1024) as u64).read_to_string(&mut text)?;
        Ok(Memory::from_file(&text))
    }

    fn rebuild_index(&self) -> Result<()> {
        let mut index = String::new();
        for m in self.all() {
            let line = format!("- {}: {}\n", m.name, m.description);
            if index.len() + line.len() > MAX_INDEX_BYTES { break; }
            index.push_str(&line);
        }
        crate::paths::open(&self.root, &format!("{}/{INDEX}", self.dir), true)?.write_all(index.as_bytes())?;
        Ok(())
    }

    fn file_of(&self, name: &str) -> String {
        format!("{}/{name}.md", self.dir)
    }
}

/// Every scope's index, read once at load so building the prompt needs no file access.
#[derive(Default)]
pub struct MemoryIndex {
    pub personal: Option<String>,
    pub company: Option<String>,
    pub teams: BTreeMap<String, String>,
}

impl MemoryIndex {
    pub fn load(layout: &crate::Layout) -> Self {
        let root = layout.shared.as_path();
        let mut teams = BTreeMap::new();
        if let Ok(dir) = crate::paths::resolve(root, &format!("{DIR}/{TEAMS}")) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let Some(role) = entry.file_name().to_str().map(String::from).filter(|r| valid_name(r)) else { continue };
                if let Some(index) = MemoryStore::scoped(root, &Scope::Team(role.clone())).index() {
                    teams.insert(role, index);
                }
            }
        }
        MemoryIndex {
            personal: MemoryStore::new(&layout.data).index(),
            company: MemoryStore::scoped(root, &Scope::Company).index(),
            teams,
        }
    }

    /// What `role` may read: company, its own team, then personal.
    pub fn visible(&self, role: Option<&str>) -> Vec<(Scope, &str)> {
        let team = role.and_then(|r| self.teams.get(r).map(|i| (Scope::Team(r.into()), i.as_str())));
        [self.company.as_deref().map(|i| (Scope::Company, i)), team, self.personal.as_deref().map(|i| (Scope::Personal, i))]
            .into_iter()
            .flatten()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDir;

    #[test]
    fn save_replace_and_forget_keep_the_index_in_sync() {
        let home = TestDir::new();
        let store = MemoryStore::new(home.path());
        assert!(store.index().is_none());

        assert!(!store.save(&Memory::new("db-choice", "We use sqlite", "Chosen for local CLI.").unwrap()).unwrap());
        assert!(store.save(&Memory::new("db-choice", "We use  postgres\n now", "Changed.").unwrap()).unwrap());
        store.save(&Memory::new("a-first", "Sorted first", "x").unwrap()).unwrap();

        assert_eq!(store.index().unwrap(), "- a-first: Sorted first\n- db-choice: We use postgres now\n");
        assert_eq!(store.all()[1].body, "Changed.");

        store.forget("db-choice").unwrap();
        assert_eq!(store.index().unwrap(), "- a-first: Sorted first\n");
        assert!(store.forget("db-choice").is_err());
    }

    #[test]
    fn rejects_bad_names_and_empty_fields() {
        for name in ["", "../x", "A", "-x", "memory", "a/b", &"x".repeat(65)] {
            assert!(Memory::new(name, "d", "b").is_err(), "{name}");
        }
        assert!(Memory::new("ok", "", "b").is_err());
        assert!(Memory::new("ok", "d", "  ").is_err());
        let store = MemoryStore::new(TestDir::new().path());
        assert!(store.forget("../permissions").is_err());
    }

    #[test]
    fn scopes_write_to_their_own_folders_and_roles_see_only_their_team() {
        let home = TestDir::new();
        let fact = |n: &str| Memory::new(n, n, "x").unwrap();
        MemoryStore::new(home.path()).save(&fact("mine")).unwrap();
        MemoryStore::scoped(home.path(), &Scope::Company).save(&fact("stack")).unwrap();
        MemoryStore::scoped(home.path(), &Scope::Team("sales".into())).save(&fact("discount")).unwrap();
        assert!(home.path().join("memory/teams/sales/discount.md").is_file());
        assert!(home.path().join("memory/company/stack.md").is_file());
        // The personal index doesn't pick up files from the shared folders.
        assert_eq!(MemoryStore::new(home.path()).index().unwrap(), "- mine: mine\n");

        let index = MemoryIndex::load(&crate::Layout::single(home.path()));
        let labels = |role| index.visible(role).iter().map(|(s, _)| s.label()).collect::<Vec<_>>();
        assert_eq!(labels(Some("sales")), ["company", "team sales", "personal"]);
        assert_eq!(labels(Some("dev")), ["company", "personal"]);
    }

    #[test]
    fn parses_scopes_from_the_callers_role() {
        assert_eq!(Scope::parse(None, None).unwrap(), Scope::Personal);
        assert_eq!(Scope::parse(Some("team"), Some("sales")).unwrap(), Scope::Team("sales".into()));
        assert!(Scope::parse(Some("team"), None).is_err());
        assert!(Scope::parse(Some("team"), Some("../x")).is_err());
        assert!(Scope::parse(Some("everyone"), Some("sales")).is_err());
    }

    #[test]
    fn personal_and_shared_memory_live_in_their_own_folders() {
        let (data, shared) = (TestDir::new(), TestDir::new());
        let layout = crate::Layout { data: data.path().into(), shared: shared.path().join("missing"), ..Default::default() };
        let fact = Memory::new("x", "d", "b").unwrap();
        MemoryStore::for_scope(&layout, &Scope::Personal).unwrap().save(&fact).unwrap();
        assert!(data.path().join("memory/x.md").is_file());
        assert!(MemoryStore::for_scope(&layout, &Scope::Company).is_err(), "no clone, so nobody would see it");

        let layout = crate::Layout { shared: shared.path().into(), ..layout };
        MemoryStore::for_scope(&layout, &Scope::Company).unwrap().save(&fact).unwrap();
        assert!(shared.path().join("memory/company/x.md").is_file());
        let index = MemoryIndex::load(&layout);
        assert!(index.personal.is_some() && index.company.is_some());
    }

    #[test]
    fn refuses_a_symlinked_memory_folder() {
        let home = TestDir::new();
        let elsewhere = TestDir::new();
        std::os::unix::fs::symlink(elsewhere.path(), home.path().join("memory")).unwrap();
        let store = MemoryStore::new(home.path());
        assert!(store.save(&Memory::new("x", "d", "b").unwrap()).is_err());
        assert!(std::fs::read_dir(elsewhere.path()).unwrap().next().is_none());
    }
}

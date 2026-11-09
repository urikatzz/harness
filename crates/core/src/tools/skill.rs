//! Skills: playbooks in `skills/<name>/SKILL.md`.

use super::read_project_file;
use crate::error::Result;
use std::path::{Path, PathBuf};

pub struct Skill {
    pub name: String,
    pub description: String,
    path: PathBuf,
}

impl Skill {
    /// Reads `<dir>/SKILL.md`. Name and description come from the frontmatter; the name
    /// falls back to the folder name.
    pub fn load(dir: &Path) -> Result<Self> {
        let text = read_project_file(dir, "SKILL.md").map_err(|e| format!("{}: SKILL.md: {e}", dir.display()))?;
        let field = |key: &str| {
            frontmatter(&text)
                .lines()
                .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix(':')))
                .map(|v| v.trim().to_string())
        };
        let fallback = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
        Ok(Skill { name: field("name").unwrap_or(fallback), description: field("description").unwrap_or_default(), path: dir.join("SKILL.md") })
    }

    /// The playbook text, read fresh so edits show up without a reload.
    pub fn read(&self, root: &Path) -> Result<String> {
        read_project_file(root, &self.path.strip_prefix(root)?.to_string_lossy())
    }
}

fn frontmatter(text: &str) -> &str {
    text.strip_prefix("---")
        .and_then(|rest| rest.find("\n---").map(|end| &rest[..end]))
        .unwrap_or("")
}

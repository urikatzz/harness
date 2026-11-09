//! Where the harness reads and writes, split by who owns the data (like Claude Code):
//!
//!   project  — the folder the harness runs in; the only place file tools work
//!   company  — `permissions.json`, `COMPANY.md`, `tools/`, `skills/`, `evals/`. Installed by IT in a
//!              folder users can't write (`/Library/Application Support/Uritheking` on macOS,
//!              `/etc/uritheking` on Linux) or set with `URITHEKING_COMPANY`. Missing: the project
//!              folder is used instead, which is fine for development but not locked.
//!   data     — this user's sessions, audit log and personal memory for this project:
//!              `~/.uritheking/projects/<project path>/` (`URITHEKING_USER_DIR` replaces `~/.uritheking`)
//!   shared   — a clone of the company's memory repo (team and company memory):
//!              `~/.uritheking/shared` or `URITHEKING_SHARED`

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub project: PathBuf,
    pub company: PathBuf,
    pub data: PathBuf,
    pub shared: PathBuf,
}

/// Where IT installs the company folder.
fn managed_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(PathBuf::from("/Library/Application Support/Uritheking"))
    } else if cfg!(target_os = "linux") {
        Some(PathBuf::from("/etc/uritheking"))
    } else {
        None
    }
}

fn canonical(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

/// `/Users/me/work/app` → `-Users-me-work-app`, one folder name per project.
fn project_key(project: &Path) -> String {
    project.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '-' }).collect()
}

impl Layout {
    /// Everything in one folder: tests, and setups without a user home.
    pub fn single(root: &Path) -> Self {
        let root = canonical(root.to_path_buf());
        Layout { project: root.clone(), company: root.clone(), data: root.clone(), shared: root }
    }

    /// The layout for a run in `project`, from the environment.
    pub fn discover(project: &Path) -> Self {
        let env = |key: &str| std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from);
        let project = canonical(project.to_path_buf());
        let company = env("URITHEKING_COMPANY")
            .or_else(|| managed_dir().filter(|d| d.is_dir()))
            .map(canonical)
            .unwrap_or_else(|| project.clone());
        let Some(user) = env("URITHEKING_USER_DIR").or_else(|| env("HOME").map(|h| h.join(".uritheking"))) else {
            return Layout { company, ..Layout::single(&project) };
        };
        Layout {
            data: user.join("projects").join(project_key(&project)),
            shared: env("URITHEKING_SHARED").unwrap_or_else(|| user.join("shared")),
            company,
            project,
        }
    }

    /// The company setup lives in the project, so nothing locks it (development or a missing install).
    pub fn company_in_project(&self) -> bool {
        self.company == self.project
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_key_is_one_folder_name() {
        assert_eq!(project_key(Path::new("/Users/me/my app")), "-Users-me-my-app");
    }

    #[test]
    fn single_keeps_everything_together() {
        let dir = crate::test_support::TestDir::new();
        let layout = Layout::single(dir.path());
        assert!(layout.company_in_project());
        assert_eq!(layout.data, layout.shared);
    }
}

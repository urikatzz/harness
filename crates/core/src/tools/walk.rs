//! Walks the project for the tools that look at many files at once.

use crate::error::Result;
use crate::permissions::Policy;
use ignore::WalkBuilder;
use std::path::Path;

/// Project-relative paths of regular files under `start` that the role may read without approval,
/// sorted. Git-ignored files, `.git/`, symlinks and protected files are left out.
pub(crate) fn readable_files(root: &Path, start: &str, glob: Option<&str>, policy: &Policy) -> Result<Vec<String>> {
    let scope = if crate::paths::is_root(start) { root.to_path_buf() } else { crate::paths::resolve(root, start)? };
    if !scope.is_dir() { return Err(format!("`{start}` is not a folder").into()); }
    // Always walk from the root, descending only towards `scope`, so the root's ignore files
    // apply in a subfolder too and nothing above the project is ever consulted.
    let walker = WalkBuilder::new(root)
        .follow_links(false)
        .hidden(false)
        .parents(false)
        .require_git(false)
        .git_global(false)
        .filter_entry(move |entry| {
            entry.file_name() != ".git" && (entry.path().starts_with(&scope) || scope.starts_with(entry.path()))
        })
        .build();
    let mut files = Vec::new();
    // Entries that can't be read are skipped; the rest of the walk continues.
    for entry in walker.flatten() {
        // A symlink's own type is "symlink", so links are dropped here along with folders.
        if !entry.file_type().is_some_and(|t| t.is_file()) { continue; }
        let Some(path) = entry.path().strip_prefix(root).ok().and_then(Path::to_str) else { continue };
        if glob.is_some_and(|g| !crate::permissions::glob(g, path)) { continue; }
        if policy.readable_without_approval(path) { files.push(path.to_string()); }
    }
    files.sort();
    Ok(files)
}

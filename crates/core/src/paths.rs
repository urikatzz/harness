//! One path interpretation for authorization and execution.
use std::path::{Component, Path, PathBuf};

pub fn resolve(root: &Path, path: &str) -> Result<PathBuf, String> {
    let mut result = root.to_path_buf();
    let mut count = 0;
    for component in Path::new(path).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => {
                result.push(part);
                count += 1;
                match std::fs::symlink_metadata(&result) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err("symlink paths are not allowed".into())
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.to_string()),
                }
            }
            _ => return Err("path must stay inside the project".into()),
        }
    }
    if count == 0 {
        return Err("a file path is required".into());
    }
    Ok(result)
}

/// Walk directory descriptors without following symlinks, including during concurrent renames.
#[cfg(unix)]
pub fn open(root: &Path, path: &str, write: bool) -> crate::error::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    let resolved = resolve(root, path)?;
    let relative = resolved.strip_prefix(root)?;
    let mut parts = relative.components().peekable();
    let mut directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)?;
    use std::os::unix::fs::OpenOptionsExt;
    while let Some(part) = parts.next() {
        let name = std::ffi::CString::new(part.as_os_str().as_bytes())?;
        let last = parts.peek().is_none();
        let flags = if !last {
            libc::O_RDONLY | libc::O_DIRECTORY
        } else if write {
            libc::O_WRONLY | libc::O_CREAT
        } else {
            libc::O_RDONLY
        };
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        if last {
            if !file.metadata()?.is_file() {
                return Err("path must refer to a regular file".into());
            }
            use std::os::unix::fs::MetadataExt;
            if file.metadata()?.nlink() > 1 {
                return Err("hard-linked files are not allowed".into());
            }
            if write {
                file.set_len(0)?;
            }
            return Ok(file);
        }
        directory = file;
    }
    Err("a file path is required".into())
}

#[cfg(not(unix))]
pub fn open(_: &Path, _: &str, _: bool) -> crate::error::Result<std::fs::File> {
    Err("secure file tools require Unix directory descriptors".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDir;

    #[test]
    fn rejects_traversal_and_file_aliases() {
        let project = TestDir::new();
        assert!(resolve(project.path(), "././../../outside").is_err());
        assert!(resolve(project.path(), "/tmp/outside").is_err());
        std::fs::write(project.path().join("secret"), "untouched").unwrap();
        std::os::unix::fs::symlink(project.path().join("secret"), project.path().join("alias"))
            .unwrap();
        assert!(open(project.path(), "alias", false).is_err());
        assert!(open(project.path(), "alias", true).is_err());
        std::fs::hard_link(
            project.path().join("secret"),
            project.path().join("hardlink"),
        )
        .unwrap();
        assert!(open(project.path(), "hardlink", true).is_err());
        assert_eq!(
            std::fs::read_to_string(project.path().join("secret")).unwrap(),
            "untouched"
        );
    }

    #[test]
    fn creates_files_but_rejects_non_regular_files() {
        use std::io::{Read, Write};
        let project = TestDir::new();
        open(project.path(), "./answer", true)
            .unwrap()
            .write_all(b"42")
            .unwrap();
        let mut text = String::new();
        open(project.path(), "answer", false)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "42");
        assert!(open(project.path(), ".", false).is_err());
        assert!(open(project.path(), "missing/answer", true).is_err());
    }
}

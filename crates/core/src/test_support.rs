use std::path::{Path, PathBuf};

pub struct TestDir(PathBuf);
impl TestDir {
    pub fn new() -> Self {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("uritheking-{}-{id}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

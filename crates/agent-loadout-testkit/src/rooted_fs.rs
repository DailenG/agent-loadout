use std::io;
use std::path::{Path, PathBuf};

use agent_loadout_core::paths::normalize;
use agent_loadout_core::{Fs, RealFs};

/// A real file system confined to one root. Any access to a path outside the
/// root (including relative paths and `..` escapes) panics before touching
/// the disk, so a test that would read or write real agent configs fails.
#[derive(Debug, Clone)]
pub struct RootedFs {
    root: PathBuf,
}

impl RootedFs {
    /// Confines access to `root`, which must be absolute.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        assert!(
            root.is_absolute(),
            "RootedFs root must be absolute: {}",
            root.display()
        );
        Self {
            root: normalize(&root),
        }
    }

    fn check<'a>(&self, path: &'a Path) -> &'a Path {
        if !path.is_absolute() || !normalize(path).starts_with(&self.root) {
            panic!(
                "RootedFs: access outside test root: {} (root {})",
                path.display(),
                self.root.display()
            );
        }
        path
    }
}

impl Fs for RootedFs {
    fn root(&self) -> Option<&Path> {
        Some(&self.root)
    }

    fn exists(&self, path: &Path) -> bool {
        RealFs.exists(self.check(path))
    }

    fn is_dir(&self, path: &Path) -> bool {
        RealFs.is_dir(self.check(path))
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        RealFs.read_to_string(self.check(path))
    }

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        RealFs.write(self.check(path), contents)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        RealFs.create_dir_all(self.check(path))
    }
}

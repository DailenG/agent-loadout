use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use agent_loadout_core::paths::{normalize, plain};
use agent_loadout_core::{Fs, RealFs};

/// A real file system confined to one root. Any access to a path outside the
/// root (including relative paths and `..` escapes) panics before touching
/// the disk, so a test that would read or write real agent configs fails.
/// A `\\?\` path is checked in its plain form.
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
            root: normalize(&plain(&root)),
        }
    }

    fn check<'a>(&self, path: &'a Path) -> &'a Path {
        if !path.is_absolute() || !normalize(&plain(path)).starts_with(&self.root) {
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

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        RealFs.read(self.check(path))
    }

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        RealFs.write(self.check(path), contents)
    }

    fn write_new(&self, path: &Path, contents: &[u8], like: Option<&Path>) -> io::Result<()> {
        let like = like.map(|like| self.check(like));
        RealFs.write_new(self.check(path), contents, like)
    }

    fn replace(&self, from: &Path, to: &Path) -> io::Result<()> {
        RealFs.replace(self.check(from), self.check(to))
    }

    fn write_private(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        RealFs.write_private(self.check(path), contents)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        RealFs.create_dir_all(self.check(path))
    }

    fn create_private_dir_all(&self, path: &Path) -> io::Result<()> {
        RealFs.create_private_dir_all(self.check(path))
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        RealFs.remove_file(self.check(path))
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        RealFs.remove_dir_all(self.check(path))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        RealFs.read_dir(self.check(path))
    }

    fn open_lock_file(&self, path: &Path) -> io::Result<File> {
        RealFs.open_lock_file(self.check(path))
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        RealFs.canonicalize(self.check(path))
    }
}

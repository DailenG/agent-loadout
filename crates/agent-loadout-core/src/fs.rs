//! File system facade. Core code takes `&dyn Fs` so tests can confine every
//! access to a temp root (`agent_loadout_testkit::RootedFs`).

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

pub trait Fs: Send + Sync {
    /// The directory this view is confined to, or `None` for the whole file
    /// system. Ancestor walks (such as sync-root detection) stop here.
    fn root(&self) -> Option<&Path>;

    fn exists(&self, path: &Path) -> bool;

    fn is_dir(&self, path: &Path) -> bool;

    fn read_to_string(&self, path: &Path) -> io::Result<String>;

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()>;

    fn create_dir_all(&self, path: &Path) -> io::Result<()>;

    fn remove_file(&self, path: &Path) -> io::Result<()>;

    /// Paths of the entries of a directory, sorted.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>>;

    /// Opens `path` for use as an advisory lock file, creating it if needed.
    /// The contents are never read or written.
    fn open_lock_file(&self, path: &Path) -> io::Result<File>;
}

/// The real file system, unconfined.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealFs;

impl Fs for RealFs {
    fn root(&self) -> Option<&Path> {
        None
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        std::fs::write(path, contents)
    }

    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let mut entries = std::fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<Vec<_>>>()?;
        entries.sort();
        Ok(entries)
    }

    fn open_lock_file(&self, path: &Path) -> io::Result<File> {
        File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
    }
}

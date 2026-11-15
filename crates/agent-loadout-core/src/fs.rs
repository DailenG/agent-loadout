//! File system facade. Core code takes `&dyn Fs` so tests can confine every
//! access to a temp root (`agent_loadout_testkit::RootedFs`).

use std::io;
use std::path::Path;

pub trait Fs: Send + Sync {
    /// The directory this view is confined to, or `None` for the whole file
    /// system. Ancestor walks (such as sync-root detection) stop here.
    fn root(&self) -> Option<&Path>;

    fn exists(&self, path: &Path) -> bool;

    fn is_dir(&self, path: &Path) -> bool;

    fn read_to_string(&self, path: &Path) -> io::Result<String>;

    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()>;

    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
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
}

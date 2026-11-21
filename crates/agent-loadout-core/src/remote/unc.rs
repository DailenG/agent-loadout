//! Mapped drive letters as UNC paths (SPEC 9.6): a folder remote typed as
//! `Z:\Backups\loadout` is kept as typed, but each push resolves it to
//! `\\server\share\Backups\loadout`, so pushes work from processes that
//! lack the mapping (elevated shells, scheduled tasks).
//!
//! [`UncResolver`] is the seam; tests fake it. [`SystemUnc`] asks Windows
//! through `std::fs::canonicalize` (`GetFinalPathNameByHandleW`), which
//! returns `\\?\UNC\server\share\...` for a mapped network drive, instead
//! of calling `WNetGetUniversalNameW` (no `unsafe` in this workspace). A
//! local drive, an unmapped letter or another OS keeps the path as typed.

use std::path::{Path, PathBuf};

use crate::env::Os;

/// Resolves a typed remote path before a push or pull.
pub trait UncResolver: Send + Sync {
    /// The UNC form of `path` when it is on a mapped network drive.
    fn universal_name(&self, path: &Path) -> Option<PathBuf>;
}

/// No resolution (Linux, macOS, tests).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoUnc;

impl UncResolver for NoUnc {
    fn universal_name(&self, _path: &Path) -> Option<PathBuf> {
        None
    }
}

/// Windows: the path the drive letter maps to.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemUnc;

impl UncResolver for SystemUnc {
    fn universal_name(&self, path: &Path) -> Option<PathBuf> {
        if !cfg!(windows) || !is_drive_path(path) {
            return None;
        }
        let resolved = std::fs::canonicalize(path).ok()?;
        let text = resolved.to_str()?;
        text.strip_prefix(r"\\?\UNC\")
            .map(|rest| PathBuf::from(format!(r"\\{rest}")))
    }
}

/// `X:\...` or `X:/...`.
pub fn is_drive_path(path: &Path) -> bool {
    let bytes = path.as_os_str().as_encoded_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

/// The path a push or pull uses for `typed`: its UNC form on Windows when
/// the drive is mapped, else as typed.
pub fn resolve(os: Os, resolver: &dyn UncResolver, typed: &Path) -> PathBuf {
    if os == Os::Windows && is_drive_path(typed) {
        resolver
            .universal_name(typed)
            .unwrap_or_else(|| typed.to_path_buf())
    } else {
        typed.to_path_buf()
    }
}

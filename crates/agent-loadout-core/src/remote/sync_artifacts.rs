//! Sync tool artifacts under folder remotes (SPEC 9.6 table): conflict
//! copies and temp files Dropbox, OneDrive, Google Drive, Syncthing and
//! Resilio Sync leave behind. Readers accept only names of their exact
//! grammar anyway; names that match a known pattern are also reported as
//! `sync_artifact_ignored`. They are never read and never deleted:
//! deleting a Resilio `.Conflict` file can delete the real file on a peer.

use std::path::PathBuf;

/// One reported artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub path: PathBuf,
    /// `Dropbox`, `Google Drive`, `Syncthing`, `Resilio Sync`, `OS`.
    pub tool: &'static str,
}

/// The sync tool a file or directory name belongs to, if it matches one of
/// the known conflict or temp patterns.
pub fn artifact_tool(name: &str) -> Option<&'static str> {
    // Dropbox.
    if name.contains(" (conflicted copy")
        || name.contains(" (Case Conflict")
        || name.contains(" (Selective Sync Conflict")
        || matches!(name, ".dropbox" | ".dropbox.attr" | ".dropbox.cache")
    {
        return Some("Dropbox");
    }
    // Syncthing.
    if name.contains(".sync-conflict-")
        || (name.starts_with(".syncthing.") && name.ends_with(".tmp"))
        || (name.starts_with("~syncthing~") && name.ends_with(".tmp"))
        || matches!(name, ".stfolder" | ".stversions" | ".stignore")
    {
        return Some("Syncthing");
    }
    // Resilio Sync.
    if name.contains(".Conflict")
        || name.ends_with(".!sync")
        || name.ends_with(".rsls")
        || name == ".sync"
    {
        return Some("Resilio Sync");
    }
    // Google Drive: `name (1).ext`, and its transfer folders.
    if matches!(name, ".tmp.drivedownload" | ".tmp.driveupload") || numbered_copy(name) {
        return Some("Google Drive");
    }
    if matches!(name, ".DS_Store" | "Thumbs.db" | "desktop.ini") || name.starts_with("~$") {
        return Some("OS");
    }
    None
}

/// `* (<n>)` before the extension, or at the end.
fn numbered_copy(name: &str) -> bool {
    let stem = match name.rfind('.') {
        Some(dot) if dot > 0 => &name[..dot],
        _ => name,
    };
    let Some(inner) = stem.strip_suffix(')') else {
        return false;
    };
    let Some(open) = inner.rfind(" (") else {
        return false;
    };
    let digits = &inner[open + 2..];
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

//! Content summaries of skill folders, cached by their file listing (#3,
//! #9: the app scans unmanaged folders on focus and watcher events).
//!
//! The key is the folder's path and, for every file below it (nested
//! `.git` left out, as [`read_folder`] leaves it out), its relative path,
//! size and modification time. The listing costs metadata only; a folder
//! whose listing is unchanged is not read again. A folder holding a link
//! is never cached (what it points to can change without the listing
//! changing). The listing is taken before the content is read, so a change
//! during the read shows as a new listing next time.
//!
//! An edit that keeps a file's size and modification time is not seen.
//! The cache is for listings only: adopt reads every folder again and
//! compares it with the store copy under the store lock before replacing
//! anything.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use crate::error::CoreError;
use crate::fs::Fs;
use crate::skills::{
    SKILL_FILE, SkillFiles, confine, content_hash, files_size, frontmatter, read_folder,
};

/// What a scan needs of a folder's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// [`content_hash`] of the files.
    pub hash: String,
    /// `description` from its `SKILL.md`.
    pub description: Option<String>,
    pub bytes: u64,
    pub has_skill_file: bool,
}

impl Summary {
    pub fn of(files: &SkillFiles) -> Self {
        let description = files.get(SKILL_FILE).and_then(|text| {
            frontmatter::parse(&String::from_utf8_lossy(text))
                .ok()
                .flatten()
                .and_then(|fm| fm.description().map(str::to_owned))
        });
        Self {
            hash: content_hash(files),
            description,
            bytes: files_size(files),
            has_skill_file: files.contains_key(SKILL_FILE),
        }
    }
}

/// One file of a listing: relative path, size, modification time.
type ListedFile = (String, u64, Option<SystemTime>);

/// Summaries of folders by path, valid while their listing is unchanged.
/// Machine-local and in memory: the app keeps one for its process.
#[derive(Debug, Default)]
pub struct FolderHashCache {
    entries: Mutex<HashMap<PathBuf, (Vec<ListedFile>, Summary)>>,
}

impl FolderHashCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The summary of the folder `dir`: from the cache when its listing is
    /// the one cached, else read ([`read_folder`], with its errors) and
    /// cached.
    pub fn summary(&self, fs: &dyn Fs, dir: &Path) -> Result<Summary, CoreError> {
        let listed = listing(fs, dir);
        if let Some(listed) = &listed
            && let Some((known, summary)) = self.lock().get(dir)
            && known == listed
        {
            return Ok(summary.clone());
        }
        let summary = Summary::of(&read_folder(fs, dir)?);
        let mut entries = self.lock();
        match listed {
            Some(listed) => {
                entries.insert(dir.to_path_buf(), (listed, summary.clone()));
            }
            None => {
                entries.remove(dir);
            }
        }
        Ok(summary)
    }

    /// Number of folders cached.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, (Vec<ListedFile>, Summary)>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The files below `dir`, sorted; `None` when the folder cannot be listed
/// or holds a link (then it is read every time).
fn listing(fs: &dyn Fs, dir: &Path) -> Option<Vec<ListedFile>> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<ListedFile>) -> Option<()> {
        for item in std::fs::read_dir(dir).ok()? {
            let item = item.ok()?;
            let name = item.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                continue;
            }
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            // The entry's own metadata (a link is not followed); on
            // Windows it comes with the listing, without another call.
            let meta = item.metadata().ok()?;
            if meta.file_type().is_symlink() {
                return None;
            }
            if meta.is_dir() {
                walk(&item.path(), &relative, out)?;
            } else {
                out.push((relative, meta.len(), meta.modified().ok()));
            }
        }
        Some(())
    }
    confine(fs, dir).ok()?;
    if std::fs::symlink_metadata(dir)
        .ok()?
        .file_type()
        .is_symlink()
    {
        return None;
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out)?;
    out.sort();
    Some(out)
}

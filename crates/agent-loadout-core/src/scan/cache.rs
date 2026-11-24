//! Scan results by content hash: `<LOADOUT_HOME>/cache/scan/<key>.json`,
//! machine-local and re-creatable. The key covers the content hash, the
//! rules file's fingerprint and this build's version, so new rules or a
//! new scanner scan again. Best effort: a cache that cannot be read or
//! written only costs a scan.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::ScanReport;
use super::rules::Rules;
use crate::fs::Fs;

/// Folder below `<LOADOUT_HOME>/cache`.
pub const CACHE_DIR: &str = "scan";
/// Entries kept: past this, the folder is emptied before the next write.
const MAX_ENTRIES: usize = 512;
const SCHEMA: u32 = 1;
const ENGINE: &str = concat!("loadout-builtin/", env!("CARGO_PKG_VERSION"));

#[derive(Serialize, Deserialize)]
struct Entry {
    schema: u32,
    key: String,
    report: ScanReport,
}

pub(super) struct Cache<'a> {
    fs: &'a dyn Fs,
    dir: PathBuf,
}

impl<'a> Cache<'a> {
    pub(super) fn new(fs: &'a dyn Fs, dir: PathBuf) -> Self {
        Self { fs, dir }
    }

    pub(super) fn key(rules: &Rules, content_hash: &str) -> String {
        let mut hasher = Sha256::new();
        for part in [ENGINE, rules.fingerprint(), content_hash] {
            hasher.update(part.as_bytes());
            hasher.update([0]);
        }
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    pub(super) fn get(&self, key: &str) -> Option<ScanReport> {
        let text = self.fs.read_to_string(&self.path(key)).ok()?;
        let entry: Entry = serde_json::from_str(&text).ok()?;
        (entry.schema == SCHEMA && entry.key == key).then_some(ScanReport {
            cached: true,
            ..entry.report
        })
    }

    pub(super) fn put(&self, key: &str, report: &ScanReport) {
        let entry = Entry {
            schema: SCHEMA,
            key: key.to_owned(),
            report: report.clone(),
        };
        let Ok(text) = serde_json::to_vec(&entry) else {
            return;
        };
        if self
            .fs
            .read_dir(&self.dir)
            .is_ok_and(|entries| entries.len() >= MAX_ENTRIES)
        {
            let _ = self.fs.remove_dir_all(&self.dir);
        }
        if self.fs.create_dir_all(&self.dir).is_ok() {
            let _ = self.fs.write(&self.path(key), &text);
        }
    }
}

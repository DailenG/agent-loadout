//! `machine.json` (SPEC 9.6): what one machine's subfolder of a folder
//! remote holds, written last, after objects, refs and exports. Readers
//! trust only what it lists.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;

/// The manifest file in a machine subfolder.
pub const MANIFEST_FILE: &str = "machine.json";
/// `EXPORT.json` next to a mirror's files and inside archives.
pub const EXPORT_FILE: &str = "EXPORT.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineManifest {
    pub schema: u32,
    pub store_id: String,
    pub machine_id: String,
    pub label: String,
    pub os: String,
    pub app_version: String,
    /// RFC 3339, UTC.
    pub pushed_at: String,
    /// HEAD when pushed (in `store.git` for `repo-per-machine`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// `repo-per-machine`: tag name to id in `store.git`.
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
    /// `mirror`: the commit `latest/` holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror: Option<String>,
    /// `archive`: archive file names in `archives/`, newest last.
    #[serde(default)]
    pub archives: Vec<String>,
}

/// `EXPORT.json`: what an exported tree is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportInfo {
    pub schema: u32,
    pub commit: String,
    /// RFC 3339, UTC.
    pub time: String,
    /// The machine id that exported it.
    pub machine: String,
}

impl MachineManifest {
    /// The manifest in `dir`, `None` when absent or unreadable (a sync
    /// tool may be writing it; the machine is skipped this time).
    pub fn load(fs: &dyn Fs, dir: &Path) -> Option<Self> {
        let text = fs.read_to_string(&dir.join(MANIFEST_FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Writes the manifest into `dir` by temp file and rename.
    pub fn write(&self, fs: &dyn Fs, dir: &Path) -> Result<(), CoreError> {
        let text = serde_json::to_string_pretty(self).map_err(|err| {
            CoreError::new(
                ErrorCode::InvalidValue,
                format!("cannot write manifest: {err}"),
            )
        })?;
        crate::apply::write_atomic(fs, &dir.join(MANIFEST_FILE), text.as_bytes())
    }
}

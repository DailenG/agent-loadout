//! Apply journals (SPEC 8.4 step 6): `state/journal/<apply-id>.json` grows
//! by one record per file while a run executes. A finished run moves to
//! `state/applies/<apply-id>.json` (the last 200 are kept). A journal left
//! behind by a crash is reported with its per-file outcomes; nothing is
//! replayed.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::apply::write_atomic;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::applied::{ApplyOutcome, ApplyTrigger, Scope};

/// Unfinished runs, in `state/`.
pub const JOURNAL_DIR: &str = "journal";
/// Finished runs, in `state/`.
pub const APPLIES_DIR: &str = "applies";
/// Finished runs kept in [`APPLIES_DIR`].
const KEEP_APPLIES: usize = 200;
const JOURNAL_SCHEMA: u32 = 1;

/// One apply run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Journal {
    pub schema: u32,
    pub apply_id: String,
    pub trigger: ApplyTrigger,
    /// RFC 3339, UTC.
    pub started_at: String,
    pub finished: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ApplyOutcome>,
    #[serde(default)]
    pub files: Vec<FileRecord>,
}

/// What one file got.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileRecord {
    pub clients: Vec<String>,
    pub scope: Scope,
    /// Absolute path on this machine (the journal is machine-local).
    pub file: String,
    pub outcome: ApplyOutcome,
    pub create: u32,
    pub update: u32,
    pub remove: u32,
    pub disable: u32,
    /// Whether the file was written.
    pub written: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RecordedError>,
}

/// An error code and its message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordedError {
    pub code: String,
    pub message: String,
}

impl From<&CoreError> for RecordedError {
    fn from(err: &CoreError) -> Self {
        Self {
            code: err.code().as_str().to_owned(),
            message: err.to_string(),
        }
    }
}

impl Journal {
    /// Starts the journal of a run and writes it.
    pub fn start(
        fs: &dyn Fs,
        state_dir: &Path,
        apply_id: &str,
        trigger: ApplyTrigger,
        started_at: String,
    ) -> Result<Self, CoreError> {
        let journal = Self {
            schema: JOURNAL_SCHEMA,
            apply_id: apply_id.to_owned(),
            trigger,
            started_at,
            finished: false,
            outcome: None,
            files: Vec::new(),
        };
        journal.save(fs, &journal_path(state_dir, apply_id))?;
        Ok(journal)
    }

    /// Appends a file's outcome and rewrites the journal.
    pub fn record(
        &mut self,
        fs: &dyn Fs,
        state_dir: &Path,
        file: FileRecord,
    ) -> Result<(), CoreError> {
        self.files.push(file);
        self.save(fs, &journal_path(state_dir, &self.apply_id))
    }

    /// Marks the run finished: writes `applies/<apply-id>.json`, deletes
    /// the journal and keeps the last 200 results.
    pub fn finish(
        mut self,
        fs: &dyn Fs,
        state_dir: &Path,
        outcome: ApplyOutcome,
    ) -> Result<Self, CoreError> {
        self.finished = true;
        self.outcome = Some(outcome);
        self.save(fs, &result_path(state_dir, &self.apply_id))?;
        let journal = journal_path(state_dir, &self.apply_id);
        if fs.exists(&journal) {
            fs.remove_file(&journal)
                .map_err(|err| io("cannot remove journal", &journal, &err))?;
        }
        prune(fs, &state_dir.join(APPLIES_DIR))?;
        Ok(self)
    }

    fn save(&self, fs: &dyn Fs, path: &Path) -> Result<(), CoreError> {
        let mut text = serde_json::to_string_pretty(self).map_err(|err| {
            CoreError::new(
                ErrorCode::InvalidValue,
                format!("cannot serialize journal: {err}"),
            )
        })?;
        text.push('\n');
        write_atomic(fs, path, text.as_bytes())
    }
}

/// Journals of runs that never finished, oldest first.
pub fn unfinished(fs: &dyn Fs, state_dir: &Path) -> Result<Vec<Journal>, CoreError> {
    read_all(fs, &state_dir.join(JOURNAL_DIR))
}

/// The result of a finished run, if it is still kept.
pub fn result(fs: &dyn Fs, state_dir: &Path, apply_id: &str) -> Result<Option<Journal>, CoreError> {
    let path = result_path(state_dir, apply_id);
    if !fs.exists(&path) {
        return Ok(None);
    }
    read(fs, &path).map(Some)
}

fn journal_path(state_dir: &Path, apply_id: &str) -> PathBuf {
    state_dir.join(JOURNAL_DIR).join(format!("{apply_id}.json"))
}

fn result_path(state_dir: &Path, apply_id: &str) -> PathBuf {
    state_dir.join(APPLIES_DIR).join(format!("{apply_id}.json"))
}

fn read_all(fs: &dyn Fs, dir: &Path) -> Result<Vec<Journal>, CoreError> {
    if !fs.is_dir(dir) {
        return Ok(Vec::new());
    }
    let paths = fs
        .read_dir(dir)
        .map_err(|err| io("cannot list journals", dir, &err))?;
    paths
        .iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .map(|path| read(fs, path))
        .collect()
}

fn read(fs: &dyn Fs, path: &Path) -> Result<Journal, CoreError> {
    let text = fs
        .read_to_string(path)
        .map_err(|err| io("cannot read journal", path, &err))?;
    serde_json::from_str(&text).map_err(|err| {
        CoreError::new(
            ErrorCode::ParseError,
            format!("journal does not parse: {err}"),
        )
        .with_path(path)
    })
}

/// Keeps the newest [`KEEP_APPLIES`] results (apply ids are ULIDs, so the
/// name order is the time order).
fn prune(fs: &dyn Fs, dir: &Path) -> Result<(), CoreError> {
    let paths: Vec<PathBuf> = fs
        .read_dir(dir)
        .map_err(|err| io("cannot list apply results", dir, &err))?
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    let excess = paths.len().saturating_sub(KEEP_APPLIES);
    for path in paths.iter().take(excess) {
        fs.remove_file(path)
            .map_err(|err| io("cannot prune apply results", path, &err))?;
    }
    Ok(())
}

fn io(what: &str, path: &Path, err: &std::io::Error) -> CoreError {
    CoreError::new(ErrorCode::IoError, format!("{what}: {err}")).with_path(path)
}

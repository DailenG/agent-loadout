//! Project trust (SPEC 17): `state/trust.json`, machine-local. A project is
//! trusted when a record for its folder holds the current content hash of
//! its manifests ([`Project::hash`]).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::apply::write_atomic;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::layering::manifest::{Project, same_path};

/// Trust file in `state/`.
pub const TRUST_FILE: &str = "trust.json";

const TRUST_SCHEMA: u32 = 1;

/// Whether a project's manifests may be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustState {
    /// A record for this folder holds the current hash.
    Trusted,
    /// No record for this folder.
    Untrusted,
    /// A record for this folder holds another hash: a manifest changed
    /// since it was trusted.
    Changed,
}

impl TrustState {
    pub const fn as_str(self) -> &'static str {
        match self {
            TrustState::Trusted => "trusted",
            TrustState::Untrusted => "untrusted",
            TrustState::Changed => "changed",
        }
    }
}

/// One trusted folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustRecord {
    pub id: String,
    /// `<P>`, as resolved on this machine.
    pub path: String,
    /// [`crate::layering::content_hash`] of the manifests when trusted.
    pub hash: String,
    /// Keys a newer build wrote, kept on rewrite.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `state/trust.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrustStore {
    pub schema: u32,
    #[serde(default)]
    pub projects: Vec<TrustRecord>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for TrustStore {
    fn default() -> Self {
        Self {
            schema: TRUST_SCHEMA,
            projects: Vec::new(),
            extra: serde_json::Map::new(),
        }
    }
}

impl TrustStore {
    /// Reads the trust file in `state_dir`; an absent file trusts nothing.
    pub fn load(fs: &dyn Fs, state_dir: &Path) -> Result<Self, CoreError> {
        let path = state_dir.join(TRUST_FILE);
        if !fs.exists(&path) {
            return Ok(Self::default());
        }
        let text = fs.read_to_string(&path).map_err(|err| {
            CoreError::new(
                ErrorCode::IoError,
                format!("cannot read the trust file: {err}"),
            )
            .with_path(&path)
        })?;
        let store: Self = serde_json::from_str(&text).map_err(|err| {
            CoreError::new(
                ErrorCode::ParseError,
                format!("the trust file does not parse: {err}"),
            )
            .with_path(&path)
        })?;
        if store.schema != TRUST_SCHEMA {
            return Err(CoreError::new(
                ErrorCode::UnsupportedSchema,
                format!(
                    "trust file schema {} is not supported; this build reads schema {TRUST_SCHEMA}",
                    store.schema
                ),
            )
            .with_path(&path));
        }
        Ok(store)
    }

    /// Writes the trust file atomically.
    pub fn save(&self, fs: &dyn Fs, state_dir: &Path) -> Result<(), CoreError> {
        let mut text = serde_json::to_string_pretty(self).map_err(|err| {
            CoreError::new(
                ErrorCode::InvalidValue,
                format!("cannot serialize the trust file: {err}"),
            )
        })?;
        text.push('\n');
        write_atomic(fs, &state_dir.join(TRUST_FILE), text.as_bytes())
    }

    /// The trust state of `project`'s current manifests. The record is
    /// found by folder, however its path is spelled ([`same_path`]).
    pub fn state(&self, project: &Project) -> TrustState {
        match self.record(&project.root) {
            None => TrustState::Untrusted,
            Some(record) if record.hash == project.hash() => TrustState::Trusted,
            Some(_) => TrustState::Changed,
        }
    }

    /// Trusts `project`'s current manifests, replacing the record of its
    /// folder.
    pub fn trust(&mut self, project: &Project) {
        let hash = project.hash();
        match self
            .projects
            .iter_mut()
            .find(|known| same_path(Path::new(&known.path), &project.root))
        {
            Some(slot) => {
                slot.id.clone_from(&project.id);
                slot.path = project.root.to_string_lossy().into_owned();
                slot.hash = hash;
            }
            None => self.projects.push(TrustRecord {
                id: project.id.clone(),
                path: project.root.to_string_lossy().into_owned(),
                hash,
                extra: serde_json::Map::new(),
            }),
        }
    }

    /// Drops every record of project `id`; returns whether there was one.
    pub fn forget(&mut self, id: &str) -> bool {
        let before = self.projects.len();
        self.projects.retain(|record| record.id != id);
        self.projects.len() != before
    }

    fn record(&self, root: &Path) -> Option<&TrustRecord> {
        self.projects
            .iter()
            .find(|record| same_path(Path::new(&record.path), root))
    }
}

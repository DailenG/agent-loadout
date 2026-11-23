//! Unmanaged entries (#3, #9): the foreign MCP entries an import scan finds
//! ([`crate::apply::import::scan_with`]), as findings per client and scope
//! for the read-only "not managed by Loadout" views and counts, and which
//! of them this machine has not acknowledged yet (the "new unmanaged
//! entries" notice).
//!
//! Acknowledged findings are a machine-local fact (`state/unmanaged-seen.json`,
//! never synced): they name client files of this machine. A finding is
//! identified by [`Source::key`] and the entry name (`claude-code/github`,
//! `cursor@app-1a2b/postgres`), never by a path, so the id can cross IPC.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::apply::import::{Candidate, Source};
use crate::apply::write_atomic;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::applied::Scope;

/// The file in `state/`.
pub const SEEN_FILE: &str = "unmanaged-seen.json";
const SEEN_SCHEMA: u32 = 1;

/// One foreign entry in one client file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// `<source key>/<name>` ([`finding_id`]).
    pub id: String,
    pub client: &'static str,
    pub scope: Scope,
    pub project_id: Option<String>,
    /// The project's folder, for placeholder paths.
    pub project_root: Option<PathBuf>,
    pub name: String,
    pub file: PathBuf,
}

/// The id of entry `name` of `source`: `<source key>/<name>`.
pub fn finding_id(source: &Source, name: &str) -> String {
    format!("{}/{name}", source.key())
}

/// One finding per source of each candidate, in scan order.
pub fn findings(candidates: &[Candidate]) -> Vec<Finding> {
    let mut out = Vec::new();
    for candidate in candidates {
        for source in &candidate.sources {
            out.push(Finding {
                id: finding_id(source, &candidate.name),
                client: source.client,
                scope: source.scope,
                project_id: source.project.as_ref().map(|p| p.id.clone()),
                project_root: source.project.as_ref().map(|p| p.root.clone()),
                name: candidate.name.clone(),
                file: source.file.clone(),
            });
        }
    }
    out
}

/// The findings of one client in one scope (one project).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub client: &'static str,
    pub scope: Scope,
    pub project_id: Option<String>,
    pub project_root: Option<PathBuf>,
    /// The files they were found in, in first-seen order.
    pub files: Vec<PathBuf>,
    /// `(finding, new)`: `new` when this machine has not acknowledged it.
    pub entries: Vec<(Finding, bool)>,
}

/// `findings` per client and scope (and project), each marked new unless
/// `seen` holds it. Groups are ordered by client, scope and project; their
/// entries by name.
pub fn groups(findings: Vec<Finding>, seen: &Seen) -> Vec<Group> {
    type Key = (&'static str, u8, Option<String>);
    let rank = |scope: Scope| match scope {
        Scope::User => 0,
        Scope::Project => 1,
        Scope::ProjectLocal => 2,
    };
    let mut map: BTreeMap<Key, Group> = BTreeMap::new();
    for finding in findings {
        let key = (
            finding.client,
            rank(finding.scope),
            finding.project_id.clone(),
        );
        let group = map.entry(key).or_insert_with(|| Group {
            client: finding.client,
            scope: finding.scope,
            project_id: finding.project_id.clone(),
            project_root: finding.project_root.clone(),
            files: Vec::new(),
            entries: Vec::new(),
        });
        if !group.files.contains(&finding.file) {
            group.files.push(finding.file.clone());
        }
        let new = !seen.contains(&finding.id);
        group.entries.push((finding, new));
    }
    let mut groups: Vec<Group> = map.into_values().collect();
    for group in &mut groups {
        group.entries.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    }
    groups
}

/// The findings this machine acknowledged (`state/unmanaged-seen.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    schema: u32,
    #[serde(default)]
    ids: BTreeSet<String>,
}

impl Seen {
    /// The acknowledged findings; none when the file is absent.
    pub fn load(fs: &dyn Fs, state_dir: &Path) -> Result<Self, CoreError> {
        let path = state_dir.join(SEEN_FILE);
        if !fs.exists(&path) {
            return Ok(Self {
                schema: SEEN_SCHEMA,
                ids: BTreeSet::new(),
            });
        }
        let text = fs.read_to_string(&path).map_err(|err| {
            CoreError::new(ErrorCode::IoError, format!("cannot read file: {err}")).with_path(&path)
        })?;
        let seen: Self = serde_json::from_str(&text).map_err(|err| {
            CoreError::new(
                ErrorCode::ParseError,
                format!("the file does not parse: {err}"),
            )
            .with_path(&path)
        })?;
        if seen.schema != SEEN_SCHEMA {
            return Err(CoreError::new(
                ErrorCode::UnsupportedSchema,
                format!("schema {} is not read by this build", seen.schema),
            )
            .with_path(&path));
        }
        Ok(seen)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// Acknowledges `ids` and writes the file when that changed anything.
    /// Returns how many were new.
    pub fn acknowledge(
        &mut self,
        fs: &dyn Fs,
        state_dir: &Path,
        ids: &[String],
    ) -> Result<usize, CoreError> {
        let added = ids
            .iter()
            .filter(|id| self.ids.insert((*id).clone()))
            .count();
        if added == 0 {
            return Ok(0);
        }
        self.schema = SEEN_SCHEMA;
        let mut text = serde_json::to_string_pretty(self).map_err(|err| {
            CoreError::new(ErrorCode::InvalidValue, format!("cannot serialize: {err}"))
        })?;
        text.push('\n');
        fs.create_dir_all(state_dir).map_err(|err| {
            CoreError::new(
                ErrorCode::IoError,
                format!("cannot create directory: {err}"),
            )
            .with_path(state_dir)
        })?;
        write_atomic(fs, &state_dir.join(SEEN_FILE), text.as_bytes())?;
        Ok(added)
    }
}

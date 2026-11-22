//! Push, pull and status for every configured remote alike (SPEC 9.6 to
//! 9.9), shared by the CLI and the app.
//!
//! Each push and pull is recorded per remote in the machine-local
//! `state/remote-status.toml` (time, head pushed, last error), so the
//! History page and Settings can show `ahead N`, `behind N` and the last
//! error without reaching the remote (SPEC 9.8). [`status`] reads that
//! record and, for a folder remote, the folder itself: this machine's
//! `machine.json` (what was pushed) and the siblings' heads (what there is
//! to pull). It never fetches from a git remote: `behind` comes from the
//! last fetch.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::env::Env;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::SCHEMA_VERSION;
use crate::remote::config::{RemoteConfig, RemoteKind, RemotesFile};
use crate::remote::folder::{self, PullReport, PushOutcome};
use crate::remote::git::{self, GitCli, GitPush, Runner};
use crate::remote::manifest::MachineManifest;
use crate::remote::unc::UncResolver;
use crate::store::diff::{ChangeKind, EntityChange};
use crate::store::{CommitId, Machine, Store, utc_datetime};

/// The record file in `state/`.
pub const STATUS_FILE: &str = "remote-status.toml";
/// Incoming commits counted per sibling machine at most.
const INCOMING_LIMIT: usize = 1000;

/// What pushes and pulls run with.
pub struct SyncEnv<'a> {
    pub unc: &'a dyn UncResolver,
    pub runner: &'a dyn Runner,
    /// The `loadout` binary, the credential helper of `auth = token`.
    pub helper: Option<PathBuf>,
    /// Written to a folder remote's `machine.json`.
    pub app_version: &'a str,
}

impl SyncEnv<'_> {
    fn git(&self, store: &Store<'_>) -> Result<GitCli<'_>, CoreError> {
        GitCli::from_env(store.env(), store.fs(), self.runner, self.helper.clone())
    }
}

/// What one push did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pushed {
    Folder(PushOutcome),
    Git(GitPush),
}

/// One push: the remote's name and how it went.
pub type PushResult = (String, Result<Pushed, CoreError>);
/// One pull: the remote's name and how it went.
pub type PullResult = (String, Result<PullReport, CoreError>);

/// The error recorded for a remote's last push or pull.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedError {
    pub code: ErrorCode,
    pub message: String,
    /// RFC 3339, UTC.
    pub at: String,
}

/// One remote's record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteRecord {
    /// RFC 3339, UTC, of the last successful push.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed_head: Option<String>,
    /// RFC 3339, UTC, of the last successful pull.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pulled_at: Option<String>,
    /// The last push or pull failed with this; cleared by a success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RecordedError>,
}

/// `state/remote-status.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusFile {
    pub schema: i64,
    #[serde(default, rename = "remote")]
    pub remotes: BTreeMap<String, RemoteRecord>,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for StatusFile {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            remotes: BTreeMap::new(),
            extra: toml::Table::new(),
        }
    }
}

impl StatusFile {
    /// The records; none when the file is absent or unreadable (it is a
    /// cache of what happened, never needed for work).
    pub fn load(fs: &dyn Fs, state_dir: &Path) -> Self {
        fs.read_to_string(&state_dir.join(STATUS_FILE))
            .ok()
            .and_then(|text| toml::from_str::<Self>(&text).ok())
            .filter(|file| file.schema == SCHEMA_VERSION)
            .unwrap_or_default()
    }

    pub fn save(&self, fs: &dyn Fs, state_dir: &Path) -> Result<(), CoreError> {
        let text = toml::to_string_pretty(self).map_err(|err| {
            CoreError::new(
                ErrorCode::InvalidValue,
                format!("cannot write remote status: {err}"),
            )
        })?;
        crate::apply::write_atomic(fs, &state_dir.join(STATUS_FILE), text.as_bytes())
    }

    /// Records are kept only for configured remotes.
    fn retain(&mut self, remotes: &RemotesFile) {
        self.remotes.retain(|name, _| remotes.get(name).is_some());
    }
}

fn now(env: &Env) -> String {
    utc_datetime(env.now()).to_string()
}

fn record_error(env: &Env, err: &CoreError) -> RecordedError {
    RecordedError {
        code: err.code(),
        message: err.message().to_owned(),
        at: now(env),
    }
}

/// Saves the outcome of pushes and pulls; a failure to save is ignored,
/// the record is informational.
fn save_records(store: &Store<'_>, remotes: &RemotesFile, update: impl FnOnce(&mut StatusFile)) {
    let mut file = StatusFile::load(store.fs(), store.state_dir());
    update(&mut file);
    file.retain(remotes);
    let _ = file.save(store.fs(), store.state_dir());
}

/// Pushes to the selected remotes (every remote when `name` is `None`);
/// one result per remote, each recorded.
pub fn push_to(
    store: &Store<'_>,
    remotes: &RemotesFile,
    name: Option<&str>,
    sx: &SyncEnv<'_>,
    progress: &mut dyn FnMut(&str),
) -> Result<Vec<PushResult>, CoreError> {
    let results: Vec<PushResult> = remotes
        .select(name)?
        .into_iter()
        .map(|remote| (remote.name.clone(), push_one(store, remote, sx, progress)))
        .collect();
    let env = store.env();
    save_records(store, remotes, |file| {
        for (name, result) in &results {
            let record = file.remotes.entry(name.clone()).or_default();
            match result {
                Ok(pushed) => {
                    record.pushed_at = Some(now(env));
                    record.pushed_head = Some(
                        match pushed {
                            Pushed::Folder(outcome) => &outcome.head,
                            Pushed::Git(outcome) => &outcome.head,
                        }
                        .to_string(),
                    );
                    record.error = None;
                }
                Err(err) => record.error = Some(record_error(env, err)),
            }
        }
    });
    Ok(results)
}

fn push_one(
    store: &Store<'_>,
    remote: &RemoteConfig,
    sx: &SyncEnv<'_>,
    progress: &mut dyn FnMut(&str),
) -> Result<Pushed, CoreError> {
    match remote.kind {
        RemoteKind::Folder => {
            folder::push(store, remote, sx.unc, sx.app_version).map(Pushed::Folder)
        }
        RemoteKind::Git => {
            let cli = sx.git(store)?;
            git::push(store, &cli, remote, progress).map(Pushed::Git)
        }
    }
}

/// Pulls from the selected remotes and merges (SPEC 9.9); one result per
/// remote, each recorded.
pub fn pull_from(
    store: &Store<'_>,
    remotes: &RemotesFile,
    name: Option<&str>,
    sx: &SyncEnv<'_>,
    progress: &mut dyn FnMut(&str),
) -> Result<Vec<PullResult>, CoreError> {
    let mut results = Vec::new();
    for remote in remotes.select(name)? {
        let report = match remote.kind {
            RemoteKind::Folder => folder::pull(store, remote, sx.unc),
            RemoteKind::Git => sx
                .git(store)
                .and_then(|cli| git::pull(store, &cli, remote, progress)),
        };
        results.push((remote.name.clone(), report));
    }
    let env = store.env();
    save_records(store, remotes, |file| {
        for (name, result) in &results {
            let record = file.remotes.entry(name.clone()).or_default();
            match result {
                Ok(_) => {
                    record.pulled_at = Some(now(env));
                    record.error = None;
                }
                Err(err) => record.error = Some(record_error(env, err)),
            }
        }
    });
    Ok(results)
}

/// Servers and skills a pull changed (SPEC 9.9 step 6).
pub fn changed_by_pull(store: &Store<'_>, report: &PullReport) -> (usize, usize) {
    if report.head_before == report.head_after {
        return (0, 0);
    }
    let changes: Vec<EntityChange> = store
        .semantic_diff(Some(&report.head_before), &report.head_after)
        .unwrap_or_default();
    let mut servers: Vec<&str> = Vec::new();
    let mut skills: Vec<&str> = Vec::new();
    for change in &changes {
        if change.kind == ChangeKind::Changed && change.fields.is_empty() && change.text.is_none() {
            continue;
        }
        let list = if change.entity.starts_with("server/") {
            &mut servers
        } else if change.entity.starts_with("skill/") {
            &mut skills
        } else {
            continue;
        };
        if !list.contains(&change.entity.as_str()) {
            list.push(&change.entity);
        }
    }
    (servers.len(), skills.len())
}

/// A remote as Settings and History show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteStatus {
    pub name: String,
    /// Folder remotes: whether the folder is there now. Git remotes are not
    /// contacted (`None`).
    pub reachable: Option<bool>,
    /// What this machine pushed last, as far as is known.
    pub pushed_head: Option<CommitId>,
    pub pushed_at: Option<String>,
    pub pulled_at: Option<String>,
    /// Commits of HEAD the remote does not have; `None` when unknown (never
    /// pushed, or the pushed head is not in this store's history).
    pub ahead: Option<usize>,
    /// Commits the remote has that HEAD lacks: other machines' heads in a
    /// folder remote, the last fetch of a git remote. `None` when unknown.
    pub behind: Option<usize>,
    pub last_error: Option<RecordedError>,
}

/// The status of `remote`, without contacting a git remote. `store` is
/// `None` before the store exists.
pub fn status(
    env: &Env,
    fs: &dyn Fs,
    unc: &dyn UncResolver,
    state_dir: &Path,
    store: Option<&Store<'_>>,
    remote: &RemoteConfig,
) -> RemoteStatus {
    let record = StatusFile::load(fs, state_dir)
        .remotes
        .remove(&remote.name)
        .unwrap_or_default();
    let mut status = RemoteStatus {
        name: remote.name.clone(),
        reachable: None,
        pushed_head: record
            .pushed_head
            .as_deref()
            .and_then(|id| CommitId::from_text(id).ok()),
        pushed_at: record.pushed_at,
        pulled_at: record.pulled_at,
        ahead: None,
        behind: None,
        last_error: record.error,
    };
    match remote.kind {
        RemoteKind::Folder => folder_status(env, fs, unc, state_dir, store, remote, &mut status),
        RemoteKind::Git => git_status(store, remote, &mut status),
    }
    status
}

fn folder_status(
    env: &Env,
    fs: &dyn Fs,
    unc: &dyn UncResolver,
    state_dir: &Path,
    store: Option<&Store<'_>>,
    remote: &RemoteConfig,
    status: &mut RemoteStatus,
) {
    let Ok(root) = folder::reachable_root(fs, env.os(), unc, remote) else {
        status.reachable = Some(false);
        return;
    };
    status.reachable = Some(true);
    let Some(store) = store else {
        return;
    };
    let Ok(Some(machine)) = Machine::load(fs, state_dir) else {
        return;
    };
    // What the folder says this machine pushed wins over the record: the
    // folder may have been restored or emptied since.
    let own = MachineManifest::load(fs, &root.join(store.store_id()).join(&machine.id));
    status.pushed_head = own
        .as_ref()
        .and_then(|manifest| manifest.head.as_deref())
        .and_then(|id| CommitId::from_text(id).ok());
    if let Some(manifest) = &own {
        status.pushed_at = Some(manifest.pushed_at.clone());
    }
    status.ahead = ahead(store, status.pushed_head.as_ref());
    let Ok(listing) = folder::list(fs, &root, Some(store.store_id())) else {
        return;
    };
    let mut behind = 0;
    for sibling in &listing.machines {
        if sibling.machine_id == machine.id || !sibling.has_repo(fs) {
            continue;
        }
        let Some(head) = sibling.manifest.head.as_deref() else {
            continue;
        };
        // A sibling still being copied counts as nothing yet.
        behind += store
            .bare_incoming(&sibling.dir.join("store.git"), head, INCOMING_LIMIT)
            .unwrap_or(0);
    }
    status.behind = Some(behind);
}

fn git_status(store: Option<&Store<'_>>, remote: &RemoteConfig, status: &mut RemoteStatus) {
    let Some(store) = store else {
        return;
    };
    let tracking = store
        .reference_target(&format!("refs/remotes/{}/main", remote.name))
        .ok()
        .flatten();
    // The newer of what was pushed and what was fetched.
    let upstream = match (&status.pushed_head, &tracking) {
        (Some(pushed), Some(fetched)) if store.contains(fetched) && !store.contains(pushed) => {
            Some(fetched.clone())
        }
        (Some(pushed), _) => Some(pushed.clone()),
        (None, fetched) => fetched.clone(),
    };
    status.ahead = ahead(store, upstream.as_ref());
    status.behind = match &tracking {
        Some(fetched) => store
            .head()
            .ok()
            .and_then(|head| store.ahead_behind(&head, fetched).ok())
            .map(|(_, behind)| behind),
        None => upstream.as_ref().map(|_| 0),
    };
}

/// Commits of HEAD not in `upstream`, when `upstream` is in HEAD's history.
fn ahead(store: &Store<'_>, upstream: Option<&CommitId>) -> Option<usize> {
    let upstream = upstream?;
    if !store.contains(upstream) {
        return None;
    }
    let head = store.head().ok()?;
    store
        .ahead_behind(&head, upstream)
        .ok()
        .map(|(ahead, _)| ahead)
}

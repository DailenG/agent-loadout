//! History page commands (SPEC 13.5, 9.3 to 9.5): the commit list with its
//! tags, a commit's semantic and raw diff (masked), tags, restore of a
//! version or of some entities with a preview first, client file backups
//! with per-file restore, merge conflicts waiting for a decision, and a zip
//! of a version.
//!
//! A store restore commits without applying: the page opens the apply
//! drawer next, because a restore can touch many files (SPEC 9.5). Undoing
//! an apply is `undo_apply` (`apply.rs`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use agent_loadout_core::apply::backup;
use agent_loadout_core::apply::placeholder_path;
use agent_loadout_core::model::TomlDocument;
use agent_loadout_core::model::applied::{AppliedRecord, ApplyTrigger};
use agent_loadout_core::remote::folder::export_zip;
use agent_loadout_core::store::diff::{ChangeKind, EntityChange, entity_of};
use agent_loadout_core::store::merge::read_merge_state;
use agent_loadout_core::store::restore::{self, FileOutcome, RestoreRequest};
use agent_loadout_core::store::tags::{TagInfo, TagKind};
use agent_loadout_core::store::{
    CommitId, LogEntry, LogFilter, Machine, Scope as TrailerScope, Store, StoreRead, utc_datetime,
};
use agent_loadout_core::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use specta::Type;

use super::apply::mask_diff;
use super::{AppState, CmdResult, Core, FileScope, IpcError, OpCounts, OutcomeKind, blocking};

/// Which commits the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum HistoryScope {
    All,
    System,
    Projects,
}

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryFilter {
    pub scope: HistoryScope,
    /// `server/github`, `secrets`, `project/<id>`: commits naming it or
    /// changing its files.
    pub entity: Option<String>,
    pub limit: Option<u32>,
}

/// The apply a commit recorded (`Loadout-Apply`).
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ApplyMark {
    pub apply_id: String,
    pub outcome: OutcomeKind,
    /// An apply of this machine: "Undo this apply" is offered.
    pub undoable: bool,
    /// This machine's newest apply: undoing it never asks (SPEC 8.7).
    pub latest: bool,
}

/// One commit of the list, newest first.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub short_id: String,
    /// RFC 3339, UTC.
    pub time: String,
    pub summary: String,
    pub op: Option<String>,
    pub entities: Vec<String>,
    /// `system` or `project/<id>`.
    pub scope: Option<String>,
    /// The machine's label (`desktop`), or the author name of a commit
    /// made outside Loadout.
    pub machine: String,
    pub this_machine: bool,
    pub apply: Option<ApplyMark>,
    /// Tags on this commit, user tags first.
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TagKindDto {
    User,
    Auto,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TagEntry {
    pub name: String,
    pub kind: TagKindDto,
    pub commit: String,
    pub message: Option<String>,
    /// RFC 3339, UTC.
    pub time: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum EntityChangeKind {
    Added,
    Removed,
    Changed,
}

impl From<ChangeKind> for EntityChangeKind {
    fn from(kind: ChangeKind) -> Self {
        match kind {
            ChangeKind::Added => Self::Added,
            ChangeKind::Removed => Self::Removed,
            ChangeKind::Changed => Self::Changed,
        }
    }
}

/// `env.GITHUB_TOOLSETS: "repos" -> "repos,issues"`; refs stay refs,
/// anything that may be a secret is masked.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FieldDiff {
    pub field: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EntityDiff {
    pub entity: String,
    pub file: String,
    pub change: EntityChangeKind,
    pub fields: Vec<FieldDiff>,
    /// Unified diff of a file that is not TOML (skill files), masked.
    pub text: Option<String>,
}

/// One client file an apply commit recorded.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppliedTargetEntry {
    pub client: String,
    pub scope: FileScope,
    pub project_id: Option<String>,
    /// Under its placeholder root, as recorded.
    pub file: String,
    pub counts: OpCounts,
    pub outcome: OutcomeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ApplyTriggerKind {
    Instant,
    Review,
    Restore,
    Rollback,
    Pull,
}

impl From<ApplyTrigger> for ApplyTriggerKind {
    fn from(trigger: ApplyTrigger) -> Self {
        match trigger {
            ApplyTrigger::Instant => Self::Instant,
            ApplyTrigger::Review => Self::Review,
            ApplyTrigger::Restore => Self::Restore,
            ApplyTrigger::Rollback => Self::Rollback,
            ApplyTrigger::Pull => Self::Pull,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppliedDetail {
    pub trigger: ApplyTriggerKind,
    pub outcome: OutcomeKind,
    pub targets: Vec<AppliedTargetEntry>,
}

/// One commit with its diff against its first parent.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetail {
    pub entry: HistoryEntry,
    /// The message without its `Loadout-*` trailers.
    pub body: String,
    pub changes: Vec<EntityDiff>,
    /// The unified git diff, masked.
    pub raw: String,
    /// The files an apply commit touched and their outcomes (SPEC 13.5).
    pub applied: Option<AppliedDetail>,
}

/// What restoring a version would change. Read-only.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RestorePreview {
    pub target: String,
    pub short_id: String,
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
    /// Store files restored.
    pub files: Vec<String>,
    /// Shared project manifests written back to their folders (`<P>/...`);
    /// the restore needs a confirmation for them.
    pub shared_manifests: Vec<String>,
    /// Restored manifests of projects this machine has no folder for.
    pub unplaced: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    /// The `auto/pre-restore/<ts>` tag; `None` when nothing changed.
    pub tag: Option<String>,
    pub commit: Option<String>,
    pub files: Vec<String>,
    /// Manifests written back (`<P>/.loadout/...`).
    pub manifests: Vec<String>,
}

/// One apply run's client file backups.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BackupRunEntry {
    /// Names the run in `restore_client_files`: its directory name
    /// (`<ts>-<apply id>`), unique per run.
    pub run: String,
    pub apply_id: String,
    /// RFC 3339, UTC.
    pub time: String,
    /// The bytes a client file restore replaced (`restore-...`).
    pub undo_of_restore: bool,
    /// Under their placeholder roots.
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FileRestoreOutcome {
    Restored,
    Unchanged,
    Failed,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FileRestoreEntry {
    pub file: String,
    pub outcome: FileRestoreOutcome,
    pub error: Option<IpcError>,
}

/// A field two machines changed differently (SPEC 9.9), waiting in
/// `state/merge/MERGE_STATE.json`.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MergeConflictEntry {
    /// `<remote>/<machine-id>`, `<remote>` or `restore/<source>`.
    pub source: String,
    pub entity: String,
    pub file: String,
    pub field: Option<String>,
}

fn require_store(core: &Core) -> Result<Store<'_>, IpcError> {
    core.open_store()?.ok_or_else(|| IpcError {
        code: ErrorCode::NotFound,
        message: "there is no store yet".to_owned(),
    })
}

fn machine_id(core: &Core) -> Result<Option<String>, CoreError> {
    Ok(Machine::load(core.fs.as_ref(), &core.home.state_dir())?.map(|machine| machine.id))
}

/// `value` with anything that may be a secret masked, read as the value of
/// the last key of `field`.
fn mask_field(field: &str, value: &str) -> String {
    let key = field.rsplit('.').next().unwrap_or(field);
    let prefix = format!("{key} = ");
    let masked = mask_diff(&format!("{prefix}{value}"));
    masked
        .strip_prefix(&prefix)
        .map_or(masked.clone(), str::to_owned)
}

fn tags_by_commit(tags: &[TagInfo]) -> BTreeMap<String, Vec<String>> {
    let mut by_commit: BTreeMap<String, Vec<(TagKind, String)>> = BTreeMap::new();
    for tag in tags {
        by_commit
            .entry(tag.target.to_string())
            .or_default()
            .push((tag.kind, tag.name.clone()));
    }
    by_commit
        .into_iter()
        .map(|(commit, mut names)| {
            names.sort_by_key(|(kind, name)| (*kind == TagKind::Auto, name.clone()));
            (commit, names.into_iter().map(|(_, name)| name).collect())
        })
        .collect()
}

struct Marks {
    machine: Option<String>,
    /// This machine's apply ids, newest first.
    applies: Vec<String>,
    tags: BTreeMap<String, Vec<String>>,
}

impl Marks {
    fn load(core: &Core, store: &Store<'_>) -> Result<Self, IpcError> {
        let machine = machine_id(core)?;
        let applies = match &machine {
            Some(id) => agent_loadout_core::apply::rollback::apply_commits(store, id)?
                .iter()
                .filter_map(|entry| entry.trailers.apply.as_ref().map(|a| a.id.clone()))
                .collect(),
            None => Vec::new(),
        };
        Ok(Self {
            machine,
            applies,
            tags: tags_by_commit(&store.tags()?),
        })
    }

    fn entry(&self, entry: &LogEntry) -> HistoryEntry {
        let trailers = &entry.trailers;
        let this_machine = trailers
            .machine
            .as_ref()
            .is_some_and(|machine| Some(&machine.id) == self.machine.as_ref());
        HistoryEntry {
            id: entry.id.to_string(),
            short_id: entry.id.short().to_owned(),
            time: entry.time_rfc3339(),
            summary: entry.summary.clone(),
            op: trailers.op.clone(),
            entities: trailers.entities.clone(),
            scope: trailers.scope.as_ref().map(ToString::to_string),
            machine: trailers
                .machine
                .as_ref()
                .map_or_else(|| entry.author_name.clone(), |m| m.label.clone()),
            this_machine,
            apply: trailers.apply.as_ref().map(|apply| {
                let position = self.applies.iter().position(|id| *id == apply.id);
                ApplyMark {
                    apply_id: apply.id.clone(),
                    outcome: apply.outcome.into(),
                    undoable: this_machine && position.is_some(),
                    latest: position == Some(0),
                }
            }),
            tags: self
                .tags
                .get(entry.id.as_str())
                .cloned()
                .unwrap_or_default(),
        }
    }
}

/// The commit list, newest first.
pub fn history(core: &Core, filter: &HistoryFilter) -> CmdResult<Vec<HistoryEntry>> {
    let Some(store) = core.open_store()? else {
        return Ok(Vec::new());
    };
    let entity = filter
        .entity
        .as_deref()
        .map(str::trim)
        .filter(|entity| !entity.is_empty());
    let entries = store.log(&LogFilter {
        entity: entity.map(str::to_owned),
        scope: match filter.scope {
            HistoryScope::System => Some(TrailerScope::System),
            HistoryScope::All | HistoryScope::Projects => None,
        },
        limit: None,
    })?;
    let marks = Marks::load(core, &store)?;
    let limit = filter.limit.map_or(usize::MAX, |n| n as usize);
    Ok(entries
        .iter()
        .filter(|entry| {
            filter.scope != HistoryScope::Projects
                || matches!(entry.trailers.scope, Some(TrailerScope::Project(_)))
        })
        .take(limit)
        .map(|entry| marks.entry(entry))
        .collect())
}

fn tag_entry(tag: &TagInfo) -> TagEntry {
    TagEntry {
        name: tag.name.clone(),
        kind: match tag.kind {
            TagKind::User => TagKindDto::User,
            TagKind::Auto => TagKindDto::Auto,
        },
        commit: tag.target.to_string(),
        message: tag.message.clone(),
        time: tag.time.map(|time| utc_datetime(time).to_string()),
    }
}

/// Every tag, user tags first, newest first within each kind.
pub fn tags(core: &Core) -> CmdResult<Vec<TagEntry>> {
    let Some(store) = core.open_store()? else {
        return Ok(Vec::new());
    };
    let mut tags = store.tags()?;
    tags.sort_by(|a, b| {
        (a.kind == TagKind::Auto)
            .cmp(&(b.kind == TagKind::Auto))
            .then_with(|| b.time.cmp(&a.time))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(tags.iter().map(tag_entry).collect())
}

fn applied_detail(store: &Store<'_>, entry: &LogEntry) -> Option<AppliedDetail> {
    let apply = entry.trailers.apply.as_ref()?;
    let machine = entry.trailers.machine.as_ref()?;
    let text = store
        .revision(&entry.id)
        .ok()?
        .read_file(&format!("machines/{}.applied.toml", machine.id))
        .ok()??;
    let record = TomlDocument::<AppliedRecord>::parse(&text)
        .ok()?
        .into_value();
    (record.apply_id == apply.id).then(|| AppliedDetail {
        trigger: record.trigger.into(),
        outcome: record.outcome.into(),
        targets: record
            .targets
            .into_iter()
            .map(|target| AppliedTargetEntry {
                client: target.client,
                scope: target.scope.into(),
                project_id: target.project_id,
                file: target.file,
                counts: OpCounts {
                    create: target.create,
                    update: target.update,
                    remove: target.remove,
                    disable: target.disable,
                },
                outcome: target.outcome.into(),
            })
            .collect(),
    })
}

fn entity_diff(change: &EntityChange) -> EntityDiff {
    EntityDiff {
        entity: change.entity.clone(),
        file: change.file.clone(),
        change: change.kind.into(),
        fields: change
            .fields
            .iter()
            .map(|field| FieldDiff {
                field: field.field.clone(),
                before: field.before.as_deref().map(|v| mask_field(&field.field, v)),
                after: field.after.as_deref().map(|v| mask_field(&field.field, v)),
            })
            .collect(),
        text: change.text.as_deref().map(mask_diff),
    }
}

/// One commit, its trailers stripped from the body, and its diffs.
pub fn detail(core: &Core, id: &str) -> CmdResult<CommitDetail> {
    let store = require_store(core)?;
    let id = store.resolve(id)?;
    let (entry, message) = store.entry(&id)?;
    let parent = store.first_parent(&id)?;
    let marks = Marks::load(core, &store)?;
    let body = message
        .lines()
        .skip(1)
        .filter(|line| !line.starts_with("Loadout-"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    Ok(CommitDetail {
        entry: marks.entry(&entry),
        body,
        changes: store
            .semantic_diff(parent.as_ref(), &id)?
            .iter()
            .map(entity_diff)
            .collect(),
        raw: mask_diff(&store.raw_diff(parent.as_ref(), &id)?),
        applied: applied_detail(&store, &entry),
    })
}

/// Tags `commit` (HEAD when `None`) as `name`.
pub fn tag(
    core: &Core,
    name: &str,
    commit: Option<&str>,
    message: Option<&str>,
) -> CmdResult<TagEntry> {
    let store = require_store(core)?;
    let target = match commit {
        Some(commit) => store.resolve(commit)?,
        None => store.head()?,
    };
    let message = message.map(str::trim).filter(|m| !m.is_empty());
    Ok(tag_entry(&store.create_user_tag(name, &target, message)?))
}

/// Deletes a user tag; automatic tags are pruned by age only.
pub fn untag(core: &Core, name: &str) -> CmdResult<()> {
    let store = require_store(core)?;
    let tag = store
        .tag(name)?
        .ok_or_else(|| CoreError::new(ErrorCode::NotFound, format!("there is no tag {name:?}")))?;
    if tag.kind == TagKind::Auto {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            "automatic tags are removed after their retention period only",
        )
        .into());
    }
    store.delete_tag(name)?;
    Ok(())
}

fn request(
    store: &Store<'_>,
    rev: &str,
    entities: &[String],
    confirmed: bool,
) -> CmdResult<RestoreRequest> {
    Ok(RestoreRequest {
        target: store.resolve(rev)?,
        entities: entities.to_vec(),
        confirmed,
        apply: false,
    })
}

/// What restoring `rev` (only `entities`, when given) changes.
pub fn preview(core: &Core, rev: &str, entities: &[String]) -> CmdResult<RestorePreview> {
    let store = require_store(core)?;
    let machine = Machine::load_or_create(&core.env, core.fs.as_ref(), &core.home.state_dir())?;
    let request = request(&store, rev, entities, false)?;
    let plan = restore::prepare(&store, &machine.id, &request)?;
    let files: BTreeSet<&str> = plan.files.iter().map(String::as_str).collect();
    let (mut added, mut changed, mut removed) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    // Changes from HEAD to the target, limited to the files restored.
    for change in store.semantic_diff(Some(&plan.head), &plan.target)? {
        if !files.contains(change.file.as_str()) {
            continue;
        }
        let set = match change.kind {
            ChangeKind::Added => &mut added,
            ChangeKind::Changed => &mut changed,
            ChangeKind::Removed => &mut removed,
        };
        set.insert(change.entity);
    }
    // Entities with an added and a removed file (a skill) changed.
    for entity in added.clone().intersection(&removed.clone()) {
        added.remove(entity);
        removed.remove(entity);
        changed.insert(entity.clone());
    }
    for file in &plan.files {
        let entity = entity_of(file);
        if !added.contains(&entity) && !removed.contains(&entity) {
            changed.insert(entity);
        }
    }
    let project_folder = |project: &str| {
        plan.manifests
            .iter()
            .find(|m| m.project == project)
            .and_then(|m| m.path.parent()?.parent().map(PathBuf::from))
    };
    Ok(RestorePreview {
        target: plan.target.to_string(),
        short_id: plan.target.short().to_owned(),
        added: added.into_iter().collect(),
        changed: changed.into_iter().collect(),
        removed: removed.into_iter().collect(),
        files: plan.files.clone(),
        shared_manifests: plan
            .manifests
            .iter()
            .filter(|m| m.shared)
            .map(|m| placeholder_path(&core.env, &m.path, project_folder(&m.project).as_deref()))
            .collect(),
        unplaced: plan.unplaced.clone(),
    })
}

/// Restores `rev` (only `entities`, when given) as one new commit, tagged
/// `auto/pre-restore/<ts>`. Client files are not touched.
pub fn restore_version(
    core: &Core,
    rev: &str,
    entities: &[String],
    confirmed: bool,
) -> CmdResult<RestoreResult> {
    let store = require_store(core)?;
    let request = request(&store, rev, entities, confirmed)?;
    let report = restore::restore(&store, &core.apply_env(), &request)?;
    Ok(RestoreResult {
        tag: report.tag.clone(),
        commit: report.commit.as_ref().map(CommitId::to_string),
        files: report.plan.files.clone(),
        manifests: report
            .written
            .iter()
            .map(|path| {
                let project = path.parent().and_then(|dir| dir.parent());
                placeholder_path(&core.env, path, project)
            })
            .collect(),
    })
}

/// Every apply run with client file backups on this machine.
pub fn file_backups(core: &Core) -> CmdResult<Vec<BackupRunEntry>> {
    Ok(backup::list_runs(core.fs.as_ref(), &core.home)?
        .into_iter()
        .map(|run| BackupRunEntry {
            undo_of_restore: run.apply_id.starts_with("restore-"),
            run: run.dir,
            apply_id: run.apply_id,
            time: utc_datetime(run.time).to_string(),
            files: run
                .files
                .iter()
                .map(|file| placeholder_path(&core.env, file, None))
                .collect(),
        })
        .collect())
}

/// Writes back the client files of the backup run `run` (its directory
/// name, [`BackupRunEntry::run`]): all of them, or the one listed as `file`
/// by [`file_backups`].
pub fn restore_files(
    core: &Core,
    run: &str,
    file: Option<&str>,
) -> CmdResult<Vec<FileRestoreEntry>> {
    let listed = backup::list_runs(core.fs.as_ref(), &core.home)?
        .into_iter()
        .find(|listed| listed.dir == run)
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::NotFound,
                format!("no backup run {run:?} on this machine"),
            )
        })?;
    let only = match file {
        None => None,
        Some(wanted) => {
            let matches: Vec<PathBuf> = listed
                .files
                .into_iter()
                .filter(|path| placeholder_path(&core.env, path, None) == wanted)
                .collect();
            match matches.as_slice() {
                [one] => Some(one.clone()),
                [] => {
                    return Err(CoreError::new(
                        ErrorCode::NotFound,
                        format!("backup run {run} has no backup of {wanted}"),
                    )
                    .into());
                }
                _ => {
                    return Err(CoreError::new(
                        ErrorCode::InvalidValue,
                        format!("{wanted} names more than one backed-up file; restore the run"),
                    )
                    .into());
                }
            }
        }
    };
    // The exact directory name: never another run of the same apply.
    let results = restore::restore_files(&core.apply_env(), &listed.dir, only.as_deref())?;
    Ok(results
        .iter()
        .map(|result| {
            let (outcome, error) = match &result.outcome {
                FileOutcome::Restored { .. } => (FileRestoreOutcome::Restored, None),
                FileOutcome::Unchanged => (FileRestoreOutcome::Unchanged, None),
                FileOutcome::Failed(err) => (
                    FileRestoreOutcome::Failed,
                    Some(super::scrubbed(&core.env, err, None)),
                ),
            };
            FileRestoreEntry {
                file: placeholder_path(&core.env, &result.file, None),
                outcome,
                error,
            }
        })
        .collect())
}

/// Fields of pulled or restored changes that conflict (SPEC 9.9), as
/// recorded on this machine.
pub fn merge_conflicts(core: &Core) -> CmdResult<Vec<MergeConflictEntry>> {
    Ok(read_merge_state(core.fs.as_ref(), &core.home.state_dir())
        .into_iter()
        .flat_map(|merge| {
            merge
                .conflicts
                .into_iter()
                .map(move |conflict| MergeConflictEntry {
                    source: merge.source.clone(),
                    entity: entity_of(&conflict.file),
                    file: conflict.file,
                    field: conflict.field,
                })
        })
        .collect())
}

/// The zip of `rev` (HEAD when `None`) and its suggested file name.
pub fn zip(core: &Core, rev: Option<&str>) -> CmdResult<(String, Vec<u8>)> {
    let store = require_store(core)?;
    let rev = match rev {
        Some(rev) => store.resolve(rev)?,
        None => store.head()?,
    };
    let machine = Machine::load_or_create(&core.env, core.fs.as_ref(), &core.home.state_dir())?;
    Ok(export_zip(&store, &rev, &machine.id)?)
}

#[tauri::command]
#[specta::specta]
pub async fn list_history(
    state: tauri::State<'_, AppState>,
    filter: HistoryFilter,
) -> CmdResult<Vec<HistoryEntry>> {
    blocking(&state, move |core| history(core, &filter)).await
}

#[tauri::command]
#[specta::specta]
pub async fn commit_detail(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CmdResult<CommitDetail> {
    blocking(&state, move |core| detail(core, &id)).await
}

#[tauri::command]
#[specta::specta]
pub async fn list_tags(state: tauri::State<'_, AppState>) -> CmdResult<Vec<TagEntry>> {
    blocking(&state, tags).await
}

#[tauri::command]
#[specta::specta]
pub async fn create_tag(
    state: tauri::State<'_, AppState>,
    name: String,
    commit: Option<String>,
    message: Option<String>,
) -> CmdResult<TagEntry> {
    blocking(&state, move |core| {
        tag(core, &name, commit.as_deref(), message.as_deref())
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn delete_tag(state: tauri::State<'_, AppState>, name: String) -> CmdResult<()> {
    blocking(&state, move |core| untag(core, &name)).await
}

#[tauri::command]
#[specta::specta]
pub async fn restore_preview(
    state: tauri::State<'_, AppState>,
    rev: String,
    entities: Vec<String>,
) -> CmdResult<RestorePreview> {
    blocking(&state, move |core| preview(core, &rev, &entities)).await
}

#[tauri::command]
#[specta::specta]
pub async fn restore_store_version(
    state: tauri::State<'_, AppState>,
    rev: String,
    entities: Vec<String>,
    confirmed: bool,
) -> CmdResult<RestoreResult> {
    blocking(&state, move |core| {
        restore_version(core, &rev, &entities, confirmed)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn list_file_backups(
    state: tauri::State<'_, AppState>,
) -> CmdResult<Vec<BackupRunEntry>> {
    blocking(&state, file_backups).await
}

#[tauri::command]
#[specta::specta]
pub async fn restore_client_files(
    state: tauri::State<'_, AppState>,
    run: String,
    file: Option<String>,
) -> CmdResult<Vec<FileRestoreEntry>> {
    blocking(&state, move |core| {
        restore_files(core, &run, file.as_deref())
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn list_merge_conflicts(
    state: tauri::State<'_, AppState>,
) -> CmdResult<Vec<MergeConflictEntry>> {
    blocking(&state, merge_conflicts).await
}

/// "Download zip": the version as a zip, saved where the user picks in
/// the OS dialog (run from Rust; the path never crosses IPC). `false` when
/// the dialog was cancelled.
#[tauri::command]
#[specta::specta]
pub async fn save_version_zip(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    rev: Option<String>,
) -> CmdResult<bool> {
    use tauri_plugin_dialog::DialogExt;
    let (name, bytes) = blocking(&state, move |core| zip(core, rev.as_deref())).await?;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_file_name(&name)
            .add_filter("Zip", &["zip"])
            .blocking_save_file()
    })
    .await
    .map_err(|err| IpcError {
        code: ErrorCode::IoError,
        message: format!("the save dialog failed: {err}"),
    })?;
    let Some(picked) = picked else {
        return Ok(false);
    };
    let path = picked.into_path().map_err(|_| IpcError {
        code: ErrorCode::InvalidPath,
        message: "the picked item is not a local file".to_owned(),
    })?;
    blocking(&state, move |core| {
        core.fs.write(&path, &bytes).map_err(|err| IpcError {
            code: ErrorCode::IoError,
            message: format!("cannot write the zip: {err}"),
        })
    })
    .await?;
    Ok(true)
}

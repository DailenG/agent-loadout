//! Restore (SPEC 9.5).
//!
//! - **Store restore** ([`restore`]): tag `auto/pre-restore/<ts>`, then one
//!   new commit whose files equal another revision's, for the whole store
//!   or only some entities. History is never rewritten, so a restore is
//!   undone by restoring its tag. Other machines' `machines/*` files and
//!   this machine's apply record are never changed (single-writer rule).
//!   A restored project manifest is written back to the project folder:
//!   `project.local.toml` always, the shared `project.toml` only when
//!   confirmed, since a team may share it.
//! - **Client-file restore** ([`restore_files`]): the bytes an apply run
//!   backed up, written back through the same atomic compare-and-swap
//!   write as an apply, with the current bytes backed up first and the
//!   ledger updated to what the files hold now.

use std::path::{Path, PathBuf};

use crate::adapters::adapter;
use crate::apply::exec::{ApplyReport, ApplyRequest, apply_in, swap_file};
use crate::apply::ledger::{EntryKind, Ledger};
use crate::apply::plan::{PlanOptions, read_target};
use crate::apply::{ApplyEnv, backup};
use crate::brand;
use crate::error::{CoreError, ErrorCode};
use crate::formats::Document;
use crate::model::TomlDocument;
use crate::model::applied::ApplyTrigger;
use crate::model::machine::MachineOverlay;
use crate::secrets::catalog::overlay_path;
use crate::store::diff::trailer_entities;
use crate::store::merge::Files;
use crate::store::{
    CommitId, LOCK_TIMEOUT, Machine, Operation, Store, StoreLock, StoreRead, entity_paths,
};

/// Manifest file names in `projects/<id>/` and `<P>/.loadout/`.
const SHARED_MANIFEST: &str = "project.toml";
const LOCAL_MANIFEST: &str = "project.local.toml";

/// What to restore.
#[derive(Debug, Clone)]
pub struct RestoreRequest {
    pub target: CommitId,
    /// Only these entities (`server/github`, `secrets`, `project/<id>`);
    /// empty: the whole store.
    pub entities: Vec<String>,
    /// Write a restored shared `project.toml` back to its project folder.
    pub confirmed: bool,
    /// Apply in the same commit.
    pub apply: bool,
}

/// A project manifest a restore writes back to its folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestWrite {
    pub project: String,
    /// Store-relative manifest.
    pub relative: String,
    /// `<P>/.loadout/project[.local].toml`.
    pub path: PathBuf,
    /// The shared `project.toml` (needs confirmation).
    pub shared: bool,
}

/// What a restore changes. Read-only.
#[derive(Debug, Clone)]
pub struct RestorePlan {
    pub target: CommitId,
    /// HEAD the plan was made against.
    pub head: CommitId,
    /// Store files restored (store-relative).
    pub files: Vec<String>,
    /// Manifests written back to project folders.
    pub manifests: Vec<ManifestWrite>,
    /// Restored manifests of projects this machine has no folder for.
    pub unplaced: Vec<String>,
}

impl RestorePlan {
    pub fn needs_confirmation(&self) -> bool {
        self.manifests.iter().any(|manifest| manifest.shared)
    }
}

/// The result of a store restore.
#[derive(Debug)]
pub struct RestoreReport {
    pub plan: RestorePlan,
    /// The `auto/pre-restore/<ts>` tag; `None` when nothing changed.
    pub tag: Option<String>,
    pub commit: Option<CommitId>,
    /// Manifests written back.
    pub written: Vec<PathBuf>,
    pub apply: Option<ApplyReport>,
}

/// Whether a restore may change `file` on machine `machine_id`: every
/// file but other machines' `machines/*` files and this machine's apply
/// record.
fn restorable(file: &str, machine_id: &str) -> bool {
    match file.strip_prefix("machines/") {
        Some(name) => name == format!("{machine_id}.toml"),
        None => true,
    }
}

/// Works out what restoring `request` changes. Read-only.
pub fn prepare(
    store: &Store<'_>,
    machine_id: &str,
    request: &RestoreRequest,
) -> Result<RestorePlan, CoreError> {
    let mut selected: Vec<String> = Vec::new();
    for entity in &request.entities {
        let paths = entity_paths(entity);
        if paths.is_empty() {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!(
                    "unknown entity {entity:?}; expected server/<id>, skill/<id>, project/<id>, \
                     machine/<id>, secrets, clients, settings or sources"
                ),
            ));
        }
        if entity
            .strip_prefix("machine/")
            .is_some_and(|id| id != machine_id)
        {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!("{entity} belongs to another machine; only that machine writes its files"),
            ));
        }
        selected.extend(paths);
    }
    let head = store.head()?;
    let files: Vec<String> = store
        .changed_files(&head, &request.target)?
        .into_iter()
        .filter(|file| restorable(file, machine_id))
        .filter(|file| {
            selected.is_empty()
                || selected
                    .iter()
                    .any(|path| file == path || file.starts_with(&format!("{path}/")))
        })
        .collect();

    let target = store.revision(&request.target)?;
    // Where projects live on this machine, after the restore.
    let overlay_file = overlay_path(machine_id)?;
    let overlay_text = if files.contains(&overlay_file) {
        target.read_file(&overlay_file)?
    } else {
        store.read_file(&overlay_file)?
    };
    let overlay = overlay_text
        .and_then(|text| TomlDocument::<MachineOverlay>::parse(&text).ok())
        .map(TomlDocument::into_value);
    let mut manifests = Vec::new();
    let mut unplaced = Vec::new();
    for file in &files {
        let Some((project, name)) = manifest_parts(file) else {
            continue;
        };
        let Some(bytes) = target.read_bytes(file)? else {
            // A manifest removed by the restore stays in the project folder.
            continue;
        };
        // Only an existing, absolute project folder: a relative or stale
        // path never makes the restore write elsewhere or create folders.
        let folder = overlay
            .as_ref()
            .and_then(|overlay| overlay.projects.as_ref())
            .and_then(|projects| projects.get(project))
            .map(|project| PathBuf::from(&project.path))
            .filter(|folder| folder.is_absolute() && store.fs.is_dir(folder));
        let Some(folder) = folder else {
            unplaced.push(file.clone());
            continue;
        };
        let path = folder.join(brand::PROJECT_DIR).join(name);
        if read_target(store.fs, &path)?.as_deref() == Some(bytes.as_slice()) {
            continue;
        }
        manifests.push(ManifestWrite {
            project: project.to_owned(),
            relative: file.clone(),
            path,
            shared: name == SHARED_MANIFEST,
        });
    }
    Ok(RestorePlan {
        target: request.target.clone(),
        head,
        files,
        manifests,
        unplaced,
    })
}

/// `projects/<id>/project[.local].toml` as `(id, file name)`.
fn manifest_parts(file: &str) -> Option<(&str, &str)> {
    let rest = file.strip_prefix("projects/")?;
    let (id, name) = rest.split_once('/')?;
    matches!(name, SHARED_MANIFEST | LOCAL_MANIFEST).then_some((id, name))
}

/// Restores `request`: tag, one commit (with the apply when asked), then
/// the manifest write-back. Fails with `confirmation_required` before
/// changing anything when a shared manifest would be written back without
/// [`RestoreRequest::confirmed`].
pub fn restore(
    store: &Store<'_>,
    ax: &ApplyEnv<'_>,
    request: &RestoreRequest,
) -> Result<RestoreReport, CoreError> {
    let summary = format!("restore: {}", request.target.short());
    restore_as(store, ax, request, "store.restore", summary, None)
}

/// [`restore`] committed with operation `op` and `summary` (Discard
/// pending is a restore of selected entities, `store.discard`). With
/// `expected_head`, the store must still be at that commit when the plan is
/// made (the caller worked out what to restore from it), else
/// `file_changed_during_apply` before anything changes; the transaction
/// checks the plan's HEAD again under the lock.
pub(crate) fn restore_as(
    store: &Store<'_>,
    ax: &ApplyEnv<'_>,
    request: &RestoreRequest,
    op: &str,
    summary: String,
    expected_head: Option<&CommitId>,
) -> Result<RestoreReport, CoreError> {
    let machine = Machine::load_or_create(ax.env, ax.fs, &ax.home.state_dir())?;
    let plan = prepare(store, &machine.id, request)?;
    if expected_head.is_some_and(|head| *head != plan.head) {
        return Err(CoreError::new(
            ErrorCode::FileChangedDuringApply,
            "the store changed since the pending changes were read; review them again",
        ));
    }
    if plan.needs_confirmation() && !request.confirmed {
        let files: Vec<String> = plan
            .manifests
            .iter()
            .filter(|manifest| manifest.shared)
            .map(|manifest| manifest.path.display().to_string())
            .collect();
        return Err(CoreError::new(
            ErrorCode::ConfirmationRequired,
            format!(
                "the restore writes shared project manifests a team may use ({}); confirm to \
                 write them back",
                files.join(", ")
            ),
        ));
    }
    if plan.files.is_empty() {
        return Ok(RestoreReport {
            plan,
            tag: None,
            commit: None,
            written: Vec::new(),
            apply: None,
        });
    }
    let mut operation = Operation::new(op, summary);
    for entity in trailer_entities(&plan.files) {
        operation = operation.entity(entity);
    }
    let committed = store.transaction(operation, |tx| {
        // The plan was made without the lock: another process may have
        // committed since, and then the files to restore differ.
        if tx.store().head()? != plan.head {
            return Err(CoreError::new(
                ErrorCode::FileChangedDuringApply,
                "the store changed while the restore was being prepared; run it again",
            ));
        }
        let source = tx.store().revision(&plan.target)?;
        let mut contents = Vec::with_capacity(plan.files.len());
        for file in &plan.files {
            contents.push((file.clone(), source.read_bytes(file)?));
        }
        drop(source);
        for (file, content) in contents {
            match content {
                Some(bytes) => tx.write(&file, bytes)?,
                None => tx.remove(&file)?,
            }
        }
        if request.apply {
            let request = ApplyRequest {
                options: PlanOptions::default(),
                trigger: ApplyTrigger::Restore,
            };
            apply_in(tx, ax, &request).map(Some)
        } else {
            Ok(None)
        }
    })?;
    // Tagged once the restore is in, on the commit it replaced.
    let tag = store.create_auto_tag_at("pre-restore", &plan.head)?;

    let target = store.revision(&plan.target)?;
    let mut written = Vec::new();
    for manifest in &plan.manifests {
        let Some(bytes) = target.read_bytes(&manifest.relative)? else {
            continue;
        };
        crate::apply::write_atomic(ax.fs, &manifest.path, &bytes)?;
        written.push(manifest.path.clone());
    }
    Ok(RestoreReport {
        plan,
        tag: Some(tag),
        commit: committed.commit,
        written,
        apply: committed.value,
    })
}

/// Restores an exported tree (a mirror or a zip of this store) as one new
/// commit, like [`restore`] does a revision: files the export holds are
/// written, files it lacks are removed, never another machine's
/// `machines/*` files or this machine's apply record. The `auto/pre-restore`
/// tag marks the replaced HEAD. Manifests are not written back.
pub fn restore_tree(
    store: &Store<'_>,
    ax: &ApplyEnv<'_>,
    files: &Files,
    label: &str,
    apply: bool,
) -> Result<RestoreReport, CoreError> {
    let machine = Machine::load_or_create(ax.env, ax.fs, &ax.home.state_dir())?;
    let head = store.head()?;
    let current = store.files_at(&head)?;
    let changed: Vec<String> = current
        .keys()
        .chain(files.keys())
        .filter(|file| restorable(file, &machine.id))
        .filter(|file| current.get(*file) != files.get(*file))
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let plan = RestorePlan {
        target: head.clone(),
        head: head.clone(),
        files: changed,
        manifests: Vec::new(),
        unplaced: Vec::new(),
    };
    if plan.files.is_empty() {
        return Ok(RestoreReport {
            plan,
            tag: None,
            commit: None,
            written: Vec::new(),
            apply: None,
        });
    }
    let mut operation = Operation::new("store.restore", format!("restore: from {label}"));
    for entity in trailer_entities(&plan.files) {
        operation = operation.entity(entity);
    }
    let committed = store.transaction(operation, |tx| {
        if tx.store().head()? != plan.head {
            return Err(CoreError::new(
                ErrorCode::FileChangedDuringApply,
                "the store changed while the restore was being prepared; run it again",
            ));
        }
        for file in &plan.files {
            match files.get(file) {
                Some(bytes) => tx.write(file, bytes.clone())?,
                None => tx.remove(file)?,
            }
        }
        if apply {
            let request = ApplyRequest {
                options: PlanOptions::default(),
                trigger: ApplyTrigger::Restore,
            };
            apply_in(tx, ax, &request).map(Some)
        } else {
            Ok(None)
        }
    })?;
    let tag = store.create_auto_tag_at("pre-restore", &plan.head)?;
    Ok(RestoreReport {
        plan,
        tag: Some(tag),
        commit: committed.commit,
        written: Vec::new(),
        apply: committed.value,
    })
}

/// What restoring one client file did.
#[derive(Debug)]
pub enum FileOutcome {
    /// The backed-up bytes are back; the bytes it had are in `previous`.
    Restored { previous: Option<PathBuf> },
    /// The file already holds the backed-up bytes.
    Unchanged,
    /// Not restored (`file_changed_during_apply`, `file_locked`, I/O).
    Failed(CoreError),
    /// Not restored: its backup held old secret values and was removed on
    /// request (M35), so the content it had before the run is no longer
    /// kept.
    Removed,
}

/// One client file of a [`restore_files`] run.
#[derive(Debug)]
pub struct FileRestore {
    pub file: PathBuf,
    pub outcome: FileOutcome,
}

/// Restores the client files a backup run holds
/// (`backups/files/<ts>-<apply-id>/`), all of them or only `only`. `run`
/// is the run's exact directory name (`<ts>-<apply-id>`, what the app
/// lists), or an apply id, which names that apply's newest run. Each
/// file's current bytes go to a new run `<ts>-restore-<apply-id>-<ulid>`
/// first (unique, so restoring twice never makes two runs of one id), so
/// this is undoable the same way. A file whose backup was removed because
/// it held old secret values is reported [`FileOutcome::Removed`]. Holds
/// the store lock.
pub fn restore_files(
    ax: &ApplyEnv<'_>,
    run: &str,
    only: Option<&Path>,
) -> Result<Vec<FileRestore>, CoreError> {
    let fs = ax.fs;
    let state_dir = ax.home.state_dir();
    let _lock = StoreLock::acquire(fs, &state_dir, LOCK_TIMEOUT)?;
    let apply_id = run;
    // A run result that cannot be read only leaves the removed backups
    // unreported; the backups on disk still restore.
    let removed: Vec<PathBuf> = backup::removed_backups(fs, &state_dir, run_apply_id(run))
        .unwrap_or_default()
        .into_iter()
        .filter(|file| only.is_none_or(|only| same_path(ax, file, only)))
        .collect();
    // A run whose every backup was removed has no directory any more.
    let run = match find_run(ax, run) {
        Ok(run) => Some(run),
        Err(err) if err.code() == ErrorCode::NotFound && !removed.is_empty() => None,
        Err(err) => return Err(err),
    };
    let entries = match &run {
        Some(run) => fs.read_dir(run).map_err(|err| {
            CoreError::new(ErrorCode::IoError, format!("cannot list backups: {err}")).with_path(run)
        })?,
        None => Vec::new(),
    };
    let mut files: Vec<(PathBuf, PathBuf)> = entries
        .into_iter()
        .filter_map(|backup| {
            let name = backup.file_name()?.to_str()?.to_owned();
            let original = PathBuf::from(backup::decode_path(&name)?);
            Some((original, backup))
        })
        .filter(|(original, _)| only.is_none_or(|only| same_path(ax, original, only)))
        // The record wins over the listing: a backup recorded as removed may
        // still be on disk, zeroed, and is never written back.
        .filter(|(original, _)| !removed.iter().any(|file| same_path(ax, original, file)))
        .collect();
    files.sort();
    if files.is_empty() && removed.is_empty() {
        return Err(CoreError::new(
            ErrorCode::NotFound,
            match only {
                Some(only) => format!("apply {apply_id} has no backup of {}", only.display()),
                None => format!("apply {apply_id} has no file backups"),
            },
        ));
    }
    let mut results: Vec<FileRestore> = removed
        .into_iter()
        .map(|file| FileRestore {
            file,
            outcome: FileOutcome::Removed,
        })
        .collect();
    let Some(run) = run else {
        return Ok(results);
    };
    // `restore-<apply id>-<ulid>`, with the apply id the run goes back to
    // (a restore of a restore names the original apply), so names never
    // grow.
    let run_id = run
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.split_once('-'))
        .map_or(apply_id, |(_, id)| id);
    let base = run_id
        .strip_prefix("restore-")
        .and_then(|rest| rest.split('-').next())
        .unwrap_or(run_id);
    let undo_id = format!(
        "restore-{base}-{}",
        crate::model::ids::new_ulid(ax.env.now())?
    );
    let undo_dir = backup::run_dir(ax.home, ax.env.now(), &undo_id);
    let mut ledger = Ledger::load(fs, &state_dir)?;
    for (file, saved) in files {
        // A failed file never prevents the others; the ledger is saved
        // after each restored file (SPEC 8.4 step 5).
        let outcome = restore_one(ax, &undo_dir, &file, &saved).and_then(|outcome| {
            if matches!(outcome, FileOutcome::Restored { .. }) {
                refresh_ledger(ax, &mut ledger, &file)?;
                ledger.save(fs, &state_dir)?;
            }
            Ok(outcome)
        });
        let outcome = outcome.unwrap_or_else(|err| FileOutcome::Failed(err.with_path(&file)));
        results.push(FileRestore { file, outcome });
    }
    Ok(results)
}

/// The apply id a run argument names: the id in `<ts>-<apply id>`, else the
/// argument itself.
fn run_apply_id(run: &str) -> &str {
    match run.get(..16).and_then(backup::parse_timestamp) {
        Some(_) if run.as_bytes().get(16) == Some(&b'-') => &run[17..],
        _ => run,
    }
}

/// The backup run directory named `run` exactly, else the newest run of
/// apply id `run`.
fn find_run(ax: &ApplyEnv<'_>, run: &str) -> Result<PathBuf, CoreError> {
    let root = ax.home.backups_dir().join(backup::FILES_DIR);
    let runs = if ax.fs.is_dir(&root) {
        ax.fs.read_dir(&root).map_err(|err| {
            CoreError::new(ErrorCode::IoError, format!("cannot list backups: {err}"))
                .with_path(&root)
        })?
    } else {
        Vec::new()
    };
    let runs: Vec<(String, PathBuf)> = runs
        .into_iter()
        .filter(|dir| ax.fs.is_dir(dir))
        .filter_map(|dir| Some((dir.file_name()?.to_str()?.to_owned(), dir)))
        .collect();
    if let Some((_, dir)) = runs.iter().find(|(name, _)| name == run) {
        return Ok(dir.clone());
    }
    // Directory names start with a sortable timestamp: the greatest is the
    // newest.
    runs.into_iter()
        .filter(|(name, _)| {
            name.split_once('-')
                .is_some_and(|(_, id)| id.eq_ignore_ascii_case(run))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, dir)| dir)
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::NotFound,
                format!("no file backups of {run:?} on this machine"),
            )
        })
}

fn same_path(ax: &ApplyEnv<'_>, a: &Path, b: &Path) -> bool {
    match ax.env.os() {
        crate::env::Os::Windows => a
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy()),
        _ => a == b,
    }
}

/// Writes the backed-up bytes of `file` back: current bytes saved to
/// `undo_dir`, temp file, compare-and-swap check, replace.
fn restore_one(
    ax: &ApplyEnv<'_>,
    undo_dir: &Path,
    file: &Path,
    saved: &Path,
) -> Result<FileOutcome, CoreError> {
    let fs = ax.fs;
    let bytes = fs.read(saved).map_err(|err| {
        CoreError::new(ErrorCode::IoError, format!("cannot read backup: {err}")).with_path(saved)
    })?;
    let current = read_target(fs, file)?;
    if current.as_deref() == Some(bytes.as_slice()) {
        return Ok(FileOutcome::Unchanged);
    }
    let previous = match &current {
        Some(current) => Some(backup::save(fs, ax.home, undo_dir, file, current)?),
        None => None,
    };
    if !swap_file(fs, file, current.as_deref(), &bytes)? {
        return Err(CoreError::new(
            ErrorCode::FileChangedDuringApply,
            "the file changed while it was being restored; nothing was written",
        ));
    }
    Ok(FileOutcome::Restored { previous })
}

/// After a restore, owned entries that are no longer in `file` are no
/// longer owned. Every other entry keeps its ledger record as it is, hash
/// included: what the file holds now is compared with what Loadout wrote,
/// so a restored hand edit plans as drift and a restored foreign entry as
/// a collision, never as Loadout's own. A file that does not parse keeps
/// every record. Only MCP entries are checked (see the M11 note).
fn refresh_ledger(ax: &ApplyEnv<'_>, ledger: &mut Ledger, file: &Path) -> Result<(), CoreError> {
    let name = file.to_string_lossy().into_owned();
    let bytes = read_target(ax.fs, file)?;
    let owned: Vec<_> = ledger
        .entries
        .iter()
        .filter(|entry| entry.file == name && entry.kind == EntryKind::McpEntry)
        .cloned()
        .collect();
    for entry in owned {
        let present = match &bytes {
            None => false,
            Some(bytes) => {
                let Some(format) = adapter(&entry.client)
                    .and_then(|spec| spec.mcp.as_ref())
                    .map(|mcp| mcp.format)
                else {
                    continue;
                };
                let Ok(doc) = Document::parse(bytes, format) else {
                    continue;
                };
                let keys = pointer_keys(&entry.container);
                let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
                !matches!(doc.get(&keys, &entry.key), Ok(None))
            }
        };
        if !present {
            ledger.remove(&entry.slot(), &entry.key);
        }
    }
    Ok(())
}

/// The keys of an RFC 6901 pointer (`/projects/C:~1code/mcpServers`).
fn pointer_keys(pointer: &str) -> Vec<String> {
    pointer
        .split('/')
        .skip(1)
        .map(|key| key.replace("~1", "/").replace("~0", "~"))
        .collect()
}

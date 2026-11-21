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
use crate::apply::exec::{ApplyReport, ApplyRequest, apply_in};
use crate::apply::ledger::{EntryKind, Ledger, LedgerKey};
use crate::apply::plan::{PlanOptions, canonical, read_target};
use crate::apply::{ApplyEnv, backup, temp_path};
use crate::brand;
use crate::error::{CoreError, ErrorCode};
use crate::formats::Document;
use crate::model::TomlDocument;
use crate::model::applied::ApplyTrigger;
use crate::model::machine::MachineOverlay;
use crate::secrets::catalog::overlay_path;
use crate::store::diff::trailer_entities;
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
                     machine/<id>, secrets, clients or settings"
                ),
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
        let folder = overlay
            .as_ref()
            .and_then(|overlay| overlay.projects.as_ref())
            .and_then(|projects| projects.get(project))
            .map(|project| PathBuf::from(&project.path));
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
    let machine = Machine::load_or_create(ax.env, ax.fs, &ax.home.state_dir())?;
    let plan = prepare(store, &machine.id, request)?;
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
    let tag = store.create_auto_tag("pre-restore")?;

    let mut operation = Operation::new(
        "store.restore",
        format!("restore: {}", request.target.short()),
    );
    for entity in trailer_entities(&plan.files) {
        operation = operation.entity(entity);
    }
    let committed = store.transaction(operation, |tx| {
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

/// What restoring one client file did.
#[derive(Debug)]
pub enum FileOutcome {
    /// The backed-up bytes are back; the bytes it had are in `previous`.
    Restored { previous: Option<PathBuf> },
    /// The file already holds the backed-up bytes.
    Unchanged,
    /// Not restored (`file_changed_during_apply`, `file_locked`, I/O).
    Failed(CoreError),
}

/// One client file of a [`restore_files`] run.
#[derive(Debug)]
pub struct FileRestore {
    pub file: PathBuf,
    pub outcome: FileOutcome,
}

/// Restores the client files an apply run backed up
/// (`backups/files/<ts>-<apply-id>/`), all of them or only `only`. Each
/// file's current bytes go to a new backup run `<ts>-restore-<apply-id>`
/// first, so this is undoable the same way. Holds the store lock.
pub fn restore_files(
    ax: &ApplyEnv<'_>,
    apply_id: &str,
    only: Option<&Path>,
) -> Result<Vec<FileRestore>, CoreError> {
    let fs = ax.fs;
    let state_dir = ax.home.state_dir();
    let _lock = StoreLock::acquire(fs, &state_dir, LOCK_TIMEOUT)?;
    let run = find_run(ax, apply_id)?;
    let entries = fs.read_dir(&run).map_err(|err| {
        CoreError::new(ErrorCode::IoError, format!("cannot list backups: {err}")).with_path(&run)
    })?;
    let mut files: Vec<(PathBuf, PathBuf)> = entries
        .into_iter()
        .filter_map(|backup| {
            let name = backup.file_name()?.to_str()?.to_owned();
            let original = PathBuf::from(backup::decode_path(&name)?);
            Some((original, backup))
        })
        .filter(|(original, _)| only.is_none_or(|only| same_path(ax, original, only)))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(CoreError::new(
            ErrorCode::NotFound,
            match only {
                Some(only) => format!("apply {apply_id} has no backup of {}", only.display()),
                None => format!("apply {apply_id} has no file backups"),
            },
        ));
    }

    let undo_dir = backup::run_dir(ax.home, ax.env.now(), &format!("restore-{apply_id}"));
    let mut ledger = Ledger::load(fs, &state_dir)?;
    let key = LedgerKey::load_or_create(fs, &state_dir)?;
    let mut results = Vec::new();
    for (file, saved) in files {
        let outcome = match restore_one(ax, &undo_dir, &file, &saved) {
            Ok(outcome) => {
                if matches!(outcome, FileOutcome::Restored { .. }) {
                    refresh_ledger(ax, &mut ledger, &key, &file)?;
                }
                outcome
            }
            Err(err) => FileOutcome::Failed(err.with_path(&file)),
        };
        results.push(FileRestore { file, outcome });
    }
    ledger.save(fs, &state_dir)?;
    Ok(results)
}

/// The backup run directory of `apply_id`.
fn find_run(ax: &ApplyEnv<'_>, apply_id: &str) -> Result<PathBuf, CoreError> {
    let root = ax.home.backups_dir().join(backup::FILES_DIR);
    let runs = if ax.fs.is_dir(&root) {
        ax.fs.read_dir(&root).map_err(|err| {
            CoreError::new(ErrorCode::IoError, format!("cannot list backups: {err}"))
                .with_path(&root)
        })?
    } else {
        Vec::new()
    };
    runs.into_iter()
        .filter(|dir| ax.fs.is_dir(dir))
        .find(|dir| {
            dir.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.split_once('-'))
                .is_some_and(|(_, id)| id.eq_ignore_ascii_case(apply_id))
        })
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::NotFound,
                format!("no file backups of apply {apply_id:?} on this machine"),
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
    let io = |what: &str, at: &Path, err: std::io::Error| {
        CoreError::new(ErrorCode::IoError, format!("{what}: {err}")).with_path(at)
    };
    let bytes = fs
        .read(saved)
        .map_err(|err| io("cannot read backup", saved, err))?;
    let current = read_target(fs, file)?;
    if current.as_deref() == Some(bytes.as_slice()) {
        return Ok(FileOutcome::Unchanged);
    }
    let previous = match &current {
        Some(current) => Some(backup::save(fs, ax.home, undo_dir, file, current)?),
        None => None,
    };
    if let Some(dir) = file.parent() {
        fs.create_dir_all(dir)
            .map_err(|err| io("cannot create directory", dir, err))?;
    }
    let temp = temp_path(file)?;
    fs.write_new(&temp, &bytes, current.is_some().then_some(file))
        .map_err(|err| io("cannot write temp file", &temp, err))?;
    if read_target(fs, file)? != current {
        let _ = fs.remove_file(&temp);
        return Err(CoreError::new(
            ErrorCode::FileChangedDuringApply,
            "the file changed while it was being restored; nothing was written",
        ));
    }
    fs.replace(&temp, file).map_err(|err| {
        let _ = fs.remove_file(&temp);
        io("cannot replace file", file, err)
    })?;
    Ok(FileOutcome::Restored { previous })
}

/// After a restore, the ledger records what `file` holds: owned entries
/// that are gone are dropped, the others get the hash of their restored
/// form (the restore is a write by this machine).
fn refresh_ledger(
    ax: &ApplyEnv<'_>,
    ledger: &mut Ledger,
    key: &LedgerKey,
    file: &Path,
) -> Result<(), CoreError> {
    let name = file.to_string_lossy().into_owned();
    let bytes = read_target(ax.fs, file)?;
    let owned: Vec<_> = ledger
        .entries
        .iter()
        .filter(|entry| entry.file == name && entry.kind == EntryKind::McpEntry)
        .cloned()
        .collect();
    for mut entry in owned {
        let format = adapter(&entry.client)
            .and_then(|spec| spec.mcp.as_ref())
            .map(|mcp| mcp.format);
        let value = match (&bytes, format) {
            (Some(bytes), Some(format)) => {
                let keys = pointer_keys(&entry.container);
                let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
                Document::parse(bytes, format)
                    .ok()
                    .and_then(|doc| doc.get(&keys, &entry.key).ok().flatten())
            }
            _ => None,
        };
        match value.and_then(|value| canonical(&entry.client, &value)) {
            Some(canonical) => {
                entry.written_hash = Some(key.hash(&canonical));
                ledger.upsert(entry);
            }
            None => {
                ledger.remove(&entry.file, &entry.container, &entry.key);
            }
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

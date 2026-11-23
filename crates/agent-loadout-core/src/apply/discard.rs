//! Discard pending (SPEC 13.4, review mode): throw away store edits this
//! machine has not applied yet, without touching any client file.
//!
//! "Last applied" is the desired state of this machine's newest apply
//! commit (the commit carrying its `Loadout-Apply` trailer), or the
//! initial commit when this machine never applied: the same base "Undo
//! this apply" restores to (SPEC 8.7). What waits for an apply in review
//! mode are edits of server definitions (`servers/<id>.toml`) and of the
//! client preferences (`clients.toml`); those are the discardable
//! entities. Everything else keeps its own path: settings (the review
//! mode itself) and the secret catalog are not apply edits, skills sync
//! at once, project manifests live in the project folder, and this
//! machine's overlay records known projects and decisions already applied.
//!
//! Discarding is a store restore of the chosen entities to that base
//! (SPEC 9.5): an `auto/pre-restore/<ts>` tag, then one new commit
//! (`discard: ...`, op `store.discard`), never applied. History is not
//! rewritten, so a discard is undone like any restore.

use crate::apply::ApplyEnv;
use crate::apply::rollback::{apply_commits, initial_commit};
use crate::error::{CoreError, ErrorCode};
use crate::store::diff::entity_of;
use crate::store::restore::{RestoreReport, RestoreRequest, restore_as};
use crate::store::{CommitId, Machine, Store};

/// How an entity differs from the last applied state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingKind {
    /// New since the last apply: discarding removes it.
    Added,
    Changed,
    /// Removed since the last apply: discarding brings it back.
    Removed,
}

/// One discardable entity (`server/<id>` or `clients`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingStoreChange {
    pub entity: String,
    pub kind: PendingKind,
}

/// The store edits waiting for an apply on this machine.
#[derive(Debug, Clone)]
pub struct PendingStore {
    /// The last applied commit (the restore target).
    pub base: CommitId,
    /// HEAD when the changes were read; [`discard`] refuses another.
    pub head: CommitId,
    pub changes: Vec<PendingStoreChange>,
}

/// Whether a store file holds desired state that waits for an apply in
/// review mode.
fn discardable(file: &str) -> bool {
    file == "clients.toml"
        || file
            .strip_prefix("servers/")
            .is_some_and(|name| name.ends_with(".toml") && !name.contains('/'))
}

/// This machine's discardable store edits since its last apply, in file
/// order. `None` when there is no machine identity yet (nothing applied,
/// nothing edited here).
pub fn pending(store: &Store<'_>, ax: &ApplyEnv<'_>) -> Result<Option<PendingStore>, CoreError> {
    let Some(machine) = Machine::load(ax.fs, &ax.home.state_dir())? else {
        return Ok(None);
    };
    let head = store.head()?;
    let base = match apply_commits(store, &machine.id)?.first() {
        Some(last) => last.id.clone(),
        None => initial_commit(store, &head)?,
    };
    let at_base = store.revision(&base)?;
    let at_head = store.revision(&head)?;
    let mut changes = Vec::new();
    for file in store.changed_files(&base, &head)? {
        if !discardable(&file) {
            continue;
        }
        let kind = match (at_base.read_bytes(&file)?, at_head.read_bytes(&file)?) {
            (None, Some(_)) => PendingKind::Added,
            (Some(_), None) => PendingKind::Removed,
            (Some(old), Some(new)) if old != new => PendingKind::Changed,
            _ => continue,
        };
        changes.push(PendingStoreChange {
            entity: entity_of(&file),
            kind,
        });
    }
    Ok(Some(PendingStore {
        base,
        head,
        changes,
    }))
}

/// Discards the pending edits of `entities` (`None`: all of them) of
/// `pending` (what [`pending`] returned) by restoring them to its base, in
/// one commit that is not applied. An entity without a pending edit is
/// `not_found`. When the store moved on since `pending` was read (another
/// commit landed), nothing is discarded: `file_changed_during_apply`, so a
/// change the caller never saw is not thrown away. Nothing to discard makes
/// no commit.
pub fn discard(
    store: &Store<'_>,
    ax: &ApplyEnv<'_>,
    pending: &PendingStore,
    entities: Option<&[String]>,
) -> Result<Option<RestoreReport>, CoreError> {
    let chosen: Vec<String> = match entities {
        None => pending
            .changes
            .iter()
            .map(|change| change.entity.clone())
            .collect(),
        Some(entities) => {
            let mut chosen: Vec<String> = Vec::new();
            for entity in entities {
                if !pending
                    .changes
                    .iter()
                    .any(|change| &change.entity == entity)
                {
                    return Err(CoreError::new(
                        ErrorCode::NotFound,
                        format!("{entity} has no pending change to discard"),
                    ));
                }
                if !chosen.contains(entity) {
                    chosen.push(entity.clone());
                }
            }
            chosen
        }
    };
    // An empty entity list would restore the whole store.
    if chosen.is_empty() {
        return Ok(None);
    }
    let summary = match chosen.as_slice() {
        [one] => format!("discard: {one}"),
        many => format!("discard: {} pending changes", many.len()),
    };
    let request = RestoreRequest {
        target: pending.base.clone(),
        entities: chosen,
        confirmed: false,
        apply: false,
    };
    restore_as(
        store,
        ax,
        &request,
        "store.discard",
        summary,
        Some(&pending.head),
    )
    .map(Some)
}

//! The drift resolver (SPEC 8.2, 8.6, 13.4): one entry of the pending plan
//! that needs a decision, in three columns (last written, current file,
//! desired), and the actions its state allows, carried out by the
//! planner's per-entry decision on that file alone, in one commit.

use agent_loadout_core::apply::resolve::{filter_of, fingerprint, resolve, three_way};
use agent_loadout_core::apply::{
    self, EntryPlan, EntryState, OnConflict, PlanOptions, TargetPlan, placeholder_path,
};
use agent_loadout_core::model::applied::Scope as TargetScope;
use agent_loadout_core::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use specta::Type;

use super::apply::{FileRef, file_ref, mask_diff};
use super::{
    AppState, ChangeResult, CmdResult, Core, EntryStateKind, IpcError, blocking, scrubbed,
};

/// One entry of a drawer file: the file as the drawer names it, the client
/// and the native entry name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EntryRef {
    pub file: FileRef,
    pub client: String,
    pub name: String,
}

/// A resolver action (SPEC 8.2 default actions, 13.4 buttons).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ResolveAction {
    /// Write the desired entry over the file's (adopting a collision).
    Overwrite,
    /// Turn the file's entry into the store server ("Import their
    /// change", "Import theirs").
    ImportTheirs,
    /// Stop managing the entry: it stays as it is, and the server is no
    /// longer enabled for this client.
    Release,
    /// Leave it for now.
    Skip,
    /// A deleted entry: write it again.
    Recreate,
    /// A deleted entry: remove the client from the server's enable list.
    RespectDeletion,
    /// A drifted entry no longer desired: remove it anyway.
    RemoveAnyway,
}

impl ResolveAction {
    fn resolution(self) -> OnConflict {
        match self {
            Self::Overwrite | Self::Recreate | Self::RemoveAnyway => OnConflict::Overwrite,
            Self::ImportTheirs | Self::RespectDeletion => OnConflict::Import,
            Self::Release => OnConflict::Release,
            Self::Skip => OnConflict::Skip,
        }
    }
}

/// What the resolver shows for one entry.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ResolverDetail {
    /// The file under its placeholder root.
    pub file: String,
    /// The project folder under its placeholder root, for a project file.
    pub project: Option<String>,
    pub client: String,
    pub name: String,
    pub server: Option<String>,
    pub state: EntryStateKind,
    /// The entry as Loadout last wrote it; `None` when it was never
    /// written (a collision) or cannot be recovered.
    pub last_written: Option<String>,
    /// The entry in the file now; `None` when it was deleted.
    pub current: Option<String>,
    /// The entry the store wants; `None` when it is no longer desired.
    pub desired: Option<String>,
    /// The actions this state allows, the default first.
    pub actions: Vec<ResolveAction>,
    pub problem: Option<IpcError>,
    /// Keyed hash of what is shown (state, file entry, desired entry);
    /// `resolve_entry` sends it back and is refused (`entry_changed`) when
    /// the entry changed in between.
    pub fingerprint: String,
}

/// The actions `entry` allows. Imports and releases edit store servers, so
/// a project entry (from its manifests) does not offer them.
fn actions(entry: &EntryPlan, project: bool) -> Vec<ResolveAction> {
    use ResolveAction::*;
    let store_edits = !project && entry.server_id.is_some();
    let mut out = match (entry.state, entry.desired.is_some()) {
        (EntryState::Drifted | EntryState::Conflict, true) => {
            vec![Overwrite, ImportTheirs, Release, Skip]
        }
        (EntryState::Drifted, false) => vec![RemoveAnyway, Release, Skip],
        (EntryState::Missing, _) => vec![Recreate, RespectDeletion, Skip],
        (EntryState::Collision, _) => vec![Overwrite, ImportTheirs, Skip],
        _ => Vec::new(),
    };
    if !store_edits {
        out.retain(|action| {
            !matches!(action, ImportTheirs | RespectDeletion)
                && !(*action == Release && entry.desired.is_some())
        });
    }
    out
}

/// The current plan's target and entry `entry` names.
fn locate(core: &Core, entry: &EntryRef) -> Result<(TargetPlan, EntryPlan), CoreError> {
    let store = core.open_store()?;
    let plan = apply::preview(store.as_ref(), &core.apply_env(), &PlanOptions::default())?;
    let gone = || {
        CoreError::new(
            ErrorCode::NotFound,
            "the entry is not in the pending plan any more",
        )
    };
    let target = plan
        .targets
        .into_iter()
        .find(|target| file_ref(&core.env, target) == entry.file)
        .ok_or_else(gone)?;
    let found = target
        .entries
        .iter()
        .find(|e| e.name == entry.name && e.client == Some(entry.client.as_str()))
        .cloned()
        .ok_or_else(gone)?;
    Ok((target, found))
}

/// The resolver for `entry`: its three columns, masked, and its actions.
pub fn detail(core: &Core, entry: &EntryRef) -> CmdResult<ResolverDetail> {
    let (target, found) = locate(core, entry)?;
    let root = target.project.as_deref();
    let store = core.open_store()?;
    let columns = three_way(store.as_ref(), &core.apply_env(), &target, &found)
        .map_err(|err| scrubbed(&core.env, &err, root))?;
    let show =
        |text: Option<String>| text.map(|text| mask_diff(&super::scrub(&core.env, &text, root)));
    Ok(ResolverDetail {
        file: placeholder_path(&core.env, &target.file, root),
        project: root.map(|root| placeholder_path(&core.env, root, None)),
        client: entry.client.clone(),
        name: found.name.clone(),
        server: found.server_id.clone(),
        state: found.state.into(),
        last_written: show(columns.last_written),
        current: show(columns.current),
        desired: show(columns.desired),
        actions: actions(&found, target.scope != TargetScope::User),
        problem: found
            .problem
            .as_ref()
            .map(|err| scrubbed(&core.env, err, root)),
        fingerprint: fingerprint(&core.apply_env(), &found)?,
    })
}

/// Carries out `action` for `entry`: the planner's decision for that entry
/// on its file alone (other pending changes of the same file apply with
/// it), in one store commit. `skip` changes nothing. Refused with
/// `entry_changed` unless the entry still has `shown` (the fingerprint of
/// the detail the user acted on); the core checks it again inside the
/// transaction.
pub fn run(
    core: &Core,
    entry: &EntryRef,
    action: ResolveAction,
    shown: &str,
) -> CmdResult<ChangeResult> {
    let (target, found) = locate(core, entry)?;
    let root = target.project.clone();
    if fingerprint(&core.apply_env(), &found)? != shown {
        return Err(IpcError {
            code: ErrorCode::EntryChanged,
            message: format!(
                "{}/{} changed since the resolver showed it; nothing was done",
                entry.client, entry.name
            ),
        });
    }
    let allowed = actions(&found, target.scope != TargetScope::User);
    if !allowed.contains(&action) {
        return Err(IpcError {
            code: ErrorCode::InvalidValue,
            message: format!(
                "{action:?} does not apply to a {} entry here",
                found.state.as_str()
            ),
        });
    }
    if action == ResolveAction::Skip {
        return Ok(ChangeResult::new(&core.env, None, None));
    }
    let store = core.open_store_for_change()?;
    let (report, commit) = resolve(
        &store,
        &core.apply_env(),
        filter_of(&target),
        &entry.client,
        &entry.name,
        found.state,
        action.resolution(),
        shown,
    )
    .map_err(|err| scrubbed(&core.env, &err, root.as_deref()))?;
    Ok(ChangeResult::new(&core.env, commit.as_ref(), Some(&report)))
}

#[tauri::command]
#[specta::specta]
pub async fn resolver_detail(
    state: tauri::State<'_, AppState>,
    entry: EntryRef,
) -> CmdResult<ResolverDetail> {
    blocking(&state, move |core| detail(core, &entry)).await
}

#[tauri::command]
#[specta::specta]
pub async fn resolve_entry(
    state: tauri::State<'_, AppState>,
    entry: EntryRef,
    action: ResolveAction,
    fingerprint: String,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| run(core, &entry, action, &fingerprint)).await
}

//! Apply drawer commands (SPEC 13.4) and "Undo this apply" (SPEC 8.7):
//! the pending plan with masked diffs, `loadout apply` (all clients or the
//! selected files' clients) and `loadout rollback`.

use agent_loadout_core::Env;
use agent_loadout_core::apply::render::DesiredState;
use agent_loadout_core::apply::rollback::{self, ApplySelector};
use agent_loadout_core::apply::{
    self, Action, ApplyRequest, EntryState, PlanOptions, ScopeFilter, TargetPlan, placeholder_path,
};
use agent_loadout_core::model::applied::ApplyTrigger;
use agent_loadout_core::model::settings::ApplyMode;
use agent_loadout_core::store::Machine;
use serde::Serialize;
use specta::Type;

use super::{
    AppState, ApplyRun, ChangeResult, CmdResult, Core, EntryNote, FileScope, IpcError, OpCounts,
    blocking, count, errors,
};

/// What the executor does with an entry (SPEC 8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    None,
    Create,
    Update,
    Remove,
    Disable,
    Adopt,
    AdoptOverwrite,
    Refresh,
    Forget,
    Release,
    Import,
    Skip,
}

impl From<Action> for ActionKind {
    fn from(action: Action) -> Self {
        match action {
            Action::None => Self::None,
            Action::Create => Self::Create,
            Action::Update => Self::Update,
            Action::Remove => Self::Remove,
            Action::Disable => Self::Disable,
            Action::Adopt => Self::Adopt,
            Action::AdoptOverwrite => Self::AdoptOverwrite,
            Action::Refresh => Self::Refresh,
            Action::Forget => Self::Forget,
            Action::Release => Self::Release,
            Action::Import => Self::Import,
            Action::Skip => Self::Skip,
        }
    }
}

/// Whether store edits apply at once (the default) or wait for review
/// (SPEC 8.7 `apply_mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ApplyModeKind {
    Instant,
    Review,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PlannedEntry {
    #[serde(flatten)]
    pub note: EntryNote,
    pub action: ActionKind,
}

/// One client file of the pending plan.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PlannedFile {
    pub file: String,
    pub clients: Vec<String>,
    pub scope: FileScope,
    pub project_id: Option<String>,
    pub counts: OpCounts,
    /// Unified diff of the file text, values written `inline` masked;
    /// empty when the file stays as it is.
    pub diff: String,
    /// Entries that change, need a decision or are blocked.
    pub entries: Vec<PlannedEntry>,
    /// The whole file is blocked (`parse_error`, `trust_required`, ...).
    pub problem: Option<IpcError>,
    pub warnings: Vec<IpcError>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PendingPlan {
    pub apply_mode: ApplyModeKind,
    /// Files with work, decisions or problems.
    pub files: Vec<PlannedFile>,
    /// Entries waiting for an apply, a decision, or blocked: the header's
    /// pending-changes pill (SPEC 13.1).
    pub pending: u32,
    pub writes: u32,
    pub unresolved: u32,
    pub warnings: Vec<IpcError>,
}

/// An apply run this machine recorded, newest first.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppliedRun {
    pub apply_id: String,
    pub commit: String,
    /// RFC 3339, UTC.
    pub time: String,
    pub summary: String,
    /// The newest run: undoing it never asks (SPEC 8.7).
    pub latest: bool,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UndoResult {
    pub apply_id: String,
    /// Store files restored from before the run.
    pub restored: Vec<String>,
    /// The `auto/pre-rollback/<ts>` tag.
    pub tag: String,
    pub commit: Option<String>,
    pub apply: ApplyRun,
}

fn needs_attention(entry: &agent_loadout_core::apply::EntryPlan) -> bool {
    entry.action != Action::None
        || entry.unresolved
        || entry.problem.is_some()
        || entry.state == EntryState::Blocked
}

fn planned_file(env: &Env, target: &TargetPlan) -> PlannedFile {
    // The file under its placeholder root, in the row and in the diff
    // headers: the drawer never shows the user's profile path.
    let file = placeholder_path(env, &target.file, target.project.as_deref());
    PlannedFile {
        diff: target
            .diff()
            .replace(&*target.file.to_string_lossy(), &file),
        file,
        clients: target.clients.iter().map(|c| (*c).to_owned()).collect(),
        scope: target.scope.into(),
        project_id: target.project_id.clone(),
        counts: target.counts().into(),
        entries: target
            .entries
            .iter()
            .filter(|entry| needs_attention(entry))
            .map(|entry| PlannedEntry {
                note: entry.into(),
                action: entry.action.into(),
            })
            .collect(),
        problem: target.problem.as_ref().map(IpcError::from),
        warnings: errors(&target.warnings),
    }
}

/// The drawer: every file the next apply would touch, with masked diffs.
pub fn pending(core: &Core) -> CmdResult<PendingPlan> {
    let store = core.open_store()?;
    let ax = core.apply_env();
    let plan = apply::preview(store.as_ref(), &ax, &PlanOptions::default())?;
    let apply_mode = match &store {
        Some(store) => {
            let machine = Machine::load(core.fs.as_ref(), &core.home.state_dir())?;
            DesiredState::load(store, machine.as_ref().map(|m| m.id.as_str()))?.apply_mode()
        }
        None => ApplyMode::Instant,
    };
    let pending = plan
        .targets
        .iter()
        .flat_map(|target| &target.entries)
        .filter(|entry| {
            entry.state.is_pending() || entry.unresolved || entry.state == EntryState::Blocked
        })
        .count();
    let files = plan
        .targets
        .iter()
        .filter(|target| {
            target.has_work()
                || target.problem.is_some()
                || target.entries.iter().any(needs_attention)
        })
        .map(|target| planned_file(&core.env, target))
        .collect();
    Ok(PendingPlan {
        apply_mode: match apply_mode {
            ApplyMode::Instant => ApplyModeKind::Instant,
            ApplyMode::Review => ApplyModeKind::Review,
        },
        files,
        pending: count(pending),
        writes: count(plan.writes()),
        unresolved: count(plan.unresolved()),
        warnings: errors(&plan.warnings),
    })
}

/// `loadout apply`, for every client or only `clients` (Apply selected).
pub fn run(core: &Core, clients: Option<Vec<String>>) -> CmdResult<ChangeResult> {
    let store = core.open_store_for_change()?;
    let request = ApplyRequest {
        options: PlanOptions {
            clients: clients.filter(|clients| !clients.is_empty()),
            scope: ScopeFilter::All,
            ..PlanOptions::default()
        },
        trigger: ApplyTrigger::Review,
    };
    let (report, commit) = apply::run(&store, &core.apply_env(), &request)?;
    Ok(ChangeResult::new(&core.env, commit.as_ref(), Some(&report)))
}

/// This machine's recorded apply runs, newest first ("Undo this apply").
pub fn applied_runs(core: &Core) -> CmdResult<Vec<AppliedRun>> {
    let Some(store) = core.open_store()? else {
        return Ok(Vec::new());
    };
    let Some(machine) = Machine::load(core.fs.as_ref(), &core.home.state_dir())? else {
        return Ok(Vec::new());
    };
    let runs = rollback::apply_commits(&store, &machine.id)?;
    Ok(runs
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let apply = entry.trailers.apply.as_ref()?;
            Some(AppliedRun {
                apply_id: apply.id.clone(),
                commit: entry.id.to_string(),
                time: entry.time_rfc3339(),
                summary: entry.summary.clone(),
                latest: index == 0,
            })
        })
        .collect())
}

/// `loadout rollback <apply-id>`: restore the desired state from before
/// the run and apply it. An older run over later changes needs
/// `confirmed` (`confirmation_required` otherwise).
pub fn undo(core: &Core, apply_id: &str, confirmed: bool) -> CmdResult<UndoResult> {
    let store = core.open_store()?.ok_or_else(|| IpcError {
        code: agent_loadout_core::ErrorCode::NotFound,
        message: "there is no store yet, so no apply run to undo".to_owned(),
    })?;
    let report = rollback::rollback(
        &store,
        &core.apply_env(),
        &ApplySelector::parse(apply_id),
        confirmed,
    )?;
    Ok(UndoResult {
        apply_id: report.plan.apply_id.clone(),
        restored: report.plan.files.clone(),
        tag: report.tag.clone(),
        commit: report.commit.as_ref().map(ToString::to_string),
        apply: ApplyRun::new(&core.env, &report.apply),
    })
}

#[tauri::command]
#[specta::specta]
pub async fn pending_plan(state: tauri::State<'_, AppState>) -> CmdResult<PendingPlan> {
    blocking(&state, pending).await
}

#[tauri::command]
#[specta::specta]
pub async fn run_apply(
    state: tauri::State<'_, AppState>,
    clients: Option<Vec<String>>,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| run(core, clients)).await
}

#[tauri::command]
#[specta::specta]
pub async fn list_applied_runs(state: tauri::State<'_, AppState>) -> CmdResult<Vec<AppliedRun>> {
    blocking(&state, applied_runs).await
}

#[tauri::command]
#[specta::specta]
pub async fn undo_apply(
    state: tauri::State<'_, AppState>,
    apply_id: String,
    confirmed: bool,
) -> CmdResult<UndoResult> {
    blocking(&state, move |core| undo(core, &apply_id, confirmed)).await
}

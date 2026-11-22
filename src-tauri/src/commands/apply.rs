//! Apply drawer commands (SPEC 13.4) and "Undo this apply" (SPEC 8.7):
//! the pending plan with masked diffs, `loadout apply` (every file or only
//! the selected ones) and `loadout rollback`.

use std::ops::Range;

use agent_loadout_core::apply::render::DesiredState;
use agent_loadout_core::apply::rollback::{self, ApplySelector};
use agent_loadout_core::apply::{
    self, Action, ApplyRequest, EntryState, PlanOptions, ScopeFilter, TargetFilter, TargetPlan,
    placeholder_path,
};
use agent_loadout_core::model::applied::ApplyTrigger;
use agent_loadout_core::model::settings::ApplyMode;
use agent_loadout_core::secrets::classify::{classify_import, find_in_text, is_sensitive_key};
use agent_loadout_core::store::Machine;
use agent_loadout_core::{CoreError, Env, ErrorCode};
use serde::{Deserialize, Serialize};
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
    /// The file under its placeholder root (`~/...`, `<P>/...`).
    pub file: String,
    pub clients: Vec<String>,
    pub scope: FileScope,
    pub project_id: Option<String>,
    /// The project folder under its placeholder root, for a project or
    /// project-local file.
    pub project: Option<String>,
    pub counts: OpCounts,
    /// Unified diff of the file text, with values written `inline` and
    /// anything else that may be a secret masked; empty when the file
    /// stays as it is.
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

/// One drawer file, as "Apply selected" names it: the same project, scope
/// and placeholder path as its [`PlannedFile`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FileRef {
    pub project_id: Option<String>,
    pub scope: FileScope,
    pub file: String,
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

const MASK: &str = "********";

/// Masks what may be a secret in each line of a diff, context lines and
/// entries Loadout does not own included: token formats and URL
/// credentials ([`find_in_text`]), literal values under sensitive keys
/// (`"API_TOKEN": "..."`, `password = "..."`, `token: ...`) and
/// `--token=VALUE` arguments. Client reference syntaxes (`${NAME}`,
/// `$NAME`, `{env:NAME}`, `%NAME%`) stay readable.
pub(super) fn mask_diff(diff: &str) -> String {
    diff.split_inclusive('\n').map(mask_line).collect()
}

fn mask_line(line: &str) -> String {
    let mut ranges: Vec<Range<usize>> = find_in_text(line)
        .into_iter()
        .map(|hit| hit.range)
        .collect();
    sensitive_values(line, &mut ranges);
    if ranges.is_empty() {
        return line.to_owned();
    }
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    let mut masked = String::with_capacity(line.len());
    let mut at = 0;
    for range in merged {
        masked.push_str(&line[at..range.start]);
        masked.push_str(MASK);
        at = range.end;
    }
    masked.push_str(&line[at..]);
    masked
}

fn is_bare_key(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')
}

/// The inside of the string quoted at `start`, and the index after it.
fn quoted(bytes: &[u8], start: usize) -> Option<(Range<usize>, usize)> {
    let quote = bytes[start];
    let mut k = start + 1;
    while k < bytes.len() {
        match bytes[k] {
            b'\\' if quote == b'"' => k += 2,
            b if b == quote => return Some((start + 1..k, k + 1)),
            _ => k += 1,
        }
    }
    None
}

fn skip_blanks(bytes: &[u8], mut k: usize) -> usize {
    while k < bytes.len() && matches!(bytes[k], b' ' | b'\t') {
        k += 1;
    }
    k
}

/// The scalar value starting at `k` (quoted or bare), whether it was
/// quoted, and the index after it; `None` for an object, a list or
/// nothing.
fn value_at(bytes: &[u8], k: usize) -> Option<(Range<usize>, bool, usize)> {
    match bytes.get(k)? {
        b'"' | b'\'' => quoted(bytes, k).map(|(inner, end)| (inner, true, end)),
        b'{' | b'[' | b'\r' | b'\n' | b',' => None,
        _ => {
            let end = bytes[k..]
                .iter()
                .position(|b| matches!(b, b',' | b'}' | b']' | b'#' | b'\r' | b'\n'))
                .map_or(bytes.len(), |n| k + n);
            let mut trimmed = end;
            while trimmed > k && matches!(bytes[trimmed - 1], b' ' | b'\t') {
                trimmed -= 1;
            }
            (trimmed > k).then_some((k..trimmed, false, end))
        }
    }
}

/// A literal that may be a secret under `key`: not only references, and
/// for a bare value not a boolean, null or number.
fn secret_value(key: &str, value: &str, quoted: bool) -> bool {
    let plain = !quoted
        && (matches!(value, "true" | "false" | "null" | "~") || value.parse::<f64>().is_ok());
    !plain
        && is_sensitive_key(key)
        && classify_import(Some(key), value).is_some()
        && !only_references(value)
}

/// Value ranges in `line` that may be secrets: `key: value`, `key = value`
/// under a sensitive key, and `"--token=VALUE"` arguments.
fn sensitive_values(line: &str, out: &mut Vec<Range<usize>>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let (key, after) = match bytes[i] {
            b'"' | b'\'' => match quoted(bytes, i) {
                Some((inner, end)) => {
                    let text = &line[inner.clone()];
                    if let Some((name, value)) = text.split_once('=')
                        && name.starts_with('-')
                        && secret_value(name, value, true)
                    {
                        out.push(inner.start + name.len() + 1..inner.end);
                    }
                    (inner, end)
                }
                None => {
                    i += 1;
                    continue;
                }
            },
            b if is_bare_key(b) => {
                let end = bytes[i..]
                    .iter()
                    .position(|b| !is_bare_key(*b))
                    .map_or(bytes.len(), |n| i + n);
                (i..end, end)
            }
            _ => {
                i += 1;
                continue;
            }
        };
        let k = skip_blanks(bytes, after);
        if !matches!(bytes.get(k), Some(b':' | b'=')) {
            i = after;
            continue;
        }
        let k = skip_blanks(bytes, k + 1);
        match value_at(bytes, k) {
            Some((value, quoted, end)) => {
                if secret_value(&line[key], &line[value.clone()], quoted) {
                    out.push(value);
                }
                i = end;
            }
            // An object or a list: its keys are scanned in turn.
            None => i = k.max(after),
        }
    }
}

/// Whether `value` holds client references and nothing else but an auth
/// scheme (`Bearer ${TOKEN}`).
fn only_references(value: &str) -> bool {
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let bytes = value.as_bytes();
    let mut rest = String::new();
    let mut found = false;
    let mut k = 0;
    while k < bytes.len() {
        let closing = |from: usize| bytes[from..].iter().position(|b| *b == b'}');
        if value[k..].starts_with("${") || value[k..].starts_with("{env:") {
            if let Some(n) = closing(k) {
                found = true;
                k += n + 1;
                continue;
            }
        } else if bytes[k] == b'$' && bytes.get(k + 1).is_some_and(|b| ident(*b)) {
            found = true;
            k += 1;
            while k < bytes.len() && ident(bytes[k]) {
                k += 1;
            }
            continue;
        } else if bytes[k] == b'%'
            && let Some(n) = bytes[k + 1..].iter().position(|b| *b == b'%')
            && n > 0
            && bytes[k + 1..k + 1 + n].iter().all(|b| ident(*b))
        {
            found = true;
            k += n + 2;
            continue;
        }
        let width = value[k..].chars().next().map_or(1, char::len_utf8);
        rest.push_str(&value[k..k + width]);
        k += width;
    }
    let rest = rest.trim().to_ascii_lowercase();
    found && matches!(rest.as_str(), "" | "bearer" | "basic" | "token")
}

/// The drawer's key of a target: project, scope and placeholder path.
pub(super) fn file_ref(env: &Env, target: &TargetPlan) -> FileRef {
    FileRef {
        project_id: target.project_id.clone(),
        scope: target.scope.into(),
        file: placeholder_path(env, &target.file, target.project.as_deref()),
    }
}

fn planned_file(env: &Env, target: &TargetPlan) -> PlannedFile {
    // The file under its placeholder root, in the row and in the diff
    // headers: the drawer never shows the user's profile path.
    let FileRef {
        project_id,
        scope,
        file,
    } = file_ref(env, target);
    PlannedFile {
        diff: mask_diff(
            &target
                .diff()
                .replace(&*target.file.to_string_lossy(), &file),
        ),
        file,
        clients: target.clients.iter().map(|c| (*c).to_owned()).collect(),
        scope,
        project_id,
        project: target
            .project
            .as_deref()
            .map(|root| placeholder_path(env, root, None)),
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

/// `loadout apply`, for every file or only `files` (Apply selected): each
/// is matched to its target in the current plan, and only those targets
/// are planned and written.
pub fn run(core: &Core, files: Option<Vec<FileRef>>) -> CmdResult<ChangeResult> {
    let store = core.open_store_for_change()?;
    let ax = core.apply_env();
    let targets = match files {
        None => None,
        Some(files) => {
            let plan = apply::preview(Some(&store), &ax, &PlanOptions::default())?;
            let targets: Vec<TargetFilter> = plan
                .targets
                .iter()
                .filter(|target| files.contains(&file_ref(&core.env, target)))
                .map(|target| TargetFilter {
                    scope: target.scope,
                    project: target.project.clone(),
                    file: target.file.clone(),
                })
                .collect();
            if targets.is_empty() {
                return Err(CoreError::new(
                    ErrorCode::NotFound,
                    "none of the selected files is in the pending plan any more",
                )
                .into());
            }
            Some(targets)
        }
    };
    let request = ApplyRequest {
        options: PlanOptions {
            scope: ScopeFilter::All,
            targets,
            ..PlanOptions::default()
        },
        trigger: ApplyTrigger::Review,
    };
    let (report, commit) = apply::run(&store, &ax, &request)?;
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
    files: Option<Vec<FileRef>>,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| run(core, files)).await
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

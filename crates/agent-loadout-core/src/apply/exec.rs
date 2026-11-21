//! Execution (SPEC 8.4) and apply commits (SPEC 8.7).
//!
//! Per target file, holding the store lock: re-read and re-plan if the file
//! changed since planning (up to three times), back up the original bytes,
//! self-check the edited text, write a temp file, compare-and-swap against
//! the planned bytes, replace, then update the ledger and the journal. A
//! failed file never stops the others.
//!
//! [`apply_in`] runs inside a [`Transaction`]: the run is recorded in
//! `machines/<machine-id>.applied.toml` and the `Loadout-Apply` trailer of
//! that transaction's single commit. [`run`] wraps it in its own
//! transaction, which commits only when a file was written.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crate::apply::backup;
use crate::apply::journal::{FileRecord, Journal, RecordedError};
use crate::apply::ledger::{EntryKind, Ledger, LedgerEntry, LedgerKey, Origin};
use crate::apply::plan::{
    self, Action, EntryPlan, EntryState, OpCounts, Plan, PlanOptions, TargetPlan, read_target,
};
use crate::apply::render::{DesiredState, RenderCx};
use crate::apply::{ApplyEnv, import, placeholder_path, temp_path};
use crate::error::{CoreError, ErrorCode};
use crate::formats::{Document, NativeMap, NativeValue};
use crate::model::applied::{AppliedRecord, AppliedTarget, ApplyOutcome, ApplyTrigger};
use crate::model::ids;
use crate::model::{SCHEMA_VERSION, TomlDocument};
use crate::store::{ApplyRef, CommitId, Machine, Operation, Store, Transaction, utc_datetime};

/// Times a file is re-planned after it changed underneath (SPEC 8.4).
const MAX_REPLANS: u32 = 3;
/// How long a replace is retried while another process holds the file.
const LOCK_RETRY: Duration = Duration::from_secs(2);

/// One apply run's inputs.
#[derive(Debug, Clone)]
pub struct ApplyRequest {
    pub options: PlanOptions,
    pub trigger: ApplyTrigger,
}

/// What one target got.
#[derive(Debug, Clone)]
pub struct TargetResult {
    pub clients: Vec<&'static str>,
    pub scope: crate::model::applied::Scope,
    pub file: PathBuf,
    /// Operations done (zero when the file was not written).
    pub counts: OpCounts,
    pub outcome: ApplyOutcome,
    /// Why the file failed or is blocked as a whole.
    pub error: Option<CoreError>,
    /// The plan the file was executed with (after any re-plan).
    pub entries: Vec<EntryPlan>,
    pub written: bool,
    pub backup: Option<PathBuf>,
    pub warnings: Vec<CoreError>,
}

impl TargetResult {
    pub fn unresolved(&self) -> usize {
        self.entries.iter().filter(|entry| entry.unresolved).count()
    }
}

/// The result of an apply run.
#[derive(Debug, Clone)]
pub struct ApplyReport {
    pub apply_id: String,
    pub trigger: ApplyTrigger,
    pub outcome: ApplyOutcome,
    pub targets: Vec<TargetResult>,
    pub warnings: Vec<CoreError>,
    /// Store servers changed by `import` resolutions.
    pub imported: Vec<String>,
    /// Whether the run was recorded in the store (`applied.toml` and the
    /// `Loadout-Apply` trailer).
    pub recorded: bool,
}

impl ApplyReport {
    /// Files written.
    pub fn writes(&self) -> usize {
        self.targets.iter().filter(|target| target.written).count()
    }

    /// Entries that need a decision that was not given.
    pub fn unresolved(&self) -> usize {
        self.targets.iter().map(TargetResult::unresolved).sum()
    }
}

/// Plans against the store's working tree (or an empty desired state when
/// there is no store yet). Read-only: creates no ledger key, lock or file.
pub fn preview(
    store: Option<&Store<'_>>,
    ax: &ApplyEnv<'_>,
    options: &PlanOptions,
) -> Result<Plan, CoreError> {
    let state_dir = ax.home.state_dir();
    let machine = Machine::load(ax.fs, &state_dir)?;
    let desired = match store {
        Some(store) => DesiredState::load(store, machine.as_ref().map(|m| m.id.as_str()))?,
        None => DesiredState::empty(),
    };
    let ledger = Ledger::load(ax.fs, &state_dir)?;
    // Without a key file nothing was ever hashed: any key plans the same.
    let key = match LedgerKey::load(ax.fs, &state_dir)? {
        Some(key) => key,
        None => LedgerKey::from_bytes([0; 32]),
    };
    let memory = match store {
        Some(store) => store.secret_memory().clone(),
        None => std::sync::Arc::new(crate::secrets::scanner::SecretMemory::new()?),
    };
    plan::plan(&ax.render_cx(memory), &desired, &ledger, &key, options)
}

/// A standalone apply run in its own transaction: one commit when a file
/// was written, none otherwise (SPEC 8.7).
pub fn run(
    store: &Store<'_>,
    ax: &ApplyEnv<'_>,
    request: &ApplyRequest,
) -> Result<(ApplyReport, Option<CommitId>), CoreError> {
    let operation = Operation::new("apply", "apply");
    let committed = store.transaction(operation, |tx| apply_in(tx, ax, request))?;
    Ok((committed.value, committed.commit))
}

/// Plans against the transaction's view of the store (staged edits
/// included), executes, and records the run in the transaction: always
/// when the transaction commits anyway (an instant-mode edit, a rollback),
/// otherwise only when a file was written.
pub fn apply_in(
    tx: &mut Transaction<'_>,
    ax: &ApplyEnv<'_>,
    request: &ApplyRequest,
) -> Result<ApplyReport, CoreError> {
    let state_dir = ax.home.state_dir();
    let machine = tx.machine().clone();
    let cx = ax.render_cx(tx.store().secret_memory().clone());
    let mut ledger = Ledger::load(ax.fs, &state_dir)?;
    let key = LedgerKey::load_or_create(ax.fs, &state_dir)?;

    let mut desired = DesiredState::load(tx, Some(&machine.id))?;
    let mut plan = plan::plan(&cx, &desired, &ledger, &key, &request.options)?;
    let mut imported = Vec::new();
    if plan
        .targets
        .iter()
        .flat_map(|target| &target.entries)
        .any(|entry| entry.action == Action::Import)
    {
        let resolutions = import::stage_resolutions(tx, &plan, &desired)?;
        imported = resolutions.changed;
        desired = DesiredState::load(tx, Some(&machine.id))?;
        plan = plan::plan(&cx, &desired, &ledger, &key, &request.options)?;
        // Whatever still asks for an import could not be imported.
        for entry in plan
            .targets
            .iter_mut()
            .flat_map(|target| &mut target.entries)
        {
            if entry.action != Action::Import {
                continue;
            }
            let reason = resolutions
                .failed
                .iter()
                .find(|(client, name, _)| {
                    Some(client.as_str()) == entry.client && *name == entry.name
                })
                .map(|(_, _, err)| err.clone());
            entry.action = Action::Skip;
            entry.unresolved = true;
            entry.problem = Some(reason.unwrap_or_else(|| {
                CoreError::new(
                    ErrorCode::InvalidValue,
                    "the store definition still renders differently after the import",
                )
            }));
        }
    }

    let base = tx.store().head()?;
    let now = ax.env.now();
    let apply_id = ids::new_ulid(now)?;
    let mut journal = Journal::start(
        ax.fs,
        &state_dir,
        &apply_id,
        request.trigger,
        utc_datetime(now).to_string(),
    )?;
    let mut executor = Executor {
        cx: &cx,
        ax,
        key: &key,
        options: &request.options,
        run_dir: backup::run_dir(ax.home, now, &apply_id),
        written_at: utc_datetime(now).to_string(),
        store_commit: base.short().to_owned(),
    };
    let mut targets = Vec::with_capacity(plan.targets.len());
    for target in plan.targets {
        let result = executor.execute(target, &mut ledger);
        journal.record(ax.fs, &state_dir, file_record(&result))?;
        targets.push(result);
    }
    let outcome = run_outcome(&targets);
    let writes = targets.iter().filter(|target| target.written).count();

    let commits_anyway = tx.has_changes();
    let recorded = commits_anyway || writes > 0;
    if recorded {
        let record = AppliedRecord {
            schema: SCHEMA_VERSION,
            machine_id: machine.id.clone(),
            apply_id: apply_id.clone(),
            time: utc_datetime(now),
            store_revision: base.as_str().to_owned(),
            trigger: request.trigger,
            outcome,
            targets: targets
                .iter()
                .map(|target| AppliedTarget {
                    client: target.clients[0].to_owned(),
                    scope: target.scope,
                    project_id: None,
                    file: placeholder_path(ax.env, &target.file),
                    create: target.counts.create,
                    update: target.counts.update,
                    remove: target.counts.remove,
                    disable: target.counts.disable,
                    outcome: target.outcome,
                    extra: toml::Table::new(),
                })
                .collect(),
            extra: toml::Table::new(),
        };
        let relative = format!("machines/{}.applied.toml", machine.id);
        tx.write(&relative, TomlDocument::new(record).to_text()?)?;
        tx.set_apply(ApplyRef {
            id: apply_id.clone(),
            outcome,
        });
        if !commits_anyway {
            tx.set_summary(summary(&targets));
        }
    }
    journal.finish(ax.fs, &state_dir, outcome)?;
    backup::prune(ax.fs, ax.home, now)?;

    Ok(ApplyReport {
        apply_id,
        trigger: request.trigger,
        outcome,
        targets,
        warnings: plan.warnings,
        imported,
        recorded,
    })
}

/// `apply: 2 files (claude-code, codex)`.
fn summary(targets: &[TargetResult]) -> String {
    let written: Vec<&TargetResult> = targets.iter().filter(|target| target.written).collect();
    let mut clients: Vec<&str> = Vec::new();
    for target in &written {
        for client in &target.clients {
            if !clients.contains(client) {
                clients.push(client);
            }
        }
    }
    let files = if written.len() == 1 { "file" } else { "files" };
    format!("apply: {} {files} ({})", written.len(), clients.join(", "))
}

/// The outcome of a run from its targets: `ok` when every target is ok,
/// `failed` or `blocked` when none got anywhere, `partial` otherwise.
fn run_outcome(targets: &[TargetResult]) -> ApplyOutcome {
    let count = |outcome| targets.iter().filter(|t| t.outcome == outcome).count();
    let (ok, partial, failed) = (
        count(ApplyOutcome::Ok),
        count(ApplyOutcome::Partial),
        count(ApplyOutcome::Failed),
    );
    if ok == targets.len() {
        ApplyOutcome::Ok
    } else if ok + partial == 0 && failed > 0 {
        ApplyOutcome::Failed
    } else if ok + partial == 0 {
        ApplyOutcome::Blocked
    } else {
        ApplyOutcome::Partial
    }
}

fn file_record(result: &TargetResult) -> FileRecord {
    FileRecord {
        clients: result.clients.iter().map(|c| (*c).to_owned()).collect(),
        scope: result.scope,
        file: result.file.to_string_lossy().into_owned(),
        outcome: result.outcome,
        create: result.counts.create,
        update: result.counts.update,
        remove: result.counts.remove,
        disable: result.counts.disable,
        written: result.written,
        backup: result
            .backup
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        error: result.error.as_ref().map(RecordedError::from),
    }
}

struct Executor<'a> {
    cx: &'a RenderCx<'a>,
    ax: &'a ApplyEnv<'a>,
    key: &'a LedgerKey,
    options: &'a PlanOptions,
    run_dir: PathBuf,
    written_at: String,
    store_commit: String,
}

impl Executor<'_> {
    fn execute(&mut self, plan: TargetPlan, ledger: &mut Ledger) -> TargetResult {
        let mut plan = plan;
        let mut replans = 0;
        loop {
            let current = match read_target(self.ax.fs, &plan.file) {
                Ok(bytes) => bytes,
                Err(err) => return failed(plan, err),
            };
            if current != plan.before {
                replans += 1;
                if replans > MAX_REPLANS {
                    let err = changed_during_apply(&plan.file);
                    return failed(plan, err);
                }
                plan = self.replan(plan, current, ledger);
                continue;
            }
            if let Some(problem) = plan.problem.clone() {
                return finished(plan, ApplyOutcome::Blocked, Some(problem), false, None);
            }
            let Some(after) = plan.after.clone() else {
                // Ledger-only work (adopt, refresh, forget, release).
                if let Err(err) = self.update_ledger(&plan, ledger) {
                    return failed(plan, err);
                }
                let outcome = entry_outcome(&plan, false);
                return finished(plan, outcome, None, false, None);
            };
            if let Err(err) = self_check(&plan, &after) {
                return failed(plan, err);
            }
            let backup = match &current {
                Some(bytes) => {
                    match backup::save(self.ax.fs, self.ax.home, &self.run_dir, &plan.file, bytes) {
                        Ok(path) => Some(path),
                        Err(err) => return failed(plan, err),
                    }
                }
                None => None,
            };
            match self.write(&plan, &after) {
                Ok(true) => {}
                Ok(false) => {
                    // Compare-and-swap failed: the file changed after the
                    // re-read above. Re-plan against what is there now.
                    replans += 1;
                    if replans > MAX_REPLANS {
                        let err = changed_during_apply(&plan.file);
                        return failed(plan, err);
                    }
                    let now = match read_target(self.ax.fs, &plan.file) {
                        Ok(bytes) => bytes,
                        Err(err) => return failed(plan, err),
                    };
                    plan = self.replan(plan, now, ledger);
                    continue;
                }
                Err(err) => return failed(plan, err),
            }
            if let Err(err) = self.update_ledger(&plan, ledger) {
                let mut result = failed(plan, err);
                result.written = true;
                result.backup = backup;
                return result;
            }
            let outcome = entry_outcome(&plan, true);
            return finished(plan, outcome, None, true, backup);
        }
    }

    fn replan(&self, plan: TargetPlan, bytes: Option<Vec<u8>>, ledger: &Ledger) -> TargetPlan {
        plan::plan_target(self.cx, plan.input, bytes, ledger, self.key, self.options)
    }

    /// Temp file, compare-and-swap, replace. `Ok(false)` when the file no
    /// longer holds the planned bytes; the temp file is removed then.
    fn write(&self, plan: &TargetPlan, after: &[u8]) -> Result<bool, CoreError> {
        swap_file(self.ax.fs, &plan.file, plan.before.as_deref(), after)
    }
}

/// SPEC 8.4 step 4: writes `after` to a temp file next to `file` (with the
/// file's permissions when it exists), checks that `file` still holds
/// `expected` (compare-and-swap), then replaces it, retrying a Windows
/// sharing violation for [`LOCK_RETRY`] before `file_locked`. `Ok(false)`
/// when the file changed; the temp file is removed then.
pub(crate) fn swap_file(
    fs: &dyn crate::fs::Fs,
    file: &Path,
    expected: Option<&[u8]>,
    after: &[u8],
) -> Result<bool, CoreError> {
    let io = |what: &str, at: &Path, err: std::io::Error| {
        CoreError::new(ErrorCode::IoError, format!("{what}: {err}")).with_path(at)
    };
    if let Some(dir) = file.parent() {
        fs.create_dir_all(dir)
            .map_err(|err| io("cannot create directory", dir, err))?;
    }
    let temp = temp_path(file)?;
    let like = expected.is_some().then_some(file);
    fs.write_new(&temp, after, like)
        .map_err(|err| io("cannot write temp file", &temp, err))?;
    let discard = || {
        let _ = fs.remove_file(&temp);
    };
    match read_target(fs, file) {
        Ok(now) if now.as_deref() == expected => {}
        Ok(_) => {
            discard();
            return Ok(false);
        }
        Err(err) => {
            discard();
            return Err(err);
        }
    }
    let deadline = Instant::now() + LOCK_RETRY;
    let mut pause = Duration::from_millis(25);
    loop {
        match fs.replace(&temp, file) {
            Ok(()) => return Ok(true),
            Err(err) if is_sharing_violation(&err) && Instant::now() < deadline => {
                thread::sleep(pause.min(deadline.saturating_duration_since(Instant::now())));
                pause = (pause * 2).min(Duration::from_millis(200));
            }
            Err(err) if is_sharing_violation(&err) => {
                discard();
                return Err(CoreError::new(
                    ErrorCode::FileLocked,
                    format!(
                        "another process kept the file open for {} s: {err}",
                        LOCK_RETRY.as_secs()
                    ),
                )
                .with_path(file));
            }
            Err(err) => {
                discard();
                return Err(io("cannot replace file", file, err));
            }
        }
    }
}

impl Executor<'_> {
    /// Records every entry's new ownership state and saves the ledger.
    fn update_ledger(&self, plan: &TargetPlan, ledger: &mut Ledger) -> Result<(), CoreError> {
        let file = plan.file.to_string_lossy().into_owned();
        let mut changed = false;
        for entry in &plan.entries {
            let origin = match entry.action {
                Action::Create => Some(Origin::Created),
                Action::Adopt | Action::AdoptOverwrite => Some(Origin::Adopted),
                Action::Update | Action::Disable | Action::Refresh => Some(
                    ledger
                        .get(&file, &plan.pointer, &entry.name)
                        .and_then(|known| known.origin)
                        .unwrap_or(Origin::Created),
                ),
                Action::Remove | Action::Forget | Action::Release => {
                    changed |= ledger.remove(&file, &plan.pointer, &entry.name);
                    None
                }
                Action::None | Action::Import | Action::Skip => None,
            };
            let Some(origin) = origin else {
                continue;
            };
            let (Some(client), Some(value)) = (entry.client, &entry.written) else {
                continue;
            };
            let Some(canonical) = plan::canonical(client, value) else {
                continue;
            };
            ledger.upsert(LedgerEntry {
                kind: EntryKind::McpEntry,
                client: client.to_owned(),
                scope: plan.scope,
                project_id: None,
                file: file.clone(),
                container: plan.pointer.clone(),
                key: entry.name.clone(),
                server_id: entry.server_id.clone(),
                origin: Some(origin),
                written_hash: Some(self.key.hash(&canonical)),
                written_at: Some(self.written_at.clone()),
                store_commit: Some(self.store_commit.clone()),
                delivery: entry.delivery.map(str::to_owned),
                extra: serde_json::Map::new(),
            });
            changed = true;
        }
        if changed {
            ledger.save(self.ax.fs, &self.ax.home.state_dir())?;
        }
        Ok(())
    }
}

fn changed_during_apply(file: &Path) -> CoreError {
    CoreError::new(
        ErrorCode::FileChangedDuringApply,
        format!(
            "the file kept changing during apply ({MAX_REPLANS} re-plans); it was left as it is"
        ),
    )
    .with_path(file)
}

/// The outcome of a target that was executed: blocked or unresolved
/// entries make it `partial` when something was done, `blocked` otherwise.
fn entry_outcome(plan: &TargetPlan, wrote: bool) -> ApplyOutcome {
    let stuck = plan
        .entries
        .iter()
        .any(|entry| entry.state == EntryState::Blocked || entry.unresolved);
    let did = wrote
        || plan.entries.iter().any(|entry| {
            matches!(
                entry.action,
                Action::Adopt | Action::Refresh | Action::Forget | Action::Release
            )
        });
    match (stuck, did) {
        (false, _) => ApplyOutcome::Ok,
        (true, true) => ApplyOutcome::Partial,
        (true, false) => ApplyOutcome::Blocked,
    }
}

fn finished(
    plan: TargetPlan,
    outcome: ApplyOutcome,
    error: Option<CoreError>,
    written: bool,
    backup: Option<PathBuf>,
) -> TargetResult {
    TargetResult {
        counts: if written {
            plan.counts()
        } else {
            OpCounts::default()
        },
        clients: plan.clients,
        scope: plan.scope,
        file: plan.file,
        outcome,
        error,
        entries: plan.entries,
        written,
        backup,
        warnings: plan.warnings,
    }
}

fn failed(plan: TargetPlan, err: CoreError) -> TargetResult {
    finished(plan, ApplyOutcome::Failed, Some(err), false, None)
}

#[cfg(windows)]
fn is_sharing_violation(err: &std::io::Error) -> bool {
    // ERROR_ACCESS_DENIED (a handle without FILE_SHARE_DELETE),
    // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
    matches!(err.raw_os_error(), Some(5 | 32 | 33))
}

#[cfg(not(windows))]
fn is_sharing_violation(_err: &std::io::Error) -> bool {
    false
}

/// SPEC 8.4 step 3: the edited text parses, owned entries read back as
/// planned, and everything else (other entries of the container and
/// everything outside it) is unchanged.
fn self_check(plan: &TargetPlan, after: &[u8]) -> Result<(), CoreError> {
    let fail = |detail: String| {
        Err(CoreError::new(ErrorCode::SelfCheckFailed, detail).with_path(&plan.file))
    };
    let format = plan.input.target.format;
    let after_doc = match Document::parse(after, format) {
        Ok(doc) => doc,
        Err(err) => return fail(format!("the edited file does not parse: {err}")),
    };
    let before_doc = match &plan.before {
        Some(bytes) => Document::parse(bytes, format)?,
        None => plan.input.target.new_document(),
    };
    let container: Vec<&str> = plan.container.iter().map(String::as_str).collect();
    let mut planned: Vec<&str> = Vec::new();
    for entry in &plan.entries {
        planned.push(&entry.name);
        let got = after_doc.get(&container, &entry.name)?;
        let ok = match entry.action {
            Action::Create | Action::Update | Action::AdoptOverwrite | Action::Disable => {
                match (entry.client, &entry.written, &got) {
                    (Some(client), Some(want), Some(got)) => {
                        plan::canonical(client, got) == plan::canonical(client, want)
                    }
                    _ => false,
                }
            }
            Action::Remove => got.is_none(),
            _ => got == entry.observed,
        };
        if !ok {
            return fail(format!(
                "entry {:?} does not read back as planned",
                entry.name
            ));
        }
    }
    for name in before_doc.list(&container)? {
        if !planned.contains(&name.as_str())
            && before_doc.get(&container, &name)? != after_doc.get(&container, &name)?
        {
            return fail(format!("entry {name:?} is not owned but changed"));
        }
    }
    if outside(&before_doc, &container)? != outside(&after_doc, &container)? {
        return fail("something outside the server container changed".to_owned());
    }
    Ok(())
}

/// The whole document without the container at `path` (and the empty
/// objects that held only it).
fn outside(doc: &Document, path: &[&str]) -> Result<NativeValue, CoreError> {
    let mut root = NativeMap::new();
    for key in doc.list(&[])? {
        if let Some(value) = doc.get(&[], &key)? {
            root.insert(key, value);
        }
    }
    let mut root = NativeValue::Object(root);
    remove_path(&mut root, path);
    Ok(root)
}

/// Removes `path` from `value`, then each ancestor left empty.
fn remove_path(value: &mut NativeValue, path: &[&str]) -> bool {
    let NativeValue::Object(map) = value else {
        return false;
    };
    match path {
        [] => false,
        [last] => {
            map.remove(last);
            map.is_empty()
        }
        [first, rest @ ..] => {
            let Some(child) = map.get_mut(first) else {
                return false;
            };
            if remove_path(child, rest) {
                map.remove(first);
            }
            map.is_empty()
        }
    }
}

//! The optional external scanner, NVIDIA SkillSpector (#30), for
//! Settings > Security and the install and update reviews: status,
//! Prepare (the one-time uvx download, with progress events, cancelable),
//! the switch, and one run on an install review, an available update or a
//! library skill. Nothing runs unless the user turned the scanner on.
//! Details and progress lines are scrubbed of the home folder.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use agent_loadout_core::scan::external::skillspector::{self, Availability, RuntimeKind};
use agent_loadout_core::scan::external::{ExternalReport, ExternalStatus, NotScanned, Scanner};
use agent_loadout_core::skills::install::{self, Prepared};
use agent_loadout_core::skills::{SkillFiles, content_hash, store as skill_store};
use agent_loadout_core::{CoreError, Env, ErrorCode};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

use super::scan::{ScanFinding, ScanVerdict};
use super::{AppState, CmdResult, Core, IpcError, blocking, scrub, scrubbed};

/// Which external scanner made a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
pub enum ExternalScanner {
    #[serde(rename = "skillspector")]
    SkillSpector,
    #[serde(rename = "snyk-agent-scan")]
    SnykAgentScan,
}

impl From<Scanner> for ExternalScanner {
    fn from(scanner: Scanner) -> Self {
        match scanner {
            Scanner::SkillSpector => Self::SkillSpector,
            Scanner::SnykAgentScan => Self::SnykAgentScan,
        }
    }
}

/// Whether the run happened and was understood.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ExternalScanStatus {
    Scanned,
    NotScanned,
    NotUnderstood,
}

impl From<ExternalStatus> for ExternalScanStatus {
    fn from(status: ExternalStatus) -> Self {
        match status {
            ExternalStatus::Scanned => Self::Scanned,
            ExternalStatus::NotScanned => Self::NotScanned,
            ExternalStatus::NotUnderstood => Self::NotUnderstood,
        }
    }
}

/// Why the content was not scanned; never blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ExternalNotScanned {
    NotAvailable,
    NotPrepared,
    NotInstalled,
    UnsupportedPlatform,
    NoToken,
    TokenUnavailable,
    TokenRejected,
    LimitReached,
    TooLarge,
    Unreachable,
    Timeout,
    Canceled,
    WouldStartServer,
    Failed,
}

impl From<NotScanned> for ExternalNotScanned {
    fn from(reason: NotScanned) -> Self {
        match reason {
            NotScanned::NotAvailable => Self::NotAvailable,
            NotScanned::NotPrepared => Self::NotPrepared,
            NotScanned::NotInstalled => Self::NotInstalled,
            NotScanned::UnsupportedPlatform => Self::UnsupportedPlatform,
            NotScanned::NoToken => Self::NoToken,
            NotScanned::TokenUnavailable => Self::TokenUnavailable,
            NotScanned::TokenRejected => Self::TokenRejected,
            NotScanned::LimitReached => Self::LimitReached,
            NotScanned::TooLarge => Self::TooLarge,
            NotScanned::Unreachable => Self::Unreachable,
            NotScanned::Timeout => Self::Timeout,
            NotScanned::Canceled => Self::Canceled,
            NotScanned::WouldStartServer => Self::WouldStartServer,
            NotScanned::Failed => Self::Failed,
        }
    }
}

/// One external scan, shown beside the built-in scan.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExternalScanResult {
    pub scanner: ExternalScanner,
    /// The version that ran.
    pub version: String,
    pub status: ExternalScanStatus,
    pub reason: Option<ExternalNotScanned>,
    /// Meaningful only when `status` is `scanned`.
    pub verdict: ScanVerdict,
    /// The scanner did not fully scan the content: never "clean" then.
    pub incomplete: bool,
    pub findings: Vec<ScanFinding>,
    /// English, one line, no paths or values.
    pub detail: Option<String>,
}

impl ExternalScanResult {
    pub fn of(env: &Env, report: &ExternalReport) -> Self {
        Self {
            scanner: report.scanner.into(),
            version: report.version.clone(),
            status: report.status.into(),
            reason: report.reason.map(ExternalNotScanned::from),
            verdict: report.verdict.into(),
            incomplete: report.incomplete,
            findings: report.findings.iter().map(ScanFinding::from).collect(),
            detail: report
                .detail
                .as_deref()
                .map(|detail| scrub(env, detail, None)),
        }
    }
}

/// Whether SkillSpector can run on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ScannerAvailability {
    Ready,
    NeedsPrepare,
    NotAvailable,
}

/// How SkillSpector runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ScannerRuntime {
    Uvx,
    Path,
}

impl From<RuntimeKind> for ScannerRuntime {
    fn from(kind: RuntimeKind) -> Self {
        match kind {
            RuntimeKind::Uvx => Self::Uvx,
            RuntimeKind::Path => Self::Path,
        }
    }
}

/// SkillSpector on this machine.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExternalScannerStatus {
    pub scanner: ExternalScanner,
    /// The version this Loadout is tested with.
    pub tested_version: String,
    /// The uvx form's interpreter.
    pub python: String,
    pub availability: ScannerAvailability,
    /// Ready: the form that runs; needs_prepare: `uvx`; not_available: none.
    pub runtime: Option<ScannerRuntime>,
    /// Ready only: the version that will run.
    pub version: Option<String>,
    /// `version == testedVersion` (true unless ready with another version).
    pub version_tested: bool,
    pub enabled: bool,
    /// Seconds one scan may take (`[skillspector] timeout_secs`, default
    /// 120).
    pub timeout_secs: u32,
    /// A 0.7.0 Snyk Agent Scan install remains (`loadout scanner remove`
    /// deletes it).
    pub snyk_installed: bool,
}

/// One line of uv's output while `prepare_scanner` runs (control
/// characters removed, the home folder scrubbed).
#[derive(Debug, Clone, Serialize, Deserialize, Type, Event)]
pub struct ScannerPrepareProgress {
    pub line: String,
}

/// Runs in flight (each cancelable, by the caller's run id; Prepare has
/// none) and the external reports of update reviews, by skill id, until
/// `update_skill` uses them.
#[derive(Default)]
pub(crate) struct ScannerState {
    /// Runs and the Prepare (id `None`); `preparing` and
    /// `cancel_pending` change only under this lock.
    running: Mutex<Vec<(Option<String>, Arc<AtomicBool>)>>,
    preparing: AtomicBool,
    /// `cancel_external_scan(null)` arrived while no Prepare was
    /// registered (the GUI's Cancel can beat `prepare_scanner` to the
    /// backend): the next Prepare is canceled before it starts.
    cancel_pending: AtomicBool,
    /// Bumped each time the scanner is turned off: a run attaches or
    /// remembers its report only if it did not change since the run began.
    generation: Arc<AtomicU64>,
    updates: Mutex<HashMap<String, ExternalReport>>,
}

impl ScannerState {
    fn running(&self) -> std::sync::MutexGuard<'_, Vec<(Option<String>, Arc<AtomicBool>)>> {
        self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A cancel flag for one run (`run_id`: the caller's, for a per-run
    /// cancel), registered until the guard drops.
    pub(crate) fn start(&self, run_id: Option<String>) -> Running<'_> {
        let flag = Arc::new(AtomicBool::new(false));
        self.running().push((run_id, Arc::clone(&flag)));
        Running { state: self, flag }
    }

    /// Cancels the run `run_id`, or every run and Prepare with `None`
    /// (the next Prepare too, when none is registered yet).
    pub(crate) fn cancel(&self, run_id: Option<&str>) {
        let running = self.running();
        for (id, flag) in running.iter() {
            if run_id.is_none() || id.as_deref() == run_id {
                flag.store(true, Ordering::SeqCst);
            }
        }
        if run_id.is_none() && !self.preparing.load(Ordering::SeqCst) {
            self.cancel_pending.store(true, Ordering::SeqCst);
        }
    }

    /// The Prepare slot with its cancel flag; `locked` while another
    /// Prepare runs, `canceled` when a cancel arrived before it.
    pub(crate) fn begin_prepare(&self) -> CmdResult<Preparing<'_>> {
        let mut running = self.running();
        if self.preparing.load(Ordering::SeqCst) {
            return Err(IpcError {
                code: ErrorCode::Locked,
                message: "SkillSpector is already being prepared".to_owned(),
            });
        }
        if self.cancel_pending.swap(false, Ordering::SeqCst) {
            return Err(IpcError {
                code: ErrorCode::Canceled,
                message: "preparing SkillSpector was canceled".to_owned(),
            });
        }
        self.preparing.store(true, Ordering::SeqCst);
        let flag = Arc::new(AtomicBool::new(false));
        running.push((None, Arc::clone(&flag)));
        Ok(Preparing { state: self, flag })
    }

    /// The current switch-off generation.
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// The generation counter, for a worker thread.
    pub(crate) fn generation_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.generation)
    }

    pub(crate) fn remember_update(&self, id: &str, report: ExternalReport) {
        self.updates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_owned(), report);
    }

    /// Remembers `report` unless the scanner was turned off since
    /// `generation` (checked under the lock [`Self::turned_off`] clears).
    pub(crate) fn remember_update_since(
        &self,
        id: &str,
        report: ExternalReport,
        generation: u64,
    ) -> bool {
        let mut updates = self.updates.lock().unwrap_or_else(PoisonError::into_inner);
        if self.generation() != generation {
            return false;
        }
        updates.insert(id.to_owned(), report);
        true
    }

    /// The external report of `id`'s update review, taken out.
    pub(crate) fn take_update(&self, id: &str) -> Option<ExternalReport> {
        self.updates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id)
    }

    /// Puts a report back (the update failed and may be retried).
    pub(crate) fn restore_update(&self, id: &str, report: ExternalReport) {
        self.remember_update(id, report);
    }

    /// Drops every remembered update report.
    pub(crate) fn forget_updates(&self) {
        self.updates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// The scanner was turned off: the generation moves on (runs still
    /// going attach nothing), every run is canceled (a Prepare goes on)
    /// and the remembered update reports are dropped.
    pub(crate) fn turned_off(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        for (id, flag) in self.running().iter() {
            if id.is_some() {
                flag.store(true, Ordering::SeqCst);
            }
        }
        self.forget_updates();
    }
}

pub(crate) struct Running<'a> {
    state: &'a ScannerState,
    pub(crate) flag: Arc<AtomicBool>,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.state
            .running()
            .retain(|(_, flag)| !Arc::ptr_eq(flag, &self.flag));
    }
}

/// Holds the Prepare slot and its cancel flag until dropped.
pub(crate) struct Preparing<'a> {
    state: &'a ScannerState,
    pub(crate) flag: Arc<AtomicBool>,
}

impl Drop for Preparing<'_> {
    fn drop(&mut self) {
        let mut running = self.state.running();
        running.retain(|(_, flag)| !Arc::ptr_eq(flag, &self.flag));
        self.state.preparing.store(false, Ordering::SeqCst);
    }
}

fn ipc(core: &Core, err: &CoreError) -> IpcError {
    scrubbed(&core.env, err, None)
}

fn u32_of(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

pub fn status(core: &Core) -> CmdResult<ExternalScannerStatus> {
    let status = skillspector::status(&core.env, core.fs.as_ref(), &core.home)
        .map_err(|err| ipc(core, &err))?;
    let (availability, runtime, version) = match &status.availability {
        Availability::Ready(runtime) => (
            ScannerAvailability::Ready,
            Some(runtime.kind.into()),
            Some(runtime.version.clone()),
        ),
        Availability::NeedsPrepare { .. } => (
            ScannerAvailability::NeedsPrepare,
            Some(ScannerRuntime::Uvx),
            None,
        ),
        Availability::NotAvailable => (ScannerAvailability::NotAvailable, None, None),
    };
    Ok(ExternalScannerStatus {
        scanner: skillspector::SCANNER.into(),
        tested_version: skillspector::VERSION.to_owned(),
        python: skillspector::PYTHON.to_owned(),
        availability,
        runtime,
        version_tested: version
            .as_deref()
            .is_none_or(|v| v == skillspector::VERSION),
        version,
        enabled: status.enabled,
        timeout_secs: u32_of(status.timeout.as_secs()),
        snyk_installed: status.snyk_installed,
    })
}

/// Downloads the pinned SkillSpector into uv's cache; `progress` gets each
/// line uv prints (scrubbed). `canceled` once `cancel` is set. Does not
/// turn the scanner on.
pub fn prepare(
    core: &Core,
    progress: &mut dyn FnMut(&str),
    cancel: &AtomicBool,
) -> CmdResult<ExternalScannerStatus> {
    let env = &core.env;
    skillspector::prepare(
        env,
        &core.home,
        &mut |line| progress(&scrub(env, line, None)),
        Some(cancel),
    )
    .map_err(|err| ipc(core, &err))?;
    status(core)
}

pub fn set_enabled(core: &Core, enabled: bool) -> CmdResult<ExternalScannerStatus> {
    skillspector::set_enabled(core.fs.as_ref(), &core.home, enabled)
        .map_err(|err| ipc(core, &err))?;
    status(core)
}

/// `scanner_unavailable` unless SkillSpector is turned on.
fn require_enabled(core: &Core) -> CmdResult<()> {
    let settings =
        skillspector::settings(core.fs.as_ref(), &core.home).map_err(|err| ipc(core, &err))?;
    if settings.enabled {
        Ok(())
    } else {
        Err(IpcError {
            code: ErrorCode::ScannerUnavailable,
            message: "SkillSpector is off on this machine; turn it on in Settings > Security"
                .to_owned(),
        })
    }
}

/// One run, while the scanner is on: the switch is checked before
/// detection, again just before the scanner starts, and once more after
/// it ends, so a run the user turned off meanwhile
/// (`set_scanner_enabled(false)` also cancels it) leaves no report behind.
/// On but not ready: the `not_scanned` report (`not_available` /
/// `not_prepared`), nothing runs.
fn scan(core: &Core, files: &SkillFiles, cancel: &AtomicBool) -> CmdResult<ExternalReport> {
    require_enabled(core)?;
    let runner = skillspector::prepare_run(&core.env, core.fs.as_ref(), &core.home);
    require_enabled(core)?;
    let report = match runner {
        Ok(runner) => runner.scan_files(&core.env, &core.home, files, Some(cancel)),
        Err(mut report) => {
            report.content_hash = Some(content_hash(files));
            report
        }
    };
    require_enabled(core)?;
    Ok(report)
}

/// `external_scan_required` when SkillSpector is on and ready but
/// `has_report` is false: the install or update would skip the external
/// scan the user turned on (the GUI runs it first; a canceled or
/// not-scanned run still counts as a report).
pub(crate) fn require_report(core: &Core, has_report: bool) -> CmdResult<()> {
    if has_report
        || !skillspector::required(&core.env, core.fs.as_ref(), &core.home)
            .map_err(|err| ipc(core, &err))?
    {
        return Ok(());
    }
    Err(IpcError {
        code: ErrorCode::ExternalScanRequired,
        message: "SkillSpector is on but has not scanned this content yet; run the scan, \
                  then try again"
            .to_owned(),
    })
}

/// `scanner_unavailable`: the scanner was turned off while a run went on.
fn turned_off_meanwhile() -> IpcError {
    IpcError {
        code: ErrorCode::ScannerUnavailable,
        message: "SkillSpector was turned off during the scan; its report was dropped".to_owned(),
    }
}

/// Scans an install review's content and attaches the report to it. An
/// earlier report is dropped first; the new one is attached only while
/// `generation` still reads `since` (the scanner was not turned off
/// since the run began). The caller holds the review's lock throughout.
pub fn scan_review(
    core: &Core,
    prepared: &mut Prepared,
    cancel: &AtomicBool,
    generation: &AtomicU64,
    since: u64,
) -> CmdResult<ExternalScanResult> {
    prepared.set_external(None);
    let report = scan(core, prepared.files(), cancel)?;
    if generation.load(Ordering::SeqCst) != since {
        return Err(turned_off_meanwhile());
    }
    let result = ExternalScanResult::of(&core.env, &report);
    prepared.set_external(Some(report));
    // Turned off between the check and the attach: `turned_off` could not
    // take this review's lock, so drop the report here.
    if generation.load(Ordering::SeqCst) != since {
        prepared.set_external(None);
        return Err(turned_off_meanwhile());
    }
    Ok(result)
}

/// Scans the content the last update check of `id` fetched (the cache
/// clone, not fetched again); `update_skill` gates on the report.
pub fn scan_update(
    core: &Core,
    id: &str,
    cancel: &AtomicBool,
) -> CmdResult<(ExternalReport, ExternalScanResult)> {
    let store = core.open_store()?.ok_or_else(|| IpcError {
        code: ErrorCode::NotFound,
        message: "there is no skill in the store yet".to_owned(),
    })?;
    let files = install::cached_latest(&store, &super::skills::sx(core), id)
        .map_err(|err| ipc(core, &err))?
        .ok_or_else(|| IpcError {
            code: ErrorCode::EntryChanged,
            message: format!("skill {id:?} has no checked update; check for updates again"),
        })?;
    drop(store);
    let report = scan(core, &files, cancel)?;
    let result = ExternalScanResult::of(&core.env, &report);
    Ok((report, result))
}

/// Scans a library skill's store content; report only.
pub fn scan_library(core: &Core, id: &str, cancel: &AtomicBool) -> CmdResult<ExternalScanResult> {
    let store = core.open_store()?.ok_or_else(|| IpcError {
        code: ErrorCode::NotFound,
        message: "there is no skill in the store yet".to_owned(),
    })?;
    let files = skill_store::read_content(&store, id).map_err(|err| ipc(core, &err))?;
    drop(store);
    let report = scan(core, &files, cancel)?;
    Ok(ExternalScanResult::of(&core.env, &report))
}

#[tauri::command]
#[specta::specta]
pub async fn scanner_status(state: tauri::State<'_, AppState>) -> CmdResult<ExternalScannerStatus> {
    blocking(&state, status).await
}

/// Downloads the pinned SkillSpector once (uv on `PATH`), emitting
/// `ScannerPrepareProgress`; `cancel_external_scan(null)` stops it
/// (`canceled`). Does not turn the scanner on. A second Prepare while one
/// runs is `locked`.
#[tauri::command]
#[specta::specta]
pub async fn prepare_scanner(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> CmdResult<ExternalScannerStatus> {
    let preparing = state.scanner.begin_prepare()?;
    let flag = Arc::clone(&preparing.flag);
    let result = blocking(&state, move |core| {
        prepare(
            core,
            &mut |line| {
                let _ = ScannerPrepareProgress {
                    line: line.to_owned(),
                }
                .emit(&app);
            },
            &flag,
        )
    })
    .await;
    drop(preparing);
    result
}

/// The scanner is off: runs are canceled (they then attach nothing), the
/// reports remembered for update reviews and attached to install reviews
/// are dropped (installs and updates are then gated on the built-in scan
/// only).
pub(crate) fn turned_off(state: &AppState) {
    state.scanner.turned_off();
    for (_, review) in &state.reviews().items {
        // A review being installed right now keeps its report.
        if let Ok(mut prepared) = review.try_lock() {
            prepared.set_external(None);
        }
    }
}

/// Turns the scanner on or off; off cancels every external scan (not a
/// Prepare) and drops the external reports of open reviews.
#[tauri::command]
#[specta::specta]
pub async fn set_scanner_enabled(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> CmdResult<ExternalScannerStatus> {
    let status = blocking(&state, move |core| set_enabled(core, enabled)).await?;
    if !enabled {
        turned_off(&state);
    }
    Ok(status)
}

/// Runs the scanner on an install review's content and attaches the
/// report: `install_reviewed_skill` then needs `allowDangerous` for a
/// dangerous external verdict. `run_id` names the run for
/// `cancel_external_scan`.
#[tauri::command]
#[specta::specta]
pub async fn external_scan_review(
    state: tauri::State<'_, AppState>,
    token: String,
    run_id: String,
) -> CmdResult<ExternalScanResult> {
    let since = state.scanner.generation();
    let generation = state.scanner.generation_counter();
    let review = state.review(&token)?;
    let running = state.scanner.start(Some(run_id));
    let flag = Arc::clone(&running.flag);
    let result = blocking(&state, move |core| {
        let mut prepared = review.lock().unwrap_or_else(PoisonError::into_inner);
        scan_review(core, &mut prepared, &flag, &generation, since)
    })
    .await;
    drop(running);
    result
}

/// Runs the scanner on the content `check_skill_updates` fetched for `id`
/// and remembers the report for `update_skill(id, latest, ..)`. A report
/// remembered for `id` before is dropped first, so a failed run never
/// leaves an older one. `latest` is the checked commit; when the source
/// moved on since, `update_skill` answers `entry_changed`.
#[tauri::command]
#[specta::specta]
pub async fn external_scan_update(
    state: tauri::State<'_, AppState>,
    id: String,
    latest: String,
    run_id: String,
) -> CmdResult<ExternalScanResult> {
    let _ = latest;
    let since = state.scanner.generation();
    let _ = state.scanner.take_update(&id);
    let running = state.scanner.start(Some(run_id));
    let flag = Arc::clone(&running.flag);
    let key = id.clone();
    let result = blocking(&state, move |core| scan_update(core, &id, &flag)).await;
    drop(running);
    let (report, result) = result?;
    if !state.scanner.remember_update_since(&key, report, since) {
        return Err(turned_off_meanwhile());
    }
    Ok(result)
}

/// Runs the scanner on a library skill's store content; report only.
#[tauri::command]
#[specta::specta]
pub async fn external_scan_skill(
    state: tauri::State<'_, AppState>,
    id: String,
    run_id: String,
) -> CmdResult<ExternalScanResult> {
    let running = state.scanner.start(Some(run_id));
    let flag = Arc::clone(&running.flag);
    let result = blocking(&state, move |core| scan_library(core, &id, &flag)).await;
    drop(running);
    result
}

/// Cancels the external scan `run_id`; `null` cancels every running scan
/// and a running Prepare.
#[tauri::command]
#[specta::specta]
pub async fn cancel_external_scan(
    state: tauri::State<'_, AppState>,
    run_id: Option<String>,
) -> CmdResult<()> {
    state.scanner.cancel(run_id.as_deref());
    Ok(())
}

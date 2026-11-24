//! The optional external scanner, Snyk Agent Scan (#27), for Settings >
//! Security and the install and update reviews: status, install (with
//! progress events, cancelable), remove, the switch, the token reference,
//! and one run on an install review, an available update or a library
//! skill. Nothing runs unless the user turned the scanner on. No token
//! value crosses IPC (only its reference); details are scrubbed of the
//! home folder.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use agent_loadout_core::scan::external::{
    self, ExternalReport, ExternalStatus, NotScanned, Runner, TokenRef, Tool, UreqDownload,
};
use agent_loadout_core::skills::install::{self, Prepared};
use agent_loadout_core::skills::{SkillFiles, content_hash, store as skill_store};
use agent_loadout_core::store::StoreRead;
use agent_loadout_core::{CoreError, Env, ErrorCode};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;

use super::scan::{ScanFinding, ScanVerdict};
use super::{AppState, CmdResult, Core, IpcError, blocking, scrub, scrubbed};

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
    /// `snyk-agent-scan`.
    pub scanner: String,
    pub version: String,
    pub status: ExternalScanStatus,
    pub reason: Option<ExternalNotScanned>,
    /// Meaningful only when `status` is `scanned`.
    pub verdict: ScanVerdict,
    pub findings: Vec<ScanFinding>,
    /// English, one line, no paths or values.
    pub detail: Option<String>,
}

impl ExternalScanResult {
    pub fn of(env: &Env, report: &ExternalReport) -> Self {
        Self {
            scanner: report.scanner.clone(),
            version: report.version.clone(),
            status: report.status.into(),
            reason: report.reason.map(ExternalNotScanned::from),
            verdict: report.verdict.into(),
            findings: report.findings.iter().map(ScanFinding::from).collect(),
            detail: report
                .detail
                .as_deref()
                .map(|detail| scrub(env, detail, None)),
        }
    }
}

/// How the token reference resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ScannerTokenKind {
    Alias,
    Env,
    Keyring,
    OnePassword,
}

/// Snyk Agent Scan on this machine.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExternalScannerStatus {
    /// `snyk-agent-scan`.
    pub scanner: String,
    /// The pinned version.
    pub version: String,
    /// A pinned build exists for this OS and architecture.
    pub supported: bool,
    pub installed: bool,
    pub enabled: bool,
    /// The reference text, never a value.
    pub token_ref: Option<String>,
    pub token_kind: Option<ScannerTokenKind>,
    /// Download size of this platform's build.
    pub size_bytes: Option<u32>,
    /// Seconds one run may take (`[snyk_agent_scan] timeout_secs`, default
    /// 60).
    pub timeout_secs: u32,
}

/// Whether the token reference resolves (Snyk is not contacted).
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ScannerTokenCheck {
    pub ok: bool,
    pub reason: Option<ExternalNotScanned>,
}

/// Download progress of `install_scanner`, in bytes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, Event)]
pub struct ScannerInstallProgress {
    pub received: u32,
    pub total: u32,
}

/// Runs in flight (each cancelable, by the caller's run id) and the
/// external reports of update reviews, by skill id, until `update_skill`
/// uses them.
#[derive(Default)]
pub(crate) struct ScannerState {
    running: Mutex<Vec<(Option<String>, Arc<AtomicBool>)>>,
    installing: AtomicBool,
    updates: Mutex<HashMap<String, ExternalReport>>,
}

impl ScannerState {
    /// A cancel flag for one run (`run_id`: the caller's, for a per-run
    /// cancel), registered until the guard drops.
    pub(crate) fn start(&self, run_id: Option<String>) -> Running<'_> {
        let flag = Arc::new(AtomicBool::new(false));
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((run_id, Arc::clone(&flag)));
        Running { state: self, flag }
    }

    /// Cancels the run `run_id`, or every run and install with `None`.
    pub(crate) fn cancel(&self, run_id: Option<&str>) {
        for (id, flag) in self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            if run_id.is_none() || id.as_deref() == run_id {
                flag.store(true, Ordering::SeqCst);
            }
        }
    }

    /// The install slot; `locked` while another install runs.
    pub(crate) fn begin_install(&self) -> CmdResult<Installing<'_>> {
        if self.installing.swap(true, Ordering::SeqCst) {
            return Err(IpcError {
                code: ErrorCode::Locked,
                message: "Snyk Agent Scan is already being installed".to_owned(),
            });
        }
        Ok(Installing(self))
    }

    pub(crate) fn remember_update(&self, id: &str, report: ExternalReport) {
        self.updates
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_owned(), report);
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

    /// The scanner was turned off (or removed): every run is canceled (an
    /// install goes on) and the remembered update reports are dropped.
    pub(crate) fn turned_off(&self) {
        for (id, flag) in self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
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
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(_, flag)| !Arc::ptr_eq(flag, &self.flag));
    }
}

/// Holds the install slot until dropped.
pub(crate) struct Installing<'a>(&'a ScannerState);

impl Drop for Installing<'_> {
    fn drop(&mut self) {
        self.0.installing.store(false, Ordering::SeqCst);
    }
}

fn ipc(core: &Core, err: &CoreError) -> IpcError {
    scrubbed(&core.env, err, None)
}

fn u32_of(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

pub fn status(core: &Core) -> CmdResult<ExternalScannerStatus> {
    let status = external::status(core.fs.as_ref(), &core.home).map_err(|err| ipc(core, &err))?;
    let token_kind = status
        .token
        .as_deref()
        .and_then(|text| TokenRef::parse(text).ok())
        .map(|parsed| match parsed.kind() {
            "alias" => ScannerTokenKind::Alias,
            "env" => ScannerTokenKind::Env,
            "keyring" => ScannerTokenKind::Keyring,
            _ => ScannerTokenKind::OnePassword,
        });
    Ok(ExternalScannerStatus {
        scanner: external::SCANNER.to_owned(),
        version: status.version.to_owned(),
        supported: status.supported,
        installed: status.installed,
        enabled: status.enabled,
        token_ref: status.token,
        token_kind,
        size_bytes: status.asset.map(|asset| u32_of(asset.size)),
        timeout_secs: u32_of(status.timeout.as_secs()),
    })
}

/// Downloads and verifies the pinned build; `progress(received, total)`.
/// `canceled` once `cancel` is set (nothing installed).
pub fn install(
    core: &Core,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> CmdResult<ExternalScannerStatus> {
    if !external::AVAILABLE {
        return Err(ipc(core, &external::unavailable()));
    }
    let tool = Tool::pinned(&core.home).map_err(|err| ipc(core, &err))?;
    tool.install(&UreqDownload::new(&core.env), progress, Some(cancel))
        .map_err(|err| ipc(core, &err))?;
    status(core)
}

/// Deletes the scanner and turns it off (the token reference is kept).
pub fn remove(core: &Core) -> CmdResult<ExternalScannerStatus> {
    let tool = Tool::pinned(&core.home).map_err(|err| ipc(core, &err))?;
    external::remove(core.fs.as_ref(), &core.home, &tool).map_err(|err| ipc(core, &err))?;
    status(core)
}

pub fn set_enabled(core: &Core, enabled: bool) -> CmdResult<ExternalScannerStatus> {
    external::set_enabled(core.fs.as_ref(), &core.home, enabled).map_err(|err| ipc(core, &err))?;
    status(core)
}

/// Sets or clears the reference; an alias must be defined in the secrets
/// catalog. `invalid_ref` never echoes the input (it may be a token).
pub fn set_token(core: &Core, reference: Option<String>) -> CmdResult<ExternalScannerStatus> {
    let store = core.open_store()?;
    let read = store.as_ref().map(|store| store as &dyn StoreRead);
    external::set_token(core.fs.as_ref(), &core.home, read, reference.as_deref())
        .map_err(|err| ipc(core, &err))?;
    status(core)
}

/// Resolves the token reference (may show the 1Password prompt); the value
/// is dropped at once.
pub fn check_token(core: &Core) -> CmdResult<ScannerTokenCheck> {
    let settings =
        external::settings(core.fs.as_ref(), &core.home).map_err(|err| ipc(core, &err))?;
    let Some(reference) = settings.token else {
        return Ok(ScannerTokenCheck {
            ok: false,
            reason: Some(ExternalNotScanned::NoToken),
        });
    };
    let store = core.open_store()?;
    let resolved = external::resolve_token(
        &core.env,
        core.fs.as_ref(),
        &core.home,
        store.as_ref(),
        &core.delivery,
        &reference,
    );
    Ok(match resolved {
        Ok(_) => ScannerTokenCheck {
            ok: true,
            reason: None,
        },
        Err((reason, _)) => ScannerTokenCheck {
            ok: false,
            reason: Some(reason.into()),
        },
    })
}

/// `scanner_unavailable` unless the scanner is available in this build
/// ([`external::AVAILABLE`]) and turned on.
fn require_enabled(core: &Core) -> CmdResult<()> {
    if !external::AVAILABLE {
        return Err(IpcError {
            code: ErrorCode::ScannerUnavailable,
            message: "Snyk Agent Scan is disabled in this version of Loadout".to_owned(),
        });
    }
    let settings =
        external::settings(core.fs.as_ref(), &core.home).map_err(|err| ipc(core, &err))?;
    if settings.enabled {
        Ok(())
    } else {
        Err(IpcError {
            code: ErrorCode::ScannerUnavailable,
            message: "Snyk Agent Scan is off on this machine; turn it on in Settings > Security"
                .to_owned(),
        })
    }
}

/// A ready run, or the report saying why not.
fn prepare(core: &Core) -> CmdResult<Result<Runner, ExternalReport>> {
    let store = core.open_store()?;
    Ok(external::prepare_run(
        &core.env,
        core.fs.as_ref(),
        &core.home,
        store.as_ref(),
        &core.delivery,
    ))
}

/// One run on `files` with `runner` (or its not-scanned report).
pub(crate) fn scan_with(
    core: &Core,
    runner: &Result<Runner, ExternalReport>,
    files: &SkillFiles,
    cancel: &AtomicBool,
) -> ExternalReport {
    match runner {
        Ok(runner) => runner.scan_files(&core.env, &core.home, files, Some(cancel)),
        Err(report) => {
            let mut report = report.clone();
            report.content_hash = Some(content_hash(files));
            report
        }
    }
}

/// One run, while the scanner is on: the switch is checked before the
/// token is resolved (which may wait on the 1Password prompt), again just
/// before the scanner starts, and once more after it ends, so a run the
/// user turned off meanwhile (`set_scanner_enabled(false)` also cancels
/// it) leaves no report behind.
fn scan(core: &Core, files: &SkillFiles, cancel: &AtomicBool) -> CmdResult<ExternalReport> {
    require_enabled(core)?;
    let runner = prepare(core)?;
    require_enabled(core)?;
    let report = scan_with(core, &runner, files, cancel);
    require_enabled(core)?;
    Ok(report)
}

/// `external_scan_required` when the scanner is on, installed and has a
/// token but `has_report` is false: the install or update would skip the
/// external scan the user turned on (the GUI runs it first; a canceled or
/// not-scanned run still counts as a report).
pub(crate) fn require_report(core: &Core, has_report: bool) -> CmdResult<()> {
    if has_report
        || !external::required(core.fs.as_ref(), &core.home).map_err(|err| ipc(core, &err))?
    {
        return Ok(());
    }
    Err(IpcError {
        code: ErrorCode::ExternalScanRequired,
        message: "Snyk Agent Scan is on but has not scanned this content yet; run the scan, \
                  then try again"
            .to_owned(),
    })
}

/// Scans an install review's content and attaches the report to it.
pub fn scan_review(
    core: &Core,
    prepared: &mut Prepared,
    cancel: &AtomicBool,
) -> CmdResult<ExternalScanResult> {
    let report = scan(core, prepared.files(), cancel)?;
    let result = ExternalScanResult::of(&core.env, &report);
    prepared.set_external(Some(report));
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

/// Downloads and verifies the pinned build, emitting
/// `ScannerInstallProgress`; `cancel_external_scan` stops it (`canceled`,
/// nothing installed). Installing does not turn the scanner on. A second
/// install while one runs is `locked`.
#[tauri::command]
#[specta::specta]
pub async fn install_scanner(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> CmdResult<ExternalScannerStatus> {
    let installing = state.scanner.begin_install()?;
    let running = state.scanner.start(None);
    let flag = Arc::clone(&running.flag);
    let result = blocking(&state, move |core| {
        let mut last = u64::MAX;
        install(
            core,
            &mut |received, total| {
                // About 1% steps, plus the first and the last.
                let step = (total / 100).max(1);
                if received / step != last || received == total {
                    last = received / step;
                    let _ = ScannerInstallProgress {
                        received: u32_of(received),
                        total: u32_of(total),
                    }
                    .emit(&app);
                }
            },
            &flag,
        )
    })
    .await;
    drop(running);
    drop(installing);
    result
}

/// Deletes the scanner and turns it off, like [`set_scanner_enabled`]
/// with `false`.
#[tauri::command]
#[specta::specta]
pub async fn remove_scanner(state: tauri::State<'_, AppState>) -> CmdResult<ExternalScannerStatus> {
    let status = blocking(&state, remove).await?;
    turned_off(&state);
    Ok(status)
}

/// The scanner is off: runs are canceled (they then attach nothing), the
/// reports remembered for update reviews and attached to install reviews
/// are dropped (installs and updates are then gated on the built-in scan
/// only).
fn turned_off(state: &AppState) {
    state.scanner.turned_off();
    for (_, review) in &state.reviews().items {
        // A review being installed right now keeps its report.
        if let Ok(mut prepared) = review.try_lock() {
            prepared.set_external(None);
        }
    }
}

/// Turns the scanner on or off; off cancels every external scan (not an
/// install) and drops the external reports of open reviews.
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

/// Sets (`invalid_ref` when it does not parse, or names an alias the
/// secrets catalog does not define; the input is never echoed) or clears
/// the token reference: a secret alias or a provider reference, never a
/// value.
#[tauri::command]
#[specta::specta]
pub async fn set_scanner_token(
    state: tauri::State<'_, AppState>,
    reference: Option<String>,
) -> CmdResult<ExternalScannerStatus> {
    blocking(&state, move |core| set_token(core, reference)).await
}

#[tauri::command]
#[specta::specta]
pub async fn check_scanner_token(
    state: tauri::State<'_, AppState>,
) -> CmdResult<ScannerTokenCheck> {
    blocking(&state, check_token).await
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
    let review = state.review(&token)?;
    let running = state.scanner.start(Some(run_id));
    let flag = Arc::clone(&running.flag);
    let result = blocking(&state, move |core| {
        let mut prepared = review.lock().unwrap_or_else(PoisonError::into_inner);
        scan_review(core, &mut prepared, &flag)
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
    let _ = state.scanner.take_update(&id);
    let running = state.scanner.start(Some(run_id));
    let flag = Arc::clone(&running.flag);
    let key = id.clone();
    let result = blocking(&state, move |core| scan_update(core, &id, &flag)).await;
    drop(running);
    let (report, result) = result?;
    state.scanner.remember_update(&key, report);
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
/// and the scanner install.
#[tauri::command]
#[specta::specta]
pub async fn cancel_external_scan(
    state: tauri::State<'_, AppState>,
    run_id: Option<String>,
) -> CmdResult<()> {
    state.scanner.cancel(run_id.as_deref());
    Ok(())
}

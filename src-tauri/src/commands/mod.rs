//! Tauri commands: thin wrappers over `agent-loadout-core` (SPEC 4.1). Every
//! value that crosses IPC is a typed struct or enum; display strings live in
//! the frontend catalog (`src/i18n/en.ts`), keyed by these enums and by
//! [`ErrorCode`]. No secret value crosses IPC: rendered client entries and
//! diffs are masked by the core, and drawer diffs also have every possible
//! secret in the client file masked, before they leave this crate.
//!
//! Each command is a thin `#[tauri::command]` over a plain function taking
//! a [`Core`], so the same functions run against a temp home in tests.

pub mod apply;
pub mod discover;
pub mod effective;
pub mod history;
pub mod import;
pub mod migrate;
pub mod project_servers;
pub mod projects;
pub mod remotes;
pub mod resolve;
pub mod secrets;
pub mod servers;
pub mod skills;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_discover;
#[cfg(test)]
mod tests_import;
#[cfg(test)]
mod tests_migrate;
#[cfg(test)]
mod tests_perf;
#[cfg(test)]
mod tests_project_servers;
#[cfg(test)]
mod tests_read_only;
#[cfg(test)]
mod tests_skills;
#[cfg(test)]
mod tests_versioning;
#[cfg(test)]
mod tests_watch;
pub mod tools;
pub mod updates;
pub mod watch;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agent_loadout_core::access::{self, ReadOnlyKind};
use agent_loadout_core::adapters::detect::NoRegistry;
use agent_loadout_core::apply::cache::{CachedPlan, PlanCache};
use agent_loadout_core::apply::{
    ApplyEnv, ApplyReport, ApplyStatus, EntryPlan, EntryState, TargetResult, placeholder_path,
};
use agent_loadout_core::model::applied::{ApplyOutcome, Scope as TargetScope};
use agent_loadout_core::paths::{self, HomeSource, LoadoutHome};
use agent_loadout_core::secrets::delivery::DeliveryEnv;
use agent_loadout_core::secrets::providers::keyring::{KeyringBackend, OsKeyring};
use agent_loadout_core::store::{CommitId, Store};
use agent_loadout_core::{CoreError, Env, ErrorCode, Fs, Os, RealFs, brand};
use serde::{Deserialize, Serialize};
use specta::Type;

/// The CLI has no Windows registry probe yet and neither has the app (M4
/// note); detection uses paths and `PATH` only.
static NO_REGISTRY: NoRegistry = NoRegistry;

/// What every command runs against: the process environment, the file
/// system, the data directory and the secret delivery environment. Tests
/// build one over a temp home.
pub struct Core {
    pub env: Env,
    pub fs: Box<dyn Fs>,
    pub home: LoadoutHome,
    pub delivery: DeliveryEnv,
    /// The pending plan the views share (#15): planned once per change.
    pub plan_cache: PlanCache,
}

impl Core {
    /// The running process: real file system, OS keyring, and the
    /// `loadout` binary next to the app as the `launch` launcher.
    pub fn from_process() -> Result<Self, CoreError> {
        let env = Env::from_process()?;
        let home = paths::loadout_home(&env, &RealFs, None)?;
        let keyring: Arc<dyn KeyringBackend> = Arc::new(OsKeyring);
        let delivery = DeliveryEnv::new(sibling_launcher(), keyring);
        Ok(Self {
            env,
            fs: Box::new(RealFs),
            home,
            delivery,
            plan_cache: PlanCache::new(),
        })
    }

    /// What an apply runs against.
    pub fn apply_env(&self) -> ApplyEnv<'_> {
        ApplyEnv {
            env: &self.env,
            fs: self.fs.as_ref(),
            home: &self.home,
            registry: &NO_REGISTRY,
            delivery: &self.delivery,
        }
    }

    /// The store, or `None` when it does not exist yet. Creates nothing.
    pub fn open_store(&self) -> Result<Option<Store<'_>>, CoreError> {
        Store::open(&self.env, self.fs.as_ref(), &self.home, &client_name())
    }

    /// The store, created on first use (the first change).
    pub fn open_store_for_change(&self) -> Result<Store<'_>, CoreError> {
        Store::open_or_init(&self.env, self.fs.as_ref(), &self.home, &client_name())
    }

    /// The plan of every scope and project of this machine, from the plan
    /// cache: planned again only when something it was planned from
    /// changed (the views' refetches on focus and drift cost a check).
    pub fn plan(&self) -> Result<Arc<CachedPlan>, CoreError> {
        self.plan_cache.plan(&self.apply_env(), &|fs| {
            Store::open(&self.env, fs, &self.home, &client_name())
        })
    }

    pub fn os(&self) -> Os {
        self.env.os()
    }
}

/// `Loadout-Client` trailer value: `loadout-app/<version>`.
fn client_name() -> String {
    format!("{}/{}", brand::APP_CLIENT_NAME, env!("CARGO_PKG_VERSION"))
}

/// The `loadout` binary installed next to the app, if there is one; `launch`
/// delivery reports `launcher_not_found` otherwise (SPEC 11.4, 11.5).
fn sibling_launcher() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join(format!(
        "{}{}",
        brand::BIN_NAME,
        std::env::consts::EXE_SUFFIX
    ));
    candidate.is_file().then_some(candidate)
}

/// Process-wide state the commands read. The core is built once at
/// startup; a failure is kept and reported by every command that needs it.
pub struct AppState {
    core: Result<Arc<Core>, CoreError>,
    /// Folders the user picked in the OS dialog, by the token the frontend
    /// got for each: the absolute path never crosses IPC (SPEC 17).
    picked: Mutex<HashMap<String, PathBuf>>,
}

impl AppState {
    pub fn from_process() -> Self {
        Self {
            core: Core::from_process().map(Arc::new),
            picked: Mutex::new(HashMap::new()),
        }
    }

    pub fn core(&self) -> Result<Arc<Core>, IpcError> {
        self.core.clone().map_err(IpcError::from)
    }

    /// Keeps a picked folder and returns its token.
    fn remember_folder(&self, folder: PathBuf) -> String {
        let mut picked = self.picked.lock().unwrap_or_else(|err| err.into_inner());
        let token = format!("folder-{}", picked.len() + 1);
        picked.insert(token.clone(), folder);
        token
    }

    /// The folder picked under `token`.
    fn picked_folder(&self, token: &str) -> Result<PathBuf, IpcError> {
        let picked = self.picked.lock().unwrap_or_else(|err| err.into_inner());
        picked.get(token).cloned().ok_or_else(|| IpcError {
            code: ErrorCode::NotFound,
            message: "the picked folder is not known any more; pick it again".to_owned(),
        })
    }
}

/// `text` with the folders that hold the user name replaced by their
/// placeholder roots: the project folder (`<P>`, when given), then the home
/// folder (`~`), each as written and with forward slashes. Core messages
/// may name absolute paths; the app sends them through this.
pub(crate) fn scrub(env: &Env, text: &str, project: Option<&Path>) -> String {
    let mut out = text.to_owned();
    let roots = [
        project.map(|root| (root.to_path_buf(), "<P>")),
        Some((env.home().to_path_buf(), "~")),
    ];
    for (root, placeholder) in roots.into_iter().flatten() {
        let native = root.to_string_lossy().into_owned();
        if native.is_empty() {
            continue;
        }
        let forward = native.replace('\\', "/");
        for form in [native.as_str(), forward.as_str()] {
            out = out.replace(form, placeholder);
        }
    }
    out
}

/// [`IpcError::from`] with [`scrub`] applied to the message.
pub(crate) fn scrubbed(env: &Env, err: &CoreError, project: Option<&Path>) -> IpcError {
    IpcError {
        code: err.code(),
        message: scrub(env, err.message(), project),
    }
}

/// A failed command: the stable code the frontend localizes from, plus
/// English diagnostic detail (never shown as state).
#[derive(Debug, Clone, Serialize, Type)]
pub struct IpcError {
    pub code: ErrorCode,
    pub message: String,
}

impl From<CoreError> for IpcError {
    fn from(err: CoreError) -> Self {
        Self::from(&err)
    }
}

impl From<&CoreError> for IpcError {
    fn from(err: &CoreError) -> Self {
        Self {
            code: err.code(),
            message: err.message().to_owned(),
        }
    }
}

pub type CmdResult<T> = Result<T, IpcError>;

fn errors(list: &[CoreError]) -> Vec<IpcError> {
    list.iter().map(IpcError::from).collect()
}

/// This account cannot write a file or folder (SPEC 8.2
/// `read_only_target`); the views show it and disable the controls that
/// would write it. `null` where it is writable or the probe could not tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
pub struct ReadOnly {
    pub kind: ReadOnlyKind,
}

impl ReadOnly {
    pub fn of(kind: Option<ReadOnlyKind>) -> Option<Self> {
        kind.map(|kind| Self { kind })
    }
}

/// A read-only file, under its placeholder root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct ReadOnlyFile {
    pub file: String,
    pub kind: ReadOnlyKind,
}

/// Whether this account can write each of `paths` (a file, or the folder
/// it would be created in; a folder for skill folders), probed together
/// under the probe budget and kept per file fingerprint in the plan
/// cache's probes (SPEC 8.2).
pub(crate) fn read_only_of(
    core: &Core,
    paths: &[PathBuf],
) -> HashMap<PathBuf, Option<ReadOnlyKind>> {
    let fs = core.fs.as_ref();
    let mut files: Vec<(PathBuf, Option<u64>)> = Vec::new();
    for path in paths {
        if files.iter().any(|(known, _)| known == path) {
            continue;
        }
        let bytes = fs
            .is_regular_file(path)
            .then(|| fs.read(path).ok())
            .flatten();
        files.push((path.clone(), access::fingerprint(bytes.as_deref())));
    }
    access::probe_files(fs, &files, Some(core.plan_cache.probes()))
        .into_iter()
        .map(|(path, found)| (path, found.read_only()))
        .collect()
}

/// Operating system the app runs on; selects the window chrome (SPEC 13.1:
/// custom controls on Windows, native decorations on Linux, Overlay title
/// bar on macOS).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Windows,
    Linux,
    Macos,
}

impl From<Os> for Platform {
    fn from(os: Os) -> Self {
        match os {
            Os::Windows => Platform::Windows,
            Os::Linux => Platform::Linux,
            Os::MacOs => Platform::Macos,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    /// The app crate's version (`CARGO_PKG_VERSION`).
    pub version: String,
    pub platform: Platform,
}

/// Version and platform. Brand strings are constants in the bindings.
#[tauri::command]
#[specta::specta]
pub fn app_info() -> AppInfo {
    let os = if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    };
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        platform: os.into(),
    }
}

/// Where the data directory setting came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DataHomeSource {
    /// The `--home` flag (CLI only; listed so every core source maps).
    Flag,
    /// The `LOADOUT_HOME` environment variable.
    EnvVar,
    /// The per-OS default (SPEC 4.5).
    Default,
}

impl From<HomeSource> for DataHomeSource {
    fn from(source: HomeSource) -> Self {
        match source {
            HomeSource::Flag => DataHomeSource::Flag,
            HomeSource::EnvVar => DataHomeSource::EnvVar,
            HomeSource::Default => DataHomeSource::Default,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DataHome {
    /// The folder under its placeholder root (`%LOCALAPPDATA%/Loadout`),
    /// for display.
    pub path: String,
    pub source: DataHomeSource,
}

/// The resolved `LOADOUT_HOME` (SPEC 4.5). Reads nothing inside it.
#[tauri::command]
#[specta::specta]
pub fn data_home(state: tauri::State<'_, AppState>) -> CmdResult<DataHome> {
    let core = state.core()?;
    let home = &core.home;
    Ok(DataHome {
        path: placeholder_path(&core.env, &home.path, None),
        source: home.source.into(),
    })
}

/// Which layer a client file belongs to (SPEC 6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FileScope {
    User,
    Project,
    ProjectLocal,
}

impl From<TargetScope> for FileScope {
    fn from(scope: TargetScope) -> Self {
        match scope {
            TargetScope::User => FileScope::User,
            TargetScope::Project => FileScope::Project,
            TargetScope::ProjectLocal => FileScope::ProjectLocal,
        }
    }
}

/// An entry compared with what is desired and what was written (SPEC 8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum EntryStateKind {
    Synced,
    PendingCreate,
    PendingUpdate,
    PendingRemove,
    Drifted,
    Missing,
    Conflict,
    Foreign,
    Collision,
    Blocked,
}

impl From<EntryState> for EntryStateKind {
    fn from(state: EntryState) -> Self {
        match state {
            EntryState::Synced => Self::Synced,
            EntryState::PendingCreate => Self::PendingCreate,
            EntryState::PendingUpdate => Self::PendingUpdate,
            EntryState::PendingRemove => Self::PendingRemove,
            EntryState::Drifted => Self::Drifted,
            EntryState::Missing => Self::Missing,
            EntryState::Conflict => Self::Conflict,
            EntryState::Foreign => Self::Foreign,
            EntryState::Collision => Self::Collision,
            EntryState::Blocked => Self::Blocked,
        }
    }
}

/// Result of an apply run or of one file (SPEC 8.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    Ok,
    Partial,
    Blocked,
    Failed,
}

impl From<ApplyOutcome> for OutcomeKind {
    fn from(outcome: ApplyOutcome) -> Self {
        match outcome {
            ApplyOutcome::Ok => Self::Ok,
            ApplyOutcome::Partial => Self::Partial,
            ApplyOutcome::Blocked => Self::Blocked,
            ApplyOutcome::Failed => Self::Failed,
        }
    }
}

/// What an apply run left for the user (the CLI's exit codes 0, 1, 4, 5, 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ApplyStatusKind {
    Done,
    FileFailed,
    NeedsDecision,
    MissingSecrets,
    TrustRequired,
}

impl From<ApplyStatus> for ApplyStatusKind {
    fn from(status: ApplyStatus) -> Self {
        match status {
            ApplyStatus::Done => Self::Done,
            ApplyStatus::FileFailed => Self::FileFailed,
            ApplyStatus::NeedsDecision => Self::NeedsDecision,
            ApplyStatus::MissingSecrets => Self::MissingSecrets,
            ApplyStatus::TrustRequired => Self::TrustRequired,
        }
    }
}

/// Planned or done create, update, remove and disable operations.
#[derive(Debug, Clone, Copy, Default, Serialize, Type)]
pub struct OpCounts {
    pub create: u32,
    pub update: u32,
    pub remove: u32,
    pub disable: u32,
}

impl From<agent_loadout_core::apply::plan::OpCounts> for OpCounts {
    fn from(counts: agent_loadout_core::apply::plan::OpCounts) -> Self {
        Self {
            create: counts.create,
            update: counts.update,
            remove: counts.remove,
            disable: counts.disable,
        }
    }
}

/// One entry of a client file that needs attention or changed.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EntryNote {
    /// The native entry key.
    pub name: String,
    pub client: Option<String>,
    pub server: Option<String>,
    pub state: EntryStateKind,
    /// The entry needs a decision (Overwrite, Import theirs, Skip).
    pub unresolved: bool,
    pub problem: Option<IpcError>,
}

impl From<&EntryPlan> for EntryNote {
    fn from(entry: &EntryPlan) -> Self {
        Self {
            name: entry.name.clone(),
            client: entry.client.map(str::to_owned),
            server: entry.server_id.clone(),
            state: entry.state.into(),
            unresolved: entry.unresolved,
            problem: entry.problem.as_ref().map(IpcError::from),
        }
    }
}

/// One client file of an apply run.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppliedFile {
    pub file: String,
    pub clients: Vec<String>,
    pub scope: FileScope,
    pub outcome: OutcomeKind,
    pub written: bool,
    pub counts: OpCounts,
    pub error: Option<IpcError>,
    /// Entries with a problem or a pending decision.
    pub notes: Vec<EntryNote>,
    pub warnings: Vec<IpcError>,
    /// This account cannot write the file: found while planning, or the OS
    /// refused the write (`error.code` `read_only_target`, `outcome`
    /// `blocked`).
    pub read_only: Option<ReadOnly>,
}

impl AppliedFile {
    fn new(env: &Env, target: &TargetResult) -> Self {
        Self {
            // Shown under its placeholder root, never the profile path (the
            // drawer opens nothing; SPEC 8.7 placeholders).
            file: placeholder_path(env, &target.file, target.project.as_deref()),
            clients: target.clients.iter().map(|c| (*c).to_owned()).collect(),
            scope: target.scope.into(),
            outcome: target.outcome.into(),
            written: target.written,
            counts: target.counts.into(),
            error: target.error.as_ref().map(IpcError::from),
            notes: target
                .entries
                .iter()
                .filter(|entry| entry.unresolved || entry.problem.is_some())
                .map(EntryNote::from)
                .collect(),
            warnings: errors(&target.warnings),
            read_only: ReadOnly::of(target.read_only),
        }
    }
}

/// An apply run as the app shows it (`loadout apply` report).
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ApplyRun {
    pub apply_id: String,
    pub outcome: OutcomeKind,
    pub status: ApplyStatusKind,
    /// Recorded in the store (`applied.toml` and the `Loadout-Apply`
    /// trailer), so it can be undone.
    pub recorded: bool,
    pub files_written: u32,
    pub unresolved: u32,
    /// Files written or needing attention.
    pub files: Vec<AppliedFile>,
    pub warnings: Vec<IpcError>,
}

impl ApplyRun {
    fn new(env: &Env, report: &ApplyReport) -> Self {
        Self {
            apply_id: report.apply_id.clone(),
            outcome: report.outcome.into(),
            status: report.status().into(),
            recorded: report.recorded,
            files_written: count(report.writes()),
            unresolved: count(report.unresolved()),
            files: report
                .targets
                .iter()
                .filter(|target| target.written || target.outcome != ApplyOutcome::Ok)
                .map(|target| AppliedFile::new(env, target))
                .collect(),
            warnings: errors(&report.warnings),
        }
    }
}

/// A store change and the apply that went with it: instant mode applies in
/// the same commit (SPEC 8.7); review mode leaves `apply` empty.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ChangeResult {
    pub commit: Option<String>,
    pub apply: Option<ApplyRun>,
}

impl ChangeResult {
    fn new(env: &Env, commit: Option<&CommitId>, report: Option<&ApplyReport>) -> Self {
        Self {
            commit: commit.map(CommitId::to_string),
            apply: report.map(|report| ApplyRun::new(env, report)),
        }
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Runs a blocking core call off the main thread, so the window stays
/// responsive while files are planned and written.
async fn blocking<T: Send + 'static>(
    state: &AppState,
    f: impl FnOnce(&Core) -> CmdResult<T> + Send + 'static,
) -> CmdResult<T> {
    let core = state.core()?;
    tauri::async_runtime::spawn_blocking(move || f(&core))
        .await
        .map_err(|err| IpcError {
            code: ErrorCode::IoError,
            message: format!("background task failed: {err}"),
        })?
}

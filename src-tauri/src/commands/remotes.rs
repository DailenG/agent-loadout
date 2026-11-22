//! Settings > Versioning & Backup and the History page's remote strip
//! (SPEC 13.6 item 4, 9.6 to 9.9, 15.2): this machine's remotes with their
//! status, the Add remote wizard (Git URL and Folder as equal choices),
//! push and pull, a git remote's token (one-way into the keyring), the
//! first run's "Add a backup remote" step, and "Restore from remote..."
//! with its machine list.
//!
//! Remotes live in the machine-local `state/remotes.toml`; none exists
//! until the user adds one. Folders come from the OS dialog and stay in the
//! backend (a token crosses IPC); paths and URLs cross only with the home
//! folder as `~` and URL credentials masked.

use std::path::{Component, Path, PathBuf};

use agent_loadout_core::KnownFolder;
use agent_loadout_core::local_settings::{BackupRemoteOffer, LocalSettings};
use agent_loadout_core::paths::{SyncRootKind, detect_sync_root};
use agent_loadout_core::remote::config::{FolderMode, RemoteConfig, RemoteKind, RemotesFile};
use agent_loadout_core::remote::credential::token_account;
use agent_loadout_core::remote::folder::{self, MachineEntry, Source, SourceChoice};
use agent_loadout_core::remote::gh;
use agent_loadout_core::remote::git::{self, GitAuth, SystemRunner};
use agent_loadout_core::remote::restore::{FromOutcome, restore_from};
use agent_loadout_core::remote::sync::{self, Pushed, RemoteStatus, SyncEnv};
use agent_loadout_core::remote::unc::SystemUnc;
use agent_loadout_core::secrets::SecretValue;
use agent_loadout_core::store::merge::MergeOutcome;
use agent_loadout_core::store::tags::AUTO_TAG_RETENTION;
use agent_loadout_core::{CoreError, ErrorCode, brand};
use serde::{Deserialize, Serialize};
use specta::Type;

use super::{AppState, ApplyRun, CmdResult, Core, IpcError, blocking, client_name, scrub};

static UNC: SystemUnc = SystemUnc;
static RUNNER: SystemRunner = SystemRunner {
    idle_timeout: git::IDLE_TIMEOUT,
};

/// A remote folder as the frontend sees it: under the longest of `~`,
/// `%LOCALAPPDATA%`, `%APPDATA%` and `%TEMP%` (folders that hold the user
/// name), else as typed (a drive, a share, a mount).
fn folder_text(core: &Core, folder: &Path) -> String {
    let env = &core.env;
    let spell = |name: &str| {
        if env.os() == agent_loadout_core::Os::Windows {
            format!("%{name}%")
        } else {
            format!("${name}")
        }
    };
    let mut bases: Vec<(String, PathBuf)> = vec![("~".to_owned(), env.home().to_path_buf())];
    for (name, known) in [
        ("LOCALAPPDATA", KnownFolder::LocalAppData),
        ("APPDATA", KnownFolder::RoamingAppData),
    ] {
        if let Some(path) = env.known_folder(known) {
            bases.push((spell(name), path.to_path_buf()));
        }
    }
    for name in ["TEMP", "TMP", "TMPDIR"] {
        if let Some(path) = env.var(name).map(PathBuf::from).filter(|p| p.is_absolute()) {
            bases.push((spell(name), path));
        }
    }
    let best = bases
        .iter()
        .filter_map(|(name, base)| {
            folder
                .strip_prefix(base)
                .ok()
                .map(|rest| (name, base, rest))
        })
        .max_by_key(|(_, base, _)| base.as_os_str().len());
    match best {
        Some((name, _, rest)) if rest.as_os_str().is_empty() => name.clone(),
        Some((name, _, rest)) => format!("{name}/{}", rest.to_string_lossy().replace('\\', "/")),
        None => scrub(env, &folder.to_string_lossy(), None),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RemoteKindDto {
    Git,
    Folder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FolderModeDto {
    RepoPerMachine,
    Mirror,
    Archive,
}

impl From<FolderMode> for FolderModeDto {
    fn from(mode: FolderMode) -> Self {
        match mode {
            FolderMode::RepoPerMachine => Self::RepoPerMachine,
            FolderMode::Mirror => Self::Mirror,
            FolderMode::Archive => Self::Archive,
        }
    }
}

impl From<FolderModeDto> for FolderMode {
    fn from(mode: FolderModeDto) -> Self {
        match mode {
            FolderModeDto::RepoPerMachine => Self::RepoPerMachine,
            FolderModeDto::Mirror => Self::Mirror,
            FolderModeDto::Archive => Self::Archive,
        }
    }
}

/// How a git remote authenticates (SPEC 9.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum GitAuthDto {
    /// The user's own git credentials (GCM, SSH agent, any helper).
    System,
    /// The GitHub CLI, for github.com URLs.
    Gh,
    /// A token in the OS keyring.
    Token,
}

impl From<GitAuth> for GitAuthDto {
    fn from(auth: GitAuth) -> Self {
        match auth {
            GitAuth::System => Self::System,
            GitAuth::Gh => Self::Gh,
            GitAuth::Token => Self::Token,
        }
    }
}

impl From<GitAuthDto> for GitAuth {
    fn from(auth: GitAuthDto) -> Self {
        match auth {
            GitAuthDto::System => Self::System,
            GitAuthDto::Gh => Self::Gh,
            GitAuthDto::Token => Self::Token,
        }
    }
}

/// A sync tool whose folder holds a folder remote: supported, since each
/// machine writes only its own subfolder (SPEC 9.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SyncTool {
    Resilio,
    Syncthing,
    OneDrive,
    Dropbox,
    GoogleDrive,
}

impl From<SyncRootKind> for SyncTool {
    fn from(kind: SyncRootKind) -> Self {
        match kind {
            SyncRootKind::Resilio => Self::Resilio,
            SyncRootKind::Syncthing => Self::Syncthing,
            SyncRootKind::OneDrive => Self::OneDrive,
            SyncRootKind::Dropbox => Self::Dropbox,
            SyncRootKind::GoogleDrive => Self::GoogleDrive,
        }
    }
}

/// A failure recorded for a remote's last push or pull.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LastError {
    pub error: IpcError,
    /// RFC 3339, UTC.
    pub at: String,
}

/// `ahead N`, `behind N`, last error (SPEC 9.8), without contacting a git
/// remote.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatusDto {
    /// Folder remotes: whether the folder is there now; `None` for git.
    pub reachable: Option<bool>,
    pub pushed_at: Option<String>,
    pub pulled_at: Option<String>,
    /// `None`: never pushed from this machine (or unknown).
    pub ahead: Option<u32>,
    /// Other machines' commits this store lacks; `None`: unknown (a git
    /// remote never fetched).
    pub behind: Option<u32>,
    pub last_error: Option<LastError>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEntry {
    pub name: String,
    pub kind: RemoteKindDto,
    /// The folder with the home folder as `~`, or the URL with any
    /// credentials masked.
    pub location: String,
    pub mode: Option<FolderModeDto>,
    pub auth: Option<GitAuthDto>,
    pub username: Option<String>,
    /// `manual`, `on-commit` or `interval:15m`.
    pub push: Option<String>,
    pub pull: Option<String>,
    /// `auth = token`: whether the keyring holds the token.
    pub token_stored: Option<bool>,
    /// The folder lies inside a sync tool's folder.
    pub sync_tool: Option<SyncTool>,
    pub status: RemoteStatusDto,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RemotesOverview {
    /// In the order they were added; none is ranked.
    pub remotes: Vec<RemoteEntry>,
    /// The first run's "Add a backup remote" step is still to be shown: no
    /// remote, and the user has not answered it on this machine.
    pub offer_backup_remote: bool,
    pub git_found: bool,
    pub gh_found: bool,
    /// Automatic tags are pruned after this many days (SPEC 9.4).
    pub auto_tag_retention_days: u32,
}

/// A folder picked for a remote or a restore.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PickedRemoteFolder {
    pub token: String,
    /// With the home folder as `~`.
    pub path: String,
    pub sync_tool: Option<SyncTool>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AddedRemote {
    pub remote: RemoteEntry,
    /// `tool_not_found` (git or gh missing), `remote_unreachable`.
    pub warnings: Vec<IpcError>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RemovedRemote {
    pub name: String,
    /// The git remote's token was deleted from the keyring.
    pub token_deleted: bool,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PushEntry {
    pub remote: String,
    pub ok: bool,
    pub head: Option<String>,
    /// A git push that was rejected pulled first.
    pub pulled: bool,
    pub error: Option<IpcError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MergeKind {
    UpToDate,
    FastForward,
    Merged,
    Conflicts,
}

impl From<&MergeOutcome> for MergeKind {
    fn from(outcome: &MergeOutcome) -> Self {
        match outcome {
            MergeOutcome::UpToDate => Self::UpToDate,
            MergeOutcome::FastForward(_) => Self::FastForward,
            MergeOutcome::Merged(_) => Self::Merged,
            MergeOutcome::Conflicts { .. } => Self::Conflicts,
        }
    }
}

/// One incoming head of a pull: a machine of a folder remote, or a git
/// remote's `main`.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PulledHead {
    pub machine_id: Option<String>,
    pub label: String,
    pub merge: Option<MergeKind>,
    pub conflicts: u32,
    /// `remote_incomplete` (objects still on their way) or another failure.
    pub error: Option<IpcError>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PullEntry {
    pub remote: String,
    pub ok: bool,
    pub heads: Vec<PulledHead>,
    pub conflicts: u32,
    /// "N servers, M skills changed by pull. Review and apply?" (SPEC 9.9).
    pub changed_servers: u32,
    pub changed_skills: u32,
    /// Sync tool artifacts skipped and left in place.
    pub artifacts_ignored: u32,
    pub error: Option<IpcError>,
}

/// Where "Restore from remote..." reads.
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RestoreSource {
    /// A configured folder remote, by name.
    Remote { name: String },
    /// A folder picked with `pick_remote_folder`: a folder remote, a
    /// machine folder, a bare repository or a mirror.
    Folder { token: String },
}

/// One machine of a folder remote (SPEC 9.6 "Restore lists machines").
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RemoteMachine {
    pub store_id: String,
    /// The store this machine uses.
    pub same_store: bool,
    pub machine_id: String,
    pub label: String,
    pub os: String,
    /// RFC 3339, UTC.
    pub pushed_at: String,
    pub head: Option<String>,
    /// The head commit's subject (repo-per-machine pushes).
    pub subject: Option<String>,
    pub this_machine: bool,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RestoreListing {
    /// The source is one store already (a machine folder, a bare
    /// repository, a mirror): restore it without picking.
    pub direct: bool,
    pub machines: Vec<RemoteMachine>,
    /// Sync tool artifacts skipped and left in place.
    pub artifacts_ignored: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RestoreFromKind {
    /// No store here before: created from the source.
    Created,
    /// The source's history merged into this store.
    Merged,
    /// An exported tree restored as one commit.
    Restored,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RestoreFromResult {
    pub kind: RestoreFromKind,
    /// `label (machine id)` of the source machine, or the folder's name.
    pub from: String,
    pub merge: Option<MergeKind>,
    pub conflicts: u32,
    pub tag: Option<String>,
    pub head: Option<String>,
    pub apply: Option<ApplyRun>,
}

/// `url` with userinfo (`https://user:token@host/...`) masked.
pub(super) fn mask_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let host_end = rest.find('/').unwrap_or(rest.len());
    match rest[..host_end].rfind('@') {
        Some(at) => format!("{scheme}://********@{}", &rest[at + 1..]),
        None => url.to_owned(),
    }
}

fn remotes_file(core: &Core) -> Result<RemotesFile, CoreError> {
    RemotesFile::load(core.fs.as_ref(), &core.home.state_dir())
}

fn sync_env() -> SyncEnv<'static> {
    SyncEnv {
        unc: &UNC,
        runner: &RUNNER,
        // `auth = token` asks the `loadout` binary installed with the app.
        helper: super::sibling_launcher(),
        app_version: env!("CARGO_PKG_VERSION"),
    }
}

fn status_dto(core: &Core, status: RemoteStatus) -> RemoteStatusDto {
    RemoteStatusDto {
        reachable: status.reachable,
        pushed_at: status.pushed_at,
        pulled_at: status.pulled_at,
        ahead: status.ahead.map(super::count),
        behind: status.behind.map(super::count),
        last_error: status.last_error.map(|error| LastError {
            error: IpcError {
                code: error.code,
                message: scrub(&core.env, &error.message, None),
            },
            at: error.at,
        }),
    }
}

fn entry(
    core: &Core,
    store: Option<&agent_loadout_core::store::Store<'_>>,
    remote: &RemoteConfig,
) -> RemoteEntry {
    let fs = core.fs.as_ref();
    let status = sync::status(&core.env, fs, &UNC, &core.home.state_dir(), store, remote);
    let auth = (remote.kind == RemoteKind::Git)
        .then(|| GitAuth::parse(remote.auth.as_deref()).ok())
        .flatten();
    let token_stored = (auth == Some(GitAuth::Token)).then(|| {
        core.delivery
            .keyring()
            .get(brand::KEYRING_SERVICE, &token_account(&remote.name))
            .is_ok_and(|value| value.is_some())
    });
    let sync_tool = remote
        .path
        .as_deref()
        .filter(|_| remote.kind == RemoteKind::Folder)
        .and_then(|path| detect_sync_root(&core.env, fs, Path::new(path)))
        .map(|root| root.kind.into());
    RemoteEntry {
        name: remote.name.clone(),
        kind: match remote.kind {
            RemoteKind::Git => RemoteKindDto::Git,
            RemoteKind::Folder => RemoteKindDto::Folder,
        },
        location: match remote.kind {
            RemoteKind::Folder => {
                folder_text(core, Path::new(remote.path.as_deref().unwrap_or("")))
            }
            // Git URLs may be local paths (`C:\Users\...\store.git`,
            // `file:///...`): the home folder goes out as `~` there too.
            RemoteKind::Git => scrub(
                &core.env,
                &mask_url(remote.url.as_deref().unwrap_or("")),
                None,
            ),
        },
        mode: (remote.kind == RemoteKind::Folder).then(|| remote.folder_mode().into()),
        auth: auth.map(Into::into),
        username: remote.username.clone(),
        push: remote.push.clone(),
        pull: remote.pull.clone(),
        token_stored,
        sync_tool,
        status: status_dto(core, status),
    }
}

/// This machine's remotes with their status.
pub fn overview(core: &Core) -> CmdResult<RemotesOverview> {
    let file = remotes_file(core)?;
    let store = core.open_store()?;
    let settings = LocalSettings::load(core.fs.as_ref(), &core.home.state_dir())?;
    Ok(RemotesOverview {
        remotes: file
            .remotes
            .iter()
            .map(|remote| entry(core, store.as_ref(), remote))
            .collect(),
        offer_backup_remote: file.remotes.is_empty() && settings.backup_remote_offer.is_none(),
        git_found: git::locate(&core.env, core.fs.as_ref()).is_some(),
        gh_found: gh::locate(&core.env, core.fs.as_ref()).is_some(),
        auto_tag_retention_days: super::count(
            usize::try_from(AUTO_TAG_RETENTION.as_secs() / 86_400).unwrap_or(usize::MAX),
        ),
    })
}

fn record_offer(core: &Core, answer: BackupRemoteOffer) -> Result<(), CoreError> {
    let state_dir = core.home.state_dir();
    let mut settings = LocalSettings::load(core.fs.as_ref(), &state_dir)?;
    settings.backup_remote_offer = Some(answer);
    settings.save(core.fs.as_ref(), &state_dir)
}

/// "Not now" on the first run's step: the store stays local-only and
/// `remotes.toml` is not written.
pub fn dismiss_offer(core: &Core) -> CmdResult<()> {
    Ok(record_offer(core, BackupRemoteOffer::NotNow)?)
}

fn invalid(why: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidPath, why.to_owned())
}

/// Checks a picked folder (SPEC 17): absolute, no `.` or `..` parts, an
/// existing folder, not inside the data directory (a remote there would
/// back the store up into itself).
pub fn validate_remote_folder(core: &Core, folder: &Path) -> Result<PathBuf, CoreError> {
    if !folder.is_absolute() {
        return Err(invalid("the folder is not an absolute path"));
    }
    if folder
        .components()
        .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(invalid("the folder path holds `.` or `..` parts"));
    }
    if !core.fs.is_dir(folder) {
        return Err(CoreError::new(
            ErrorCode::NotFound,
            "the folder does not exist",
        ));
    }
    let fs = core.fs.as_ref();
    let real = agent_loadout_core::layering::resolved(fs, folder);
    let data = agent_loadout_core::layering::resolved(fs, &core.home.path);
    if real.starts_with(&data) {
        return Err(invalid(&format!(
            "a remote cannot live inside the {} data folder",
            brand::PRODUCT_NAME
        )));
    }
    Ok(folder.to_path_buf())
}

/// A picked folder as the frontend sees it.
pub fn picked_info(core: &Core, folder: &Path, token: String) -> CmdResult<PickedRemoteFolder> {
    let folder = validate_remote_folder(core, folder)?;
    Ok(PickedRemoteFolder {
        token,
        path: folder_text(core, &folder),
        sync_tool: detect_sync_root(&core.env, core.fs.as_ref(), &folder).map(|r| r.kind.into()),
    })
}

/// Adds a folder remote at a picked folder. `first_run`: added from the
/// first run's step.
pub fn add_folder(
    core: &Core,
    name: &str,
    folder: &Path,
    mode: FolderModeDto,
    first_run: bool,
) -> CmdResult<AddedRemote> {
    let folder = validate_remote_folder(core, folder)?;
    let remote = RemoteConfig::folder(name, &folder.to_string_lossy(), mode.into());
    let mut file = remotes_file(core)?;
    file.add(remote.clone())?;
    file.save(core.fs.as_ref(), &core.home.state_dir())?;
    if first_run {
        record_offer(core, BackupRemoteOffer::Added)?;
    }
    let store = core.open_store()?;
    Ok(AddedRemote {
        remote: entry(core, store.as_ref(), &remote),
        warnings: Vec::new(),
    })
}

/// Adds a git remote. `username` is for `auth = token` (default
/// `x-access-token`).
pub fn add_git(
    core: &Core,
    name: &str,
    url: &str,
    auth: GitAuthDto,
    username: Option<&str>,
    first_run: bool,
) -> CmdResult<AddedRemote> {
    let url = url.trim();
    let auth: GitAuth = auth.into();
    if url.is_empty() || url.starts_with('-') || url.contains(char::is_whitespace) {
        return Err(CoreError::new(ErrorCode::InvalidValue, "invalid git URL").into());
    }
    if auth == GitAuth::Gh && !git::is_github_url(url) {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            "auth gh is for github.com URLs; use system or token for other hosts",
        )
        .into());
    }
    let mut remote = RemoteConfig::git(name, url, auth);
    remote.username = username
        .map(str::trim)
        .filter(|u| !u.is_empty() && auth == GitAuth::Token)
        .map(str::to_owned);
    let mut file = remotes_file(core)?;
    file.add(remote.clone())?;
    file.save(core.fs.as_ref(), &core.home.state_dir())?;
    if first_run {
        record_offer(core, BackupRemoteOffer::Added)?;
    }
    let mut warnings = Vec::new();
    if git::locate(&core.env, core.fs.as_ref()).is_none() {
        warnings.push(IpcError {
            code: ErrorCode::ToolNotFound,
            message: format!("git is not on PATH; pushes to {name} need it"),
        });
    }
    if auth == GitAuth::Gh && gh::locate(&core.env, core.fs.as_ref()).is_none() {
        warnings.push(IpcError {
            code: ErrorCode::ToolNotFound,
            message: "the GitHub CLI (gh) is not on PATH".to_owned(),
        });
    }
    let store = core.open_store()?;
    Ok(AddedRemote {
        remote: entry(core, store.as_ref(), &remote),
        warnings,
    })
}

/// Removes a remote; its files stay where they are, a stored token is
/// deleted.
pub fn remove(core: &Core, name: &str) -> CmdResult<RemovedRemote> {
    let mut file = remotes_file(core)?;
    let removed = file.remove(name)?;
    file.save(core.fs.as_ref(), &core.home.state_dir())?;
    let token_deleted = removed.kind == RemoteKind::Git
        && core
            .delivery
            .keyring()
            .delete(brand::KEYRING_SERVICE, &token_account(name))
            .unwrap_or(false);
    Ok(RemovedRemote {
        name: name.to_owned(),
        token_deleted,
    })
}

fn token_remote(core: &Core, name: &str) -> Result<(), CoreError> {
    let file = remotes_file(core)?;
    let remote = file
        .get(name)
        .ok_or_else(|| CoreError::new(ErrorCode::NotFound, format!("no remote named {name:?}")))?;
    if remote.kind != RemoteKind::Git || GitAuth::parse(remote.auth.as_deref())? != GitAuth::Token {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{name} does not use auth token"),
        ));
    }
    Ok(())
}

/// Stores a git remote's token in the OS keyring. The value goes one way:
/// nothing returns it.
pub fn set_token(core: &Core, name: &str, token: &str) -> CmdResult<()> {
    token_remote(core, name)?;
    let token = token.trim();
    if token.is_empty() {
        return Err(CoreError::new(ErrorCode::InvalidValue, "the token is empty").into());
    }
    let account = token_account(name);
    core.delivery
        .keyring()
        .set(brand::KEYRING_SERVICE, &account, &SecretValue::new(token))
        .map_err(|resolution| resolution.into_error(&account))?;
    Ok(())
}

/// Deletes a git remote's token; `false` when there was none.
pub fn clear_token(core: &Core, name: &str) -> CmdResult<bool> {
    let account = token_account(name);
    Ok(core
        .delivery
        .keyring()
        .delete(brand::KEYRING_SERVICE, &account)
        .map_err(|resolution| resolution.into_error(&account))?)
}

fn require_remotes(core: &Core) -> Result<RemotesFile, CoreError> {
    let file = remotes_file(core)?;
    if file.remotes.is_empty() {
        return Err(CoreError::new(ErrorCode::NotFound, "no backup remote"));
    }
    Ok(file)
}

fn require_store(core: &Core) -> Result<agent_loadout_core::store::Store<'_>, CoreError> {
    core.open_store()?
        .ok_or_else(|| CoreError::new(ErrorCode::NotFound, "there is no store yet"))
}

/// Pushes to one remote, or every remote.
pub fn push(core: &Core, name: Option<&str>) -> CmdResult<Vec<PushEntry>> {
    let store = require_store(core)?;
    let file = require_remotes(core)?;
    let results = sync::push_to(&store, &file, name, &sync_env(), &mut |_, _| {})?;
    Ok(results
        .into_iter()
        .map(|(remote, result)| match result {
            Ok(pushed) => {
                let (head, pulled) = match &pushed {
                    Pushed::Folder(outcome) => (&outcome.head, false),
                    Pushed::Git(outcome) => (&outcome.head, outcome.pulled.is_some()),
                };
                PushEntry {
                    remote,
                    ok: true,
                    head: Some(head.to_string()),
                    pulled,
                    error: None,
                }
            }
            Err(err) => PushEntry {
                remote,
                ok: false,
                head: None,
                pulled: false,
                error: Some(super::scrubbed(&core.env, &err, None)),
            },
        })
        .collect())
}

/// Pulls from one remote, or every remote, and merges (SPEC 9.9). Apply
/// never runs here: the page offers it.
pub fn pull(core: &Core, name: Option<&str>) -> CmdResult<Vec<PullEntry>> {
    let store = require_store(core)?;
    let file = require_remotes(core)?;
    let results = sync::pull_from(&store, &file, name, &sync_env(), &mut |_, _| {})?;
    Ok(results
        .into_iter()
        .map(|(remote, result)| match result {
            Ok(report) => {
                let (servers, skills) = sync::changed_by_pull(&store, &report);
                PullEntry {
                    remote,
                    ok: true,
                    heads: report
                        .siblings
                        .iter()
                        .map(|sibling| {
                            let (merge, conflicts, error) = match &sibling.outcome {
                                Ok(outcome) => (
                                    Some(MergeKind::from(outcome)),
                                    match outcome {
                                        MergeOutcome::Conflicts { conflicts, .. } => {
                                            conflicts.len()
                                        }
                                        _ => 0,
                                    },
                                    None,
                                ),
                                Err(err) => (None, 0, Some(super::scrubbed(&core.env, err, None))),
                            };
                            PulledHead {
                                machine_id: sibling.machine_id.clone(),
                                label: sibling.label.clone(),
                                merge,
                                conflicts: super::count(conflicts),
                                error,
                            }
                        })
                        .collect(),
                    conflicts: super::count(report.conflicts()),
                    changed_servers: super::count(servers),
                    changed_skills: super::count(skills),
                    artifacts_ignored: super::count(report.artifacts.len()),
                    error: None,
                }
            }
            Err(err) => PullEntry {
                remote,
                ok: false,
                heads: Vec::new(),
                conflicts: 0,
                changed_servers: 0,
                changed_skills: 0,
                artifacts_ignored: 0,
                error: Some(super::scrubbed(&core.env, &err, None)),
            },
        })
        .collect())
}

/// A restore source with a picked folder's token resolved.
#[derive(Debug, Clone)]
pub enum SourceRef {
    Remote(String),
    Folder(PathBuf),
}

impl SourceRef {
    fn resolve(state: &AppState, source: RestoreSource) -> Result<Self, IpcError> {
        Ok(match source {
            RestoreSource::Remote { name } => Self::Remote(name),
            RestoreSource::Folder { token } => Self::Folder(state.picked_folder(&token)?),
        })
    }
}

/// The folder a restore source names: a configured folder remote, which
/// must be reachable now, or a picked folder.
fn source_folder(core: &Core, source: &SourceRef) -> Result<PathBuf, CoreError> {
    match source {
        SourceRef::Remote(name) => {
            let file = remotes_file(core)?;
            let remote = file.get(name).ok_or_else(|| {
                CoreError::new(ErrorCode::NotFound, format!("no remote named {name:?}"))
            })?;
            if remote.kind != RemoteKind::Folder {
                return Err(CoreError::new(
                    ErrorCode::InvalidValue,
                    format!("{name} is a git remote; only folder remotes hold machine subfolders"),
                ));
            }
            folder::reachable_root(core.fs.as_ref(), core.env.os(), &UNC, remote)
        }
        SourceRef::Folder(folder) => validate_remote_folder(core, folder),
    }
}

fn machine(core: &Core, entry: &MachineEntry, store_id: Option<&str>) -> RemoteMachine {
    let this = agent_loadout_core::store::Machine::load(core.fs.as_ref(), &core.home.state_dir())
        .ok()
        .flatten()
        .is_some_and(|m| m.id == entry.machine_id);
    RemoteMachine {
        same_store: store_id == Some(entry.store_id.as_str()),
        store_id: entry.store_id.clone(),
        machine_id: entry.machine_id.clone(),
        label: entry.manifest.label.clone(),
        os: entry.manifest.os.clone(),
        pushed_at: entry.manifest.pushed_at.clone(),
        head: entry.manifest.head.clone(),
        subject: entry.subject.clone(),
        this_machine: this,
    }
}

/// The machines a source holds, to pick one from.
pub fn machines(core: &Core, source: &SourceRef) -> CmdResult<RestoreListing> {
    let folder = &source_folder(core, source)?;
    let store = core.open_store()?;
    let store_id = store.as_ref().map(|s| s.store_id().to_owned());
    match folder::open_source(core.fs.as_ref(), folder, None)? {
        SourceChoice::One { machine: entry, .. } => Ok(RestoreListing {
            direct: true,
            machines: entry
                .iter()
                .map(|entry| machine(core, entry, store_id.as_deref()))
                .collect(),
            artifacts_ignored: 0,
        }),
        SourceChoice::Choose(entries) => {
            let artifacts = folder::list(core.fs.as_ref(), folder, None)
                .map(|listing| listing.artifacts.len())
                .unwrap_or(0);
            Ok(RestoreListing {
                direct: false,
                machines: entries
                    .iter()
                    .map(|entry| machine(core, entry, store_id.as_deref()))
                    .collect(),
                artifacts_ignored: super::count(artifacts),
            })
        }
    }
}

/// Restores from `folder` (the machine `machine_id` when it holds
/// several). Client files are not touched: the page shows the secrets
/// checklist and the apply plan next (SPEC 11.9, 9.5).
pub fn restore(
    core: &Core,
    source: &SourceRef,
    machine_id: Option<&str>,
) -> CmdResult<RestoreFromResult> {
    let folder = &source_folder(core, source)?;
    let (source, entry): (Source, Option<Box<MachineEntry>>) =
        match folder::open_source(core.fs.as_ref(), folder, machine_id)? {
            SourceChoice::One { source, machine } => (source, machine),
            SourceChoice::Choose(entries) => {
                return Err(CoreError::new(
                    ErrorCode::InvalidValue,
                    format!("the folder holds {} machines; pick one", entries.len()),
                )
                .into());
            }
        };
    let label = entry.as_ref().map_or_else(
        || {
            folder
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
        },
        |e| format!("{} ({})", e.manifest.label, e.machine_id),
    );
    let report = restore_from(
        &core.apply_env(),
        &client_name(),
        &source,
        entry.as_deref(),
        &label,
        false,
    )
    .map_err(|err| super::scrubbed(&core.env, &err, None))?;
    let conflicts = super::count(report.conflicts());
    let apply = report.apply().map(|run| ApplyRun::new(&core.env, run));
    Ok(match report.outcome {
        FromOutcome::Created { head, .. } => RestoreFromResult {
            kind: RestoreFromKind::Created,
            from: label,
            merge: None,
            conflicts,
            tag: None,
            head: Some(head.to_string()),
            apply,
        },
        FromOutcome::Merged {
            outcome, tag, head, ..
        } => RestoreFromResult {
            kind: RestoreFromKind::Merged,
            from: label,
            merge: Some(MergeKind::from(&outcome)),
            conflicts,
            tag,
            head: Some(head.to_string()),
            apply,
        },
        FromOutcome::Restored(report) => RestoreFromResult {
            kind: RestoreFromKind::Restored,
            from: label,
            merge: None,
            conflicts,
            tag: report.tag,
            head: report.commit.map(|c| c.to_string()),
            apply,
        },
    })
}

#[tauri::command]
#[specta::specta]
pub async fn remotes_overview(state: tauri::State<'_, AppState>) -> CmdResult<RemotesOverview> {
    blocking(&state, overview).await
}

#[tauri::command]
#[specta::specta]
pub async fn dismiss_backup_remote_offer(state: tauri::State<'_, AppState>) -> CmdResult<()> {
    blocking(&state, dismiss_offer).await
}

/// The OS folder dialog (run from Rust; the path stays in the backend).
/// `None` when cancelled.
#[tauri::command]
#[specta::specta]
pub async fn pick_remote_folder(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> CmdResult<Option<PickedRemoteFolder>> {
    use tauri_plugin_dialog::DialogExt;
    let picked =
        tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_folder())
            .await
            .map_err(|err| IpcError {
                code: ErrorCode::IoError,
                message: format!("the folder picker failed: {err}"),
            })?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let folder = picked.into_path().map_err(|_| IpcError {
        code: ErrorCode::InvalidPath,
        message: "the picked item is not a local folder".to_owned(),
    })?;
    let token = state.remember_folder(folder.clone());
    blocking(&state, move |core| {
        picked_info(core, &folder, token).map(Some)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn add_folder_remote(
    state: tauri::State<'_, AppState>,
    name: String,
    token: String,
    mode: FolderModeDto,
    first_run: bool,
) -> CmdResult<AddedRemote> {
    let folder = state.picked_folder(&token)?;
    blocking(&state, move |core| {
        add_folder(core, &name, &folder, mode, first_run)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn add_git_remote(
    state: tauri::State<'_, AppState>,
    name: String,
    url: String,
    auth: GitAuthDto,
    username: Option<String>,
    first_run: bool,
) -> CmdResult<AddedRemote> {
    blocking(&state, move |core| {
        add_git(core, &name, &url, auth, username.as_deref(), first_run)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn remove_remote(
    state: tauri::State<'_, AppState>,
    name: String,
) -> CmdResult<RemovedRemote> {
    blocking(&state, move |core| remove(core, &name)).await
}

/// The one-way token input of an `auth = token` remote.
#[tauri::command]
#[specta::specta]
pub async fn set_remote_token(
    state: tauri::State<'_, AppState>,
    name: String,
    token: String,
) -> CmdResult<()> {
    blocking(&state, move |core| set_token(core, &name, &token)).await
}

#[tauri::command]
#[specta::specta]
pub async fn clear_remote_token(
    state: tauri::State<'_, AppState>,
    name: String,
) -> CmdResult<bool> {
    blocking(&state, move |core| clear_token(core, &name)).await
}

#[tauri::command]
#[specta::specta]
pub async fn push_remotes(
    state: tauri::State<'_, AppState>,
    name: Option<String>,
) -> CmdResult<Vec<PushEntry>> {
    blocking(&state, move |core| push(core, name.as_deref())).await
}

#[tauri::command]
#[specta::specta]
pub async fn pull_remotes(
    state: tauri::State<'_, AppState>,
    name: Option<String>,
) -> CmdResult<Vec<PullEntry>> {
    blocking(&state, move |core| pull(core, name.as_deref())).await
}

#[tauri::command]
#[specta::specta]
pub async fn list_restore_machines(
    state: tauri::State<'_, AppState>,
    source: RestoreSource,
) -> CmdResult<RestoreListing> {
    let source = SourceRef::resolve(&state, source)?;
    blocking(&state, move |core| machines(core, &source)).await
}

#[tauri::command]
#[specta::specta]
pub async fn restore_from_remote(
    state: tauri::State<'_, AppState>,
    source: RestoreSource,
    machine_id: Option<String>,
) -> CmdResult<RestoreFromResult> {
    let source = SourceRef::resolve(&state, source)?;
    blocking(&state, move |core| {
        restore(core, &source, machine_id.as_deref())
    })
    .await
}

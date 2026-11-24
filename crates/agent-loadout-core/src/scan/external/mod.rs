//! The optional external scanner (#27): Snyk Agent Scan, a second opinion
//! beside the built-in scanner. Off by default; when the user turns it on,
//! installs it ([`Tool::install`]: a pinned, verified build) and sets a
//! token reference, Loadout runs it on a skill before it is installed or
//! updated, or on a library skill on request ([`Runner::scan_files`]).
//!
//! **The skill's content leaves the machine**: the scanner sends every file
//! of the skill (Snyk redacts secrets it detects), the folder's path, the
//! user name and its version to Snyk's analysis API. Nothing is sent
//! unless the user turned the scanner on (or asked for one run with
//! `--external`).
//!
//! Results map into the M40 findings model ([`Finding`], `scanner =
//! "snyk-agent-scan"`) and are shown beside the built-in findings, never
//! instead of them. A `dangerous` external verdict needs the same explicit
//! "install anyway" as a built-in one ([`gate`]); a run that did not
//! happen or whose output was not understood never blocks and is never
//! shown as clean.

mod adapter;
mod install;
mod pin;
mod run;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use adapter::parse;
pub use install::{
    DOWNLOAD_HOSTS, Download, TOOLS_DIR, Tool, UreqDownload, check_download_url, copy,
};
pub use pin::{
    Asset, MAX_DOWNLOAD, PINNED, PinnedAsset, SIGNING_FINGERPRINT, VERSION, asset_for,
    download_url, pinned_asset,
};
pub use run::{PASSED_VARS, RUNS_DIR, Runner, TEMP_VARS, TOKEN_VAR, child_env};

use super::{Finding, Verdict};
use crate::env::Env;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::local_settings::{LocalSettings, SnykAgentScanSettings};
use crate::model::ids::{self, IdKind};
use crate::paths::LoadoutHome;
use crate::secrets::SecretValue;
use crate::secrets::catalog::Catalog;
use crate::secrets::delivery::{DeliveryEnv, op_env};
use crate::secrets::grammar::SecretRef;
use crate::secrets::providers::Resolver;
use crate::secrets::scanner::SecretMemory;
use crate::secrets::status::SecretStatus;
use crate::store::{Machine, Store, StoreRead};

/// The `scanner` of Snyk Agent Scan's findings and reports.
pub const SCANNER: &str = "snyk-agent-scan";
/// Seconds per run unless `[snyk_agent_scan] timeout_secs` says otherwise.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Whether the run happened and was understood.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalStatus {
    /// The scanner ran and its output was understood: the findings (none
    /// for a clean verdict) are its answer.
    Scanned,
    /// The content was not scanned ([`ExternalReport::reason`]).
    NotScanned,
    /// The scanner ran but its output is not the schema this build reads
    /// ("scanner output not understood (version X)"); never clean.
    NotUnderstood,
}

impl ExternalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            ExternalStatus::Scanned => "scanned",
            ExternalStatus::NotScanned => "not_scanned",
            ExternalStatus::NotUnderstood => "not_understood",
        }
    }
}

/// Why the content was not scanned. None of these blocks an install.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotScanned {
    /// The scanner is not installed on this machine.
    NotInstalled,
    /// Snyk publishes no build for this OS and architecture.
    UnsupportedPlatform,
    /// No token reference is set, or it resolved to an empty value.
    NoToken,
    /// The token reference did not resolve (missing, locked, provider
    /// unavailable).
    TokenUnavailable,
    /// Snyk refused the token (401).
    TokenRejected,
    /// Snyk's daily usage limit is reached.
    LimitReached,
    /// The skill is too large for Snyk's analysis (413).
    TooLarge,
    /// Snyk's analysis API could not be reached.
    Unreachable,
    /// The run took longer than the timeout and was stopped.
    Timeout,
    /// The user canceled the run.
    Canceled,
    /// Scanning would start an MCP server (declined).
    WouldStartServer,
    /// Any other failure (the scanner did not start, Snyk reported an
    /// analysis error): see [`ExternalReport::detail`].
    Failed,
}

impl NotScanned {
    pub const fn as_str(self) -> &'static str {
        match self {
            NotScanned::NotInstalled => "not_installed",
            NotScanned::UnsupportedPlatform => "unsupported_platform",
            NotScanned::NoToken => "no_token",
            NotScanned::TokenUnavailable => "token_unavailable",
            NotScanned::TokenRejected => "token_rejected",
            NotScanned::LimitReached => "limit_reached",
            NotScanned::TooLarge => "too_large",
            NotScanned::Unreachable => "unreachable",
            NotScanned::Timeout => "timeout",
            NotScanned::Canceled => "canceled",
            NotScanned::WouldStartServer => "would_start_server",
            NotScanned::Failed => "failed",
        }
    }

    /// English, for the CLI: "not scanned (<this>)".
    pub const fn text(self) -> &'static str {
        match self {
            NotScanned::NotInstalled => "not installed; run `loadout scanner install`",
            NotScanned::UnsupportedPlatform => "no build for this platform",
            NotScanned::NoToken => "no token; set one with `loadout scanner token`",
            NotScanned::TokenUnavailable => "the token could not be read",
            NotScanned::TokenRejected => "Snyk did not accept the token",
            NotScanned::LimitReached => "Snyk limit reached",
            NotScanned::TooLarge => "too large",
            NotScanned::Unreachable => "Snyk could not be reached",
            NotScanned::Timeout => "timed out",
            NotScanned::Canceled => "canceled",
            NotScanned::WouldStartServer => "would start the server",
            NotScanned::Failed => "the scanner failed",
        }
    }
}

/// One external scan of one skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalReport {
    /// [`SCANNER`].
    pub scanner: String,
    /// The scanner's version ([`VERSION`]).
    pub version: String,
    pub status: ExternalStatus,
    /// Set when `status` is [`ExternalStatus::NotScanned`].
    pub reason: Option<NotScanned>,
    /// The worst finding. `Clean` also when nothing was scanned: always
    /// read [`ExternalReport::status`] first.
    pub verdict: Verdict,
    /// Sorted by file, line and rule.
    pub findings: Vec<Finding>,
    /// [`crate::skills::content_hash`] of what was scanned.
    pub content_hash: Option<String>,
    /// English, one line, no paths or values: Snyk's message for a
    /// failure, what was not understood.
    pub detail: Option<String>,
}

impl ExternalReport {
    pub fn not_scanned(version: &str, reason: NotScanned, detail: Option<String>) -> Self {
        Self {
            scanner: SCANNER.to_owned(),
            version: version.to_owned(),
            status: ExternalStatus::NotScanned,
            reason: Some(reason),
            verdict: Verdict::Clean,
            findings: Vec::new(),
            content_hash: None,
            detail,
        }
    }

    pub fn not_understood(version: &str, why: &str) -> Self {
        Self {
            scanner: SCANNER.to_owned(),
            version: version.to_owned(),
            status: ExternalStatus::NotUnderstood,
            reason: None,
            verdict: Verdict::Clean,
            findings: Vec::new(),
            content_hash: None,
            detail: Some(format!(
                "scanner output not understood (version {version}): {why}"
            )),
        }
    }

    /// One line for the CLI: `dangerous (2 findings)`, `not scanned
    /// (Snyk limit reached)`, `scanner output not understood (version
    /// 0.7.1)`.
    pub fn summary(&self) -> String {
        match (self.status, self.reason) {
            (ExternalStatus::NotScanned, Some(reason)) => {
                format!("not scanned ({})", reason.text())
            }
            (ExternalStatus::NotUnderstood, _) => {
                format!("scanner output not understood (version {})", self.version)
            }
            _ => {
                let n = self.findings.len();
                let plural = if n == 1 { "" } else { "s" };
                format!("{} ({n} finding{plural})", self.verdict.as_str())
            }
        }
    }

    /// The findings that make the verdict `dangerous`.
    pub fn dangerous(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.severity == super::Severity::Dangerous)
    }
}

/// Refuses content the external scanner found `dangerous` (`scan_blocked`)
/// unless the user explicitly chose to install it anyway. A report that is
/// not scanned or not understood never blocks (its findings, if any, are
/// still Snyk's and do).
pub fn gate(report: &ExternalReport, allow_dangerous: bool) -> Result<(), CoreError> {
    if allow_dangerous || report.verdict != Verdict::Dangerous {
        return Ok(());
    }
    let labels: Vec<String> = report.dangerous().map(Finding::label).collect();
    let mut shown = labels
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if labels.len() > 3 {
        shown.push_str(&format!(" and {} more", labels.len() - 3));
    }
    let plural = if labels.len() == 1 { "" } else { "s" };
    Err(CoreError::new(
        ErrorCode::ScanBlocked,
        format!(
            "Snyk Agent Scan found {} dangerous finding{plural}: {shown}; review them, and install anyway only if you trust this content",
            labels.len()
        ),
    ))
}

/// A token reference: a `secrets.toml` alias or a provider reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenRef {
    Alias(String),
    Provider(SecretRef),
}

impl TokenRef {
    /// `env:NAME`, `keyring://service/account`, `op://vault/item/field`,
    /// or an alias name. `op-env://` is refused: Loadout never reads a
    /// 1Password Environment value itself.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        let text = text.trim();
        if text.contains(':') {
            // The input may be a pasted token: never echoed.
            let reference = SecretRef::parse(text).map_err(|_| not_a_reference())?;
            if matches!(reference, SecretRef::OnePasswordEnvironment { .. }) {
                return Err(CoreError::new(
                    ErrorCode::InvalidRef,
                    "an op-env:// reference cannot hold the scanner token: Loadout never reads \
                     1Password Environment values; use op://vault/item/field instead",
                ));
            }
            return Ok(Self::Provider(reference));
        }
        if ids::is_valid(IdKind::SecretAlias, text) {
            return Ok(Self::Alias(text.to_owned()));
        }
        Err(not_a_reference())
    }

    /// `alias`, `env`, `keyring` or `one_password`.
    pub fn kind(&self) -> &'static str {
        match self {
            TokenRef::Alias(_) => "alias",
            TokenRef::Provider(SecretRef::Env { .. }) => "env",
            TokenRef::Provider(SecretRef::Keyring { .. }) => "keyring",
            TokenRef::Provider(_) => "one_password",
        }
    }
}

/// The scanner's settings on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    /// The token reference text.
    pub token: Option<String>,
    pub timeout_secs: Option<u64>,
}

impl Settings {
    pub fn timeout(&self) -> Duration {
        self.timeout_secs
            .filter(|secs| *secs > 0)
            .map_or(DEFAULT_TIMEOUT, Duration::from_secs)
    }
}

pub fn settings(fs: &dyn Fs, home: &LoadoutHome) -> Result<Settings, CoreError> {
    let saved = LocalSettings::load(fs, &home.state_dir())?
        .snyk_agent_scan
        .unwrap_or_default();
    Ok(Settings {
        enabled: saved.enabled == Some(true),
        token: saved.token,
        timeout_secs: saved.timeout_secs,
    })
}

fn update(
    fs: &dyn Fs,
    home: &LoadoutHome,
    change: impl FnOnce(&mut SnykAgentScanSettings),
) -> Result<(), CoreError> {
    let state = home.state_dir();
    let mut settings = LocalSettings::load(fs, &state)?;
    let mut scanner = settings.snyk_agent_scan.take().unwrap_or_default();
    change(&mut scanner);
    settings.snyk_agent_scan = (scanner != SnykAgentScanSettings::default()).then_some(scanner);
    settings.save(fs, &state)
}

/// Turns the scanner on or off on this machine.
pub fn set_enabled(fs: &dyn Fs, home: &LoadoutHome, enabled: bool) -> Result<(), CoreError> {
    update(fs, home, |scanner| scanner.enabled = Some(enabled))
}

/// Deletes the installed scanner and turns it off (nothing could run);
/// the token reference and timeout are kept. `false` when nothing was
/// installed.
pub fn remove(fs: &dyn Fs, home: &LoadoutHome, tool: &Tool) -> Result<bool, CoreError> {
    let removed = tool.remove()?;
    set_enabled(fs, home, false)?;
    Ok(removed)
}

/// The reference does not parse. Never echoes the input: it may be a
/// pasted token.
fn not_a_reference() -> CoreError {
    CoreError::new(
        ErrorCode::InvalidRef,
        "this is not a secret alias or a provider reference (env:NAME, \
         keyring://service/account, op://vault/item/field); store the token with `loadout \
         secrets set` and use its alias",
    )
}

/// The secrets catalog this machine sees.
fn catalog(fs: &dyn Fs, home: &LoadoutHome, store: &dyn StoreRead) -> Result<Catalog, CoreError> {
    let machine = Machine::load(fs, &home.state_dir())?.map(|machine| machine.id);
    Catalog::load(store, machine.as_deref())
}

/// Sets (checked by [`TokenRef::parse`]) or clears the token reference.
/// An alias must be defined in the secrets catalog (`store`): a pasted
/// token can look like one, and is then refused without being echoed.
pub fn set_token(
    fs: &dyn Fs,
    home: &LoadoutHome,
    store: Option<&dyn StoreRead>,
    reference: Option<&str>,
) -> Result<(), CoreError> {
    let reference = reference.map(str::trim).filter(|text| !text.is_empty());
    if let Some(text) = reference
        && let TokenRef::Alias(alias) = TokenRef::parse(text)?
    {
        let defined = match store {
            Some(store) => catalog(fs, home, store)?.get(&alias).is_some(),
            None => false,
        };
        if !defined {
            return Err(CoreError::new(
                ErrorCode::InvalidRef,
                "this is not a secret alias defined in secrets.toml (see `loadout secrets ls`) \
                 or a provider reference (env:NAME, keyring://service/account, \
                 op://vault/item/field); store the token with `loadout secrets set` and use \
                 its alias",
            ));
        }
    }
    let reference = reference.map(str::to_owned);
    update(fs, home, |scanner| scanner.token = reference)
}

/// What `scanner status` and Settings show. No network, no secret read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// A pinned build exists for this platform.
    pub supported: bool,
    pub installed: bool,
    pub enabled: bool,
    /// The pinned version.
    pub version: &'static str,
    /// This platform's asset.
    pub asset: Option<Asset>,
    /// The token reference text, never a value.
    pub token: Option<String>,
    /// The installed binary.
    pub binary: Option<std::path::PathBuf>,
    pub timeout: Duration,
}

pub fn status(fs: &dyn Fs, home: &LoadoutHome) -> Result<Status, CoreError> {
    status_of(fs, home, Tool::pinned(home).ok())
}

/// [`status`] of `tool` (`None`: no build for this platform). Cheap: the
/// marker and the size, no hash.
pub fn status_of(fs: &dyn Fs, home: &LoadoutHome, tool: Option<Tool>) -> Result<Status, CoreError> {
    let settings = settings(fs, home)?;
    let installed = tool.as_ref().is_some_and(Tool::installed);
    Ok(Status {
        supported: tool.is_some(),
        installed,
        enabled: settings.enabled,
        version: VERSION,
        asset: tool.as_ref().map(|tool| tool.asset().clone()),
        token: settings.token.clone(),
        binary: tool.filter(|_| installed).map(|tool| tool.binary()),
        timeout: settings.timeout(),
    })
}

/// Whether an install or update must carry an external report: the
/// scanner is on, installed and has a token reference (whether the token
/// resolves is the run's business: a not-scanned report is a report).
pub fn required(fs: &dyn Fs, home: &LoadoutHome) -> Result<bool, CoreError> {
    let status = status(fs, home)?;
    Ok(status.enabled && status.installed && status.token.is_some())
}

/// A per-run override of the setting (`--external` / `--no-external`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Choice {
    /// The machine's setting.
    #[default]
    Default,
    On,
    Off,
}

/// Whether to run: [`Choice::On`], or [`Choice::Default`] with the scanner
/// turned on.
pub fn wanted(settings: &Settings, choice: Choice) -> bool {
    match choice {
        Choice::On => true,
        Choice::Off => false,
        Choice::Default => settings.enabled,
    }
}

/// Resolves the token reference with this machine's providers (an alias
/// through `secrets.toml` and this machine's overlay). May show the
/// 1Password approval prompt. The value is recorded in the process's
/// [`SecretMemory`], so a store commit that holds it is refused.
pub fn resolve_token(
    env: &Env,
    fs: &dyn Fs,
    home: &LoadoutHome,
    store: Option<&Store<'_>>,
    delivery: &DeliveryEnv,
    reference: &str,
) -> Result<SecretValue, (NotScanned, String)> {
    let unavailable = |why: String| (NotScanned::TokenUnavailable, why);
    let parsed = TokenRef::parse(reference).map_err(|err| unavailable(err.message().to_owned()))?;
    let (reference, account) = match parsed {
        TokenRef::Provider(reference) => (reference, None),
        TokenRef::Alias(alias) => {
            // The alias name is never shown: it may be a pasted token.
            let Some(store) = store else {
                return Err(unavailable(
                    "the token's secret alias is not defined (there is no store yet)".to_owned(),
                ));
            };
            let catalog =
                catalog(fs, home, store).map_err(|err| unavailable(err.message().to_owned()))?;
            let alias_entry = catalog
                .get(&alias)
                .ok_or_else(|| unavailable("the token's secret alias is not defined".to_owned()))?;
            let reference = alias_entry
                .parsed()
                .map_err(|err| unavailable(err.message().to_owned()))?;
            (reference, catalog.op_account.clone())
        }
    };
    let memory = match store {
        Some(store) => store.secret_memory().clone(),
        None => Arc::new(SecretMemory::new().map_err(|err| unavailable(err.message().to_owned()))?),
    };
    let op_env = op_env(env, delivery.keyring(), account.as_deref());
    let op = delivery.op(env, fs);
    Resolver::new(&op_env, delivery.keyring(), op, memory)
        .resolve(&reference)
        .map_err(|resolution| {
            let reason = match resolution.status {
                SecretStatus::Missing => "the token is not set where its reference points",
                SecretStatus::Locked => "the token's provider is locked",
                _ => "the token could not be read",
            };
            unavailable(reason.to_owned())
        })
        .and_then(|value| {
            if value.expose().trim().is_empty() {
                Err((NotScanned::NoToken, "the token is empty".to_owned()))
            } else {
                Ok(value)
            }
        })
}

/// A run that is ready, or the `not_scanned` report saying why not: not
/// supported here, not installed (or the binary no longer matches the pin:
/// it is hashed here, once per command, and marked not installed so
/// `install` repairs it), no token reference, or a token that did not
/// resolve. Resolving may show the 1Password approval prompt.
// The `Err` is the report every skill of the command gets, built once per
// command, not an error on a hot path.
#[allow(clippy::result_large_err)]
pub fn prepare_run(
    env: &Env,
    fs: &dyn Fs,
    home: &LoadoutHome,
    store: Option<&Store<'_>>,
    delivery: &DeliveryEnv,
) -> Result<Runner, ExternalReport> {
    let Ok(tool) = Tool::pinned(home) else {
        return Err(ExternalReport::not_scanned(
            VERSION,
            NotScanned::UnsupportedPlatform,
            None,
        ));
    };
    prepare_run_with(tool, env, fs, home, store, delivery)
}

/// [`prepare_run`] with `tool` (tests run a stub with its own pin).
#[allow(clippy::result_large_err)]
pub fn prepare_run_with(
    tool: Tool,
    env: &Env,
    fs: &dyn Fs,
    home: &LoadoutHome,
    store: Option<&Store<'_>>,
    delivery: &DeliveryEnv,
) -> Result<Runner, ExternalReport> {
    let version = tool.version().to_owned();
    let not_scanned =
        |reason, detail: Option<String>| ExternalReport::not_scanned(&version, reason, detail);
    if !tool.installed() {
        return Err(not_scanned(NotScanned::NotInstalled, None));
    }
    if let Err(why) = tool.verify() {
        return Err(not_scanned(NotScanned::NotInstalled, Some(why)));
    }
    let settings = settings(fs, home)
        .map_err(|err| not_scanned(NotScanned::Failed, Some(err.message().to_owned())))?;
    let Some(reference) = settings.token.as_deref() else {
        return Err(not_scanned(NotScanned::NoToken, None));
    };
    let token = resolve_token(env, fs, home, store, delivery, reference)
        .map_err(|(reason, why)| not_scanned(reason, Some(why)))?;
    Ok(Runner::new(tool, token, settings.timeout()))
}

/// What is sent to Snyk, verbatim as `docs/security.md` lists it; the CLI
/// prints it when the scanner is turned on and in `scanner status`.
pub const DISCLOSURE: &str = "Snyk Agent Scan sends the skill to Snyk's analysis API \
(api.snyk.io): every file of the skill (Snyk redacts secrets it detects), the skill's name, \
the path of the folder it scans, your user name, the scanner's version and this machine's \
host name (the scanner adds it itself where its platform reports one). Loadout adds no host \
name or machine serial, and runs it only on skills, never on MCP servers. Nothing is sent \
unless you turn it on (or pass --external).";

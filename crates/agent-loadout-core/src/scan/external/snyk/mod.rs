//! Snyk Agent Scan (#27, 0.7.0): the first optional external scanner, a
//! pinned, verified build ([`Tool::install`]) with a token reference, run
//! on a skill before it is installed or updated ([`Runner::scan_files`]).
//! It sent every file of the skill, the folder's path, the user name and
//! its version to Snyk's analysis API.
//!
//! **Disabled since 0.7.1** ([`AVAILABLE`]): Snyk's free tier refuses new
//! accounts. Nothing here runs, downloads or executes; `loadout scanner
//! remove` still deletes a 0.7.0 install ([`remove`]). The code stays for
//! a possible return; SkillSpector ([`super::skillspector`]) is the
//! optional scanner since 0.8.0.

mod adapter;
mod install;
mod pin;
mod run;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

pub use adapter::parse;
pub use install::{
    DOWNLOAD_HOSTS, Download, TOOLS_DIR, Tool, UreqDownload, check_download_url, copy,
};
pub use pin::{
    Asset, MAX_DOWNLOAD, PINNED, PinnedAsset, SIGNING_FINGERPRINT, VERSION, asset_for,
    download_url, pinned_asset,
};
pub use run::{PASSED_VARS, Runner, TEMP_VARS, TOKEN_VAR, child_env};

use super::{Choice, ExternalReport, NotScanned, Scanner};
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
pub const SCANNER: Scanner = Scanner::SnykAgentScan;
/// Seconds per run unless `[snyk_agent_scan] timeout_secs` says otherwise.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Whether this build offers the external scanner at all. `false` since
/// 0.7.1 (2026-11-24): Snyk's free tier refuses new accounts by every
/// path, so the maintainer dropped Snyk. While `false` the scanner never
/// runs (whatever `[snyk_agent_scan] enabled` says), is never downloaded
/// or executed, and its CLI and GUI surfaces are hidden; only `scanner
/// remove` (cleaning up a 0.7.0 install) and `scanner status` work. The
/// code stays for a possible future scanner: flip this to re-enable.
pub const AVAILABLE: bool = false;

/// `scanner_unavailable`: the external scanner is disabled in this build.
pub fn unavailable() -> CoreError {
    CoreError::new(
        ErrorCode::ScannerUnavailable,
        "the optional external scanner (Snyk Agent Scan) is disabled in this build; run \
         `loadout scanner remove` to delete a 0.7.0 install",
    )
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
/// the token reference and timeout are kept, unless the scanner is
/// disabled in this build ([`AVAILABLE`]): then the whole
/// `[snyk_agent_scan]` entry goes too, cleaning up a 0.7.0 install.
/// `false` when nothing was installed.
pub fn remove(fs: &dyn Fs, home: &LoadoutHome, tool: &Tool) -> Result<bool, CoreError> {
    let removed = tool.remove()?;
    if AVAILABLE {
        set_enabled(fs, home, false)?;
    } else {
        update(fs, home, |scanner| {
            *scanner = SnykAgentScanSettings::default()
        })?;
    }
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
    if !AVAILABLE {
        return Ok(false);
    }
    let status = status(fs, home)?;
    Ok(status.enabled && status.installed && status.token.is_some())
}

/// Whether to run: never unless [`AVAILABLE`]; then [`wanted_by`].
pub fn wanted(settings: &Settings, choice: Choice) -> bool {
    AVAILABLE && wanted_by(settings, choice)
}

/// What the setting and `choice` ask for: [`Choice::On`], or
/// [`Choice::Default`] with the scanner turned on.
pub fn wanted_by(settings: &Settings, choice: Choice) -> bool {
    choice.wants(settings.enabled)
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
            SCANNER,
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
    let not_scanned = |reason, detail: Option<String>| {
        ExternalReport::not_scanned(SCANNER, &version, reason, detail)
    };
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

//! The 1Password CLI (`op`), SPEC 11.2 and 11.11.
//!
//! - Detection: `op` on `PATH` (on Windows also
//!   `%LOCALAPPDATA%\Microsoft\WinGet\Links`) and `op --version`. Nothing
//!   here signs in; sign-in state shows only when a reference is checked.
//! - Item references: `op read --no-newline <ref>`, any 2.x.
//! - Environment references (`op-env://`) need 2.41.0 (`op run
//!   --environment`, used by `loadout exec`); listing Environment variable
//!   names needs 2.42.0 (`op environment secret list <id> --format json`),
//!   whose output is reduced to names at once.
//! - Loadout never calls `op environment read` (deprecated, removed in
//!   2027).
//!
//! `op` runs with exactly the variables of the injected [`Env`], so a
//! service account token or `OP_*` setting in Loadout's environment
//! reaches it, and tests control everything it sees.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde::Deserialize;

use crate::env::{Env, KnownFolder, Os};
use crate::fs::Fs;
use crate::secrets::SecretValue;
use crate::secrets::providers::{ProviderInfo, ProviderKind, ProviderState};
use crate::secrets::status::{Resolution, SecretStatus};

/// What `loadout secrets providers` and every `op` status say when the CLI
/// is absent (SPEC 11.11).
pub const OP_NOT_FOUND: &str = "1Password CLI not found";

/// A dotted `op` version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OpVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl OpVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Reads `2.42.0` (the first line of `op --version`); a pre-release
    /// suffix (`2.42.0-beta.01`) is ignored.
    pub fn parse(text: &str) -> Option<Self> {
        let line = text.lines().next()?.trim();
        let core = line.split(['-', '+', ' ']).next()?;
        let mut parts = core.split('.').map(|part| part.parse::<u32>().ok());
        let version = Self::new(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for OpVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Version-gated `op` features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpFeature {
    /// `op read` of `op://` item references.
    ItemRead,
    /// `op run --environment` for `op-env://` references.
    Environments,
    /// `op environment secret list` for bulk mapping and checks.
    EnvironmentListing,
}

impl OpFeature {
    pub const ALL: [OpFeature; 3] = [
        OpFeature::ItemRead,
        OpFeature::Environments,
        OpFeature::EnvironmentListing,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            OpFeature::ItemRead => "item_read",
            OpFeature::Environments => "environments",
            OpFeature::EnvironmentListing => "environment_listing",
        }
    }

    /// The oldest CLI with this feature.
    pub const fn min_version(self) -> OpVersion {
        match self {
            OpFeature::ItemRead => OpVersion::new(2, 0, 0),
            OpFeature::Environments => OpVersion::new(2, 41, 0),
            OpFeature::EnvironmentListing => OpVersion::new(2, 42, 0),
        }
    }

    const fn describe(self) -> &'static str {
        match self {
            OpFeature::ItemRead => "1Password item references",
            OpFeature::Environments => "1Password Environments",
            OpFeature::EnvironmentListing => "Listing 1Password Environment variable names",
        }
    }
}

/// The 1Password CLI on this machine, as detected once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpCli {
    NotFound,
    Found {
        path: PathBuf,
        /// The version, or why `op --version` could not be read.
        version: Result<OpVersion, String>,
    },
}

impl OpCli {
    /// Finds `op` and reads its version. Never prompts.
    pub fn detect(env: &Env, fs: &dyn Fs) -> Self {
        let Some(path) = locate(env, fs) else {
            return Self::NotFound;
        };
        let version = match run(env, &path, &["--version"]) {
            Ok(output) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout);
                OpVersion::parse(&text)
                    .ok_or_else(|| format!("unexpected `op --version` output {:?}", text.trim()))
            }
            Ok(output) => Err(format!("`op --version` failed ({})", output.status)),
            Err(err) => Err(format!("cannot run {}: {err}", path.display())),
        };
        Self::Found { path, version }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            OpCli::NotFound => None,
            OpCli::Found { path, .. } => Some(path),
        }
    }

    pub fn version(&self) -> Option<OpVersion> {
        match self {
            OpCli::Found {
                version: Ok(version),
                ..
            } => Some(*version),
            _ => None,
        }
    }

    pub fn supports(&self, feature: OpFeature) -> bool {
        self.require(feature).is_ok()
    }

    /// The `op` path when `feature` is available, else the status that
    /// blocks it: `provider_unavailable` ("1Password CLI not found"),
    /// `provider_too_old` or `error`.
    pub fn require(&self, feature: OpFeature) -> Result<&Path, Resolution> {
        match self {
            OpCli::NotFound => Err(Resolution::new(
                SecretStatus::ProviderUnavailable,
                OP_NOT_FOUND,
            )),
            OpCli::Found {
                version: Err(err), ..
            } => Err(Resolution::error(format!(
                "cannot read the 1Password CLI version: {err}"
            ))),
            OpCli::Found {
                path,
                version: Ok(version),
            } => {
                let min = feature.min_version();
                // 1.x (another command syntax altogether) is below 2.0.0.
                if *version < min {
                    return Err(Resolution::new(
                        SecretStatus::ProviderTooOld,
                        format!(
                            "{} need 1Password CLI {min} or newer; this machine has {version}",
                            feature.describe()
                        ),
                    ));
                }
                Ok(path)
            }
        }
    }

    /// How `loadout secrets providers` shows the CLI.
    pub fn info(&self) -> ProviderInfo {
        let features = OpFeature::ALL
            .iter()
            .map(|feature| (*feature, self.supports(*feature)))
            .collect();
        let (state, detail, version) = match self {
            OpCli::NotFound => (ProviderState::Unavailable, OP_NOT_FOUND.to_owned(), None),
            OpCli::Found {
                path,
                version: Ok(version),
            } => (
                ProviderState::Available,
                path.display().to_string(),
                Some(version.to_string()),
            ),
            OpCli::Found {
                version: Err(err), ..
            } => (ProviderState::Error, err.clone(), None),
        };
        ProviderInfo {
            kind: ProviderKind::OnePassword,
            state,
            detail,
            version,
            features,
        }
    }

    /// `op read --no-newline <reference>`.
    pub fn read(&self, env: &Env, reference: &str) -> Result<SecretValue, Resolution> {
        let path = self.require(OpFeature::ItemRead)?;
        let output = run(env, path, &["read", "--no-newline", reference])
            .map_err(|err| Resolution::error(format!("cannot run the 1Password CLI: {err}")))?;
        if !output.status.success() {
            return Err(failure(&output));
        }
        String::from_utf8(output.stdout)
            .map(SecretValue::new)
            .map_err(|_| Resolution::error("the 1Password value is not UTF-8 text"))
    }

    /// Variable names of a 1Password Environment (2.42.0+). Values in the
    /// output are dropped while parsing and never logged (SPEC 11.10).
    pub fn list_environment(
        &self,
        env: &Env,
        environment: &str,
    ) -> Result<Vec<String>, Resolution> {
        let path = self.require(OpFeature::EnvironmentListing)?;
        let output = run(
            env,
            path,
            &[
                "environment",
                "secret",
                "list",
                environment,
                "--format",
                "json",
            ],
        )
        .map_err(|err| Resolution::error(format!("cannot run the 1Password CLI: {err}")))?;
        if !output.status.success() {
            return Err(failure(&output));
        }
        #[derive(Deserialize)]
        struct Named {
            name: String,
        }
        let names: Vec<Named> = serde_json::from_slice(&output.stdout).map_err(|_| {
            Resolution::error("unexpected `op environment secret list` output (not a JSON array of named entries)")
        })?;
        drop(output);
        Ok(names.into_iter().map(|named| named.name).collect())
    }

    /// Whether `name` is a variable of `environment`: `resolved` when it is
    /// listed (the value is never read), `missing` when not, `unverifiable`
    /// on 2.41.x, which cannot list names.
    pub fn check_environment_variable(
        &self,
        env: &Env,
        environment: &str,
        name: &str,
    ) -> Resolution {
        if let Err(blocked) = self.require(OpFeature::Environments) {
            return blocked;
        }
        if let Err(too_old) = self.require(OpFeature::EnvironmentListing) {
            return Resolution::new(
                SecretStatus::Unverifiable,
                format!(
                    "{}; the variable is checked when a server starts",
                    too_old.message.unwrap_or_default()
                ),
            );
        }
        match self.list_environment(env, environment) {
            Ok(names) if names.iter().any(|listed| listed == name) => Resolution::resolved(),
            Ok(_) => Resolution::missing(format!(
                "{name} is not a variable of 1Password Environment {environment}"
            )),
            Err(resolution) => resolution,
        }
    }
}

/// `op` on `PATH`, then (Windows) in the WinGet links folder. Relative
/// `PATH` entries are skipped, so a project folder cannot shadow `op`.
pub fn locate(env: &Env, fs: &dyn Fs) -> Option<PathBuf> {
    let (separator, file) = match env.os() {
        Os::Windows => (';', "op.exe"),
        Os::Linux | Os::MacOs => (':', "op"),
    };
    let path_var = env
        .var("PATH")
        .map(|path| path.to_string_lossy().into_owned());
    let mut dirs: Vec<PathBuf> = path_var
        .iter()
        .flat_map(|path| path.split(separator))
        .map(|dir| PathBuf::from(dir.trim().trim_matches('"')))
        .filter(|dir| dir.is_absolute())
        .collect();
    if env.os() == Os::Windows
        && let Some(local) = env.known_folder(KnownFolder::LocalAppData)
    {
        dirs.push(local.join("Microsoft").join("WinGet").join("Links"));
    }
    dirs.into_iter()
        .map(|dir| dir.join(file))
        .find(|candidate| fs.exists(candidate) && !fs.is_dir(candidate))
}

fn run(env: &Env, path: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new(path)
        .args(args)
        .env_clear()
        .envs(
            env.vars()
                .map(|(name, value)| (OsString::from(name), value)),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
}

/// A failed `op` call as a status. `op` exits 1 for every error, so the
/// status comes from its message [verify: exact wording per version]; the
/// message itself is reduced to one line and never holds a value.
fn failure(output: &Output) -> Resolution {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lower = stderr.to_lowercase();
    const LOCKED: [&str; 9] = [
        "not currently signed in",
        "not signed in",
        "sign in",
        "signin",
        "authorization prompt",
        "authorization timeout",
        "authorization was denied",
        "session expired",
        "locked",
    ];
    const MISSING: [&str; 8] = [
        "isn't an item",
        "isn't a vault",
        "isn't a field",
        "does not have a field",
        "could not find",
        "not found",
        "no item",
        "doesn't exist",
    ];
    if LOCKED.iter().any(|needle| lower.contains(needle)) {
        return Resolution::new(SecretStatus::Locked, "1Password needs sign-in or approval");
    }
    if MISSING.iter().any(|needle| lower.contains(needle)) {
        return Resolution::missing("1Password item, field or Environment variable not found");
    }
    let first = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no error message");
    let first: String = first.chars().take(200).collect();
    Resolution::error(format!("1Password CLI failed ({}): {first}", output.status))
}

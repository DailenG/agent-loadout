//! Delivery modes (SPEC 11.4): how a secret reference reaches an MCP server
//! for one client and field, and what blocks it on this machine.
//!
//! - `env`: the client expands a variable reference itself (or forwards a
//!   variable by name, Codex).
//! - `launch`: the client starts `loadout exec`, which resolves the
//!   references in its argv and starts the server with the values in its
//!   environment (SPEC 11.5). Stdio `env` values only.
//! - `command`: the client runs a Loadout command at connect time (omp and
//!   Pi `!command`, Claude Code `headersHelper`).
//! - `inline`: Loadout writes the value into the client's file. Explicit
//!   per server, or the fallback for `${env:..}` the client cannot get
//!   otherwise; never into a shared file.
//!
//! [`Policy::choose`] implements the `auto` rules and checks an explicit
//! `delivery.mode`. Planning never reads a 1Password value (that may
//! prompt): a `launch` or `command` entry is blocked only when the
//! provider is not usable here ([`check_available`]).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::adapters::caps::{Caps, CommandRef, EnvRefSyntax, FieldSet};
use crate::brand;
use crate::env::Env;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::server::{DeliveryMode, Transport};
use crate::secrets::catalog::{CATALOG_FILE, Catalog};
use crate::secrets::grammar::SecretRef;
use crate::secrets::providers::keyring::KeyringBackend;
use crate::secrets::providers::onepassword::{OpCli, OpFeature};
use crate::secrets::scanner::SecretMemory;
use crate::secrets::status::SecretStatus;

/// The variable `op` reads a service account token from.
pub const SERVICE_ACCOUNT_VAR: &str = "OP_SERVICE_ACCOUNT_TOKEN";
/// Keyring account of the service account token (service
/// [`brand::KEYRING_SERVICE`]): `keyring://loadout/OP_SERVICE_ACCOUNT_TOKEN`.
pub const SERVICE_ACCOUNT_KEYRING_ACCOUNT: &str = "OP_SERVICE_ACCOUNT_TOKEN";

/// A delivery mode, ordered so that the most consequential mode of an entry
/// is the greatest (the one an entry reports).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mode {
    Env,
    Command,
    Launch,
    Inline,
}

impl Mode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::Env => "env",
            Mode::Command => "command",
            Mode::Launch => "launch",
            Mode::Inline => "inline",
        }
    }
}

/// Whether a target file is private to this user or shared (tracked by a
/// project's git repository, or flagged shared).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sharing {
    /// User-scope files and private project files.
    Private,
    /// Shared project files: no `inline` values, and the launcher is the
    /// bare `loadout` so the file stays portable (SPEC 11.5).
    Shared,
}

/// Where the value of a template reference comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `${env:NAME}`, or an alias whose reference is `env:NAME`: a variable
    /// of the environment the client (or Loadout) runs in.
    Env { name: String },
    /// An alias with a provider reference (`op://`, `op-env://`,
    /// `keyring://`).
    Provider {
        alias: String,
        reference: SecretRef,
        /// The variable `env` delivery uses (`env_name`, else the alias).
        env_name: String,
    },
}

impl Source {
    /// The variable `env` delivery spells.
    pub fn variable(&self) -> &str {
        match self {
            Source::Env { name } => name,
            Source::Provider { env_name, .. } => env_name,
        }
    }

    /// The reference `loadout exec` and `loadout secret get` resolve.
    pub fn reference(&self) -> SecretRef {
        match self {
            Source::Env { name } => SecretRef::Env { name: name.clone() },
            Source::Provider { reference, .. } => reference.clone(),
        }
    }

    pub fn is_provider(&self) -> bool {
        matches!(self, Source::Provider { .. })
    }

    /// How messages name it: the alias, or the variable.
    pub fn label(&self) -> &str {
        match self {
            Source::Env { name } => name,
            Source::Provider { alias, .. } => alias,
        }
    }
}

/// The source of `${secret:ALIAS}` on this machine. An alias no catalog
/// defines is `missing_secret` (SPEC 11.9).
pub fn source_of_alias(catalog: &Catalog, alias: &str) -> Result<Source, CoreError> {
    let entry = catalog.get(alias).ok_or_else(|| {
        CoreError::new(
            ErrorCode::MissingSecret,
            format!(
                "{alias} is not defined in {CATALOG_FILE}; set it with `{} secrets set {alias} ...`",
                brand::BIN_NAME
            ),
        )
    })?;
    Ok(match entry.parsed()? {
        SecretRef::Env { name } => Source::Env { name },
        reference => Source::Provider {
            alias: alias.to_owned(),
            reference,
            env_name: entry.env_name().to_owned(),
        },
    })
}

/// One value that holds references: the field it is in and its sources.
#[derive(Debug, Clone, Copy)]
pub struct Slot<'a> {
    pub field: FieldSet,
    pub transport: Transport,
    /// The value is exactly one reference, without literal text around it.
    pub whole: bool,
    /// The client forwards the variable by name here (Codex `env_vars`
    /// under the same key, `bearer_token_env_var`, `env_http_headers`).
    pub forwardable: bool,
    pub sources: &'a [Source],
}

/// What decides the mode of a value: the server's `delivery.mode`, the
/// client, the target and this machine's choices.
#[derive(Debug, Clone, Copy)]
pub struct Policy<'a> {
    pub requested: DeliveryMode,
    pub caps: &'a Caps,
    /// The client's display name, for messages.
    pub client: &'a str,
    pub sharing: Sharing,
    /// `[delivery] export_provider_secrets` of the machine overlay.
    pub export_provider_secrets: bool,
}

impl Policy<'_> {
    /// The client's spelling of a variable reference in `field`, when it
    /// expands references there itself.
    pub fn env_syntax(&self, field: FieldSet) -> Option<EnvRefSyntax> {
        self.caps
            .env_ref
            .filter(|syntax| *syntax != EnvRefSyntax::CodexForward)
            .filter(|_| field != FieldSet::NONE && self.caps.env_ref_fields.contains(field))
    }

    /// Whether the client forwards variables by name (Codex).
    pub fn forwards(&self) -> bool {
        self.caps.env_ref == Some(EnvRefSyntax::CodexForward)
    }

    /// Whether the client can get this value by running a command.
    fn command_ok(&self, slot: &Slot<'_>) -> bool {
        match self.caps.command_ref {
            Some(CommandRef::Bang) => matches!(slot.field, FieldSet::ENV | FieldSet::HEADERS),
            Some(CommandRef::HeadersHelper) => {
                slot.field == FieldSet::HEADERS && slot.transport != Transport::Stdio
            }
            None => false,
        }
    }

    /// The mode for one value (SPEC 11.4 `auto` rules, or the explicit
    /// mode when it can deliver the value).
    pub fn choose(&self, slot: &Slot<'_>) -> Result<Mode, CoreError> {
        let env_ok = self.env_syntax(slot.field).is_some() || slot.forwardable;
        let launch_ok =
            slot.transport == Transport::Stdio && slot.field == FieldSet::ENV && slot.whole;
        let command_ok = self.command_ok(slot);
        let provider = slot.sources.iter().any(Source::is_provider);
        let field = field_name(slot.field);
        let unavailable = |mode: &str, why: &str| {
            Err(CoreError::new(
                ErrorCode::DeliveryUnavailable,
                format!(
                    "`{mode}` delivery cannot bring a reference in `{field}` to {}: {why}",
                    self.client
                ),
            ))
        };
        match self.requested {
            DeliveryMode::Env if env_ok => Ok(Mode::Env),
            DeliveryMode::Env => unavailable(
                "env",
                if self.forwards() {
                    "it forwards whole variables under their own name only"
                } else {
                    "it does not expand variables in this field"
                },
            ),
            DeliveryMode::Launch if launch_ok => Ok(Mode::Launch),
            DeliveryMode::Launch => unavailable(
                "launch",
                "`loadout exec` sets whole `env` values of stdio servers only",
            ),
            DeliveryMode::Command if command_ok => Ok(Mode::Command),
            DeliveryMode::Command => unavailable("command", "it runs no command for this field"),
            DeliveryMode::Inline => self.inline(),
            DeliveryMode::Auto if !provider => {
                if env_ok {
                    Ok(Mode::Env)
                } else if launch_ok {
                    Ok(Mode::Launch)
                } else {
                    self.inline()
                }
            }
            DeliveryMode::Auto => {
                if launch_ok {
                    Ok(Mode::Launch)
                } else if command_ok && slot.transport != Transport::Stdio {
                    Ok(Mode::Command)
                } else if (slot.forwardable && slot.field == FieldSet::HEADERS)
                    || (self.export_provider_secrets && env_ok)
                {
                    // Codex forwards header variables by name (SPEC 11.4
                    // rule 2); elsewhere `env` needs the user's export.
                    Ok(Mode::Env)
                } else {
                    let labels: Vec<&str> = slot
                        .sources
                        .iter()
                        .filter(|source| source.is_provider())
                        .map(Source::label)
                        .collect();
                    Err(CoreError::new(
                        ErrorCode::NoSecretFreeDelivery,
                        format!(
                            "{} cannot get {} in `{field}` without the value in its config file; \
                             fixes: export it to the environment (`{bin} env --export`, then \
                             `[delivery] export_provider_secrets = true` in this machine's overlay), \
                             or allow inline for this server (`[delivery] mode = \"inline\"`)",
                            self.client,
                            labels.join(", "),
                            bin = brand::BIN_NAME,
                        ),
                    ))
                }
            }
        }
    }

    fn inline(&self) -> Result<Mode, CoreError> {
        match self.sharing {
            Sharing::Private => Ok(Mode::Inline),
            Sharing::Shared => Err(CoreError::new(
                ErrorCode::SecretInSharedFile,
                "inline delivery would write a secret value into a shared file; use `env` or \
                 `launch` delivery, or move the server to the project-local layer",
            )),
        }
    }
}

/// The name of a field in messages.
pub fn field_name(field: FieldSet) -> &'static str {
    match field {
        FieldSet::COMMAND => "command",
        FieldSet::ARGS => "args",
        FieldSet::ENV => "env",
        FieldSet::CWD => "cwd",
        FieldSet::URL => "url",
        FieldSet::HEADERS => "headers",
        _ => "client fields",
    }
}

/// What delivery needs from this machine: the launcher path, the keyring
/// and the 1Password CLI (detected once, on first use).
pub struct DeliveryEnv {
    launcher: Option<PathBuf>,
    keyring: Arc<dyn KeyringBackend>,
    op: OnceLock<OpCli>,
}

impl DeliveryEnv {
    /// `launcher` is the absolute path of the installed `loadout` binary
    /// (as the target machine spells it), `None` when it cannot be
    /// resolved.
    pub fn new(launcher: Option<PathBuf>, keyring: Arc<dyn KeyringBackend>) -> Self {
        Self {
            launcher,
            keyring,
            op: OnceLock::new(),
        }
    }

    /// Uses `op` instead of detecting the CLI.
    pub fn with_op(self, op: OpCli) -> Self {
        let _ = self.op.set(op);
        self
    }

    pub fn launcher_path(&self) -> Option<&Path> {
        self.launcher.as_deref()
    }

    pub fn keyring(&self) -> &dyn KeyringBackend {
        self.keyring.as_ref()
    }

    /// The 1Password CLI of this machine (`op --version` runs once).
    pub fn op(&self, env: &Env, fs: &dyn Fs) -> &OpCli {
        self.op.get_or_init(|| OpCli::detect(env, fs))
    }

    /// How a target starts `loadout`: the absolute path for private files,
    /// the bare name for shared ones (SPEC 11.5).
    pub fn launcher(&self, sharing: Sharing) -> Result<String, CoreError> {
        match (sharing, &self.launcher) {
            (Sharing::Shared, _) => Ok(brand::BIN_NAME.to_owned()),
            (Sharing::Private, Some(path)) => Ok(path.to_string_lossy().into_owned()),
            (Sharing::Private, None) => Err(CoreError::new(
                ErrorCode::LauncherNotFound,
                format!(
                    "the path of the `{}` binary is not known, so the server cannot be started \
                     through `{} exec`",
                    brand::BIN_NAME,
                    brand::BIN_NAME
                ),
            )),
        }
    }
}

impl std::fmt::Debug for DeliveryEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeliveryEnv")
            .field("launcher", &self.launcher)
            .field("keyring", &self.keyring.name())
            .field("op", &self.op.get())
            .finish()
    }
}

/// `env` for an `op` child: with the service account token from the OS
/// keyring (SPEC 11.5) unless `env` already has one. An unreadable keyring
/// means no token.
pub fn op_env(env: &Env, keyring: &dyn KeyringBackend) -> Env {
    if env.var(SERVICE_ACCOUNT_VAR).is_some() {
        return env.clone();
    }
    match keyring.get(brand::KEYRING_SERVICE, SERVICE_ACCOUNT_KEYRING_ACCOUNT) {
        Ok(Some(token)) => env.with_var(SERVICE_ACCOUNT_VAR, token.expose()),
        _ => env.clone(),
    }
}

/// Whether `source` can be delivered when the server starts (`launch`,
/// `command`): the provider is installed and new enough, and a keyring
/// entry exists (its value is recorded for the leak scanner, never kept).
/// A 1Password value is never read here: that may prompt (SPEC 11.11), so
/// `exec` reports a missing item when the server starts (exit 77).
pub fn check_available(
    source: &Source,
    env: &Env,
    fs: &dyn Fs,
    delivery: &DeliveryEnv,
    memory: &SecretMemory,
) -> Result<(), CoreError> {
    let Source::Provider {
        alias, reference, ..
    } = source
    else {
        // Read from the client's environment when the server starts.
        return Ok(());
    };
    let feature = match reference {
        SecretRef::OnePassword { .. } => OpFeature::ItemRead,
        SecretRef::OnePasswordEnvironment { .. } => OpFeature::Environments,
        SecretRef::Keyring { service, account } => {
            return match delivery.keyring().get(service, account) {
                Ok(Some(value)) => {
                    memory.record(value.expose());
                    Ok(())
                }
                Ok(None) => Err(CoreError::new(
                    ErrorCode::MissingSecret,
                    format!(
                        "{alias}: no {} entry for {reference}; store one with \
                         `{} secrets set {alias} --keyring`",
                        delivery.keyring().name(),
                        brand::BIN_NAME
                    ),
                )),
                Err(resolution) if resolution.status == SecretStatus::ProviderUnavailable => {
                    Err(resolution.into_error(alias))
                }
                // Locked or failing now: the server start tries again.
                Err(_) => Ok(()),
            };
        }
        SecretRef::Env { .. } => return Ok(()),
    };
    delivery
        .op(env, fs)
        .require(feature)
        .map(|_| ())
        .map_err(|resolution| {
            let not_found = resolution.status == SecretStatus::ProviderUnavailable;
            let err = resolution.into_error(alias);
            if !not_found {
                return err;
            }
            CoreError::new(
                err.code(),
                format!(
                    "{}; install it or map {alias} to keyring/env for this machine (`{} secrets \
                     set {alias} --this-machine --keyring` or `--ref env:NAME`)",
                    err.message(),
                    brand::BIN_NAME
                ),
            )
        })
}

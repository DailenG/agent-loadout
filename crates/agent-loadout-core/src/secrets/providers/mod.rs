//! Secret providers (SPEC 11.2, 11.11): the process environment, the OS
//! keyring and the 1Password CLI. A [`Resolver`] dispatches a reference to
//! its provider and records every value it hands out in the process's
//! [`SecretMemory`], so the leak scanner can refuse a commit that holds it.

pub mod env;
pub mod keyring;
pub mod onepassword;

use std::sync::Arc;

use crate::env::Env;
use crate::secrets::SecretValue;
use crate::secrets::grammar::SecretRef;
use crate::secrets::providers::keyring::KeyringBackend;
use crate::secrets::providers::onepassword::{OpCli, OpFeature};
use crate::secrets::scanner::SecretMemory;
use crate::secrets::status::{Resolution, SecretStatus};

/// Which provider resolves a reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    /// `env:NAME`
    Env,
    /// `keyring://service/account`
    Keyring,
    /// `op://...` and `op-env://...`
    OnePassword,
}

impl ProviderKind {
    /// Stable `snake_case` name.
    pub const fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Env => "env",
            ProviderKind::Keyring => "keyring",
            ProviderKind::OnePassword => "onepassword",
        }
    }
}

/// Whether a provider can be used on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderState {
    Available,
    /// Not installed or not running ("1Password CLI not found").
    Unavailable,
    /// Installed, but its version could not be read.
    Error,
}

impl ProviderState {
    pub const fn as_str(self) -> &'static str {
        match self {
            ProviderState::Available => "available",
            ProviderState::Unavailable => "unavailable",
            ProviderState::Error => "error",
        }
    }
}

/// One provider as `loadout secrets providers` reports it. Detection only:
/// nothing here signs in or prompts (SPEC 11.11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInfo {
    pub kind: ProviderKind,
    pub state: ProviderState,
    /// English detail ("1Password CLI not found", "Windows Credential
    /// Manager", the `op` path).
    pub detail: String,
    pub version: Option<String>,
    /// Version-gated features and whether this machine has them.
    pub features: Vec<(OpFeature, bool)>,
}

/// The providers of this machine, by kind.
pub fn describe(keyring: &dyn KeyringBackend, op: &OpCli) -> Vec<ProviderInfo> {
    vec![
        ProviderInfo {
            kind: ProviderKind::Env,
            state: ProviderState::Available,
            detail: "process environment".to_owned(),
            version: None,
            features: Vec::new(),
        },
        ProviderInfo {
            kind: ProviderKind::Keyring,
            state: ProviderState::Available,
            detail: keyring.name().to_owned(),
            version: None,
            features: Vec::new(),
        },
        op.info(),
    ]
}

/// Resolves references with this machine's providers.
pub struct Resolver<'a> {
    env: &'a Env,
    keyring: &'a dyn KeyringBackend,
    op: &'a OpCli,
    memory: Arc<SecretMemory>,
}

impl<'a> Resolver<'a> {
    /// `memory` is the process's [`SecretMemory`] (`Store::secret_memory`).
    pub fn new(
        env: &'a Env,
        keyring: &'a dyn KeyringBackend,
        op: &'a OpCli,
        memory: Arc<SecretMemory>,
    ) -> Self {
        Self {
            env,
            keyring,
            op,
            memory,
        }
    }

    /// The value of `reference`. 1Password Environment variables have no
    /// value here: `op run --environment` hands them to the server process
    /// at start (SPEC 11.5); Loadout never calls `op environment read`.
    pub fn resolve(&self, reference: &SecretRef) -> Result<SecretValue, Resolution> {
        let value = match reference {
            SecretRef::Env { name } => env::resolve(self.env, name),
            SecretRef::Keyring { service, account } => {
                keyring::resolve(self.keyring, service, account)
            }
            SecretRef::OnePassword { .. } => self.op.read(self.env, &reference.to_string()),
            SecretRef::OnePasswordEnvironment { .. } => {
                self.op.require(OpFeature::Environments)?;
                Err(Resolution::new(
                    SecretStatus::Unverifiable,
                    "1Password Environment variables are read by `op run --environment` when a \
                     server starts, never by Loadout itself",
                ))
            }
        }?;
        self.memory.record(value.expose());
        Ok(value)
    }

    /// The status of `reference`; the value, if fetched, is dropped. May
    /// prompt (1Password approval), so only on an explicit check.
    pub fn check(&self, reference: &SecretRef) -> Resolution {
        match reference {
            SecretRef::OnePasswordEnvironment { environment, name } => self
                .op
                .check_environment_variable(self.env, environment, name),
            _ => match self.resolve(reference) {
                Ok(_) => Resolution::resolved(),
                Err(resolution) => resolution,
            },
        }
    }

    /// Stores `value` in the OS keyring and remembers it for the scanner.
    pub fn set_keyring(
        &self,
        service: &str,
        account: &str,
        value: &SecretValue,
    ) -> Result<(), Resolution> {
        self.memory.record(value.expose());
        self.keyring.set(service, account, value)
    }

    /// Deletes a keyring entry; `false` when there was none.
    pub fn delete_keyring(&self, service: &str, account: &str) -> Result<bool, Resolution> {
        self.keyring.delete(service, account)
    }
}

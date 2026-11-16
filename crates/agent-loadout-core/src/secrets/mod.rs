//! Secrets (SPEC 11): references, providers and the leak scanner.
//!
//! - [`grammar`]: templates (`${secret:ALIAS}`, ...) and provider
//!   references (`op://`, `op-env://`, `keyring://`, `env:`).
//! - [`catalog`]: `secrets.toml` with this machine's overlay applied.
//! - [`providers`]: env, OS keyring and the 1Password CLI; [`status`] is
//!   what resolving found.
//! - [`classify`] and [`scanner`]: the SPEC 11.7 classifier and the leak
//!   scanner every store commit passes through.
//! - [`delivery`] and [`render`]: how references reach a server for one
//!   client (SPEC 11.4: `env`, `launch`, `command`, `inline`).
//!
//! Values exist only in memory, inside a [`SecretValue`]: it has no
//! `Display`, no `Serialize`, and its `Debug` is redacted, so a value
//! cannot end up in a store file, a log line or an error by accident.
//! Every value a provider hands out is recorded (as a keyed hash) in the
//! process's [`scanner::SecretMemory`], so a later commit that holds it is
//! refused.

pub mod catalog;
pub mod classify;
pub mod delivery;
pub mod grammar;
pub mod providers;
pub mod render;
pub mod scanner;
pub mod status;

use std::fmt;

/// A resolved secret value. Read it with [`SecretValue::expose`] at the
/// one place that hands it on (a child process environment, the keyring).
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(<redacted>)")
    }
}

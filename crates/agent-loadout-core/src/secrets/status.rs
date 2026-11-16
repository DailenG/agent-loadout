//! Resolution statuses (SPEC 11.3).

use std::fmt;

use crate::error::{CoreError, ErrorCode};

/// What resolving a reference found. Never carries the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretStatus {
    /// The value is available.
    Resolved,
    /// Env var unset, keyring entry absent, 1Password item or Environment
    /// variable not found, or an alias no catalog defines.
    Missing,
    /// 1Password needs sign-in or approval (or the keyring is locked).
    Locked,
    /// The provider is not on this machine: "1Password CLI not found", no
    /// Secret Service on Linux.
    ProviderUnavailable,
    /// The provider is below the version a feature needs (SPEC 11.11).
    ProviderTooOld,
    /// Cannot be checked from here: the value may exist where it is used
    /// (env delivery, or an Environment variable on a CLI that cannot list
    /// Environment names).
    Unverifiable,
    /// Anything else (a malformed reference, an unexpected provider error).
    Error,
}

impl SecretStatus {
    /// Stable `snake_case` name for CLI and IPC output.
    pub const fn as_str(self) -> &'static str {
        match self {
            SecretStatus::Resolved => "resolved",
            SecretStatus::Missing => "missing",
            SecretStatus::Locked => "locked",
            SecretStatus::ProviderUnavailable => "provider_unavailable",
            SecretStatus::ProviderTooOld => "provider_too_old",
            SecretStatus::Unverifiable => "unverifiable",
            SecretStatus::Error => "error",
        }
    }
}

impl fmt::Display for SecretStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A status with English detail. The detail never holds a value: provider
/// output is reduced to a fixed message before it gets here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub status: SecretStatus,
    pub message: Option<String>,
}

impl Resolution {
    pub fn new(status: SecretStatus, message: impl Into<String>) -> Self {
        Self {
            status,
            message: Some(message.into()),
        }
    }

    pub fn resolved() -> Self {
        Self {
            status: SecretStatus::Resolved,
            message: None,
        }
    }

    pub fn missing(message: impl Into<String>) -> Self {
        Self::new(SecretStatus::Missing, message)
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(SecretStatus::Error, message)
    }

    /// The error a command reports when it needed the value: `context`
    /// names what was being resolved.
    pub fn into_error(self, context: &str) -> CoreError {
        let code = match self.status {
            SecretStatus::Missing => ErrorCode::MissingSecret,
            SecretStatus::Locked => ErrorCode::SecretLocked,
            SecretStatus::ProviderUnavailable => ErrorCode::ProviderUnavailable,
            SecretStatus::ProviderTooOld => ErrorCode::ProviderTooOld,
            SecretStatus::Resolved | SecretStatus::Unverifiable | SecretStatus::Error => {
                ErrorCode::SecretError
            }
        };
        CoreError::new(code, format!("{context}: {self}"))
    }
}

impl fmt::Display for Resolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.message {
            Some(message) => write!(f, "{}: {message}", self.status),
            None => f.write_str(self.status.as_str()),
        }
    }
}

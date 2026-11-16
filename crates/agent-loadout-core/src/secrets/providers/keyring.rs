//! `keyring://<service>/<account>`: the OS keyring through the `keyring`
//! crate (Windows Credential Manager, macOS Keychain, Secret Service on
//! Linux). [`KeyringBackend`] is the seam: the CLI and the app use
//! [`OsKeyring`]; tests use the in-memory backend of the testkit and never
//! touch the real keyring.

use crate::secrets::SecretValue;
use crate::secrets::status::{Resolution, SecretStatus};

/// A credential store.
pub trait KeyringBackend: Send + Sync {
    /// What the store is called on this OS ("Windows Credential Manager").
    fn name(&self) -> &str;

    /// The entry, or `None` when there is none.
    fn get(&self, service: &str, account: &str) -> Result<Option<SecretValue>, Resolution>;

    fn set(&self, service: &str, account: &str, value: &SecretValue) -> Result<(), Resolution>;

    /// Removes the entry; `false` when there was none.
    fn delete(&self, service: &str, account: &str) -> Result<bool, Resolution>;
}

/// Resolves a keyring reference: an absent entry is `missing`.
pub fn resolve(
    backend: &dyn KeyringBackend,
    service: &str,
    account: &str,
) -> Result<SecretValue, Resolution> {
    backend.get(service, account)?.ok_or_else(|| {
        Resolution::missing(format!(
            "no {} entry for service {service:?}, account {account:?}",
            backend.name()
        ))
    })
}

/// The OS keyring.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsKeyring;

impl OsKeyring {
    fn entry(&self, service: &str, account: &str) -> Result<::keyring::Entry, Resolution> {
        ::keyring::Entry::new(service, account).map_err(|err| self.map_error(&err))
    }

    fn map_error(&self, err: &::keyring::Error) -> Resolution {
        use ::keyring::Error;
        match err {
            Error::NoDefaultStore => Resolution::new(
                SecretStatus::ProviderUnavailable,
                format!("{} is not available on this machine", self.name()),
            ),
            Error::NoStorageAccess(_) => Resolution::new(
                SecretStatus::Locked,
                format!("{} is locked or denied access", self.name()),
            ),
            Error::BadEncoding(_) => Resolution::error("the keyring entry is not UTF-8 text"),
            other => Resolution::error(format!("{}: {other}", self.name())),
        }
    }
}

impl KeyringBackend for OsKeyring {
    fn name(&self) -> &str {
        if cfg!(windows) {
            "Windows Credential Manager"
        } else if cfg!(target_os = "macos") {
            "macOS Keychain"
        } else {
            "Secret Service"
        }
    }

    fn get(&self, service: &str, account: &str) -> Result<Option<SecretValue>, Resolution> {
        match self.entry(service, account)?.get_password() {
            Ok(value) => Ok(Some(SecretValue::new(value))),
            Err(::keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(self.map_error(&err)),
        }
    }

    fn set(&self, service: &str, account: &str, value: &SecretValue) -> Result<(), Resolution> {
        self.entry(service, account)?
            .set_password(value.expose())
            .map_err(|err| self.map_error(&err))
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool, Resolution> {
        match self.entry(service, account)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(::keyring::Error::NoEntry) => Ok(false),
            Err(err) => Err(self.map_error(&err)),
        }
    }
}

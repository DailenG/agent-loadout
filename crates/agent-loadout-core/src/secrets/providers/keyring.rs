//! `keyring://<service>/<account>`: the OS keyring through the `keyring`
//! crate (Windows Credential Manager, macOS Keychain, Secret Service on
//! Linux). [`KeyringBackend`] is the seam: the CLI and the app use
//! [`OsKeyring`]; tests use the in-memory backend of the testkit and never
//! touch the real keyring.
//!
//! Linux: the `keyring` crate unlocks locked Secret Service items, which
//! shows an unlock prompt and waits for it; where no prompt can appear
//! (SSH, WSL, a service) the wait never ends (gate V1 FB-6). So a locked
//! entry, or a locked collection to write into, is reported as `locked`
//! first, and Loadout never asks for an unlock.

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

    #[cfg(target_os = "linux")]
    fn locked(&self) -> Resolution {
        Resolution::new(
            SecretStatus::Locked,
            format!(
                "{} is locked; unlock the keyring (for example by logging in to the desktop \
                 session) and try again",
                self.name()
            ),
        )
    }

    /// `locked` when reading the entry would need an unlock.
    fn refuse_locked(&self, service: &str, account: &str) -> Result<(), Resolution> {
        #[cfg(target_os = "linux")]
        if secret_service_lock::entry_locked(service, account) {
            return Err(self.locked());
        }
        let _ = (service, account);
        Ok(())
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

/// Lock state from the Secret Service itself, without unlocking anything.
/// When the service cannot be asked, the `keyring` crate's own error
/// (`provider_unavailable`) follows.
#[cfg(target_os = "linux")]
mod secret_service_lock {
    use std::collections::HashMap;

    use secret_service::EncryptionType;
    use secret_service::blocking::SecretService;

    /// Whether an item of this service and account (the attributes the
    /// `keyring` crate stores) is locked.
    pub(super) fn entry_locked(service: &str, account: &str) -> bool {
        let Ok(ss) = SecretService::connect(EncryptionType::Plain) else {
            return false;
        };
        let attributes = HashMap::from([("service", service), ("username", account)]);
        ss.search_items(attributes)
            .is_ok_and(|found| !found.locked.is_empty())
    }

    /// Whether the default collection, where new entries go, is locked.
    pub(super) fn default_collection_locked() -> bool {
        let Ok(ss) = SecretService::connect(EncryptionType::Plain) else {
            return false;
        };
        ss.get_default_collection()
            .and_then(|collection| collection.is_locked())
            .unwrap_or(false)
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
        self.refuse_locked(service, account)?;
        match self.entry(service, account)?.get_password() {
            Ok(value) => Ok(Some(SecretValue::new(value))),
            Err(::keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(self.map_error(&err)),
        }
    }

    fn set(&self, service: &str, account: &str, value: &SecretValue) -> Result<(), Resolution> {
        self.refuse_locked(service, account)?;
        #[cfg(target_os = "linux")]
        if secret_service_lock::default_collection_locked() {
            return Err(self.locked());
        }
        self.entry(service, account)?
            .set_password(value.expose())
            .map_err(|err| self.map_error(&err))
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool, Resolution> {
        self.refuse_locked(service, account)?;
        match self.entry(service, account)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(::keyring::Error::NoEntry) => Ok(false),
            Err(err) => Err(self.map_error(&err)),
        }
    }
}

//! In-memory keyring for tests (SPEC 19 item 7): tests never touch the
//! real OS keyring.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use agent_loadout_core::secrets::SecretValue;
use agent_loadout_core::secrets::providers::keyring::KeyringBackend;
use agent_loadout_core::secrets::status::{Resolution, SecretStatus};

/// How the fake keyring behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemKeyringMode {
    #[default]
    Available,
    /// Every call is `locked` (a locked keychain or a denied prompt).
    Locked,
    /// Every call is `provider_unavailable` (no Secret Service).
    Unavailable,
}

/// A keyring held in a map, keyed by `(service, account)`.
#[derive(Debug, Default)]
pub struct MemKeyring {
    entries: Mutex<BTreeMap<(String, String), String>>,
    mode: Mutex<MemKeyringMode>,
}

impl MemKeyring {
    pub fn new() -> Self {
        Self::default()
    }

    /// A keyring holding one entry.
    pub fn with(service: &str, account: &str, value: &str) -> Self {
        let keyring = Self::new();
        keyring.insert(service, account, value);
        keyring
    }

    pub fn insert(&self, service: &str, account: &str, value: &str) {
        self.lock()
            .insert((service.to_owned(), account.to_owned()), value.to_owned());
    }

    pub fn set_mode(&self, mode: MemKeyringMode) {
        *self.mode.lock().unwrap_or_else(PoisonError::into_inner) = mode;
    }

    /// The stored value, bypassing the mode.
    pub fn peek(&self, service: &str, account: &str) -> Option<String> {
        self.lock()
            .get(&(service.to_owned(), account.to_owned()))
            .cloned()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<(String, String), String>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn available(&self) -> Result<(), Resolution> {
        match *self.mode.lock().unwrap_or_else(PoisonError::into_inner) {
            MemKeyringMode::Available => Ok(()),
            MemKeyringMode::Locked => Err(Resolution::new(
                SecretStatus::Locked,
                "the in-memory keyring is locked",
            )),
            MemKeyringMode::Unavailable => Err(Resolution::new(
                SecretStatus::ProviderUnavailable,
                "the in-memory keyring is not available",
            )),
        }
    }
}

impl KeyringBackend for MemKeyring {
    fn name(&self) -> &str {
        "in-memory keyring"
    }

    fn get(&self, service: &str, account: &str) -> Result<Option<SecretValue>, Resolution> {
        self.available()?;
        Ok(self.peek(service, account).map(SecretValue::new))
    }

    fn set(&self, service: &str, account: &str, value: &SecretValue) -> Result<(), Resolution> {
        self.available()?;
        self.insert(service, account, value.expose());
        Ok(())
    }

    fn delete(&self, service: &str, account: &str) -> Result<bool, Resolution> {
        self.available()?;
        Ok(self
            .lock()
            .remove(&(service.to_owned(), account.to_owned()))
            .is_some())
    }
}

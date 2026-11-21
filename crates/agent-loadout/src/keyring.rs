//! The keyring the CLI uses: the OS keyring.
//!
//! Debug builds also read `LOADOUT_TEST_KEYRING`: when set to an absolute
//! path, a JSON file there (`{"<service>/<account>": "<value>"}`) stands in
//! for the keyring, so CLI tests never touch the real one (SPEC 19 item
//! 1). A release build with the variable set refuses every keyring call
//! instead, so `cargo test --release` cannot reach the real keyring
//! either.

use std::sync::Arc;

use agent_loadout_core::secrets::SecretValue;
use agent_loadout_core::secrets::providers::keyring::{KeyringBackend, OsKeyring};
use agent_loadout_core::secrets::status::Resolution;
use agent_loadout_core::{Env, brand};

/// The keyring for this process.
pub fn keyring(env: &Env) -> Arc<dyn KeyringBackend> {
    let Some(value) = env.var(brand::TEST_KEYRING_ENV_VAR) else {
        return Arc::new(OsKeyring);
    };
    #[cfg(debug_assertions)]
    {
        let path = std::path::PathBuf::from(value);
        if path.is_absolute() {
            return Arc::new(test_file::FileKeyring { path });
        }
    }
    let _ = value;
    Arc::new(Refused)
}

/// Every call fails: the test keyring variable is set where it cannot be
/// honored (a release build, or a relative path).
struct Refused;

impl Refused {
    fn refuse<T>(&self) -> Result<T, Resolution> {
        Err(Resolution::error(format!(
            "{} is set; only debug builds read it, with an absolute path, and the OS keyring \
             is not used while it is set",
            brand::TEST_KEYRING_ENV_VAR
        )))
    }
}

impl KeyringBackend for Refused {
    fn name(&self) -> &str {
        "refused keyring"
    }

    fn get(&self, _: &str, _: &str) -> Result<Option<SecretValue>, Resolution> {
        self.refuse()
    }

    fn set(&self, _: &str, _: &str, _: &SecretValue) -> Result<(), Resolution> {
        self.refuse()
    }

    fn delete(&self, _: &str, _: &str) -> Result<bool, Resolution> {
        self.refuse()
    }
}

#[cfg(debug_assertions)]
mod test_file {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use agent_loadout_core::secrets::SecretValue;
    use agent_loadout_core::secrets::providers::keyring::KeyringBackend;
    use agent_loadout_core::secrets::status::Resolution;

    /// A keyring kept in a JSON file (debug builds, tests only).
    pub struct FileKeyring {
        pub path: PathBuf,
    }

    impl FileKeyring {
        fn load(&self) -> Result<BTreeMap<String, String>, Resolution> {
            match std::fs::read_to_string(&self.path) {
                Ok(text) => serde_json::from_str(&text)
                    .map_err(|err| Resolution::error(format!("test keyring file: {err}"))),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
                Err(err) => Err(Resolution::error(format!("test keyring file: {err}"))),
            }
        }

        fn save(&self, entries: &BTreeMap<String, String>) -> Result<(), Resolution> {
            let text = serde_json::to_string_pretty(entries).expect("a map serializes");
            std::fs::write(&self.path, text)
                .map_err(|err| Resolution::error(format!("test keyring file: {err}")))
        }
    }

    fn key(service: &str, account: &str) -> String {
        format!("{service}/{account}")
    }

    impl KeyringBackend for FileKeyring {
        fn name(&self) -> &str {
            "test keyring file"
        }

        fn get(&self, service: &str, account: &str) -> Result<Option<SecretValue>, Resolution> {
            Ok(self
                .load()?
                .remove(&key(service, account))
                .map(SecretValue::new))
        }

        fn set(&self, service: &str, account: &str, value: &SecretValue) -> Result<(), Resolution> {
            let mut entries = self.load()?;
            entries.insert(key(service, account), value.expose().to_owned());
            self.save(&entries)
        }

        fn delete(&self, service: &str, account: &str) -> Result<bool, Resolution> {
            let mut entries = self.load()?;
            let removed = entries.remove(&key(service, account)).is_some();
            if removed {
                self.save(&entries)?;
            }
            Ok(removed)
        }
    }
}

//! The keyring the CLI uses: the OS keyring.
//!
//! Debug builds also read `LOADOUT_TEST_KEYRING`: when set to an absolute
//! path, a JSON file there (`{"<service>/<account>": "<value>"}`) stands in
//! for the keyring, so CLI tests, which run the debug binary, never touch
//! the real one (SPEC 19 item 1). Release builds ignore the variable.

use std::sync::Arc;

use agent_loadout_core::Env;
use agent_loadout_core::secrets::providers::keyring::{KeyringBackend, OsKeyring};

/// The keyring for this process.
pub fn keyring(env: &Env) -> Arc<dyn KeyringBackend> {
    #[cfg(debug_assertions)]
    if let Some(path) = env
        .var(test_file::VAR)
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Arc::new(test_file::FileKeyring { path });
    }
    let _ = env;
    Arc::new(OsKeyring)
}

#[cfg(debug_assertions)]
mod test_file {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use agent_loadout_core::secrets::SecretValue;
    use agent_loadout_core::secrets::providers::keyring::KeyringBackend;
    use agent_loadout_core::secrets::status::Resolution;

    pub const VAR: &str = "LOADOUT_TEST_KEYRING";

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

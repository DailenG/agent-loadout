//! `state/local-settings.toml` (SPEC 4.5): this machine's own settings,
//! never synced. Only the keys this build reads are typed; every other key
//! is kept as it is when the file is written.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::SCHEMA_VERSION;

/// The file in `state/`.
pub const LOCAL_SETTINGS_FILE: &str = "local-settings.toml";

/// What the user answered to the first run's "Add a backup remote" step
/// (SPEC 15.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackupRemoteOffer {
    /// "Not now": the store stays local-only; Settings still offers Add.
    NotNow,
    /// A remote was added from the step.
    Added,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalSettings {
    pub schema: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_remote_offer: Option<BackupRemoteOffer>,
    /// Settings > Updates switch (SPEC 16.4). Absent means on; read only
    /// by builds that carry the updater.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_for_updates: Option<bool>,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for LocalSettings {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            backup_remote_offer: None,
            check_for_updates: None,
            extra: toml::Table::new(),
        }
    }
}

impl LocalSettings {
    /// The settings of this machine; defaults when the file is absent.
    pub fn load(fs: &dyn Fs, state_dir: &Path) -> Result<Self, CoreError> {
        let path = state_dir.join(LOCAL_SETTINGS_FILE);
        if !fs.exists(&path) {
            return Ok(Self::default());
        }
        let text = fs.read_to_string(&path).map_err(|err| {
            CoreError::new(ErrorCode::IoError, format!("cannot read file: {err}")).with_path(&path)
        })?;
        let settings: Self = toml::from_str(&text).map_err(|err| {
            CoreError::new(ErrorCode::ParseError, err.message().to_owned()).with_path(&path)
        })?;
        if settings.schema != SCHEMA_VERSION {
            return Err(CoreError::new(
                ErrorCode::UnsupportedSchema,
                format!("schema {} is not read by this build", settings.schema),
            )
            .with_path(&path));
        }
        Ok(settings)
    }

    pub fn save(&self, fs: &dyn Fs, state_dir: &Path) -> Result<(), CoreError> {
        let text = toml::to_string_pretty(self).map_err(|err| {
            CoreError::new(
                ErrorCode::InvalidValue,
                format!("cannot write local settings: {err}"),
            )
        })?;
        fs.create_dir_all(state_dir).map_err(|err| {
            CoreError::new(
                ErrorCode::IoError,
                format!("cannot create directory: {err}"),
            )
            .with_path(state_dir)
        })?;
        crate::apply::write_atomic(fs, &state_dir.join(LOCAL_SETTINGS_FILE), text.as_bytes())
    }
}

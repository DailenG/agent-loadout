//! `store/settings.toml`: portable UI and behavior preferences (SPEC 5.7).
//! Machine-specific values live in `state/local-settings.toml`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::validate::Diagnostics;
use crate::model::{SCHEMA_VERSION, StoreModel};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
    System,
}

/// When edits reach client files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ApplyMode {
    /// Apply with every edit (default).
    Instant,
    /// Collect changes for an explicit apply.
    Review,
}

/// Initial enablement of newly added servers or skills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum NewItemEnable {
    AllDetected,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "Loadout portable settings (store/settings.toml)")]
pub struct Settings {
    /// Schema version; always 1.
    #[schemars(extend("const" = 1))]
    pub schema: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<Theme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apply_mode: Option<ApplyMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_server_enable: Option<NewItemEnable>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_skill_enable: Option<NewItemEnable>,
    /// Scan `~/.agents/skills` read-only (SPEC 12.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents_cli_compat: Option<bool>,
    /// List view preferences, owned by the GUI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<std::collections::BTreeMap<String, serde_json::Value>>")]
    pub list_view: Option<toml::Table>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

impl StoreModel for Settings {
    const SCHEMA_NAME: &'static str = "settings";
}

impl Settings {
    pub fn new() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            theme: None,
            apply_mode: None,
            new_server_enable: None,
            new_skill_enable: None,
            agents_cli_compat: None,
            list_view: None,
            extra: toml::Table::new(),
        }
    }

    /// Every rule is a type rule enforced on parse; nothing more to check.
    pub fn validate(&self) -> Diagnostics {
        Diagnostics::new()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::new()
    }
}

//! `store/machines/<machine-id>.toml`: per-machine overlay (SPEC 5.6).
//! Written only by the machine it describes.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::env::Os;
use crate::error::ErrorCode;
use crate::model::ids::IdKind;
use crate::model::secrets::SecretEntry;
use crate::model::server::{OS_NAMES, ServerOverride};
use crate::model::validate::{Diagnostics, join};
use crate::model::{SCHEMA_VERSION, StoreModel};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "Loadout machine overlay (store/machines/<machine-id>.toml)")]
pub struct MachineOverlay {
    /// Schema version; always 1.
    #[schemars(extend("const" = 1))]
    pub schema: i64,
    /// ULID; equals the file name.
    #[schemars(extend("pattern" = crate::model::ids::ULID_PATTERN))]
    pub machine_id: String,
    /// Human label, also the commit author name.
    pub label: String,
    /// `windows`, `linux` or `macos`.
    pub os: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable: Option<MachineEnable>,
    /// Per-machine reference overrides by alias.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<BTreeMap<String, SecretEntry>>,
    /// Per-machine field overrides for system servers, by server id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub servers: Option<BTreeMap<String, ServerOverride>>,
    /// Where each project lives on this machine, by project id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<BTreeMap<String, MachineProject>>,
    /// How secrets are delivered on this machine (SPEC 11.4, 11.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<MachineDelivery>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

impl StoreModel for MachineOverlay {
    const SCHEMA_NAME: &'static str = "machine";
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MachineEnable {
    /// Clients never written on this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_clients: Option<Vec<String>>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

/// Per-machine delivery choices.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MachineDelivery {
    /// The user exports provider secrets to the clients' environment on
    /// this machine (`loadout env --export`, SPEC 11.6), so `auto`
    /// delivery may use `env` for provider references a client cannot get
    /// otherwise (SPEC 11.4 rule 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_provider_secrets: Option<bool>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MachineProject {
    /// Absolute project folder on this machine.
    pub path: String,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

impl MachineOverlay {
    pub fn new(machine_id: impl Into<String>, label: impl Into<String>, os: Os) -> Self {
        Self {
            schema: SCHEMA_VERSION,
            machine_id: machine_id.into(),
            label: label.into(),
            os: os.as_str().to_owned(),
            enable: None,
            secrets: None,
            servers: None,
            projects: None,
            delivery: None,
            extra: toml::Table::new(),
        }
    }

    /// Validates per SPEC 5.6. `file_id` is the file name without `.toml`.
    pub fn validate(&self, file_id: Option<&str>) -> Diagnostics {
        let mut d = Diagnostics::new();
        d.check_named_id(IdKind::Machine, &self.machine_id, "machine_id", file_id);
        if self.label.trim().is_empty() {
            d.error(
                ErrorCode::InvalidValue,
                "label",
                "`label` must not be empty",
            );
        }
        let os = os_from_name(&self.os);
        if os.is_none() {
            d.warning(
                ErrorCode::UnknownOs,
                "os",
                format!(
                    "unknown OS {:?}; expected one of {OS_NAMES:?}; kept as written",
                    self.os
                ),
            );
        }
        if let Some(clients) = self
            .enable
            .as_ref()
            .and_then(|e| e.exclude_clients.as_ref())
        {
            d.check_client_ids(clients, "enable.exclude_clients");
        }
        for (alias, entry) in self.secrets.iter().flatten() {
            let path = join("secrets", alias);
            d.check_id(IdKind::SecretAlias, alias, &path);
            entry.validate_into(&path, &mut d);
        }
        for (id, over) in self.servers.iter().flatten() {
            let path = join("servers", id);
            d.check_id(IdKind::Server, id, &path);
            over.fields.check_templates(&path, &mut d);
        }
        for (id, project) in self.projects.iter().flatten() {
            let path = join("projects", id);
            d.check_id(IdKind::Project, id, &path);
            if let Some(os) = os
                && !is_absolute_on(os, &project.path)
            {
                d.error(
                    ErrorCode::InvalidPath,
                    join(&path, "path"),
                    format!("{:?} is not an absolute {} path", project.path, os.as_str()),
                );
            }
        }
        d
    }
}

fn os_from_name(name: &str) -> Option<Os> {
    [Os::Windows, Os::Linux, Os::MacOs]
        .into_iter()
        .find(|os| os.as_str() == name)
}

/// Lexical check for a path of another machine, so it does not depend on
/// the OS running this code.
fn is_absolute_on(os: Os, path: &str) -> bool {
    match os {
        Os::Windows => {
            let bytes = path.as_bytes();
            let drive = bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'\\' | b'/');
            drive || path.starts_with(r"\\") || path.starts_with("//")
        }
        Os::Linux | Os::MacOs => path.starts_with('/'),
    }
}

//! `store/secrets.toml` (SPEC 5.3). The reference grammar it uses lives in
//! [`crate::secrets::grammar`].
//!
//! The catalog holds references and metadata only, never values.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;
use crate::model::ids::IdKind;
use crate::model::validate::{Diagnostics, join};
use crate::model::{SCHEMA_VERSION, StoreModel};
use crate::secrets::grammar::is_op_environment_id;

/// `store/secrets.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "Loadout secret catalog (store/secrets.toml)")]
pub struct SecretCatalog {
    /// Schema version; always 1.
    #[schemars(extend("const" = 1))]
    pub schema: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onepassword: Option<OnePassword>,
    /// Aliases (`${secret:ALIAS}`) and the references they resolve to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<BTreeMap<String, SecretEntry>>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

impl StoreModel for SecretCatalog {
    const SCHEMA_NAME: &'static str = "secrets";
}

/// 1Password settings shared by every machine.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OnePassword {
    /// Id of the one 1Password Environment holding MCP secrets (an
    /// identifier, not a secret).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

/// One alias. Also the shape of a per-machine override (SPEC 5.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SecretEntry {
    /// Provider reference: `env:NAME`, `op://vault/item/field`,
    /// `op-env://<environment-id>/<NAME>` or `keyring://<service>/<account>`.
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Environment variable name used by env delivery (default: the alias).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_name: Option<String>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

impl SecretCatalog {
    pub fn new() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            onepassword: None,
            secrets: None,
            extra: toml::Table::new(),
        }
    }

    pub fn validate(&self) -> Diagnostics {
        let mut d = Diagnostics::new();
        if let Some(environment) = self
            .onepassword
            .as_ref()
            .and_then(|op| op.environment.as_ref())
            && !is_op_environment_id(environment)
        {
            d.error(
                ErrorCode::InvalidValue,
                "onepassword.environment",
                format!("invalid 1Password Environment id {environment:?}"),
            );
        }
        for (alias, entry) in self.secrets.iter().flatten() {
            let path = join("secrets", alias);
            d.check_id(IdKind::SecretAlias, alias, &path);
            entry.validate_into(&path, &mut d);
        }
        d
    }
}

impl Default for SecretCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretEntry {
    pub(crate) fn validate_into(&self, path: &str, d: &mut Diagnostics) {
        d.check_secret_ref(&self.reference, &join(path, "ref"));
        if let Some(env_name) = &self.env_name {
            d.check_id(IdKind::EnvVar, env_name, &join(path, "env_name"));
        }
    }
}

//! `store/secrets.toml` (SPEC 5.3) and the reference grammar (SPEC 11.2).
//!
//! The catalog holds references and metadata only, never values.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, ErrorCode};
use crate::model::ids::{self, IdKind};
use crate::model::validate::{Diagnostics, join};
use crate::model::{SCHEMA_VERSION, StoreModel};

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

/// A parsed provider reference (the `ref` of a catalog entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretRef {
    /// `env:NAME`: a process environment variable.
    Env { name: String },
    /// `op://vault/item[/section]/field[?query]`: a 1Password item field.
    OnePassword {
        vault: String,
        item: String,
        section: Option<String>,
        field: String,
        query: Option<String>,
    },
    /// `op-env://<environment-id>/<NAME>`: a 1Password Environment variable.
    OnePasswordEnvironment { environment: String, name: String },
    /// `keyring://<service>/<account>`: an OS keyring entry.
    Keyring { service: String, account: String },
}

impl SecretRef {
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        let invalid = |detail: &str| {
            Err(CoreError::new(
                ErrorCode::InvalidRef,
                format!("invalid secret reference {text:?}: {detail}"),
            ))
        };
        if let Some(name) = text.strip_prefix("env:") {
            if !ids::is_valid(IdKind::EnvVar, name) {
                return invalid("`env:` needs an environment variable name");
            }
            return Ok(Self::Env {
                name: name.to_owned(),
            });
        }
        if let Some(rest) = text.strip_prefix("op-env://") {
            let Some((environment, name)) = rest.split_once('/') else {
                return invalid("expected op-env://<environment-id>/<NAME>");
            };
            if !is_op_environment_id(environment) || !ids::is_valid(IdKind::EnvVar, name) {
                return invalid("expected op-env://<environment-id>/<NAME>");
            }
            return Ok(Self::OnePasswordEnvironment {
                environment: environment.to_owned(),
                name: name.to_owned(),
            });
        }
        if let Some(rest) = text.strip_prefix("op://") {
            let (path, query) = match rest.split_once('?') {
                Some((path, query)) => (path, Some(query)),
                None => (rest, None),
            };
            let segments: Vec<&str> = path.split('/').collect();
            if segments.iter().any(|segment| segment.trim().is_empty()) {
                return invalid("op:// segments must not be empty");
            }
            if let Some(query) = query {
                let well_formed = query.split('&').all(|pair| {
                    pair.split_once('=')
                        .is_some_and(|(key, value)| !key.is_empty() && !value.is_empty())
                });
                if !well_formed {
                    return invalid("op:// query must be key=value pairs");
                }
            }
            let query = query.map(str::to_owned);
            return match segments.as_slice() {
                [vault, item, field] => Ok(Self::OnePassword {
                    vault: (*vault).to_owned(),
                    item: (*item).to_owned(),
                    section: None,
                    field: (*field).to_owned(),
                    query,
                }),
                [vault, item, section, field] => Ok(Self::OnePassword {
                    vault: (*vault).to_owned(),
                    item: (*item).to_owned(),
                    section: Some((*section).to_owned()),
                    field: (*field).to_owned(),
                    query,
                }),
                _ => invalid("expected op://vault/item/field or op://vault/item/section/field"),
            };
        }
        if let Some(rest) = text.strip_prefix("keyring://") {
            return match rest.split_once('/') {
                Some((service, account)) if !service.is_empty() && !account.is_empty() => {
                    Ok(Self::Keyring {
                        service: service.to_owned(),
                        account: account.to_owned(),
                    })
                }
                _ => invalid("expected keyring://<service>/<account>"),
            };
        }
        invalid("expected env:, op://, op-env:// or keyring://")
    }
}

/// 1Password Environment ids are lower-case alphanumeric.
fn is_op_environment_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// A non-secret path variable usable in templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathVar {
    /// `${home}`
    Home,
    /// `${project}`
    Project,
    /// `${loadout_home}`
    LoadoutHome,
}

/// One piece of a template string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplatePart {
    /// Literal text, with `$${` already unescaped to `${`.
    Literal(String),
    /// `${env:NAME}`
    Env(String),
    /// `${secret:ALIAS}`
    Secret(String),
    /// `${home}`, `${project}`, `${loadout_home}`
    Var(PathVar),
}

/// Parses a string field of a server definition (SPEC 11.2). Provider URIs
/// are not allowed inside `${...}`; they go through a `secrets.toml` alias.
pub fn parse_template(text: &str) -> Result<Vec<TemplatePart>, CoreError> {
    let invalid = |detail: String| {
        Err(CoreError::new(
            ErrorCode::InvalidRef,
            format!("invalid template {text:?}: {detail}"),
        ))
    };
    let mut parts = Vec::new();
    let mut literal = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        if rest[..start].ends_with('$') {
            // `$${` is a literal `${`.
            literal.push_str(&rest[..start - 1]);
            literal.push_str("${");
            rest = &rest[start + 2..];
            continue;
        }
        literal.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return invalid("unterminated `${`".to_owned());
        };
        let token = &after[..end];
        let part = if let Some(name) = token.strip_prefix("env:") {
            if !ids::is_valid(IdKind::EnvVar, name) {
                return invalid(format!("`${{{token}}}` needs an environment variable name"));
            }
            TemplatePart::Env(name.to_owned())
        } else if let Some(alias) = token.strip_prefix("secret:") {
            if !ids::is_valid(IdKind::SecretAlias, alias) {
                return invalid(format!("`${{{token}}}` needs a secret alias"));
            }
            TemplatePart::Secret(alias.to_owned())
        } else {
            match token {
                "home" => TemplatePart::Var(PathVar::Home),
                "project" => TemplatePart::Var(PathVar::Project),
                "loadout_home" => TemplatePart::Var(PathVar::LoadoutHome),
                _ => {
                    return invalid(format!(
                        "unknown `${{{token}}}`; use ${{env:NAME}}, ${{secret:ALIAS}}, \
                         ${{home}}, ${{project}}, ${{loadout_home}} or $${{ for a literal"
                    ));
                }
            }
        };
        if !literal.is_empty() {
            parts.push(TemplatePart::Literal(std::mem::take(&mut literal)));
        }
        parts.push(part);
        rest = &after[end + 1..];
    }
    literal.push_str(rest);
    if !literal.is_empty() {
        parts.push(TemplatePart::Literal(literal));
    }
    Ok(parts)
}

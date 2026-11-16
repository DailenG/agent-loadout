//! Reference grammar (SPEC 11.2).
//!
//! - Templates: every string field of a server definition may hold
//!   `${env:NAME}`, `${secret:ALIAS}`, `${home}`, `${project}`,
//!   `${loadout_home}`, and `$${` for a literal `${`.
//! - Provider references: the whole-value `ref` of a `secrets.toml` alias
//!   (`env:NAME`, `op://vault/item/field`, `op-env://<id>/<NAME>`,
//!   `keyring://<service>/<account>`). They never appear inline in a
//!   template, which keeps the template parser trivial and a literal
//!   `op://` in an argument just text.

use std::fmt;

use crate::error::{CoreError, ErrorCode};
use crate::model::ids::{self, IdKind};
use crate::secrets::providers::ProviderKind;

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

    /// The provider that resolves this reference.
    pub fn provider(&self) -> ProviderKind {
        match self {
            SecretRef::Env { .. } => ProviderKind::Env,
            SecretRef::Keyring { .. } => ProviderKind::Keyring,
            SecretRef::OnePassword { .. } | SecretRef::OnePasswordEnvironment { .. } => {
                ProviderKind::OnePassword
            }
        }
    }
}

/// The canonical text, which [`SecretRef::parse`] reads back unchanged.
impl fmt::Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretRef::Env { name } => write!(f, "env:{name}"),
            SecretRef::OnePassword {
                vault,
                item,
                section,
                field,
                query,
            } => {
                write!(f, "op://{vault}/{item}/")?;
                if let Some(section) = section {
                    write!(f, "{section}/")?;
                }
                f.write_str(field)?;
                match query {
                    Some(query) => write!(f, "?{query}"),
                    None => Ok(()),
                }
            }
            SecretRef::OnePasswordEnvironment { environment, name } => {
                write!(f, "op-env://{environment}/{name}")
            }
            SecretRef::Keyring { service, account } => write!(f, "keyring://{service}/{account}"),
        }
    }
}

/// 1Password Environment ids are lower-case alphanumeric.
pub(crate) fn is_op_environment_id(value: &str) -> bool {
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

/// Whether `text` parses and holds at least one `${env:..}` or
/// `${secret:..}` reference.
pub fn holds_reference(text: &str) -> bool {
    parse_template(text).is_ok_and(|parts| {
        parts
            .iter()
            .any(|part| matches!(part, TemplatePart::Env(_) | TemplatePart::Secret(_)))
    })
}

/// The literal pieces of `text` (between references and path variables);
/// `text` itself when it does not parse. This is what a classifier looks
/// at in a value that also holds references.
pub fn literal_parts(text: &str) -> Vec<String> {
    match parse_template(text) {
        Ok(parts) => parts
            .into_iter()
            .filter_map(|part| match part {
                TemplatePart::Literal(literal) => Some(literal),
                _ => None,
            })
            .collect(),
        Err(_) => vec![text.to_owned()],
    }
}

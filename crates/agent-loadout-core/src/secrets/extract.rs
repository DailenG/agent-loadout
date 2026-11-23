//! Secret extraction on import, adopt and migration (SPEC 11.7), shared by
//! the SkillDock migration ([`crate::migrate`]) and the import of client
//! entries ([`crate::apply::import`]).
//!
//! An [`Extractor`] replaces each sensitive literal of a server definition
//! (`args`, the URL, `env` and `headers` values) by a `${secret:<alias>}`
//! reference and keeps the value in memory only ([`Extracted::value`]).
//! [`stage_choices`] turns the destinations the user chose into
//! `secrets.toml` aliases: the OS keyring (the value is written there, never
//! to the store), a 1Password reference or an environment variable.

use std::collections::{BTreeMap, BTreeSet};

use crate::apply::ApplyEnv;
use crate::brand;
use crate::error::{CoreError, ErrorCode};
use crate::model::ids::{self, IdKind};
use crate::secrets::SecretValue;
use crate::secrets::catalog::{self, AliasUpdate, SetScope};
use crate::secrets::classify::{
    Reason, classify_args_import, classify_import, find_in_text, is_sensitive_argument_name,
};
use crate::secrets::grammar::SecretRef;
use crate::secrets::providers::Resolver;
use crate::secrets::providers::onepassword::OpCli;
use crate::store::Transaction;

/// Secret aliases in use; [`Aliases::new_alias`] picks free ones.
#[derive(Debug, Clone, Default)]
pub struct Aliases {
    taken: BTreeSet<String>,
}

impl Aliases {
    pub fn new(taken: impl IntoIterator<Item = String>) -> Self {
        Self {
            taken: taken.into_iter().collect(),
        }
    }

    /// `<SERVER>_<KEY>` in upper case with `_` for anything else, free of
    /// every alias seen so far (`_2`, `_3` when taken).
    pub fn new_alias(&mut self, server: &str, key: &str) -> String {
        let mut base: String = format!("{server}_{key}")
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect();
        while base.contains("__") {
            base = base.replace("__", "_");
        }
        let mut base = base.trim_matches('_').to_owned();
        if base.is_empty() || base.as_bytes()[0].is_ascii_digit() {
            base.insert(0, '_');
        }
        base.truncate(60);
        let mut alias = base.clone();
        let mut n = 2;
        while self.taken.contains(&alias) || !ids::is_valid(IdKind::SecretAlias, &alias) {
            alias = format!("{base}_{n}");
            n += 1;
        }
        self.taken.insert(alias.clone());
        alias
    }
}

/// One plaintext value routed to secret extraction. The value is not part
/// of it: the store gets a `${secret:<alias>}` reference in its place.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Extraction {
    pub server: String,
    /// `env.NAME`, `headers.NAME`, `args[N]` (1-based) or `url`.
    pub field: String,
    pub alias: String,
    /// Why it counts as sensitive ([`Reason::as_str`]).
    pub reason: &'static str,
    /// The default destination (`keyring://loadout/<alias>`); the wizard
    /// also offers 1Password or an environment variable.
    pub suggested_ref: String,
}

/// An [`Extraction`] with the value it replaced, in memory only
/// ([`SecretValue`] has no `Display` or `Serialize`, and a redacted
/// `Debug`).
#[derive(Debug, Clone)]
pub struct Extracted {
    pub extraction: Extraction,
    pub value: SecretValue,
}

/// `${secret:<alias>}`.
pub fn reference(alias: &str) -> String {
    format!("${{secret:{alias}}}")
}

/// Replaces the sensitive literals of one server's fields by references.
pub struct Extractor<'a> {
    server: &'a str,
    aliases: &'a mut Aliases,
    found: Vec<Extracted>,
}

impl<'a> Extractor<'a> {
    /// Extracts for store server `server` (aliases are `<SERVER>_<KEY>`).
    pub fn new(server: &'a str, aliases: &'a mut Aliases) -> Self {
        Self {
            server,
            aliases,
            found: Vec::new(),
        }
    }

    /// What was extracted, in field order.
    pub fn finish(self) -> Vec<Extracted> {
        self.found
    }

    fn push(&mut self, field: String, key: &str, reason: Reason, value: &str) -> String {
        let alias = self.aliases.new_alias(self.server, key);
        self.found.push(Extracted {
            extraction: Extraction {
                server: self.server.to_owned(),
                field,
                alias: alias.clone(),
                reason: reason.as_str(),
                suggested_ref: format!("keyring://{}/{alias}", brand::KEYRING_SERVICE),
            },
            value: SecretValue::new(value),
        });
        alias
    }

    /// Arguments with each sensitive one (or the value part of
    /// `--flag=value`) replaced by a reference.
    pub fn args(&mut self, mut args: Vec<String>) -> Vec<String> {
        let flagged = classify_args_import(&args);
        for (index, reason) in flagged.into_iter().enumerate() {
            let Some(reason) = reason else {
                continue;
            };
            let arg = args[index].clone();
            let (prefix, key, value) = match arg.split_once('=') {
                Some((flag, value)) if flag.starts_with('-') => (
                    format!("{flag}="),
                    flag.trim_start_matches('-').to_owned(),
                    value.to_owned(),
                ),
                _ => {
                    let previous = index
                        .checked_sub(1)
                        .map(|i| args[i].as_str())
                        .filter(|prev| prev.starts_with('-') && is_sensitive_argument_name(prev));
                    (
                        String::new(),
                        previous.map_or_else(
                            || format!("arg{}", index + 1),
                            |flag| flag.trim_start_matches('-').to_owned(),
                        ),
                        arg.clone(),
                    )
                }
            };
            let alias = self.push(format!("args[{}]", index + 1), &key, reason, &value);
            args[index] = format!("{prefix}{}", reference(&alias));
        }
        args
    }

    /// The URL with each credential part (userinfo, token query parameter,
    /// token-shaped text) replaced by a reference. A URL the classifier
    /// flags without a part to cut out is `needs_secret_extraction` (edit
    /// it by hand).
    pub fn url(&mut self, url: String) -> Result<String, CoreError> {
        let Some(reason) = classify_import(None, &url) else {
            return Ok(url);
        };
        let hits = find_in_text(&url);
        if hits.is_empty() {
            return Err(CoreError::new(
                ErrorCode::NeedsSecretExtraction,
                format!(
                    "the URL may hold a secret ({}); edit it by hand",
                    reason.describe()
                ),
            ));
        }
        // Aliases and the listing follow URL order; the text is cut from
        // the end so earlier ranges stay valid.
        let mut aliases = Vec::with_capacity(hits.len());
        for (n, hit) in hits.iter().enumerate() {
            let key = if hits.len() == 1 {
                "url".to_owned()
            } else {
                format!("url{}", n + 1)
            };
            let value = url[hit.range.clone()].to_owned();
            aliases.push(self.push("url".to_owned(), &key, hit.reason, &value));
        }
        let mut out = url;
        for (hit, alias) in hits.iter().zip(&aliases).rev() {
            out.replace_range(hit.range.clone(), &reference(alias));
        }
        Ok(out)
    }

    /// An `env` or `headers` map (`label`) with each sensitive value
    /// replaced by a reference; an `Authorization: Bearer` style scheme in
    /// a header stays around the reference.
    pub fn map(&mut self, label: &str, map: BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for (name, value) in map {
            let value = match classify_import(Some(&name), &value) {
                Some(reason) => {
                    let scheme = value.split_once(' ').filter(|(scheme, _)| {
                        label == "headers"
                            && matches!(
                                scheme.to_ascii_lowercase().as_str(),
                                "bearer" | "basic" | "token"
                            )
                    });
                    match scheme {
                        Some((scheme, secret)) => {
                            let alias = self.push(format!("{label}.{name}"), &name, reason, secret);
                            format!("{scheme} {}", reference(&alias))
                        }
                        None => {
                            let alias = self.push(format!("{label}.{name}"), &name, reason, &value);
                            reference(&alias)
                        }
                    }
                }
                None => value,
            };
            out.insert(name, value);
        }
        out
    }
}

/// Where one extracted value goes (SPEC 11.7).
pub enum ExtractionTarget {
    /// The OS keyring, as `keyring://loadout/<alias>`; the value goes one
    /// way and is never stored or returned.
    Keyring(SecretValue),
    /// A 1Password or environment reference (`op://...`, `op-env://...`,
    /// `env:NAME`).
    Reference(SecretRef),
}

/// The destination of one extraction, by alias.
pub struct ExtractionChoice {
    pub alias: String,
    pub target: ExtractionTarget,
}

/// Stages each choice as a `secrets.toml` alias in `tx`. Keyring values
/// are written through the resolver first, which remembers them, so the
/// leak scanner refuses the commit should one reach a store file. A
/// keyring reference given instead of a value is `invalid_value`.
pub fn stage_choices(
    tx: &mut Transaction<'_>,
    ax: &ApplyEnv<'_>,
    choices: &[ExtractionChoice],
) -> Result<(), CoreError> {
    if choices.is_empty() {
        return Ok(());
    }
    if choices.iter().any(|choice| {
        matches!(
            choice.target,
            ExtractionTarget::Reference(SecretRef::Keyring { .. })
        )
    }) {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            "a keyring destination takes the value, not a reference",
        ));
    }
    let resolver = Resolver::new(
        ax.env,
        ax.delivery.keyring(),
        &OpCli::NotFound,
        tx.store().secret_memory().clone(),
    );
    for choice in choices {
        let reference = match &choice.target {
            ExtractionTarget::Keyring(value) => {
                resolver
                    .set_keyring(brand::KEYRING_SERVICE, &choice.alias, value)
                    .map_err(|resolution| {
                        resolution.into_error(&format!("cannot store {}", choice.alias))
                    })?;
                SecretRef::Keyring {
                    service: brand::KEYRING_SERVICE.to_owned(),
                    account: choice.alias.clone(),
                }
            }
            ExtractionTarget::Reference(reference) => reference.clone(),
        };
        let update = AliasUpdate {
            reference,
            description: None,
            env_name: None,
        };
        catalog::stage_set(tx, &choice.alias, &update, SetScope::Store)?;
    }
    tx.add_entity("secrets".to_owned());
    Ok(())
}

fn invalid(message: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidValue, message.to_owned())
}

/// A 1Password item reference, `op://vault/item/field` or
/// `op://vault/item/section/field`, checked: three or four non-empty parts.
pub fn onepassword_reference(text: &str) -> Result<SecretRef, CoreError> {
    let text = text.trim();
    let parts: Vec<&str> = text
        .strip_prefix("op://")
        .map(|rest| rest.split('/').collect())
        .unwrap_or_default();
    if !(3..=4).contains(&parts.len()) || parts.iter().any(|part| part.trim().is_empty()) {
        return Err(invalid("a 1Password reference is op://vault/item/field"));
    }
    match SecretRef::parse(text)? {
        reference @ SecretRef::OnePassword { .. } => Ok(reference),
        _ => Err(invalid("a 1Password reference is op://vault/item/field")),
    }
}

/// An environment variable reference (`env:NAME`), the name checked.
pub fn env_reference(name: &str) -> Result<SecretRef, CoreError> {
    let name = name.trim();
    ids::check(IdKind::EnvVar, name)?;
    SecretRef::parse(&format!("env:{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(found: &[Extracted]) -> Vec<(&str, &str, &str)> {
        found
            .iter()
            .map(|e| {
                (
                    e.extraction.field.as_str(),
                    e.extraction.alias.as_str(),
                    e.value.expose(),
                )
            })
            .collect()
    }

    #[test]
    fn extracts_each_literal_with_its_value() {
        let mut aliases = Aliases::new(["DOCS_API_KEY".to_owned()]);
        let mut extractor = Extractor::new("docs", &mut aliases);
        let args = extractor.args(vec![
            "-y".to_owned(),
            "--token=planted-arg".to_owned(),
            "--password".to_owned(),
            "planted-next".to_owned(),
        ]);
        let env = extractor.map(
            "env",
            BTreeMap::from([
                ("API_KEY".to_owned(), "planted-env".to_owned()),
                ("LOG_LEVEL".to_owned(), "info".to_owned()),
            ]),
        );
        let headers = extractor.map(
            "headers",
            BTreeMap::from([(
                "Authorization".to_owned(),
                "Bearer planted-header".to_owned(),
            )]),
        );
        let found = extractor.finish();
        assert_eq!(
            args,
            [
                "-y",
                "--token=${secret:DOCS_TOKEN}",
                "--password",
                "${secret:DOCS_PASSWORD}"
            ]
        );
        assert_eq!(env["API_KEY"], "${secret:DOCS_API_KEY_2}");
        assert_eq!(env["LOG_LEVEL"], "info");
        assert_eq!(
            headers["Authorization"],
            "Bearer ${secret:DOCS_AUTHORIZATION}"
        );
        assert_eq!(
            values(&found),
            [
                ("args[2]", "DOCS_TOKEN", "planted-arg"),
                ("args[4]", "DOCS_PASSWORD", "planted-next"),
                ("env.API_KEY", "DOCS_API_KEY_2", "planted-env"),
                (
                    "headers.Authorization",
                    "DOCS_AUTHORIZATION",
                    "planted-header"
                ),
            ]
        );
        assert!(!format!("{found:?}").contains("planted"), "{found:?}");
    }

    #[test]
    fn references_are_checked() {
        assert!(onepassword_reference("op://Vault/Item/field").is_ok());
        assert!(onepassword_reference("op://Vault/Item/section/field").is_ok());
        for bad in [
            "op://Vault/Item",
            "op://Vault//field",
            "env:X",
            "keyring://a/b",
        ] {
            assert_eq!(
                onepassword_reference(bad).unwrap_err().code(),
                ErrorCode::InvalidValue,
                "{bad}"
            );
        }
        assert_eq!(
            env_reference(" GITHUB_PAT ").unwrap(),
            SecretRef::Env {
                name: "GITHUB_PAT".to_owned()
            }
        );
        assert!(env_reference("not a name").is_err());
    }
}

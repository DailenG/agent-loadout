//! The secret catalog as this machine sees it: `secrets.toml` with the
//! `[secrets.*]` refs of `machines/<machine-id>.toml` on top (SPEC 5.3,
//! 5.6, 11.11), so one alias can resolve from 1Password on one machine and
//! from the keyring on another. References and metadata only.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::error::{CoreError, ErrorCode};
use crate::model::ids::{self, IdKind};
use crate::model::machine::MachineOverlay;
use crate::model::secrets::{SecretCatalog, SecretEntry};
use crate::model::{StoreModel, TomlDocument};
use crate::secrets::grammar::{SecretRef, TemplatePart, parse_template};
use crate::secrets::providers::Resolver;
use crate::secrets::status::{Resolution, SecretStatus};
use crate::store::{StoreRead, Transaction};

/// The catalog file at the store root.
pub const CATALOG_FILE: &str = "secrets.toml";

/// `machines/<machine-id>.toml`.
pub fn overlay_path(machine_id: &str) -> Result<String, CoreError> {
    ids::check(IdKind::Machine, machine_id)?;
    Ok(format!("machines/{machine_id}.toml"))
}

/// Where an alias's effective reference comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefSource {
    /// `secrets.toml`, shared by every machine.
    Store,
    /// This machine's overlay.
    Machine,
}

impl RefSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            RefSource::Store => "store",
            RefSource::Machine => "machine",
        }
    }
}

/// One alias with this machine's reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub alias: String,
    /// The effective reference text.
    pub reference: String,
    pub source: RefSource,
    /// The `secrets.toml` reference when the machine overrides it.
    pub store_reference: Option<String>,
    pub description: Option<String>,
    /// `env_name` as written; see [`Alias::env_name`].
    pub env_name: Option<String>,
}

impl Alias {
    pub fn parsed(&self) -> Result<SecretRef, CoreError> {
        SecretRef::parse(&self.reference)
    }

    /// The variable name env delivery uses: `env_name`, else the alias.
    pub fn env_name(&self) -> &str {
        self.env_name.as_deref().unwrap_or(&self.alias)
    }
}

/// The effective catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Catalog {
    aliases: BTreeMap<String, Alias>,
    /// `[onepassword] environment`.
    pub environment: Option<String>,
}

impl Catalog {
    /// Loads `secrets.toml` and, with `machine_id`, that machine's overlay.
    /// Either file failing validation fails the load.
    pub fn load(src: &dyn StoreRead, machine_id: Option<&str>) -> Result<Self, CoreError> {
        let mut catalog = Self::default();
        if let Some(file) = load::<SecretCatalog>(src, CATALOG_FILE)? {
            checked(file.validate().first_error(Some(Path::new(CATALOG_FILE))))?;
            catalog.environment = file.onepassword.and_then(|op| op.environment);
            for (alias, entry) in file.secrets.into_iter().flatten() {
                catalog.aliases.insert(
                    alias.clone(),
                    Alias {
                        alias,
                        reference: entry.reference,
                        source: RefSource::Store,
                        store_reference: None,
                        description: entry.description,
                        env_name: entry.env_name,
                    },
                );
            }
        }
        let Some(machine_id) = machine_id else {
            return Ok(catalog);
        };
        let relative = overlay_path(machine_id)?;
        if let Some(overlay) = load::<MachineOverlay>(src, &relative)? {
            checked(
                overlay
                    .validate(Some(machine_id))
                    .first_error(Some(Path::new(&relative))),
            )?;
            for (alias, entry) in overlay.secrets.into_iter().flatten() {
                let shared = catalog.aliases.remove(&alias);
                let store_reference = shared.as_ref().map(|shared| shared.reference.clone());
                let (description, env_name) = match shared {
                    Some(shared) => (
                        entry.description.or(shared.description),
                        entry.env_name.or(shared.env_name),
                    ),
                    None => (entry.description, entry.env_name),
                };
                catalog.aliases.insert(
                    alias.clone(),
                    Alias {
                        alias,
                        reference: entry.reference,
                        source: RefSource::Machine,
                        store_reference,
                        description,
                        env_name,
                    },
                );
            }
        }
        Ok(catalog)
    }

    pub fn get(&self, alias: &str) -> Option<&Alias> {
        self.aliases.get(alias)
    }

    /// Every alias, sorted.
    pub fn iter(&self) -> impl Iterator<Item = &Alias> {
        self.aliases.values()
    }

    pub fn is_empty(&self) -> bool {
        self.aliases.is_empty()
    }
}

/// Which file [`stage_set`] writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetScope {
    /// `secrets.toml`: every machine.
    Store,
    /// This machine's overlay only (SPEC 11.11 "use keyring on this
    /// machine").
    Machine,
}

/// What [`stage_set`] writes for an alias. `None` fields keep what the
/// entry has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasUpdate {
    pub reference: SecretRef,
    pub description: Option<String>,
    pub env_name: Option<String>,
}

/// Stages an alias's reference in `secrets.toml` or in this machine's
/// overlay (created when absent), keeping comments and unknown keys.
pub fn stage_set(
    tx: &mut Transaction<'_>,
    alias: &str,
    update: &AliasUpdate,
    scope: SetScope,
) -> Result<(), CoreError> {
    ids::check(IdKind::SecretAlias, alias)?;
    if let Some(env_name) = &update.env_name {
        ids::check(IdKind::EnvVar, env_name)?;
    }
    let merge = |entries: &mut Option<BTreeMap<String, SecretEntry>>| {
        let entries = entries.get_or_insert_with(BTreeMap::new);
        let entry = entries
            .entry(alias.to_owned())
            .or_insert_with(|| SecretEntry {
                reference: String::new(),
                description: None,
                env_name: None,
                extra: toml::Table::new(),
            });
        entry.reference = update.reference.to_string();
        if update.description.is_some() {
            entry.description.clone_from(&update.description);
        }
        if update.env_name.is_some() {
            entry.env_name.clone_from(&update.env_name);
        }
    };
    match scope {
        SetScope::Store => {
            let mut doc = load_doc::<SecretCatalog>(tx, CATALOG_FILE)?
                .unwrap_or_else(|| TomlDocument::new(SecretCatalog::new()));
            merge(&mut doc.value_mut().secrets);
            tx.write(CATALOG_FILE, doc.to_text()?)
        }
        SetScope::Machine => {
            let machine = tx.machine().clone();
            let relative = overlay_path(&machine.id)?;
            let os = tx.store().env().os();
            let mut doc = load_doc::<MachineOverlay>(tx, &relative)?.unwrap_or_else(|| {
                TomlDocument::new(MachineOverlay::new(&machine.id, &machine.label, os))
            });
            merge(&mut doc.value_mut().secrets);
            tx.write(&relative, doc.to_text()?)
        }
    }
}

/// Aliases the servers use (`${secret:ALIAS}` in any string field of
/// `servers/*.toml`, and of this machine's server overrides), each with the
/// server ids that use it. Files that do not parse are skipped; validation
/// reports them elsewhere.
pub fn usage(
    src: &dyn StoreRead,
    machine_id: Option<&str>,
) -> Result<BTreeMap<String, BTreeSet<String>>, CoreError> {
    let mut used: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for name in src.list_files("servers")? {
        let Some(id) = name.strip_suffix(".toml") else {
            continue;
        };
        let Some(text) = src.read_file(&format!("servers/{name}"))? else {
            continue;
        };
        if let Ok(table) = text.parse::<toml::Table>() {
            for alias in aliases_in(&toml::Value::Table(table)) {
                used.entry(alias).or_default().insert(id.to_owned());
            }
        }
    }
    if let Some(machine_id) = machine_id
        && let Some(text) = src.read_file(&overlay_path(machine_id)?)?
        && let Ok(table) = text.parse::<toml::Table>()
        && let Some(toml::Value::Table(servers)) = table.get("servers")
    {
        for (id, fields) in servers {
            for alias in aliases_in(fields) {
                used.entry(alias).or_default().insert(id.clone());
            }
        }
    }
    Ok(used)
}

fn aliases_in(value: &toml::Value) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            toml::Value::String(text) => {
                for part in parse_template(text).into_iter().flatten() {
                    if let TemplatePart::Secret(alias) = part {
                        found.insert(alias);
                    }
                }
            }
            toml::Value::Array(items) => stack.extend(items),
            toml::Value::Table(table) => stack.extend(table.values()),
            _ => {}
        }
    }
    found
}

/// One alias's status on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasStatus {
    pub alias: String,
    /// `None` when no catalog defines the alias.
    pub reference: Option<String>,
    pub source: Option<RefSource>,
    pub resolution: Resolution,
    /// Servers using the alias.
    pub servers: BTreeSet<String>,
}

/// Checks every alias of the catalog and every alias a server uses (or
/// only `only`, when not empty). An alias used but not defined is
/// `missing`; a reference that does not parse is `error`. May prompt
/// (1Password approval): run on explicit request only.
pub fn check(
    catalog: &Catalog,
    usage: &BTreeMap<String, BTreeSet<String>>,
    resolver: &Resolver<'_>,
    only: &[String],
) -> Vec<AliasStatus> {
    let names: BTreeSet<&str> = catalog
        .iter()
        .map(|alias| alias.alias.as_str())
        .chain(usage.keys().map(String::as_str))
        .filter(|name| only.is_empty() || only.iter().any(|wanted| wanted == name))
        .chain(only.iter().map(String::as_str))
        .collect();
    names
        .into_iter()
        .map(|name| {
            let servers = usage.get(name).cloned().unwrap_or_default();
            let Some(alias) = catalog.get(name) else {
                return AliasStatus {
                    alias: name.to_owned(),
                    reference: None,
                    source: None,
                    resolution: Resolution::missing(format!(
                        "{name} is not defined in {CATALOG_FILE}"
                    )),
                    servers,
                };
            };
            let resolution = match alias.parsed() {
                Ok(reference) => resolver.check(&reference),
                Err(err) => Resolution::new(SecretStatus::Error, err.message()),
            };
            AliasStatus {
                alias: name.to_owned(),
                reference: Some(alias.reference.clone()),
                source: Some(alias.source),
                resolution,
                servers,
            }
        })
        .collect()
}

fn load<T: StoreModel>(src: &dyn StoreRead, relative: &str) -> Result<Option<T>, CoreError> {
    Ok(load_doc(src, relative)?.map(TomlDocument::into_value))
}

fn load_doc<T: StoreModel>(
    src: &dyn StoreRead,
    relative: &str,
) -> Result<Option<TomlDocument<T>>, CoreError> {
    let Some(text) = src.read_file(relative)? else {
        return Ok(None);
    };
    TomlDocument::parse(&text)
        .map(Some)
        .map_err(|diagnostic| diagnostic.to_core_error(Some(Path::new(relative))))
}

fn checked(error: Option<CoreError>) -> Result<(), CoreError> {
    match error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// `invalid_value` for a reference that is not a keyring reference, used
/// where only keyring entries make sense (`secrets set --keyring`).
pub fn keyring_parts(reference: &SecretRef) -> Result<(&str, &str), CoreError> {
    match reference {
        SecretRef::Keyring { service, account } => Ok((service, account)),
        other => Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{other} is not a keyring reference"),
        )),
    }
}

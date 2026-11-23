//! Adopt and import (SPEC 8.6): native entries into the store.
//!
//! - [`scan_with`] lists foreign entries (no ledger record) of every
//!   client's user files and of the project files of every project this
//!   machine knows (trusted or not: reading changes nothing), grouped by
//!   name. Sources with the same store definition form one variant; a
//!   candidate with several variants is a conflict. [`scan`] reads user
//!   files only (what `adopt --all-identical` and the migration adopt).
//! - [`stage_import`] carries out an [`ImportRequest`]: one store server
//!   per chosen variant (`name`, or `name-<client>` for a second variant),
//!   with each sensitive literal extracted to the destination chosen for
//!   it. The apply that follows adopts the native entries silently when
//!   they render identically.
//! - [`stage_adopt_identical`] enables existing store servers for clients
//!   that already hold an identical entry.
//! - [`stage_resolutions`] carries out `import` decisions of a plan ("import
//!   their change", "respect the deletion").
//!
//! A read-only client (`vscode-legacy`) is a source only: what is imported
//! from it is never enabled for it, and VS Code's `${input:...}` values are
//! `needs_manual_secret`. A variant found only in project files becomes a
//! store definition without enablement (a project layer comes from the
//! project's manifests, never from a store server, SPEC 6.1); the project
//! entry stays foreign until the project's manifest names it.
//!
//! Secret extraction (SPEC 11.7): every literal in `env`, `headers`,
//! `args` and the URL goes through the classifier. An import replaces each
//! sensitive literal by `${secret:<alias>}` and needs a destination for it
//! ([`SecretChoice`]: the OS keyring, which receives the value read from the
//! client file, a 1Password reference or an environment variable);
//! without one the variant is skipped (`needs_secret_extraction`), so no
//! value that may be a secret reaches the store. Non-sensitive literals
//! (`LOG_LEVEL = "info"`) are imported as written. Other store edits of
//! native entries ([`server_from_native`]: "import their change", pasted
//! snippets) still refuse sensitive literals.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use crate::adapters::caps::FieldSet;
use crate::adapters::mapper::WriteCx;
use crate::adapters::{ADAPTERS, AdapterSpec, Target, adapter};
use crate::apply::ledger::{Ledger, LedgerSlot, container_pointer};
use crate::apply::plan::{Action, EntryPlan, EntryState, Plan, read_target};
use crate::apply::render::{DesiredState, native_name, native_to_template, render};
use crate::apply::{
    ApplyEnv, ApplyReport, ApplyRequest, Decision, OnConflict, PlanOptions, ScopeFilter, apply_in,
};
use crate::error::{CoreError, ErrorCode};
use crate::formats::{Document, Format, NativeValue};
use crate::layering::projects::{overlay_doc, unknown_project};
use crate::layering::{ProjectStatus, TrustState, TrustStore, known_projects};
use crate::model::TomlDocument;
use crate::model::applied::{ApplyTrigger, Scope};
use crate::model::clients::{ClientSelection, NATIVE_NAME_KEY};
use crate::model::ids::{self, IdKind};
use crate::model::server::{Enable, Meta, Server, ServerFields, ToolFilter};
use crate::model::settings::ApplyMode;
use crate::secrets::catalog::{Catalog, overlay_path};
use crate::secrets::classify::{classify_args_import, classify_import};
use crate::secrets::delivery::Sharing;
use crate::secrets::extract::{
    self, Aliases, Extracted, Extraction, ExtractionChoice, ExtractionTarget, Extractor,
};
use crate::secrets::grammar::SecretRef;
use crate::store::{Machine, Store, Transaction, server_path};

/// A native entry as store fields.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedDef {
    pub fields: ServerFields,
    pub tools: Option<ToolFilter>,
    /// The client's allowlisted native fields, for `[client.<id>]`.
    pub passthrough: toml::Table,
}

/// Reads a native entry of `spec`'s client as store fields: client
/// variable syntax becomes `${env:NAME}`, other `${` becomes `$${`. A
/// literal that may be a secret is `needs_secret_extraction` (import
/// extracts it instead, [`read_native`]).
pub fn server_from_native(
    spec: &AdapterSpec,
    key: &str,
    value: &NativeValue,
) -> Result<ImportedDef, CoreError> {
    let def = read_native(spec, key, value)?;
    match sensitive_field(&def.fields) {
        // Named by position or key, never by value: the message may reach
        // a UI.
        Some(what) => Err(CoreError::new(
            ErrorCode::NeedsSecretExtraction,
            format!("{what} may be a secret; it needs secret extraction before import"),
        )),
        None => Ok(def),
    }
}

/// [`server_from_native`] with sensitive literals kept as they are, for an
/// import that extracts them. The result may hold secret values: it stays
/// in memory and is never formatted into a message.
pub fn read_native(
    spec: &AdapterSpec,
    key: &str,
    value: &NativeValue,
) -> Result<ImportedDef, CoreError> {
    let mcp = spec.mcp.as_ref().ok_or_else(|| {
        CoreError::new(
            ErrorCode::ScopeNotSupported,
            format!("{} has no MCP config", spec.display_name),
        )
    })?;
    let def = mcp.mapper.from_native(key, value)?;
    if def.headers_helper.is_some() {
        return Err(CoreError::new(
            ErrorCode::UnsupportedField,
            "`headersHelper` has no store field yet",
        ));
    }
    let caps = &mcp.caps;
    let convert = |text: &str, field: FieldSet| {
        native_to_template(text, caps, field).ok_or_else(|| {
            CoreError::new(
                ErrorCode::UnsupportedField,
                format!(
                    "a value uses a {} variable or command form the store cannot express",
                    spec.display_name
                ),
            )
        })
    };
    let native = def.fields;
    if let Some(what) = prompted_input(&native) {
        return Err(CoreError::new(
            ErrorCode::NeedsManualSecret,
            format!(
                "{what} is a {} `${{input:...}}` value the editor prompts for; store the \
                 value as a secret by hand (`loadout secrets set`) and reference it",
                spec.display_name
            ),
        ));
    }
    let mut fields = ServerFields {
        transport: native.transport,
        ..ServerFields::default()
    };
    if let Some(command) = &native.command {
        fields.command = Some(convert(command, FieldSet::COMMAND)?);
    }
    if let Some(args) = &native.args {
        let args = args
            .iter()
            .map(|arg| convert(arg, FieldSet::ARGS))
            .collect::<Result<Vec<_>, _>>()?;
        fields.args = (!args.is_empty()).then_some(args);
    }
    if let Some(cwd) = &native.cwd {
        fields.cwd = Some(convert(cwd, FieldSet::CWD)?);
    }
    if let Some(url) = &native.url {
        fields.url = Some(convert(url, FieldSet::URL)?);
    }
    for (field, source, target) in [
        (FieldSet::ENV, &native.env, &mut fields.env),
        (FieldSet::HEADERS, &native.headers, &mut fields.headers),
    ] {
        let mut map = BTreeMap::new();
        for (name, value) in source.iter().flatten() {
            map.insert(name.clone(), convert(value, field)?);
        }
        *target = (!map.is_empty()).then_some(map);
    }
    let tools = (!def.tools.is_empty()).then(|| ToolFilter {
        allow: def.tools.allow.clone(),
        deny: def.tools.deny.clone(),
        extra: toml::Table::new(),
    });
    let mut passthrough = toml::Table::new();
    for (name, value) in def.passthrough.iter() {
        passthrough.insert(name.to_owned(), to_toml(value)?);
    }
    Ok(ImportedDef {
        fields,
        tools,
        passthrough,
    })
}

/// The first field holding a literal the classifier flags, by position or
/// key.
fn sensitive_field(fields: &ServerFields) -> Option<String> {
    if let Some(index) = fields
        .args
        .as_deref()
        .and_then(|args| classify_args_import(args).iter().position(Option::is_some))
    {
        return Some(format!("argument {}", index + 1));
    }
    if fields
        .url
        .as_deref()
        .is_some_and(|url| classify_import(None, url).is_some())
    {
        return Some("the URL".to_owned());
    }
    [&fields.env, &fields.headers]
        .into_iter()
        .flat_map(|map| map.iter().flatten())
        .find(|(name, value)| classify_import(Some(name), value).is_some())
        .map(|(name, _)| format!("the literal value of {name:?}"))
}

/// The field holding a VS Code input variable (`${input:id}`), a value the
/// editor prompts the user for and keeps itself, if any.
fn prompted_input(fields: &ServerFields) -> Option<String> {
    const INPUT: &str = "${input:";
    let named = |field: &str, value: &str| value.contains(INPUT).then(|| field.to_owned());
    fields
        .command
        .as_deref()
        .and_then(|value| named("`command`", value))
        .or_else(|| {
            fields
                .args
                .iter()
                .flatten()
                .find_map(|arg| arg.contains(INPUT).then(|| format!("argument {arg:?}")))
        })
        .or_else(|| {
            fields
                .cwd
                .as_deref()
                .and_then(|value| named("`cwd`", value))
        })
        .or_else(|| {
            fields
                .url
                .as_deref()
                .and_then(|value| named("the URL", value))
        })
        .or_else(|| {
            [("env", &fields.env), ("headers", &fields.headers)]
                .into_iter()
                .find_map(|(kind, map)| {
                    map.iter().flatten().find_map(|(name, value)| {
                        value
                            .contains(INPUT)
                            .then(|| format!("the {kind} value of {name:?}"))
                    })
                })
        })
}

fn to_toml(value: &NativeValue) -> Result<toml::Value, CoreError> {
    Ok(match value {
        NativeValue::Null => {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                "`null` has no TOML form",
            ));
        }
        NativeValue::Bool(b) => toml::Value::Boolean(*b),
        NativeValue::Integer(n) => toml::Value::Integer(*n),
        NativeValue::Float(n) => toml::Value::Float(*n),
        NativeValue::String(s) => toml::Value::String(s.clone()),
        NativeValue::Datetime(d) => d
            .parse()
            .map(toml::Value::Datetime)
            .unwrap_or_else(|_| toml::Value::String(d.clone())),
        NativeValue::Array(items) => {
            toml::Value::Array(items.iter().map(to_toml).collect::<Result<_, _>>()?)
        }
        NativeValue::Object(map) => toml::Value::Table(
            map.iter()
                .map(|(key, item)| Ok((key.to_owned(), to_toml(item)?)))
                .collect::<Result<_, CoreError>>()?,
        ),
    })
}

/// The known project whose files held a source.
#[derive(Debug, Clone)]
pub struct SourceProject {
    /// The overlay key.
    pub id: String,
    pub root: PathBuf,
    /// Its manifests are trusted on this machine as they are now. An
    /// untrusted project is read like any other (reading changes nothing);
    /// its manifest takes the entry only after a trust review.
    pub trusted: bool,
}

/// Where a foreign entry was found.
#[derive(Debug, Clone)]
pub struct Source {
    pub client: &'static str,
    pub scope: Scope,
    /// The project of a project file.
    pub project: Option<SourceProject>,
    /// Which of the client's files in its scope, when it has several: the
    /// profile name (omp profiles), else the file's position (`2`, `3`:
    /// a packaged mirror, an editor variant or profile). `None` for the
    /// first file.
    pub slot: Option<String>,
    pub file: PathBuf,
    /// The native entry.
    pub value: NativeValue,
    /// The entry as store fields, sensitive literals included (an import
    /// extracts them), or why it cannot be imported.
    pub def: Result<ImportedDef, CoreError>,
}

impl Source {
    /// The source within its candidate: `<client>` for a user file,
    /// `<client>@<project-id>` for a project file and
    /// `<client>@<project-id>/local` for a private project file, with
    /// `#<slot>` after the client for a client's second file in that scope
    /// (`omp#work`, `claude-desktop#2`).
    pub fn key(&self) -> String {
        source_key(
            self.client,
            self.slot.as_deref(),
            self.scope,
            self.project.as_ref().map(|project| project.id.as_str()),
        )
    }

    /// Importing this source enables the store server for its client: a
    /// user file of a client Loadout writes.
    pub fn enables(&self) -> bool {
        self.scope == Scope::User && adapter(self.client).is_none_or(|spec| !is_read_only(spec))
    }
}

/// [`Source::key`] from its parts.
pub(crate) fn source_key(
    client: &str,
    slot: Option<&str>,
    scope: Scope,
    project_id: Option<&str>,
) -> String {
    let client = match slot {
        Some(slot) => format!("{client}#{slot}"),
        None => client.to_owned(),
    };
    match (project_id, scope) {
        (None, _) => client,
        (Some(id), Scope::ProjectLocal) => format!("{client}@{id}/local"),
        (Some(id), _) => format!("{client}@{id}"),
    }
}

/// Foreign entries sharing one name.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub name: String,
    pub sources: Vec<Source>,
    /// The sources grouped by definition (indexes into `sources`), in
    /// first-seen order: sources that read as the same store fields and
    /// tool filter share a variant; one that cannot be imported is a
    /// variant of its own.
    pub variants: Vec<Vec<usize>>,
    /// One variant, and it can be imported.
    pub identical: bool,
    /// The store already has a server with this id.
    pub in_store: bool,
}

impl Candidate {
    /// The variant holding the source with key `key` ([`Source::key`]).
    pub fn variant_of(&self, key: &str) -> Option<usize> {
        self.variants
            .iter()
            .position(|v| v.iter().any(|&i| self.sources[i].key() == key))
    }

    /// The definition of variant `v`, or why it cannot be imported.
    pub fn variant_def(&self, v: usize) -> Result<&ImportedDef, &CoreError> {
        self.sources[self.variants[v][0]].def.as_ref()
    }

    /// The variants that can be imported.
    pub fn importable(&self) -> Vec<usize> {
        (0..self.variants.len())
            .filter(|&v| self.variant_def(v).is_ok())
            .collect()
    }

    /// Differing definitions: more than one variant can be imported.
    pub fn conflict(&self) -> bool {
        self.importable().len() > 1
    }

    /// Importing variant `v` enables the server for some client (it was
    /// found in a user file); otherwise it becomes a definition only.
    pub fn enables(&self, v: usize) -> bool {
        self.variants[v].iter().any(|&i| self.sources[i].enables())
    }

    /// The store id each variant gets by default: the name in lower case
    /// for the first variant that can be imported, `name-<client>` for the
    /// others (the client of the variant's first user file, else of its
    /// first source), numbered when two would be the same.
    pub fn suggested_ids(&self) -> Vec<String> {
        let base = self.name.to_ascii_lowercase();
        let mut taken = BTreeSet::new();
        let mut named = false;
        let mut ids = Vec::with_capacity(self.variants.len());
        for (v, indexes) in self.variants.iter().enumerate() {
            let id = if !named && self.variant_def(v).is_ok() {
                named = true;
                base.clone()
            } else {
                let source = indexes
                    .iter()
                    .map(|&i| &self.sources[i])
                    .find(|source| source.scope == Scope::User)
                    .unwrap_or(&self.sources[indexes[0]]);
                format!("{base}-{}", source.client)
            };
            let mut unique = id.clone();
            let mut n = 2;
            while !taken.insert(unique.clone()) {
                unique = format!("{id}-{n}");
                n += 1;
            }
            ids.push(unique);
        }
        ids
    }

    /// Importing the variant of the source with key `source`
    /// ([`Source::key`]), else of the source found in client `source` (its
    /// user file first), under the name, or with `rename` as
    /// `name-<client>`.
    pub fn choice_for(&self, source: &str, rename: bool) -> Result<VariantChoice, CoreError> {
        let found = self
            .sources
            .iter()
            .find(|candidate| candidate.key() == source)
            .or_else(|| {
                self.sources
                    .iter()
                    .filter(|candidate| candidate.client == source)
                    .min_by_key(|candidate| candidate.scope != Scope::User)
            })
            .ok_or_else(|| {
                CoreError::new(
                    ErrorCode::NotFound,
                    format!("{:?} was not found in {source}", self.name),
                )
            })?;
        let base = self.name.to_ascii_lowercase();
        Ok(VariantChoice {
            name: self.name.clone(),
            source: found.key(),
            id: if rename {
                format!("{base}-{}", found.client)
            } else {
                base
            },
        })
    }

    fn group(&mut self) {
        let mut variants: Vec<Vec<usize>> = Vec::new();
        for (index, source) in self.sources.iter().enumerate() {
            let found = source.def.as_ref().ok().and_then(|def| {
                variants.iter().position(|v| {
                    matches!(&self.sources[v[0]].def,
                        Ok(first) if first.fields == def.fields && first.tools == def.tools)
                })
            });
            match found {
                Some(v) => variants[v].push(index),
                None => variants.push(vec![index]),
            }
        }
        self.identical = variants.len() == 1 && self.sources[0].def.is_ok();
        self.variants = variants;
    }

    fn conflict_error(&self) -> CoreError {
        let keys: Vec<String> = self.sources.iter().map(Source::key).collect();
        CoreError::new(
            ErrorCode::Collision,
            format!(
                "{:?} differs between {}; pick one (`--pick NAME=CLIENT`) or import several \
                 (`--as NAME=CLIENT` imports one as NAME-CLIENT)",
                self.name,
                keys.join(", ")
            ),
        )
    }
}

/// Which files a scan reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ScanScope {
    /// Every client's user files and every known project's files.
    #[default]
    All,
    /// User files only.
    User,
    /// The files of every known project.
    Projects,
    /// The files of one known project, by id.
    Project(String),
}

/// What a scan reads.
#[derive(Debug, Clone, Default)]
pub struct ScanFilter {
    /// Only these clients (`None`: every client).
    pub clients: Option<Vec<String>>,
    pub scope: ScanScope,
}

/// Foreign entries of the selected clients' user files (no ledger record),
/// grouped by name in first-seen order. Read-only.
pub fn scan(
    ax: &ApplyEnv<'_>,
    desired: &DesiredState,
    ledger: &Ledger,
    clients: Option<&[String]>,
) -> Result<(Vec<Candidate>, Vec<CoreError>), CoreError> {
    let filter = ScanFilter {
        clients: clients.map(<[String]>::to_vec),
        scope: ScanScope::User,
    };
    scan_with(ax, desired, ledger, &filter)
}

/// Foreign entries (no ledger record) of the files `filter` selects: user
/// files, and the project and private project files of the projects this
/// machine knows (`desired`'s overlay), trusted or not. Grouped by name in
/// first-seen order, user files first. Read-only. A project that is
/// missing is left out, one that cannot be loaded is a warning; an unknown
/// project id is `not_found`.
pub fn scan_with(
    ax: &ApplyEnv<'_>,
    desired: &DesiredState,
    ledger: &Ledger,
    filter: &ScanFilter,
) -> Result<(Vec<Candidate>, Vec<CoreError>), CoreError> {
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    let mut keys: HashSet<(usize, String)> = HashSet::new();
    let mut skips = Vec::new();
    let mut warnings = Vec::new();
    walk(ax, desired, ledger, filter, &mut warnings, &mut |found| {
        let Some(value) = found.doc.get(found.container, &found.name)? else {
            return Ok(());
        };
        let spec = found.spec;
        let name = found.name;
        if let Some(reason) = spec
            .mcp
            .as_ref()
            .and_then(|mcp| mcp.mapper.skipped_by_client(&value))
        {
            skips.push(
                CoreError::new(
                    ErrorCode::ClientSkipsEntry,
                    format!(
                        "{} entry {name:?} {reason}; importing it starts a server {} does not \
                         run today",
                        spec.display_name, spec.display_name
                    ),
                )
                .with_path(&found.target.file),
            );
        }
        let def = read_native(spec, &name, &value);
        let source = Source {
            client: spec.id,
            scope: found.target.scope,
            project: found.project.cloned(),
            slot: found.slot,
            file: found.target.file.clone(),
            value,
            def,
        };
        let at = match by_name.get(&name) {
            Some(&at) => at,
            None => {
                by_name.insert(name.clone(), candidates.len());
                candidates.push(Candidate {
                    in_store: desired.servers.contains_key(&name.to_ascii_lowercase()),
                    name,
                    sources: Vec::new(),
                    variants: Vec::new(),
                    identical: false,
                });
                candidates.len() - 1
            }
        };
        if keys.insert((at, source.key())) {
            candidates[at].sources.push(source);
        }
        Ok(())
    })?;
    warnings.extend(skips);
    for candidate in &mut candidates {
        candidate.group();
    }
    Ok((candidates, warnings))
}

/// One foreign entry [`walk`] meets.
pub(crate) struct Found<'a> {
    pub spec: &'static AdapterSpec,
    pub target: &'a Target,
    pub project: Option<&'a SourceProject>,
    /// [`Source::slot`].
    pub slot: Option<String>,
    pub doc: &'a Document,
    pub container: &'a [&'a str],
    pub name: String,
}

/// Calls `visit` for every foreign entry of the files `filter` selects (see
/// [`scan_with`]), in file order. Each file is read and parsed once, however
/// many targets it holds (`~/.claude.json`: the user container and one per
/// project); ownership is looked up once per target.
pub(crate) fn walk(
    ax: &ApplyEnv<'_>,
    desired: &DesiredState,
    ledger: &Ledger,
    filter: &ScanFilter,
    warnings: &mut Vec<CoreError>,
    visit: &mut dyn FnMut(Found<'_>) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    let selected: Vec<&'static AdapterSpec> = ADAPTERS
        .iter()
        .copied()
        .filter(|spec| {
            filter
                .clients
                .as_ref()
                .is_none_or(|ids| ids.iter().any(|id| id == spec.id))
        })
        .collect();
    let mut files = Files::default();
    let mut read = |spec: &'static AdapterSpec,
                    targets: &[Target],
                    project: Option<&SourceProject>,
                    warnings: &mut Vec<CoreError>|
     -> Result<(), CoreError> {
        for (index, target) in targets.iter().enumerate() {
            let Some(doc) = files.doc(ax.fs, target, warnings)? else {
                continue;
            };
            let keys = target.container_keys()?;
            let container: Vec<&str> = keys.iter().map(String::as_str).collect();
            let names = match doc.list(&container) {
                Ok(names) => names,
                Err(err) => {
                    warnings.push(err.with_path(&target.file));
                    continue;
                }
            };
            let pointer = container_pointer(&keys);
            let file = target.file.to_string_lossy().into_owned();
            let owned: HashSet<&str> = ledger
                .in_container(LedgerSlot {
                    scope: target.scope,
                    project_id: project.map(|project| project.id.as_str()),
                    file: &file,
                    container: &pointer,
                })
                .map(|entry| entry.key.as_str())
                .collect();
            let slot = target
                .profile
                .clone()
                .or_else(|| (index > 0).then(|| (index + 1).to_string()));
            for name in names {
                if owned.contains(name.as_str()) {
                    continue;
                }
                visit(Found {
                    spec,
                    target,
                    project,
                    slot: slot.clone(),
                    doc,
                    container: &container,
                    name,
                })?;
            }
        }
        Ok(())
    };
    if matches!(filter.scope, ScanScope::All | ScanScope::User) {
        for &spec in &selected {
            let prefs = desired.prefs(spec.id);
            if let Ok(targets) = spec.mcp_targets(ax.env, ax.fs, Scope::User, None, &prefs) {
                read(spec, &targets, None, warnings)?;
            }
        }
    }
    if filter.scope != ScanScope::User {
        let trust = TrustStore::load(ax.fs, &ax.home.state_dir()).unwrap_or_else(|err| {
            warnings.push(err);
            TrustStore::default()
        });
        let known = known_projects(ax.fs, ax.env, desired.overlay.as_ref(), &trust);
        if let ScanScope::Project(id) = &filter.scope
            && !known.iter().any(|project| &project.id == id)
        {
            return Err(unknown_project(id));
        }
        for project in &known {
            if matches!(&filter.scope, ScanScope::Project(id) if *id != project.id) {
                continue;
            }
            let trusted = match &project.status {
                ProjectStatus::Missing => continue,
                ProjectStatus::Failed(err) => {
                    warnings.push(err.clone());
                    continue;
                }
                ProjectStatus::Loaded { trust, .. } => *trust == TrustState::Trusted,
            };
            let source_project = SourceProject {
                id: project.id.clone(),
                root: project.root.clone(),
                trusted,
            };
            for &spec in &selected {
                let prefs = desired.prefs(spec.id);
                for scope in [Scope::Project, Scope::ProjectLocal] {
                    if let Ok(targets) =
                        spec.mcp_targets(ax.env, ax.fs, scope, Some(&project.root), &prefs)
                    {
                        read(spec, &targets, Some(&source_project), warnings)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// The client files one scan read: each read and parsed once (a parse
/// error is reported once, for the first target that reads the file).
#[derive(Default)]
struct Files {
    docs: HashMap<(PathBuf, Format), Option<Document>>,
}

impl Files {
    fn doc(
        &mut self,
        fs: &dyn crate::fs::Fs,
        target: &Target,
        warnings: &mut Vec<CoreError>,
    ) -> Result<Option<&Document>, CoreError> {
        let key = (target.file.clone(), target.format);
        if !self.docs.contains_key(&key) {
            let doc = match read_target(fs, &target.file)? {
                None => None,
                Some(bytes) => match Document::parse(&bytes, target.format) {
                    Ok(doc) => Some(doc),
                    Err(err) => {
                        warnings.push(err.with_path(&target.file));
                        None
                    }
                },
            };
            self.docs.insert(key.clone(), doc);
        }
        Ok(self.docs[&key].as_ref())
    }
}

/// What a scan runs against as this machine sees the store: the desired
/// state (an empty one when there is no store yet) with this machine's
/// overlay, and the ledger. Read-only: creates nothing.
pub fn store_inputs(
    store: Option<&Store<'_>>,
    ax: &ApplyEnv<'_>,
) -> Result<(DesiredState, Ledger), CoreError> {
    let state = ax.home.state_dir();
    let machine = Machine::load(ax.fs, &state)?;
    let desired = match store {
        Some(store) => DesiredState::load(store, machine.as_ref().map(|m| m.id.as_str()))?,
        None => DesiredState::empty(),
    };
    Ok((desired, Ledger::load(ax.fs, &state)?))
}

/// A scan as this machine sees the store: its known projects and its
/// ledger ([`scan_with`]), with the desired state it was made against.
pub struct StoreScan {
    pub candidates: Vec<Candidate>,
    pub warnings: Vec<CoreError>,
    pub desired: DesiredState,
}

/// [`scan_with`] against `store`, or an empty desired state when there is
/// no store yet. Read-only: creates nothing.
pub fn scan_store(
    store: Option<&Store<'_>>,
    ax: &ApplyEnv<'_>,
    filter: &ScanFilter,
) -> Result<StoreScan, CoreError> {
    let (desired, ledger) = store_inputs(store, ax)?;
    let (candidates, warnings) = scan_with(ax, &desired, &ledger, filter)?;
    Ok(StoreScan {
        candidates,
        warnings,
        desired,
    })
}

/// What importing one variant under its suggested id stores.
#[derive(Debug, Clone)]
pub struct VariantPreview {
    /// [`Candidate::suggested_ids`].
    pub id: String,
    /// The server as it would be stored (references in place of sensitive
    /// literals), or why the variant cannot be imported.
    pub server: Result<Server, CoreError>,
    /// The sensitive literals that need a destination, with suggested
    /// aliases. No value.
    pub extractions: Vec<Extraction>,
}

/// [`VariantPreview`]s of every variant of `candidates` (same order),
/// aliases suggested against `catalog` and each other.
pub fn previews(candidates: &[Candidate], catalog: &Catalog) -> Vec<Vec<VariantPreview>> {
    let mut aliases = Aliases::new(catalog.iter().map(|alias| alias.alias.clone()));
    candidates
        .iter()
        .map(|candidate| {
            candidate
                .suggested_ids()
                .into_iter()
                .enumerate()
                .map(
                    |(v, id)| match variant_server(candidate, v, &id, false, &mut aliases) {
                        Ok((server, extracted)) => VariantPreview {
                            id,
                            server: Ok(server),
                            extractions: extracted
                                .into_iter()
                                .map(|extracted| extracted.extraction)
                                .collect(),
                        },
                        Err(err) => VariantPreview {
                            id,
                            server: Err(err),
                            extractions: Vec::new(),
                        },
                    },
                )
                .collect()
        })
        .collect()
}

/// The store server variant `v` of `candidate` becomes under `id`, each
/// sensitive literal replaced by a `${secret:<alias>}` reference (aliases
/// from `aliases`), and the values replaced. It is enabled for the clients
/// whose user file holds the variant, unless `only`; a variant found only
/// in project files or read-only clients becomes a definition without
/// enablement.
pub fn variant_server(
    candidate: &Candidate,
    v: usize,
    id: &str,
    only: bool,
    aliases: &mut Aliases,
) -> Result<(Server, Vec<Extracted>), CoreError> {
    ids::check(IdKind::Server, id)?;
    let indexes = candidate.variants.get(v).ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotFound,
            format!("{:?} has no variant {v}", candidate.name),
        )
    })?;
    let sources: Vec<&Source> = indexes.iter().map(|&i| &candidate.sources[i]).collect();
    let first = sources[0].def.as_ref().map_err(Clone::clone)?;
    let mut fields = first.fields.clone();
    let mut extractor = Extractor::new(id, aliases);
    if let Some(args) = fields.args.take() {
        fields.args = Some(extractor.args(args));
    }
    if let Some(url) = fields.url.take() {
        fields.url = Some(extractor.url(url)?);
    }
    if let Some(env) = fields.env.take() {
        fields.env = Some(extractor.map("env", env));
    }
    if let Some(headers) = fields.headers.take() {
        fields.headers = Some(extractor.map("headers", headers));
    }
    let extracted = extractor.finish();
    let mut server = Server::new(id.to_owned());
    set_fields(&mut server, &fields, first.tools.clone());
    // Read-only clients (`vscode-legacy`) and project files are sources
    // only: no enablement, and no `[client.<id>]` table.
    let enabling: Vec<&Source> = sources
        .iter()
        .copied()
        .filter(|source| source.enables())
        .collect();
    let mut clients = BTreeMap::new();
    for source in &enabling {
        let Ok(def) = &source.def else {
            continue;
        };
        let mut table = def.passthrough.clone();
        if candidate.name != id {
            table.insert(
                NATIVE_NAME_KEY.to_owned(),
                toml::Value::String(candidate.name.clone()),
            );
        }
        if !table.is_empty() {
            clients.insert(source.client.to_owned(), table);
        }
    }
    server.body.client = (!clients.is_empty()).then_some(clients);
    server.body.meta = Some(Meta {
        origin: Some(format!("imported:{}", sources[0].client)),
        ..Meta::default()
    });
    if !only && !enabling.is_empty() {
        server.enable = Some(Enable {
            clients: Some(ClientSelection::List({
                let mut ids: Vec<String> = Vec::new();
                for source in &enabling {
                    if !ids.iter().any(|id| id == source.client) {
                        ids.push(source.client.to_owned());
                    }
                }
                ids
            })),
            ..Enable::default()
        });
    }
    if let Some(err) = server.validate(Some(id)).first_error(None) {
        return Err(err);
    }
    Ok((server, extracted))
}

/// One variant to import: the candidate, any source of the variant
/// ([`Source::key`]) and the store id it becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantChoice {
    pub name: String,
    pub source: String,
    pub id: String,
}

/// Where a sensitive literal goes (SPEC 11.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretDestination {
    /// The OS keyring (`keyring://loadout/<alias>`), which receives the
    /// value read from the client file.
    Keyring,
    /// A 1Password item (`op://...`) or Environment variable
    /// (`op-env://...`), or an environment variable (`env:NAME`). Nothing
    /// is uploaded anywhere.
    Reference(SecretRef),
}

impl SecretDestination {
    /// `keyring`, `op://vault/item/field` (also written `op:op://...`),
    /// `op-env://<environment-id>/<NAME>` or `env:NAME`.
    pub fn parse(text: &str) -> Result<Self, CoreError> {
        let text = text.trim();
        if text == "keyring" {
            return Ok(Self::Keyring);
        }
        if let Some(name) = text.strip_prefix("env:") {
            return extract::env_reference(name).map(Self::Reference);
        }
        let reference = match text.strip_prefix("op:") {
            Some(rest) if rest.starts_with("op://") || rest.starts_with("op-env://") => rest,
            _ => text,
        };
        if reference.starts_with("op://") {
            return extract::onepassword_reference(reference).map(Self::Reference);
        }
        if reference.starts_with("op-env://")
            && let reference @ SecretRef::OnePasswordEnvironment { .. } =
                SecretRef::parse(reference)?
        {
            return Ok(Self::Reference(reference));
        }
        Err(CoreError::new(
            ErrorCode::InvalidValue,
            "a destination is keyring, op://vault/item/field, op-env://<environment>/<NAME> \
             or env:NAME",
        ))
    }

    /// `keyring`, `onepassword` or `env`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Keyring => "keyring",
            Self::Reference(SecretRef::Env { .. }) => "env",
            Self::Reference(_) => "onepassword",
        }
    }
}

/// Where the sensitive literals of candidate `name` go: one `field`
/// (`env.NAME`, `headers.NAME`, `args[N]`, `url`), or with `field` `None`
/// every one no other choice names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretChoice {
    pub name: String,
    pub field: Option<String>,
    pub destination: SecretDestination,
}

/// What to import (one commit).
#[derive(Debug, Clone, Default)]
pub struct ImportRequest {
    pub picks: Vec<VariantChoice>,
    pub secrets: Vec<SecretChoice>,
    /// Copy the definitions without enabling them (no ownership taken).
    pub only: bool,
    /// Candidates left out before staging, with the reason; reported as
    /// skipped.
    pub unchosen: Vec<(String, CoreError)>,
}

impl ImportRequest {
    /// Every candidate (or those in `names`) with one variant that can be
    /// imported, under its name. A conflict, or a candidate that cannot be
    /// imported, is left in [`ImportRequest::unchosen`].
    pub fn defaults(candidates: &[Candidate], names: Option<&[String]>, only: bool) -> Self {
        let mut request = Self {
            only,
            ..Self::default()
        };
        for candidate in candidates
            .iter()
            .filter(|candidate| names.is_none_or(|names| names.contains(&candidate.name)))
        {
            match candidate.importable().as_slice() {
                [v] => request.picks.push(VariantChoice {
                    name: candidate.name.clone(),
                    source: candidate.sources[candidate.variants[*v][0]].key(),
                    id: candidate.name.to_ascii_lowercase(),
                }),
                [] => {
                    let err = candidate.sources[0]
                        .def
                        .as_ref()
                        .err()
                        .cloned()
                        .expect("a candidate without importable variants has a failed source");
                    request.unchosen.push((candidate.name.clone(), err));
                }
                _ => request
                    .unchosen
                    .push((candidate.name.clone(), candidate.conflict_error())),
            }
        }
        request
    }

    /// Replaces what is chosen for candidate `name` by `picks` (empty:
    /// skip it).
    pub fn choose(&mut self, name: &str, picks: Vec<VariantChoice>) {
        self.picks.retain(|pick| pick.name != name);
        self.unchosen.retain(|(unchosen, _)| unchosen != name);
        self.picks.extend(picks);
    }

    fn destination(&self, name: &str, field: &str) -> Option<&SecretDestination> {
        let of_name = || self.secrets.iter().filter(|choice| choice.name == name);
        of_name()
            .find(|choice| choice.field.as_deref() == Some(field))
            .or_else(|| of_name().find(|choice| choice.field.is_none()))
            .map(|choice| &choice.destination)
    }
}

/// A secret alias an import created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedAlias {
    pub alias: String,
    pub server: String,
    /// `env.NAME`, `headers.NAME`, `args[N]` or `url`.
    pub field: String,
    /// [`SecretDestination::kind`].
    pub destination: &'static str,
}

/// Result of [`stage_import`].
#[derive(Debug, Clone, Default)]
pub struct ImportResult {
    /// New server ids.
    pub imported: Vec<String>,
    /// New servers found only in project files or read-only clients (not
    /// `only`): definitions without enablement, for a project manifest to
    /// name (`from`).
    pub definition_only: Vec<String>,
    /// Candidates left out, with the reason.
    pub skipped: Vec<(String, CoreError)>,
    pub aliases: Vec<ImportedAlias>,
    /// New server ids enabled for some client (not `only`, not definitions
    /// only): what [`apply_import`] plans.
    pub enabled: Vec<String>,
    /// `(client, native name)` of the user-file entries the new servers
    /// take over without asking: every user file of that client holds the
    /// imported definition ([`apply_import`]).
    pub entries: Vec<(String, String)>,
}

/// The instant apply of an import (SPEC 8.6, 8.7): in instant mode, plans
/// the user files of the enabled servers and takes over the entries they
/// were imported from. An entry that renders identically is adopted
/// without a write; one that does not (a literal secret now a reference)
/// is overwritten with the rendered entry, unless another user file of the
/// same client holds a different definition under that name (then it is
/// left a collision to decide). Project files are not planned. `None` in
/// review mode, where Pending changes shows those entries for a decision.
pub fn apply_import(
    tx: &mut Transaction<'_>,
    ax: &ApplyEnv<'_>,
    result: &ImportResult,
) -> Result<Option<ApplyReport>, CoreError> {
    if result.enabled.is_empty() {
        // Definitions only (`only`, project files): nothing to write.
        return Ok(None);
    }
    let desired = DesiredState::load(&*tx, Some(&tx.machine().id.clone()))?;
    if desired.apply_mode() != ApplyMode::Instant {
        return Ok(None);
    }
    let request = ApplyRequest {
        options: PlanOptions {
            decisions: result
                .entries
                .iter()
                .map(|(client, name)| Decision {
                    client: client.clone(),
                    name: name.clone(),
                    resolution: OnConflict::Overwrite,
                })
                .collect(),
            servers: Some(result.enabled.clone()),
            // User files only: a project entry of the same name (a
            // manifest's `from`) is not one the user chose to import.
            scope: ScopeFilter::System,
            ..PlanOptions::default()
        },
        trigger: ApplyTrigger::Instant,
    };
    apply_in(tx, ax, &request).map(Some)
}

/// Stages `request` in `tx`: one store server per pick, each sensitive
/// literal stored as an alias at the destination chosen for it (keyring
/// values written right before the commit). A pick that cannot be staged
/// (gone since the scan, an id that is taken, a literal without a
/// destination) is skipped; the rest still import.
pub fn stage_import(
    tx: &mut Transaction<'_>,
    ax: &ApplyEnv<'_>,
    candidates: &[Candidate],
    request: &ImportRequest,
) -> Result<ImportResult, CoreError> {
    let mut result = ImportResult {
        skipped: request.unchosen.clone(),
        ..ImportResult::default()
    };
    let machine = tx.machine().id.clone();
    let catalog = Catalog::load(&*tx, Some(&machine))?;
    let mut aliases = Aliases::new(catalog.iter().map(|alias| alias.alias.clone()));
    let memory = tx.store().secret_memory().clone();
    let mut staged: BTreeSet<String> = BTreeSet::new();
    let mut picked: BTreeSet<(&str, usize)> = BTreeSet::new();
    let mut choices = Vec::new();
    for pick in &request.picks {
        let name = pick.name.as_str();
        let id = pick.id.as_str();
        let staged_pick = (|| {
            let candidate = candidates
                .iter()
                .find(|candidate| candidate.name == name)
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::NotFound,
                        format!("{name:?} is not a foreign entry any more; scan again"),
                    )
                })?;
            let v = candidate.variant_of(&pick.source).ok_or_else(|| {
                CoreError::new(
                    ErrorCode::NotFound,
                    format!("{name:?} has no source {:?}; scan again", pick.source),
                )
            })?;
            if picked.contains(&(name, v)) {
                return Err(CoreError::new(
                    ErrorCode::InvalidValue,
                    format!("this definition of {name:?} is chosen twice"),
                ));
            }
            if staged.contains(id) || tx.exists(&server_path(id)?)? {
                return Err(CoreError::new(
                    ErrorCode::AlreadyExists,
                    format!("server {id:?} already exists"),
                ));
            }
            let mut trial = aliases.clone();
            let (server, extracted) = variant_server(candidate, v, id, request.only, &mut trial)?;
            let missing: Vec<&str> = extracted
                .iter()
                .map(|extracted| extracted.extraction.field.as_str())
                .filter(|field| request.destination(name, field).is_none())
                .collect();
            if !missing.is_empty() {
                return Err(CoreError::new(
                    ErrorCode::NeedsSecretExtraction,
                    format!(
                        "{} may hold a secret; choose where each goes (the OS keyring, \
                         1Password or an environment variable)",
                        missing.join(", ")
                    ),
                ));
            }
            aliases = trial;
            Ok((candidate, v, server, extracted))
        })();
        let (candidate, v, server, extracted) = match staged_pick {
            Ok(staged_pick) => staged_pick,
            Err(err) => {
                result.skipped.push((pick.name.clone(), err));
                continue;
            }
        };
        picked.insert((name, v));
        let enabled = server.enable.is_some();
        tx.add_server(server)?;
        tx.add_entity(format!("server/{id}"));
        staged.insert(id.to_owned());
        result.imported.push(id.to_owned());
        if enabled {
            result.enabled.push(id.to_owned());
            // Take an entry over without asking only when every user file
            // of that client holds this definition; a client whose other
            // file (a profile, a mirror) holds another one is left to Ask.
            for &i in &candidate.variants[v] {
                let source = &candidate.sources[i];
                let whole_client = candidate.sources.iter().enumerate().all(|(j, other)| {
                    other.client != source.client
                        || !other.enables()
                        || candidate.variants[v].contains(&j)
                });
                let pair = (source.client.to_owned(), candidate.name.clone());
                if source.enables() && whole_client && !result.entries.contains(&pair) {
                    result.entries.push(pair);
                }
            }
        } else if !request.only {
            result.definition_only.push(id.to_owned());
        }
        for extracted in extracted {
            let destination = request
                .destination(name, &extracted.extraction.field)
                .expect("checked above");
            // The value stays in the client file until the apply rewrites
            // the entry; a store file holding it is refused.
            memory.record(extracted.value.expose());
            let Extraction { alias, field, .. } = extracted.extraction;
            result.aliases.push(ImportedAlias {
                alias: alias.clone(),
                server: id.to_owned(),
                field,
                destination: destination.kind(),
            });
            choices.push(ExtractionChoice {
                alias,
                target: match destination {
                    SecretDestination::Keyring => ExtractionTarget::Keyring(extracted.value),
                    SecretDestination::Reference(reference) => {
                        ExtractionTarget::Reference(reference.clone())
                    }
                },
            });
        }
    }
    extract::stage_choices(tx, ax, &choices)?;
    Ok(result)
}

fn is_read_only(spec: &AdapterSpec) -> bool {
    spec.mcp.as_ref().is_some_and(|mcp| mcp.read_only)
}

fn set_fields(server: &mut Server, fields: &ServerFields, tools: Option<ToolFilter>) {
    let body = &mut server.body;
    body.transport = fields.transport;
    body.command = fields.command.clone();
    body.args = fields.args.clone();
    body.cwd = fields.cwd.clone();
    body.url = fields.url.clone();
    body.headers = fields.headers.clone();
    body.env = fields.env.clone();
    body.tools = tools;
}

/// Enables store servers for clients whose user file already holds an
/// identical foreign entry under the server's native name, so the apply
/// that follows adopts them without a write. Returns `client/name` pairs.
pub fn stage_adopt_identical(
    tx: &mut Transaction<'_>,
    ax: &ApplyEnv<'_>,
    desired: &DesiredState,
    candidates: &[Candidate],
) -> Result<Vec<String>, CoreError> {
    let cx = ax.render_cx(tx.store().secret_memory().clone());
    let mut adopted = Vec::new();
    for candidate in candidates {
        // Enabling a store server reaches user files only; a project entry
        // comes from the project's manifests.
        for source in candidate.sources.iter().filter(|s| s.scope == Scope::User) {
            // A read-only client (`vscode-legacy`) is never enabled.
            let Some(spec) = adapter(source.client).filter(|spec| !is_read_only(spec)) else {
                continue;
            };
            let Some(server) = desired
                .servers
                .values()
                .find(|server| native_name(server, spec.id) == candidate.name)
            else {
                continue;
            };
            let enabled = match server.enable.as_ref().and_then(|e| e.clients.as_ref()) {
                Some(ClientSelection::List(ids)) => ids.iter().any(|id| id == spec.id),
                Some(ClientSelection::AllDetected) => true,
                None => false,
            };
            if enabled {
                continue;
            }
            let Ok(rendered) = render(&cx, desired, server, spec, Sharing::Private) else {
                continue;
            };
            let rendered = rendered.server;
            let mcp = spec.mcp.as_ref().expect("rendered clients have MCP");
            let cx_write = WriteCx {
                os: ax.env.os(),
                existing: Some(&source.value),
            };
            let Ok(native) = mcp.mapper.to_native(&rendered, &cx_write) else {
                continue;
            };
            if mcp.mapper.canonical(&native) != mcp.mapper.canonical(&source.value) {
                continue;
            }
            update_server(tx, &server.id, |server| enable_client(server, spec.id))?;
            adopted.push(format!("{}/{}", spec.id, candidate.name));
        }
    }
    Ok(adopted)
}

/// Whether `entry` is resolved by a store edit: an `import`, or the
/// release of an entry that is still desired (recorded in this machine's
/// overlay).
pub fn is_store_resolution(entry: &EntryPlan) -> bool {
    entry.action == Action::Import || (entry.action == Action::Release && entry.desired.is_some())
}

/// Carries out the store side of a plan's decisions ([`is_store_resolution`]):
/// a changed native entry becomes the server definition; a deleted one
/// stops the server from being enabled for that client (synced, SPEC 8.2
/// "respect deletion"); a released one is recorded in this machine's
/// overlay only (`[enable.released]`), so other machines keep managing
/// their entry. Returns the changed server ids; entries that cannot be
/// staged are left for the re-plan to report.
pub fn stage_resolutions(
    tx: &mut Transaction<'_>,
    plan: &Plan,
    desired: &DesiredState,
) -> Result<ImportResolutions, CoreError> {
    let mut result = ImportResolutions::default();
    for target in &plan.targets {
        for entry in target.entries.iter().filter(|e| is_store_resolution(e)) {
            let (Some(client), Some(id)) = (entry.client, entry.server_id.as_ref()) else {
                continue;
            };
            if target.project_id.is_some() {
                // A project entry comes from the project's manifests, never
                // from a store server.
                let message = if entry.action == Action::Release {
                    "a project entry cannot be released here; remove the client from the \
                     entry's clients in the project's manifest instead"
                } else {
                    "a project entry cannot be imported into the store; edit the project's \
                     manifest instead"
                };
                result.failed.push((
                    client.to_owned(),
                    entry.name.clone(),
                    CoreError::new(ErrorCode::ScopeNotSupported, message),
                ));
                continue;
            }
            let Some(spec) = adapter(client) else {
                continue;
            };
            if !desired.servers.contains_key(id) {
                continue;
            }
            if entry.action == Action::Release {
                if let Err(err) = release_client(tx, id, client) {
                    result
                        .failed
                        .push((client.to_owned(), entry.name.clone(), err));
                }
                continue;
            }
            let edit = match (&entry.state, &entry.observed) {
                (EntryState::Missing, _) => {
                    update_server(tx, id, |server| disable_client(server, client))
                }
                (_, Some(observed)) => {
                    server_from_native(spec, &entry.name, observed).and_then(|def| {
                        update_server(tx, id, |server| {
                            set_fields(server, &def.fields, def.tools.clone());
                            set_passthrough(server, client, &def.passthrough);
                            Ok(())
                        })
                    })
                }
                _ => continue,
            };
            match edit {
                Ok(()) if !result.changed.contains(id) => {
                    tx.add_entity(format!("server/{id}"));
                    result.changed.push(id.clone());
                }
                Ok(()) => {}
                Err(err) => result
                    .failed
                    .push((client.to_owned(), entry.name.clone(), err)),
            }
        }
    }
    Ok(result)
}

/// Records in this machine's overlay that it released `client`'s entry of
/// server `id` (`[enable.released]`).
fn release_client(tx: &mut Transaction<'_>, id: &str, client: &str) -> Result<(), CoreError> {
    let mut doc = overlay_doc(tx)?;
    let enable = doc.value_mut().enable.get_or_insert_with(Default::default);
    let clients = enable
        .released
        .get_or_insert_with(BTreeMap::new)
        .entry(id.to_owned())
        .or_default();
    if clients.iter().any(|c| c == client) {
        return Ok(());
    }
    clients.push(client.to_owned());
    let relative = overlay_path(&tx.machine().id)?;
    tx.write(&relative, doc.to_text()?)?;
    tx.add_entity(format!("machine/{}", tx.machine().id));
    Ok(())
}

/// Drops this machine's releases of server `id` for `clients` (they are
/// enabled again by an explicit choice); returns whether the overlay
/// changed.
pub fn unrelease_clients(
    tx: &mut Transaction<'_>,
    id: &str,
    clients: &[String],
) -> Result<bool, CoreError> {
    let mut doc = overlay_doc(tx)?;
    let Some(enable) = doc.value_mut().enable.as_mut() else {
        return Ok(false);
    };
    let Some(released) = enable.released.as_mut() else {
        return Ok(false);
    };
    let Some(list) = released.get_mut(id) else {
        return Ok(false);
    };
    let before = list.len();
    list.retain(|client| !clients.contains(client));
    if list.len() == before {
        return Ok(false);
    }
    if list.is_empty() {
        released.remove(id);
    }
    if released.is_empty() {
        enable.released = None;
    }
    let relative = overlay_path(&tx.machine().id)?;
    tx.write(&relative, doc.to_text()?)?;
    Ok(true)
}

/// Result of [`stage_resolutions`].
#[derive(Debug, Clone, Default)]
pub struct ImportResolutions {
    /// Store servers changed.
    pub changed: Vec<String>,
    /// `(client, entry name, reason)` of entries that could not be imported.
    pub failed: Vec<(String, String, CoreError)>,
}

/// Reads server `id` from the transaction, edits it and stages it back
/// (comments and unknown keys survive).
fn update_server(
    tx: &mut Transaction<'_>,
    id: &str,
    edit: impl FnOnce(&mut Server) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    let relative = server_path(id)?;
    let text = tx.read_to_string(&relative)?.ok_or_else(|| {
        CoreError::new(ErrorCode::NotFound, format!("server {id:?} does not exist"))
    })?;
    let mut doc = TomlDocument::<Server>::parse(&text)
        .map_err(|diagnostic| diagnostic.to_core_error(Some(std::path::Path::new(&relative))))?;
    edit(doc.value_mut())?;
    tx.write(&relative, doc.to_text()?)
}

fn enable_client(server: &mut Server, client: &str) -> Result<(), CoreError> {
    let enable = server.enable.get_or_insert_with(Enable::default);
    match enable
        .clients
        .get_or_insert_with(|| ClientSelection::List(Vec::new()))
    {
        ClientSelection::List(ids) => {
            if !ids.iter().any(|id| id == client) {
                ids.push(client.to_owned());
            }
        }
        ClientSelection::AllDetected => {}
    }
    Ok(())
}

fn disable_client(server: &mut Server, client: &str) -> Result<(), CoreError> {
    match server.enable.as_mut().and_then(|e| e.clients.as_mut()) {
        Some(ClientSelection::List(ids)) => {
            ids.retain(|id| id != client);
            Ok(())
        }
        Some(ClientSelection::AllDetected) => Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!(
                "{:?} is enabled for all detected clients; list clients to leave {client} out",
                server.id
            ),
        )),
        None => Ok(()),
    }
}

fn set_passthrough(server: &mut Server, client: &str, passthrough: &toml::Table) {
    let clients = server.body.client.get_or_insert_with(BTreeMap::new);
    let table = clients.entry(client.to_owned()).or_default();
    table.retain(|key, _| key == NATIVE_NAME_KEY);
    for (key, value) in passthrough {
        table.insert(key.clone(), value.clone());
    }
    if table.is_empty() {
        clients.remove(client);
    }
    if clients.is_empty() {
        server.body.client = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations_parse() {
        assert_eq!(
            SecretDestination::parse(" keyring ").unwrap(),
            SecretDestination::Keyring
        );
        for text in ["op://Vault/Item/field", "op:op://Vault/Item/field"] {
            let destination = SecretDestination::parse(text).unwrap();
            assert_eq!(destination.kind(), "onepassword", "{text}");
            assert!(matches!(
                destination,
                SecretDestination::Reference(SecretRef::OnePassword { .. })
            ));
        }
        assert_eq!(
            SecretDestination::parse("env:CHATTY_KEY").unwrap().kind(),
            "env"
        );
        for bad in [
            "op://Vault/Item",
            "keyring://loadout/X",
            "vault",
            "env:not a name",
        ] {
            assert!(SecretDestination::parse(bad).is_err(), "{bad}");
        }
    }
}

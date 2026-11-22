//! Adopt and import (SPEC 8.6): native entries into the store.
//!
//! - [`scan`] lists foreign entries of every client's user file, grouped by
//!   name; identical definitions collapse into one candidate.
//! - [`stage_import`] turns candidates into store servers; the apply that
//!   follows adopts the native entries silently when they render
//!   identically.
//! - [`stage_adopt_identical`] enables existing store servers for clients
//!   that already hold an identical entry.
//! - [`stage_resolutions`] carries out `import` decisions of a plan ("import
//!   their change", "respect the deletion").
//!
//! A read-only client (`vscode-legacy`) is a source only: what is imported
//! from it is never enabled for it, and VS Code's `${input:...}` values are
//! `needs_manual_secret`.
//!
//! Secret extraction (SPEC 11.7): every literal in `env`, `headers`,
//! `args` and the URL goes through the classifier. Sensitive literals make
//! the entry `needs_secret_extraction` (the extraction destinations are the
//! import wizard's), so no value that may be a secret reaches the store;
//! non-sensitive literals (`LOG_LEVEL = "info"`) are imported as written.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::adapters::caps::FieldSet;
use crate::adapters::mapper::WriteCx;
use crate::adapters::{ADAPTERS, AdapterSpec, adapter};
use crate::apply::ApplyEnv;
use crate::apply::ledger::Ledger;
use crate::apply::plan::{Action, EntryPlan, EntryState, Plan, read_target};
use crate::apply::render::{DesiredState, native_name, native_to_template, render};
use crate::error::{CoreError, ErrorCode};
use crate::formats::{Document, NativeValue};
use crate::layering::projects::overlay_doc;
use crate::model::TomlDocument;
use crate::model::applied::Scope;
use crate::model::clients::{ClientSelection, NATIVE_NAME_KEY};
use crate::model::ids::{self, IdKind};
use crate::model::server::{Enable, Meta, Server, ServerFields, ToolFilter};
use crate::secrets::catalog::overlay_path;
use crate::secrets::classify::{classify_args_import, classify_import};
use crate::secrets::delivery::Sharing;
use crate::store::{Transaction, server_path};

/// A native entry as store fields.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedDef {
    pub fields: ServerFields,
    pub tools: Option<ToolFilter>,
    /// The client's allowlisted native fields, for `[client.<id>]`.
    pub passthrough: toml::Table,
}

/// Reads a native entry of `spec`'s client as store fields: client
/// variable syntax becomes `${env:NAME}`, other `${` becomes `$${`.
pub fn server_from_native(
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
                    "{text:?} uses a {} variable or command form the store cannot express",
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
        let flagged = classify_args_import(&args);
        // Named by position, never by value: the message may reach a UI.
        if let Some(index) = flagged.iter().position(Option::is_some) {
            return Err(needs_extraction(format!("argument {}", index + 1)));
        }
        fields.args = (!args.is_empty()).then_some(args);
    }
    if let Some(cwd) = &native.cwd {
        fields.cwd = Some(convert(cwd, FieldSet::CWD)?);
    }
    if let Some(url) = &native.url {
        let url = convert(url, FieldSet::URL)?;
        if classify_import(None, &url).is_some() {
            return Err(needs_extraction("the URL".to_owned()));
        }
        fields.url = Some(url);
    }
    for (field, source, target) in [
        (FieldSet::ENV, &native.env, &mut fields.env),
        (FieldSet::HEADERS, &native.headers, &mut fields.headers),
    ] {
        let mut map = BTreeMap::new();
        for (name, value) in source.iter().flatten() {
            let value = convert(value, field)?;
            if classify_import(Some(name), &value).is_some() {
                return Err(needs_extraction(format!("the literal value of {name:?}")));
            }
            map.insert(name.clone(), value);
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

fn needs_extraction(what: String) -> CoreError {
    CoreError::new(
        ErrorCode::NeedsSecretExtraction,
        format!("{what} may be a secret; it needs secret extraction before import"),
    )
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

/// Where a foreign entry was found.
#[derive(Debug, Clone)]
pub struct Source {
    pub client: &'static str,
    pub file: PathBuf,
    /// The native entry.
    pub value: NativeValue,
    /// The entry as store fields, or why it cannot be imported.
    pub def: Result<ImportedDef, CoreError>,
}

/// Foreign entries sharing one name.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub name: String,
    pub sources: Vec<Source>,
    /// Every source reads as the same store definition.
    pub identical: bool,
    /// The store already has a server with this id.
    pub in_store: bool,
}

/// Foreign entries of the selected clients' user files (no ledger record),
/// grouped by name in first-seen order. Read-only.
pub fn scan(
    ax: &ApplyEnv<'_>,
    desired: &DesiredState,
    ledger: &Ledger,
    clients: Option<&[String]>,
) -> Result<(Vec<Candidate>, Vec<CoreError>), CoreError> {
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut warnings = Vec::new();
    for spec in ADAPTERS {
        if clients.is_some_and(|ids| !ids.iter().any(|id| id == spec.id)) {
            continue;
        }
        let targets =
            match spec.mcp_targets(ax.env, ax.fs, Scope::User, None, &desired.prefs(spec.id)) {
                Ok(targets) => targets,
                Err(_) => continue,
            };
        for target in targets {
            let Some(bytes) = read_target(ax.fs, &target.file)? else {
                continue;
            };
            let doc = match Document::parse(&bytes, target.format) {
                Ok(doc) => doc,
                Err(err) => {
                    warnings.push(err.with_path(&target.file));
                    continue;
                }
            };
            let keys = target.container_keys()?;
            let container: Vec<&str> = keys.iter().map(String::as_str).collect();
            let pointer = crate::apply::ledger::container_pointer(&keys);
            let file = target.file.to_string_lossy().into_owned();
            let names = match doc.list(&container) {
                Ok(names) => names,
                Err(err) => {
                    warnings.push(err.with_path(&target.file));
                    continue;
                }
            };
            for name in names {
                let slot = crate::apply::ledger::LedgerSlot {
                    scope: Scope::User,
                    project_id: None,
                    file: &file,
                    container: &pointer,
                };
                if ledger.get(&slot, &name).is_some() {
                    continue;
                }
                let Some(value) = doc.get(&container, &name)? else {
                    continue;
                };
                let def = server_from_native(spec, &name, &value);
                if let Some(reason) = spec
                    .mcp
                    .as_ref()
                    .and_then(|mcp| mcp.mapper.skipped_by_client(&value))
                {
                    warnings.push(
                        CoreError::new(
                            ErrorCode::ClientSkipsEntry,
                            format!(
                                "{} entry {name:?} {reason}; importing it starts a server {} \
                                 does not run today",
                                spec.display_name, spec.display_name
                            ),
                        )
                        .with_path(&target.file),
                    );
                }
                let source = Source {
                    client: spec.id,
                    file: target.file.clone(),
                    value,
                    def,
                };
                match candidates.iter_mut().find(|c| c.name == name) {
                    Some(candidate) => {
                        if !candidate.sources.iter().any(|s| s.client == spec.id) {
                            candidate.sources.push(source);
                        }
                    }
                    None => candidates.push(Candidate {
                        in_store: desired.servers.contains_key(&name.to_ascii_lowercase()),
                        name,
                        sources: vec![source],
                        identical: true,
                    }),
                }
            }
        }
    }
    for candidate in &mut candidates {
        let first = candidate.sources[0].def.as_ref().ok();
        candidate.identical = candidate
            .sources
            .iter()
            .all(|source| match (&source.def, first) {
                (Ok(def), Some(first)) => def.fields == first.fields && def.tools == first.tools,
                _ => false,
            });
    }
    Ok((candidates, warnings))
}

/// Result of [`stage_import`].
#[derive(Debug, Clone, Default)]
pub struct ImportResult {
    /// New server ids.
    pub imported: Vec<String>,
    /// Candidates left out, with the reason.
    pub skipped: Vec<(String, CoreError)>,
}

/// Stages one store server per candidate (all, or those in `names`).
/// Each server is enabled for the clients it was found in, unless `only`
/// (import the definition without taking ownership).
pub fn stage_import(
    tx: &mut Transaction<'_>,
    candidates: &[Candidate],
    names: Option<&[String]>,
    only: bool,
) -> Result<ImportResult, CoreError> {
    let mut result = ImportResult::default();
    for candidate in candidates {
        if names.is_some_and(|names| !names.contains(&candidate.name)) {
            continue;
        }
        match candidate_server(candidate, only) {
            Ok(server) => {
                let id = server.id.clone();
                let relative = server_path(&id)?;
                if tx.exists(&relative)? {
                    result.skipped.push((
                        candidate.name.clone(),
                        CoreError::new(
                            ErrorCode::AlreadyExists,
                            format!("server {id:?} already exists"),
                        ),
                    ));
                    continue;
                }
                tx.add_server(server)?;
                tx.add_entity(format!("server/{id}"));
                result.imported.push(id);
            }
            Err(err) => result.skipped.push((candidate.name.clone(), err)),
        }
    }
    Ok(result)
}

fn candidate_server(candidate: &Candidate, only: bool) -> Result<Server, CoreError> {
    let id = candidate.name.to_ascii_lowercase();
    ids::check(IdKind::Server, &id)?;
    let mut defs = Vec::new();
    for source in &candidate.sources {
        defs.push((source.client, source.def.clone()?));
    }
    if !candidate.identical {
        return Err(CoreError::new(
            ErrorCode::Collision,
            format!(
                "{:?} differs between {}; import it from one client with --client",
                candidate.name,
                candidate
                    .sources
                    .iter()
                    .map(|s| s.client)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    let (first_client, first) = &defs[0];
    let mut server = Server::new(id.clone());
    set_fields(&mut server, &first.fields, first.tools.clone());
    // Read-only clients (`vscode-legacy`) are sources only: the server is
    // not enabled for them, and they get no `[client.<id>]` table.
    let writable: Vec<&(&'static str, ImportedDef)> = defs
        .iter()
        .filter(|(client, _)| adapter(client).is_none_or(|spec| !is_read_only(spec)))
        .collect();
    let mut clients = BTreeMap::new();
    for (client, def) in &writable {
        let mut table = def.passthrough.clone();
        if candidate.name != id {
            table.insert(
                NATIVE_NAME_KEY.to_owned(),
                toml::Value::String(candidate.name.clone()),
            );
        }
        if !table.is_empty() {
            clients.insert((*client).to_owned(), table);
        }
    }
    server.body.client = (!clients.is_empty()).then_some(clients);
    server.body.meta = Some(Meta {
        origin: Some(format!("imported:{first_client}")),
        ..Meta::default()
    });
    if !only && !writable.is_empty() {
        server.enable = Some(Enable {
            clients: Some(ClientSelection::List(
                writable
                    .iter()
                    .map(|(client, _)| (*client).to_owned())
                    .collect(),
            )),
            ..Enable::default()
        });
    }
    if let Some(err) = server.validate(Some(&id)).first_error(None) {
        return Err(err);
    }
    Ok(server)
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
        for source in &candidate.sources {
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

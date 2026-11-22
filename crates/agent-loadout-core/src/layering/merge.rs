//! The project layer (SPEC 6.2): `project.local.toml` merged over
//! `project.toml` per server name, `from` resolved against the store,
//! `unset` applied, suppressions told apart, and each entry's client set
//! and layer of origin (which decides where it is written, SPEC 6.5).
//!
//! The merge is the same everywhere: tables merge recursively (`env`,
//! `headers`, `client.<id>`), arrays and scalars replace.

use std::collections::{BTreeMap, BTreeSet};

use crate::adapters::AdapterSpec;
use crate::adapters::caps::ProjectDisable;
use crate::apply::render::effective_fields;
use crate::env::Env;
use crate::error::{CoreError, ErrorCode};
use crate::layering::manifest::{ManifestFile, Project};
use crate::model::clients::ClientSelection;
use crate::model::machine::MachineOverlay;
use crate::model::project::ProjectServer;
use crate::model::server::{Server, ServerBody};
use crate::model::validate::{Diagnostics, join};

/// Which manifest an entry comes from, for placement (SPEC 6.1, 6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryLayer {
    /// Defined only by `project.toml`: written to the shared project file.
    Shared,
    /// Defined or touched by `project.local.toml`: written privately.
    Local,
}

impl EntryLayer {
    pub const fn as_str(self) -> &'static str {
        match self {
            EntryLayer::Shared => "project",
            EntryLayer::Local => "project-local",
        }
    }
}

/// What a merged project entry asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum EntryBody {
    /// A server to write: the fully resolved definition (its `id` is the
    /// entry name; no OS overrides left, the machine overlay applied to a
    /// `from` base), or why it cannot be written (`unknown_template`, an
    /// incomplete definition).
    Server(Result<Box<Server>, CoreError>),
    /// `enabled = false` alone: hide the same-named system server here.
    Suppression,
}

/// One server name of a project after both layers merged.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectEntry {
    pub name: String,
    pub layer: EntryLayer,
    /// The entry's `clients`, else the merged `[defaults].clients`, else
    /// `all-detected` (SPEC 6.1).
    pub clients: ClientSelection,
    /// The store server the entry starts from.
    pub from: Option<String>,
    /// Inherited keys removed after the merge.
    pub unset: Vec<String>,
    /// The merged `enabled` (`true` when absent).
    pub enabled: bool,
    pub body: EntryBody,
    /// For a name both files define: the entry as `project.toml` alone
    /// defines it (with its own `[defaults]`). It stays in the shared
    /// project file of a client that holds the merged entry privately, so
    /// a local override never changes the committed file.
    pub shared: Option<Box<ProjectEntry>>,
}

/// Merges `over` into `base`: tables recursively, anything else replaces.
pub fn deep_merge(base: &mut toml::Value, over: &toml::Value) {
    match (base, over) {
        (toml::Value::Table(base), toml::Value::Table(over)) => {
            for (key, value) in over {
                match base.get_mut(key) {
                    Some(slot) => deep_merge(slot, value),
                    None => {
                        base.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, over) => *base = over.clone(),
    }
}

/// Removes the key at the dotted `path` (`env.FOO`); returns whether it was
/// there.
pub fn remove_path(value: &mut toml::Value, path: &str) -> bool {
    let mut parts: Vec<&str> = path.split('.').collect();
    let Some(last) = parts.pop() else {
        return false;
    };
    let mut current = value;
    for part in parts {
        match current.get_mut(part) {
            Some(next) => current = next,
            None => return false,
        }
    }
    current
        .as_table_mut()
        .is_some_and(|table| table.remove(last).is_some())
}

/// The merged `[defaults].clients` of a project (local over shared), else
/// `all-detected`.
pub fn default_clients(project: &Project) -> ClientSelection {
    [project.local.as_ref(), project.shared.as_ref()]
        .into_iter()
        .flatten()
        .find_map(|file| {
            file.manifest
                .defaults
                .as_ref()
                .and_then(|defaults| defaults.clients.clone())
        })
        .unwrap_or(ClientSelection::AllDetected)
}

/// Every server name of `project`, merged and resolved. `servers` are the
/// store's valid servers (enabled system-wide or not); a `from` base gets
/// its `[os.<os>]` overrides and the machine `overlay` first, as at system
/// scope.
pub fn project_entries(
    project: &Project,
    servers: &BTreeMap<String, Server>,
    env: &Env,
    overlay: Option<&MachineOverlay>,
) -> Vec<ProjectEntry> {
    let defaults = default_clients(project);
    let shared_defaults = project
        .shared
        .as_ref()
        .and_then(|file| file.manifest.defaults.as_ref())
        .and_then(|defaults| defaults.clients.clone())
        .unwrap_or(ClientSelection::AllDetected);
    let layer_servers = |file: Option<&ManifestFile>| {
        file.and_then(|file| file.manifest.servers.clone())
            .unwrap_or_default()
    };
    let shared = layer_servers(project.shared.as_ref());
    let local = layer_servers(project.local.as_ref());
    let names: BTreeSet<&String> = shared.keys().chain(local.keys()).collect();
    let resolver = Resolver {
        servers,
        env,
        overlay,
    };
    let shared_path = project.shared.as_ref().map(|file| file.path.as_path());
    let local_path = project.local.as_ref().map(|file| file.path.as_path());
    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        let (shared_table, local_table) = (shared.get(name), local.get(name));
        let mut entry = match local_table {
            Some(_) => resolver.entry(
                name,
                EntryLayer::Local,
                [shared_table, local_table],
                &defaults,
                local_path,
            ),
            None => resolver.entry(
                name,
                EntryLayer::Shared,
                [shared_table, None],
                &defaults,
                shared_path,
            ),
        };
        if local_table.is_some() && shared_table.is_some() {
            entry.shared = Some(Box::new(resolver.entry(
                name,
                EntryLayer::Shared,
                [shared_table, None],
                &shared_defaults,
                shared_path,
            )));
        }
        entries.push(entry);
    }
    entries
}

/// Resolves merged entries against the store for this machine.
struct Resolver<'a> {
    servers: &'a BTreeMap<String, Server>,
    env: &'a Env,
    overlay: Option<&'a MachineOverlay>,
}

impl Resolver<'_> {
    /// The entry `name` from `layers` (shared, local) merged in order;
    /// errors name `path`.
    fn entry(
        &self,
        name: &str,
        layer: EntryLayer,
        layers: [Option<&ProjectServer>; 2],
        defaults: &ClientSelection,
        path: Option<&std::path::Path>,
    ) -> ProjectEntry {
        let merged = match merge_layers(layers[0], layers[1]) {
            Ok(merged) => merged,
            Err(err) => {
                return ProjectEntry {
                    name: name.to_owned(),
                    layer,
                    clients: defaults.clone(),
                    from: None,
                    unset: Vec::new(),
                    enabled: true,
                    body: EntryBody::Server(Err(with_file(err, path))),
                    shared: None,
                };
            }
        };
        let body = if merged.is_suppression() {
            EntryBody::Suppression
        } else {
            EntryBody::Server(
                resolve(name, &merged, self.servers, self.env, self.overlay)
                    .map(Box::new)
                    .map_err(|err| with_file(err, path)),
            )
        };
        ProjectEntry {
            name: name.to_owned(),
            layer,
            clients: merged.clients.clone().unwrap_or_else(|| defaults.clone()),
            from: merged.from.clone(),
            unset: merged.unset.clone().unwrap_or_default(),
            enabled: merged.enabled != Some(false),
            body,
            shared: None,
        }
    }
}

/// How `spec` hides a user server in a project; `cannot_suppress_in_project`
/// when it has no way to (SPEC 6.3).
pub fn suppression_mechanism(
    spec: &AdapterSpec,
    server: &str,
) -> Result<ProjectDisable, CoreError> {
    spec.mcp
        .as_ref()
        .and_then(|mcp| mcp.caps.project_disable)
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::CannotSuppressInProject,
                format!(
                    "{} cannot hide the system server {server:?} in a project; remove {} from \
                     the system enable list and add the server to the projects that need it",
                    spec.display_name, spec.id
                ),
            )
        })
}

fn with_file(err: CoreError, path: Option<&std::path::Path>) -> CoreError {
    match (err.path(), path) {
        (None, Some(path)) => err.with_path(path),
        _ => err,
    }
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<toml::Value, CoreError> {
    toml::Value::try_from(value).map_err(|err| {
        CoreError::new(
            ErrorCode::InvalidValue,
            format!("cannot merge the project entry: {err}"),
        )
    })
}

fn from_value<T: serde::de::DeserializeOwned>(value: toml::Value) -> Result<T, CoreError> {
    value.try_into().map_err(|err: toml::de::Error| {
        CoreError::new(
            ErrorCode::InvalidValue,
            format!("the merged project entry is not valid: {}", err.message()),
        )
    })
}

/// The local table merged over the shared one.
fn merge_layers(
    shared: Option<&ProjectServer>,
    local: Option<&ProjectServer>,
) -> Result<ProjectServer, CoreError> {
    let mut merged = toml::Value::Table(toml::Table::new());
    for layer in [shared, local].into_iter().flatten() {
        deep_merge(&mut merged, &to_value(layer)?);
    }
    from_value(merged)
}

/// The definition `merged` stands for: its `from` base (resolved for this
/// machine), the entry's fields over it, the entry's own `[os.<os>]` over
/// those, then `unset`.
fn resolve(
    name: &str,
    merged: &ProjectServer,
    servers: &BTreeMap<String, Server>,
    env: &Env,
    overlay: Option<&MachineOverlay>,
) -> Result<Server, CoreError> {
    let mut value = match &merged.from {
        Some(from) => {
            let base = servers.get(from).ok_or_else(|| {
                CoreError::new(
                    ErrorCode::UnknownTemplate,
                    format!(
                        "{}: no valid store server {from:?} to start from",
                        join(&join("servers", name), "from")
                    ),
                )
            })?;
            to_value(&resolved_body(base, env, overlay))?
        }
        None => toml::Value::Table(toml::Table::new()),
    };
    let mut body = merged.body.clone();
    let os_override = body
        .os
        .take()
        .and_then(|mut overrides| overrides.remove(env.os().as_str()));
    deep_merge(&mut value, &to_value(&body)?);
    if let Some(over) = os_override {
        deep_merge(&mut value, &to_value(&over.fields)?);
    }
    for path in merged.unset.iter().flatten() {
        remove_path(&mut value, path);
    }
    let body: ServerBody = from_value(value)?;
    let mut d = Diagnostics::new();
    body.validate_into(&join("servers", name), true, &mut d);
    if let Some(err) = d.first_error(None) {
        return Err(err);
    }
    let mut server = Server::new(name);
    server.body = body;
    Ok(server)
}

/// `server`'s body with `[os.<os>]` and the machine overlay applied, and no
/// OS overrides left.
fn resolved_body(server: &Server, env: &Env, overlay: Option<&MachineOverlay>) -> ServerBody {
    let fields = effective_fields(server, env, overlay);
    ServerBody {
        transport: fields.transport,
        command: fields.command,
        args: fields.args,
        cwd: fields.cwd,
        url: fields.url,
        headers: fields.headers,
        env: fields.env,
        os: None,
        ..server.body.clone()
    }
}

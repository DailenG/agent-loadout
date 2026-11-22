//! Desired state (SPEC 6.1 system layer) and its rendering into one
//! [`RenderedServer`] per server and client.
//!
//! Rendering resolves OS overrides and the machine overlay, fills path
//! variables and delivers every reference through
//! [`crate::secrets::render`] (SPEC 11.4): `env` in the client's own
//! syntax, `launch` through `loadout exec`, `command` through the client's
//! command hooks, or `inline`. A reference with no allowed mode blocks the
//! entry; nothing is ever written in plain text in its place.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use crate::adapters::caps::{Caps, EnvRefSyntax, FieldSet};
use crate::adapters::detect::{RegistryProbe, detect};
use crate::adapters::mapper::{
    Launch, MapError, RemoteLaunch, RenderedServer, StdioLaunch, ToolLists,
};
use crate::adapters::{ADAPTERS, AdapterSpec, adapter};
use crate::env::Env;
use crate::error::{CoreError, ErrorCode};
use crate::formats::{NativeMap, NativeValue};
use crate::fs::Fs;
use crate::model::TomlDocument;
use crate::model::clients::{
    ClientPrefs, ClientPrefsFile, ClientSelection, NATIVE_NAME_KEY, known_client,
};
use crate::model::machine::MachineOverlay;
use crate::model::server::{DeliveryMode, Server, ServerFields, Transport};
use crate::model::settings::{ApplyMode, Settings};
use crate::secrets::SecretValue;
use crate::secrets::catalog::Catalog;
use crate::secrets::delivery::{DeliveryEnv, Mode, Policy, Sharing};
use crate::secrets::grammar::{PathVar, TemplatePart, parse_template};
use crate::secrets::render::{EnvValue, HeaderValue, RenderEnv, Renderer};
use crate::secrets::scanner::SecretMemory;
use crate::store::StoreRead;

/// Everything the system layer is computed from, read from one view of the
/// store (working tree, transaction or past revision).
#[derive(Debug, Clone)]
pub struct DesiredState {
    /// Valid servers by id.
    pub servers: BTreeMap<String, Server>,
    /// Server files that do not load or validate, by id. Entries owned for
    /// these servers are left alone (never removed) until the file is fixed.
    pub invalid: BTreeMap<String, CoreError>,
    pub clients: ClientPrefsFile,
    /// This machine's overlay (`machines/<machine-id>.toml`).
    pub overlay: Option<MachineOverlay>,
    pub settings: Settings,
    /// `secrets.toml` with this machine's overlay applied.
    pub catalog: Catalog,
}

impl DesiredState {
    /// Loads the desired state. A broken `clients.toml`, `settings.toml`,
    /// `secrets.toml` or machine overlay fails the whole load, since it
    /// changes every target; a broken server file only marks that server
    /// [`DesiredState::invalid`].
    pub fn load(src: &dyn StoreRead, machine_id: Option<&str>) -> Result<Self, CoreError> {
        let mut servers = BTreeMap::new();
        let mut invalid = BTreeMap::new();
        for name in src.list_files("servers")? {
            let Some(id) = name.strip_suffix(".toml") else {
                continue;
            };
            let relative = format!("servers/{name}");
            let loaded = load_model::<Server>(src, &relative).and_then(|server| {
                let server = server.expect("listed files exist");
                match server
                    .validate(Some(id))
                    .first_error(Some(Path::new(&relative)))
                {
                    Some(err) => Err(err),
                    None => Ok(server),
                }
            });
            match loaded {
                Ok(server) => {
                    servers.insert(id.to_owned(), server);
                }
                Err(err) => {
                    invalid.insert(id.to_owned(), err);
                }
            }
        }
        let clients = match load_model::<ClientPrefsFile>(src, "clients.toml")? {
            Some(file) => checked(
                file.validate().first_error(Some(Path::new("clients.toml"))),
                file,
            )?,
            None => ClientPrefsFile::new(),
        };
        let settings = match load_model::<Settings>(src, "settings.toml")? {
            Some(file) => checked(
                file.validate()
                    .first_error(Some(Path::new("settings.toml"))),
                file,
            )?,
            None => Settings::new(),
        };
        let overlay = match machine_id {
            Some(id) => {
                let relative = format!("machines/{id}.toml");
                match load_model::<MachineOverlay>(src, &relative)? {
                    Some(file) => Some(checked(
                        file.validate(Some(id))
                            .first_error(Some(Path::new(&relative))),
                        file,
                    )?),
                    None => None,
                }
            }
            None => None,
        };
        let catalog = Catalog::load(src, machine_id)?;
        Ok(Self {
            servers,
            invalid,
            clients,
            overlay,
            settings,
            catalog,
        })
    }

    /// No servers and default preferences: a machine without a store.
    pub fn empty() -> Self {
        Self {
            servers: BTreeMap::new(),
            invalid: BTreeMap::new(),
            clients: ClientPrefsFile::new(),
            overlay: None,
            settings: Settings::new(),
            catalog: Catalog::default(),
        }
    }

    /// `apply_mode` from `settings.toml`; instant by default (SPEC 5.7).
    pub fn apply_mode(&self) -> ApplyMode {
        self.settings.apply_mode.unwrap_or(ApplyMode::Instant)
    }

    /// Whether this machine exports provider secrets to the environment
    /// (`[delivery] export_provider_secrets` in the overlay).
    pub fn export_provider_secrets(&self) -> bool {
        self.overlay
            .as_ref()
            .and_then(|overlay| overlay.delivery.as_ref())
            .and_then(|delivery| delivery.export_provider_secrets)
            == Some(true)
    }

    /// The `clients.toml` table of a client (empty when absent).
    pub fn prefs(&self, client: &str) -> ClientPrefs {
        self.clients
            .clients
            .get(client)
            .cloned()
            .unwrap_or_default()
    }

    /// Whether this machine's overlay excludes `client` or `clients.toml`
    /// opts it out.
    pub(crate) fn excluded(&self, client: &str) -> bool {
        let by_machine = self
            .overlay
            .as_ref()
            .and_then(|overlay| overlay.enable.as_ref())
            .and_then(|enable| enable.exclude_clients.as_ref())
            .is_some_and(|clients| clients.iter().any(|id| id == client));
        let opted_out = self
            .clients
            .clients
            .get(client)
            .and_then(|prefs| prefs.enabled)
            == Some(false);
        by_machine || opted_out
    }

    /// Whether this machine released `client`'s entry of server `id`
    /// (overlay `[enable.released]`): it is not written or owned here.
    pub fn released(&self, id: &str, client: &str) -> bool {
        self.overlay
            .as_ref()
            .and_then(|overlay| overlay.enable.as_ref())
            .and_then(|enable| enable.released.as_ref())
            .and_then(|released| released.get(id))
            .is_some_and(|clients| clients.iter().any(|c| c == client))
    }
}

fn load_model<T: crate::model::StoreModel>(
    src: &dyn StoreRead,
    relative: &str,
) -> Result<Option<T>, CoreError> {
    let Some(text) = src.read_file(relative)? else {
        return Ok(None);
    };
    TomlDocument::<T>::parse(&text)
        .map(|doc| Some(doc.into_value()))
        .map_err(|diagnostic| diagnostic.to_core_error(Some(Path::new(relative))))
}

fn checked<T>(error: Option<CoreError>, value: T) -> Result<T, CoreError> {
    match error {
        Some(err) => Err(err),
        None => Ok(value),
    }
}

/// One server as one client should have it.
#[derive(Debug, Clone)]
pub struct DesiredEntry {
    pub client: &'static str,
    pub server_id: String,
    /// The native entry key (`native_name` or the server id).
    pub name: String,
    /// The rendered server, or why it cannot be written (the entry is
    /// `blocked`).
    pub server: Result<RenderedServer, CoreError>,
    /// The entry's delivery mode (`env`, `command`, `launch`, `inline`)
    /// when it holds references.
    pub delivery: Option<&'static str>,
    /// Values written in plain text (`inline`), masked in diffs.
    pub inline: Vec<SecretValue>,
    /// Delivery warnings (`inline_env_fallback`).
    pub warnings: Vec<CoreError>,
    /// A native entry written as it is instead of mapping
    /// [`DesiredEntry::server`] (a project suppression entry such as Codex
    /// `{ enabled = false }`).
    pub native: Option<NativeValue>,
}

/// The system layer for this machine.
#[derive(Debug, Clone, Default)]
pub struct Rendering {
    pub entries: Vec<DesiredEntry>,
    /// Clients that get at least one entry, in adapter table order.
    pub clients: Vec<&'static str>,
    /// Clients named in an enable list that cannot be written here
    /// (`no_adapter`, `unknown_client`, `not_supported_on_os`).
    pub warnings: Vec<CoreError>,
}

/// Where rendering happens: the paths `${home}` and `${loadout_home}` stand
/// for, what detection may look at, this machine's secret providers and the
/// launcher, and the store's [`SecretMemory`] (values read while rendering
/// are recorded there, so the store refuses to commit them).
pub struct RenderCx<'a> {
    pub env: &'a Env,
    pub fs: &'a dyn Fs,
    pub loadout_home: &'a Path,
    pub registry: &'a dyn RegistryProbe,
    pub delivery: &'a DeliveryEnv,
    pub memory: Arc<SecretMemory>,
}

/// Renders every enabled store server for every client it is enabled for
/// at system scope (user files), after machine exclusions and
/// `clients.toml` opt-outs.
pub fn system_entries(cx: &RenderCx<'_>, desired: &DesiredState) -> Rendering {
    let mut rendering = Rendering::default();
    let mut warned: BTreeSet<String> = BTreeSet::new();
    let mut detected = Detected::default();
    for (id, server) in &desired.servers {
        for client in detected.system_clients(cx, desired, server) {
            let spec = match writable(&client, cx.env) {
                Ok(spec) => spec,
                Err(err) => {
                    if warned.insert(client.clone()) {
                        rendering.warnings.push(err);
                    }
                    continue;
                }
            };
            if rendering
                .entries
                .iter()
                .any(|e| e.client == spec.id && e.server_id == *id)
            {
                continue;
            }
            let name = native_name(server, spec.id);
            let entry = match render(cx, desired, server, spec, Sharing::Private) {
                Ok(out) => DesiredEntry {
                    client: spec.id,
                    server_id: id.clone(),
                    name,
                    server: Ok(out.server),
                    delivery: out.delivery.map(Mode::as_str),
                    inline: out.inline,
                    warnings: out.warnings,
                    native: None,
                },
                Err(err) => DesiredEntry {
                    client: spec.id,
                    server_id: id.clone(),
                    name,
                    server: Err(err),
                    delivery: None,
                    inline: Vec::new(),
                    warnings: Vec::new(),
                    native: None,
                },
            };
            rendering.entries.push(entry);
        }
    }
    rendering.clients = ADAPTERS
        .iter()
        .map(|spec| spec.id)
        .filter(|id| rendering.entries.iter().any(|entry| entry.client == *id))
        .collect();
    rendering
}

/// Detection results, probed once per client and run.
#[derive(Default)]
pub struct Detected(BTreeMap<&'static str, bool>);

impl Detected {
    /// The clients `server` is written for at system scope on this machine,
    /// as [`system_entries`] picks them: its selection resolved by
    /// [`Detected::select`] (forced on in `clients.toml`, detected, fitting
    /// its transport), minus machine exclusions, entries this machine
    /// released and `clients.toml` opt-outs.
    /// Empty when the server names no clients.
    pub fn system_clients(
        &mut self,
        cx: &RenderCx<'_>,
        desired: &DesiredState,
        server: &Server,
    ) -> Vec<String> {
        let Some(selection) = server.enable.as_ref().and_then(|e| e.clients.as_ref()) else {
            return Vec::new();
        };
        let transport = server_transport(server, cx.env);
        let mut clients = self.select(cx, desired, selection, transport);
        clients.retain(|client| !desired.excluded(client) && !desired.released(&server.id, client));
        clients
    }

    /// The client ids `selection` names: the list as written, or for
    /// `all-detected` every client forced on in `clients.toml` or detected
    /// whose MCP support fits `transport` on this OS (never a read-only
    /// one).
    pub(crate) fn select(
        &mut self,
        cx: &RenderCx<'_>,
        desired: &DesiredState,
        selection: &ClientSelection,
        transport: Option<Transport>,
    ) -> Vec<String> {
        match selection {
            ClientSelection::List(ids) => ids.clone(),
            ClientSelection::AllDetected => ADAPTERS
                .iter()
                .filter(|spec| {
                    let fits = spec.mcp.as_ref().is_some_and(|mcp| {
                        !mcp.read_only
                            && mcp.oses.contains(&cx.env.os())
                            && transport.is_some_and(|t| mcp.caps.transports.contains(t))
                    });
                    fits && self.detected(cx, desired, spec)
                })
                .map(|spec| spec.id.to_owned())
                .collect(),
        }
    }

    /// Whether `spec` is forced on in `clients.toml` or detected here.
    pub(crate) fn detected(
        &mut self,
        cx: &RenderCx<'_>,
        desired: &DesiredState,
        spec: &'static AdapterSpec,
    ) -> bool {
        let forced = desired.prefs(spec.id).enabled == Some(true);
        forced
            || *self
                .0
                .entry(spec.id)
                .or_insert_with(|| detect(spec, cx.env, cx.fs, cx.registry).detected())
    }
}

/// The adapter of `client`, if Loadout can write its user file here.
/// A read-only client (`vscode-legacy`) is `read_only_client`.
pub(crate) fn writable(client: &str, env: &Env) -> Result<&'static AdapterSpec, CoreError> {
    let Some(spec) = adapter(client) else {
        let code = if known_client(client).is_some() {
            ErrorCode::NoAdapter
        } else {
            ErrorCode::UnknownClient
        };
        return Err(CoreError::new(
            code,
            format!("{client:?} has no adapter in this build; nothing is written for it"),
        ));
    };
    match &spec.mcp {
        Some(mcp) if mcp.read_only => Err(CoreError::new(
            ErrorCode::ReadOnlyClient,
            format!(
                "{} is read and imported only; nothing is written for it (enable `copilot` \
                 for VS Code instead)",
                spec.display_name
            ),
        )),
        Some(mcp) if mcp.oses.contains(&env.os()) => Ok(spec),
        _ => Err(CoreError::new(
            ErrorCode::NotSupportedOnOs,
            format!(
                "{} has no MCP config on {}",
                spec.display_name,
                env.os().as_str()
            ),
        )),
    }
}

/// The native entry key of `server` for `client`.
pub fn native_name(server: &Server, client: &str) -> String {
    server
        .body
        .client
        .as_ref()
        .and_then(|clients| clients.get(client))
        .and_then(|table| table.get(NATIVE_NAME_KEY))
        .and_then(toml::Value::as_str)
        .map_or_else(|| server.id.clone(), str::to_owned)
}

pub(crate) fn server_transport(server: &Server, env: &Env) -> Option<Transport> {
    effective_fields(server, env, None).transport
}

/// The launch fields after `[os.<os>]` and the machine overlay.
pub(crate) fn effective_fields(
    server: &Server,
    env: &Env,
    overlay: Option<&MachineOverlay>,
) -> ServerFields {
    let mut fields = server.body.fields();
    if let Some(over) = server
        .body
        .os
        .as_ref()
        .and_then(|overrides| overrides.get(env.os().as_str()))
    {
        fields = fields.merged(&over.fields);
    }
    if let Some(over) = overlay
        .and_then(|overlay| overlay.servers.as_ref())
        .and_then(|servers| servers.get(&server.id))
    {
        fields = fields.merged(&over.fields);
    }
    fields
}

/// One server rendered for one client.
#[derive(Debug, Clone)]
pub struct RenderOutput {
    pub server: RenderedServer,
    /// The entry's most consequential delivery mode, if it holds references.
    pub delivery: Option<Mode>,
    /// Values written in plain text (`inline`).
    pub inline: Vec<SecretValue>,
    pub warnings: Vec<CoreError>,
}

/// How a server is rendered: the [`Sharing`] of the target file, the
/// project folder `${project}` stands for, and whether the machine overlay
/// still applies (a project entry was resolved with it already).
#[derive(Debug, Clone, Copy)]
pub struct RenderTarget<'a> {
    pub sharing: Sharing,
    pub project: Option<&'a Path>,
    pub overlay: bool,
}

/// One server for one client and a target of the given [`Sharing`].
pub fn render(
    cx: &RenderCx<'_>,
    desired: &DesiredState,
    server: &Server,
    spec: &AdapterSpec,
    sharing: Sharing,
) -> Result<RenderOutput, CoreError> {
    render_for(
        cx,
        desired,
        server,
        spec,
        RenderTarget {
            sharing,
            project: None,
            overlay: true,
        },
    )
}

/// One server for one client and `target`.
pub fn render_for(
    cx: &RenderCx<'_>,
    desired: &DesiredState,
    server: &Server,
    spec: &AdapterSpec,
    target: RenderTarget<'_>,
) -> Result<RenderOutput, CoreError> {
    let sharing = target.sharing;
    let mcp = spec.mcp.as_ref().ok_or_else(|| {
        CoreError::new(
            ErrorCode::ScopeNotSupported,
            format!("{} has no MCP config", spec.display_name),
        )
    })?;
    let overlay = desired.overlay.as_ref().filter(|_| target.overlay);
    let fields = effective_fields(server, cx.env, overlay);
    let transport = fields.transport.ok_or_else(|| {
        CoreError::new(
            ErrorCode::MissingField,
            "`transport` is required: stdio, http or sse",
        )
    })?;
    // A transport the client cannot run blocks the entry before delivery
    // is considered (the mapper reports the same error).
    if !mcp.caps.transports.contains(transport) {
        return Err(MapError::UnsupportedTransport {
            transport: transport.as_str().to_owned(),
        }
        .into());
    }
    let rx = RenderEnv {
        env: cx.env,
        fs: cx.fs,
        loadout_home: cx.loadout_home,
        catalog: &desired.catalog,
        delivery: cx.delivery,
        memory: &cx.memory,
        project: target.project,
    };
    let policy = Policy {
        requested: server
            .body
            .delivery
            .as_ref()
            .and_then(|delivery| delivery.mode)
            .unwrap_or(DeliveryMode::Auto),
        caps: &mcp.caps,
        client: spec.display_name,
        sharing,
        export_provider_secrets: desired.export_provider_secrets(),
    };
    let mut r = Renderer::new(rx, policy, transport);
    let launch = match transport {
        Transport::Stdio => {
            let command = fields.command.clone().ok_or_else(|| {
                CoreError::new(ErrorCode::MissingField, "stdio servers need `command`")
            })?;
            let mut stdio = StdioLaunch {
                command: r.value(&command, FieldSet::COMMAND)?,
                ..StdioLaunch::default()
            };
            for arg in fields.args.iter().flatten() {
                stdio.args.push(r.value(arg, FieldSet::ARGS)?);
            }
            for (key, value) in fields.env.iter().flatten() {
                match r.env_value(key, value)? {
                    EnvValue::Value(value) => {
                        stdio.env.insert(key.clone(), value);
                    }
                    EnvValue::Forward(name) => stdio.forward_env.push(name),
                    EnvValue::Launched => {}
                }
            }
            if let Some(cwd) = &fields.cwd {
                stdio.cwd = Some(r.value(cwd, FieldSet::CWD)?);
            }
            r.finish_stdio(&mut stdio)?;
            Launch::Stdio(stdio)
        }
        Transport::Http | Transport::Sse => {
            let url = fields.url.clone().ok_or_else(|| {
                CoreError::new(ErrorCode::MissingField, "remote servers need `url`")
            })?;
            let mut remote = RemoteLaunch {
                url: r.value(&url, FieldSet::URL)?,
                ..RemoteLaunch::default()
            };
            for (key, value) in fields.headers.iter().flatten() {
                match r.header(key, value)? {
                    HeaderValue::Value(value) => {
                        remote.headers.insert(key.clone(), value);
                    }
                    HeaderValue::Bearer(name) => remote.bearer_env = Some(name),
                    HeaderValue::FromEnv(name) => {
                        remote.headers_env.insert(key.clone(), name);
                    }
                    HeaderValue::Helper => {}
                }
            }
            r.finish_remote(&mut remote)?;
            match transport {
                Transport::Http => Launch::Http(remote),
                _ => Launch::Sse(remote),
            }
        }
    };
    let mut passthrough = NativeMap::new();
    if let Some(table) = server
        .body
        .client
        .as_ref()
        .and_then(|clients| clients.get(spec.id))
    {
        for (key, value) in table {
            if key != NATIVE_NAME_KEY {
                passthrough.insert(key.clone(), native(&mut r, value)?);
            }
        }
    }
    let tools = server
        .body
        .tools
        .as_ref()
        .map(|tools| ToolLists {
            allow: tools.allow.clone(),
            deny: tools.deny.clone(),
        })
        .unwrap_or_default();
    let mut warnings = std::mem::take(&mut r.warnings);
    if sharing == Sharing::Shared
        && target.project.is_some()
        && uses_project_path(&fields, server, spec.id)
    {
        warnings.push(CoreError::new(
            ErrorCode::PersonalPathInSharedFile,
            format!(
                "`${{project}}` in {:?} is written as this machine's folder into a shared \
                 file; teammates get a path that is not theirs",
                server.id
            ),
        ));
    }
    Ok(RenderOutput {
        server: RenderedServer {
            name: native_name(server, spec.id),
            launch,
            enabled: true,
            tools,
            passthrough,
        },
        delivery: r.delivery(),
        inline: std::mem::take(&mut r.inline),
        warnings,
    })
}

/// Whether a launch field (`fields`) or `client` passthrough value of
/// `server` uses `${project}`.
fn uses_project_path(fields: &ServerFields, server: &Server, client: &str) -> bool {
    fn has(text: &str) -> bool {
        parse_template(text).is_ok_and(|parts| {
            parts
                .iter()
                .any(|part| matches!(part, TemplatePart::Var(PathVar::Project)))
        })
    }
    fn in_value(value: &toml::Value) -> bool {
        match value {
            toml::Value::String(text) => has(text),
            toml::Value::Array(items) => items.iter().any(in_value),
            toml::Value::Table(table) => table.values().any(in_value),
            _ => false,
        }
    }
    let in_fields = fields
        .command
        .iter()
        .chain(fields.cwd.iter())
        .chain(fields.url.iter())
        .chain(fields.args.iter().flatten())
        .chain(fields.env.iter().flat_map(|env| env.values()))
        .chain(fields.headers.iter().flat_map(|headers| headers.values()))
        .any(|text| has(text));
    in_fields
        || server
            .body
            .client
            .as_ref()
            .and_then(|clients| clients.get(client))
            .is_some_and(|table| table.values().any(in_value))
}

/// A passthrough value; strings may hold literals, path variables and
/// references.
fn native(r: &mut Renderer<'_>, value: &toml::Value) -> Result<NativeValue, CoreError> {
    Ok(match value {
        toml::Value::String(text) => NativeValue::String(r.value(text, FieldSet::NONE)?),
        toml::Value::Integer(n) => NativeValue::Integer(*n),
        toml::Value::Float(n) => NativeValue::Float(*n),
        toml::Value::Boolean(b) => NativeValue::Bool(*b),
        toml::Value::Datetime(d) => NativeValue::Datetime(d.to_string()),
        toml::Value::Array(items) => NativeValue::Array(
            items
                .iter()
                .map(|item| native(r, item))
                .collect::<Result<_, _>>()?,
        ),
        toml::Value::Table(table) => {
            let mut map = NativeMap::new();
            for (key, item) in table {
                map.insert(key.clone(), native(r, item)?);
            }
            NativeValue::Object(map)
        }
    })
}

/// A native string value as a store template (SPEC 11.2): the client's own
/// variable references become `${env:NAME}`, any other `${` becomes the
/// literal `$${`. `None` when the value uses a client feature the store
/// cannot express (`${NAME:-default}`, `${userHome}`, OpenCode's
/// `{file:path}`, a Pi `!command`).
pub fn native_to_template(text: &str, caps: &Caps, field: FieldSet) -> Option<String> {
    let syntax = caps
        .env_ref
        .filter(|_| caps.env_ref_fields.contains(field) && field != FieldSet::NONE);
    match syntax {
        Some(EnvRefSyntax::DollarBrace { bare: true, .. }) => bare_dollar_to_template(text),
        Some(EnvRefSyntax::DollarBrace { .. }) => convert_refs(text, "${", |token| {
            env_name(token).then(|| format!("${{env:{token}}}"))
        }),
        Some(EnvRefSyntax::Dollar) => dollar_to_template(text),
        Some(EnvRefSyntax::DollarBraceEnv) => convert_refs(text, "${", |token| {
            token
                .strip_prefix("env:")
                .filter(|name| env_name(name))
                .map(|name| format!("${{env:{name}}}"))
        }),
        Some(EnvRefSyntax::BraceEnv) => brace_env_to_template(text),
        // Codex reads forwarded variables back as `${env:NAME}` already.
        Some(EnvRefSyntax::CodexForward) => match parse_template(text) {
            Ok(parts)
                if parts
                    .iter()
                    .all(|p| matches!(p, TemplatePart::Literal(_) | TemplatePart::Env(_))) =>
            {
                Some(text.to_owned())
            }
            _ => Some(text.replace("${", "$${")),
        },
        None => Some(text.replace("${", "$${")),
    }
}

fn env_name(token: &str) -> bool {
    crate::model::ids::is_valid(crate::model::ids::IdKind::EnvVar, token)
}

/// A value of a client expanding `${NAME}` and a bare `$NAME` with no
/// escape (Gemini, Qwen Code) as a store template: both are references,
/// any other `$` is literal. `None` for a `${...}` that is not a variable
/// name and for a literal `$` right before a reference, which the store
/// cannot spell.
fn bare_dollar_to_template(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut literal = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        literal.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (name, consumed) = if let Some(braced) = after.strip_prefix('{') {
            match braced.find('}') {
                Some(end) => (&braced[..end], end + 2),
                // An unterminated `${` is literal text.
                None => ("", 0),
            }
        } else {
            let len = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            (&after[..len], len)
        };
        if name.is_empty() || (!after.starts_with('{') && !env_name(name)) {
            literal.push('$');
            rest = after;
            continue;
        }
        if !env_name(name) || literal.ends_with('$') {
            return None;
        }
        out.push_str(&literal.replace("${", "$${"));
        literal.clear();
        out.push_str(&format!("${{env:{name}}}"));
        rest = &after[consumed..];
    }
    literal.push_str(rest);
    out.push_str(&literal.replace("${", "$${"));
    Some(out)
}

/// An OpenCode value as a store template: `{env:NAME}` is a reference and
/// `${` literal text. `None` for `{file:path}` (no store form) and for a
/// literal `$` right before a reference.
fn brace_env_to_template(text: &str) -> Option<String> {
    if text.contains("{file:") {
        return None;
    }
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{env:") {
        let literal = &rest[..start];
        if literal.ends_with('$') {
            return None;
        }
        out.push_str(&literal.replace("${", "$${"));
        let after = &rest[start + "{env:".len()..];
        let end = after.find('}')?;
        let name = &after[..end];
        if !env_name(name) {
            return None;
        }
        out.push_str(&format!("${{env:{name}}}"));
        rest = &after[end + 1..];
    }
    out.push_str(&rest.replace("${", "$${"));
    Some(out)
}

/// A Pi `env` or `headers` value as a store template, read the way Pi reads
/// it (`resolve-config-value`): `$$` and `$!` are a literal `$` and `!`,
/// `${NAME}` and a bare `$NAME` are references, and any other `$` is
/// literal. `None` for a value starting with `!` (Pi runs it as a command),
/// and for a literal `$` right before a reference, which the store cannot
/// spell (`$${` is a literal `${`).
fn dollar_to_template(text: &str) -> Option<String> {
    fn pi_name_len(text: &str) -> usize {
        match text.as_bytes().first() {
            Some(first) if first.is_ascii_alphabetic() || *first == b'_' => text
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(text.len()),
            _ => 0,
        }
    }
    if text.starts_with('!') {
        return None;
    }
    let mut out = String::new();
    let mut literal = String::new();
    let reference = |out: &mut String, literal: &mut String, name: &str| {
        if literal.ends_with('$') || !env_name(name) {
            return false;
        }
        out.push_str(&literal.replace("${", "$${"));
        literal.clear();
        out.push_str(&format!("${{env:{name}}}"));
        true
    };
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        literal.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        match after.as_bytes().first() {
            Some(b'$' | b'!') => {
                literal.push_str(&after[..1]);
                rest = &after[1..];
            }
            Some(b'{') => match after[1..].find('}') {
                Some(end) => {
                    let name = &after[1..1 + end];
                    if !name.is_empty() && pi_name_len(name) == name.len() {
                        if !reference(&mut out, &mut literal, name) {
                            return None;
                        }
                    } else {
                        literal.push('$');
                        literal.push_str(&after[..end + 2]);
                    }
                    rest = &after[end + 2..];
                }
                None => {
                    literal.push('$');
                    rest = after;
                }
            },
            _ => match pi_name_len(after) {
                0 => {
                    literal.push('$');
                    rest = after;
                }
                len => {
                    if !reference(&mut out, &mut literal, &after[..len]) {
                        return None;
                    }
                    rest = &after[len..];
                }
            },
        }
    }
    literal.push_str(rest);
    out.push_str(&literal.replace("${", "$${"));
    Some(out)
}

/// Replaces every `<open>TOKEN}` in `text` through `convert`; `None` when a
/// token cannot be converted or is unterminated.
fn convert_refs(
    text: &str,
    open: &str,
    convert: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        out.push_str(&rest[..start]);
        let after = &rest[start + open.len()..];
        let end = after.find('}')?;
        out.push_str(&convert(&after[..end])?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

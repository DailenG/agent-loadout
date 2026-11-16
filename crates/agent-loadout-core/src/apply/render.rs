//! Desired state (SPEC 6.1 system layer) and its rendering into one
//! [`RenderedServer`] per server and client.
//!
//! Rendering resolves OS overrides and the machine overlay, fills path
//! variables and writes `${env:NAME}` references in the client's own syntax
//! where the client expands that field (`env` delivery, SPEC 11.4 rule 1).
//! Every other reference blocks the entry with `delivery_unavailable`: the
//! `launch`, `command` and `inline` modes and `${secret:..}` arrive with
//! the secrets milestones, and nothing is ever written in plain text in
//! their place.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::adapters::caps::{Caps, EnvRefSyntax, FieldSet};
use crate::adapters::detect::{RegistryProbe, detect};
use crate::adapters::mapper::{Launch, RemoteLaunch, RenderedServer, StdioLaunch, ToolLists};
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
use crate::secrets::grammar::{PathVar, TemplatePart, parse_template};
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
}

impl DesiredState {
    /// Loads the desired state. A broken `clients.toml`, `settings.toml` or
    /// machine overlay fails the whole load, since it changes every target;
    /// a broken server file only marks that server [`DesiredState::invalid`].
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
        Ok(Self {
            servers,
            invalid,
            clients,
            overlay,
            settings,
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
        }
    }

    /// `apply_mode` from `settings.toml`; instant by default (SPEC 5.7).
    pub fn apply_mode(&self) -> ApplyMode {
        self.settings.apply_mode.unwrap_or(ApplyMode::Instant)
    }

    /// The `clients.toml` table of a client (empty when absent).
    pub fn prefs(&self, client: &str) -> ClientPrefs {
        self.clients
            .clients
            .get(client)
            .cloned()
            .unwrap_or_default()
    }

    fn excluded(&self, client: &str) -> bool {
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
    /// `env` when the entry holds client-expanded references.
    pub delivery: Option<&'static str>,
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
/// for, and what detection may look at.
pub struct RenderCx<'a> {
    pub env: &'a Env,
    pub fs: &'a dyn Fs,
    pub loadout_home: &'a Path,
    pub registry: &'a dyn RegistryProbe,
}

/// Renders every enabled store server for every client it is enabled for
/// at system scope (user files), after machine exclusions and
/// `clients.toml` opt-outs.
pub fn system_entries(cx: &RenderCx<'_>, desired: &DesiredState) -> Rendering {
    let mut rendering = Rendering::default();
    let mut warned: BTreeSet<String> = BTreeSet::new();
    let mut detected: BTreeMap<&'static str, bool> = BTreeMap::new();
    for (id, server) in &desired.servers {
        let Some(selection) = server.enable.as_ref().and_then(|e| e.clients.as_ref()) else {
            continue;
        };
        let transport = server_transport(server, cx.env);
        let clients: Vec<String> = match selection {
            ClientSelection::List(ids) => ids.clone(),
            ClientSelection::AllDetected => ADAPTERS
                .iter()
                .filter(|spec| {
                    let forced = desired.prefs(spec.id).enabled == Some(true);
                    let found = *detected
                        .entry(spec.id)
                        .or_insert_with(|| detect(spec, cx.env, cx.fs, cx.registry).detected());
                    let fits = spec.mcp.as_ref().is_some_and(|mcp| {
                        mcp.oses.contains(&cx.env.os())
                            && transport.is_some_and(|t| mcp.caps.transports.contains(t))
                    });
                    (forced || found) && fits
                })
                .map(|spec| spec.id.to_owned())
                .collect(),
        };
        for client in clients {
            if desired.excluded(&client) {
                continue;
            }
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
            let rendered = render(cx, desired, server, spec);
            let delivery = rendered.as_ref().ok().and_then(|(_, delivery)| *delivery);
            rendering.entries.push(DesiredEntry {
                client: spec.id,
                server_id: id.clone(),
                name,
                server: rendered.map(|(server, _)| server),
                delivery,
            });
        }
    }
    rendering.clients = ADAPTERS
        .iter()
        .map(|spec| spec.id)
        .filter(|id| rendering.entries.iter().any(|entry| entry.client == *id))
        .collect();
    rendering
}

/// The adapter of `client`, if Loadout can write its user file here.
fn writable(client: &str, env: &Env) -> Result<&'static AdapterSpec, CoreError> {
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

fn server_transport(server: &Server, env: &Env) -> Option<Transport> {
    effective_fields(server, env, None).transport
}

/// The launch fields after `[os.<os>]` and the machine overlay.
fn effective_fields(server: &Server, env: &Env, overlay: Option<&MachineOverlay>) -> ServerFields {
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

/// One server for one client, with the delivery mode it used.
pub fn render(
    cx: &RenderCx<'_>,
    desired: &DesiredState,
    server: &Server,
    spec: &AdapterSpec,
) -> Result<(RenderedServer, Option<&'static str>), CoreError> {
    let mcp = spec.mcp.as_ref().ok_or_else(|| {
        CoreError::new(
            ErrorCode::ScopeNotSupported,
            format!("{} has no MCP config", spec.display_name),
        )
    })?;
    let fields = effective_fields(server, cx.env, desired.overlay.as_ref());
    let mut tx = Templates {
        cx,
        caps: &mcp.caps,
        client: spec.display_name,
        used_env: false,
    };
    let refs_allowed = matches!(
        server.body.delivery.as_ref().and_then(|d| d.mode),
        None | Some(DeliveryMode::Auto | DeliveryMode::Env)
    );
    let transport = fields.transport.ok_or_else(|| {
        CoreError::new(
            ErrorCode::MissingField,
            "`transport` is required: stdio, http or sse",
        )
    })?;
    let launch = match transport {
        Transport::Stdio => {
            let command = fields.command.clone().ok_or_else(|| {
                CoreError::new(ErrorCode::MissingField, "stdio servers need `command`")
            })?;
            let mut stdio = StdioLaunch {
                command: tx.render(&command, FieldSet::COMMAND)?,
                ..StdioLaunch::default()
            };
            for arg in fields.args.iter().flatten() {
                stdio.args.push(tx.render(arg, FieldSet::ARGS)?);
            }
            for (key, value) in fields.env.iter().flatten() {
                match tx.forwarded(value)? {
                    Some(name) if name == *key => stdio.forward_env.push(name),
                    _ => {
                        stdio
                            .env
                            .insert(key.clone(), tx.render(value, FieldSet::ENV)?);
                    }
                }
            }
            if let Some(cwd) = &fields.cwd {
                stdio.cwd = Some(tx.render(cwd, FieldSet::CWD)?);
            }
            Launch::Stdio(stdio)
        }
        Transport::Http | Transport::Sse => {
            let url = fields.url.clone().ok_or_else(|| {
                CoreError::new(ErrorCode::MissingField, "remote servers need `url`")
            })?;
            let mut remote = RemoteLaunch {
                url: tx.render(&url, FieldSet::URL)?,
                ..RemoteLaunch::default()
            };
            for (key, value) in fields.headers.iter().flatten() {
                if key.eq_ignore_ascii_case("authorization")
                    && let Some(rest) = value.strip_prefix("Bearer ")
                    && let Some(name) = tx.forwarded(rest)?
                {
                    remote.bearer_env = Some(name);
                    continue;
                }
                match tx.forwarded(value)? {
                    Some(name) => {
                        remote.headers_env.insert(key.clone(), name);
                    }
                    None => {
                        remote
                            .headers
                            .insert(key.clone(), tx.render(value, FieldSet::HEADERS)?);
                    }
                }
            }
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
                passthrough.insert(key.clone(), tx.native(value)?);
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
    if tx.used_env && !refs_allowed {
        return Err(CoreError::new(
            ErrorCode::DeliveryUnavailable,
            "this build delivers references only in `env` mode (delivery.mode auto or env)",
        ));
    }
    let rendered = RenderedServer {
        name: native_name(server, spec.id),
        launch,
        enabled: true,
        tools,
        passthrough,
    };
    Ok((rendered, tx.used_env.then_some("env")))
}

/// Template rendering for one client.
struct Templates<'a, 'b> {
    cx: &'a RenderCx<'b>,
    caps: &'a Caps,
    client: &'static str,
    used_env: bool,
}

impl Templates<'_, '_> {
    /// The client's spelling of a `${env:NAME}` reference in `field`, if
    /// it expands references there itself.
    fn env_syntax(&self, field: FieldSet) -> Option<EnvRefSyntax> {
        self.caps
            .env_ref
            .filter(|syntax| *syntax != EnvRefSyntax::CodexForward)
            .filter(|_| self.caps.env_ref_fields.contains(field) && field != FieldSet::NONE)
    }

    fn render(&mut self, text: &str, field: FieldSet) -> Result<String, CoreError> {
        let syntax = self.env_syntax(field);
        let mut out = String::new();
        for part in parse_template(text)? {
            match part {
                TemplatePart::Literal(literal) => {
                    if syntax.is_some() && literal.contains("${") {
                        return Err(CoreError::new(
                            ErrorCode::DeliveryUnavailable,
                            format!(
                                "{} would expand the literal `${{` in {text:?}; it has no escape for it",
                                self.client
                            ),
                        ));
                    }
                    out.push_str(&literal);
                }
                TemplatePart::Var(PathVar::Home) => {
                    out.push_str(&self.cx.env.home().to_string_lossy());
                }
                TemplatePart::Var(PathVar::LoadoutHome) => {
                    out.push_str(&self.cx.loadout_home.to_string_lossy());
                }
                TemplatePart::Var(PathVar::Project) => {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRef,
                        format!("`${{project}}` in {text:?} has no value at system scope"),
                    ));
                }
                TemplatePart::Env(name) => {
                    let spelled = match syntax {
                        Some(EnvRefSyntax::DollarBrace { .. }) => format!("${{{name}}}"),
                        Some(EnvRefSyntax::DollarBraceEnv) => format!("${{env:{name}}}"),
                        Some(EnvRefSyntax::BraceEnv) => format!("{{env:{name}}}"),
                        Some(EnvRefSyntax::CodexForward) | None => {
                            return Err(CoreError::new(
                                ErrorCode::DeliveryUnavailable,
                                format!(
                                    "{} does not expand `${{env:{name}}}` in this field, and this build has no `launch` delivery yet",
                                    self.client
                                ),
                            ));
                        }
                    };
                    self.used_env = true;
                    out.push_str(&spelled);
                }
                TemplatePart::Secret(alias) => {
                    return Err(CoreError::new(
                        ErrorCode::DeliveryUnavailable,
                        format!("this build cannot deliver `${{secret:{alias}}}` to clients yet"),
                    ));
                }
            }
        }
        Ok(out)
    }

    /// For a client that forwards variables by name (Codex): the variable
    /// when `text` is exactly one `${env:NAME}` reference.
    fn forwarded(&mut self, text: &str) -> Result<Option<String>, CoreError> {
        if self.caps.env_ref != Some(EnvRefSyntax::CodexForward) {
            return Ok(None);
        }
        match parse_template(text)?.as_slice() {
            [TemplatePart::Env(name)] => {
                self.used_env = true;
                Ok(Some(name.clone()))
            }
            _ => Ok(None),
        }
    }

    /// A passthrough value; strings may hold literals and path variables.
    fn native(&mut self, value: &toml::Value) -> Result<NativeValue, CoreError> {
        Ok(match value {
            toml::Value::String(text) => NativeValue::String(self.render(text, FieldSet::NONE)?),
            toml::Value::Integer(n) => NativeValue::Integer(*n),
            toml::Value::Float(n) => NativeValue::Float(*n),
            toml::Value::Boolean(b) => NativeValue::Bool(*b),
            toml::Value::Datetime(d) => NativeValue::Datetime(d.to_string()),
            toml::Value::Array(items) => NativeValue::Array(
                items
                    .iter()
                    .map(|item| self.native(item))
                    .collect::<Result<_, _>>()?,
            ),
            toml::Value::Table(table) => {
                let mut map = NativeMap::new();
                for (key, item) in table {
                    map.insert(key.clone(), self.native(item)?);
                }
                NativeValue::Object(map)
            }
        })
    }
}

/// A native string value as a store template (SPEC 11.2): the client's own
/// variable references become `${env:NAME}`, any other `${` becomes the
/// literal `$${`. `None` when the value uses a client feature the store
/// cannot express (`${NAME:-default}`, `${userHome}`).
pub fn native_to_template(text: &str, caps: &Caps, field: FieldSet) -> Option<String> {
    let syntax = caps
        .env_ref
        .filter(|_| caps.env_ref_fields.contains(field) && field != FieldSet::NONE);
    match syntax {
        Some(EnvRefSyntax::DollarBrace { .. }) => convert_refs(text, "${", |token| {
            env_name(token).then(|| format!("${{env:{token}}}"))
        }),
        Some(EnvRefSyntax::DollarBraceEnv) => convert_refs(text, "${", |token| {
            token
                .strip_prefix("env:")
                .filter(|name| env_name(name))
                .map(|name| format!("${{env:{name}}}"))
        }),
        // A literal `${` next to OpenCode's `{env:..}` has no store spelling
        // that reads back the same.
        Some(EnvRefSyntax::BraceEnv) if text.contains("${") => None,
        Some(EnvRefSyntax::BraceEnv) => convert_refs(text, "{env:", |token| {
            env_name(token).then(|| format!("${{env:{token}}}"))
        }),
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

//! MCP page commands (SPEC 13.2): the server list with per-client pill
//! states, the edit dialog (Form, TOML, Preview), paste-anything conversion
//! and client enablement. Every change is one store commit with its instant
//! apply, as `loadout server add|rm` does (SPEC 8.7).

use std::collections::BTreeMap;
use std::sync::Arc;

use agent_loadout_core::adapters::detect::detect;
use agent_loadout_core::adapters::mapper::WriteCx;
use agent_loadout_core::adapters::{ADAPTERS, AdapterSpec, adapter};
use agent_loadout_core::apply::import::server_from_native;
use agent_loadout_core::apply::render::{DesiredState, Detected, RenderCx, native_name, render};
use agent_loadout_core::apply::{
    self, EntryState, Plan, PlanOptions, ScopeFilter, TargetPlan, instant_apply,
};
use agent_loadout_core::formats::{Document, Format};
use agent_loadout_core::model::TomlDocument;
use agent_loadout_core::model::applied::Scope as TargetScope;
use agent_loadout_core::model::clients::ClientSelection;
use agent_loadout_core::model::ids::{self, IdKind};
use agent_loadout_core::model::server::{
    Delivery, DeliveryMode, Enable, Meta, Server, ServerFields, ToolFilter, Transport,
};
use agent_loadout_core::secrets::catalog::Catalog;
use agent_loadout_core::secrets::delivery::Sharing;
use agent_loadout_core::secrets::scanner::SecretMemory;
use agent_loadout_core::store::{Machine, Operation, Store, server_path};
use agent_loadout_core::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use specta::Type;

use super::{AppState, ChangeResult, CmdResult, Core, IpcError, NO_REGISTRY, blocking, errors};

/// A client the MCP page can enable servers for.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub id: String,
    pub name: String,
    /// Installed on this machine (SPEC 7.7 probes).
    pub detected: bool,
    /// Has an MCP config on this operating system.
    pub supported: bool,
}

/// A pill's state (SPEC 13.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum PillState {
    /// Not enabled, and nothing of it in the client's file.
    Disabled,
    /// Enabled and written as desired.
    Enabled,
    /// The store and the client file differ until the next apply.
    Pending,
    /// The file changed since it was written, or collides with a foreign
    /// entry: needs a decision.
    Drift,
    /// Cannot be written; see the reason.
    Blocked,
    /// The client has no MCP config at this scope or on this OS.
    NotSupported,
}

/// One server for one client.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ClientPill {
    pub client: String,
    /// The store enables the server for this client.
    pub selected: bool,
    pub state: PillState,
    pub reason: Option<IpcError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    Stdio,
    Http,
    Sse,
}

impl From<Transport> for TransportKind {
    fn from(transport: Transport) -> Self {
        match transport {
            Transport::Stdio => Self::Stdio,
            Transport::Http => Self::Http,
            Transport::Sse => Self::Sse,
        }
    }
}

impl From<TransportKind> for Transport {
    fn from(kind: TransportKind) -> Self {
        match kind {
            TransportKind::Stdio => Self::Stdio,
            TransportKind::Http => Self::Http,
            TransportKind::Sse => Self::Sse,
        }
    }
}

/// How secret references reach the server (SPEC 11.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryKind {
    Auto,
    Env,
    Launch,
    Command,
    Inline,
}

impl From<DeliveryMode> for DeliveryKind {
    fn from(mode: DeliveryMode) -> Self {
        match mode {
            DeliveryMode::Auto => Self::Auto,
            DeliveryMode::Env => Self::Env,
            DeliveryMode::Launch => Self::Launch,
            DeliveryMode::Command => Self::Command,
            DeliveryMode::Inline => Self::Inline,
        }
    }
}

impl From<DeliveryKind> for DeliveryMode {
    fn from(kind: DeliveryKind) -> Self {
        match kind {
            DeliveryKind::Auto => Self::Auto,
            DeliveryKind::Env => Self::Env,
            DeliveryKind::Launch => Self::Launch,
            DeliveryKind::Command => Self::Command,
            DeliveryKind::Inline => Self::Inline,
        }
    }
}

/// The clients a server is enabled for at system scope.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ClientChoice {
    /// Every detected client whose capabilities fit (`all-detected`).
    pub all_detected: bool,
    /// The listed clients, when not `all_detected`.
    pub clients: Vec<String>,
}

impl ClientChoice {
    fn from_selection(selection: Option<&ClientSelection>) -> Self {
        match selection {
            Some(ClientSelection::AllDetected) => Self {
                all_detected: true,
                clients: Vec::new(),
            },
            Some(ClientSelection::List(ids)) => Self {
                all_detected: false,
                clients: ids.clone(),
            },
            None => Self::default(),
        }
    }

    fn to_selection(&self) -> Option<ClientSelection> {
        if self.all_detected {
            Some(ClientSelection::AllDetected)
        } else if self.clients.is_empty() {
            None
        } else {
            let mut ids: Vec<String> = Vec::new();
            for id in &self.clients {
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
            Some(ClientSelection::List(ids))
        }
    }
}

/// An environment variable or header: the value is a literal or holds
/// references (`${secret:ALIAS}`, `${env:NAME}`), never a secret itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

/// The edit dialog's Form (SPEC 13.2). Store values only: the store never
/// holds a secret value (SPEC 11.8), so nothing here needs masking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ServerDraft {
    pub id: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub transport: TransportKind,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub cwd: Option<String>,
    pub env: Vec<KeyValue>,
    pub headers: Vec<KeyValue>,
    pub clients: ClientChoice,
    pub delivery: Option<DeliveryKind>,
    pub tools_allow: Vec<String>,
    pub tools_deny: Vec<String>,
}

/// One row of the MCP page.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ServerRow {
    pub id: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub transport: Option<TransportKind>,
    /// Command and arguments, or the URL, as stored: references are shown,
    /// values never.
    pub launch: String,
    pub origin: Option<String>,
    pub tags: Vec<String>,
    pub clients: ClientChoice,
    /// One pill per client the page lists, in [`ServerList::clients`] order.
    pub pills: Vec<ClientPill>,
    /// Problems of this server's entries (`missing_secret`, ...), once each.
    pub problems: Vec<IpcError>,
    /// The server file does not load; the row offers only Edit (TOML) and
    /// Delete.
    pub invalid: Option<IpcError>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ServerList {
    pub servers: Vec<ServerRow>,
    /// Clients with an MCP config, in adapter table order.
    pub clients: Vec<ClientInfo>,
    /// Plan problems outside any server row.
    pub warnings: Vec<IpcError>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ServerDetail {
    /// The Form view; `None` when the file does not load (see `invalid`),
    /// so only the TOML tab can fix it.
    pub draft: Option<ServerDraft>,
    pub invalid: Option<IpcError>,
    /// `store/servers/<id>.toml` as it is.
    pub toml: String,
}

/// One client's rendered entry for the Preview tab.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ClientPreview {
    pub client: String,
    /// The native entry key.
    pub name: String,
    /// `json`, `jsonc`, `toml`, `json5` or `yaml`.
    pub format: String,
    /// The entry inside its container, as the client file would hold it,
    /// with values written in plain text (`inline`) masked.
    pub text: Option<String>,
    /// The delivery mode chosen for references (`env`, `launch`, ...).
    pub delivery: Option<String>,
    /// Why the client cannot get this server.
    pub problem: Option<IpcError>,
    pub warnings: Vec<IpcError>,
}

/// One server found in a pasted snippet.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ConvertedServer {
    pub name: String,
    /// The client whose shape the snippet has.
    pub source_client: String,
    pub draft: Option<ServerDraft>,
    /// Why it cannot be taken as it is (`needs_secret_extraction`, ...).
    pub problem: Option<IpcError>,
}

const MASK: &str = "********";

fn not_found(id: &str) -> CoreError {
    CoreError::new(ErrorCode::NotFound, format!("server {id:?} does not exist"))
}

/// Clients with an MCP config Loadout can write, in table order.
pub fn client_infos(core: &Core) -> Vec<ClientInfo> {
    ADAPTERS
        .iter()
        .filter_map(|spec| {
            let mcp = spec.mcp.as_ref()?;
            if mcp.read_only {
                return None;
            }
            let detection = detect(spec, &core.env, core.fs.as_ref(), &NO_REGISTRY);
            Some(ClientInfo {
                id: spec.id.to_owned(),
                name: spec.display_name.to_owned(),
                detected: detection.detected(),
                supported: mcp.oses.contains(&core.os()),
            })
        })
        .collect()
}

fn system_plan(core: &Core, store: Option<&Store<'_>>) -> Result<Plan, CoreError> {
    let options = PlanOptions {
        scope: ScopeFilter::System,
        ..PlanOptions::default()
    };
    apply::preview(store, &core.apply_env(), &options)
}

/// What rendering reads: the desired state of the store's working tree (or
/// none) and the store's secret memory.
pub(super) fn render_inputs(
    core: &Core,
    store: Option<&Store<'_>>,
) -> Result<(DesiredState, Arc<SecretMemory>), CoreError> {
    let machine = Machine::load(core.fs.as_ref(), &core.home.state_dir())?;
    Ok(match store {
        Some(store) => (
            DesiredState::load(store, machine.as_ref().map(|m| m.id.as_str()))?,
            store.secret_memory().clone(),
        ),
        None => (DesiredState::empty(), Arc::new(SecretMemory::new()?)),
    })
}

fn pill_state(state: EntryState) -> Option<PillState> {
    match state {
        EntryState::Synced => Some(PillState::Enabled),
        EntryState::PendingCreate
        | EntryState::PendingUpdate
        | EntryState::PendingRemove
        | EntryState::Missing => Some(PillState::Pending),
        EntryState::Drifted | EntryState::Conflict | EntryState::Collision => {
            Some(PillState::Drift)
        }
        EntryState::Blocked => Some(PillState::Blocked),
        EntryState::Foreign => None,
    }
}

/// `selected`: the clients the core writes the server for here
/// ([`Detected::system_clients`]).
fn pill(plan: &Plan, server: &str, selected: &[String], client: &ClientInfo) -> ClientPill {
    let unsupported = IpcError {
        code: ErrorCode::NotSupportedOnOs,
        message: format!("{} has no MCP config on this OS", client.name),
    };
    let targets = plan
        .targets
        .iter()
        .filter(|target| target.scope == TargetScope::User);
    pill_in(targets, server, selected, client, unsupported)
}

/// The pill of `server` for `client` from the entries of `targets`: the
/// most pressing entry state, else pending when selected and disabled
/// otherwise. A client that is not `supported` gets `unsupported`.
pub(super) fn pill_in<'a>(
    targets: impl Iterator<Item = &'a TargetPlan>,
    server: &str,
    selected: &[String],
    client: &ClientInfo,
    unsupported: IpcError,
) -> ClientPill {
    let selected = selected.contains(&client.id);
    if !client.supported {
        return ClientPill {
            client: client.id.clone(),
            selected,
            state: PillState::NotSupported,
            reason: Some(unsupported),
        };
    }
    let mut worst: Option<(PillState, Option<IpcError>)> = None;
    let entries = targets.flat_map(|target| &target.entries).filter(|entry| {
        entry.server_id.as_deref() == Some(server) && entry.client == Some(client.id.as_str())
    });
    for entry in entries {
        let Some(state) = pill_state(entry.state) else {
            continue;
        };
        if worst.as_ref().is_none_or(|(current, _)| state > *current) {
            worst = Some((state, entry.problem.as_ref().map(IpcError::from)));
        }
    }
    let (state, reason) = worst.unwrap_or(if selected {
        (PillState::Pending, None)
    } else {
        (PillState::Disabled, None)
    });
    ClientPill {
        client: client.id.clone(),
        selected,
        state,
        reason,
    }
}

pub(super) fn launch_text(fields: &Server) -> String {
    let body = &fields.body;
    match (&body.url, &body.command) {
        (Some(url), _) => url.clone(),
        (None, Some(command)) => std::iter::once(command)
            .chain(body.args.iter().flatten())
            .cloned()
            .collect::<Vec<_>>()
            .join(" "),
        (None, None) => String::new(),
    }
}

fn row(plan: &Plan, clients: &[ClientInfo], selected: &[String], server: &Server) -> ServerRow {
    let body = &server.body;
    let choice = ClientChoice::from_selection(
        server
            .enable
            .as_ref()
            .and_then(|enable| enable.clients.as_ref()),
    );
    let mut problems: Vec<IpcError> = Vec::new();
    for entry in plan
        .targets
        .iter()
        .flat_map(|target| &target.entries)
        .filter(|entry| entry.server_id.as_deref() == Some(server.id.as_str()))
    {
        if let Some(problem) = &entry.problem
            && !problems
                .iter()
                .any(|known| known.code == problem.code() && known.message == problem.message())
        {
            problems.push(problem.into());
        }
    }
    ServerRow {
        id: server.id.clone(),
        display_name: body.display_name.clone(),
        description: body.description.clone(),
        transport: body.transport.map(TransportKind::from),
        launch: launch_text(server),
        origin: body.meta.as_ref().and_then(|meta| meta.origin.clone()),
        tags: body.tags.clone().unwrap_or_default(),
        pills: clients
            .iter()
            .map(|client| pill(plan, &server.id, selected, client))
            .collect(),
        clients: choice,
        problems,
        invalid: None,
    }
}

/// The MCP page: every store server with its per-client state.
pub fn list(core: &Core) -> CmdResult<ServerList> {
    let clients = client_infos(core);
    let store = core.open_store()?;
    let plan = system_plan(core, store.as_ref())?;
    let entries = match &store {
        Some(store) => store.servers()?,
        None => Vec::new(),
    };
    let (desired, memory) = render_inputs(core, store.as_ref())?;
    let ax = core.apply_env();
    let cx = ax.render_cx(memory);
    let mut detected = Detected::default();
    let servers = entries
        .iter()
        .map(|entry| match &entry.document {
            Ok(doc) => {
                let selected = detected.system_clients(&cx, &desired, doc.value());
                row(&plan, &clients, &selected, doc.value())
            }
            Err(err) => ServerRow {
                id: entry.id.clone(),
                display_name: None,
                description: None,
                transport: None,
                launch: String::new(),
                origin: None,
                tags: Vec::new(),
                clients: ClientChoice::default(),
                pills: Vec::new(),
                problems: Vec::new(),
                invalid: Some(err.into()),
            },
        })
        .collect();
    Ok(ServerList {
        servers,
        clients,
        warnings: errors(&plan.warnings),
    })
}

pub(super) fn non_empty(text: &Option<String>) -> Option<String> {
    text.as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn non_empty_list(items: &[String]) -> Option<Vec<String>> {
    let items: Vec<String> = items
        .iter()
        .map(|item| item.trim().to_owned())
        .filter(|item| !item.is_empty())
        .collect();
    (!items.is_empty()).then_some(items)
}

pub(super) fn pairs(
    rows: &[KeyValue],
    what: &str,
) -> Result<Option<BTreeMap<String, String>>, CoreError> {
    let mut map = BTreeMap::new();
    for KeyValue { key, value } in rows {
        let key = key.trim();
        if key.is_empty() {
            if value.is_empty() {
                continue;
            }
            return Err(CoreError::new(
                ErrorCode::MissingField,
                format!("a {what} value has no name"),
            ));
        }
        map.insert(key.to_owned(), value.clone());
    }
    Ok((!map.is_empty()).then_some(map))
}

pub(super) fn to_pairs(map: &Option<BTreeMap<String, String>>) -> Vec<KeyValue> {
    map.iter()
        .flatten()
        .map(|(key, value)| KeyValue {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// The Form view of a stored server.
pub fn draft_of(server: &Server) -> ServerDraft {
    let body = &server.body;
    let tools = body.tools.as_ref();
    ServerDraft {
        id: server.id.clone(),
        display_name: body.display_name.clone(),
        description: body.description.clone(),
        transport: body
            .transport
            .map_or(TransportKind::Stdio, TransportKind::from),
        command: body.command.clone(),
        args: body.args.clone().unwrap_or_default(),
        url: body.url.clone(),
        cwd: body.cwd.clone(),
        env: to_pairs(&body.env),
        headers: to_pairs(&body.headers),
        clients: ClientChoice::from_selection(
            server
                .enable
                .as_ref()
                .and_then(|enable| enable.clients.as_ref()),
        ),
        delivery: body
            .delivery
            .as_ref()
            .and_then(|delivery| delivery.mode)
            .map(DeliveryKind::from),
        tools_allow: tools.and_then(|t| t.allow.clone()).unwrap_or_default(),
        tools_deny: tools.and_then(|t| t.deny.clone()).unwrap_or_default(),
    }
}

/// Writes the Form's fields into `server`, keeping everything the Form does
/// not show (`os`, `client`, `meta`, unknown keys).
fn apply_draft(server: &mut Server, draft: &ServerDraft) -> Result<(), CoreError> {
    let body = &mut server.body;
    body.display_name = non_empty(&draft.display_name);
    body.description = non_empty(&draft.description);
    body.transport = Some(draft.transport.into());
    match draft.transport {
        TransportKind::Stdio => {
            body.command = non_empty(&draft.command);
            body.args = (!draft.args.is_empty()).then(|| draft.args.clone());
            body.cwd = non_empty(&draft.cwd);
            body.env = pairs(&draft.env, "environment")?;
            body.url = None;
            body.headers = None;
        }
        TransportKind::Http | TransportKind::Sse => {
            body.url = non_empty(&draft.url);
            body.headers = pairs(&draft.headers, "header")?;
            body.command = None;
            body.args = None;
            body.cwd = None;
            body.env = None;
        }
    }
    body.delivery = match (draft.delivery, body.delivery.take()) {
        (Some(mode), Some(mut delivery)) => {
            delivery.mode = Some(mode.into());
            Some(delivery)
        }
        (Some(mode), None) => Some(Delivery {
            mode: Some(mode.into()),
            ..Delivery::default()
        }),
        (None, Some(mut delivery)) => {
            delivery.mode = None;
            (!delivery.extra.is_empty()).then_some(delivery)
        }
        (None, None) => None,
    };
    let allow = non_empty_list(&draft.tools_allow);
    let deny = non_empty_list(&draft.tools_deny);
    body.tools = match body.tools.take() {
        Some(mut tools) => {
            tools.allow = allow;
            tools.deny = deny;
            (tools.allow.is_some() || tools.deny.is_some() || !tools.extra.is_empty())
                .then_some(tools)
        }
        None if allow.is_some() || deny.is_some() => Some(ToolFilter {
            allow,
            deny,
            ..ToolFilter::default()
        }),
        None => None,
    };
    set_clients(server, &draft.clients);
    Ok(())
}

fn set_clients(server: &mut Server, choice: &ClientChoice) {
    let selection = choice.to_selection();
    server.enable = match (server.enable.take(), selection) {
        (Some(mut enable), selection) => {
            enable.clients = selection;
            (enable.clients.is_some() || !enable.extra.is_empty()).then_some(enable)
        }
        (None, Some(selection)) => Some(Enable {
            clients: Some(selection),
            ..Enable::default()
        }),
        (None, None) => None,
    };
}

fn validated(server: &Server) -> Result<(), CoreError> {
    match server.validate(Some(&server.id)).first_error(None) {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// The Form and TOML of a stored server.
pub fn get(core: &Core, id: &str) -> CmdResult<ServerDetail> {
    ids::check(IdKind::Server, id)?;
    let store = core.open_store()?.ok_or_else(|| not_found(id))?;
    let toml = store
        .read_to_string(&server_path(id)?)?
        .ok_or_else(|| not_found(id))?;
    let (draft, invalid) = match store.server(id) {
        Ok(Some(doc)) => (Some(draft_of(doc.value())), None),
        Ok(None) => return Err(not_found(id).into()),
        Err(err) => (None, Some(err.into())),
    };
    Ok(ServerDetail {
        draft,
        invalid,
        toml,
    })
}

/// Edits one server file in one commit with its instant apply.
fn change_server(
    core: &Core,
    op: &str,
    id: &str,
    summary: String,
    edit: impl FnOnce(&mut agent_loadout_core::store::Transaction<'_>) -> Result<(), CoreError>,
) -> CmdResult<ChangeResult> {
    let store = core.open_store_for_change()?;
    let ax = core.apply_env();
    let operation = Operation::new(op, summary).entity(format!("server/{id}"));
    let committed = store.transaction(operation, |tx| {
        edit(tx)?;
        instant_apply(tx, &ax)
    })?;
    Ok(ChangeResult::new(
        &core.env,
        committed.commit.as_ref(),
        committed.value.as_ref(),
    ))
}

/// Saves the Form: a new server (`create`) or the fields of an existing
/// one, keeping its other keys and comments.
pub fn save(core: &Core, draft: &ServerDraft, create: bool) -> CmdResult<ChangeResult> {
    ids::check(IdKind::Server, &draft.id)?;
    let id = draft.id.clone();
    if create {
        let mut server = Server::new(id.clone());
        server.body.meta = Some(Meta {
            origin: Some("manual".to_owned()),
            ..Meta::default()
        });
        apply_draft(&mut server, draft)?;
        validated(&server)?;
        return change_server(core, "server.add", &id, format!("server: add {id}"), |tx| {
            tx.add_server(server)
        });
    }
    let store = core.open_store()?.ok_or_else(|| not_found(&id))?;
    let mut doc = store.server(&id)?.ok_or_else(|| not_found(&id))?;
    apply_draft(doc.value_mut(), draft)?;
    validated(doc.value())?;
    let text = doc.to_text()?;
    drop(store);
    change_server(
        core,
        "server.update",
        &id,
        format!("server: update {id}"),
        |tx| tx.write(&server_path(&id)?, text),
    )
}

/// `text` as server `id`'s file: it parses, validates and names `id`.
fn parse_server(id: &str, text: &str) -> Result<TomlDocument<Server>, CoreError> {
    ids::check(IdKind::Server, id)?;
    let doc =
        TomlDocument::<Server>::parse(text).map_err(|diagnostic| diagnostic.to_core_error(None))?;
    validated(doc.value())?;
    if doc.value().id != id {
        return Err(CoreError::new(
            ErrorCode::IdMismatch,
            format!("the file says id {:?}, not {id:?}", doc.value().id),
        ));
    }
    Ok(doc)
}

/// The TOML tab after Form edits: the Form's fields written into `text`
/// (the file as the TOML tab has it), keeping its comments and the keys
/// the Form does not show. Nothing is saved.
pub fn draft_to_toml(draft: &ServerDraft, text: &str) -> CmdResult<String> {
    let mut doc = parse_server(&draft.id, text)?;
    apply_draft(doc.value_mut(), draft)?;
    Ok(doc.to_text()?)
}

/// The Form after TOML edits: `text` read as server `id`'s file. Nothing
/// is saved.
pub fn toml_to_draft(id: &str, text: &str) -> CmdResult<ServerDraft> {
    Ok(draft_of(parse_server(id, text)?.value()))
}

/// Saves the TOML tab: the text must parse as this server's file.
pub fn save_toml(core: &Core, id: &str, text: &str) -> CmdResult<ChangeResult> {
    parse_server(id, text)?;
    let relative = server_path(id)?;
    let store = core.open_store()?.ok_or_else(|| not_found(id))?;
    if store.read_to_string(&relative)?.is_none() {
        return Err(not_found(id).into());
    }
    drop(store);
    let text = text.to_owned();
    change_server(
        core,
        "server.update",
        id,
        format!("server: update {id}"),
        |tx| tx.write(&relative, text),
    )
}

/// Removes servers in one commit with its instant apply.
pub fn remove(core: &Core, ids_to_remove: &[String]) -> CmdResult<ChangeResult> {
    let mut unique: Vec<String> = Vec::new();
    for id in ids_to_remove {
        ids::check(IdKind::Server, id)?;
        if !unique.contains(id) {
            unique.push(id.clone());
        }
    }
    let Some(first) = unique.first() else {
        return Err(CoreError::new(ErrorCode::MissingField, "no server to remove").into());
    };
    let store = core.open_store()?.ok_or_else(|| not_found(first))?;
    let summary = match unique.as_slice() {
        [one] => format!("server: remove {one}"),
        many => format!(
            "server: remove {} servers ({})",
            many.len(),
            many.join(", ")
        ),
    };
    let operation = unique
        .iter()
        .fold(Operation::new("server.remove", summary), |operation, id| {
            operation.entity(format!("server/{id}"))
        });
    let ax = core.apply_env();
    let committed = store.transaction(operation, |tx| {
        unique.iter().try_for_each(|id| tx.remove_server(id))?;
        instant_apply(tx, &ax)
    })?;
    Ok(ChangeResult::new(
        &core.env,
        committed.commit.as_ref(),
        committed.value.as_ref(),
    ))
}

/// Enables a server for exactly these clients (pill toggles, Enable All,
/// Disable All). A client named explicitly is managed again on this
/// machine even if its entry was released here.
pub fn set_clients_for(core: &Core, id: &str, choice: &ClientChoice) -> CmdResult<ChangeResult> {
    ids::check(IdKind::Server, id)?;
    let store = core.open_store()?.ok_or_else(|| not_found(id))?;
    let mut doc = store.server(id)?.ok_or_else(|| not_found(id))?;
    // Clients whose entry this machine released are not shown as selected
    // here, but other machines still manage them: they stay in the synced
    // list unless the choice names them (which manages them here again).
    let (desired, _) = render_inputs(core, Some(&store))?;
    let requested = choice.clients.clone();
    let mut choice = choice.clone();
    if !choice.all_detected
        && let Some(ClientSelection::List(old)) =
            doc.value().enable.as_ref().and_then(|e| e.clients.as_ref())
    {
        for client in old {
            if desired.released(id, client) && !choice.clients.contains(client) {
                choice.clients.push(client.clone());
            }
        }
    }
    let choice = &choice;
    set_clients(doc.value_mut(), choice);
    validated(doc.value())?;
    let text = doc.to_text()?;
    drop(store);
    let relative = server_path(id)?;
    change_server(
        core,
        "server.enable",
        id,
        format!("server: enable {id} for clients"),
        |tx| {
            tx.write(&relative, text)?;
            agent_loadout_core::apply::import::unrelease_clients(tx, id, &requested).map(drop)
        },
    )
}

fn masked(text: &str, values: &[agent_loadout_core::secrets::SecretValue]) -> String {
    let mut text = text.to_owned();
    for value in values {
        let raw = value.expose();
        if raw.chars().count() < 4 {
            continue;
        }
        let escaped = serde_json::to_string(raw).unwrap_or_default();
        let escaped = escaped.trim_matches('"').to_owned();
        for form in [escaped.as_str(), raw] {
            text = text.replace(form, MASK);
        }
    }
    text
}

fn render_preview(
    cx: &RenderCx<'_>,
    desired: &DesiredState,
    server: &Server,
    spec: &AdapterSpec,
    os: agent_loadout_core::Os,
) -> ClientPreview {
    let name = native_name(server, spec.id);
    let mut preview = ClientPreview {
        client: spec.id.to_owned(),
        name: name.clone(),
        format: String::new(),
        text: None,
        delivery: None,
        problem: None,
        warnings: Vec::new(),
    };
    let Some(mcp) = spec.mcp.as_ref() else {
        return preview;
    };
    preview.format = mcp.format.as_str().to_owned();
    let result = (|| -> Result<String, CoreError> {
        let output = render(cx, desired, server, spec, Sharing::Private)?;
        preview.delivery = output.delivery.map(|mode| mode.as_str().to_owned());
        preview.warnings = errors(&output.warnings);
        let native = mcp
            .mapper
            .to_native(&output.server, &WriteCx { os, existing: None })?;
        let keys = mcp.container.keys(os, None)?;
        let container: Vec<&str> = keys.iter().map(String::as_str).collect();
        let mut doc = Document::empty(mcp.format);
        doc.upsert(&container, &name, &native)?;
        let text = String::from_utf8_lossy(&doc.to_bytes()).into_owned();
        Ok(masked(&text, &output.inline))
    })();
    match result {
        Ok(text) => preview.text = Some(text),
        Err(err) => preview.problem = Some(err.into()),
    }
    preview
}

/// The Preview tab: the draft rendered for each client the core writes it
/// for, masked, or why a client cannot have it. An existing server keeps
/// what the Form does not show (`[client.*]`, `[os.*]`, other keys), as
/// [`save`] does. Nothing is written.
pub fn preview(core: &Core, draft: &ServerDraft) -> CmdResult<Vec<ClientPreview>> {
    ids::check(IdKind::Server, &draft.id)?;
    let clients = client_infos(core);
    let store = core.open_store()?;
    let stored = match &store {
        Some(store) => store
            .server(&draft.id)
            .ok()
            .flatten()
            .map(|doc| doc.value().clone()),
        None => None,
    };
    let mut server = stored.unwrap_or_else(|| Server::new(draft.id.clone()));
    apply_draft(&mut server, draft)?;
    validated(&server)?;
    let (desired, memory) = render_inputs(core, store.as_ref())?;
    let ax = core.apply_env();
    let cx = ax.render_cx(memory);
    let selected = Detected::default().system_clients(&cx, &desired, &server);
    Ok(clients
        .iter()
        .filter(|client| selected.contains(&client.id))
        .filter_map(|client| adapter(&client.id).map(|spec| (client, spec)))
        .map(|(client, spec)| {
            if !client.supported {
                return ClientPreview {
                    client: spec.id.to_owned(),
                    name: native_name(&server, spec.id),
                    format: String::new(),
                    text: None,
                    delivery: None,
                    problem: Some(IpcError {
                        code: ErrorCode::NotSupportedOnOs,
                        message: format!("{} has no MCP config on this OS", spec.display_name),
                    }),
                    warnings: Vec::new(),
                };
            }
            render_preview(&cx, &desired, &server, spec, core.os())
        })
        .collect())
}

/// A store id from a native entry name: lower case, other characters
/// become `-`.
fn id_from_name(name: &str) -> String {
    let mut id: String = name
        .trim()
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    id = id.trim_matches(|c| matches!(c, '-' | '.' | '_')).to_owned();
    id.truncate(64);
    if id.is_empty() {
        "server".to_owned()
    } else {
        id
    }
}

fn draft_from_fields(id: String, fields: &ServerFields, tools: Option<&ToolFilter>) -> ServerDraft {
    ServerDraft {
        id,
        display_name: None,
        description: None,
        transport: fields
            .transport
            .map_or(TransportKind::Stdio, TransportKind::from),
        command: fields.command.clone(),
        args: fields.args.clone().unwrap_or_default(),
        url: fields.url.clone(),
        cwd: fields.cwd.clone(),
        env: to_pairs(&fields.env),
        headers: to_pairs(&fields.headers),
        clients: ClientChoice::default(),
        delivery: None,
        tools_allow: tools.and_then(|t| t.allow.clone()).unwrap_or_default(),
        tools_deny: tools.and_then(|t| t.deny.clone()).unwrap_or_default(),
    }
}

fn parse_any(text: &str) -> Option<Document> {
    [Format::Jsonc, Format::Toml, Format::Yaml]
        .into_iter()
        .find_map(|format| Document::parse(text.as_bytes(), format).ok())
}

fn same_family(a: Format, b: Format) -> bool {
    let family = |format: Format| match format {
        Format::Toml => 1,
        Format::Yaml => 2,
        _ => 0,
    };
    family(a) == family(b)
}

fn convert_entries(
    doc: &Document,
    spec: &AdapterSpec,
    container: &[&str],
    names: &[String],
) -> Vec<ConvertedServer> {
    names
        .iter()
        .map(|name| {
            let converted = doc
                .get(container, name)
                .and_then(|value| {
                    value.ok_or_else(|| CoreError::new(ErrorCode::NotFound, "entry vanished"))
                })
                .and_then(|value| server_from_native(spec, name, &value));
            match converted {
                Ok(def) => ConvertedServer {
                    name: name.clone(),
                    source_client: spec.id.to_owned(),
                    draft: Some(draft_from_fields(
                        id_from_name(name),
                        &def.fields,
                        def.tools.as_ref(),
                    )),
                    problem: None,
                },
                Err(err) => ConvertedServer {
                    name: name.clone(),
                    source_client: spec.id.to_owned(),
                    draft: None,
                    problem: Some(err.into()),
                },
            }
        })
        .collect()
}

/// Paste-anything (SPEC 13.2): a client JSON, JSONC, TOML or YAML snippet in
/// any client's shape, converted to drafts. A literal that may be a secret
/// makes its entry `needs_secret_extraction` (SPEC 11.7) instead of a draft,
/// so no pasted secret comes back.
pub fn convert(core: &Core, text: &str) -> CmdResult<Vec<ConvertedServer>> {
    let doc = parse_any(text).ok_or_else(|| {
        CoreError::new(
            ErrorCode::ParseError,
            "the snippet is not JSON, JSONC, TOML or YAML",
        )
    })?;
    let os = core.os();
    for spec in ADAPTERS.iter().copied() {
        let Some(mcp) = spec.mcp.as_ref() else {
            continue;
        };
        if !same_family(mcp.format, doc.format()) {
            continue;
        }
        let Ok(keys) = mcp.container.keys(os, None) else {
            continue;
        };
        if keys.is_empty() {
            continue;
        }
        let container: Vec<&str> = keys.iter().map(String::as_str).collect();
        if let Ok(names) = doc.list(&container)
            && !names.is_empty()
        {
            return Ok(convert_entries(&doc, spec, &container, &names));
        }
    }
    // No client container: a bare map of entries, or one entry. JSON reads
    // with Claude Code's mapper, TOML with Codex's.
    let fallback = match doc.format() {
        Format::Toml => "codex",
        _ => "claude-code",
    };
    let spec = adapter(fallback).expect("adapter in the table");
    let names = doc.list(&[]).unwrap_or_default();
    let is_entry = |value: &agent_loadout_core::formats::NativeValue| {
        value
            .as_object()
            .is_some_and(|map| map.contains_key("command") || map.contains_key("url"))
    };
    if names.iter().any(|name| name == "command" || name == "url") {
        let mut wrapped = Document::empty(doc.format());
        let mut root = agent_loadout_core::formats::NativeMap::new();
        for name in &names {
            if let Ok(Some(value)) = doc.get(&[], name) {
                root.insert(name.clone(), value);
            }
        }
        wrapped.upsert(&[], "server", &root.into())?;
        return Ok(convert_entries(&wrapped, spec, &[], &["server".to_owned()]));
    }
    let entries: Vec<String> = names
        .into_iter()
        .filter(|name| matches!(doc.get(&[], name), Ok(Some(value)) if is_entry(&value)))
        .collect();
    if entries.is_empty() {
        return Err(CoreError::new(
            ErrorCode::UnexpectedShape,
            "the snippet holds no MCP server entry",
        )
        .into());
    }
    Ok(convert_entries(&doc, spec, &[], &entries))
}

/// Secret aliases for the Form's reference picker (names only).
pub fn secret_aliases(core: &Core) -> CmdResult<Vec<String>> {
    let Some(store) = core.open_store()? else {
        return Ok(Vec::new());
    };
    let machine = Machine::load(core.fs.as_ref(), &core.home.state_dir())?;
    let catalog = Catalog::load(&store, machine.as_ref().map(|m| m.id.as_str()))?;
    Ok(catalog.iter().map(|alias| alias.alias.clone()).collect())
}

#[tauri::command]
#[specta::specta]
pub async fn list_servers(state: tauri::State<'_, AppState>) -> CmdResult<ServerList> {
    blocking(&state, list).await
}

#[tauri::command]
#[specta::specta]
pub async fn get_server(state: tauri::State<'_, AppState>, id: String) -> CmdResult<ServerDetail> {
    blocking(&state, move |core| get(core, &id)).await
}

#[tauri::command]
#[specta::specta]
pub async fn save_server(
    state: tauri::State<'_, AppState>,
    draft: ServerDraft,
    create: bool,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| save(core, &draft, create)).await
}

#[tauri::command]
#[specta::specta]
pub async fn save_server_toml(
    state: tauri::State<'_, AppState>,
    id: String,
    text: String,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| save_toml(core, &id, &text)).await
}

/// The Form's fields written into the TOML tab's text (tab switch).
#[tauri::command]
#[specta::specta]
pub async fn server_draft_to_toml(draft: ServerDraft, text: String) -> CmdResult<String> {
    draft_to_toml(&draft, &text)
}

/// The TOML tab's text read into the Form (tab switch).
#[tauri::command]
#[specta::specta]
pub async fn server_toml_to_draft(id: String, text: String) -> CmdResult<ServerDraft> {
    toml_to_draft(&id, &text)
}

#[tauri::command]
#[specta::specta]
pub async fn remove_servers(
    state: tauri::State<'_, AppState>,
    ids: Vec<String>,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| remove(core, &ids)).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_server_clients(
    state: tauri::State<'_, AppState>,
    id: String,
    clients: ClientChoice,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| set_clients_for(core, &id, &clients)).await
}

#[tauri::command]
#[specta::specta]
pub async fn preview_server(
    state: tauri::State<'_, AppState>,
    draft: ServerDraft,
) -> CmdResult<Vec<ClientPreview>> {
    blocking(&state, move |core| preview(core, &draft)).await
}

#[tauri::command]
#[specta::specta]
pub async fn convert_snippet(
    state: tauri::State<'_, AppState>,
    text: String,
) -> CmdResult<Vec<ConvertedServer>> {
    blocking(&state, move |core| convert(core, &text)).await
}

#[tauri::command]
#[specta::specta]
pub async fn list_secret_aliases(state: tauri::State<'_, AppState>) -> CmdResult<Vec<String>> {
    blocking(&state, secret_aliases).await
}

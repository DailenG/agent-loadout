//! The MCP page at project scope (SPEC 13.2): the servers of a known
//! project's manifests with per-client pills from the project plan, the
//! store servers the folder inherits, and edits of `[servers.<name>]` in
//! `project.toml` (shared) or `project.local.toml` (private). Every edit is
//! one commit that trusts what Loadout wrote and applies it (core
//! `layering::project_servers`). An untrusted or changed project is read
//! only: edits are `trust_required` until it is reviewed and trusted, as
//! `loadout project trust` asks.

use std::path::Path;

use agent_loadout_core::adapters::ADAPTERS;
use agent_loadout_core::apply::render::Detected;
use agent_loadout_core::apply::{self, PlanOptions, ProjectSelection, ScopeFilter};
use agent_loadout_core::layering::project_servers::{ServerEdit, edit_server};
use agent_loadout_core::layering::{
    EntryBody, EntryLayer, KnownProject, Project, ProjectEntry, project_entries,
};
use agent_loadout_core::model::applied::Scope as TargetScope;
use agent_loadout_core::model::clients::ClientSelection;
use agent_loadout_core::model::project::ProjectServer;
use agent_loadout_core::model::server::ServerBody;
use agent_loadout_core::{CoreError, ErrorCode, apply::placeholder_path};
use serde::{Deserialize, Serialize};
use specta::Type;

use super::projects::{self, ProjectTrust};
use super::servers::{
    ClientChoice, ClientInfo, ClientPill, KeyValue, TransportKind, client_infos, launch_text,
    non_empty, pairs, pill_in, render_inputs, to_pairs,
};
use super::{AppState, ChangeResult, CmdResult, Core, IpcError, blocking, errors, scrubbed};

/// One of a project's two manifest files (SPEC 5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ManifestLayerKind {
    /// `project.toml`, committed with the project.
    Shared,
    /// `project.local.toml`, personal.
    Local,
}

impl From<ManifestLayerKind> for EntryLayer {
    fn from(kind: ManifestLayerKind) -> Self {
        match kind {
            ManifestLayerKind::Shared => Self::Shared,
            ManifestLayerKind::Local => Self::Local,
        }
    }
}

/// What a manifest entry does (SPEC 5.5, 6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ProjectServerKind {
    /// A complete project-only definition.
    Define,
    /// A store server (`from`) with project fields over it.
    Override,
    /// `enabled = false` alone: the same-named store server is hidden here.
    Suppress,
}

/// The project the MCP page is scoped to.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectScopeInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub trust: ProjectTrust,
    /// Content hash of the manifests as listed; edits send it back, and
    /// other content is `trust_required` (SPEC 17).
    pub hash: String,
    /// The files an edit can write: `shared` when `project.toml` exists,
    /// `local` always (created next to a shared file when needed).
    pub layers: Vec<ManifestLayerKind>,
    /// The merged `[defaults] clients`.
    pub default_clients: ClientChoice,
}

/// One server name of the project's manifests, both layers merged.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectServerRow {
    pub name: String,
    pub kind: ProjectServerKind,
    pub from: Option<String>,
    /// The files that define it.
    pub layers: Vec<ManifestLayerKind>,
    /// Where pill toggles and Delete write: the private file when it holds
    /// the entry, else the shared one.
    pub edit_layer: ManifestLayerKind,
    pub transport: Option<TransportKind>,
    /// The resolved command and arguments, or URL; references shown,
    /// values never.
    pub launch: String,
    /// The entry's client selection as the manifests write it (its
    /// `clients`, else the project's defaults), before this machine's
    /// exclusions: pill toggles start from this, never from the pills.
    pub clients: ClientChoice,
    /// No table of this name sets `clients`: the project's defaults apply.
    pub uses_defaults: bool,
    /// One pill per client in [`ProjectServerList::clients`] order. For a
    /// suppression, selected means hidden for that client.
    pub pills: Vec<ClientPill>,
    pub problems: Vec<IpcError>,
    /// The entry does not resolve (`unknown_template`, an incomplete
    /// definition).
    pub invalid: Option<IpcError>,
}

/// A store server the folder inherits from the system layer.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct InheritedServer {
    pub id: String,
    pub display_name: Option<String>,
    pub transport: Option<TransportKind>,
    pub launch: String,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectServerList {
    pub project: ProjectScopeInfo,
    pub servers: Vec<ProjectServerRow>,
    /// Store servers enabled system-wide that the manifests do not name.
    pub inherited: Vec<InheritedServer>,
    /// Clients with an MCP config; `supported` means a project config on
    /// this OS.
    pub clients: Vec<ClientInfo>,
    /// Ids of the store's servers, the choices for `from`.
    pub store_servers: Vec<String>,
    /// Manifest and plan warnings.
    pub warnings: Vec<IpcError>,
}

/// One manifest table as the project dialog edits it. Keys it does not
/// show (`unset`, `[os.*]`, `[client.*]`, `cwd`, ...) are kept on save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectServerDraft {
    pub name: String,
    pub layer: ManifestLayerKind,
    pub kind: ProjectServerKind,
    /// The store server an override starts from.
    pub from: Option<String>,
    /// Required for a definition; an override may leave it to `from`.
    pub transport: Option<TransportKind>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub env: Vec<KeyValue>,
    pub headers: Vec<KeyValue>,
    /// `None`: the project's defaults; an empty list: no client.
    pub clients: Option<ClientChoice>,
    /// A `project.local.toml` table over the same name in `project.toml`:
    /// a partial table, empty fields keep the shared entry's values. Read
    /// only (ignored on save).
    #[serde(default)]
    pub over_shared: bool,
}

fn selection(choice: &ClientChoice) -> ClientSelection {
    if choice.all_detected {
        return ClientSelection::AllDetected;
    }
    let mut ids: Vec<String> = Vec::new();
    for id in &choice.clients {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    ClientSelection::List(ids)
}

fn choice(selection: &ClientSelection) -> ClientChoice {
    match selection {
        ClientSelection::AllDetected => ClientChoice {
            all_detected: true,
            clients: Vec::new(),
        },
        ClientSelection::List(ids) => ClientChoice {
            all_detected: false,
            clients: ids.clone(),
        },
    }
}

/// The known project `id` and its loaded manifests.
fn known_project(core: &Core, id: &str) -> CmdResult<(KnownProject, Project)> {
    let store = core.open_store()?;
    let known = projects::find(core, store.as_ref(), id)?;
    let project = projects::loaded(&known)
        .map_err(|err| scrubbed(&core.env, &err, Some(&known.root)))?
        .clone();
    Ok((known, project))
}

fn table<'a>(
    project: &'a Project,
    layer: ManifestLayerKind,
    name: &str,
) -> Option<&'a ProjectServer> {
    let file = match layer {
        ManifestLayerKind::Shared => project.shared.as_ref(),
        ManifestLayerKind::Local => project.local.as_ref(),
    };
    file?.manifest.servers.as_ref()?.get(name)
}

/// The layers whose file names `name`, shared first.
fn layers_of(project: &Project, name: &str) -> Vec<ManifestLayerKind> {
    [ManifestLayerKind::Shared, ManifestLayerKind::Local]
        .into_iter()
        .filter(|layer| table(project, *layer, name).is_some())
        .collect()
}

/// Clients with an MCP config Loadout writes, `supported` meaning a
/// project or project-local config on this OS.
fn project_clients(core: &Core) -> Vec<ClientInfo> {
    let mut clients = client_infos(core);
    for client in &mut clients {
        let has_project_files = ADAPTERS
            .iter()
            .find(|spec| spec.id == client.id)
            .and_then(|spec| spec.mcp.as_ref())
            .is_some_and(|mcp| !(mcp.project.is_empty() && mcp.project_local.is_empty()));
        client.supported = client.supported && has_project_files;
    }
    clients
}

fn kind_of(entry: &ProjectEntry) -> ProjectServerKind {
    match (&entry.body, &entry.from) {
        (EntryBody::Suppression, _) => ProjectServerKind::Suppress,
        (_, Some(_)) => ProjectServerKind::Override,
        (_, None) => ProjectServerKind::Define,
    }
}

/// The MCP page at project scope.
pub fn list(core: &Core, id: &str) -> CmdResult<ProjectServerList> {
    let (known, project) = known_project(core, id)?;
    let root = known.root.as_path();
    let scrub = |err: &CoreError| scrubbed(&core.env, err, Some(root));
    let store = core.open_store()?;
    let (desired, memory) = render_inputs(core, store.as_ref())?;
    let ax = core.apply_env();
    let cx = ax.render_cx(memory);
    let options = PlanOptions {
        scope: ScopeFilter::Project,
        projects: ProjectSelection::Only(root.to_path_buf()),
        ..PlanOptions::default()
    };
    let plan = apply::preview(store.as_ref(), &ax, &options).map_err(|err| scrub(&err))?;
    let targets: Vec<_> = plan
        .targets
        .iter()
        .filter(|target| {
            target.project_id.as_deref() == Some(project.id.as_str())
                && matches!(
                    target.scope,
                    TargetScope::Project | TargetScope::ProjectLocal
                )
        })
        .collect();
    let clients = project_clients(core);
    let entries = project_entries(
        &project,
        &desired.servers,
        &core.env,
        desired.overlay.as_ref(),
    );
    let mut detected = Detected::default();
    let servers = entries
        .iter()
        .map(|entry| {
            let layers = layers_of(&project, &entry.name);
            let edit_layer = if layers.contains(&ManifestLayerKind::Local) {
                ManifestLayerKind::Local
            } else {
                ManifestLayerKind::Shared
            };
            let uses_defaults = layers.iter().all(|layer| {
                table(&project, *layer, &entry.name).is_none_or(|table| table.clients.is_none())
            });
            let selected = detected.project_clients(&cx, &desired, entry);
            let unsupported = |client: &ClientInfo| IpcError {
                code: ErrorCode::ScopeNotSupported,
                message: format!("{} has no project config on this OS", client.name),
            };
            let pills = clients
                .iter()
                .map(|client| {
                    pill_in(
                        targets.iter().copied(),
                        &entry.name,
                        &selected,
                        client,
                        unsupported(client),
                    )
                })
                .collect();
            let mut problems: Vec<IpcError> = Vec::new();
            for problem in targets
                .iter()
                .flat_map(|target| &target.entries)
                .filter(|planned| planned.server_id.as_deref() == Some(entry.name.as_str()))
                .filter_map(|planned| planned.problem.as_ref())
            {
                let problem = scrub(problem);
                let known = problems
                    .iter()
                    .any(|seen| seen.code == problem.code && seen.message == problem.message);
                if !known {
                    problems.push(problem);
                }
            }
            let (transport, launch, invalid) = match &entry.body {
                EntryBody::Server(Ok(server)) => (
                    server.body.transport.map(TransportKind::from),
                    launch_text(server),
                    None,
                ),
                EntryBody::Server(Err(err)) => (None, String::new(), Some(scrub(err))),
                EntryBody::Suppression => (None, String::new(), None),
            };
            ProjectServerRow {
                name: entry.name.clone(),
                kind: kind_of(entry),
                from: entry.from.clone(),
                layers,
                edit_layer,
                transport,
                launch,
                clients: choice(&entry.clients),
                uses_defaults,
                pills,
                problems,
                invalid,
            }
        })
        .collect();
    let named: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    let inherited = desired
        .servers
        .values()
        .filter(|server| !named.contains(&server.id.as_str()))
        .filter(|server| !detected.system_clients(&cx, &desired, server).is_empty())
        .map(|server| InheritedServer {
            id: server.id.clone(),
            display_name: server.body.display_name.clone(),
            transport: server.body.transport.map(TransportKind::from),
            launch: launch_text(server),
        })
        .collect();
    let mut layers = Vec::new();
    if project.shared.is_some() {
        layers.push(ManifestLayerKind::Shared);
    }
    layers.push(ManifestLayerKind::Local);
    let mut warnings: Vec<IpcError> = project.warnings.iter().map(scrub).collect();
    warnings.extend(errors(&plan.warnings).into_iter().map(|warning| IpcError {
        message: super::scrub(&core.env, &warning.message, Some(root)),
        ..warning
    }));
    Ok(ProjectServerList {
        project: ProjectScopeInfo {
            id: project.id.clone(),
            name: project.name().to_owned(),
            path: placeholder_path(&core.env, root, None),
            trust: projects::trust_of(&known),
            hash: project.hash(),
            layers,
            default_clients: choice(&agent_loadout_core::layering::default_clients(&project)),
        },
        servers,
        inherited,
        clients,
        store_servers: desired.servers.keys().cloned().collect(),
        warnings,
    })
}

/// The dialog's view of `[servers.<name>]` in the `layer` file.
fn draft_of(
    name: &str,
    layer: ManifestLayerKind,
    server: &ProjectServer,
    over_shared: bool,
) -> ProjectServerDraft {
    let body = &server.body;
    let kind = if server.is_suppression() {
        ProjectServerKind::Suppress
    } else if server.from.is_some() {
        ProjectServerKind::Override
    } else {
        ProjectServerKind::Define
    };
    ProjectServerDraft {
        name: name.to_owned(),
        layer,
        kind,
        from: server.from.clone(),
        transport: body.transport.map(TransportKind::from),
        command: body.command.clone(),
        args: body.args.clone().unwrap_or_default(),
        url: body.url.clone(),
        env: to_pairs(&body.env),
        headers: to_pairs(&body.headers),
        clients: server.clients.as_ref().map(choice),
        over_shared,
    }
}

/// `[servers.<name>]` of the `layer` file, for the edit dialog.
pub fn get(
    core: &Core,
    id: &str,
    name: &str,
    layer: ManifestLayerKind,
) -> CmdResult<ProjectServerDraft> {
    let (_, project) = known_project(core, id)?;
    let server = table(&project, layer, name).ok_or_else(|| {
        IpcError::from(CoreError::new(
            ErrorCode::NotFound,
            format!("the manifest has no server {name:?} in that file"),
        ))
    })?;
    let over_shared = layer == ManifestLayerKind::Local
        && table(&project, ManifestLayerKind::Shared, name).is_some();
    Ok(draft_of(name, layer, server, over_shared))
}

/// Writes the dialog's fields into `server`, keeping what it does not
/// show. A transport's fields replace the other transport's.
fn apply_draft(
    server: &mut ProjectServer,
    draft: &ProjectServerDraft,
    store_servers: &[String],
    create: bool,
) -> Result<(), CoreError> {
    server.clients = draft.clients.as_ref().map(selection);
    if draft.kind == ProjectServerKind::Suppress {
        server.enabled = Some(false);
        server.from = None;
        server.unset = None;
        server.body = ServerBody::default();
        return Ok(());
    }
    if server.enabled == Some(false) {
        server.enabled = None;
    }
    server.from = match draft.kind {
        ProjectServerKind::Override => {
            let from = non_empty(&draft.from).ok_or_else(|| {
                CoreError::new(
                    ErrorCode::MissingField,
                    "an override needs the store server it starts from",
                )
            })?;
            if !store_servers.contains(&from) {
                return Err(CoreError::new(
                    ErrorCode::NotFound,
                    format!("server {from:?} does not exist in the store"),
                ));
            }
            Some(from)
        }
        _ => None,
    };
    let body = &mut server.body;
    // A new definition defaults to stdio; an existing table keeps what it
    // has (a local table layering over the shared entry may name no
    // transport at all).
    let transport = match (draft.kind, draft.transport) {
        (ProjectServerKind::Define, None) if create => Some(TransportKind::Stdio),
        (_, transport) => transport,
    };
    body.transport = transport.map(Into::into);
    let stdio = matches!(transport, Some(TransportKind::Stdio));
    let remote = matches!(transport, Some(TransportKind::Http | TransportKind::Sse));
    // An override without a transport may set fields of either kind.
    body.command = if remote {
        None
    } else {
        non_empty(&draft.command)
    };
    body.args = (!remote && !draft.args.is_empty()).then(|| draft.args.clone());
    body.env = if remote {
        None
    } else {
        pairs(&draft.env, "environment")?
    };
    body.url = if stdio { None } else { non_empty(&draft.url) };
    body.headers = if stdio {
        None
    } else {
        pairs(&draft.headers, "header")?
    };
    if remote {
        body.cwd = None;
    }
    Ok(())
}

/// Runs one manifest edit of known project `id` (one commit with its
/// apply); core messages naming the folder are scrubbed.
fn edit(
    core: &Core,
    id: &str,
    hash: &str,
    layer: ManifestLayerKind,
    name: &str,
    change: ServerEdit,
) -> CmdResult<ChangeResult> {
    let store = core.open_store_for_change()?;
    let known = projects::find(core, Some(&store), id)?;
    let root: &Path = &known.root;
    let (report, commit) = edit_server(
        &store,
        &core.apply_env(),
        root,
        hash,
        layer.into(),
        name,
        change,
    )
    .map_err(|err| scrubbed(&core.env, &err, Some(root)))?;
    Ok(ChangeResult::new(
        &core.env,
        commit.as_ref(),
        report.as_ref(),
    ))
}

/// Saves the dialog: a new table (`create`) or the fields of the table in
/// the draft's layer, keeping its other keys.
pub fn save(
    core: &Core,
    id: &str,
    hash: &str,
    draft: &ProjectServerDraft,
    create: bool,
) -> CmdResult<ChangeResult> {
    let (_, project) = known_project(core, id)?;
    let name = draft.name.trim();
    let mut server = if create {
        ProjectServer::default()
    } else {
        table(&project, draft.layer, name).cloned().ok_or_else(|| {
            IpcError::from(CoreError::new(
                ErrorCode::NotFound,
                format!("the manifest has no server {name:?} in that file"),
            ))
        })?
    };
    let store_servers = match core.open_store()? {
        Some(store) => {
            let (desired, _) = render_inputs(core, Some(&store))?;
            desired.servers.keys().cloned().collect()
        }
        None => Vec::new(),
    };
    apply_draft(&mut server, draft, &store_servers, create)?;
    edit(
        core,
        id,
        hash,
        draft.layer,
        name,
        ServerEdit::Set {
            server: Box::new(server),
            create,
        },
    )
}

/// Removes `[servers.<name>]` from the `layer` file.
pub fn remove(
    core: &Core,
    id: &str,
    hash: &str,
    name: &str,
    layer: ManifestLayerKind,
) -> CmdResult<ChangeResult> {
    edit(core, id, hash, layer, name, ServerEdit::Remove)
}

/// Sets the clients of `[servers.<name>]` in the `layer` file (pill
/// toggles, Enable All, Disable All); `None` goes back to the project's
/// defaults.
pub fn set_clients(
    core: &Core,
    id: &str,
    hash: &str,
    name: &str,
    layer: ManifestLayerKind,
    clients: Option<&ClientChoice>,
) -> CmdResult<ChangeResult> {
    edit(
        core,
        id,
        hash,
        layer,
        name,
        ServerEdit::Clients(clients.map(selection)),
    )
}

#[tauri::command]
#[specta::specta]
pub async fn list_project_servers(
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> CmdResult<ProjectServerList> {
    blocking(&state, move |core| list(core, &project_id)).await
}

#[tauri::command]
#[specta::specta]
pub async fn get_project_server(
    state: tauri::State<'_, AppState>,
    project_id: String,
    name: String,
    layer: ManifestLayerKind,
) -> CmdResult<ProjectServerDraft> {
    blocking(&state, move |core| get(core, &project_id, &name, layer)).await
}

#[tauri::command]
#[specta::specta]
pub async fn save_project_server(
    state: tauri::State<'_, AppState>,
    project_id: String,
    hash: String,
    draft: ProjectServerDraft,
    create: bool,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| {
        save(core, &project_id, &hash, &draft, create)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn remove_project_server(
    state: tauri::State<'_, AppState>,
    project_id: String,
    hash: String,
    name: String,
    layer: ManifestLayerKind,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| {
        remove(core, &project_id, &hash, &name, layer)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn set_project_server_clients(
    state: tauri::State<'_, AppState>,
    project_id: String,
    hash: String,
    name: String,
    layer: ManifestLayerKind,
    clients: Option<ClientChoice>,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| {
        set_clients(core, &project_id, &hash, &name, layer, clients.as_ref())
    })
    .await
}

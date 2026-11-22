//! Tools page commands (SPEC 13.7): every client with an MCP config, its
//! detection, user and project files, and a capability summary; opening a
//! client's folder takes a client id, never a path from the frontend
//! (SPEC 17).

use std::path::Path;

use agent_loadout_core::adapters::detect::detect;
use agent_loadout_core::adapters::location::Location;
use agent_loadout_core::adapters::spec::Tier;
use agent_loadout_core::adapters::{ADAPTERS, AdapterSpec, adapter};
use agent_loadout_core::apply::placeholder_path;
use agent_loadout_core::apply::render::DesiredState;
use agent_loadout_core::model::applied::Scope as TargetScope;
use agent_loadout_core::model::clients::ClientPrefs;
use agent_loadout_core::store::Machine;
use agent_loadout_core::{CoreError, ErrorCode};
use serde::Serialize;
use specta::Type;

use super::{AppState, CmdResult, Core, IpcError, NO_REGISTRY, blocking};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TierKind {
    A,
    B,
    C,
    D,
}

impl From<Tier> for TierKind {
    fn from(tier: Tier) -> Self {
        match tier {
            Tier::A => Self::A,
            Tier::B => Self::B,
            Tier::C => Self::C,
            Tier::D => Self::D,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ToolFile {
    pub path: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Type)]
pub struct Transports {
    pub stdio: bool,
    pub http: bool,
    pub sse: bool,
}

/// One client on the Tools page.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub id: String,
    pub name: String,
    pub tier: TierKind,
    pub detected: bool,
    /// Has an MCP config on this operating system.
    pub supported: bool,
    /// Loadout only reads this client (`vscode-legacy`).
    pub read_only: bool,
    /// User-scope MCP files (the first existing one is used).
    pub user_files: Vec<ToolFile>,
    /// Project files, relative to the project folder (`<P>/...`) or absolute
    /// for a user file keyed by project.
    pub project_files: Vec<String>,
    pub transports: Transports,
    /// The client expands environment references (`${env:NAME}`) itself.
    pub env_refs: bool,
    /// The client reads its file only at launch.
    pub restart_required: bool,
    pub warnings: Vec<IpcError>,
}

fn project_file(location: &Location, core: &Core) -> Option<String> {
    match location {
        Location::Project(rel) => Some(format!("<P>/{rel}")),
        other => other
            .resolve(&core.env, None)
            .map(|path| placeholder_path(&core.env, &path, None)),
    }
}

fn tool(core: &Core, spec: &AdapterSpec, desired: Option<&DesiredState>) -> Option<ToolInfo> {
    let mcp = spec.mcp.as_ref()?;
    let detection = detect(spec, &core.env, core.fs.as_ref(), &NO_REGISTRY);
    let prefs = prefs_of(desired, spec.id);
    let mut warnings: Vec<IpcError> = detection.warnings.iter().map(IpcError::from).collect();
    let user_files =
        match spec.mcp_targets(&core.env, core.fs.as_ref(), TargetScope::User, None, &prefs) {
            Ok(targets) => targets
                .iter()
                .map(|target| ToolFile {
                    // Opening goes through `reveal_tool_folder` by client id.
                    path: placeholder_path(&core.env, &target.file, None),
                    exists: core.fs.exists(&target.file),
                })
                .collect(),
            Err(err) => {
                if err.code() != ErrorCode::NotSupportedOnOs {
                    warnings.push(err.into());
                }
                Vec::new()
            }
        };
    Some(ToolInfo {
        id: spec.id.to_owned(),
        name: spec.display_name.to_owned(),
        tier: spec.tier.into(),
        detected: detection.detected(),
        supported: mcp.oses.contains(&core.os()),
        read_only: mcp.read_only,
        user_files,
        project_files: mcp
            .project
            .iter()
            .chain(mcp.project_local)
            .filter_map(|location| project_file(location, core))
            .collect(),
        transports: Transports {
            stdio: mcp.caps.transports.stdio,
            http: mcp.caps.transports.http,
            sse: mcp.caps.transports.sse,
        },
        env_refs: mcp.caps.env_ref.is_some(),
        restart_required: mcp.caps.restart_required,
        warnings,
    })
}

/// The store's desired state, for `clients.toml` preferences (omp profiles);
/// none without a store.
fn desired_state(core: &Core) -> Result<Option<DesiredState>, CoreError> {
    let Some(store) = core.open_store()? else {
        return Ok(None);
    };
    let machine = Machine::load(core.fs.as_ref(), &core.home.state_dir())?;
    DesiredState::load(&store, machine.as_ref().map(|m| m.id.as_str())).map(Some)
}

fn prefs_of(desired: Option<&DesiredState>, client: &str) -> ClientPrefs {
    desired.map_or_else(ClientPrefs::default, |desired| desired.prefs(client))
}

/// Every client with an MCP config, in table order.
pub fn list(core: &Core) -> CmdResult<Vec<ToolInfo>> {
    let desired = desired_state(core)?;
    Ok(ADAPTERS
        .iter()
        .filter_map(|spec| tool(core, spec, desired.as_ref()))
        .collect())
}

/// The folder a client's user file lives in (the file's parent), opened in
/// the OS file manager. The path comes from the adapter table and the same
/// preferences as [`list`], not the caller.
pub fn folder_of(core: &Core, client: &str) -> Result<std::path::PathBuf, CoreError> {
    let spec = adapter(client).ok_or_else(|| {
        CoreError::new(
            ErrorCode::UnknownClient,
            format!("unknown client {client:?}"),
        )
    })?;
    let prefs = prefs_of(desired_state(core)?.as_ref(), spec.id);
    let targets = spec.mcp_targets(&core.env, core.fs.as_ref(), TargetScope::User, None, &prefs)?;
    let file = targets
        .first()
        .map(|target| target.file.clone())
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::ScopeNotSupported,
                format!("{} has no user MCP file", spec.display_name),
            )
        })?;
    let mut folder = file.parent().map(Path::to_path_buf).unwrap_or(file);
    while !core.fs.exists(&folder) {
        match folder.parent() {
            Some(parent) => folder = parent.to_path_buf(),
            None => break,
        }
    }
    Ok(folder)
}

fn open_folder(folder: &Path) -> Result<(), CoreError> {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(folder)
        .spawn()
        .map(drop)
        .map_err(|err| {
            CoreError::new(
                ErrorCode::ToolNotFound,
                format!("cannot start {program}: {err}"),
            )
            .with_path(folder)
        })
}

#[tauri::command]
#[specta::specta]
pub async fn list_tools(state: tauri::State<'_, AppState>) -> CmdResult<Vec<ToolInfo>> {
    blocking(&state, list).await
}

/// Opens the folder of `client`'s user MCP file.
#[tauri::command]
#[specta::specta]
pub async fn reveal_tool_folder(
    state: tauri::State<'_, AppState>,
    client: String,
) -> CmdResult<()> {
    blocking(&state, move |core| {
        let folder = folder_of(core, &client)?;
        open_folder(&folder).map_err(IpcError::from)
    })
    .await
}

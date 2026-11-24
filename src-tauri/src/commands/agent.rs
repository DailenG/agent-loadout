//! Settings > Agent integration (#8, M46): the read-only Loadout skill for
//! coding agents, bundled with the app (a resource) and installed as the
//! store skill `loadout`, enabled per client through the normal skill sync.
//! At startup the frontend asks once for [`refresh_agent_skill`]: an older
//! store copy the user did not edit is updated (a commit), and an edited
//! one is reported as customized, never touched.

use std::sync::atomic::{AtomicBool, Ordering};

use agent_loadout_core::CoreError;
use agent_loadout_core::adapters::adapter;
use agent_loadout_core::agent::{self, AgentStatus, CopyState, Refresh};
use serde::Serialize;
use specta::Type;
use tauri::Manager;

use super::skills::{SkillChange, change_of, sx};
use super::{AppState, CmdResult, Core, blocking, scrubbed};

/// How the store copy compares with the bundled one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AgentSkillState {
    Current,
    /// Updated at the next start.
    Outdated,
    /// Edited by the user: never updated.
    Customized,
    /// From a newer Loadout (a synced store): never replaced.
    Newer,
}

impl From<CopyState> for AgentSkillState {
    fn from(state: CopyState) -> Self {
        match state {
            CopyState::Current => Self::Current,
            CopyState::Outdated => Self::Outdated,
            CopyState::Customized => Self::Customized,
            CopyState::Newer => Self::Newer,
        }
    }
}

/// The Loadout skill in the store.
#[derive(Debug, Clone, Serialize, Type)]
pub struct AgentSkillCopy {
    pub version: String,
    pub state: AgentSkillState,
}

/// One client with a skill folder.
#[derive(Debug, Clone, Serialize, Type)]
pub struct AgentSkillClient {
    pub id: String,
    pub name: String,
    pub detected: bool,
    pub enabled: bool,
}

/// Settings > Agent integration.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillStatus {
    pub bundled_version: String,
    /// `None` when the skill is not in the store.
    pub copy: Option<AgentSkillCopy>,
    /// Another store skill uses the id `loadout`.
    pub taken: bool,
    /// Every client with a skill folder, omp first.
    pub clients: Vec<AgentSkillClient>,
}

/// What the startup refresh did, for a notification.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AgentSkillNotice {
    /// `needs_attention`: a client folder needs a decision after the
    /// update (a changed copy, a collision); the Skills page shows it.
    Updated {
        from: String,
        to: String,
        needs_attention: bool,
    },
    Customized {
        version: String,
        bundled: String,
    },
}

fn status_of(status: AgentStatus) -> AgentSkillStatus {
    AgentSkillStatus {
        bundled_version: status.bundled_version.to_owned(),
        copy: status.copy.map(|copy| AgentSkillCopy {
            version: copy.version,
            state: copy.state.into(),
        }),
        taken: status.taken,
        clients: status
            .clients
            .into_iter()
            .map(|client| AgentSkillClient {
                id: client.client.to_owned(),
                name: adapter(client.client)
                    .map_or(client.client, |spec| spec.display_name)
                    .to_owned(),
                detected: client.detected,
                enabled: client.enabled,
            })
            .collect(),
    }
}

/// The Loadout skill's state. Reads only.
pub fn status(core: &Core) -> CmdResult<AgentSkillStatus> {
    let env = &core.env;
    let store = core.open_store()?;
    agent::status(store.as_ref(), &sx(core))
        .map(status_of)
        .map_err(|err| scrubbed(env, &err, None))
}

/// Enables the Loadout skill for `client` (installing it first when it is
/// not in the store), or disables it there.
pub fn set_client(core: &Core, client: &str, enabled: bool) -> CmdResult<SkillChange> {
    let env = &core.env;
    let clients = [client.to_owned()];
    let change = if enabled {
        let store = core.open_store_for_change()?;
        agent::install(&store, &sx(core), &clients, false)
    } else {
        match core.open_store()? {
            Some(store) => agent::remove(&store, &sx(core), &clients),
            None => Ok(agent::Change::default()),
        }
    }
    .map_err(|err| scrubbed(env, &err, None))?;
    Ok(change_of(env, change.commit.as_ref(), &change.report))
}

/// Removes the Loadout skill from the store, with its links and copies.
pub fn remove(core: &Core) -> CmdResult<SkillChange> {
    let env = &core.env;
    let Some(store) = core.open_store()? else {
        return Ok(change_of(env, None, &Default::default()));
    };
    let change = agent::remove(&store, &sx(core), &[]).map_err(|err| scrubbed(env, &err, None))?;
    Ok(change_of(env, change.commit.as_ref(), &change.report))
}

/// Replaces the store copy with the bundled one, edited or not ("Restore
/// Loadout's version"); the clients it is enabled for stay as they are.
pub fn reset(core: &Core) -> CmdResult<SkillChange> {
    let env = &core.env;
    let store = core.open_store_for_change()?;
    let change =
        agent::install(&store, &sx(core), &[], true).map_err(|err| scrubbed(env, &err, None))?;
    Ok(change_of(env, change.commit.as_ref(), &change.report))
}

/// The startup refresh ([`agent::startup_refresh`]); `None` when there is
/// nothing to say or no store.
pub fn refresh(core: &Core) -> CmdResult<Option<AgentSkillNotice>> {
    let env = &core.env;
    let sx = sx(core);
    if !agent::written_older(sx.fs, sx.home) {
        return Ok(None);
    }
    let Some(store) = core.open_store()? else {
        return Ok(None);
    };
    let refreshed = agent::startup_refresh(&store, &sx).map_err(|err| scrubbed(env, &err, None))?;
    Ok(refreshed.map(|refresh| match refresh {
        Refresh::Updated {
            from, to, report, ..
        } => AgentSkillNotice::Updated {
            from,
            to,
            needs_attention: report.needs_attention(),
        },
        Refresh::Customized { version, bundled } => {
            AgentSkillNotice::Customized { version, bundled }
        }
    }))
}

/// Set once the startup refresh ran in this process.
static REFRESHED: AtomicBool = AtomicBool::new(false);

#[tauri::command]
#[specta::specta]
pub async fn agent_skill_status(state: tauri::State<'_, AppState>) -> CmdResult<AgentSkillStatus> {
    blocking(&state, status).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_agent_skill_client(
    state: tauri::State<'_, AppState>,
    client: String,
    enabled: bool,
) -> CmdResult<SkillChange> {
    blocking(&state, move |core| set_client(core, &client, enabled)).await
}

#[tauri::command]
#[specta::specta]
pub async fn remove_agent_skill(state: tauri::State<'_, AppState>) -> CmdResult<SkillChange> {
    blocking(&state, remove).await
}

#[tauri::command]
#[specta::specta]
pub async fn reset_agent_skill(state: tauri::State<'_, AppState>) -> CmdResult<SkillChange> {
    blocking(&state, reset).await
}

/// Runs the startup refresh once per app start; later calls answer `None`.
#[tauri::command]
#[specta::specta]
pub async fn refresh_agent_skill(
    state: tauri::State<'_, AppState>,
) -> CmdResult<Option<AgentSkillNotice>> {
    if REFRESHED.swap(true, Ordering::SeqCst) {
        return Ok(None);
    }
    blocking(&state, refresh).await
}

/// Opens the folder of the bundled skill: the app's resource folder, or
/// (in a build without it) a copy in the data directory's cache.
#[tauri::command]
#[specta::specta]
pub async fn reveal_agent_skill(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> CmdResult<()> {
    let resource = app
        .path()
        .resource_dir()
        .ok()
        .map(|dir| dir.join(agent::RESOURCE_DIR));
    blocking(&state, move |core| {
        let fs = core.fs.as_ref();
        let dir = match resource.filter(|dir| agent::is_bundled_copy(fs, dir)) {
            Some(dir) => dir,
            None => {
                agent::materialize(fs, &core.home).map_err(|err| scrubbed(&core.env, &err, None))?
            }
        };
        super::tools::open_folder(&dir).map_err(|err: CoreError| scrubbed(&core.env, &err, None))
    })
    .await
}

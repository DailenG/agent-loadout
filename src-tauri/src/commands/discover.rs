//! Discover projects (#3): the folders the clients already know (Claude
//! Code projects, Codex trusted projects, omp and Pi sessions) that this
//! machine does not know as projects, and adding the chosen ones. Every
//! operation is the core's (`layering::discover`), as `loadout project
//! discover` runs it: one store commit, never a trust (SPEC 17). Folders go
//! out under their placeholder roots; candidates are named by id.

use agent_loadout_core::apply::placeholder_path;
use agent_loadout_core::layering::discover;
use agent_loadout_core::layering::projects;
use serde::Serialize;
use specta::Type;

use super::projects::{FoundManifest, ProjectTrust, find, trust_of};
use super::{AppState, CmdResult, Core, IpcError, blocking, scrubbed};

/// A folder the clients know that is not a project here yet.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredProject {
    /// Names the folder in `add_discovered_projects`; stable across scans.
    pub id: String,
    /// The folder's own name.
    pub name: String,
    /// The folder under its placeholder root.
    pub path: String,
    /// Client ids that know the folder.
    pub clients: Vec<String>,
    /// The folder's manifest, when it has one (adding binds it); `None`:
    /// adding writes a private one, as Init does, without trusting it.
    pub manifest: Option<FoundManifest>,
    /// Why the folder cannot be added.
    pub error: Option<IpcError>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDiscovery {
    pub candidates: Vec<DiscoveredProject>,
    /// Client files that could not be read (the others still were).
    pub warnings: Vec<IpcError>,
}

/// A folder added to this machine's projects.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AddedDiscoveredProject {
    /// The candidate id.
    pub candidate: String,
    /// The project id.
    pub id: String,
    pub path: String,
    /// A private manifest was written (the folder had none).
    pub created_manifest: bool,
    /// Never `trusted`: each waits for its review.
    pub trust: ProjectTrust,
}

/// A folder that could not be added.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NotAddedProject {
    pub candidate: String,
    pub path: String,
    pub error: IpcError,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredProjectsAdded {
    pub commit: Option<String>,
    pub added: Vec<AddedDiscoveredProject>,
    pub failed: Vec<NotAddedProject>,
}

/// The folders the clients know that this machine does not know as
/// projects. Reads only.
pub fn discover_list(core: &Core) -> CmdResult<ProjectDiscovery> {
    let env = &core.env;
    let fs = core.fs.as_ref();
    let store = core.open_store()?;
    let found = discover::discover(store.as_ref(), env, fs, &core.home)
        .map_err(|err| scrubbed(env, &err, None))?;
    Ok(ProjectDiscovery {
        candidates: found
            .candidates
            .iter()
            .map(|candidate| {
                let root = Some(candidate.root.as_path());
                DiscoveredProject {
                    id: candidate.id.clone(),
                    name: candidate.name.clone(),
                    path: placeholder_path(env, &candidate.root, None),
                    clients: candidate
                        .clients
                        .iter()
                        .map(|client| (*client).to_owned())
                        .collect(),
                    manifest: candidate.manifest.as_ref().and_then(|project| {
                        Some(FoundManifest {
                            id: project.id.clone(),
                            name: project.name().to_owned(),
                            layout: projects::layout(fs, &candidate.root)?.into(),
                            known: false,
                        })
                    }),
                    error: candidate
                        .problem
                        .as_ref()
                        .map(|err| scrubbed(env, err, root)),
                }
            })
            .collect(),
        warnings: found
            .warnings
            .iter()
            .map(|err| scrubbed(env, err, None))
            .collect(),
    })
}

/// Adds the discovered folders `ids` in one store commit, untrusted.
pub fn add_discovered(core: &Core, ids: &[String]) -> CmdResult<DiscoveredProjectsAdded> {
    let env = &core.env;
    let store = core.open_store_for_change()?;
    let done = discover::add(&store, env, core.fs.as_ref(), &core.home, ids)
        .map_err(|err| scrubbed(env, &err, None))?;
    let trust = |id: &str| {
        find(core, Some(&store), id).map_or(ProjectTrust::Untrusted, |known| trust_of(&known))
    };
    Ok(DiscoveredProjectsAdded {
        commit: done.commit.as_ref().map(ToString::to_string),
        added: done
            .added
            .iter()
            .map(|added| AddedDiscoveredProject {
                candidate: added.candidate.clone(),
                id: added.project_id.clone(),
                path: placeholder_path(env, &added.root, None),
                created_manifest: added.created_manifest,
                trust: trust(&added.project_id),
            })
            .collect(),
        failed: done
            .failed
            .iter()
            .map(|failed| NotAddedProject {
                candidate: failed.candidate.clone(),
                path: placeholder_path(env, &failed.root, None),
                error: scrubbed(env, &failed.error, Some(&failed.root)),
            })
            .collect(),
    })
}

#[tauri::command]
#[specta::specta]
pub async fn discover_projects(state: tauri::State<'_, AppState>) -> CmdResult<ProjectDiscovery> {
    blocking(&state, discover_list).await
}

#[tauri::command]
#[specta::specta]
pub async fn add_discovered_projects(
    state: tauri::State<'_, AppState>,
    ids: Vec<String>,
) -> CmdResult<DiscoveredProjectsAdded> {
    blocking(&state, move |core| add_discovered(core, &ids)).await
}

//! The effective config matrix of a project folder (SPEC 6.4, 13.3): rows
//! are server names, columns clients; each cell is the core's effective
//! entry (`loadout effective <P>`) with its provenance: the winner's layer
//! and file, the entries it shadows, the other clients that read the same
//! file, its plan state, warnings and the masked native snippet.

use std::collections::BTreeMap;

use agent_loadout_core::layering::{Layer, effective};
use serde::Serialize;
use specta::Type;

use super::projects::validate_known_root;
use super::servers::client_infos;
use super::{AppState, CmdResult, Core, EntryStateKind, IpcError, blocking, scrub, scrubbed};

/// The layer an entry comes from (SPEC 6.4); the legend's S, P, L, F, C.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LayerKind {
    System,
    Project,
    ProjectLocal,
    /// An entry Loadout does not own in one of the client's files.
    Foreign,
    /// An entry the client reads from another tool's file.
    ClientCompat,
}

impl From<&Layer> for LayerKind {
    fn from(layer: &Layer) -> Self {
        match layer {
            Layer::System => Self::System,
            Layer::Project => Self::Project,
            Layer::ProjectLocal => Self::ProjectLocal,
            Layer::Foreign(_) => Self::Foreign,
            Layer::ClientCompat(_) => Self::ClientCompat,
        }
    }
}

/// A layer and the file (under its placeholder root) of one entry.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LayerRef {
    pub layer: LayerKind,
    pub file: String,
}

/// One client's entry of a server name: the matrix cell and its
/// provenance chain.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatrixCell {
    pub client: String,
    /// The entry the client uses.
    pub winner: LayerRef,
    /// `store:<id>`, `store:<id>+project`, `project:<name>` or `foreign`.
    pub source: String,
    /// The client starts it (not suppressed, disabled or blocked).
    pub loaded: bool,
    pub shadowed: Vec<LayerRef>,
    /// Other clients that read the winner's file and container.
    pub also_visible_to: Vec<String>,
    pub status: Option<EntryStateKind>,
    pub warnings: Vec<IpcError>,
    /// The winner's native entry as its file writes it, masked.
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatrixRow {
    pub name: String,
    /// One cell per client that loads (or would load) the name; a client
    /// whose own layers coexist (Cursor) can have several.
    pub cells: Vec<MatrixCell>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MatrixClient {
    pub id: String,
    pub name: String,
    pub detected: bool,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveMatrix {
    pub project_id: String,
    pub project_name: String,
    /// The project folder under its placeholder root.
    pub path: String,
    /// Columns: clients with an entry here, then other detected clients,
    /// in table order.
    pub clients: Vec<MatrixClient>,
    pub rows: Vec<MatrixRow>,
    /// Files that could not be read and other problems outside any entry.
    pub warnings: Vec<IpcError>,
}

/// The effective matrix of known project `id`, at the folder this machine
/// knows it by. Reads files; writes nothing.
pub fn matrix(core: &Core, id: &str) -> CmdResult<EffectiveMatrix> {
    let store = core.open_store()?;
    let (root, name) = validate_known_root(core, store.as_ref(), id)?;
    let env = &core.env;
    let view = effective(store.as_ref(), &core.apply_env(), &root, None)
        .map_err(|err| scrubbed(env, &err, Some(&root)))?;
    let mut rows: BTreeMap<String, Vec<MatrixCell>> = BTreeMap::new();
    for (client, entries) in &view.clients {
        for entry in entries {
            let cell = MatrixCell {
                client: (*client).to_owned(),
                winner: LayerRef {
                    layer: (&entry.winner.layer).into(),
                    file: entry.winner.file.clone(),
                },
                source: entry.winner.source.clone(),
                loaded: entry.loaded,
                shadowed: entry
                    .shadowed
                    .iter()
                    .map(|shadowed| LayerRef {
                        layer: (&shadowed.layer).into(),
                        file: shadowed.file.clone(),
                    })
                    .collect(),
                also_visible_to: entry
                    .also_visible_to
                    .iter()
                    .map(|id| (*id).to_owned())
                    .collect(),
                status: entry.status.map(Into::into),
                warnings: entry
                    .warnings
                    .iter()
                    .map(|warning| scrubbed(env, warning, Some(&root)))
                    .collect(),
                // Masked by the plan (inline values) and again line by line
                // (literals under sensitive keys, token formats).
                snippet: entry
                    .snippet
                    .as_deref()
                    .map(|text| super::apply::mask_diff(&scrub(env, text, Some(&root)))),
            };
            rows.entry(entry.name.clone()).or_default().push(cell);
        }
    }
    let infos = client_infos(core);
    let used = |id: &str| rows.values().flatten().any(|cell| cell.client == id);
    let clients = infos
        .iter()
        .filter(|info| view.clients.contains_key(info.id.as_str()))
        .filter(|info| used(&info.id) || info.detected)
        .map(|info| MatrixClient {
            id: info.id.clone(),
            name: info.name.clone(),
            detected: info.detected,
        })
        .collect();
    Ok(EffectiveMatrix {
        project_id: id.to_owned(),
        project_name: name,
        path: agent_loadout_core::apply::placeholder_path(env, &root, None),
        clients,
        rows: rows
            .into_iter()
            .map(|(name, cells)| MatrixRow { name, cells })
            .collect(),
        warnings: view
            .warnings
            .iter()
            .map(|warning| scrubbed(env, warning, Some(&root)))
            .collect(),
    })
}

#[tauri::command]
#[specta::specta]
pub async fn effective_matrix(
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> CmdResult<EffectiveMatrix> {
    blocking(&state, move |core| matrix(core, &project_id)).await
}

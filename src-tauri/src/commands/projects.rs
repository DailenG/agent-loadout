//! Projects page commands (SPEC 13.3, 17): the known projects with trust
//! and pending counts, Add folder (picked in the OS dialog; the folder
//! stays in the backend under a token), Init manifest (private by
//! default), the trust review with the manifest diff, trust, apply and
//! remove. Every operation is the core's (`layering::projects`), as
//! `loadout project` runs it: one store commit, applied in instant mode.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use agent_loadout_core::apply::{
    self, ApplyRequest, EntryState, PlanOptions, ProjectSelection, ScopeFilter, placeholder_path,
};
use agent_loadout_core::layering::projects::{self, Layout, ManifestChange};
use agent_loadout_core::layering::{
    KnownProject, Project, ProjectStatus, TrustState, TrustStore, known_projects, resolved,
    same_folder, same_path,
};
use agent_loadout_core::model::applied::ApplyTrigger;
use agent_loadout_core::store::Store;
use agent_loadout_core::{CoreError, ErrorCode};
use serde::Serialize;
use specta::Type;

use super::{AppState, ChangeResult, CmdResult, Core, IpcError, blocking, count, scrub, scrubbed};

/// The manifest files of a project folder (SPEC 5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ManifestLayout {
    /// Only `.loadout/project.local.toml`.
    Private,
    /// Only `.loadout/project.toml`.
    Shared,
    /// Both files.
    SharedAndLocal,
}

impl From<Layout> for ManifestLayout {
    fn from(layout: Layout) -> Self {
        match layout {
            Layout::Private => Self::Private,
            Layout::Shared => Self::Shared,
            Layout::SharedAndLocal => Self::SharedAndLocal,
        }
    }
}

/// Trust of a known project's manifests (SPEC 17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ProjectTrust {
    Trusted,
    /// Never trusted on this machine.
    Untrusted,
    /// Changed since trusted (for example after `git pull`).
    Changed,
    /// The folder is gone.
    Missing,
    /// No usable manifest (see the row's error).
    Invalid,
}

/// One project of the Projects page.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    /// The folder under its placeholder root (`~/Code/app`).
    pub path: String,
    pub layout: Option<ManifestLayout>,
    pub trust: ProjectTrust,
    pub error: Option<IpcError>,
    /// Server names the manifests define.
    pub servers: u32,
    /// Clients with an entry of this project.
    pub clients: u32,
    /// Entries waiting for an apply, a decision, or blocked.
    pub pending: u32,
}

/// A manifest the folder holds already (Add binds it).
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FoundManifest {
    pub id: String,
    pub name: String,
    pub layout: ManifestLayout,
    /// This machine knows the project already.
    pub known: bool,
}

/// A folder picked in the OS dialog, as the frontend sees it.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PickedFolder {
    /// Names the folder in `add_project` and `init_project`.
    pub token: String,
    /// The folder under its placeholder root.
    pub path: String,
    /// The folder's own name (the default project name).
    pub name: String,
    /// `None`: no manifest; the page offers Init (Private preselected).
    pub manifest: Option<FoundManifest>,
}

/// What a project change did.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProjectChange {
    pub id: String,
    pub trust: ProjectTrust,
    #[serde(flatten)]
    pub change: ChangeResult,
}

/// How a manifest file compares with the content last trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ManifestChangeKind {
    New,
    Changed,
    Unchanged,
    Removed,
}

impl From<ManifestChange> for ManifestChangeKind {
    fn from(change: ManifestChange) -> Self {
        match change {
            ManifestChange::New => Self::New,
            ManifestChange::Changed => Self::Changed,
            ManifestChange::Unchanged => Self::Unchanged,
            ManifestChange::Removed => Self::Removed,
        }
    }
}

/// One manifest file of the trust review.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ReviewedFile {
    /// `project.toml` or `project.local.toml`.
    pub file: String,
    /// `<P>/.loadout/<file>`.
    pub path: String,
    pub change: ManifestChangeKind,
    /// The file now (masked); for a new file the whole review.
    pub content: Option<String>,
    /// Unified diff from the trusted content (masked), for a changed file.
    pub diff: Option<String>,
}

/// The trust review of a project: what trusting it would accept.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TrustReview {
    pub id: String,
    pub name: String,
    pub path: String,
    pub trust: ProjectTrust,
    /// The manifests' content hash; `trust_project` trusts only this
    /// content.
    pub hash: String,
    pub files: Vec<ReviewedFile>,
    /// Manifest validation warnings.
    pub warnings: Vec<IpcError>,
}

pub(super) fn trust_of(project: &KnownProject) -> ProjectTrust {
    match &project.status {
        ProjectStatus::Missing => ProjectTrust::Missing,
        ProjectStatus::Failed(_) => ProjectTrust::Invalid,
        ProjectStatus::Loaded { trust, .. } => match trust {
            TrustState::Trusted => ProjectTrust::Trusted,
            TrustState::Untrusted => ProjectTrust::Untrusted,
            TrustState::Changed => ProjectTrust::Changed,
        },
    }
}

/// The projects this machine knows, in id order.
fn known(core: &Core, store: Option<&Store<'_>>) -> Result<Vec<KnownProject>, CoreError> {
    let fs = core.fs.as_ref();
    let state = core.home.state_dir();
    let overlay = match store {
        Some(store) => projects::this_overlay(store, fs, &state)?,
        None => None,
    };
    let trust = TrustStore::load(fs, &state)?;
    Ok(known_projects(fs, &core.env, overlay.as_ref(), &trust))
}

/// The known project `id`, or `not_found`.
pub(super) fn find(
    core: &Core,
    store: Option<&Store<'_>>,
    id: &str,
) -> Result<KnownProject, CoreError> {
    known(core, store)?
        .into_iter()
        .find(|project| project.id == id)
        .ok_or_else(|| projects::unknown_project(id))
}

/// The loaded project of `known`, or why there is none.
pub(super) fn loaded(known: &KnownProject) -> Result<&Project, CoreError> {
    match &known.status {
        ProjectStatus::Loaded { project, .. } => Ok(project),
        ProjectStatus::Missing => Err(CoreError::new(
            ErrorCode::NotFound,
            "the project folder does not exist",
        )),
        ProjectStatus::Failed(err) => Err(err.clone()),
    }
}

/// The folder and name of known project `id` whose folder holds its
/// manifests: the only folders the matrix reads (SPEC 17, commands take
/// ids).
pub(super) fn validate_known_root(
    core: &Core,
    store: Option<&Store<'_>>,
    id: &str,
) -> CmdResult<(PathBuf, String)> {
    let known = find(core, store, id)?;
    let project = loaded(&known).map_err(|err| scrubbed(&core.env, &err, Some(&known.root)))?;
    Ok((known.root.clone(), project.name().to_owned()))
}

fn manifest_servers(project: &Project) -> usize {
    let mut names = BTreeSet::new();
    for file in [project.shared.as_ref(), project.local.as_ref()]
        .into_iter()
        .flatten()
    {
        names.extend(file.manifest.servers.iter().flatten().map(|(name, _)| name));
    }
    names.len()
}

/// The Projects page: each known project with its layout, trust, server
/// and client counts and pending entries (from one project plan).
pub fn list(core: &Core) -> CmdResult<Vec<ProjectRow>> {
    let store = core.open_store()?;
    let known = known(core, store.as_ref())?;
    if known.is_empty() {
        return Ok(Vec::new());
    }
    let options = PlanOptions {
        scope: ScopeFilter::Project,
        ..PlanOptions::default()
    };
    let plan = apply::preview(store.as_ref(), &core.apply_env(), &options)?;
    let mut pending: BTreeMap<&str, usize> = BTreeMap::new();
    let mut clients: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for target in &plan.targets {
        let Some(id) = target.project_id.as_deref() else {
            continue;
        };
        for entry in &target.entries {
            if entry.state.is_pending() || entry.unresolved || entry.state == EntryState::Blocked {
                *pending.entry(id).or_default() += 1;
            }
            if let Some(client) = entry.client
                && (entry.desired.is_some() || entry.state == EntryState::Blocked)
            {
                clients.entry(id).or_default().insert(client);
            }
        }
    }
    Ok(known
        .iter()
        .map(|project| {
            let fs = core.fs.as_ref();
            let root = Some(project.root.as_path());
            ProjectRow {
                id: project.id.clone(),
                name: project
                    .project()
                    .map_or_else(|| project.id.clone(), |p| p.name().to_owned()),
                path: placeholder_path(&core.env, &project.root, None),
                layout: projects::layout(fs, &project.root).map(Into::into),
                trust: trust_of(project),
                error: match &project.status {
                    ProjectStatus::Failed(err) => Some(scrubbed(&core.env, err, root)),
                    _ => None,
                },
                servers: count(project.project().map_or(0, manifest_servers)),
                clients: count(clients.get(project.id.as_str()).map_or(0, BTreeSet::len)),
                pending: count(pending.get(project.id.as_str()).copied().unwrap_or(0)),
            }
        })
        .collect())
}

fn invalid(folder: &Path, why: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidPath, why.to_owned()).with_path(folder)
}

/// Checks a folder the user picked (SPEC 17): absolute, without `.` or
/// `..` parts, an existing folder, not a drive or file-system root, not
/// the home folder (a manifest there would apply everywhere) and not
/// inside Loadout's data directory.
pub fn validate_folder(core: &Core, folder: &Path) -> Result<PathBuf, CoreError> {
    if !folder.is_absolute() {
        return Err(invalid(folder, "the folder is not an absolute path"));
    }
    if folder
        .components()
        .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(invalid(folder, "the folder path holds `.` or `..` parts"));
    }
    if !core.fs.is_dir(folder) {
        return Err(
            CoreError::new(ErrorCode::NotFound, "the folder does not exist").with_path(folder),
        );
    }
    // Compared with links resolved (a junction to the home folder is the
    // home folder); the overlay keeps the spelling the user picked.
    let fs = core.fs.as_ref();
    let real = resolved(fs, folder);
    if folder.parent().is_none() || real.parent().is_none() {
        return Err(invalid(
            folder,
            "a drive or file-system root cannot be a project; pick a project folder",
        ));
    }
    if same_folder(fs, folder, core.env.home()) {
        return Err(invalid(
            folder,
            "a manifest in the home folder would apply to every folder; pick a project folder",
        ));
    }
    let data = resolved(fs, &core.home.path);
    let inside_data = |path: &Path| {
        path.ancestors()
            .any(|ancestor| same_path(ancestor, &data) || same_path(ancestor, &core.home.path))
    };
    if inside_data(folder) || inside_data(&real) {
        return Err(invalid(
            folder,
            "the folder is inside Loadout's data directory",
        ));
    }
    Ok(folder.to_path_buf())
}

/// What a validated picked folder holds.
pub fn inspect(core: &Core, folder: &Path, token: String) -> CmdResult<PickedFolder> {
    let folder = validate_folder(core, folder).map_err(|err| scrubbed(&core.env, &err, None))?;
    let fs = core.fs.as_ref();
    let manifest = match projects::layout(fs, &folder) {
        None => None,
        Some(layout) => {
            let project = projects::load_project(fs, &folder)
                .map_err(|err| scrubbed(&core.env, &err, Some(&folder)))?;
            let store = core.open_store()?;
            let known = known(core, store.as_ref())?
                .iter()
                .any(|known| known.id == project.id && same_path(&known.root, &folder));
            Some(FoundManifest {
                id: project.id.clone(),
                name: project.name().to_owned(),
                layout: layout.into(),
                known,
            })
        }
    };
    Ok(PickedFolder {
        token,
        path: placeholder_path(&core.env, &folder, None),
        name: folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        manifest,
    })
}

fn trust_now(core: &Core, id: &str) -> Result<ProjectTrust, CoreError> {
    let store = core.open_store()?;
    Ok(trust_of(&find(core, store.as_ref(), id)?))
}

/// Add folder for a folder with a manifest: binds the project (and its id)
/// to this machine; it is applied once trusted.
pub fn add(core: &Core, folder: &Path) -> CmdResult<ProjectChange> {
    let folder = validate_folder(core, folder).map_err(|err| scrubbed(&core.env, &err, None))?;
    let scrub_err = |err: CoreError| scrubbed(&core.env, &err, Some(&folder));
    let project = projects::load_project(core.fs.as_ref(), &folder).map_err(scrub_err)?;
    let store = core.open_store_for_change()?;
    let commit = projects::add(&store, &core.apply_env(), &project).map_err(scrub_err)?;
    drop(store);
    Ok(ProjectChange {
        trust: trust_now(core, &project.id)?,
        id: project.id,
        change: ChangeResult::new(&core.env, commit.as_ref(), None),
    })
}

/// Init manifest: a private `project.local.toml` unless `shared`, trusted
/// and applied in one commit.
pub fn init(
    core: &Core,
    folder: &Path,
    shared: bool,
    name: Option<&str>,
) -> CmdResult<ProjectChange> {
    let folder = validate_folder(core, folder).map_err(|err| scrubbed(&core.env, &err, None))?;
    let scrub_err = |err: CoreError| scrubbed(&core.env, &err, Some(&folder));
    let plan =
        projects::prepare_init(&core.env, core.fs.as_ref(), &folder, name).map_err(scrub_err)?;
    let store = core.open_store_for_change()?;
    let done = projects::init(&store, &core.apply_env(), plan, shared).map_err(scrub_err)?;
    drop(store);
    Ok(ProjectChange {
        trust: trust_now(core, &done.id)?,
        change: ChangeResult::new(&core.env, done.commit.as_ref(), done.report.as_ref()),
        id: done.id,
    })
}

/// The trust review of project `id`: each manifest file against the
/// content last trusted, with the content hash `trust` accepts. Values
/// that may be secrets are masked; the folder is shown as `<P>`.
pub fn review(core: &Core, id: &str) -> CmdResult<TrustReview> {
    let store = core.open_store()?;
    let known = find(core, store.as_ref(), id)?;
    let root = known.root.clone();
    let scrub_err = |err: &CoreError| scrubbed(&core.env, err, Some(&root));
    let project = loaded(&known).map_err(|err| scrub_err(&err))?;
    let trust_store = TrustStore::load(core.fs.as_ref(), &core.home.state_dir())?;
    let reviews = match &store {
        Some(store) => {
            projects::review(store, project, &trust_store).map_err(|err| scrub_err(&err))?
        }
        None => Vec::new(),
    };
    let files = reviews
        .into_iter()
        .map(|review| {
            let path = placeholder_path(&core.env, &review.path, Some(&root));
            let native = review.path.display().to_string();
            ReviewedFile {
                file: review.file.to_owned(),
                content: review.content.as_deref().map(super::apply::mask_diff),
                diff: review.diff.as_deref().map(|diff| {
                    super::apply::mask_diff(&scrub(
                        &core.env,
                        &diff.replace(&native, &path),
                        Some(&root),
                    ))
                }),
                path,
                change: review.change.into(),
            }
        })
        .collect();
    Ok(TrustReview {
        id: project.id.clone(),
        name: project.name().to_owned(),
        path: placeholder_path(&core.env, &root, None),
        trust: trust_of(&known),
        hash: project.hash(),
        files,
        warnings: project.warnings.iter().map(scrub_err).collect(),
    })
}

/// Trusts project `id` as reviewed: only when its manifests still hash to
/// `hash`, then records, mirrors and applies it in one commit.
pub fn trust(core: &Core, id: &str, hash: &str) -> CmdResult<ProjectChange> {
    let store = core.open_store_for_change()?;
    let known = find(core, Some(&store), id)?;
    let root = known.root.clone();
    let scrub_err = |err: CoreError| scrubbed(&core.env, &err, Some(&root));
    let project = loaded(&known).map_err(scrub_err)?;
    let (report, commit) =
        projects::trust(&store, &core.apply_env(), project, Some(hash)).map_err(scrub_err)?;
    drop(store);
    Ok(ProjectChange {
        id: id.to_owned(),
        trust: trust_now(core, id)?,
        change: ChangeResult::new(&core.env, commit.as_ref(), report.as_ref()),
    })
}

/// Remove from list: forgets the project on this machine; the apply in
/// the same commit removes the entries Loadout owns in its files.
pub fn remove(core: &Core, id: &str) -> CmdResult<ChangeResult> {
    let store = core
        .open_store()?
        .ok_or_else(|| projects::unknown_project(id))?;
    let removed = projects::remove(&store, &core.apply_env(), id)
        .map_err(|err| scrubbed(&core.env, &err, None))?;
    Ok(ChangeResult::new(
        &core.env,
        removed.commit.as_ref(),
        removed.report.as_ref(),
    ))
}

/// Apply: the project's files only (`loadout apply <P>`), one commit when
/// a file is written.
pub fn apply_project(core: &Core, id: &str) -> CmdResult<ChangeResult> {
    let store = core.open_store_for_change()?;
    let known = find(core, Some(&store), id)?;
    let request = ApplyRequest {
        options: PlanOptions {
            scope: ScopeFilter::Project,
            projects: ProjectSelection::Only(known.root.clone()),
            ..PlanOptions::default()
        },
        trigger: ApplyTrigger::Review,
    };
    let (report, commit) = apply::run(&store, &core.apply_env(), &request)
        .map_err(|err| scrubbed(&core.env, &err, Some(&known.root)))?;
    Ok(ChangeResult::new(&core.env, commit.as_ref(), Some(&report)))
}

#[tauri::command]
#[specta::specta]
pub async fn list_projects(state: tauri::State<'_, AppState>) -> CmdResult<Vec<ProjectRow>> {
    blocking(&state, list).await
}

/// Opens the OS folder picker; `None` when the user cancels. The folder is
/// validated and kept here; the frontend gets a token and the placeholder
/// path.
#[tauri::command]
#[specta::specta]
pub async fn pick_project_folder(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> CmdResult<Option<PickedFolder>> {
    use tauri_plugin_dialog::DialogExt;
    let picked =
        tauri::async_runtime::spawn_blocking(move || app.dialog().file().blocking_pick_folder())
            .await
            .map_err(|err| IpcError {
                code: ErrorCode::IoError,
                message: format!("the folder picker failed: {err}"),
            })?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let folder = picked.into_path().map_err(|_| IpcError {
        code: ErrorCode::InvalidPath,
        message: "the picked item is not a local folder".to_owned(),
    })?;
    let token = state.remember_folder(folder.clone());
    blocking(&state, move |core| inspect(core, &folder, token).map(Some)).await
}

#[tauri::command]
#[specta::specta]
pub async fn add_project(
    state: tauri::State<'_, AppState>,
    token: String,
) -> CmdResult<ProjectChange> {
    let folder = state.picked_folder(&token)?;
    blocking(&state, move |core| add(core, &folder)).await
}

#[tauri::command]
#[specta::specta]
pub async fn init_project(
    state: tauri::State<'_, AppState>,
    token: String,
    shared: bool,
    name: Option<String>,
) -> CmdResult<ProjectChange> {
    let folder = state.picked_folder(&token)?;
    blocking(&state, move |core| {
        init(core, &folder, shared, name.as_deref())
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn review_project(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CmdResult<TrustReview> {
    blocking(&state, move |core| review(core, &id)).await
}

#[tauri::command]
#[specta::specta]
pub async fn trust_project(
    state: tauri::State<'_, AppState>,
    id: String,
    hash: String,
) -> CmdResult<ProjectChange> {
    blocking(&state, move |core| trust(core, &id, &hash)).await
}

#[tauri::command]
#[specta::specta]
pub async fn remove_project(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| remove(core, &id)).await
}

/// Open: the known project's folder in the file manager. The folder comes
/// from this machine's overlay, never from the frontend.
#[tauri::command]
#[specta::specta]
pub async fn reveal_project_folder(state: tauri::State<'_, AppState>, id: String) -> CmdResult<()> {
    blocking(&state, move |core| {
        let store = core.open_store()?;
        let (root, _) = validate_known_root(core, store.as_ref(), &id)?;
        super::tools::open_folder(&root).map_err(|err| scrubbed(&core.env, &err, Some(&root)))
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn apply_project_files(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| apply_project(core, &id)).await
}

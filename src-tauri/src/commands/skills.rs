//! Skills and Install page commands (SPEC 12, 13.7, 17): the store's skills
//! with their clients, links and sources, per scope (user, or a known
//! project); client skill folders classified as managed, unmanaged or
//! mismatch; installs from a git URL or a picked folder or archive (the
//! path stays in the backend under a token); enable, disable, tags,
//! updates with a per-file diff, removal; import, bulk adopt (unmanaged
//! folders into the store, replaced by links), trash and agents CLI
//! vendoring. Every operation is the core's (`skills::*`), as
//! `loadout skills` runs it: one store commit per change, then client
//! folders follow at once. Commands take ids (skill, client, project),
//! never a path; paths go out under their placeholder roots.

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, PoisonError};

use agent_loadout_core::adapters::adapter;
use agent_loadout_core::apply::{ScopeFilter, TargetPlan, placeholder_path};
use agent_loadout_core::layering::{Project, resolved, same_path};
use agent_loadout_core::model::applied::Scope as TargetScope;
use agent_loadout_core::model::clients::ClientSelection;
use agent_loadout_core::model::skill_meta::{DistributionMethod, SkillSourceKind};
use agent_loadout_core::remote::git::{IDLE_TIMEOUT, SystemRunner};
use agent_loadout_core::skills::adopt;
use agent_loadout_core::skills::agents_cli::{self, CliAction};
use agent_loadout_core::skills::classify::{self, EntryClass};
use agent_loadout_core::skills::hash_cache::FolderHashCache;
use agent_loadout_core::skills::install::{
    self, ChangeKind, Decisions, InstallRequest, Prepared, Source, UpdateStatus,
};
use agent_loadout_core::skills::link::{
    self, CLIENT_SKILLS, DetectedClients, LinkMethod, LinkStatus, ProjectSkills, SkillLedger,
};
use agent_loadout_core::skills::{
    self, Installed, LinkKind, Naming, SkillFiles, SkillsEnv, StoreSkill, SyncReport, store,
};
use agent_loadout_core::sources::SourceRef;
use agent_loadout_core::store::{CommitId, Store};
use agent_loadout_core::{CoreError, Env, ErrorCode};
use serde::{Deserialize, Serialize};
use specta::Type;

use super::projects::{self, ProjectTrust};
use super::remotes::mask_url;
use super::scan::ScanResult;
use super::sources::{self, SourceTrustInfo, TrustGrantArg};
use super::{
    AppState, CmdResult, Core, FileScope, IpcError, NO_REGISTRY, ReadOnly, SharedReview, blocking,
    count, read_only_of, scrubbed,
};

static RUNNER: SystemRunner = SystemRunner {
    idle_timeout: IDLE_TIMEOUT,
};

/// Largest file the file browser shows as text.
const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;

/// What skill operations run against: the app's environment, junctions on
/// Windows (no Developer Mode or admin needed, SPEC 12.2).
pub(super) fn sx(core: &Core) -> SkillsEnv<'_> {
    SkillsEnv {
        env: &core.env,
        fs: core.fs.as_ref(),
        home: &core.home,
        registry: &NO_REGISTRY,
        runner: &RUNNER,
        link_kind: LinkKind::Junction,
    }
}

/// The scope the Skills page shows (SPEC 13.7: project scope shows the
/// project-enabled skills).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillScope {
    User,
    Project { id: String },
}

/// Where a store skill came from (SPEC 12.1 `[source] kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SkillOrigin {
    Git,
    Local,
    AgentsCli,
    Catalog,
    Authored,
    /// Shipped with Loadout (the Loadout skill for agents).
    Bundled,
}

impl From<SkillSourceKind> for SkillOrigin {
    fn from(kind: SkillSourceKind) -> Self {
        match kind {
            SkillSourceKind::Git => Self::Git,
            SkillSourceKind::Local => Self::Local,
            SkillSourceKind::AgentsCli => Self::AgentsCli,
            SkillSourceKind::Catalog => Self::Catalog,
            SkillSourceKind::Authored => Self::Authored,
        }
    }
}

/// How a skill reaches client folders at user scope (SPEC 12.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SkillMethod {
    Link,
    Copy,
}

impl From<SkillMethod> for DistributionMethod {
    fn from(method: SkillMethod) -> Self {
        match method {
            SkillMethod::Link => Self::Link,
            SkillMethod::Copy => Self::Copy,
        }
    }
}

/// How one owned entry was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SkillLinkMethod {
    Junction,
    Symlink,
    Copy,
}

impl From<LinkMethod> for SkillLinkMethod {
    fn from(method: LinkMethod) -> Self {
        match method {
            LinkMethod::Junction => Self::Junction,
            LinkMethod::Symlink => Self::Symlink,
            LinkMethod::Copy => Self::Copy,
        }
    }
}

/// A skill's source, URL userinfo masked and local paths under their
/// placeholder roots.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillSourceInfo {
    pub kind: SkillOrigin,
    pub url: Option<String>,
    /// The skill folder inside the repository.
    pub path: Option<String>,
    pub git_ref: Option<String>,
    pub commit: Option<String>,
}

/// A `SKILL.md` frontmatter field (SPEC 12.1: all fields shown): strings as
/// written, other values as JSON.
#[derive(Debug, Clone, Serialize, Type)]
pub struct FrontmatterField {
    pub key: String,
    pub value: String,
}

/// An entry Loadout owns in a client skill folder.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillLinkInfo {
    pub client: String,
    pub scope: FileScope,
    pub project_id: Option<String>,
    pub path: String,
    pub method: SkillLinkMethod,
}

/// One store skill on the Skills page.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub fields: Vec<FrontmatterField>,
    pub tags: Vec<String>,
    pub source: Option<SkillSourceInfo>,
    /// User scope: `enable.clients` is `all-detected`.
    pub all_detected: bool,
    /// User scope: the clients the skill is enabled for (`all-detected`
    /// resolved).
    pub clients: Vec<String>,
    pub method: SkillMethod,
    /// Project scope: listed in the project's `[skills] enable`.
    pub project_enabled: bool,
    /// Project scope: enabled by the shared `project.toml` of a project
    /// that has a private file too, so it cannot be turned off here.
    pub project_shared: bool,
    pub links: Vec<SkillLinkInfo>,
    /// Size of the content in bytes.
    pub bytes: u32,
    /// Only skills with content are distributed.
    pub has_content: bool,
    pub problems: Vec<IpcError>,
}

/// A client with a skill folder (SPEC 7.6).
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillClient {
    pub id: String,
    pub name: String,
    pub detected: bool,
    /// This account cannot write the client's skill folder at the page's
    /// scope (or create it; SPEC 8.2): links and copies there are
    /// `read_only_target`.
    pub read_only: Option<ReadOnly>,
}

/// The project the Skills page is scoped to.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillProject {
    pub id: String,
    pub name: String,
    pub path: String,
    pub trust: ProjectTrust,
    /// Content hash of the manifests as shown; project enables send it
    /// back, and other content is `trust_required` (SPEC 17).
    pub hash: String,
    /// The project's `defaults.clients`, resolved.
    pub clients: Vec<String>,
    /// `[skills] share`: copies to commit instead of hidden links.
    pub share: bool,
    /// The whole project is read only for this account (SPEC 8.2), as
    /// `ProjectRow::read_only`.
    pub read_only: Option<ReadOnly>,
}

/// A skill of the agents CLI (SPEC 12.4), read only.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentsCliSkill {
    pub name: String,
    pub path: String,
    pub is_link: bool,
    pub source: Option<String>,
    pub source_url: Option<String>,
    pub description: Option<String>,
}

/// `~/.agents/skills` when the compatibility setting is on.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AgentsCliList {
    pub dir: String,
    pub skills: Vec<AgentsCliSkill>,
    /// The `skills` CLI is on `PATH` (update and remove need it).
    pub cli_found: bool,
    pub warnings: Vec<IpcError>,
}

/// The Skills page.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillList {
    pub skills: Vec<SkillInfo>,
    pub clients: Vec<SkillClient>,
    pub project: Option<SkillProject>,
    pub agents_cli: Option<AgentsCliList>,
    /// The store's skills pass 500 MB together.
    pub size_warning: Option<IpcError>,
}

/// Class of a client skill folder entry (SPEC 12.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SkillEntryClass {
    Managed,
    Unmanaged,
    Mismatch,
}

impl From<EntryClass> for SkillEntryClass {
    fn from(class: EntryClass) -> Self {
        match class {
            EntryClass::Managed => Self::Managed,
            EntryClass::Unmanaged => Self::Unmanaged,
            EntryClass::Mismatch => Self::Mismatch,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ToolSkillEntry {
    pub name: String,
    pub path: String,
    pub class: SkillEntryClass,
    pub link_target: Option<String>,
    /// The store skill of a managed entry.
    pub skill_id: Option<String>,
    pub description: Option<String>,
}

/// One client's skill folder in the page's scope.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ToolSkillDir {
    pub client: String,
    pub name: String,
    pub dir: String,
    pub exists: bool,
    pub entries: Vec<ToolSkillEntry>,
    /// This account cannot write the folder, or create it (SPEC 8.2).
    pub read_only: Option<ReadOnly>,
}

/// What happened to one client folder entry (SPEC 12.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SkillLinkStatus {
    Created,
    Adopted,
    Unchanged,
    Updated,
    Removed,
    Foreign,
    Collision,
    Changed,
    Blocked,
}

impl From<LinkStatus> for SkillLinkStatus {
    fn from(status: LinkStatus) -> Self {
        match status {
            LinkStatus::Created => Self::Created,
            LinkStatus::Adopted => Self::Adopted,
            LinkStatus::Unchanged => Self::Unchanged,
            LinkStatus::Updated => Self::Updated,
            LinkStatus::Removed => Self::Removed,
            LinkStatus::Foreign => Self::Foreign,
            LinkStatus::Collision => Self::Collision,
            LinkStatus::Changed => Self::Changed,
            LinkStatus::Blocked => Self::Blocked,
        }
    }
}

/// An entry a change made, removed, or left for the user.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillLinkNote {
    pub client: String,
    pub scope: FileScope,
    pub project_id: Option<String>,
    pub skill_id: String,
    pub path: String,
    pub method: SkillLinkMethod,
    pub status: SkillLinkStatus,
    /// Where a colliding link points.
    pub found_target: Option<String>,
    pub error: Option<IpcError>,
    /// The entry was not made or removed: this account cannot write there
    /// (`status` `blocked`, `error.code` `read_only_target`).
    pub read_only: Option<ReadOnly>,
}

/// What client folders did after a change.
#[derive(Debug, Clone, Default, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillSync {
    /// Entries other than `unchanged`.
    pub notes: Vec<SkillLinkNote>,
    pub warnings: Vec<IpcError>,
    /// An entry needs a decision (foreign, collision, changed) or failed.
    pub needs_attention: bool,
}

/// A store change and the client folders that followed.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SkillChange {
    pub commit: Option<String>,
    pub links: SkillSync,
}

/// An install (or import, or vendoring) into the store.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillInstalled {
    pub id: String,
    pub name: String,
    pub commit: Option<String>,
    pub source_commit: Option<String>,
    /// The id the name asked for, when it was taken (confirmed).
    pub renamed_from: Option<String>,
    pub links: Option<SkillSync>,
    pub warnings: Vec<IpcError>,
    /// The built-in scan of what was installed.
    pub scan: ScanResult,
}

/// Where reviewed content comes from. A local source shows no path (it
/// stays in the backend under the pick token).
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillReviewSource {
    pub kind: SkillOrigin,
    /// Git URL, userinfo masked.
    pub url: Option<String>,
    /// The skill folder inside the repository.
    pub path: Option<String>,
    pub git_ref: Option<String>,
    /// The commit that would be installed.
    pub commit: Option<String>,
}

/// Fetched and scanned content before install (SPEC 12.4, 17): what the
/// install review shows. Nothing is in the store yet.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillInstallReview {
    /// Names this content in `install_reviewed_skill` and
    /// `discard_skill_review`.
    pub token: String,
    pub name: String,
    pub files: Vec<SkillFileEntry>,
    pub source: SkillReviewSource,
    pub scan: ScanResult,
    /// The trust dialog's view of a git source (#24); `null` for a local
    /// folder or archive. An untrusted source needs the dialog's answer in
    /// `install_reviewed_skill`.
    pub trust: Option<SourceTrustInfo>,
    /// The trust dialog's view of the repository the host redirected the
    /// URL to, when it moved to another repository; `null` otherwise. It
    /// needs its own answer (`movedTrust` in `install_reviewed_skill`)
    /// unless it is trusted: `trust`'s answer never carries over.
    pub moved_trust: Option<SourceTrustInfo>,
}

/// What the local install picks in the OS dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SkillSourcePick {
    Folder,
    /// A `.zip` or `.skill` file.
    Archive,
}

/// A picked folder or archive, as the frontend sees it.
#[derive(Debug, Clone, Serialize, Type)]
pub struct PickedSkillSource {
    /// Names the source in `install_local_skill`.
    pub token: String,
    /// Under its placeholder root.
    pub path: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SkillUpdateStatus {
    UpToDate,
    Available,
    NotTracked,
    SourceUnreachable,
    /// A block covers the source; it was not contacted (see the error).
    SourceBlocked,
    /// The check itself failed for this skill (see the error); other
    /// skills are still checked.
    Failed,
}

impl From<UpdateStatus> for SkillUpdateStatus {
    fn from(status: UpdateStatus) -> Self {
        match status {
            UpdateStatus::UpToDate => Self::UpToDate,
            UpdateStatus::Available => Self::Available,
            UpdateStatus::NotTracked => Self::NotTracked,
            UpdateStatus::Unreachable => Self::SourceUnreachable,
            UpdateStatus::Blocked => Self::SourceBlocked,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SkillFileChangeKind {
    Added,
    Modified,
    Removed,
}

impl From<ChangeKind> for SkillFileChangeKind {
    fn from(kind: ChangeKind) -> Self {
        match kind {
            ChangeKind::Added => Self::Added,
            ChangeKind::Modified => Self::Modified,
            ChangeKind::Removed => Self::Removed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct SkillFileChange {
    pub path: String,
    pub kind: SkillFileChangeKind,
    /// An added or modified file that looks executable (#24): highlight it.
    pub script: bool,
}

/// An update check (SPEC 12.4).
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillUpdateCheck {
    pub id: String,
    pub status: SkillUpdateStatus,
    pub current: Option<String>,
    pub latest: Option<String>,
    pub changes: Vec<SkillFileChange>,
    pub error: Option<IpcError>,
    /// The built-in scan of the new content, when an update is available.
    pub scan: Option<ScanResult>,
    /// The trust check of a git skill's source (#24), with what changed
    /// about it (force-push, moved); `null` for other skills. An untrusted
    /// source needs the dialog's answer in `update_skill`.
    pub trust: Option<SourceTrustInfo>,
    /// The trust check of the repository the source moved to, when the
    /// host redirected it to another repository; `null` otherwise. It
    /// needs its own answer (`movedTrust` in `update_skill`) unless it is
    /// trusted; the update then moves the skill's URL there.
    pub moved_trust: Option<SourceTrustInfo>,
}

/// One file of an update preview: the store copy against the source as
/// the last check fetched it. `None` where the file does not exist.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SkillFileDiff {
    pub path: String,
    pub before: Option<String>,
    pub after: Option<String>,
    /// Either side is not UTF-8 text (or too large to show).
    pub binary: bool,
}

/// What an update did.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SkillUpdated {
    pub id: String,
    pub from: Option<String>,
    pub to: String,
    pub commit: Option<String>,
    pub changes: Vec<SkillFileChange>,
    pub links: SkillSync,
    /// The built-in scan of the new content.
    pub scan: ScanResult,
}

/// A file of a store skill.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SkillFileEntry {
    pub path: String,
    pub bytes: u32,
}

/// A file's text for the file browser.
#[derive(Debug, Clone, Serialize, Type)]
pub struct SkillFileText {
    pub path: String,
    /// `None` for a binary or very large file.
    pub text: Option<String>,
    pub bytes: u32,
}

/// The agents `skills` CLI actions (SPEC 12.4: only through the CLI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AgentsCliAction {
    Update,
    Remove,
}

fn size(n: u64) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn commit_text(commit: Option<&CommitId>) -> Option<String> {
    commit.map(CommitId::to_string)
}

fn client_name(id: &str) -> String {
    adapter(id).map_or_else(|| id.to_owned(), |spec| spec.display_name.to_owned())
}

fn sync_of(env: &Env, report: &SyncReport) -> SkillSync {
    let path = |path: &Path| placeholder_path(env, path, None);
    SkillSync {
        notes: report
            .notable()
            .map(|result| SkillLinkNote {
                client: result.client.clone(),
                scope: result.scope.into(),
                project_id: result.project_id.clone(),
                skill_id: result.skill_id.clone(),
                path: path(&result.path),
                method: result.method.into(),
                status: result.status.into(),
                found_target: result.found_target.as_deref().map(path),
                error: result.error.as_ref().map(|err| scrubbed(env, err, None)),
                read_only: ReadOnly::of(result.error.as_ref().and_then(CoreError::read_only)),
            })
            .collect(),
        warnings: report
            .warnings
            .iter()
            .map(|err| scrubbed(env, err, None))
            .collect(),
        needs_attention: report.needs_attention(),
    }
}

pub(super) fn change_of(env: &Env, commit: Option<&CommitId>, report: &SyncReport) -> SkillChange {
    SkillChange {
        commit: commit_text(commit),
        links: sync_of(env, report),
    }
}

fn installed_of(env: &Env, installed: &Installed) -> SkillInstalled {
    SkillInstalled {
        id: installed.id.clone(),
        name: installed.name.clone(),
        commit: commit_text(installed.commit.as_ref()),
        source_commit: installed.source_commit.clone(),
        renamed_from: installed.renamed_from.clone(),
        links: installed.report.as_ref().map(|report| sync_of(env, report)),
        warnings: installed
            .warnings
            .iter()
            .map(|err| scrubbed(env, err, None))
            .collect(),
        scan: ScanResult::from(&installed.scan),
    }
}

fn review_of(env: &Env, prepared: &Prepared) -> SkillInstallReview {
    let source = prepared.source();
    SkillInstallReview {
        token: String::new(),
        name: prepared.name().to_owned(),
        files: prepared
            .files()
            .iter()
            .map(|(path, bytes)| SkillFileEntry {
                path: path.clone(),
                bytes: count(bytes.len()),
            })
            .collect(),
        source: SkillReviewSource {
            kind: source.kind.into(),
            url: source.url.as_deref().map(|url| source_url(env, url)),
            path: source.path.clone(),
            git_ref: source.git_ref.clone(),
            commit: source.commit.clone(),
        },
        scan: ScanResult::from(prepared.scan()),
        trust: None,
        moved_trust: None,
    }
}

/// A URL with userinfo masked and a local path under its placeholder root.
fn source_url(env: &Env, url: &str) -> String {
    super::scrub(env, &mask_url(url), None)
}

fn field_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The project scope of the page: the known project and its `[skills]`.
struct ProjectScope {
    project: Project,
    wanted: ProjectSkills,
    info: SkillProject,
}

fn project_scope(
    core: &Core,
    store: Option<&Store<'_>>,
    detected: &DetectedClients<'_>,
    id: &str,
) -> CmdResult<ProjectScope> {
    let known = projects::find(core, store, id)?;
    let root = known.root.clone();
    let project = projects::loaded(&known)
        .map_err(|err| scrubbed(&core.env, &err, Some(&root)))?
        .clone();
    let wanted = link::project_skills(&project);
    let cached = core
        .plan()
        .map_err(|err| scrubbed(&core.env, &err, Some(&root)))?;
    let plan = cached.plan.scope_part(ScopeFilter::Project);
    let targets: Vec<&TargetPlan> = plan.targets.iter().collect();
    let access = projects::project_access(core, &[(id, &root)], &targets)
        .remove(id)
        .unwrap_or_default();
    let info = SkillProject {
        id: project.id.clone(),
        name: project.name().to_owned(),
        path: placeholder_path(&core.env, &root, None),
        trust: projects::trust_of(&known),
        hash: project.hash(),
        clients: detected
            .selected(&wanted.clients)
            .into_iter()
            .map(str::to_owned)
            .collect(),
        share: wanted.share,
        read_only: access.read_only,
    };
    Ok(ProjectScope {
        project,
        wanted,
        info,
    })
}

fn skill_info(
    core: &Core,
    detected: &DetectedClients<'_>,
    skill: &StoreSkill,
    ledger: &SkillLedger,
    project: Option<&ProjectScope>,
) -> SkillInfo {
    let env = &core.env;
    let meta = skill.meta.as_ref();
    let enable = meta.and_then(|meta| meta.enable.as_ref());
    let selection = enable.and_then(|enable| enable.clients.as_ref());
    let (project_enabled, project_shared) = match project {
        Some(scope) => {
            let shared = scope.project.local.is_some()
                && scope
                    .project
                    .shared
                    .as_ref()
                    .is_some_and(|file| link::manifest_enables(&file.manifest, &skill.id));
            (scope.wanted.enable.contains(&skill.id), shared)
        }
        None => (false, false),
    };
    SkillInfo {
        id: skill.id.clone(),
        name: skill.name().to_owned(),
        description: skill.description().map(str::to_owned),
        fields: skill
            .frontmatter
            .as_ref()
            .map(|frontmatter| {
                frontmatter
                    .fields
                    .iter()
                    .map(|(key, value)| FrontmatterField {
                        key: key.clone(),
                        value: field_text(&skills::frontmatter::to_json(value)),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        tags: meta.and_then(|meta| meta.tags.clone()).unwrap_or_default(),
        source: match meta {
            // The Loadout skill is marked by `[bundled]` alone (no `[source]`).
            Some(meta) if agent_loadout_core::agent::is_bundled(meta) => Some(SkillSourceInfo {
                kind: SkillOrigin::Bundled,
                url: None,
                path: None,
                git_ref: None,
                commit: None,
            }),
            _ => meta
                .and_then(|meta| meta.source.as_ref())
                .map(|source| SkillSourceInfo {
                    kind: source.kind.into(),
                    url: source.url.as_deref().map(|url| source_url(env, url)),
                    path: source.path.clone(),
                    git_ref: source.git_ref.clone(),
                    commit: source.commit.clone(),
                }),
        },
        all_detected: matches!(selection, Some(ClientSelection::AllDetected)),
        clients: selection
            .map(|selection| {
                detected
                    .selected(selection)
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        method: match enable.and_then(|enable| enable.method) {
            Some(DistributionMethod::Copy) => SkillMethod::Copy,
            _ => SkillMethod::Link,
        },
        project_enabled,
        project_shared,
        links: link::owned_entries(ledger, &skill.id)
            .map(|entry| SkillLinkInfo {
                client: entry.client.clone(),
                scope: entry.scope.into(),
                project_id: entry.project_id.clone(),
                path: placeholder_path(env, Path::new(&entry.path), None),
                method: entry.method.into(),
            })
            .collect(),
        bytes: size(skill.bytes),
        has_content: skill.has_content,
        problems: skill
            .problems
            .iter()
            .map(|err| scrubbed(env, err, None))
            .collect(),
    }
}

/// The Skills page for `scope`.
pub fn list(core: &Core, scope: &SkillScope) -> CmdResult<SkillList> {
    let sx = sx(core);
    let env = &core.env;
    let detected = DetectedClients::new(&sx);
    let store = core.open_store()?;
    let project = match scope {
        SkillScope::User => None,
        SkillScope::Project { id } => Some(project_scope(core, store.as_ref(), &detected, id)?),
    };
    let skills = match &store {
        Some(store) => store::list(store)?,
        None => Vec::new(),
    };
    let ledger = SkillLedger::load(sx.fs, &sx.home.state_dir())?;
    let (dir_scope, dir_root) = match &project {
        Some(scope) => (TargetScope::Project, Some(scope.project.root.clone())),
        None => (TargetScope::User, None),
    };
    let dirs: Vec<Option<PathBuf>> = CLIENT_SKILLS
        .iter()
        .map(|entry| link::skills_dir(env, entry.client, dir_scope, dir_root.as_deref()).ok())
        .collect();
    let probed: Vec<PathBuf> = dirs.iter().flatten().cloned().collect();
    let access = read_only_of(core, &probed);
    let clients = CLIENT_SKILLS
        .iter()
        .zip(&dirs)
        .map(|(entry, dir)| SkillClient {
            id: entry.client.to_owned(),
            name: client_name(entry.client),
            detected: detected.is_detected(entry.client),
            read_only: ReadOnly::of(
                dir.as_ref()
                    .and_then(|dir| access.get(dir).copied().flatten()),
            ),
        })
        .collect();
    let agents_cli = if store
        .as_ref()
        .is_none_or(|store| agents_cli::enabled(store))
    {
        let scan = agents_cli::scan(&sx).map_err(|err| scrubbed(env, &err, None))?;
        Some(AgentsCliList {
            dir: placeholder_path(env, &agents_cli::skills_dir(&sx), None),
            skills: scan
                .skills
                .iter()
                .map(|skill| AgentsCliSkill {
                    name: skill.name.clone(),
                    path: placeholder_path(env, &skill.path, None),
                    is_link: skill.is_link,
                    source: skill.source.clone(),
                    source_url: skill.source_url.as_deref().map(|url| source_url(env, url)),
                    description: skill.description.clone(),
                })
                .collect(),
            cli_found: agents_cli::locate_cli(&sx).is_some(),
            warnings: scan
                .warnings
                .iter()
                .map(|err| scrubbed(env, err, None))
                .collect(),
        })
    } else {
        None
    };
    Ok(SkillList {
        skills: skills
            .iter()
            .map(|skill| skill_info(core, &detected, skill, &ledger, project.as_ref()))
            .collect(),
        clients,
        size_warning: store::size_warning(&skills).map(|err| scrubbed(env, &err, None)),
        project: project.map(|scope| scope.info),
        agents_cli,
    })
}

/// The core scope and project folder of `scope`; a project must be known
/// and its folder hold its manifests.
fn target_scope(core: &Core, scope: &SkillScope) -> CmdResult<(TargetScope, Option<PathBuf>)> {
    match scope {
        SkillScope::User => Ok((TargetScope::User, None)),
        SkillScope::Project { id } => {
            let store = core.open_store()?;
            let (root, _) = projects::validate_known_root(core, store.as_ref(), id)?;
            Ok((TargetScope::Project, Some(root)))
        }
    }
}

/// Every client's skill folder in `scope`, classified (SPEC 12.5).
pub fn tool_dirs(core: &Core, scope: &SkillScope) -> CmdResult<Vec<ToolSkillDir>> {
    let (scope, root) = target_scope(core, scope)?;
    let env = &core.env;
    let root = root.as_deref();
    let dirs = classify::scan(&sx(core), scope, root).map_err(|err| scrubbed(env, &err, root))?;
    let path = |path: &Path| placeholder_path(env, path, root);
    let probed: Vec<PathBuf> = dirs.iter().map(|dir| dir.dir.clone()).collect();
    let access = read_only_of(core, &probed);
    Ok(dirs
        .iter()
        .map(|dir| ToolSkillDir {
            client: dir.client.to_owned(),
            name: client_name(dir.client),
            dir: path(&dir.dir),
            exists: dir.exists,
            entries: dir
                .entries
                .iter()
                .map(|entry| ToolSkillEntry {
                    name: entry.name.clone(),
                    path: path(&entry.path),
                    class: entry.class.into(),
                    link_target: entry.link_target.as_deref().map(path),
                    skill_id: entry.skill_id.clone(),
                    description: entry.description.clone(),
                })
                .collect(),
            read_only: ReadOnly::of(access.get(&dir.dir).copied().flatten()),
        })
        .collect())
}

fn invalid_source(path: &Path, why: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidPath, why.to_owned()).with_path(path)
}

/// Checks a folder or archive the user picked (SPEC 17): absolute, without
/// `.` or `..` parts, an existing folder or a `.zip`/`.skill` file, not
/// inside Loadout's data directory (the store, caches and backups). The
/// install itself refuses zip-slip, escaping links and oversized content.
pub fn validate_source(core: &Core, path: &Path) -> Result<PathBuf, CoreError> {
    if !path.is_absolute() {
        return Err(invalid_source(path, "the path is not absolute"));
    }
    if path
        .components()
        .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(invalid_source(path, "the path holds `.` or `..` parts"));
    }
    let fs = core.fs.as_ref();
    if !fs.exists(path) {
        return Err(CoreError::new(ErrorCode::NotFound, "the path does not exist").with_path(path));
    }
    if !fs.is_dir(path) {
        let archive = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                ext.eq_ignore_ascii_case("zip") || ext.eq_ignore_ascii_case("skill")
            });
        if !archive {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                "only a folder, a .zip or a .skill file can be installed",
            )
            .with_path(path));
        }
    }
    let data = resolved(fs, &core.home.path);
    let real = resolved(fs, path);
    let inside_data = |path: &Path| {
        path.ancestors()
            .any(|ancestor| same_path(ancestor, &data) || same_path(ancestor, &core.home.path))
    };
    if inside_data(path) || inside_data(&real) {
        return Err(invalid_source(
            path,
            "the path is inside Loadout's data directory",
        ));
    }
    Ok(path.to_path_buf())
}

/// The validated picked source, its placeholder path and its name.
pub fn inspect_source(core: &Core, path: &Path) -> CmdResult<(PathBuf, String, String)> {
    let path = validate_source(core, path).map_err(|err| scrubbed(&core.env, &err, None))?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let shown = placeholder_path(&core.env, &path, None);
    Ok((path, shown, name))
}

fn naming(id: Option<String>, rename_if_taken: bool) -> Naming {
    Naming {
        id: id
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty()),
        rename_if_taken,
    }
}

fn install_with(core: &Core, request: &InstallRequest) -> CmdResult<SkillInstalled> {
    let env = &core.env;
    let store = core.open_store_for_change()?;
    let installed =
        install::install(&store, &sx(core), request).map_err(|err| scrubbed(env, &err, None))?;
    Ok(installed_of(env, &installed))
}

/// Installs the picked folder or archive (SPEC 12.4 local install).
pub fn install_local(
    core: &Core,
    path: &Path,
    id: Option<String>,
    rename_if_taken: bool,
) -> CmdResult<SkillInstalled> {
    let mut request = local_request(core, path)?;
    request.naming = naming(id, rename_if_taken);
    install_with(core, &request)
}

fn local_request(core: &Core, path: &Path) -> CmdResult<InstallRequest> {
    let path = validate_source(core, path).map_err(|err| scrubbed(&core.env, &err, None))?;
    Ok(InstallRequest::new(Source::Local(path)))
}

fn trimmed(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Installs from a git URL (SPEC 12.4 git install): `https://`, `ssh://`,
/// `git://`, `file://` or `user@host:path`; a GitHub `/tree/<ref>/<path>`
/// URL carries its ref and folder. A local path is refused: commands take
/// no paths (a local folder is picked in the OS dialog instead).
pub fn install_git(
    core: &Core,
    url: &str,
    path: Option<String>,
    git_ref: Option<String>,
    id: Option<String>,
    rename_if_taken: bool,
    trust: Option<TrustGrantArg>,
) -> CmdResult<SkillInstalled> {
    let mut request = git_request(core, url, path, git_ref)?;
    request.naming = naming(id, rename_if_taken);
    (request.decisions.trust, request.decisions.via) = sources::grant_of(trust);
    install_with(core, &request)
}

fn git_request(
    core: &Core,
    url: &str,
    path: Option<String>,
    git_ref: Option<String>,
) -> CmdResult<InstallRequest> {
    let url = url.trim();
    // A skills.sh address is the git install of its GitHub repository,
    // with its skill picked (#26); skills.sh itself is not contacted.
    if let Some(mapped) = agent_loadout_core::catalog::skills_sh_request(url) {
        let mut request = mapped?;
        request.path = trimmed(path);
        request.git_ref = trimmed(git_ref);
        return Ok(request);
    }
    let source = Source::parse(url, core.fs.as_ref());
    let is_url = matches!(source, Source::Git { .. }) && !Path::new(url).is_absolute();
    if !is_url {
        return Err(IpcError {
            code: ErrorCode::InvalidValue,
            message: "not a git URL".to_owned(),
        });
    }
    let mut request = InstallRequest::new(source);
    request.path = trimmed(path);
    request.git_ref = trimmed(git_ref);
    Ok(request)
}

/// Fetches and scans a git source for the install review, without
/// changing the store. Same URL rules as [`install_git`]. A blocked
/// source is refused (`source_blocked`) before it is contacted.
pub fn review_git(
    core: &Core,
    url: &str,
    path: Option<String>,
    git_ref: Option<String>,
) -> CmdResult<(Prepared, SkillInstallReview)> {
    let request = git_request(core, url, path, git_ref)?;
    review_request(core, &request)
}

/// The install review of a git `request` (a catalog entry's too, #26); a
/// blocked source is refused before it is contacted.
pub(super) fn review_request(
    core: &Core,
    request: &InstallRequest,
) -> CmdResult<(Prepared, SkillInstallReview)> {
    if let Source::Git { url, .. } = &request.source {
        let source = SourceRef::from_git_url(url).map_err(|err| scrubbed(&core.env, &err, None))?;
        sources::refuse_blocked(core, &source)?;
    }
    review_with(core, request)
}

/// Reads and scans the picked folder or archive for the install review.
pub fn review_local(core: &Core, path: &Path) -> CmdResult<(Prepared, SkillInstallReview)> {
    let request = local_request(core, path)?;
    review_with(core, &request)
}

fn review_with(core: &Core, request: &InstallRequest) -> CmdResult<(Prepared, SkillInstallReview)> {
    let prepared =
        install::prepare(&sx(core), request).map_err(|err| scrubbed(&core.env, &err, None))?;
    let mut review = review_of(&core.env, &prepared);
    review.trust = prepared
        .trust_source()
        .map(|source| sources::check_source(core, source, prepared.source_change()))
        .transpose()?;
    review.moved_trust = prepared
        .moved_to()
        .map(|target| sources::check_source(core, target, None))
        .transpose()?;
    Ok((prepared, review))
}

/// Installs reviewed content: exactly what the review showed. A git
/// source must be trusted or `trust` carries the dialog's answer
/// (`source_trust_required`, `source_blocked` otherwise), and so must the
/// repository it moved to (`moved_trust`, the answer for the review's
/// `moved_trust`); then a `dangerous` verdict needs `allow_dangerous`
/// ("Install anyway"); `scan_blocked` otherwise. On an error `prepared`
/// stays usable.
pub fn install_reviewed(
    core: &Core,
    prepared: &mut Prepared,
    id: Option<String>,
    rename_if_taken: bool,
    allow_dangerous: bool,
    trust: Option<TrustGrantArg>,
    moved_trust: Option<TrustGrantArg>,
) -> CmdResult<SkillInstalled> {
    let env = &core.env;
    let store = core.open_store_for_change()?;
    let (trust, via) = sources::grant_of(trust);
    let decisions = Decisions {
        allow_dangerous,
        trust,
        moved_trust: sources::grant_of(moved_trust).0,
        via,
    };
    let installed = install::install_prepared(
        &store,
        &sx(core),
        prepared,
        &naming(id, rename_if_taken),
        &decisions,
    )
    .map_err(|err| scrubbed(env, &err, None))?;
    Ok(installed_of(env, &installed))
}

fn require_store(core: &Core) -> CmdResult<Store<'_>> {
    core.open_store()?.ok_or_else(|| IpcError {
        code: ErrorCode::NotFound,
        message: "there is no skill in the store yet".to_owned(),
    })
}

/// Enables (or disables) skill `id` for `clients` at user scope; no
/// clients: all detected clients (or everywhere when disabling). `method`
/// switches between links and copies.
pub fn set_clients(
    core: &Core,
    id: &str,
    clients: &[String],
    enabled: bool,
    method: Option<SkillMethod>,
) -> CmdResult<SkillChange> {
    let env = &core.env;
    let store = require_store(core)?;
    let (commit, report) = link::set_enabled(
        &store,
        &sx(core),
        id,
        clients,
        enabled,
        method.map(Into::into),
    )
    .map_err(|err| scrubbed(env, &err, None))?;
    Ok(change_of(env, commit.as_ref(), &report))
}

/// Switches skill `id` between links and copies (SPEC 12.2); its clients
/// stay as they are.
pub fn set_method(core: &Core, id: &str, method: SkillMethod) -> CmdResult<SkillChange> {
    let env = &core.env;
    let store = require_store(core)?;
    let (commit, report) = link::set_method(&store, &sx(core), id, method.into())
        .map_err(|err| scrubbed(env, &err, None))?;
    Ok(change_of(env, commit.as_ref(), &report))
}

/// Enables (or disables) skill `skill` in project `project` (SPEC 12.3);
/// `hash` is the manifests' content hash the page showed.
pub fn set_project(
    core: &Core,
    project: &str,
    skill: &str,
    enabled: bool,
    hash: &str,
) -> CmdResult<SkillChange> {
    let env = &core.env;
    let store = require_store(core)?;
    let (root, _) = projects::validate_known_root(core, Some(&store), project)?;
    let (commit, report) =
        link::set_project_enabled(&store, &sx(core), &root, skill, enabled, Some(hash))
            .map_err(|err| scrubbed(env, &err, Some(&root)))?;
    Ok(change_of(env, commit.as_ref(), &report))
}

/// Removes skill `id` from the store; its owned links and copies follow.
pub fn remove(core: &Core, id: &str) -> CmdResult<SkillChange> {
    let env = &core.env;
    let store = require_store(core)?;
    let (commit, report) =
        skills::remove(&store, &sx(core), id).map_err(|err| scrubbed(env, &err, None))?;
    Ok(change_of(env, commit.as_ref(), &report))
}

/// Sets the tags of skill `id` (one commit, none when unchanged).
pub fn set_tags(core: &Core, id: &str, tags: &[String]) -> CmdResult<Option<String>> {
    let store = require_store(core)?;
    let commit =
        store::set_tags(&store, id, tags).map_err(|err| scrubbed(&core.env, &err, None))?;
    Ok(commit_text(commit.as_ref()))
}

fn changes_of(changes: &[install::FileChange]) -> Vec<SkillFileChange> {
    changes
        .iter()
        .map(|change| SkillFileChange {
            path: change.path.clone(),
            kind: change.kind.into(),
            script: change.script,
        })
        .collect()
}

/// Checks skills for updates (`ids`, or every skill when empty). Contacts
/// git sources; runs on request only.
pub fn check_updates(core: &Core, ids: &[String]) -> CmdResult<Vec<SkillUpdateCheck>> {
    let env = &core.env;
    let Some(store) = core.open_store()? else {
        return Ok(Vec::new());
    };
    let ids = if ids.is_empty() {
        store::ids(&store)?
    } else {
        ids.to_vec()
    };
    let sx = sx(core);
    // One skill's failure is that skill's entry; the others are checked.
    Ok(ids
        .iter()
        .map(|id| match install::check_update(&store, &sx, id) {
            Ok(checked) => SkillUpdateCheck {
                id: checked.id,
                status: checked.status.into(),
                current: checked.current,
                latest: checked.latest,
                changes: changes_of(&checked.changes),
                error: checked.error.as_ref().map(|err| scrubbed(env, err, None)),
                scan: checked.scan.as_ref().map(ScanResult::from),
                trust: checked
                    .trust
                    .as_ref()
                    .map(|check| sources::trust_info(env, check, checked.source_change.as_ref())),
                moved_trust: checked
                    .moved_trust
                    .as_ref()
                    .map(|check| sources::trust_info(env, check, None)),
            },
            Err(err) => SkillUpdateCheck {
                id: id.clone(),
                status: SkillUpdateStatus::Failed,
                current: None,
                latest: None,
                changes: Vec::new(),
                error: Some(scrubbed(env, &err, None)),
                scan: None,
                trust: None,
                moved_trust: None,
            },
        })
        .collect())
}

fn text_of(bytes: Option<&Vec<u8>>) -> Result<Option<String>, ()> {
    match bytes {
        None => Ok(None),
        Some(bytes) if bytes.len() > MAX_TEXT_BYTES => Err(()),
        Some(bytes) => String::from_utf8(bytes.clone()).map(Some).map_err(drop),
    }
}

fn file_missing(file: &str) -> IpcError {
    IpcError {
        code: ErrorCode::NotFound,
        message: format!("{file:?} is not a file of this skill"),
    }
}

/// One file of the update preview: the store copy and the source as the
/// last check fetched it. Reads no path from the caller: `file` is looked
/// up among the skill's files.
pub fn update_diff(core: &Core, id: &str, file: &str) -> CmdResult<SkillFileDiff> {
    let env = &core.env;
    let store = require_store(core)?;
    let scrub = |err: CoreError| scrubbed(env, &err, None);
    let stored = store::read_content(&store, id).map_err(scrub)?;
    let latest: SkillFiles = install::cached_latest(&store, &sx(core), id)
        .map_err(scrub)?
        .unwrap_or_default();
    let (before, after) = (stored.get(file), latest.get(file));
    if before.is_none() && after.is_none() {
        return Err(file_missing(file));
    }
    let (before, after, binary) = match (text_of(before), text_of(after)) {
        (Ok(before), Ok(after)) => (before, after, false),
        _ => (None, None, true),
    };
    Ok(SkillFileDiff {
        path: file.to_owned(),
        before,
        after,
        binary,
    })
}

/// Updates a git skill to `latest`, the source commit its update preview
/// showed (one commit; copies follow). The cache clone is not fetched
/// again; `entry_changed` when it holds another commit by now. An
/// untrusted source needs `trust`, the dialog's answer
/// (`source_trust_required`, `source_blocked`), and a repository it moved
/// to needs `moved_trust` (the answer for the check's `moved_trust`); the
/// skill's URL then follows the move. A `dangerous` scan verdict needs
/// `allow_dangerous` ("Update anyway"); `scan_blocked` otherwise.
pub fn update(
    core: &Core,
    id: &str,
    latest: &str,
    allow_dangerous: bool,
    trust: Option<TrustGrantArg>,
    moved_trust: Option<TrustGrantArg>,
) -> CmdResult<SkillUpdated> {
    let env = &core.env;
    let store = require_store(core)?;
    let (trust, via) = sources::grant_of(trust);
    let decisions = Decisions {
        allow_dangerous,
        trust,
        moved_trust: sources::grant_of(moved_trust).0,
        via,
    };
    let updated = install::update_reviewed(&store, &sx(core), id, latest, &decisions)
        .map_err(|err| scrubbed(env, &err, None))?;
    Ok(SkillUpdated {
        id: updated.id,
        from: updated.from,
        to: updated.to,
        commit: commit_text(updated.commit.as_ref()),
        changes: changes_of(&updated.changes),
        links: sync_of(env, &updated.report),
        scan: ScanResult::from(&updated.scan),
    })
}

/// The files of store skill `id`, by path.
pub fn files(core: &Core, id: &str) -> CmdResult<Vec<SkillFileEntry>> {
    let store = require_store(core)?;
    let content = store::read_content(&store, id).map_err(|err| scrubbed(&core.env, &err, None))?;
    Ok(content
        .iter()
        .map(|(path, bytes)| SkillFileEntry {
            path: path.clone(),
            bytes: count(bytes.len()),
        })
        .collect())
}

/// One file of store skill `id` as text (the file browser).
pub fn read_file(core: &Core, id: &str, file: &str) -> CmdResult<SkillFileText> {
    let store = require_store(core)?;
    let content = store::read_content(&store, id).map_err(|err| scrubbed(&core.env, &err, None))?;
    let bytes = content.get(file).ok_or_else(|| file_missing(file))?;
    Ok(SkillFileText {
        path: file.to_owned(),
        text: text_of(Some(bytes)).ok().flatten(),
        bytes: count(bytes.len()),
    })
}

/// The store folder of skill `id`.
pub fn skill_folder(core: &Core, id: &str) -> Result<PathBuf, CoreError> {
    let store = core
        .open_store()?
        .ok_or_else(|| CoreError::new(ErrorCode::NotFound, "there is no store yet"))?;
    let dir = store::content_dir(&store, id)?;
    if !core.fs.is_dir(&dir) {
        return Err(CoreError::new(
            ErrorCode::NotFound,
            format!("skill {id:?} has no content folder"),
        ));
    }
    Ok(dir)
}

/// The skill folder of `client` in `scope`, when it exists.
pub fn tool_folder(core: &Core, client: &str, scope: &SkillScope) -> CmdResult<PathBuf> {
    let (scope, root) = target_scope(core, scope)?;
    let dir = link::skills_dir(&core.env, client, scope, root.as_deref())
        .map_err(|err| scrubbed(&core.env, &err, root.as_deref()))?;
    if !core.fs.is_dir(&dir) {
        return Err(IpcError {
            code: ErrorCode::NotFound,
            message: format!("{client} has no skill folder yet"),
        });
    }
    Ok(dir)
}

/// Imports an unmanaged entry of a client folder into the store.
pub fn import_entry(
    core: &Core,
    client: &str,
    scope: &SkillScope,
    name: &str,
    id: Option<String>,
    rename_if_taken: bool,
) -> CmdResult<SkillInstalled> {
    let (scope, root) = target_scope(core, scope)?;
    let env = &core.env;
    let store = core.open_store_for_change()?;
    let installed = classify::import(
        &store,
        &sx(core),
        client,
        scope,
        root.as_deref(),
        name,
        &naming(id, rename_if_taken),
    )
    .map_err(|err| scrubbed(env, &err, root.as_deref()))?;
    Ok(installed_of(env, &installed))
}

/// Moves an entry that is not managed to `backups/trash/` (never deleted).
/// Returns where it went.
pub fn trash_entry(core: &Core, client: &str, scope: &SkillScope, name: &str) -> CmdResult<String> {
    let (scope, root) = target_scope(core, scope)?;
    let env = &core.env;
    let target = classify::trash(&sx(core), client, scope, root.as_deref(), name)
        .map_err(|err| scrubbed(env, &err, root.as_deref()))?;
    Ok(placeholder_path(env, &target, None))
}

/// "Vendor into store": an agents CLI skill becomes a store skill.
pub fn vendor(
    core: &Core,
    name: &str,
    id: Option<String>,
    rename_if_taken: bool,
) -> CmdResult<SkillInstalled> {
    let env = &core.env;
    let store = core.open_store_for_change()?;
    let installed = agents_cli::vendor(&store, &sx(core), name, &naming(id, rename_if_taken))
        .map_err(|err| scrubbed(env, &err, None))?;
    Ok(installed_of(env, &installed))
}

/// Runs the agents `skills` CLI (update or remove one skill); its output.
pub fn run_agents_cli(core: &Core, action: AgentsCliAction, name: &str) -> CmdResult<String> {
    let action = match action {
        AgentsCliAction::Update => CliAction::Update,
        AgentsCliAction::Remove => CliAction::Remove,
    };
    let output = agents_cli::run_cli(&sx(core), action, name)
        .map_err(|err| scrubbed(&core.env, &err, None))?;
    Ok(super::scrub(&core.env, &output, None))
}

#[tauri::command]
#[specta::specta]
pub async fn list_skills(
    state: tauri::State<'_, AppState>,
    scope: SkillScope,
) -> CmdResult<SkillList> {
    blocking(&state, move |core| list(core, &scope)).await
}

#[tauri::command]
#[specta::specta]
pub async fn list_skill_tool_dirs(
    state: tauri::State<'_, AppState>,
    scope: SkillScope,
) -> CmdResult<Vec<ToolSkillDir>> {
    blocking(&state, move |core| tool_dirs(core, &scope)).await
}

/// Opens the OS picker for a folder or a `.zip`/`.skill` file; `None` when
/// the user cancels. The path is validated and kept here; the frontend
/// gets a token and the placeholder path.
#[tauri::command]
#[specta::specta]
pub async fn pick_skill_source(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    kind: SkillSourcePick,
) -> CmdResult<Option<PickedSkillSource>> {
    use tauri_plugin_dialog::DialogExt;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let dialog = app.dialog().file();
        match kind {
            SkillSourcePick::Folder => dialog.blocking_pick_folder(),
            SkillSourcePick::Archive => dialog
                .add_filter("Skill archive", &["zip", "skill"])
                .blocking_pick_file(),
        }
    })
    .await
    .map_err(|err| IpcError {
        code: ErrorCode::IoError,
        message: format!("the picker failed: {err}"),
    })?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|_| IpcError {
        code: ErrorCode::InvalidPath,
        message: "the picked item is not a local path".to_owned(),
    })?;
    let (path, shown, name) = blocking(&state, move |core| inspect_source(core, &path)).await?;
    let token = state.remember_folder(path);
    Ok(Some(PickedSkillSource {
        token,
        path: shown,
        name,
    }))
}

#[tauri::command]
#[specta::specta]
pub async fn install_local_skill(
    state: tauri::State<'_, AppState>,
    token: String,
    id: Option<String>,
    rename_if_taken: bool,
) -> CmdResult<SkillInstalled> {
    let path = state.picked_folder(&token)?;
    blocking(&state, move |core| {
        install_local(core, &path, id, rename_if_taken)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn install_git_skill(
    state: tauri::State<'_, AppState>,
    url: String,
    path: Option<String>,
    git_ref: Option<String>,
    id: Option<String>,
    rename_if_taken: bool,
    trust: Option<TrustGrantArg>,
) -> CmdResult<SkillInstalled> {
    blocking(&state, move |core| {
        install_git(core, &url, path, git_ref, id, rename_if_taken, trust)
    })
    .await
}

/// Fetches and scans a git source for the install review; nothing goes
/// into the store. The review stays in the app under its token.
#[tauri::command]
#[specta::specta]
pub async fn review_git_skill(
    state: tauri::State<'_, AppState>,
    url: String,
    path: Option<String>,
    git_ref: Option<String>,
) -> CmdResult<SkillInstallReview> {
    let (prepared, mut review) =
        blocking(&state, move |core| review_git(core, &url, path, git_ref)).await?;
    let (token, evicted) = state.remember_review(prepared);
    drop_off_thread(evicted);
    review.token = token;
    Ok(review)
}

/// Keeps a review for `install_reviewed_skill` and gives it its token.
pub(super) fn remember_review(
    state: &AppState,
    prepared: Prepared,
    mut review: SkillInstallReview,
) -> SkillInstallReview {
    let (token, evicted) = state.remember_review(prepared);
    drop_off_thread(evicted);
    review.token = token;
    review
}

/// Reads and scans the folder or archive picked under `token` (from
/// `pick_skill_source`) for the install review.
#[tauri::command]
#[specta::specta]
pub async fn review_local_skill(
    state: tauri::State<'_, AppState>,
    token: String,
) -> CmdResult<SkillInstallReview> {
    let path = state.picked_folder(&token)?;
    let (prepared, mut review) = blocking(&state, move |core| review_local(core, &path)).await?;
    let (token, evicted) = state.remember_review(prepared);
    drop_off_thread(evicted);
    review.token = token;
    Ok(review)
}

/// Installs the reviewed content. The review stays in the store until
/// the install succeeds: on any error (`scan_blocked`, a trust error,
/// `already_exists`, a worker failure) the dialog can ask and try again
/// under the same token. Once the store commit is made the review is
/// spent (a sync failure after it is a warning of the result).
#[tauri::command]
#[specta::specta]
pub async fn install_reviewed_skill(
    state: tauri::State<'_, AppState>,
    token: String,
    id: Option<String>,
    rename_if_taken: bool,
    allow_dangerous: bool,
    trust: Option<TrustGrantArg>,
    moved_trust: Option<TrustGrantArg>,
) -> CmdResult<SkillInstalled> {
    let answers = (trust, moved_trust);
    install_review(
        &state,
        &token,
        id,
        rename_if_taken,
        allow_dangerous,
        answers,
    )
    .await
}

/// [`install_reviewed_skill`] on any [`AppState`]; `answers` are the
/// trust dialog's for the source and for the repository it moved to.
pub(crate) async fn install_review(
    state: &AppState,
    token: &str,
    id: Option<String>,
    rename_if_taken: bool,
    allow_dangerous: bool,
    answers: (Option<TrustGrantArg>, Option<TrustGrantArg>),
) -> CmdResult<SkillInstalled> {
    let review = state.review(token)?;
    let shared = Arc::clone(&review);
    let result = blocking(state, move |core| {
        let mut prepared = shared.lock().unwrap_or_else(PoisonError::into_inner);
        let (trust, moved_trust) = answers;
        install_reviewed(
            core,
            &mut prepared,
            id,
            rename_if_taken,
            allow_dangerous,
            trust,
            moved_trust,
        )
    })
    .await;
    let mut done = vec![review];
    if result.is_ok() {
        done.extend(state.forget_review(token));
    }
    drop_off_thread(done);
    result
}

/// Drops reviews on a worker thread: the last owner of a git review
/// removes its temporary clone, which must not block the async runtime.
fn drop_off_thread(reviews: Vec<SharedReview>) {
    if !reviews.is_empty() {
        tauri::async_runtime::spawn_blocking(move || drop(reviews));
    }
}

/// Removes temporary clones left by reviews of an earlier run (the app
/// ends without dropping its state): run once at startup.
pub(crate) fn sweep_review_clones(core: &Core) {
    install::sweep_temp_clones(&sx(core), std::time::Duration::from_secs(60 * 60));
}

/// Drops a review and its temporary clone (the dialog closed).
#[tauri::command]
#[specta::specta]
pub async fn discard_skill_review(
    state: tauri::State<'_, AppState>,
    token: String,
) -> CmdResult<()> {
    drop_off_thread(state.forget_review(&token).into_iter().collect());
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn set_skill_clients(
    state: tauri::State<'_, AppState>,
    id: String,
    clients: Vec<String>,
    enabled: bool,
    method: Option<SkillMethod>,
) -> CmdResult<SkillChange> {
    blocking(&state, move |core| {
        set_clients(core, &id, &clients, enabled, method)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn set_project_skill(
    state: tauri::State<'_, AppState>,
    project: String,
    skill: String,
    enabled: bool,
    hash: String,
) -> CmdResult<SkillChange> {
    blocking(&state, move |core| {
        set_project(core, &project, &skill, enabled, &hash)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn remove_skill(state: tauri::State<'_, AppState>, id: String) -> CmdResult<SkillChange> {
    blocking(&state, move |core| remove(core, &id)).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_skill_tags(
    state: tauri::State<'_, AppState>,
    id: String,
    tags: Vec<String>,
) -> CmdResult<Option<String>> {
    blocking(&state, move |core| set_tags(core, &id, &tags)).await
}

#[tauri::command]
#[specta::specta]
pub async fn check_skill_updates(
    state: tauri::State<'_, AppState>,
    ids: Vec<String>,
) -> CmdResult<Vec<SkillUpdateCheck>> {
    blocking(&state, move |core| check_updates(core, &ids)).await
}

#[tauri::command]
#[specta::specta]
pub async fn skill_update_diff(
    state: tauri::State<'_, AppState>,
    id: String,
    file: String,
) -> CmdResult<SkillFileDiff> {
    blocking(&state, move |core| update_diff(core, &id, &file)).await
}

#[tauri::command]
#[specta::specta]
pub async fn update_skill(
    state: tauri::State<'_, AppState>,
    id: String,
    latest: String,
    allow_dangerous: bool,
    trust: Option<TrustGrantArg>,
    moved_trust: Option<TrustGrantArg>,
) -> CmdResult<SkillUpdated> {
    blocking(&state, move |core| {
        update(core, &id, &latest, allow_dangerous, trust, moved_trust)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn set_skill_method(
    state: tauri::State<'_, AppState>,
    id: String,
    method: SkillMethod,
) -> CmdResult<SkillChange> {
    blocking(&state, move |core| set_method(core, &id, method)).await
}

#[tauri::command]
#[specta::specta]
pub async fn list_skill_files(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CmdResult<Vec<SkillFileEntry>> {
    blocking(&state, move |core| files(core, &id)).await
}

#[tauri::command]
#[specta::specta]
pub async fn read_skill_file(
    state: tauri::State<'_, AppState>,
    id: String,
    file: String,
) -> CmdResult<SkillFileText> {
    blocking(&state, move |core| read_file(core, &id, &file)).await
}

/// Opens the store folder of skill `id`.
#[tauri::command]
#[specta::specta]
pub async fn reveal_skill_folder(state: tauri::State<'_, AppState>, id: String) -> CmdResult<()> {
    blocking(&state, move |core| {
        let dir = skill_folder(core, &id).map_err(|err| scrubbed(&core.env, &err, None))?;
        super::tools::open_folder(&dir).map_err(|err| scrubbed(&core.env, &err, None))
    })
    .await
}

/// Opens `client`'s skill folder in `scope`.
#[tauri::command]
#[specta::specta]
pub async fn reveal_skill_tool_folder(
    state: tauri::State<'_, AppState>,
    client: String,
    scope: SkillScope,
) -> CmdResult<()> {
    blocking(&state, move |core| {
        let dir = tool_folder(core, &client, &scope)?;
        super::tools::open_folder(&dir).map_err(|err| scrubbed(&core.env, &err, None))
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn import_tool_skill(
    state: tauri::State<'_, AppState>,
    client: String,
    scope: SkillScope,
    name: String,
    id: Option<String>,
    rename_if_taken: bool,
) -> CmdResult<SkillInstalled> {
    blocking(&state, move |core| {
        import_entry(core, &client, &scope, &name, id, rename_if_taken)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn trash_tool_skill(
    state: tauri::State<'_, AppState>,
    client: String,
    scope: SkillScope,
    name: String,
) -> CmdResult<String> {
    blocking(&state, move |core| {
        trash_entry(core, &client, &scope, &name)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn vendor_agents_skill(
    state: tauri::State<'_, AppState>,
    name: String,
    id: Option<String>,
    rename_if_taken: bool,
) -> CmdResult<SkillInstalled> {
    blocking(&state, move |core| vendor(core, &name, id, rename_if_taken)).await
}

#[tauri::command]
#[specta::specta]
pub async fn run_agents_skill_cli(
    state: tauri::State<'_, AppState>,
    action: AgentsCliAction,
    name: String,
) -> CmdResult<String> {
    blocking(&state, move |core| run_agents_cli(core, action, &name)).await
}

// Adopt and bulk import of unmanaged folders (SPEC 8.6, 12.5, #3).

/// An unmanaged folder of a client skill folder.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UnmanagedSkillFolder {
    pub client: String,
    /// Under its placeholder root.
    pub path: String,
}

/// One content of an unmanaged skill name.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UnmanagedSkillVariant {
    /// Names the content in `adopt_skills` choices.
    pub hash: String,
    /// Empty for the store's copy alone.
    pub folders: Vec<UnmanagedSkillFolder>,
    /// The store skill of this name holds this content.
    pub in_store: bool,
    pub description: Option<String>,
    pub bytes: u32,
}

/// Unmanaged folders of one name, grouped by content.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UnmanagedSkill {
    pub name: String,
    /// Several contents (the store's included): pick one, or import the
    /// others as `name-<client>`.
    pub conflict: bool,
    pub variants: Vec<UnmanagedSkillVariant>,
}

/// An unmanaged folder that cannot be adopted.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UnmanagedSkillProblem {
    pub client: String,
    pub name: String,
    pub path: String,
    pub error: IpcError,
}

/// The unmanaged skill folders of a scope.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UnmanagedSkills {
    pub candidates: Vec<UnmanagedSkill>,
    pub problems: Vec<UnmanagedSkillProblem>,
}

/// What to do with the folders of one name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillAdoptAction {
    /// Adopt the content `variant` (a hash) as the name; folders with
    /// other content are kept.
    Pick {
        variant: String,
    },
    /// Adopt `primary` as the name and every other content as
    /// `name-<client>`.
    Both {
        primary: String,
    },
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Type)]
pub struct SkillAdoptChoice {
    pub name: String,
    pub action: SkillAdoptAction,
}

/// What happened to one unmanaged folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SkillFolderStatus {
    /// Replaced by the managed link; the folder is in the trash.
    Adopted,
    /// Another content of its name was chosen; kept.
    Differs,
    /// Several contents and no choice; kept.
    Conflict,
    Skipped,
    /// Its content is in the store, but the folder could not be replaced
    /// (see the error); kept.
    Kept,
}

impl From<adopt::FolderStatus> for SkillFolderStatus {
    fn from(status: adopt::FolderStatus) -> Self {
        match status {
            adopt::FolderStatus::Adopted => Self::Adopted,
            adopt::FolderStatus::Differs => Self::Differs,
            adopt::FolderStatus::Conflict => Self::Conflict,
            adopt::FolderStatus::Skipped => Self::Skipped,
            adopt::FolderStatus::Kept => Self::Kept,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillAdoptedFolder {
    pub client: String,
    pub name: String,
    pub path: String,
    /// The store skill it went (or would go) into.
    pub skill_id: Option<String>,
    pub status: SkillFolderStatus,
    /// Where the folder went, under its placeholder root.
    pub trash: Option<String>,
    pub error: Option<IpcError>,
}

/// What an adopt did: one store commit, then the folders.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillAdoptResult {
    pub commit: Option<String>,
    /// Store skills the action added.
    pub added: Vec<String>,
    /// Store skills that held the content already.
    pub reused: Vec<String>,
    pub folders: Vec<SkillAdoptedFolder>,
    pub problems: Vec<UnmanagedSkillProblem>,
    pub links: SkillSync,
    /// A folder is left for the user or an entry needs a decision.
    pub needs_attention: bool,
}

fn problems_of(
    env: &Env,
    root: Option<&Path>,
    problems: &[adopt::FolderProblem],
) -> Vec<UnmanagedSkillProblem> {
    problems
        .iter()
        .map(|problem| UnmanagedSkillProblem {
            client: problem.client.to_owned(),
            name: problem.name.clone(),
            path: placeholder_path(env, &problem.path, root),
            error: scrubbed(env, &problem.error, root),
        })
        .collect()
}

/// Content summaries of the folders `scan_unmanaged_skills` read, for this
/// process: the unmanaged-entry notice scans on focus and watcher events,
/// and an unchanged folder (same file listing) is not read again.
static FOLDER_HASHES: std::sync::LazyLock<FolderHashCache> =
    std::sync::LazyLock::new(FolderHashCache::new);

/// The unmanaged folders of `scope` (every client's, or `client`'s),
/// grouped by name and content (SPEC 8.6, 12.5). Reads only; folder
/// contents come from [`FOLDER_HASHES`] while their listing is unchanged
/// (an adopt reads and compares every folder again).
pub fn scan_unmanaged(
    core: &Core,
    scope: &SkillScope,
    client: Option<&str>,
) -> CmdResult<UnmanagedSkills> {
    let (scope, root) = target_scope(core, scope)?;
    let env = &core.env;
    let root = root.as_deref();
    let store = core.open_store()?;
    let scan = adopt::scan_cached(
        store.as_ref(),
        &sx(core),
        scope,
        root,
        client,
        &FOLDER_HASHES,
    )
    .map_err(|err| scrubbed(env, &err, root))?;
    Ok(UnmanagedSkills {
        candidates: scan
            .candidates
            .iter()
            .map(|candidate| UnmanagedSkill {
                name: candidate.name.clone(),
                conflict: candidate.is_conflict(),
                variants: candidate
                    .variants
                    .iter()
                    .map(|variant| UnmanagedSkillVariant {
                        hash: variant.hash.clone(),
                        folders: variant
                            .folders
                            .iter()
                            .map(|folder| UnmanagedSkillFolder {
                                client: folder.client.to_owned(),
                                path: placeholder_path(env, &folder.path, root),
                            })
                            .collect(),
                        in_store: variant.in_store,
                        description: variant.description.clone(),
                        bytes: size(variant.bytes),
                    })
                    .collect(),
            })
            .collect(),
        problems: problems_of(env, root, &scan.problems),
    })
}

/// Adopts unmanaged folders of `scope` as `choices` say (names without a
/// choice are skipped): one store commit, then each folder whose content
/// equals its store copy is replaced by the managed link and moved to
/// `backups/trash/`.
pub fn adopt_folders(
    core: &Core,
    scope: &SkillScope,
    client: Option<&str>,
    choices: &[SkillAdoptChoice],
) -> CmdResult<SkillAdoptResult> {
    let (scope, root) = target_scope(core, scope)?;
    let env = &core.env;
    let root = root.as_deref();
    let choices: Vec<adopt::AdoptChoice> = choices
        .iter()
        .map(|choice| adopt::AdoptChoice {
            name: choice.name.clone(),
            action: match &choice.action {
                SkillAdoptAction::Pick { variant } => adopt::AdoptAction::Pick {
                    variant: variant.clone(),
                },
                SkillAdoptAction::Both { primary } => adopt::AdoptAction::Both {
                    primary: primary.clone(),
                },
                SkillAdoptAction::Skip => adopt::AdoptAction::Skip,
            },
        })
        .collect();
    let store = core.open_store_for_change()?;
    let report = adopt::adopt(&store, &sx(core), scope, root, client, &choices)
        .map_err(|err| scrubbed(env, &err, root))?;
    let path = |path: &Path| placeholder_path(env, path, root);
    Ok(SkillAdoptResult {
        commit: commit_text(report.commit.as_ref()),
        added: report.added.clone(),
        reused: report.reused.clone(),
        folders: report
            .folders
            .iter()
            .map(|folder| SkillAdoptedFolder {
                client: folder.client.to_owned(),
                name: folder.name.clone(),
                path: path(&folder.path),
                skill_id: folder.skill_id.clone(),
                status: folder.status.into(),
                trash: folder.trash.as_deref().map(path),
                error: folder.error.as_ref().map(|err| scrubbed(env, err, root)),
            })
            .collect(),
        problems: problems_of(env, root, &report.problems),
        links: sync_of(env, &report.sync),
        needs_attention: report.needs_attention(),
    })
}

#[tauri::command]
#[specta::specta]
pub async fn scan_unmanaged_skills(
    state: tauri::State<'_, AppState>,
    scope: SkillScope,
    client: Option<String>,
) -> CmdResult<UnmanagedSkills> {
    blocking(&state, move |core| {
        scan_unmanaged(core, &scope, client.as_deref())
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn adopt_skills(
    state: tauri::State<'_, AppState>,
    scope: SkillScope,
    client: Option<String>,
    choices: Vec<SkillAdoptChoice>,
) -> CmdResult<SkillAdoptResult> {
    blocking(&state, move |core| {
        adopt_folders(core, &scope, client.as_deref(), &choices)
    })
    .await
}

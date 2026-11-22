//! Distribution of store skills to client skill directories (SPEC 12.2,
//! 12.3, table 7.6, Tier A and B rows).
//!
//! - `link`: a directory junction on Windows (no Developer Mode or admin
//!   needed, made with the Win32 API through the `junction` crate), or a
//!   directory symbolic link when chosen ([`LinkKind::Symlink`]); a
//!   symbolic link elsewhere.
//! - `copy`: the files plus a [`MARKER_FILE`], owned by content hash, so an
//!   edit in place shows as `changed` and is never overwritten or removed.
//!
//! An existing link to something else is a `collision`, and a real folder
//! or file of the same name is `foreign`: neither is ever touched. Only
//! entries the ledger owns, and that are unchanged, are replaced or
//! removed.
//!
//! Ownership lives in `state/ledger.json` under the top-level key
//! `skill_links` (see [`SkillLedger`]), next to the MCP entries, so the
//! ledger stays the one record of what Loadout owns on this machine.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::adapters::detect::detect;
use crate::adapters::location::Location;
use crate::apply::ledger::Ledger;
use crate::env::Env;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::layering::git::ProjectRepo;
use crate::layering::projects::{mirror, this_overlay};
use crate::layering::{Project, ProjectStatus, TrustStore, known_projects, same_folder, same_path};
use crate::model::applied::Scope;
use crate::model::clients::ClientSelection;
use crate::model::ids::{self, IdKind};
use crate::model::project::ProjectManifest;
use crate::model::skill_meta::{DistributionMethod, SkillEnable, SkillMeta};
use crate::skills::store::{self, content_dir};
use crate::skills::{
    Entry, MARKER_FILE, SkillFiles, SkillsEnv, confine, content_hash, entry, has_nested_git,
    io_error, read_folder, write_files,
};
use crate::store::{
    self as store_mod, CommitId, LOCK_TIMEOUT, Operation, Store, StoreLock, utc_datetime,
};

/// Ledger key holding skill links.
pub const LEDGER_KEY: &str = "skill_links";
const RECORD_KIND: &str = "skill_link";

/// How links are made on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkKind {
    /// A directory junction (default).
    #[default]
    Junction,
    /// A directory symbolic link (needs Developer Mode or admin rights).
    Symlink,
}

/// How one entry was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkMethod {
    Junction,
    Symlink,
    Copy,
}

impl LinkMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            LinkMethod::Junction => "junction",
            LinkMethod::Symlink => "symlink",
            LinkMethod::Copy => "copy",
        }
    }

    fn is_link(self) -> bool {
        self != LinkMethod::Copy
    }
}

/// A client's skill directories (table 7.6).
#[derive(Debug, Clone, Copy)]
pub struct ClientSkills {
    pub client: &'static str,
    /// User scope folder.
    pub user: Location,
    /// Project scope folder, relative to the project root.
    pub project: &'static str,
}

/// Tier A and B clients with skill folders (table 7.6). Claude Desktop has
/// none. Codex user skills go to `$CODEX_HOME/skills` [verify: Codex may
/// read `~/.agents/skills` instead]; the project folders other than Claude
/// Code's are [verify] in the spec.
pub static CLIENT_SKILLS: [ClientSkills; 6] = [
    ClientSkills {
        client: "claude-code",
        user: Location::EnvDir {
            var: "CLAUDE_CONFIG_DIR",
            fallback: &Location::Home(".claude"),
            rel: "skills",
            tilde: false,
        },
        project: ".claude/skills",
    },
    ClientSkills {
        client: "omp",
        user: Location::EnvDir {
            var: "PI_CODING_AGENT_DIR",
            fallback: &Location::HomeNamedDir {
                var: "PI_CONFIG_DIR",
                default: ".omp",
                rel: "agent",
            },
            rel: "skills",
            tilde: false,
        },
        project: ".omp/skills",
    },
    ClientSkills {
        client: "codex",
        user: Location::EnvDir {
            var: "CODEX_HOME",
            fallback: &Location::Home(".codex"),
            rel: "skills",
            tilde: false,
        },
        project: ".agents/skills",
    },
    ClientSkills {
        client: "pi",
        user: Location::EnvDir {
            var: "PI_CODING_AGENT_DIR",
            fallback: &Location::Home(".pi/agent"),
            rel: "skills",
            tilde: true,
        },
        project: ".pi/skills",
    },
    ClientSkills {
        client: "copilot",
        user: Location::EnvDir {
            var: "COPILOT_HOME",
            fallback: &Location::Home(".copilot"),
            rel: "skills",
            tilde: false,
        },
        project: ".github/skills",
    },
    ClientSkills {
        client: "cursor",
        user: Location::Home(".cursor/skills"),
        project: ".cursor/skills",
    },
];

/// The skill folder entry of `client`, if it has one.
pub fn client_skills(client: &str) -> Option<&'static ClientSkills> {
    CLIENT_SKILLS.iter().find(|entry| entry.client == client)
}

/// The skill folder of `client` in `scope` (`project` for the project
/// scope). `scope_not_supported` for a client without skill folders.
pub fn skills_dir(
    env: &Env,
    client: &str,
    scope: Scope,
    project: Option<&Path>,
) -> Result<PathBuf, CoreError> {
    let entry = client_skills(client).ok_or_else(|| no_skills(client))?;
    match scope {
        Scope::User => entry
            .user
            .resolve(env, None)
            .ok_or_else(|| no_skills(client)),
        Scope::Project | Scope::ProjectLocal => {
            let root = project.filter(|root| root.is_absolute()).ok_or_else(|| {
                CoreError::new(
                    ErrorCode::InvalidPath,
                    "a project scope needs an absolute folder",
                )
            })?;
            Ok(entry
                .project
                .split('/')
                .fold(root.to_path_buf(), |path, part| path.join(part)))
        }
    }
}

fn no_skills(client: &str) -> CoreError {
    let code = if crate::model::clients::known_client(client).is_some() {
        ErrorCode::ScopeNotSupported
    } else {
        ErrorCode::UnknownClient
    };
    CoreError::new(
        code,
        format!("{client:?} has no skill folder Loadout writes"),
    )
}

/// One owned skill entry (`state/ledger.json` `skill_links`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillLink {
    /// Always `skill_link`.
    pub kind: String,
    pub client: String,
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub skill_id: String,
    /// The entry in the client's folder, absolute.
    pub path: String,
    pub method: LinkMethod,
    /// The store folder it was made from, absolute.
    pub target: String,
    /// Content hash of a copy ([`content_hash`]); `None` for links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_hash: Option<String>,
    /// RFC 3339, UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_at: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// The skill part of the ledger. Loaded and saved with the rest of
/// `state/ledger.json`, whose MCP entries are kept as they are.
#[derive(Debug, Clone, Default)]
pub struct SkillLedger {
    ledger: Ledger,
    pub links: Vec<SkillLink>,
}

impl SkillLedger {
    pub fn load(fs: &dyn Fs, state_dir: &Path) -> Result<Self, CoreError> {
        let ledger = Ledger::load(fs, state_dir)?;
        let links = match ledger.extra.get(LEDGER_KEY) {
            Some(value) => serde_json::from_value(value.clone()).map_err(|err| {
                CoreError::new(
                    ErrorCode::ParseError,
                    format!("the ledger's {LEDGER_KEY} do not parse: {err}"),
                )
            })?,
            None => Vec::new(),
        };
        Ok(Self { ledger, links })
    }

    pub fn save(&mut self, fs: &dyn Fs, state_dir: &Path) -> Result<(), CoreError> {
        let value = serde_json::to_value(&self.links).map_err(|err| {
            CoreError::new(
                ErrorCode::InvalidValue,
                format!("cannot serialize skill links: {err}"),
            )
        })?;
        if self.links.is_empty() {
            self.ledger.extra.remove(LEDGER_KEY);
        } else {
            self.ledger.extra.insert(LEDGER_KEY.to_owned(), value);
        }
        fs.create_dir_all(state_dir)
            .map_err(|err| io_error("cannot create directory", state_dir, &err))?;
        self.ledger.save(fs, state_dir)
    }

    /// The owned entry at `path`.
    pub fn get(&self, path: &Path) -> Option<&SkillLink> {
        self.links
            .iter()
            .find(|link| same_path(Path::new(&link.path), path))
    }

    fn upsert(&mut self, link: SkillLink) {
        self.remove(Path::new(&link.path));
        self.links.push(link);
    }

    fn remove(&mut self, path: &Path) -> bool {
        let before = self.links.len();
        self.links
            .retain(|link| !same_path(Path::new(&link.path), path));
        self.links.len() != before
    }

    /// Drops ownership of `path` (moved to the trash, imported).
    pub fn forget(&mut self, path: &Path) -> bool {
        self.remove(path)
    }
}

/// What happened to one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkStatus {
    /// Made now.
    Created,
    /// A link to the store skill was already there; now owned.
    Adopted,
    /// Owned and as desired.
    Unchanged,
    /// An owned copy was rewritten with new store content, or an owned
    /// entry was remade with another method.
    Updated,
    /// An owned entry that is no longer desired was removed (or was gone).
    Removed,
    /// A real folder or file of that name: never overwritten (offer import).
    Foreign,
    /// A link to something else (ask).
    Collision,
    /// An owned entry was edited in place (a copy's files, a link's
    /// target): left as it is and no longer owned.
    Changed,
    /// Not done; see the error (no skill content, untrusted project, a
    /// failed write).
    Blocked,
}

impl LinkStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            LinkStatus::Created => "created",
            LinkStatus::Adopted => "adopted",
            LinkStatus::Unchanged => "unchanged",
            LinkStatus::Updated => "updated",
            LinkStatus::Removed => "removed",
            LinkStatus::Foreign => "foreign",
            LinkStatus::Collision => "collision",
            LinkStatus::Changed => "changed",
            LinkStatus::Blocked => "blocked",
        }
    }

    /// Whether the entry needs the user (SPEC 14 exit code 4).
    pub fn needs_decision(self) -> bool {
        matches!(
            self,
            LinkStatus::Foreign | LinkStatus::Collision | LinkStatus::Changed
        )
    }
}

/// One entry of a [`SyncReport`].
#[derive(Debug, Clone)]
pub struct LinkResult {
    pub client: String,
    pub scope: Scope,
    pub project_id: Option<String>,
    pub skill_id: String,
    pub path: PathBuf,
    pub method: LinkMethod,
    pub status: LinkStatus,
    /// The link target found, for a collision.
    pub found_target: Option<PathBuf>,
    pub error: Option<CoreError>,
}

/// What [`sync`] did.
#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    pub results: Vec<LinkResult>,
    /// Unknown clients, missing skills, untrusted projects.
    pub warnings: Vec<CoreError>,
}

impl SyncReport {
    /// Whether any entry needs the user or failed.
    pub fn needs_attention(&self) -> bool {
        self.results
            .iter()
            .any(|result| result.status.needs_decision() || result.status == LinkStatus::Blocked)
    }

    /// Entries that changed something or need attention.
    pub fn notable(&self) -> impl Iterator<Item = &LinkResult> {
        self.results
            .iter()
            .filter(|result| result.status != LinkStatus::Unchanged)
    }
}

/// One desired entry.
#[derive(Debug, Clone)]
struct Desired {
    client: &'static str,
    scope: Scope,
    project_id: Option<String>,
    skill_id: String,
    path: PathBuf,
    source: PathBuf,
    method: LinkMethod,
    /// The project folder of a project entry. Its repository hides links
    /// (`share = false`) through `info/exclude` and shows copies
    /// (`share = true`, SPEC 12.3).
    project_root: Option<PathBuf>,
}

fn link_method(sx: &SkillsEnv<'_>) -> LinkMethod {
    if cfg!(windows) && sx.link_kind == LinkKind::Junction {
        LinkMethod::Junction
    } else {
        LinkMethod::Symlink
    }
}

/// Client ids of a selection that have skill folders; others are warnings.
fn resolve_clients(
    sx: &SkillsEnv<'_>,
    selection: &ClientSelection,
    what: &str,
    warnings: &mut Vec<CoreError>,
) -> Vec<&'static str> {
    match selection {
        ClientSelection::AllDetected => CLIENT_SKILLS
            .iter()
            .filter(|entry| {
                crate::adapters::adapter(entry.client)
                    .is_some_and(|spec| detect(spec, sx.env, sx.fs, sx.registry).detected())
            })
            .map(|entry| entry.client)
            .collect(),
        ClientSelection::List(ids) => {
            let mut out = Vec::new();
            for id in ids {
                match client_skills(id) {
                    Some(entry) => {
                        if !out.contains(&entry.client) {
                            out.push(entry.client);
                        }
                    }
                    None => warnings.push(CoreError::new(
                        no_skills(id).code(),
                        format!("{what}: {id:?} has no skill folder Loadout writes; skipped"),
                    )),
                }
            }
            out
        }
    }
}

/// The projects whose links were evaluated, and the desired entries.
fn desired(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    report: &mut SyncReport,
) -> Result<(Vec<Desired>, Vec<String>), CoreError> {
    let mut out: Vec<Desired> = Vec::new();
    let skills = store::list(store)?;
    let has_content = |id: &str| {
        skills
            .iter()
            .any(|skill| skill.id == id && skill.has_content)
    };
    let push = |out: &mut Vec<Desired>, desired: Desired| {
        if !out
            .iter()
            .any(|known| same_path(&known.path, &desired.path))
        {
            out.push(desired);
        }
    };

    for skill in &skills {
        let Some(selection) = skill
            .meta
            .as_ref()
            .and_then(|meta| meta.enable.as_ref())
            .and_then(|enable| enable.clients.as_ref())
        else {
            continue;
        };
        if !skill.has_content {
            report.warnings.push(store::not_found(&skill.id));
            continue;
        }
        let method = match skill
            .meta
            .as_ref()
            .and_then(|meta| meta.enable.as_ref())
            .and_then(|enable| enable.method)
        {
            Some(DistributionMethod::Copy) => LinkMethod::Copy,
            _ => link_method(sx),
        };
        let what = format!("skill {:?}", skill.id);
        for client in resolve_clients(sx, selection, &what, &mut report.warnings) {
            let dir = skills_dir(sx.env, client, Scope::User, None)?;
            push(
                &mut out,
                Desired {
                    client,
                    scope: Scope::User,
                    project_id: None,
                    skill_id: skill.id.clone(),
                    path: dir.join(&skill.id),
                    source: content_dir(store, &skill.id)?,
                    method,
                    project_root: None,
                },
            );
        }
    }

    let state = sx.home.state_dir();
    let overlay = this_overlay(store, sx.fs, &state)?;
    let trust = TrustStore::load(sx.fs, &state)?;
    let mut evaluated = Vec::new();
    for known in known_projects(sx.fs, sx.env, overlay.as_ref(), &trust) {
        let ProjectStatus::Loaded { project, .. } = &known.status else {
            continue;
        };
        let wanted = project_skills(project);
        if let Some(err) = known.trust_error() {
            if !wanted.enable.is_empty() {
                report.warnings.push(err);
            }
            continue;
        }
        evaluated.push(known.id.clone());
        let what = format!("project {:?}", known.id);
        let clients = resolve_clients(sx, &wanted.clients, &what, &mut report.warnings);
        let method = if wanted.share {
            LinkMethod::Copy
        } else {
            link_method(sx)
        };
        for id in &wanted.enable {
            if !ids::is_valid(IdKind::Skill, id) || !has_content(id) {
                report.warnings.push(CoreError::new(
                    ErrorCode::NotFound,
                    format!("{what} enables skill {id:?}, which is not in the store"),
                ));
                continue;
            }
            for client in &clients {
                let dir = skills_dir(sx.env, client, Scope::Project, Some(&known.root))?;
                push(
                    &mut out,
                    Desired {
                        client,
                        scope: Scope::Project,
                        project_id: Some(known.id.clone()),
                        skill_id: id.clone(),
                        path: dir.join(id),
                        source: content_dir(store, id)?,
                        method,
                        project_root: Some(known.root.clone()),
                    },
                );
            }
        }
    }
    Ok((out, evaluated))
}

/// `[skills]` of a project's manifests: `enable` from both files, `share`
/// and `defaults.clients` from the private file over the shared one.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectSkills {
    pub enable: Vec<String>,
    pub share: bool,
    pub clients: ClientSelection,
}

/// The client ids of `selection` that have skill folders, `all-detected`
/// resolved with the detection probes (for listings; unknown clients are
/// left out).
pub fn selected_clients(sx: &SkillsEnv<'_>, selection: &ClientSelection) -> Vec<&'static str> {
    let mut ignored = Vec::new();
    resolve_clients(sx, selection, "", &mut ignored)
}

/// `[skills]` of `project` as links are made from it.
pub fn project_skills(project: &Project) -> ProjectSkills {
    let layers: Vec<&ProjectManifest> = [project.local.as_ref(), project.shared.as_ref()]
        .into_iter()
        .flatten()
        .map(|file| &file.manifest)
        .collect();
    let mut enable: Vec<String> = Vec::new();
    for manifest in layers.iter().rev() {
        for id in manifest
            .skills
            .as_ref()
            .and_then(|skills| skills.enable.as_ref())
            .into_iter()
            .flatten()
        {
            if !enable.contains(id) {
                enable.push(id.clone());
            }
        }
    }
    let share = layers
        .iter()
        .find_map(|manifest| manifest.skills.as_ref().and_then(|skills| skills.share))
        .unwrap_or(false);
    let clients = layers
        .iter()
        .find_map(|manifest| {
            manifest
                .defaults
                .as_ref()
                .and_then(|defaults| defaults.clients.clone())
        })
        .unwrap_or(ClientSelection::AllDetected);
    ProjectSkills {
        enable,
        share,
        clients,
    }
}

/// Makes client skill folders match the store (SPEC 12.2, 12.3): every
/// skill's `enable.clients` at user scope, and `[skills] enable` of every
/// trusted project this machine knows. Creates missing entries, rewrites
/// owned unchanged copies whose store content changed, removes owned
/// unchanged entries that are no longer desired, and reports foreign,
/// colliding and changed entries without touching them. Projects that are
/// not trusted, missing or invalid are left alone. Runs under the store
/// lock; call it after a commit, never inside a transaction.
pub fn sync(store: &Store<'_>, sx: &SkillsEnv<'_>) -> Result<SyncReport, CoreError> {
    let state = sx.home.state_dir();
    let _lock = StoreLock::acquire(sx.fs, &state, LOCK_TIMEOUT)?;
    let mut ledger = SkillLedger::load(sx.fs, &state)?;
    let mut report = SyncReport::default();
    let (wanted, evaluated) = desired(store, sx, &mut report)?;
    let now = utc_datetime(sx.env.now()).to_string();

    for desired in &wanted {
        let result = ensure(sx, &mut ledger, desired, &now);
        report.results.push(result);
    }

    let wanted_paths: Vec<&PathBuf> = wanted.iter().map(|desired| &desired.path).collect();
    let stale: Vec<SkillLink> = ledger
        .links
        .iter()
        .filter(|link| {
            !wanted_paths
                .iter()
                .any(|path| same_path(path, Path::new(&link.path)))
        })
        .filter(|link| match link.scope {
            Scope::User => true,
            Scope::Project | Scope::ProjectLocal => link
                .project_id
                .as_ref()
                .is_some_and(|id| evaluated.contains(id)),
        })
        .cloned()
        .collect();
    for link in stale {
        report.results.push(retire(sx, &mut ledger, &link));
    }
    ledger.save(sx.fs, &state)?;
    Ok(report)
}

fn result(desired: &Desired, status: LinkStatus) -> LinkResult {
    LinkResult {
        client: desired.client.to_owned(),
        scope: desired.scope,
        project_id: desired.project_id.clone(),
        skill_id: desired.skill_id.clone(),
        path: desired.path.clone(),
        method: desired.method,
        status,
        found_target: None,
        error: None,
    }
}

fn ensure(
    sx: &SkillsEnv<'_>,
    ledger: &mut SkillLedger,
    desired: &Desired,
    now: &str,
) -> LinkResult {
    match try_ensure(sx, ledger, desired, now) {
        Ok(result) => result,
        Err(err) => LinkResult {
            error: Some(err),
            ..result(desired, LinkStatus::Blocked)
        },
    }
}

fn try_ensure(
    sx: &SkillsEnv<'_>,
    ledger: &mut SkillLedger,
    desired: &Desired,
    now: &str,
) -> Result<LinkResult, CoreError> {
    let fs = sx.fs;
    let owned = ledger.get(&desired.path).cloned();
    let status = match entry(fs, &desired.path)? {
        Entry::Missing => {
            // The exclude line first: a link that cannot be hidden is
            // never made.
            git_visibility(fs, desired)?;
            let hash = create(fs, desired)?;
            ledger.upsert(record(desired, hash, now));
            LinkStatus::Created
        }
        Entry::Link(found) => {
            let to_store = same_folder(fs, &found, &desired.source);
            let owned_link = owned.as_ref().filter(|link| {
                link.method.is_link() && same_folder(fs, &found, Path::new(&link.target))
            });
            if to_store && desired.method.is_link() {
                git_visibility(fs, desired)?;
                let status = if owned_link.is_some() {
                    LinkStatus::Unchanged
                } else {
                    LinkStatus::Adopted
                };
                let mut link = record(desired, None, now);
                if let Some(owned) = owned_link {
                    // Keep how it was made (a junction stays a junction).
                    link.method = owned.method;
                    link.written_at.clone_from(&owned.written_at);
                }
                ledger.upsert(link);
                status
            } else if owned_link.is_some() {
                // Owned and unchanged, but another target or method is
                // desired now.
                git_visibility(fs, desired)?;
                remove_link(fs, &desired.path)?;
                let hash = create(fs, desired)?;
                ledger.upsert(record(desired, hash, now));
                LinkStatus::Updated
            } else if owned.is_some() {
                ledger.remove(&desired.path);
                LinkStatus::Changed
            } else {
                return Ok(LinkResult {
                    found_target: Some(found),
                    ..result(desired, LinkStatus::Collision)
                });
            }
        }
        Entry::Dir => match owned.filter(|link| link.method == LinkMethod::Copy) {
            Some(link) => match unchanged_copy(fs, &desired.path, &link)? {
                None => {
                    ledger.remove(&desired.path);
                    LinkStatus::Changed
                }
                Some(current) => {
                    git_visibility(fs, desired)?;
                    if desired.method == LinkMethod::Copy
                        && content_hash(&read_source(fs, desired)?) == current
                    {
                        LinkStatus::Unchanged
                    } else {
                        remove_copy(fs, &desired.path)?;
                        let hash = create(fs, desired)?;
                        ledger.upsert(record(desired, hash, now));
                        LinkStatus::Updated
                    }
                }
            },
            None => LinkStatus::Foreign,
        },
        Entry::File => LinkStatus::Foreign,
    };
    Ok(result(desired, status))
}

/// The content hash of an owned copy at `path` when it is as written:
/// same hash and no `.git` added inside (the hash leaves `.git` out, and a
/// repository the user made there must never be deleted). `None` when it
/// changed.
fn unchanged_copy(fs: &dyn Fs, path: &Path, link: &SkillLink) -> Result<Option<String>, CoreError> {
    let current = content_hash(&read_folder(fs, path)?);
    if link.written_hash.as_deref() != Some(current.as_str()) || has_nested_git(fs, path)? {
        return Ok(None);
    }
    Ok(Some(current))
}

/// Makes a project entry's visibility to the project's repository match
/// its method before anything is written: a link (`share = false`) is
/// listed in `info/exclude`; a copy (`share = true`, meant to be
/// committed) is not, so an exclude line left from a link is dropped.
/// Nothing for user scope or a folder without a repository.
fn git_visibility(fs: &dyn Fs, desired: &Desired) -> Result<(), CoreError> {
    let Some(root) = &desired.project_root else {
        return Ok(());
    };
    let Some(repo) = ProjectRepo::discover(root)? else {
        return Ok(());
    };
    if desired.method.is_link() {
        repo.exclude(fs, &desired.path)?;
        return Ok(());
    }
    let Some(relative) = repo.relative(&desired.path) else {
        return Ok(());
    };
    let pattern = format!("/{relative}");
    let file = repo.exclude_file();
    if let Some(text) = crate::layering::projects::read_optional(fs, &file)?
        && text
            .lines()
            .any(|line| line.trim() == pattern || line.trim() == relative)
    {
        let kept: String = text
            .lines()
            .filter(|line| line.trim() != pattern && line.trim() != relative)
            .map(|line| format!("{line}\n"))
            .collect();
        crate::layering::projects::write_file(fs, &file, kept.as_bytes())?;
    }
    Ok(())
}

fn record(desired: &Desired, written_hash: Option<String>, now: &str) -> SkillLink {
    SkillLink {
        kind: RECORD_KIND.to_owned(),
        client: desired.client.to_owned(),
        scope: desired.scope,
        project_id: desired.project_id.clone(),
        skill_id: desired.skill_id.clone(),
        path: desired.path.to_string_lossy().into_owned(),
        method: desired.method,
        target: desired.source.to_string_lossy().into_owned(),
        written_hash,
        written_at: Some(now.to_owned()),
        extra: serde_json::Map::new(),
    }
}

fn read_source(fs: &dyn Fs, desired: &Desired) -> Result<SkillFiles, CoreError> {
    if !fs.is_dir(&desired.source) {
        return Err(store::not_found(&desired.skill_id).with_path(&desired.source));
    }
    read_folder(fs, &desired.source)
}

/// Creates the entry; the content hash for a copy.
fn create(fs: &dyn Fs, desired: &Desired) -> Result<Option<String>, CoreError> {
    if !fs.is_dir(&desired.source) {
        return Err(store::not_found(&desired.skill_id).with_path(&desired.source));
    }
    if let Some(parent) = desired.path.parent() {
        confine(fs, parent)?;
        fs.create_dir_all(parent)
            .map_err(|err| io_error("cannot create directory", parent, &err))?;
    }
    confine(fs, &desired.path)?;
    confine(fs, &desired.source)?;
    match desired.method {
        LinkMethod::Junction => {
            make_junction(&desired.source, &desired.path)?;
            Ok(None)
        }
        LinkMethod::Symlink => {
            make_symlink(&desired.source, &desired.path)?;
            Ok(None)
        }
        LinkMethod::Copy => {
            let files = read_folder(fs, &desired.source)?;
            write_files(fs, &desired.path, &files)?;
            let marker = desired.path.join(MARKER_FILE);
            let text = format!(
                "Managed by {}: a copy of skill {:?}. Edits here show as changes and are never \
                 overwritten.\n",
                crate::brand::PRODUCT_NAME,
                desired.skill_id
            );
            fs.write(&marker, text.as_bytes())
                .map_err(|err| io_error("cannot write file", &marker, &err))?;
            Ok(Some(content_hash(&files)))
        }
    }
}

#[cfg(windows)]
fn make_junction(target: &Path, link: &Path) -> Result<(), CoreError> {
    junction::create(target, link).map_err(|err| io_error("cannot create junction", link, &err))
}

#[cfg(not(windows))]
fn make_junction(target: &Path, link: &Path) -> Result<(), CoreError> {
    make_symlink(target, link)
}

#[cfg(windows)]
fn make_symlink(target: &Path, link: &Path) -> Result<(), CoreError> {
    std::os::windows::fs::symlink_dir(target, link).map_err(|err| {
        io_error(
            "cannot create symbolic link (Windows needs Developer Mode or admin rights; \
             junctions need neither)",
            link,
            &err,
        )
    })
}

#[cfg(unix)]
fn make_symlink(target: &Path, link: &Path) -> Result<(), CoreError> {
    std::os::unix::fs::symlink(target, link)
        .map_err(|err| io_error("cannot create symbolic link", link, &err))
}

/// Removes a link only, never what it points to.
pub(crate) fn remove_link(fs: &dyn Fs, path: &Path) -> Result<(), CoreError> {
    confine(fs, path)?;
    let removed = if cfg!(windows) {
        // Junctions and directory symlinks are directories to Windows;
        // removing one removes the link alone.
        std::fs::remove_dir(path).or_else(|_| std::fs::remove_file(path))
    } else {
        std::fs::remove_file(path)
    };
    removed.map_err(|err| io_error("cannot remove link", path, &err))
}

fn remove_copy(fs: &dyn Fs, path: &Path) -> Result<(), CoreError> {
    fs.remove_dir_all(path)
        .map_err(|err| io_error("cannot remove folder", path, &err))
}

/// Removes an owned entry that is no longer desired, when unchanged.
fn retire(sx: &SkillsEnv<'_>, ledger: &mut SkillLedger, link: &SkillLink) -> LinkResult {
    let path = PathBuf::from(&link.path);
    let fs = sx.fs;
    let outcome = (|| -> Result<LinkStatus, CoreError> {
        let status = match entry(fs, &path)? {
            Entry::Missing => LinkStatus::Removed,
            Entry::Link(found)
                if link.method.is_link() && same_folder(fs, &found, Path::new(&link.target)) =>
            {
                remove_link(fs, &path)?;
                LinkStatus::Removed
            }
            Entry::Dir if link.method == LinkMethod::Copy => {
                if unchanged_copy(fs, &path, link)?.is_some() {
                    remove_copy(fs, &path)?;
                    LinkStatus::Removed
                } else {
                    LinkStatus::Changed
                }
            }
            _ => LinkStatus::Changed,
        };
        ledger.remove(&path);
        Ok(status)
    })();
    let (status, error) = match outcome {
        Ok(status) => (status, None),
        Err(err) => (LinkStatus::Blocked, Some(err)),
    };
    LinkResult {
        client: link.client.clone(),
        scope: link.scope,
        project_id: link.project_id.clone(),
        skill_id: link.skill_id.clone(),
        path,
        method: link.method,
        status,
        found_target: None,
        error,
    }
}

/// Enables (or disables) skill `id` for `clients` at user scope: one
/// commit of `skill-meta/<id>.toml`, then [`sync`]. Enabling with no
/// clients selects `all-detected`; disabling with none turns the skill off
/// everywhere. `method` changes how it is distributed.
pub fn set_enabled(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    id: &str,
    clients: &[String],
    enabled: bool,
    method: Option<DistributionMethod>,
) -> Result<(Option<CommitId>, SyncReport), CoreError> {
    ids::check(IdKind::Skill, id)?;
    for client in clients {
        if client_skills(client).is_none() {
            return Err(no_skills(client));
        }
    }
    let verb = if enabled { "enable" } else { "disable" };
    let target = if clients.is_empty() {
        if enabled {
            "all detected clients".to_owned()
        } else {
            "all clients".to_owned()
        }
    } else {
        clients.join(", ")
    };
    let operation = Operation::new(
        format!("skill.{verb}"),
        format!("skill: {verb} {id} for {target}"),
    )
    .entity(format!("skill/{id}"));
    let committed = store.transaction(operation, |tx| {
        if !store::exists(tx, id)? {
            return Err(store::not_found(id));
        }
        let mut meta = store::read_meta(tx, id)?.unwrap_or_else(|| SkillMeta::new(id));
        let enable = meta.enable.get_or_insert_with(SkillEnable::default);
        let current = enable.clients.take();
        enable.clients = next_selection(sx, current, clients, enabled);
        if let Some(method) = method {
            enable.method = Some(method);
        }
        if enable.clients.is_none() && enable.method.is_none() && enable.extra.is_empty() {
            meta.enable = None;
        }
        store::stage_meta(tx, &meta)
    })?;
    let report = sync(store, sx)?;
    Ok((committed.commit, report))
}

/// Sets how skill `id` reaches client folders at user scope (SPEC 12.2:
/// link or copy), leaving its clients as they are: one commit of
/// `skill-meta/<id>.toml` (`skill: distribute <id> as <method>`), `None`
/// when unchanged, then [`sync`].
pub fn set_method(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    id: &str,
    method: DistributionMethod,
) -> Result<(Option<CommitId>, SyncReport), CoreError> {
    ids::check(IdKind::Skill, id)?;
    let name = match method {
        DistributionMethod::Link => "link",
        DistributionMethod::Copy => "copy",
    };
    let operation = Operation::new("skill.method", format!("skill: distribute {id} as {name}"))
        .entity(format!("skill/{id}"));
    let committed = store.transaction(operation, |tx| {
        if !store::exists(tx, id)? {
            return Err(store::not_found(id));
        }
        let mut meta = store::read_meta(tx, id)?.unwrap_or_else(|| SkillMeta::new(id));
        meta.enable.get_or_insert_with(SkillEnable::default).method = Some(method);
        store::stage_meta(tx, &meta)
    })?;
    let report = sync(store, sx)?;
    Ok((committed.commit, report))
}

fn next_selection(
    sx: &SkillsEnv<'_>,
    current: Option<ClientSelection>,
    clients: &[String],
    enabled: bool,
) -> Option<ClientSelection> {
    if clients.is_empty() {
        return enabled.then_some(ClientSelection::AllDetected);
    }
    let mut list: BTreeSet<String> = match &current {
        Some(ClientSelection::List(ids)) => ids.iter().cloned().collect(),
        Some(ClientSelection::AllDetected) => {
            if enabled {
                return current;
            }
            let mut ignored = Vec::new();
            resolve_clients(sx, &ClientSelection::AllDetected, "", &mut ignored)
                .into_iter()
                .map(str::to_owned)
                .collect()
        }
        None => BTreeSet::new(),
    };
    for client in clients {
        if enabled {
            list.insert(client.clone());
        } else {
            list.remove(client);
        }
    }
    (!list.is_empty()).then(|| ClientSelection::List(list.into_iter().collect()))
}

/// Enables (or disables) skill `id` in a project (SPEC 12.3): edits
/// `[skills] enable` of the project's private manifest (the shared one when
/// there is no private one), commits the store mirror, trusts the edited
/// content (Loadout wrote it), then [`sync`]s. The project must be known on
/// this machine and trusted as it is. `expected` is the manifests' content
/// hash the caller showed (the app); the manifests are re-read right
/// before the write, and content other than what was trusted (and shown)
/// is `trust_required`, with nothing written. Disabling a skill the
/// shared `project.toml` enables, in a project with a private file, is
/// `invalid_value` naming that file.
pub fn set_project_enabled(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    folder: &Path,
    id: &str,
    enabled: bool,
    expected: Option<&str>,
) -> Result<(Option<CommitId>, SyncReport), CoreError> {
    ids::check(IdKind::Skill, id)?;
    let fs = sx.fs;
    let state = sx.home.state_dir();
    let overlay = this_overlay(store, fs, &state)?;
    let trust = TrustStore::load(fs, &state)?;
    let known = known_projects(fs, sx.env, overlay.as_ref(), &trust)
        .into_iter()
        .find(|known| same_folder(fs, &known.root, folder))
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::NotFound,
                format!(
                    "the folder is not a project this machine knows; run `{} project add` first",
                    crate::brand::BIN_NAME
                ),
            )
            .with_path(folder)
        })?;
    let project = match &known.status {
        ProjectStatus::Loaded { project, .. } => {
            if let Some(err) = known.trust_error() {
                return Err(err);
            }
            project.as_ref().clone()
        }
        ProjectStatus::Missing => {
            return Err(
                CoreError::new(ErrorCode::NotFound, "the project folder does not exist")
                    .with_path(&known.root),
            );
        }
        ProjectStatus::Failed(err) => return Err(err.clone()),
    };
    if enabled {
        let has_content = store::get(store, id)?.is_some_and(|skill| skill.has_content);
        if !has_content {
            return Err(store::not_found(id));
        }
    }
    let file = project
        .local
        .as_ref()
        .or(project.shared.as_ref())
        .expect("a loaded project has a manifest");
    // Disabling in the private file leaves a skill the shared file enables
    // on: say so instead of reporting a change that did not happen.
    if !enabled
        && project.local.is_some()
        && let Some(shared) = &project.shared
        && manifest_enables(&shared.manifest, id)
    {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!(
                "skill {id:?} is enabled by the shared project.toml, which teammates use too; \
                 remove it from [skills] enable there"
            ),
        )
        .with_path(&shared.path));
    }
    let before = file.bytes.clone();
    let path = file.path.clone();
    let text = String::from_utf8(before.clone()).map_err(|_| {
        CoreError::new(ErrorCode::ParseError, "the manifest is not UTF-8").with_path(&path)
    })?;
    let Some(edited) = edit_manifest(&text, id, enabled, &path)? else {
        let report = sync(store, sx)?;
        return Ok((None, report));
    };
    // The edit is trusted below: both files must still be what was
    // trusted and read (a `git pull` in between needs a review first).
    let hash = project.hash();
    if manifests_hash(fs, &project.root)? != hash || expected.is_some_and(|shown| shown != hash) {
        return Err(CoreError::new(
            ErrorCode::TrustRequired,
            format!(
                "the manifests of project {:?} changed while being edited; review them, then \
                 run `{} project trust {}`",
                project.id,
                crate::brand::BIN_NAME,
                project.root.display()
            ),
        )
        .with_path(crate::layering::manifest::manifest_dir(&project.root)));
    }
    crate::layering::projects::write_file(fs, &path, edited.as_bytes())?;
    let verb = if enabled { "enable" } else { "disable" };
    let operation = Operation::new(
        format!("skill.project-{verb}"),
        format!("skill: {verb} {id} in project {}", project.id),
    )
    .entity(format!("skill/{id}"))
    .entity(format!("project/{}", project.id))
    .scope(store_mod::Scope::Project(project.id.clone()));
    let previous = trust.clone();
    let committed = (|| {
        let reloaded = crate::layering::projects::load_project(fs, &project.root)?;
        store.transaction(operation, |tx| {
            mirror(tx, &reloaded)?;
            let mut trust = TrustStore::load(fs, &state)?;
            trust.trust(&reloaded);
            trust.save(fs, &state)
        })
    })();
    let committed = match committed {
        Ok(committed) => committed,
        Err(err) => {
            let _ = crate::layering::projects::write_file(fs, &path, &before);
            let _ = previous.save(fs, &state);
            return Err(err);
        }
    };
    let report = sync(store, sx)?;
    Ok((committed.commit, report))
}

/// Whether `[skills] enable` of `manifest` lists `id`.
pub fn manifest_enables(manifest: &ProjectManifest, id: &str) -> bool {
    manifest
        .skills
        .as_ref()
        .and_then(|skills| skills.enable.as_ref())
        .is_some_and(|enable| enable.iter().any(|known| known == id))
}

/// [`crate::layering::content_hash`] of the manifests in `root` as they are
/// on disk now.
fn manifests_hash(fs: &dyn Fs, root: &Path) -> Result<String, CoreError> {
    let dir = crate::layering::manifest::manifest_dir(root);
    let read = |name: &str| -> Result<Option<Vec<u8>>, CoreError> {
        let path = dir.join(name);
        if !fs.exists(&path) {
            return Ok(None);
        }
        fs.read(&path)
            .map(Some)
            .map_err(|err| io_error("cannot read file", &path, &err))
    };
    let shared = read(crate::layering::SHARED_FILE)?;
    let local = read(crate::layering::LOCAL_FILE)?;
    Ok(crate::layering::content_hash(
        shared.as_deref(),
        local.as_deref(),
    ))
}

/// `[skills] enable` with `id` added or removed, comments kept; `None`
/// when nothing changes.
fn edit_manifest(
    text: &str,
    id: &str,
    enabled: bool,
    path: &Path,
) -> Result<Option<String>, CoreError> {
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|err| {
        CoreError::new(
            ErrorCode::ParseError,
            format!("the manifest does not parse: {err}"),
        )
        .with_path(path)
    })?;
    let shape = || {
        CoreError::new(
            ErrorCode::UnexpectedShape,
            "`skills` must be a table with an `enable` array",
        )
        .with_path(path)
    };
    if !doc.contains_key("skills") {
        if !enabled {
            return Ok(None);
        }
        doc["skills"] = toml_edit::table();
    }
    let skills = doc["skills"].as_table_like_mut().ok_or_else(shape)?;
    if skills.get("enable").is_none() {
        if !enabled {
            return Ok(None);
        }
        skills.insert("enable", toml_edit::value(toml_edit::Array::new()));
    }
    let array = skills
        .get_mut("enable")
        .and_then(toml_edit::Item::as_array_mut)
        .ok_or_else(shape)?;
    let position = array.iter().position(|value| value.as_str() == Some(id));
    match (enabled, position) {
        (true, None) => array.push(id),
        (false, Some(index)) => {
            array.remove(index);
        }
        _ => return Ok(None),
    }
    Ok(Some(doc.to_string()))
}

/// Every owned entry of skill `id` (for listings).
pub fn owned_entries<'a>(
    ledger: &'a SkillLedger,
    id: &'a str,
) -> impl Iterator<Item = &'a SkillLink> {
    ledger.links.iter().filter(move |link| link.skill_id == id)
}

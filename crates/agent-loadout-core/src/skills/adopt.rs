//! Adopt and bulk import of unmanaged skill folders (SPEC 8.6, 12.5, #3).
//!
//! [`scan`] reads the client skill folders of one scope (every client with
//! skill folders, or one) and groups their unmanaged folders by name: the
//! same content in several clients is one [`Variant`] with several folders;
//! different contents of one name are a conflict. The store skill of that
//! id, when there is one, is a variant too.
//!
//! [`adopt`] applies the user's choices in one store commit: each chosen
//! content becomes (or stays) a store skill, enabled where it was found
//! (user scope: `enable.clients`; project scope: the manifest's
//! `[skills] enable`). Then each folder whose content equals its store copy
//! is replaced by the managed entry (a junction on Windows, a symbolic link
//! elsewhere, or a copy) and moved to `backups/trash/`, never deleted; its
//! ledger record says `origin = adopted`. A folder with other content, or
//! one that changed in between, stays as it is and is reported.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::error::{CoreError, ErrorCode};
use crate::layering::{Project, ProjectRepo, same_path};
use crate::model::applied::Scope;
use crate::model::clients::ClientSelection;
use crate::model::ids::{self, IdKind};
use crate::model::skill_meta::{SkillEnable, SkillMeta, SkillSource, SkillSourceKind};
use crate::skills::classify::{self, EntryClass};
use crate::skills::link::{self, CLIENT_SKILLS, Replacement, SyncReport};
use crate::skills::{
    SKILL_FILE, SkillFiles, SkillsEnv, content_hash, files_size, frontmatter, read_folder, store,
};
use crate::store::{self as store_mod, CommitId, Operation, Store, Transaction};

/// An unmanaged folder in a client's skill folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundFolder {
    pub client: &'static str,
    pub path: PathBuf,
}

/// One content of a skill name: the folders holding it, and whether the
/// store skill of that id holds it too.
#[derive(Debug, Clone)]
pub struct Variant {
    /// [`content_hash`] of the files: names the variant in a choice.
    pub hash: String,
    /// In client order; empty for the store's copy alone.
    pub folders: Vec<FoundFolder>,
    /// The store skill of the candidate's name holds this content.
    pub in_store: bool,
    /// `description` from its `SKILL.md`.
    pub description: Option<String>,
    pub bytes: u64,
    files: SkillFiles,
}

impl Variant {
    /// The clients whose folders hold this content.
    pub fn clients(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.folders.iter().map(|folder| folder.client)
    }
}

/// Unmanaged folders of one name.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// The folder name, which is the store id it is adopted as.
    pub name: String,
    pub variants: Vec<Variant>,
}

impl Candidate {
    /// Several contents (the store's included): the user picks one, or
    /// imports the others as `name-<client>`.
    pub fn is_conflict(&self) -> bool {
        self.variants.len() > 1
    }

    pub fn variant(&self, hash: &str) -> Option<&Variant> {
        self.variants.iter().find(|variant| variant.hash == hash)
    }
}

/// An unmanaged folder that cannot be adopted (no `SKILL.md`, a name that
/// is not a skill id, unreadable content).
#[derive(Debug, Clone)]
pub struct FolderProblem {
    pub client: &'static str,
    pub name: String,
    pub path: PathBuf,
    pub error: CoreError,
}

/// The unmanaged folders of one scope.
#[derive(Debug, Clone)]
pub struct UnmanagedScan {
    pub scope: Scope,
    /// By name.
    pub candidates: Vec<Candidate>,
    pub problems: Vec<FolderProblem>,
}

impl UnmanagedScan {
    pub fn candidate(&self, name: &str) -> Option<&Candidate> {
        self.candidates
            .iter()
            .find(|candidate| candidate.name == name)
    }
}

/// What to do with the folders of one name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdoptAction {
    /// Adopt the content `variant` (a [`Variant::hash`]) as `name`; folders
    /// with other content stay as they are.
    Pick {
        variant: String,
    },
    /// Adopt `primary` as `name` and every other content as
    /// `name-<client>`, `<client>` being the first client holding it.
    Both {
        primary: String,
    },
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptChoice {
    pub name: String,
    pub action: AdoptAction,
}

/// What happened to one unmanaged folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderStatus {
    /// Replaced by the managed entry; the folder is in the trash.
    Adopted,
    /// Another content of its name was chosen; the folder is kept.
    Differs,
    /// Its name has several contents and no choice was given; kept.
    Conflict,
    /// The user skipped its name.
    Skipped,
    /// Its content is in the store, but the folder could not be replaced
    /// (see the error): kept.
    Kept,
}

impl FolderStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            FolderStatus::Adopted => "adopted",
            FolderStatus::Differs => "differs",
            FolderStatus::Conflict => "conflict",
            FolderStatus::Skipped => "skipped",
            FolderStatus::Kept => "kept",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FolderResult {
    pub client: &'static str,
    pub name: String,
    pub path: PathBuf,
    /// The store skill it was adopted into (or would have been).
    pub skill_id: Option<String>,
    pub status: FolderStatus,
    /// Where the folder went.
    pub trash: Option<PathBuf>,
    pub error: Option<CoreError>,
}

/// What [`adopt`] did.
#[derive(Debug, Clone, Default)]
pub struct AdoptReport {
    pub commit: Option<CommitId>,
    /// Store skills the action added.
    pub added: Vec<String>,
    /// Store skills that held the content already.
    pub reused: Vec<String>,
    pub folders: Vec<FolderResult>,
    /// Folders the scan could not offer.
    pub problems: Vec<FolderProblem>,
    /// Client folders after the change.
    pub sync: SyncReport,
}

impl AdoptReport {
    /// A folder is left for the user (a conflict, one that could not be
    /// replaced) or an entry needs a decision.
    pub fn needs_attention(&self) -> bool {
        self.sync.needs_attention()
            || self
                .folders
                .iter()
                .any(|folder| matches!(folder.status, FolderStatus::Conflict | FolderStatus::Kept))
    }
}

/// The unmanaged folders of `scope` (`project` for the project scope),
/// grouped by name, every client's or `client`'s only. Reads only.
pub fn scan(
    store: Option<&Store<'_>>,
    sx: &SkillsEnv<'_>,
    scope: Scope,
    project: Option<&Path>,
    client: Option<&str>,
) -> Result<UnmanagedScan, CoreError> {
    if let Some(client) = client {
        link::skills_dir(sx.env, client, scope, project)?;
    }
    let mut groups: BTreeMap<String, Vec<(FoundFolder, SkillFiles)>> = BTreeMap::new();
    let mut problems = Vec::new();
    // A project's repository: folders it tracks belong to the team.
    let repo = match (scope, project) {
        (Scope::Project | Scope::ProjectLocal, Some(root)) => ProjectRepo::discover(root)?,
        _ => None,
    };
    for dir in classify::scan(sx, scope, project)? {
        if client.is_some_and(|client| client != dir.client) {
            continue;
        }
        for entry in dir.entries {
            if entry.class != EntryClass::Unmanaged || !sx.fs.is_dir(&entry.path) {
                continue;
            }
            let problem = |error: CoreError| FolderProblem {
                client: dir.client,
                name: entry.name.clone(),
                path: entry.path.clone(),
                error,
            };
            if !ids::is_valid(IdKind::Skill, &entry.name) {
                problems.push(problem(
                    CoreError::new(
                        ErrorCode::InvalidId,
                        format!(
                            "{:?} is not a skill id (lower-case letters, digits and `-`), so \
                             the folder cannot be replaced by a link of that name; import it \
                             under another id instead",
                            entry.name
                        ),
                    )
                    .with_path(&entry.path),
                ));
                continue;
            }
            if let Some(repo) = &repo
                && repo.tracks_any_under(&entry.path)?
            {
                problems.push(problem(
                    CoreError::new(
                        ErrorCode::InvalidValue,
                        "the project's repository tracks files in this folder (teammates get it \
                         from there), so it is not adopted",
                    )
                    .with_path(&entry.path),
                ));
                continue;
            }
            match read_folder(sx.fs, &entry.path) {
                Ok(files) if files.contains_key(SKILL_FILE) => {
                    groups.entry(entry.name.clone()).or_default().push((
                        FoundFolder {
                            client: dir.client,
                            path: entry.path.clone(),
                        },
                        files,
                    ));
                }
                Ok(_) => problems.push(problem(store::missing_skill_file(&entry.path))),
                Err(err) => problems.push(problem(err)),
            }
        }
    }
    let mut candidates = Vec::new();
    for (name, found) in groups {
        let mut variants: Vec<Variant> = Vec::new();
        for (folder, files) in found {
            let hash = content_hash(&files);
            match variants.iter_mut().find(|variant| variant.hash == hash) {
                Some(variant) => variant.folders.push(folder),
                None => variants.push(variant(hash, vec![folder], files)),
            }
        }
        if let Some(store) = store
            && let Some(skill) = store::get(store, &name)?
        {
            if !skill.has_content {
                // Metadata without content: the id is taken, and there is
                // nothing to compare with.
                for variant in variants {
                    for folder in variant.folders {
                        problems.push(FolderProblem {
                            client: folder.client,
                            name: name.clone(),
                            error: CoreError::new(
                                ErrorCode::AlreadyExists,
                                format!(
                                    "skill {name:?} is in the store without content; remove it \
                                     or import the folder under another id"
                                ),
                            )
                            .with_path(&folder.path),
                            path: folder.path,
                        });
                    }
                }
                continue;
            }
            let files = store::read_content(store, &name)?;
            let hash = content_hash(&files);
            match variants.iter_mut().find(|variant| variant.hash == hash) {
                Some(variant) => variant.in_store = true,
                None => {
                    let mut stored = variant(hash, Vec::new(), files);
                    stored.in_store = true;
                    variants.push(stored);
                }
            }
        }
        candidates.push(Candidate { name, variants });
    }
    Ok(UnmanagedScan {
        scope,
        candidates,
        problems,
    })
}

fn variant(hash: String, folders: Vec<FoundFolder>, files: SkillFiles) -> Variant {
    Variant {
        hash,
        folders,
        in_store: false,
        description: description(&files),
        bytes: files_size(&files),
        files,
    }
}

fn description(files: &SkillFiles) -> Option<String> {
    let text = String::from_utf8_lossy(files.get(SKILL_FILE)?);
    frontmatter::parse(&text)
        .ok()
        .flatten()
        .and_then(|fm| fm.description().map(str::to_owned))
}

/// Adopt every name with one content; conflicts are left for a choice.
pub fn default_choices(scan: &UnmanagedScan) -> Vec<AdoptChoice> {
    scan.candidates
        .iter()
        .filter(|candidate| !candidate.is_conflict())
        .map(|candidate| AdoptChoice {
            name: candidate.name.clone(),
            action: AdoptAction::Pick {
                variant: candidate.variants[0].hash.clone(),
            },
        })
        .collect()
}

/// One store skill the action adopts into.
struct Wanted<'a> {
    /// The id asked for: the name, or `name-<client>`.
    id: String,
    /// `name-<client>`: another free id is taken when it holds other
    /// content.
    renamed: bool,
    variant: &'a Variant,
    /// The frontmatter name, else the folder name.
    display_name: String,
}

/// Applies `choices` to the unmanaged folders of `scope` as they are now
/// (scanned again; a choice naming a content no folder has any more is
/// `entry_changed`): one store commit (`skill: adopt ...`), then the
/// folders are replaced and client folders synced. Names without a choice
/// are skipped; a conflict without one is reported as such. In project
/// scope the project must be trusted: the skills go to its manifest's
/// `[skills] enable`.
pub fn adopt(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    scope: Scope,
    project: Option<&Path>,
    client: Option<&str>,
    choices: &[AdoptChoice],
) -> Result<AdoptReport, CoreError> {
    let project = match scope {
        Scope::User => None,
        Scope::Project | Scope::ProjectLocal => {
            let folder = project.ok_or_else(|| {
                CoreError::new(
                    ErrorCode::InvalidPath,
                    "a project scope needs an absolute folder",
                )
            })?;
            Some(link::trusted_project(store, sx, folder)?)
        }
    };
    let root = project.as_ref().map(|project| project.root.as_path());
    let scan = scan(Some(store), sx, scope, root, client)?;
    for choice in choices {
        if choice.action != AdoptAction::Skip && scan.candidate(&choice.name).is_none() {
            return Err(changed(format!(
                "there is no unmanaged skill folder {:?} any more",
                choice.name
            )));
        }
    }

    let mut report = AdoptReport {
        problems: scan.problems.clone(),
        ..AdoptReport::default()
    };
    let mut wanted: Vec<Wanted<'_>> = Vec::new();
    for candidate in &scan.candidates {
        let action = choices
            .iter()
            .find(|choice| choice.name == candidate.name)
            .map(|choice| &choice.action);
        let (primary, both) = match action {
            Some(AdoptAction::Pick { variant }) => (variant, false),
            Some(AdoptAction::Both { primary }) => (primary, true),
            None | Some(AdoptAction::Skip) => {
                let status = if action.is_none() && candidate.is_conflict() {
                    FolderStatus::Conflict
                } else {
                    FolderStatus::Skipped
                };
                for variant in &candidate.variants {
                    report.folders.extend(
                        variant
                            .folders
                            .iter()
                            .map(|folder| result(candidate, folder, None, status)),
                    );
                }
                continue;
            }
        };
        let chosen = candidate.variant(primary).ok_or_else(|| {
            changed(format!(
                "the chosen content of {:?} is in no folder any more",
                candidate.name
            ))
        })?;
        if candidate
            .variants
            .iter()
            .any(|variant| variant.in_store && variant.hash != chosen.hash)
        {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!(
                    "skill {:?} is in the store with other content; pick the store's copy, or \
                     keep it and import the folders as {:?}-<client>",
                    candidate.name, candidate.name
                ),
            ));
        }
        // The store's copy alone has no folder to hand over.
        if !chosen.folders.is_empty() {
            wanted.push(Wanted {
                id: candidate.name.clone(),
                renamed: false,
                variant: chosen,
                display_name: display_name(&candidate.name, chosen),
            });
        }
        for other in candidate
            .variants
            .iter()
            .filter(|variant| variant.hash != chosen.hash)
        {
            match other.folders.first() {
                Some(first) if both => {
                    let id = format!("{}-{}", candidate.name, first.client);
                    ids::check(IdKind::Skill, &id)?;
                    wanted.push(Wanted {
                        id,
                        renamed: true,
                        variant: other,
                        display_name: display_name(&candidate.name, other),
                    });
                }
                _ => report.folders.extend(
                    other
                        .folders
                        .iter()
                        .map(|folder| result(candidate, folder, None, FolderStatus::Differs)),
                ),
            }
        }
    }
    if wanted.is_empty() {
        sort_folders(&mut report.folders);
        return Ok(report);
    }

    // Final ids: a `name-<client>` taken by other content gets `-2`, `-3`.
    let mut resolved: Vec<(String, bool)> = Vec::new();
    for item in &wanted {
        resolved.push(resolve_id(store, sx.fs, item, &resolved)?);
    }
    let ids: Vec<&str> = resolved.iter().map(|(id, _)| id.as_str()).collect();
    let mut operation = Operation::new("skill.adopt", summary(&ids, project.as_ref()));
    for id in &ids {
        operation = operation.entity(format!("skill/{id}"));
    }
    let staging = |tx: &mut Transaction<'_>| stage(tx, scope, &wanted, &resolved);
    let committed = match &project {
        None => store.transaction(operation, staging)?,
        Some(project) => {
            let (mut text, path) = link::skills_file_text(project)?;
            for id in &ids {
                if let Some(edited) = link::edit_manifest(&text, id, true, &path)? {
                    text = edited;
                }
            }
            let operation = operation
                .entity(format!("project/{}", project.id))
                .scope(store_mod::Scope::Project(project.id.clone()));
            link::commit_project_edit(store, sx, project, &text, None, operation, staging)?
        }
    };
    report.commit = committed.commit;
    for (id, new) in &resolved {
        if *new {
            report.added.push(id.clone());
        } else if !report.reused.contains(id) {
            report.reused.push(id.clone());
        }
    }

    // Replace the folders, then bring every client folder in line.
    // One replacement per folder: omp and Pi share a folder when
    // `PI_CODING_AGENT_DIR` is set, and it is listed for both.
    let mut replacements: Vec<Replacement> = Vec::new();
    let mut aliases: Vec<(&'static str, usize)> = Vec::new();
    for (item, (id, _)) in wanted.iter().zip(&resolved) {
        for folder in &item.variant.folders {
            if let Some(first) = replacements
                .iter()
                .position(|known| same_path(&known.folder, &folder.path))
            {
                aliases.push((folder.client, first));
                continue;
            }
            let dir = link::skills_dir(sx.env, folder.client, scope, root)?;
            replacements.push(Replacement {
                client: folder.client,
                folder: folder.path.clone(),
                entry: dir.join(id),
                skill_id: id.clone(),
            });
        }
    }
    let replaced = link::replace_folders(store, sx, &replacements)?;
    let mut results: Vec<FolderResult> = Vec::new();
    for (replacement, outcome) in replacements.iter().zip(replaced) {
        let name = replacement
            .folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (status, trash, error) = match outcome {
            Ok(trash) => (FolderStatus::Adopted, Some(trash), None),
            Err(err) => (FolderStatus::Kept, None, Some(err)),
        };
        results.push(FolderResult {
            client: replacement.client,
            name,
            path: replacement.folder.clone(),
            skill_id: Some(replacement.skill_id.clone()),
            status,
            trash,
            error,
        });
    }
    // A shared folder reports the same outcome for each client.
    for (client, first) in aliases {
        let shared = FolderResult {
            client,
            ..results[first].clone()
        };
        results.push(shared);
    }
    report.folders.extend(results);
    report.sync = link::sync(store, sx)?;
    sort_folders(&mut report.folders);
    Ok(report)
}

/// By name, then in client order.
fn sort_folders(folders: &mut [FolderResult]) {
    let order = |client: &str| {
        CLIENT_SKILLS
            .iter()
            .position(|entry| entry.client == client)
    };
    folders.sort_by(|a, b| {
        (a.name.as_str(), order(a.client)).cmp(&(b.name.as_str(), order(b.client)))
    });
}

fn changed(why: String) -> CoreError {
    CoreError::new(
        ErrorCode::EntryChanged,
        format!("{why}; scan the skill folders again"),
    )
}

fn result(
    candidate: &Candidate,
    folder: &FoundFolder,
    skill_id: Option<String>,
    status: FolderStatus,
) -> FolderResult {
    FolderResult {
        client: folder.client,
        name: candidate.name.clone(),
        path: folder.path.clone(),
        skill_id,
        status,
        trash: None,
        error: None,
    }
}

fn display_name(name: &str, variant: &Variant) -> String {
    variant
        .files
        .get(SKILL_FILE)
        .and_then(|text| {
            frontmatter::parse(&String::from_utf8_lossy(text))
                .ok()
                .flatten()
        })
        .and_then(|fm| fm.name().map(str::to_owned))
        .unwrap_or_else(|| name.to_owned())
}

/// The id `item` is stored under and whether its content is new to the
/// store. `earlier` are the ids resolved before it in this action. A
/// `name-<client>` taken in the store by other content, or by any entry
/// next to a folder of the variant (where its link goes), gets `-2`, `-3`.
fn resolve_id(
    store: &Store<'_>,
    fs: &dyn crate::fs::Fs,
    item: &Wanted<'_>,
    earlier: &[(String, bool)],
) -> Result<(String, bool), CoreError> {
    if item.variant.in_store {
        return Ok((item.id.clone(), false));
    }
    let taken = |id: &str| earlier.iter().any(|(known, _)| known == id);
    let same = |id: &str| -> Result<Option<bool>, CoreError> {
        Ok(match store::get(store, id)? {
            None => None,
            Some(skill) => Some(
                skill.has_content
                    && content_hash(&store::read_content(store, id)?) == item.variant.hash,
            ),
        })
    };
    if !item.renamed {
        // The scan saw no store skill of this name.
        return match same(&item.id)? {
            None if !taken(&item.id) => Ok((item.id.clone(), true)),
            _ => Err(changed(format!(
                "skill {:?} was added to the store meanwhile",
                item.id
            ))),
        };
    }
    for n in 1..1000 {
        let id = if n == 1 {
            item.id.clone()
        } else {
            format!("{}-{n}", item.id)
        };
        if !ids::is_valid(IdKind::Skill, &id) || taken(&id) {
            continue;
        }
        let mut on_disk = false;
        for folder in &item.variant.folders {
            if let Some(dir) = folder.path.parent() {
                on_disk |=
                    crate::skills::entry(fs, &dir.join(&id))? != crate::skills::Entry::Missing;
            }
        }
        if on_disk {
            continue;
        }
        match same(&id)? {
            None => return Ok((id, true)),
            Some(true) => return Ok((id, false)),
            Some(false) => {}
        }
    }
    Err(CoreError::new(
        ErrorCode::AlreadyExists,
        format!("no free id for skill {:?}", item.id),
    ))
}

fn summary(ids: &[&str], project: Option<&Project>) -> String {
    let what = if ids.len() <= 3 {
        ids.join(", ")
    } else {
        format!("{} skills", ids.len())
    };
    match project {
        Some(project) => format!("skill: adopt {what} in project {}", project.id),
        None => format!("skill: adopt {what}"),
    }
}

/// Stages the new content and the metadata of every adopted skill; at
/// user scope each is enabled for the clients where it was found.
fn stage(
    tx: &mut Transaction<'_>,
    scope: Scope,
    wanted: &[Wanted<'_>],
    resolved: &[(String, bool)],
) -> Result<(), CoreError> {
    for (item, (id, new)) in wanted.iter().zip(resolved) {
        let mut meta = if *new {
            if store::exists(tx, id)? {
                return Err(changed(format!(
                    "skill {id:?} was added to the store meanwhile"
                )));
            }
            store::stage_content(tx, id, &item.variant.files)?;
            let mut meta = SkillMeta::new(id);
            meta.name = Some(item.display_name.clone());
            meta.source = Some(SkillSource {
                kind: SkillSourceKind::Local,
                url: None,
                path: None,
                git_ref: None,
                commit: None,
                extra: toml::Table::new(),
            });
            meta
        } else {
            store::read_meta(tx, id)?.unwrap_or_else(|| SkillMeta::new(id))
        };
        if scope == Scope::User {
            let enable = meta.enable.get_or_insert_with(SkillEnable::default);
            enable.clients = with_clients(enable.clients.take(), item.variant.clients());
        }
        store::stage_meta(tx, &meta)?;
    }
    Ok(())
}

/// `current` with `clients` added; `all-detected` stays as it is.
fn with_clients<'a>(
    current: Option<ClientSelection>,
    clients: impl Iterator<Item = &'a str>,
) -> Option<ClientSelection> {
    let mut list: BTreeSet<String> = match current {
        Some(ClientSelection::AllDetected) => return Some(ClientSelection::AllDetected),
        Some(ClientSelection::List(ids)) => ids.into_iter().collect(),
        None => BTreeSet::new(),
    };
    list.extend(clients.map(str::to_owned));
    Some(ClientSelection::List(list.into_iter().collect()))
}

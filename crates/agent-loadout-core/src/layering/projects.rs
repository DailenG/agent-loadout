//! Project operations shared by `loadout project` and the app (SPEC 5.5, 6,
//! 13.3, 17): init, add, remove, the trust review and trust.
//!
//! The project folder is the source of truth: its manifests are read from
//! `<P>/.loadout/`. This machine's overlay (`machines/<machine-id>.toml`
//! `[projects.<id>] path`) says which projects it knows, `state/trust.json`
//! which manifest contents it trusts, and the store mirror
//! `projects/<id>/` holds the content last trusted. Every change is one
//! store commit (`Loadout-Scope: project/<id>`); init, remove and trust
//! apply in that commit in instant mode.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::apply::{ApplyEnv, ApplyReport, Touched, instant_apply};
use crate::env::Env;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::layering::manifest::manifest_dir;
use crate::layering::{
    LOCAL_FILE, Project, ProjectRepo, SHARED_FILE, TrustStore, discover, same_folder, same_path,
};
use crate::model::TomlDocument;
use crate::model::ids::{self, IdKind};
use crate::model::machine::{MachineOverlay, MachineProject};
use crate::model::project::ProjectManifest;
use crate::secrets::catalog::overlay_path;
use crate::store::{CommitId, Machine, Operation, Scope, Store, StoreRead, Transaction};

/// `.loadout/.gitignore` of a shared project: the private file stays out
/// of the project's repository.
pub const GITIGNORE: &str = ".gitignore";

/// The manifest files present in a project folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Only `project.local.toml` (the default for new manifests).
    Private,
    /// Only `project.toml`.
    Shared,
    /// Both files.
    SharedAndLocal,
}

impl Layout {
    pub const fn as_str(self) -> &'static str {
        match self {
            Layout::Private => "private",
            Layout::Shared => "shared",
            Layout::SharedAndLocal => "shared+local",
        }
    }
}

/// The manifest files in `root`, if any.
pub fn layout(fs: &dyn Fs, root: &Path) -> Option<Layout> {
    let dir = manifest_dir(root);
    match (
        fs.exists(&dir.join(SHARED_FILE)),
        fs.exists(&dir.join(LOCAL_FILE)),
    ) {
        (true, true) => Some(Layout::SharedAndLocal),
        (true, false) => Some(Layout::Shared),
        (false, true) => Some(Layout::Private),
        (false, false) => None,
    }
}

/// A checked `init`: the folder, the new project id and the manifest text.
#[derive(Debug, Clone)]
pub struct InitPlan {
    pub root: PathBuf,
    pub id: String,
    pub(crate) text: String,
}

/// What `init` created.
#[derive(Debug)]
pub struct Initialized {
    pub id: String,
    /// The manifest file written.
    pub file: PathBuf,
    /// The repository's `info/exclude` file, when a private manifest was
    /// hidden through it.
    pub excluded: Option<PathBuf>,
    pub report: Option<ApplyReport>,
    pub commit: Option<CommitId>,
}

/// Checks that `root` can get a new manifest and prepares it: not the home
/// folder, an existing folder, no manifest yet. Touches nothing.
pub fn prepare_init(
    env: &Env,
    fs: &dyn Fs,
    root: &Path,
    name: Option<&str>,
) -> Result<InitPlan, CoreError> {
    if same_folder(fs, root, env.home()) {
        return Err(CoreError::new(
            ErrorCode::InvalidPath,
            "a manifest in the home folder would apply to every folder; pick a project folder",
        )
        .with_path(root));
    }
    if !fs.is_dir(root) {
        return Err(
            CoreError::new(ErrorCode::NotFound, "the project folder does not exist")
                .with_path(root),
        );
    }
    let dir = manifest_dir(root);
    for file in [SHARED_FILE, LOCAL_FILE] {
        let existing = dir.join(file);
        if fs.exists(&existing) {
            return Err(CoreError::new(
                ErrorCode::AlreadyExists,
                "this folder already has a project manifest; use `project add` or `project trust`",
            )
            .with_path(existing));
        }
    }
    let folder_name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let id = new_project_id(env, &folder_name)?;
    let mut manifest = ProjectManifest::new(Some(id.clone()));
    manifest.name = Some(
        name.map(str::to_owned)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| {
                if folder_name.is_empty() {
                    id.clone()
                } else {
                    folder_name.clone()
                }
            }),
    );
    let text = TomlDocument::new(manifest).to_text()?;
    Ok(InitPlan {
        root: root.to_path_buf(),
        id,
        text,
    })
}

/// Writes the manifest of `plan` (private `project.local.toml`, or the
/// shared `project.toml` with a `.gitignore` for the private file), hides a
/// private manifest from the project's repository, and records, mirrors
/// and trusts the project in one commit that applies it. When the commit
/// fails, the files written are removed again.
pub fn init(
    store: &Store<'_>,
    ax: &ApplyEnv<'_>,
    plan: InitPlan,
    shared: bool,
) -> Result<Initialized, CoreError> {
    let fs = ax.fs;
    let InitPlan { root, id, text } = plan;
    let dir = manifest_dir(&root);
    let dir_existed = fs.is_dir(&dir);
    let file = dir.join(if shared { SHARED_FILE } else { LOCAL_FILE });
    let mut created = vec![file.clone()];
    create_dir(fs, &dir)?;
    write_file(fs, &file, text.as_bytes())?;
    let mut excluded = None;
    let staged = (|| {
        if shared {
            let ignore = dir.join(GITIGNORE);
            if !fs.exists(&ignore) {
                created.push(ignore.clone());
            }
            ensure_line(fs, &ignore, LOCAL_FILE)?;
        } else {
            excluded = exclude_manifest_dir(fs, &root)?;
        }
        let project = load_project(fs, &root)?;
        let summary = format!("project: init {id}");
        let touched = Touched::Project(root.clone());
        commit_project(
            store,
            ax,
            "project.init",
            summary,
            &id,
            Some(touched),
            |tx| {
                set_known(tx, fs, &project.id, &root)?;
                mirror(tx, &project)?;
                save_trust(ax, &project)
            },
        )
    })();
    let (report, commit) = match staged {
        Ok(done) => done,
        Err(err) => {
            // Nothing was committed: leave the folder as it was.
            for path in &created {
                let _ = fs.remove_file(path);
            }
            if !dir_existed && fs.read_dir(&dir).is_ok_and(|entries| entries.is_empty()) {
                let _ = fs.remove_dir_all(&dir);
            }
            return Err(err);
        }
    };
    Ok(Initialized {
        id,
        file,
        excluded,
        report,
        commit,
    })
}

/// Records `project` as known at its folder on this machine (one commit,
/// no apply: it is applied once trusted).
pub fn add(
    store: &Store<'_>,
    ax: &ApplyEnv<'_>,
    project: &Project,
) -> Result<Option<CommitId>, CoreError> {
    let id = project.id.clone();
    let summary = format!("project: add {id}");
    let (_, commit) = commit_project(store, ax, "project.add", summary, &id, None, |tx| {
        set_known(tx, ax.fs, &id, &project.root)
    })?;
    Ok(commit)
}

/// What `remove` did.
#[derive(Debug)]
pub struct Removed {
    /// The folder the project was known at.
    pub root: Option<PathBuf>,
    pub report: Option<ApplyReport>,
    pub commit: Option<CommitId>,
}

/// Forgets project `id` on this machine (overlay and trust) in one commit
/// whose apply removes the entries Loadout owns in its files.
pub fn remove(store: &Store<'_>, ax: &ApplyEnv<'_>, id: &str) -> Result<Removed, CoreError> {
    ids::check(IdKind::Project, id)?;
    let summary = format!("project: remove {id}");
    let mut root = None;
    // The project is no longer known: cleaning up its files is a plan of
    // every project (owned entries of unknown projects).
    let touched = Some(Touched::All);
    let (report, commit) =
        commit_project(store, ax, "project.remove", summary, id, touched, |tx| {
            let mut doc = overlay_doc(tx)?;
            let projects = &mut doc.value_mut().projects;
            let known = projects.as_mut().and_then(|map| map.remove(id));
            let Some(known) = known else {
                return Err(unknown_project(id));
            };
            if projects.as_ref().is_some_and(BTreeMap::is_empty) {
                *projects = None;
            }
            root = Some(PathBuf::from(known.path));
            let relative = overlay_path(&tx.machine().id)?;
            tx.write(&relative, doc.to_text()?)?;
            let state = ax.home.state_dir();
            let mut trust = TrustStore::load(ax.fs, &state)?;
            if trust.forget(id) {
                trust.save(ax.fs, &state)?;
            }
            Ok(())
        })?;
    Ok(Removed {
        root,
        report,
        commit,
    })
}

/// How a manifest file compares with the content last trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestChange {
    New,
    Changed,
    Unchanged,
    Removed,
}

impl ManifestChange {
    pub const fn as_str(self) -> &'static str {
        match self {
            ManifestChange::New => "new",
            ManifestChange::Changed => "changed",
            ManifestChange::Unchanged => "unchanged",
            ManifestChange::Removed => "removed",
        }
    }
}

/// A manifest file as the trust review shows it.
#[derive(Debug, Clone)]
pub struct ManifestReview {
    /// `project.toml` or `project.local.toml`.
    pub file: &'static str,
    pub path: PathBuf,
    pub change: ManifestChange,
    /// The file now; `None` when it was removed.
    pub content: Option<String>,
    /// Unified diff from the trusted content (headers `trusted <file>` and
    /// [`ManifestReview::path`]), for a changed file.
    pub diff: Option<String>,
}

/// Each manifest file of `project` against the store mirror (the content
/// last trusted).
pub fn review(
    store: &dyn StoreRead,
    project: &Project,
    trust: &TrustStore,
) -> Result<Vec<ManifestReview>, CoreError> {
    // The mirror is synced: content trusted on another machine, or before
    // this machine forgot the project, was never shown here. Without a
    // trust record of its own, every file is reviewed in full (`new`).
    let trusted_here = trust.knows(project);
    let dir = manifest_dir(&project.root);
    let mut reviews = Vec::new();
    for (file, manifest) in [
        (SHARED_FILE, project.shared.as_ref()),
        (LOCAL_FILE, project.local.as_ref()),
    ] {
        let trusted = if trusted_here {
            store.read_file(&mirror_path(&project.id, file))?
        } else {
            None
        };
        let current = manifest.map(|m| String::from_utf8_lossy(&m.bytes).into_owned());
        let path = dir.join(file);
        let (change, diff) = match (&trusted, &current) {
            (None, None) => continue,
            (None, Some(_)) => (ManifestChange::New, None),
            (Some(_), None) => (ManifestChange::Removed, None),
            (Some(old), Some(new)) if old == new => (ManifestChange::Unchanged, None),
            (Some(old), Some(new)) => {
                let diff = similar::TextDiff::from_lines(old.as_str(), new.as_str())
                    .unified_diff()
                    .header(&format!("trusted {file}"), &path.display().to_string())
                    .to_string();
                (ManifestChange::Changed, Some(diff))
            }
        };
        reviews.push(ManifestReview {
            file,
            path,
            change,
            content: current,
            diff,
        });
    }
    Ok(reviews)
}

/// Trusts `project` as it is: records it at its folder, mirrors its
/// manifests into the store and applies it, in one commit. With
/// `expected`, the manifests must still have that content hash (the
/// content the user reviewed); otherwise `trust_required`.
pub fn trust(
    store: &Store<'_>,
    ax: &ApplyEnv<'_>,
    project: &Project,
    expected: Option<&str>,
) -> Result<(Option<ApplyReport>, Option<CommitId>), CoreError> {
    if let Some(expected) = expected
        && project.hash() != expected
    {
        return Err(CoreError::new(
            ErrorCode::TrustRequired,
            format!(
                "the manifests of project {:?} changed after the review; review them again",
                project.name()
            ),
        )
        .with_path(manifest_dir(&project.root)));
    }
    let id = project.id.clone();
    let summary = format!("project: trust {id}");
    let touched = Some(Touched::Project(project.root.clone()));
    commit_project(store, ax, "project.trust", summary, &id, touched, |tx| {
        set_known(tx, ax.fs, &id, &project.root)?;
        mirror(tx, project)?;
        save_trust(ax, project)
    })
}

/// Stages one project commit; with `apply`, the instant apply of what it
/// touched runs in it. `state/trust.json` reflects committed operations
/// only: when the transaction fails (staging, validation, the leak
/// scanner), the trust file as it was before is written back (a failure to
/// do so is added to the error's message).
pub fn commit_project(
    store: &Store<'_>,
    ax: &ApplyEnv<'_>,
    op: &str,
    summary: String,
    id: &str,
    apply: Option<Touched>,
    stage: impl FnOnce(&mut Transaction<'_>) -> Result<(), CoreError>,
) -> Result<(Option<ApplyReport>, Option<CommitId>), CoreError> {
    let operation = Operation::new(op, summary)
        .entity(format!("project/{id}"))
        .scope(Scope::Project(id.to_owned()));
    let state = ax.home.state_dir();
    let previous = TrustStore::load(ax.fs, &state).ok();
    let committed = store.transaction(operation, |tx| {
        stage(tx)?;
        match &apply {
            Some(touched) => instant_apply(tx, ax, touched),
            None => Ok(None),
        }
    });
    match committed {
        Ok(committed) => Ok((committed.value, committed.commit)),
        Err(err) => {
            if let Some(previous) = previous
                && let Err(restore) = previous.save(ax.fs, &state)
            {
                let message = format!(
                    "{}; also, the trust file cannot be restored: {restore}",
                    err.message()
                );
                let mut combined = CoreError::new(err.code(), message);
                if let Some(path) = err.path() {
                    combined = combined.with_path(path);
                }
                return Err(combined);
            }
            Err(err)
        }
    }
}

/// This machine's overlay, staged or new.
pub fn overlay_doc(tx: &Transaction<'_>) -> Result<TomlDocument<MachineOverlay>, CoreError> {
    let machine = tx.machine().clone();
    let relative = overlay_path(&machine.id)?;
    match tx.read_file(&relative)? {
        Some(text) => TomlDocument::parse(&text)
            .map_err(|diagnostic| diagnostic.to_core_error(Some(Path::new(&relative)))),
        None => Ok(TomlDocument::new(MachineOverlay::new(
            &machine.id,
            &machine.label,
            tx.store().env().os(),
        ))),
    }
}

/// Records `[projects.<id>] path` in this machine's overlay. Another id
/// recorded for the same folder (its manifest id changed) is dropped. When
/// `id` is known at another folder that still holds a manifest with this
/// id (a second checkout of the same project), the change is refused; a
/// folder that is gone or now holds another project is repointed (moved).
pub fn set_known(
    tx: &mut Transaction<'_>,
    fs: &dyn Fs,
    id: &str,
    root: &Path,
) -> Result<(), CoreError> {
    let mut doc = overlay_doc(tx)?;
    let projects = doc.value_mut().projects.get_or_insert_with(BTreeMap::new);
    if let Some(known) = projects.get(id) {
        let other = Path::new(&known.path);
        let checkout = !same_path(other, root)
            && matches!(Project::load(fs, other), Ok(Some(project)) if project.id == id);
        if checkout {
            return Err(CoreError::new(
                ErrorCode::AlreadyExists,
                format!(
                    "project {id:?} is already known at {}, which still holds its manifest; \
                     one machine applies a project from one folder. To use {} instead, run \
                     `{} project rm {id}` first",
                    other.display(),
                    root.display(),
                    crate::brand::BIN_NAME,
                ),
            )
            .with_path(root));
        }
    }
    projects.retain(|known, project| known == id || !same_path(Path::new(&project.path), root));
    let path = root.to_string_lossy().into_owned();
    projects
        .entry(id.to_owned())
        .and_modify(|project| project.path.clone_from(&path))
        .or_insert_with(|| MachineProject {
            path,
            extra: toml::Table::new(),
        });
    let relative = overlay_path(&tx.machine().id)?;
    tx.write(&relative, doc.to_text()?)
}

/// This machine's overlay from the store, when both exist.
pub fn this_overlay(
    store: &Store<'_>,
    fs: &dyn Fs,
    state_dir: &Path,
) -> Result<Option<MachineOverlay>, CoreError> {
    let Some(machine) = Machine::load(fs, state_dir)? else {
        return Ok(None);
    };
    let relative = overlay_path(&machine.id)?;
    let Some(text) = store.read_to_string(&relative)? else {
        return Ok(None);
    };
    TomlDocument::<MachineOverlay>::parse(&text)
        .map(|doc| Some(doc.into_value()))
        .map_err(|diagnostic| diagnostic.to_core_error(Some(Path::new(&relative))))
}

fn mirror_path(id: &str, file: &str) -> String {
    format!("projects/{id}/{file}")
}

/// Stages the store mirror `projects/<id>/` as exactly the folder's
/// manifests (the content being trusted).
pub fn mirror(tx: &mut Transaction<'_>, project: &Project) -> Result<(), CoreError> {
    for (file, manifest) in [
        (SHARED_FILE, project.shared.as_ref()),
        (LOCAL_FILE, project.local.as_ref()),
    ] {
        let relative = mirror_path(&project.id, file);
        match manifest {
            Some(manifest) => tx.write(&relative, manifest.bytes.clone())?,
            None if tx.exists(&relative)? => tx.remove(&relative)?,
            None => {}
        }
    }
    Ok(())
}

/// Records `project`'s content as trusted in `state/trust.json`.
pub fn save_trust(ax: &ApplyEnv<'_>, project: &Project) -> Result<(), CoreError> {
    let state = ax.home.state_dir();
    let mut trust = TrustStore::load(ax.fs, &state)?;
    trust.trust(project);
    trust.save(ax.fs, &state)
}

/// The project in `root`, or `not_found` when it has no manifest.
pub fn load_project(fs: &dyn Fs, root: &Path) -> Result<Project, CoreError> {
    Project::load(fs, root)?.ok_or_else(|| {
        CoreError::new(ErrorCode::NotFound, "the folder has no project manifest")
            .with_path(manifest_dir(root))
    })
}

/// The project folder of `folder` (absolute): the nearest folder at or
/// above it holding a manifest, spelled as this machine's overlay records
/// it.
pub fn project_root(
    store: Option<&Store<'_>>,
    ax: &ApplyEnv<'_>,
    folder: &Path,
) -> Result<PathBuf, CoreError> {
    let root = discover(ax.env, ax.fs, folder).ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotFound,
            "no project manifest (.loadout/project.toml or .loadout/project.local.toml) at or \
             above this folder",
        )
        .with_path(folder)
    })?;
    let overlay = match store {
        Some(store) => this_overlay(store, ax.fs, &ax.home.state_dir())?,
        None => None,
    };
    let known = overlay
        .iter()
        .flat_map(|overlay| overlay.projects.iter().flatten())
        .map(|(_, project)| PathBuf::from(&project.path))
        .find(|known| same_path(known, &root));
    Ok(known.unwrap_or(root))
}

/// `<slug of the folder name>-<4 random [a-z0-9]>` (SPEC 5.5).
pub fn new_project_id(env: &Env, folder_name: &str) -> Result<String, CoreError> {
    let mut slug = String::new();
    for ch in folder_name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            slug.push(ch);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.truncate(ids::MAX_ID_LEN - 5);
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "project" } else { slug };
    // The random tail of a ULID: Crockford base32, so lowercase it is
    // [a-z0-9].
    let random = ids::new_ulid(env.now())?;
    let suffix = random[random.len() - 4..].to_ascii_lowercase();
    let id = format!("{slug}-{suffix}");
    ids::check(IdKind::Project, &id)?;
    Ok(id)
}

/// Hides `<P>/.loadout/` from the repository holding P through its
/// `info/exclude` (a private project, SPEC 5.5). Returns the exclude file
/// when it was changed.
pub fn exclude_manifest_dir(fs: &dyn Fs, root: &Path) -> Result<Option<PathBuf>, CoreError> {
    let Some((repo, pattern)) = manifest_dir_pattern(root)? else {
        return Ok(None);
    };
    if repo.is_ignored(&manifest_dir(root).join(LOCAL_FILE))? {
        return Ok(None);
    }
    let file = repo.exclude_file();
    Ok(ensure_line(fs, &file, &pattern)?.then_some(file))
}

/// Drops the `info/exclude` line [`exclude_manifest_dir`] wrote, so a
/// newly shared `project.toml` can be committed; `.loadout/.gitignore`
/// keeps the private file out. Returns a note for the user.
pub fn unexclude_manifest_dir(fs: &dyn Fs, root: &Path) -> Result<Option<String>, CoreError> {
    let Some((repo, pattern)) = manifest_dir_pattern(root)? else {
        return Ok(None);
    };
    let file = repo.exclude_file();
    if let Some(text) = read_optional(fs, &file)?
        && text.lines().any(|line| line.trim() == pattern)
    {
        let kept: String = text
            .lines()
            .filter(|line| line.trim() != pattern)
            .map(|line| format!("{line}\n"))
            .collect();
        write_file(fs, &file, kept.as_bytes())?;
    }
    let shared = manifest_dir(root).join(SHARED_FILE);
    Ok(Some(if repo.is_ignored(&shared)? {
        format!(
            "{} is ignored by git; change the ignore rules to commit it",
            shared.display()
        )
    } else {
        format!("commit {} with the project", shared.display())
    }))
}

/// The repository holding `root` and the `/<path>/.loadout/` pattern for
/// its manifest folder.
fn manifest_dir_pattern(root: &Path) -> Result<Option<(ProjectRepo, String)>, CoreError> {
    let Some(repo) = ProjectRepo::discover(root)? else {
        return Ok(None);
    };
    Ok(repo
        .relative(&manifest_dir(root))
        .map(|relative| (repo, format!("/{relative}/"))))
}

/// Appends `line` to the text file `path` (created when absent) unless it
/// already holds it; returns whether the file changed.
pub fn ensure_line(fs: &dyn Fs, path: &Path, line: &str) -> Result<bool, CoreError> {
    let mut text = read_optional(fs, path)?.unwrap_or_default();
    if text.lines().any(|existing| existing.trim() == line) {
        return Ok(false);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    if let Some(dir) = path.parent() {
        create_dir(fs, dir)?;
    }
    write_file(fs, path, text.as_bytes())?;
    Ok(true)
}

/// The text of `path`, or `None` when it does not exist.
pub fn read_optional(fs: &dyn Fs, path: &Path) -> Result<Option<String>, CoreError> {
    if !fs.exists(path) {
        return Ok(None);
    }
    fs.read_to_string(path).map(Some).map_err(|err| {
        CoreError::new(ErrorCode::IoError, format!("cannot read file: {err}")).with_path(path)
    })
}

/// Writes a project file (a manifest, `.gitignore`, the repository's
/// `info/exclude`); a write the OS refuses as read only is
/// `read_only_target` (SPEC 8.2).
pub fn write_file(fs: &dyn Fs, path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    fs.write(path, bytes)
        .map_err(|err| crate::access::write_error(fs, "cannot write file", path, &err))
}

pub(crate) fn create_dir(fs: &dyn Fs, dir: &Path) -> Result<(), CoreError> {
    fs.create_dir_all(dir)
        .map_err(|err| crate::access::write_error(fs, "cannot create directory", dir, &err))
}

/// `not_found` for a project id this machine does not know.
pub fn unknown_project(id: &str) -> CoreError {
    CoreError::new(
        ErrorCode::NotFound,
        format!("project {id:?} is not known on this machine"),
    )
}

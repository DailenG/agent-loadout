//! Installs and updates (SPEC 12.4).
//!
//! - Git: the system git clones into `cache/skill-sources/<id>` (sparse
//!   checkout of the skill folder when a path is known, from a
//!   `/tree/<ref>/<path>` URL or `--path`), the skill folder is copied into
//!   the store and committed as `skill: add <id> from <url>@<sha>`.
//! - Local: a folder, or a `.zip`/`.skill` archive, with zip-slip,
//!   symbolic link and size checks (SPEC 17).
//! - Update: `git fetch` in the cache clone; an unreachable source is
//!   `source_unreachable` and the store copy stays as it is.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::clients::ClientSelection;
use crate::model::ids::{self, IdKind};
use crate::model::settings::NewItemEnable;
use crate::model::skill_meta::{SkillEnable, SkillMeta, SkillSource, SkillSourceKind};
use crate::remote::git::{Invocation, RunOutput, git_not_found, locate};
use crate::scan::external::{self, ExternalReport};
use crate::scan::{self, ScanReport, Scanner};
use crate::skills::content_hash;
use crate::skills::link::{SyncReport, sync};
use crate::skills::store::{self, content_dir, missing_skill_file, not_found};
use crate::skills::{
    Entry, MAX_ARCHIVE_FILE_BYTES, MAX_ARCHIVE_FILES, MAX_ARCHIVE_TOTAL_BYTES, SKILL_FILE,
    SkillFiles, SkillsEnv, check_name, confine, entry, frontmatter, io_error, read_folder, slug,
};
use crate::sources::{
    self, SourceChange, SourceChangeKind, SourceRef, TrustCheck, TrustGrant, TrustVia,
};
use crate::store::{CommitId, Operation, Store};

/// Cache folder of git sources, below `<LOADOUT_HOME>/cache`.
pub const SOURCES_DIR: &str = "skill-sources";
/// Deepest folder searched for `SKILL.md` in a repository without a path.
const SEARCH_DEPTH: usize = 4;

/// Where a skill comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A git repository; `path` is the skill folder inside it.
    Git {
        url: String,
        git_ref: Option<String>,
        path: Option<String>,
    },
    /// A folder, or a `.zip`/`.skill` archive (absolute).
    Local(PathBuf),
}

impl Source {
    /// Reads a source argument: a URL (`https://`, `http://`, `ssh://`,
    /// `git://`, `file://`, `user@host:path`) is git, and a GitHub
    /// `/tree/<ref>/<path>` or `/blob/<ref>/<path>/SKILL.md` URL carries
    /// the ref and the path; a bare git repository folder is git too;
    /// anything else is a local folder or archive (made absolute by the
    /// caller).
    pub fn parse(text: &str, fs: &dyn Fs) -> Self {
        let text = text.trim();
        let lower = text.to_ascii_lowercase();
        let is_url = ["https://", "http://", "ssh://", "git://", "file://"]
            .iter()
            .any(|scheme| lower.starts_with(scheme))
            || is_scp_like(text);
        if is_url {
            return github_tree(text).unwrap_or_else(|| Source::Git {
                url: text.to_owned(),
                git_ref: None,
                path: None,
            });
        }
        let path = PathBuf::from(text);
        if path.is_absolute() && is_bare_repo(fs, &path) {
            return Source::Git {
                url: text.to_owned(),
                git_ref: None,
                path: None,
            };
        }
        Source::Local(path)
    }
}

/// `user@host:path` (not a Windows drive path).
fn is_scp_like(text: &str) -> bool {
    let Some((user_host, rest)) = text.split_once(':') else {
        return false;
    };
    user_host.contains('@') && !user_host.contains(['/', '\\']) && !rest.starts_with("//")
}

fn github_tree(url: &str) -> Option<Source> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    if parts.len() < 4 || !(parts[2] == "tree" || parts[2] == "blob") {
        return None;
    }
    let repo = parts[1].trim_end_matches(".git");
    let mut path: Vec<&str> = parts[4..].to_vec();
    if parts[2] == "blob" && path.last() == Some(&SKILL_FILE) {
        path.pop();
    }
    Some(Source::Git {
        url: format!("https://github.com/{}/{repo}", parts[0]),
        git_ref: Some(parts[3].to_owned()),
        path: (!path.is_empty()).then(|| path.join("/")),
    })
}

fn is_bare_repo(fs: &dyn Fs, path: &Path) -> bool {
    fs.exists(&path.join("HEAD"))
        && fs.is_dir(&path.join("objects"))
        && fs.is_dir(&path.join("refs"))
        && !fs.exists(&path.join(SKILL_FILE))
}

/// The id a new store skill gets.
#[derive(Debug, Clone, Default)]
pub struct Naming {
    /// The id to use instead of one from the skill's name.
    pub id: Option<String>,
    /// When the id is taken: install as `<id>-2` (the user confirmed),
    /// instead of failing with `already_exists`.
    pub rename_if_taken: bool,
}

/// The user's choices for an install or update (SPEC 12.4, 17): what the
/// gates before the store commit may let through. The default lets
/// nothing through that needs a decision.
#[derive(Debug, Clone, Default)]
pub struct Decisions {
    /// Install or update even when the built-in scan's verdict is
    /// `dangerous` ("Install anyway", `--allow-dangerous`); without it,
    /// such content is refused with `scan_blocked`.
    pub allow_dangerous: bool,
    /// The user's answer to the source trust prompt (#24) for the source
    /// asked for; `None` lets only a trusted git source through
    /// (`source_trust_required` otherwise). A block is never let through
    /// (`source_blocked`).
    pub trust: Option<TrustGrant>,
    /// The user's separate answer for the repository the host redirected
    /// to, when it moved to another key ([`Prepared::moved_to`],
    /// [`UpdateCheck::moved_trust`]): [`Decisions::trust`] never carries
    /// over to a location the user did not see. `None` lets only a
    /// trusted target through.
    pub moved_trust: Option<TrustGrant>,
    /// Where those answers were given; recorded with "always trust".
    pub via: TrustVia,
    /// The external scanner's report (#27) on the content an update
    /// writes, from the update review: a `dangerous` verdict needs
    /// [`Decisions::allow_dangerous`] like a built-in one. It must be of
    /// exactly the content written (`entry_changed` otherwise). Installs
    /// carry theirs on the [`Prepared`] ([`Prepared::set_external`]).
    pub external: Option<ExternalReport>,
}

/// An install.
#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub source: Source,
    /// The skill folder inside a git source (overrides the URL's).
    pub path: Option<String>,
    /// Branch, tag or commit of a git source (overrides the URL's); a
    /// commit pins the skill.
    pub git_ref: Option<String>,
    /// The skill to pick when a git source without a path holds several:
    /// the folder with this name (a skills.sh link names the skill, not
    /// its folder).
    pub skill: Option<String>,
    /// The catalog entry this install comes from, `<catalog>/<entry id>`
    /// (#26): recorded as `source.catalog`.
    pub catalog: Option<String>,
    pub naming: Naming,
    pub decisions: Decisions,
}

impl InstallRequest {
    pub fn new(source: Source) -> Self {
        Self {
            source,
            path: None,
            git_ref: None,
            skill: None,
            catalog: None,
            naming: Naming::default(),
            decisions: Decisions::default(),
        }
    }
}

/// What an install did.
#[derive(Debug, Clone)]
pub struct Installed {
    pub id: String,
    /// Frontmatter `name`, else the folder name.
    pub name: String,
    pub commit: Option<CommitId>,
    /// The git commit the content came from.
    pub source_commit: Option<String>,
    /// The id the name asked for, when it was taken and `-N` was used.
    pub renamed_from: Option<String>,
    /// Links made for a new skill enabled by `new_skill_enable`.
    pub report: Option<SyncReport>,
    pub warnings: Vec<CoreError>,
    /// The built-in scan of the installed content (SPEC 17).
    pub scan: ScanReport,
    /// The external scanner's report the install was gated on (#27).
    pub external: Option<ExternalReport>,
}

/// The content to commit and where it came from.
#[derive(Debug)]
struct Fetched {
    files: SkillFiles,
    /// Folder or archive name, repository name: the name when the
    /// frontmatter has none, and the commit message's `from`.
    base_name: String,
    from: String,
    source: SkillSource,
    /// The clone to keep as `cache/skill-sources/<id>`.
    clone: Option<PathBuf>,
    /// Where the host redirected a git URL (a renamed or transferred
    /// repository).
    redirect: Option<String>,
}

/// A fetched and scanned source waiting for the user's decisions
/// ([`prepare`], then [`install_prepared`]). A git source's temporary
/// clone is removed when this is dropped without being installed.
#[derive(Debug)]
pub struct Prepared {
    fetched: Fetched,
    name: String,
    scan: ScanReport,
    /// The external scanner's report on the same files (#27).
    external: Option<ExternalReport>,
    /// Installed: another [`install_prepared`] is refused.
    spent: bool,
    /// The source the user asked for, which the trust gate admits (git
    /// sources).
    trust_source: Option<SourceRef>,
    /// Where the host redirected it, when that is another repository: the
    /// gate admits it too, with its own answer.
    moved_to: Option<SourceRef>,
    /// The host moved the repository the URL names.
    source_change: Option<SourceChange>,
}

impl Prepared {
    /// Frontmatter `name`, else the folder, archive or repository name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The files that would be installed, by relative path.
    pub fn files(&self) -> &SkillFiles {
        &self.fetched.files
    }

    /// Where the content comes from (the git commit for a git source).
    pub fn source(&self) -> &SkillSource {
        &self.fetched.source
    }

    /// The built-in scan of [`Prepared::files`].
    pub fn scan(&self) -> &ScanReport {
        &self.scan
    }

    /// Attaches the external scanner's report on [`Prepared::files`] (#27):
    /// [`install_prepared`] then gates on it too.
    pub fn set_external(&mut self, report: Option<ExternalReport>) {
        self.external = report;
    }

    /// The attached external report.
    pub fn external(&self) -> Option<&ExternalReport> {
        self.external.as_ref()
    }

    /// The source the trust gate admits (#24): a git source's repository
    /// as the user named it; `None` for a local folder or archive (picked
    /// on this machine, not gated). A redirect never replaces it.
    pub fn trust_source(&self) -> Option<&SourceRef> {
        self.trust_source.as_ref()
    }

    /// The repository the host redirected the URL to, when it has another
    /// key: the gate admits it too, on [`Decisions::moved_trust`].
    pub fn moved_to(&self) -> Option<&SourceRef> {
        self.moved_to.as_ref()
    }

    /// The repository the URL names moved ([`SourceChangeKind::Moved`]).
    pub fn source_change(&self) -> Option<&SourceChange> {
        self.source_change.as_ref()
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if let Some(clone) = self.fetched.clone.take() {
            // The clone lives below `LOADOUT_HOME/cache`; a leftover is
            // re-creatable and replaced by the next install of that id.
            let _ = std::fs::remove_dir_all(clone);
        }
    }
}

/// Fetches a source (a git source into a temporary clone) and scans it,
/// without touching the store: what the install review shows. Callers
/// refuse a blocked git source first ([`sources::refuse_blocked`]), so a
/// blocked source is never contacted.
pub fn prepare(sx: &SkillsEnv<'_>, request: &InstallRequest) -> Result<Prepared, CoreError> {
    let requested = match &request.source {
        Source::Git { url, .. } => Some(SourceRef::from_git_url(url)?),
        Source::Local(_) => None,
    };
    let fetched = match &request.source {
        Source::Local(path) => fetch_local(sx.fs, path)?,
        Source::Git { url, git_ref, path } => {
            let git_ref = request.git_ref.clone().or_else(|| git_ref.clone());
            let path = request.path.clone().or_else(|| path.clone());
            let mut fetched = fetch_git(sx, url, git_ref, path, request.skill.as_deref())?;
            fetched.source.catalog.clone_from(&request.catalog);
            fetched
        }
    };
    let moved_to = requested.as_ref().and_then(|requested| {
        let redirect = fetched.redirect.as_deref()?;
        sources::detect::target_of(requested, redirect)
    });
    let source_change = moved_to.as_ref().map(sources::detect::moved_to);
    let scan = Scanner::new(sx.fs, sx.home).scan_files(&fetched.files);
    let name = fetched
        .files
        .get(SKILL_FILE)
        .and_then(|text| {
            frontmatter::parse(&String::from_utf8_lossy(text))
                .ok()
                .flatten()
        })
        .and_then(|fm| fm.name().map(str::to_owned))
        .unwrap_or_else(|| fetched.base_name.clone());
    Ok(Prepared {
        fetched,
        name,
        scan,
        external: None,
        spent: false,
        trust_source: requested,
        moved_to,
        source_change,
    })
}

/// Installs prepared content into the store (one commit) when the
/// decisions let it through: a git source must be trusted, or
/// [`Decisions::trust`] carries the user's answer (`source_trust_required`
/// or `source_blocked` otherwise, #24), and so must the repository it
/// moved to, on [`Decisions::moved_trust`]; then a `dangerous` scan
/// verdict needs [`Decisions::allow_dangerous`] (`scan_blocked`
/// otherwise). On an error `prepared` stays usable, so the review can ask
/// the user and try again with other decisions; once installed it is
/// spent (`invalid_value` on another try).
pub fn install_prepared(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    prepared: &mut Prepared,
    naming: &Naming,
    decisions: &Decisions,
) -> Result<Installed, CoreError> {
    if prepared.spent {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            "this reviewed content is already installed",
        ));
    }
    admit_with_move(
        store,
        prepared.trust_source.as_ref(),
        prepared.moved_to.as_ref(),
        decisions,
    )?;
    scan::gate(&prepared.scan, decisions.allow_dangerous)?;
    if let Some(report) = &prepared.external {
        external::gate(report, decisions.allow_dangerous)?;
    }
    let mut installed =
        commit_install(store, sx, naming, &prepared.fetched, prepared.scan.clone())?;
    installed.external = prepared.external.clone();
    // Committed: the review is spent even if what follows fails, so a
    // retry never installs a second copy.
    prepared.spent = true;
    if let Some(clone) = prepared.fetched.clone.take()
        && let Err(err) = keep_clone(sx, &clone, &installed.id)
    {
        let _ = remove_dir(sx.fs, &clone);
        installed.warnings.push(err);
    }
    Ok(installed)
}

/// The trust gate of a git source and the repository it moved to: both
/// are checked before either "always" is recorded.
fn admit_with_move(
    store: &Store<'_>,
    requested: Option<&SourceRef>,
    moved_to: Option<&SourceRef>,
    decisions: &Decisions,
) -> Result<(), CoreError> {
    let Some(requested) = requested else {
        return Ok(());
    };
    sources::would_admit(store, requested, decisions.trust)?;
    if let Some(target) = moved_to {
        sources::would_admit(store, target, decisions.moved_trust)?;
    }
    sources::admit(store, requested, decisions.trust, decisions.via)?;
    if let Some(target) = moved_to {
        sources::admit(store, target, decisions.moved_trust, decisions.via)?;
    }
    Ok(())
}

/// Installs a skill into the store (one commit): [`prepare`], then
/// [`install_prepared`] with the request's decisions. A git source the
/// gate would refuse is refused before anything is fetched; a repository
/// it moved to needs [`Decisions::moved_trust`] or its own trust.
pub fn install(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    request: &InstallRequest,
) -> Result<Installed, CoreError> {
    if let Source::Git { url, .. } = &request.source {
        sources::would_admit(
            store,
            &SourceRef::from_git_url(url)?,
            request.decisions.trust,
        )?;
    }
    let mut prepared = prepare(sx, request)?;
    install_prepared(
        store,
        sx,
        &mut prepared,
        &request.naming,
        &request.decisions,
    )
}

/// Installs already-read content (agents CLI vendoring, tool folder
/// imports). `source.kind` says where it came from. The content is
/// already on this machine, in a folder a client reads, so the scan is
/// reported and never blocks.
pub(crate) fn install_files(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    naming: &Naming,
    files: SkillFiles,
    base_name: String,
    source: SkillSource,
) -> Result<Installed, CoreError> {
    let fetched = Fetched {
        files,
        from: base_name.clone(),
        base_name,
        source,
        clone: None,
        redirect: None,
    };
    let scan = Scanner::new(sx.fs, sx.home).scan_files(&fetched.files);
    commit_install(store, sx, naming, &fetched, scan)
}

fn commit_install(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    naming: &Naming,
    fetched: &Fetched,
    scan: ScanReport,
) -> Result<Installed, CoreError> {
    let Some(skill_md) = fetched.files.get(SKILL_FILE) else {
        return Err(CoreError::new(
            ErrorCode::NotFound,
            format!("the source has no {SKILL_FILE}"),
        ));
    };
    let mut warnings = Vec::new();
    let parsed = match frontmatter::parse(&String::from_utf8_lossy(skill_md)) {
        Ok(parsed) => parsed,
        Err(err) => {
            warnings.push(err);
            None
        }
    };
    let name = parsed
        .as_ref()
        .and_then(|fm| fm.name())
        .map_or_else(|| fetched.base_name.clone(), str::to_owned);
    let wanted = match &naming.id {
        Some(id) => {
            ids::check(IdKind::Skill, id)?;
            id.clone()
        }
        None => slug(&name)
            .or_else(|| slug(&fetched.base_name))
            .ok_or_else(|| {
                CoreError::new(
                    ErrorCode::InvalidId,
                    format!("no skill id can be made from {name:?}; choose one"),
                )
            })?,
    };
    let settings = store::settings(store);
    let enable_new = settings.new_skill_enable == Some(NewItemEnable::AllDetected);
    let operation = Operation::new("skill.add", format!("skill: add {wanted}"));
    let committed = store.transaction(operation, |tx| {
        let id = if store::exists(tx, &wanted)? {
            let free = store::free_id(tx, &wanted)?;
            if !naming.rename_if_taken {
                return Err(CoreError::new(
                    ErrorCode::AlreadyExists,
                    format!(
                        "skill {wanted:?} already exists; confirm to install this one as {free:?}"
                    ),
                ));
            }
            free
        } else {
            wanted.clone()
        };
        let from = match &fetched.source.commit {
            Some(commit) => format!("{}@{}", fetched.from, &commit[..commit.len().min(7)]),
            None => fetched.from.clone(),
        };
        tx.set_summary(format!("skill: add {id} from {from}"));
        tx.add_entity(format!("skill/{id}"));
        store::stage_content(tx, &id, &fetched.files)?;
        let mut meta = SkillMeta::new(&id);
        meta.name = Some(name.clone());
        meta.source = Some(fetched.source.clone());
        if enable_new {
            meta.enable = Some(SkillEnable {
                clients: Some(ClientSelection::AllDetected),
                ..SkillEnable::default()
            });
        }
        store::stage_meta(tx, &meta)?;
        Ok(id)
    })?;
    let id = committed.value;
    warnings.extend(store::total_size_warning(store));
    // The store commit is done: a sync that fails now (lock timeout,
    // ledger I/O) is a warning, and the next apply or sync makes the links.
    let report = if enable_new {
        match sync(store, sx) {
            Ok(report) => Some(report),
            Err(err) => {
                warnings.push(err);
                None
            }
        }
    } else {
        None
    };
    Ok(Installed {
        renamed_from: (id != wanted).then_some(wanted),
        id,
        name,
        commit: committed.commit,
        source_commit: fetched.source.commit.clone(),
        report,
        warnings,
        scan,
        external: None,
    })
}

fn fetch_local(fs: &dyn Fs, path: &Path) -> Result<Fetched, CoreError> {
    confine(fs, path)?;
    if !path.is_absolute() {
        return Err(
            CoreError::new(ErrorCode::InvalidPath, "a local source must be absolute")
                .with_path(path),
        );
    }
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("skill")
        .to_owned();
    let lower = file_name.to_ascii_lowercase();
    let (files, base_name) = if fs.is_dir(path) {
        let files = if fs.exists(&path.join(SKILL_FILE)) {
            read_folder(fs, path)?
        } else {
            match find_skill_dirs(fs, path)?.as_slice() {
                [] => return Err(missing_skill_file(path)),
                [one] => read_folder(fs, &path.join(one))?,
                many => return Err(several(many)),
            }
        };
        (files, file_name)
    } else if lower.ends_with(".zip") || lower.ends_with(".skill") {
        let stem = file_name
            .rsplit_once('.')
            .map_or(file_name.as_str(), |(stem, _)| stem)
            .to_owned();
        (pick_root(read_archive(fs, path)?)?, stem)
    } else if fs.exists(path) {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            "a local source is a folder, a .zip or a .skill file",
        )
        .with_path(path));
    } else {
        return Err(
            CoreError::new(ErrorCode::NotFound, "the source does not exist").with_path(path),
        );
    };
    Ok(Fetched {
        files,
        from: base_name.clone(),
        base_name,
        source: SkillSource {
            kind: SkillSourceKind::Local,
            url: None,
            path: None,
            git_ref: None,
            commit: None,
            catalog: None,
            extra: toml::Table::new(),
        },
        clone: None,
        redirect: None,
    })
}

fn several(dirs: &[String]) -> CoreError {
    CoreError::new(
        ErrorCode::InvalidValue,
        format!(
            "the source holds several skills ({}); choose one with a path",
            dirs.join(", ")
        ),
    )
}

/// Folders below `dir` (relative, `/`-separated) holding a `SKILL.md`,
/// up to [`SEARCH_DEPTH`] levels, without descending into a skill or a
/// `.git` folder.
fn find_skill_dirs(fs: &dyn Fs, dir: &Path) -> Result<Vec<String>, CoreError> {
    let mut found = Vec::new();
    search(fs, dir, "", 0, &mut found)?;
    Ok(found)
}

fn search(
    fs: &dyn Fs,
    dir: &Path,
    prefix: &str,
    depth: usize,
    found: &mut Vec<String>,
) -> Result<(), CoreError> {
    if depth >= SEARCH_DEPTH {
        return Ok(());
    }
    let entries = fs
        .read_dir(dir)
        .map_err(|err| io_error("cannot list directory", dir, &err))?;
    for path in entries {
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        // Linked folders are never candidates: a repository can link to
        // any folder on this machine (SPEC 17).
        if name.starts_with('.') || entry(fs, &path)? != Entry::Dir {
            continue;
        }
        let relative = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        if fs.exists(&path.join(SKILL_FILE)) {
            found.push(relative);
        } else {
            search(fs, &path, &relative, depth + 1, found)?;
        }
    }
    Ok(())
}

/// The files of an archive, checked per SPEC 17: no entry outside the
/// archive root (zip-slip: absolute names, drive names, `..`), no
/// symbolic links, at most [`MAX_ARCHIVE_FILES`] files of at most
/// [`MAX_ARCHIVE_FILE_BYTES`] each and [`MAX_ARCHIVE_TOTAL_BYTES`] in all.
/// Nested `.git` folders and `__MACOSX` are left out.
pub fn read_archive(fs: &dyn Fs, path: &Path) -> Result<SkillFiles, CoreError> {
    let bytes = fs
        .read(path)
        .map_err(|err| io_error("cannot read file", path, &err))?;
    let zip_error = |err: zip::result::ZipError| {
        CoreError::new(ErrorCode::ParseError, format!("not a readable zip: {err}")).with_path(path)
    };
    let refuse = |code: ErrorCode, message: String| CoreError::new(code, message).with_path(path);
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(zip_error)?;
    if archive.len() > MAX_ARCHIVE_FILES * 4 {
        return Err(refuse(
            ErrorCode::InvalidValue,
            format!("the archive holds more than {MAX_ARCHIVE_FILES} files"),
        ));
    }
    let mut files = SkillFiles::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(zip_error)?;
        let raw = entry.name().map_err(zip_error)?.replace('\\', "/");
        let parts: Vec<&str> = raw.split('/').filter(|part| !part.is_empty()).collect();
        let escapes = raw.starts_with('/')
            || parts.iter().any(|part| *part == ".." || part.contains(':'))
            || entry.enclosed_name().is_none();
        if escapes {
            return Err(refuse(
                ErrorCode::InvalidPath,
                format!("archive entry {raw:?} points outside the archive (zip-slip)"),
            ));
        }
        if entry.is_symlink() {
            return Err(refuse(
                ErrorCode::InvalidPath,
                format!("archive entry {raw:?} is a symbolic link"),
            ));
        }
        if entry.is_dir()
            || parts.first() == Some(&"__MACOSX")
            || parts.iter().any(|part| part.eq_ignore_ascii_case(".git"))
        {
            continue;
        }
        for part in &parts {
            check_name(part, path)?;
        }
        if files.len() >= MAX_ARCHIVE_FILES {
            return Err(refuse(
                ErrorCode::InvalidValue,
                format!("the archive holds more than {MAX_ARCHIVE_FILES} files"),
            ));
        }
        let too_big = |raw: &str| {
            refuse(
                ErrorCode::InvalidValue,
                format!(
                    "archive entry {raw:?} is larger than {} MB",
                    MAX_ARCHIVE_FILE_BYTES / (1024 * 1024)
                ),
            )
        };
        if entry.size() > MAX_ARCHIVE_FILE_BYTES {
            return Err(too_big(&raw));
        }
        let mut content = Vec::new();
        entry
            .take(MAX_ARCHIVE_FILE_BYTES + 1)
            .read_to_end(&mut content)
            .map_err(|err| io_error("cannot read the archive", path, &err))?;
        if content.len() as u64 > MAX_ARCHIVE_FILE_BYTES {
            return Err(too_big(&raw));
        }
        total += content.len() as u64;
        if total > MAX_ARCHIVE_TOTAL_BYTES {
            return Err(refuse(
                ErrorCode::InvalidValue,
                format!(
                    "the archive's files are larger than {} MB together",
                    MAX_ARCHIVE_TOTAL_BYTES / (1024 * 1024)
                ),
            ));
        }
        files.insert(parts.join("/"), content);
    }
    Ok(files)
}

/// The files of the one skill in `files`: the whole map when `SKILL.md` is
/// at its root, else the folder holding the only `SKILL.md`.
fn pick_root(files: SkillFiles) -> Result<SkillFiles, CoreError> {
    if files.contains_key(SKILL_FILE) {
        return Ok(files);
    }
    let mut roots: Vec<String> = files
        .keys()
        .filter_map(|path| path.strip_suffix(&format!("/{SKILL_FILE}")))
        .map(str::to_owned)
        .collect();
    // A skill's own subfolders may hold other SKILL.md files: keep the
    // shallowest.
    roots.sort_by_key(|root| root.matches('/').count());
    let Some(first) = roots.first().cloned() else {
        return Err(CoreError::new(
            ErrorCode::NotFound,
            format!("the archive has no {SKILL_FILE}"),
        ));
    };
    let depth = first.matches('/').count();
    let top: Vec<String> = roots
        .into_iter()
        .filter(|root| root.matches('/').count() == depth)
        .collect();
    if top.len() > 1 {
        return Err(several(&top));
    }
    let prefix = format!("{first}/");
    Ok(files
        .into_iter()
        .filter_map(|(path, bytes)| {
            path.strip_prefix(&prefix)
                .map(|rest| (rest.to_owned(), bytes))
        })
        .collect())
}

/// `<LOADOUT_HOME>/cache/skill-sources`.
fn sources_dir(sx: &SkillsEnv<'_>) -> PathBuf {
    sx.home.cache_dir().join(SOURCES_DIR)
}

/// Git on `PATH`. Program lookup reads the real file system, as the
/// programs live outside any confined view (a test's temp root).
pub(crate) fn git_path(sx: &SkillsEnv<'_>) -> Result<PathBuf, CoreError> {
    locate(sx.env, &crate::fs::RealFs)
        .ok_or_else(|| git_not_found("Git skill sources, MCP servers from Git, and Git catalogs"))
}

/// Git's repository-local variables, dropped so an inherited `GIT_DIR`
/// never points a clone or fetch elsewhere.
const REPOSITORY_VARS: [&str; 9] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_TERMINAL_PROMPT",
];

/// Git's messages in English whatever the user's locale: the redirect
/// warning is read from them (#24).
const LOCALE: [(&str, &str); 2] = [("LC_ALL", "C"), ("LANGUAGE", "C")];

pub(crate) fn run_git(
    sx: &SkillsEnv<'_>,
    git: &Path,
    cwd: &Path,
    args: &[&OsStr],
) -> Result<RunOutput, CoreError> {
    let mut env: Vec<(OsString, OsString)> = sx
        .env
        .vars()
        .filter(|(name, _)| {
            !REPOSITORY_VARS
                .iter()
                .chain(LOCALE.iter().map(|(var, _)| var))
                .any(|var| name.eq_ignore_ascii_case(var))
        })
        .map(|(name, value)| (OsString::from(name), value.to_owned()))
        .collect();
    env.push(("GIT_TERMINAL_PROMPT".into(), "0".into()));
    env.extend(
        LOCALE
            .iter()
            .map(|(var, value)| (OsString::from(var), OsString::from(value))),
    );
    let invocation = Invocation {
        program: git.to_path_buf(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        env,
        cwd: cwd.to_path_buf(),
    };
    sx.runner
        .run(&invocation, &mut |_| {})
        .map_err(|err| CoreError::new(ErrorCode::ToolNotFound, format!("cannot run git: {err}")))
}

/// The last non-empty line git printed to stderr.
fn git_message(output: &RunOutput) -> String {
    output
        .stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("git failed")
        .to_owned()
}

/// `source_unreachable`, without the credentials a URL (or git's message
/// about it) can carry.
pub(crate) fn unreachable(url: &str, output: &RunOutput) -> CoreError {
    CoreError::new(
        ErrorCode::SourceUnreachable,
        SourceRef::redact_userinfo(&format!(
            "the source {url} cannot be reached: {}",
            git_message(output)
        )),
    )
}

pub(crate) fn git_failed(what: &str, output: &RunOutput) -> CoreError {
    CoreError::new(
        ErrorCode::GitError,
        SourceRef::redact_userinfo(&format!("{what}: {}", git_message(output))),
    )
}

/// A relative `/`-separated path inside a repository.
pub(crate) fn check_repo_path(path: &str) -> Result<String, CoreError> {
    let trimmed = path.trim_matches('/');
    let valid = !trimmed.is_empty()
        && trimmed.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && !part.contains(['\\', ':'])
        })
        && !trimmed.starts_with('-');
    if valid {
        Ok(trimmed.to_owned())
    } else {
        Err(CoreError::new(
            ErrorCode::InvalidPath,
            format!("{path:?} is not a folder path inside the repository"),
        ))
    }
}

/// A branch, tag or full commit. A short commit (7 to 39 hex characters)
/// is refused: a shallow clone cannot fetch one by an abbreviated name.
pub(crate) fn check_ref(git_ref: &str) -> Result<(), CoreError> {
    let valid = !git_ref.is_empty()
        && !git_ref.starts_with('-')
        && !git_ref.contains("..")
        && git_ref
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b));
    if !valid {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{git_ref:?} is not a branch or tag name"),
        ));
    }
    if (7..40).contains(&git_ref.len()) && git_ref.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!(
                "{git_ref:?} looks like a short commit: pin the full 40-character commit \
                 (git rev-parse {git_ref})"
            ),
        ));
    }
    Ok(())
}

pub(crate) fn check_url(url: &str) -> Result<(), CoreError> {
    if url.is_empty() || url.starts_with('-') || url.contains(['\n', '\r']) {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{url:?} is not a git URL"),
        ));
    }
    Ok(())
}

pub(crate) fn is_remote_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    ["https://", "http://", "ssh://", "git://"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
        || is_scp_like(url)
}

/// A fresh clone.
pub(crate) struct Cloned {
    pub(crate) commit: String,
    /// The branch followed.
    pub(crate) branch: Option<String>,
    /// Where the host redirected the URL (a renamed or transferred
    /// repository).
    pub(crate) redirect: Option<String>,
}

/// The URL git contacts for `url`: a remote URL rebuilt from its parsed
/// parts ([`SourceRef::fetch_url`]), so what git contacts is exactly what
/// the trust gate checked; a local path or `file://` URL as it is.
pub(crate) fn fetch_url(url: &str) -> Result<String, CoreError> {
    if !is_remote_url(url) {
        return Ok(url.to_owned());
    }
    Ok(SourceRef::from_git_url(url)?.fetch_url(url))
}

/// Clones `url` (from its [`fetch_url`]) into `dir` (which must not
/// exist): sparse to `path` when given. Without `--quiet`, so git reports
/// an HTTP redirect.
pub(crate) fn clone(
    sx: &SkillsEnv<'_>,
    git: &Path,
    url: &str,
    git_ref: Option<&str>,
    path: Option<&str>,
    dir: &Path,
) -> Result<Cloned, CoreError> {
    check_url(url)?;
    if let Some(git_ref) = git_ref {
        check_ref(git_ref)?;
    }
    let fetch = fetch_url(url)?;
    let parent = dir.parent().expect("a clone folder has a parent");
    confine(sx.fs, dir)?;
    sx.fs
        .create_dir_all(parent)
        .map_err(|err| io_error("cannot create directory", parent, &err))?;
    let mut args: Vec<&OsStr> = vec![OsStr::new("clone"), OsStr::new("--no-progress")];
    if path.is_some() {
        args.push(OsStr::new("--no-checkout"));
    }
    if is_remote_url(url) {
        args.extend([OsStr::new("--depth"), OsStr::new("1")]);
    }
    // `--branch` takes a branch or tag; a commit is checked out after the
    // clone (fetched when the shallow clone does not hold it).
    let commit_ref = git_ref.filter(|r| is_commit_ref(r));
    if let Some(branch) = git_ref.filter(|r| !is_commit_ref(r)) {
        args.extend([OsStr::new("--branch"), OsStr::new(branch)]);
    }
    args.extend([OsStr::new("--"), OsStr::new(&fetch), dir.as_os_str()]);
    let output = run_git(sx, git, parent, &args)?;
    if !output.success {
        let _ = remove_dir(sx.fs, dir);
        return Err(unreachable(url, &output));
    }
    let redirect = sources::detect::redirect_target(&fetch, &output.stderr);
    if let Some(path) = path {
        let pattern = format!("/{path}/");
        let steps: [&[&str]; 2] = [
            &["sparse-checkout", "set", "--no-cone", &pattern],
            &["checkout", "--quiet"],
        ];
        for step in steps {
            let args: Vec<&OsStr> = step.iter().map(OsStr::new).collect();
            let output = run_git(sx, git, dir, &args)?;
            if !output.success {
                let _ = remove_dir(sx.fs, dir);
                return Err(git_failed("cannot check out the skill folder", &output));
            }
        }
    }
    if let Some(wanted) = commit_ref {
        let run = |args: &[&str]| {
            let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
            run_git(sx, git, dir, &args)
        };
        let fail = |err: CoreError| {
            let _ = remove_dir(sx.fs, dir);
            err
        };
        let mut output = run(&["checkout", "--quiet", "--detach", wanted]).map_err(fail)?;
        if !output.success && is_remote_url(url) {
            let fetched =
                run(&["fetch", "--quiet", "--depth", "1", &fetch, wanted]).map_err(fail)?;
            if fetched.success {
                output = run(&["checkout", "--quiet", "--detach", "FETCH_HEAD"]).map_err(fail)?;
            }
        }
        if !output.success {
            return Err(fail(CoreError::new(
                ErrorCode::NotFound,
                format!("the repository has no commit {wanted}"),
            )));
        }
    }
    let commit = rev_parse(sx, git, dir, "HEAD")?;
    let branch = match git_ref {
        Some(_) if commit_ref.is_some() => None,
        Some(git_ref) => Some(git_ref.to_owned()),
        None => {
            let output = run_git(
                sx,
                git,
                dir,
                &[
                    OsStr::new("symbolic-ref"),
                    OsStr::new("--quiet"),
                    OsStr::new("--short"),
                    OsStr::new("HEAD"),
                ],
            )?;
            output
                .success
                .then(|| output.stdout.trim().to_owned())
                .filter(|branch| !branch.is_empty())
        }
    };
    Ok(Cloned {
        commit,
        branch,
        redirect,
    })
}

/// A ref that names a commit (40 hex characters; [`check_ref`] refuses a
/// short one).
pub(crate) fn is_commit_ref(git_ref: &str) -> bool {
    git_ref.len() == 40 && git_ref.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(crate) fn rev_parse(
    sx: &SkillsEnv<'_>,
    git: &Path,
    dir: &Path,
    rev: &str,
) -> Result<String, CoreError> {
    let output = run_git(
        sx,
        git,
        dir,
        &[
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new(rev),
        ],
    )?;
    let commit = output.stdout.trim().to_owned();
    if !output.success || !ids::is_commit_id(&commit) {
        return Err(git_failed("cannot read the source commit", &output));
    }
    Ok(commit)
}

fn fetch_git(
    sx: &SkillsEnv<'_>,
    url: &str,
    git_ref: Option<String>,
    path: Option<String>,
    skill: Option<&str>,
) -> Result<Fetched, CoreError> {
    let git = git_path(sx)?;
    let path = path.as_deref().map(check_repo_path).transpose()?;
    let temp = sources_dir(sx).join(format!(
        ".tmp-{}",
        ids::new_ulid(sx.env.now())?.to_ascii_lowercase()
    ));
    let Cloned {
        commit,
        branch,
        redirect,
    } = clone(sx, &git, url, git_ref.as_deref(), path.as_deref(), &temp)?;
    let read = || -> Result<(SkillFiles, Option<String>), CoreError> {
        match &path {
            Some(path) => {
                let dir = join_rel(&temp, path);
                if !sx.fs.is_dir(&dir) {
                    return Err(CoreError::new(
                        ErrorCode::NotFound,
                        format!("the repository has no folder {path:?}"),
                    ));
                }
                Ok((read_in_clone(sx.fs, &temp, &dir)?, Some(path.clone())))
            }
            None if sx.fs.exists(&temp.join(SKILL_FILE)) => {
                Ok((read_in_clone(sx.fs, &temp, &temp)?, None))
            }
            None => {
                let found = find_skill_dirs(sx.fs, &temp)?;
                let named = |dir: &&String| {
                    skill.is_some_and(|skill| {
                        dir.rsplit('/')
                            .next()
                            .is_some_and(|name| name.eq_ignore_ascii_case(skill))
                    })
                };
                let picked: Vec<&String> = found.iter().filter(named).collect();
                let chosen = match picked.as_slice() {
                    [one] => Some(*one),
                    _ if found.len() == 1 => found.first(),
                    _ => None,
                };
                match chosen {
                    Some(one) => Ok((
                        read_in_clone(sx.fs, &temp, &join_rel(&temp, one))?,
                        Some(one.clone()),
                    )),
                    None if found.is_empty() => Err(missing_skill_file(&temp)),
                    None => Err(several(&found)),
                }
            }
        }
    };
    let (files, path) = match read() {
        Ok(read) => read,
        Err(err) => {
            let _ = remove_dir(sx.fs, &temp);
            return Err(err);
        }
    };
    let repo_name = url
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\', ':'])
        .next()
        .unwrap_or("skill")
        .trim_end_matches(".git")
        .to_owned();
    let base_name = path
        .as_deref()
        .and_then(|path| path.rsplit('/').next())
        .map_or(repo_name, str::to_owned);
    Ok(Fetched {
        files,
        base_name,
        from: url.to_owned(),
        source: SkillSource {
            kind: SkillSourceKind::Git,
            url: Some(url.to_owned()),
            path,
            // A commit pins the skill: its updates fetch that commit.
            git_ref: branch.or_else(|| git_ref.filter(|r| is_commit_ref(r))),
            commit: Some(commit),
            catalog: None,
            extra: toml::Table::new(),
        },
        clone: Some(temp),
        redirect,
    })
}

pub(crate) fn join_rel(base: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(base.to_path_buf(), |path, part| path.join(part))
}

/// Moves a fresh clone to `cache/skill-sources/<id>`, replacing an old one.
fn keep_clone(sx: &SkillsEnv<'_>, clone: &Path, id: &str) -> Result<(), CoreError> {
    let target = sources_dir(sx).join(id);
    confine(sx.fs, clone)?;
    confine(sx.fs, &target)?;
    if sx.fs.exists(&target) {
        remove_dir(sx.fs, &target)?;
    }
    std::fs::rename(clone, &target).map_err(|err| io_error("cannot move the clone", &target, &err))
}

pub(crate) fn remove_dir(fs: &dyn Fs, dir: &Path) -> Result<(), CoreError> {
    if fs.exists(dir) {
        fs.remove_dir_all(dir)
            .map_err(|err| io_error("cannot remove folder", dir, &err))?;
    }
    Ok(())
}

/// Removes the cache clone of `id`, if any.
pub fn forget_source(sx: &SkillsEnv<'_>, id: &str) -> Result<(), CoreError> {
    ids::check(IdKind::Skill, id)?;
    remove_dir(sx.fs, &sources_dir(sx).join(id))
}

/// Removes temporary clones (`cache/skill-sources/.tmp-*`) not changed
/// for `older_than`: left by an install review the app never finished (it
/// ends without dropping its state) or by a process that crashed. A clone
/// still in use by another process is younger than that. Returns how many
/// were removed; failures are skipped (the next sweep tries again).
pub fn sweep_temp_clones(sx: &SkillsEnv<'_>, older_than: std::time::Duration) -> usize {
    let dir = sources_dir(sx);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    let now = sx.env.now();
    entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".tmp-"))
        .filter(|entry| {
            entry
                .metadata()
                .and_then(|meta| meta.modified())
                .is_ok_and(|modified| {
                    now.duration_since(modified).unwrap_or_default() >= older_than
                })
        })
        .filter(|entry| std::fs::remove_dir_all(entry.path()).is_ok())
        .count()
}

/// One file that differs between the store copy and the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
    /// An added or modified file that looks executable (a script or
    /// binary by extension, or text starting with `#!`): highlighted in
    /// the update review (#24).
    pub script: bool,
}

/// File extensions of scripts and binaries an agent or a hook could run.
const SCRIPT_EXTENSIONS: [&str; 20] = [
    "sh", "bash", "zsh", "fish", "ps1", "psm1", "bat", "cmd", "py", "js", "mjs", "cjs", "ts", "rb",
    "pl", "php", "exe", "dll", "so", "dylib",
];

/// Whether `path` (with content `bytes`) looks executable.
pub fn looks_executable(path: &str, bytes: &[u8]) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let by_extension = name.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty() && SCRIPT_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
    });
    by_extension || bytes.starts_with(b"#!")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Removed,
}

impl ChangeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            ChangeKind::Added => "added",
            ChangeKind::Modified => "modified",
            ChangeKind::Removed => "removed",
        }
    }
}

/// What an update check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStatus {
    UpToDate,
    Available,
    /// Not a git source (local, authored, agents CLI): nothing to check.
    NotTracked,
    /// The source cannot be reached or no longer holds the skill
    /// (`source_unreachable`); the store copy is intact.
    Unreachable,
    /// A block covers the source (`source_blocked`): it was not contacted.
    Blocked,
}

impl UpdateStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            UpdateStatus::UpToDate => "up_to_date",
            UpdateStatus::Available => "available",
            UpdateStatus::NotTracked => "not_tracked",
            UpdateStatus::Unreachable => "source_unreachable",
            UpdateStatus::Blocked => "source_blocked",
        }
    }
}

/// Result of [`check_update`].
#[derive(Debug, Clone)]
pub struct UpdateCheck {
    pub id: String,
    pub status: UpdateStatus,
    /// The commit the store copy came from.
    pub current: Option<String>,
    /// The source's commit now.
    pub latest: Option<String>,
    /// Files that differ from the store copy (a store copy edited locally
    /// shows here too).
    pub changes: Vec<FileChange>,
    pub error: Option<CoreError>,
    /// The built-in scan of the source's content now, when an update is
    /// available (SPEC 17): what the update review shows.
    pub scan: Option<ScanReport>,
    /// The trust check of the skill's git source (#24): an update needs
    /// trust (or the user's answer) like an install.
    pub trust: Option<TrustCheck>,
    /// What changed about the source since the last fetch (force-push,
    /// moved); trust in it lapsed when it was trusted.
    pub source_change: Option<SourceChange>,
    /// The trust check of the repository the source moved to, when the
    /// host redirected it to another key: the update needs its trust too
    /// ([`Decisions::moved_trust`]), and moves the skill's URL there.
    pub moved_trust: Option<TrustCheck>,
}

/// The latest content of a git skill, and what changed about its source.
struct Latest {
    commit: String,
    files: SkillFiles,
    change: Option<SourceChange>,
    /// The repository the host redirected the source to (another key).
    moved_to: Option<SourceRef>,
}

/// In a cache clone's `.git`: the URL of the repository the last fetch
/// was redirected to, so a reviewed update knows where the content it
/// writes came from without fetching again.
const MOVED_MARKER: &str = "loadout-moved-to";

/// The redirect target the last fetch into the cache clone `dir` saw.
fn moved_marker(sx: &SkillsEnv<'_>, dir: &Path, url: &str) -> Option<SourceRef> {
    let requested = SourceRef::from_git_url(url).ok()?;
    let target = sx
        .fs
        .read_to_string(&dir.join(".git").join(MOVED_MARKER))
        .ok()?;
    sources::detect::target_of(&requested, target.trim())
}

fn set_moved_marker(
    sx: &SkillsEnv<'_>,
    dir: &Path,
    target: Option<&SourceRef>,
) -> Result<(), CoreError> {
    let marker = dir.join(".git").join(MOVED_MARKER);
    let result = match target {
        Some(target) => sx.fs.write(&marker, target.url().as_bytes()),
        None if sx.fs.exists(&marker) => sx.fs.remove_file(&marker),
        None => Ok(()),
    };
    result.map_err(|err| io_error("cannot write", &marker, &err))
}

/// The latest content of a git skill: the cache clone fetched (re-cloned
/// when missing) and checked out at the source's head. A redirect to
/// another repository is [`SourceChangeKind::Moved`]; a head that no
/// longer contains the commit installed last is
/// [`SourceChangeKind::HistoryRewritten`] (#24).
fn latest(sx: &SkillsEnv<'_>, id: &str, source: &SkillSource) -> Result<Latest, CoreError> {
    let url = source
        .url
        .as_deref()
        .ok_or_else(|| CoreError::new(ErrorCode::MissingField, "the git source has no url"))?;
    let git = git_path(sx)?;
    let path = source.path.as_deref().map(check_repo_path).transpose()?;
    let dir = sources_dir(sx).join(id);
    let fetch = fetch_url(url)?;
    let (commit, redirect) = if sx.fs.exists(&dir.join(".git")) {
        // Without `--quiet`, so git reports an HTTP redirect.
        let mut args: Vec<&OsStr> = vec![OsStr::new("fetch"), OsStr::new("--no-progress")];
        if is_remote_url(url) {
            args.extend([OsStr::new("--depth"), OsStr::new("1")]);
        }
        args.extend([OsStr::new("--"), OsStr::new(&fetch)]);
        if let Some(git_ref) = &source.git_ref {
            check_ref(git_ref)?;
            args.push(OsStr::new(git_ref));
        }
        let output = run_git(sx, &git, &dir, &args)?;
        if !output.success {
            return Err(unreachable(url, &output));
        }
        let redirect = sources::detect::redirect_target(&fetch, &output.stderr);
        let commit = rev_parse(sx, &git, &dir, "FETCH_HEAD")?;
        let output = run_git(
            sx,
            &git,
            &dir,
            &[
                OsStr::new("reset"),
                OsStr::new("--hard"),
                OsStr::new("--quiet"),
                OsStr::new(&commit),
            ],
        )?;
        if !output.success {
            return Err(git_failed(
                "cannot check out the new source commit",
                &output,
            ));
        }
        (commit, redirect)
    } else {
        remove_dir(sx.fs, &dir)?;
        let cloned = clone(
            sx,
            &git,
            url,
            source.git_ref.as_deref(),
            path.as_deref(),
            &dir,
        )?;
        (cloned.commit, cloned.redirect)
    };
    let moved_to = SourceRef::from_git_url(url)
        .ok()
        .and_then(|requested| sources::detect::target_of(&requested, redirect.as_deref()?));
    set_moved_marker(sx, &dir, moved_to.as_ref())?;
    let change = match &moved_to {
        Some(target) => Some(sources::detect::moved_to(target)),
        None => match &source.commit {
            Some(old) => {
                let in_clone = |args: &[&OsStr]| run_git(sx, &git, &dir, args);
                sources::detect::history_rewritten(
                    &in_clone,
                    &fetch,
                    source.git_ref.as_deref(),
                    old,
                    &commit,
                )?
                .then_some(SourceChange {
                    kind: SourceChangeKind::HistoryRewritten,
                    detail: None,
                })
            }
            None => None,
        },
    };
    let skill_dir = match &path {
        Some(path) => join_rel(&dir, path),
        None => dir.clone(),
    };
    if !sx.fs.exists(&skill_dir.join(SKILL_FILE)) {
        return Err(CoreError::new(
            ErrorCode::SourceUnreachable,
            SourceRef::redact_userinfo(&format!("the source {url} no longer holds the skill")),
        ));
    }
    Ok(Latest {
        commit,
        files: read_in_clone(sx.fs, &dir, &skill_dir)?,
        change,
        moved_to,
    })
}

/// [`read_folder`] of a skill folder inside a clone, refused with
/// `invalid_path` when the folder (or a link on its way) resolves outside
/// the clone: a repository can hold a link to any folder on this machine
/// (SPEC 17).
fn read_in_clone(fs: &dyn Fs, clone: &Path, dir: &Path) -> Result<SkillFiles, CoreError> {
    let resolve = |path: &Path| {
        fs.canonicalize(path)
            .map_err(|err| io_error("cannot resolve folder", path, &err))
    };
    if !resolve(dir)?.starts_with(resolve(clone)?) {
        return Err(CoreError::new(
            ErrorCode::InvalidPath,
            "the skill folder is a link that points outside the repository",
        )
        .with_path(dir));
    }
    read_folder(fs, dir)
}

fn changes(old: &SkillFiles, new: &SkillFiles) -> Vec<FileChange> {
    let mut out = Vec::new();
    for (path, bytes) in new {
        let kind = match old.get(path) {
            None => ChangeKind::Added,
            Some(before) if before != bytes => ChangeKind::Modified,
            Some(_) => continue,
        };
        out.push(FileChange {
            path: path.clone(),
            kind,
            script: looks_executable(path, bytes),
        });
    }
    for path in old.keys() {
        if !new.contains_key(path) {
            out.push(FileChange {
                path: path.clone(),
                kind: ChangeKind::Removed,
                script: false,
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// The trust gate's view of a git skill source; `None` for a URL that
/// names no source (a hand-edited one).
fn source_ref(source: &SkillSource) -> Option<SourceRef> {
    source
        .url
        .as_deref()
        .and_then(|url| SourceRef::from_git_url(url).ok())
}

fn git_source(store: &Store<'_>, id: &str) -> Result<Option<SkillSource>, CoreError> {
    let meta = store::read_meta(store, id)?;
    if meta.is_none() && !store.fs().is_dir(&content_dir(store, id)?) {
        return Err(not_found(id));
    }
    Ok(meta
        .and_then(|meta| meta.source)
        .filter(|source| source.kind == SkillSourceKind::Git && source.url.is_some()))
}

/// Checks a git skill for an update (SPEC 12.4). Touches only the cache
/// clone; an unreachable source is reported, not an error, and a blocked
/// one is reported without contacting it. A force-push or a move of a
/// trusted source makes its trust lapse (one store commit,
/// [`sources::lapse`]); that is the only store change a check makes.
pub fn check_update(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    id: &str,
) -> Result<UpdateCheck, CoreError> {
    let Some(source) = git_source(store, id)? else {
        return Ok(UpdateCheck {
            id: id.to_owned(),
            status: UpdateStatus::NotTracked,
            current: None,
            latest: None,
            changes: Vec::new(),
            error: None,
            scan: None,
            trust: None,
            source_change: None,
            moved_trust: None,
        });
    };
    let current = source.commit.clone();
    let gated = source_ref(&source);
    let not_fetched = |status, err: CoreError| -> Result<UpdateCheck, CoreError> {
        Ok(UpdateCheck {
            id: id.to_owned(),
            status,
            current: current.clone(),
            latest: None,
            changes: Vec::new(),
            error: Some(err),
            scan: None,
            trust: gated
                .as_ref()
                .map(|source| sources::check(store, source))
                .transpose()?,
            source_change: None,
            moved_trust: None,
        })
    };
    if let Some(gated) = &gated
        && let Err(err) = sources::refuse_blocked(store, gated)
    {
        return not_fetched(UpdateStatus::Blocked, err);
    }
    let Latest {
        commit,
        files,
        change,
        moved_to,
    } = match latest(sx, id, &source) {
        Ok(latest) => latest,
        Err(err) if err.code() == ErrorCode::SourceUnreachable => {
            return not_fetched(UpdateStatus::Unreachable, err);
        }
        Err(err) => return Err(err),
    };
    if let (Some(gated), Some(change)) = (&gated, &change) {
        sources::lapse(store, gated, change)?;
    }
    let trust = gated
        .as_ref()
        .map(|source| sources::check(store, source))
        .transpose()?;
    let moved_trust = moved_to
        .as_ref()
        .map(|target| sources::check(store, target))
        .transpose()?;
    let stored = store::read_content(store, id)?;
    let changes = changes(&stored, &files);
    let status = if changes.is_empty() && current.as_deref() == Some(commit.as_str()) {
        UpdateStatus::UpToDate
    } else {
        UpdateStatus::Available
    };
    let scan = (status == UpdateStatus::Available)
        .then(|| Scanner::new(sx.fs, sx.home).scan_files(&files));
    Ok(UpdateCheck {
        id: id.to_owned(),
        status,
        current,
        latest: Some(commit),
        changes,
        error: None,
        scan,
        trust,
        source_change: change,
        moved_trust,
    })
}

/// The source's files as the last [`check_update`] (or install, or
/// [`update`]) left them in the cache clone, without contacting the
/// source: what the update preview compares with the store copy. `None`
/// for a skill without a git source or without a cache clone.
pub fn cached_latest(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    id: &str,
) -> Result<Option<SkillFiles>, CoreError> {
    let Some(source) = git_source(store, id)? else {
        return Ok(None);
    };
    let dir = sources_dir(sx).join(id);
    if !sx.fs.is_dir(&dir) {
        return Ok(None);
    }
    let path = source.path.as_deref().map(check_repo_path).transpose()?;
    let skill_dir = match &path {
        Some(path) => join_rel(&dir, path),
        None => dir.clone(),
    };
    if !sx.fs.exists(&skill_dir.join(SKILL_FILE)) {
        return Ok(None);
    }
    read_in_clone(sx.fs, &dir, &skill_dir).map(Some)
}

/// What [`update`] did.
#[derive(Debug, Clone)]
pub struct Updated {
    pub id: String,
    pub from: Option<String>,
    pub to: String,
    pub changes: Vec<FileChange>,
    /// `None` when the store copy was already current.
    pub commit: Option<CommitId>,
    /// Copies rewritten with the new content.
    pub report: SyncReport,
    /// The built-in scan of the new content (SPEC 17).
    pub scan: ScanReport,
    /// The external scanner's report the update was gated on (#27).
    pub external: Option<ExternalReport>,
}

/// Updates a git skill: the source's latest content replaces the store
/// copy in one commit (`skill: update <id> to <sha>`), then owned copies
/// follow ([`sync`]). A blocked source is refused (`source_blocked`)
/// before it is contacted; `source_unreachable` leaves everything as it
/// is; `not_found`/`invalid_value` for skills that are not git skills;
/// `scan_blocked` for content the decisions do not let through.
pub fn update(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    id: &str,
    decisions: &Decisions,
) -> Result<Updated, CoreError> {
    let Some(source) = git_source(store, id)? else {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("skill {id:?} has no git source to update from; add it again to replace it"),
        ));
    };
    if let Some(gated) = source_ref(&source) {
        sources::refuse_blocked(store, &gated)?;
    }
    let Latest {
        commit,
        files,
        change,
        moved_to,
    } = latest(sx, id, &source)?;
    if let (Some(gated), Some(change)) = (source_ref(&source), &change) {
        sources::lapse(store, &gated, change)?;
    }
    commit_update(store, sx, id, source, commit, files, moved_to, decisions)
}

/// Updates a git skill to `expected`, the source commit the user reviewed
/// (the app's update preview): the cache clone is used as the last check
/// left it, without fetching. When the clone is at another commit (a later
/// check saw a newer push), nothing is written and the error is
/// `entry_changed`: check again and review the new content.
pub fn update_reviewed(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    id: &str,
    expected: &str,
    decisions: &Decisions,
) -> Result<Updated, CoreError> {
    let Some(source) = git_source(store, id)? else {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("skill {id:?} has no git source to update from; add it again to replace it"),
        ));
    };
    let changed = || {
        CoreError::new(
            ErrorCode::EntryChanged,
            format!("the source of skill {id:?} changed since the check; check for updates again"),
        )
    };
    let dir = sources_dir(sx).join(id);
    if !sx.fs.exists(&dir.join(".git")) {
        return Err(changed());
    }
    let git = git_path(sx)?;
    let head = rev_parse(sx, &git, &dir, "HEAD")?;
    if !head.eq_ignore_ascii_case(expected.trim()) {
        return Err(changed());
    }
    let files = cached_latest(store, sx, id)?.ok_or_else(changed)?;
    let moved_to = source
        .url
        .as_deref()
        .and_then(|url| moved_marker(sx, &dir, url));
    commit_update(store, sx, id, source, head, files, moved_to, decisions)
}

/// Writes `files` (the source at `commit`) as the store copy of `id` in one
/// commit, then lets owned copies follow; nothing is written when the
/// decisions do not let the content through. The source and the
/// repository it moved to (`moved_to`) both pass the trust gate; then the
/// skill's URL follows the move, so the next check fetches from there.
#[allow(clippy::too_many_arguments)]
fn commit_update(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    id: &str,
    source: SkillSource,
    commit: String,
    files: SkillFiles,
    moved_to: Option<SourceRef>,
    decisions: &Decisions,
) -> Result<Updated, CoreError> {
    // Source trust first (#24), then the scan's gate.
    if let Some(url) = source.url.as_deref() {
        let requested = SourceRef::from_git_url(url)?;
        admit_with_move(store, Some(&requested), moved_to.as_ref(), decisions)?;
    }
    let scan = Scanner::new(sx.fs, sx.home).scan_files(&files);
    scan::gate(&scan, decisions.allow_dangerous)?;
    if let Some(report) = &decisions.external {
        if report.content_hash.as_deref() != Some(content_hash(&files).as_str()) {
            return Err(CoreError::new(
                ErrorCode::EntryChanged,
                format!(
                    "the source of skill {id:?} changed since the external scan; check for \
                     updates again"
                ),
            ));
        }
        external::gate(report, decisions.allow_dangerous)?;
    }
    let stored = store::read_content(store, id)?;
    let changes = changes(&stored, &files);
    let short = &commit[..commit.len().min(7)];
    let operation = Operation::new("skill.update", format!("skill: update {id} to {short}"))
        .entity(format!("skill/{id}"));
    let committed = store.transaction(operation, |tx| {
        store::stage_content(tx, id, &files)?;
        let mut meta = store::read_meta(tx, id)?.unwrap_or_else(|| SkillMeta::new(id));
        if let Some(source) = meta.source.as_mut() {
            source.commit = Some(commit.clone());
            if let Some(target) = &moved_to {
                source.url = Some(target.url().to_owned());
            }
        }
        let skill_md = files
            .get(SKILL_FILE)
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
        if let Some(name) = skill_md
            .and_then(|text| frontmatter::parse(&text).ok().flatten())
            .and_then(|fm| fm.name().map(str::to_owned))
        {
            meta.name = Some(name);
        }
        store::stage_meta(tx, &meta)
    })?;
    store::prune_empty_dirs(store, id)?;
    let report = sync(store, sx)?;
    Ok(Updated {
        id: id.to_owned(),
        from: source.commit,
        to: commit,
        changes,
        commit: committed.commit,
        report,
        scan,
        external: decisions.external.clone(),
    })
}

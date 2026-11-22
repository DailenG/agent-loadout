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
use crate::remote::git::{Invocation, RunOutput, locate};
use crate::skills::link::{SyncReport, sync};
use crate::skills::store::{self, content_dir, missing_skill_file, not_found};
use crate::skills::{
    Entry, MAX_ARCHIVE_FILE_BYTES, MAX_ARCHIVE_FILES, MAX_ARCHIVE_TOTAL_BYTES, SKILL_FILE,
    SkillFiles, SkillsEnv, check_name, confine, entry, frontmatter, io_error, read_folder, slug,
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

/// An install.
#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub source: Source,
    /// The skill folder inside a git source (overrides the URL's).
    pub path: Option<String>,
    /// Branch or tag of a git source (overrides the URL's).
    pub git_ref: Option<String>,
    pub naming: Naming,
}

impl InstallRequest {
    pub fn new(source: Source) -> Self {
        Self {
            source,
            path: None,
            git_ref: None,
            naming: Naming::default(),
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
}

/// The content to commit and where it came from.
struct Fetched {
    files: SkillFiles,
    /// Folder or archive name, repository name: the name when the
    /// frontmatter has none, and the commit message's `from`.
    base_name: String,
    from: String,
    source: SkillSource,
    /// The clone to keep as `cache/skill-sources/<id>`.
    clone: Option<PathBuf>,
}

/// Installs a skill into the store (one commit).
pub fn install(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    request: &InstallRequest,
) -> Result<Installed, CoreError> {
    let fetched = match &request.source {
        Source::Local(path) => fetch_local(sx.fs, path)?,
        Source::Git { url, git_ref, path } => {
            let git_ref = request.git_ref.clone().or_else(|| git_ref.clone());
            let path = request.path.clone().or_else(|| path.clone());
            fetch_git(sx, url, git_ref, path)?
        }
    };
    let result = commit_install(store, sx, &request.naming, &fetched);
    if let (Ok(installed), Some(clone)) = (&result, &fetched.clone) {
        keep_clone(sx, clone, &installed.id)?;
    } else if let Some(clone) = &fetched.clone {
        let _ = remove_dir(sx.fs, clone);
    }
    result
}

/// Installs already-read content (agents CLI vendoring, tool folder
/// imports). `source.kind` says where it came from.
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
    };
    commit_install(store, sx, naming, &fetched)
}

fn commit_install(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    naming: &Naming,
    fetched: &Fetched,
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
    let report = if enable_new {
        Some(sync(store, sx)?)
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
            extra: toml::Table::new(),
        },
        clone: None,
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
fn git_path(sx: &SkillsEnv<'_>) -> Result<PathBuf, CoreError> {
    locate(sx.env, &crate::fs::RealFs).ok_or_else(|| {
        CoreError::new(
            ErrorCode::ToolNotFound,
            "git is not installed or not on PATH; git skill sources need it",
        )
    })
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

fn run_git(
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
                .any(|var| name.eq_ignore_ascii_case(var))
        })
        .map(|(name, value)| (OsString::from(name), value.to_owned()))
        .collect();
    env.push(("GIT_TERMINAL_PROMPT".into(), "0".into()));
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

fn unreachable(url: &str, output: &RunOutput) -> CoreError {
    CoreError::new(
        ErrorCode::SourceUnreachable,
        format!(
            "the source {url} cannot be reached: {}",
            git_message(output)
        ),
    )
}

fn git_failed(what: &str, output: &RunOutput) -> CoreError {
    CoreError::new(
        ErrorCode::GitError,
        format!("{what}: {}", git_message(output)),
    )
}

/// A relative `/`-separated path inside a repository.
fn check_repo_path(path: &str) -> Result<String, CoreError> {
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

fn check_ref(git_ref: &str) -> Result<(), CoreError> {
    let valid = !git_ref.is_empty()
        && !git_ref.starts_with('-')
        && !git_ref.contains("..")
        && git_ref
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b));
    if valid {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{git_ref:?} is not a branch or tag name"),
        ))
    }
}

fn check_url(url: &str) -> Result<(), CoreError> {
    if url.is_empty() || url.starts_with('-') || url.contains(['\n', '\r']) {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{url:?} is not a git URL"),
        ));
    }
    Ok(())
}

fn is_remote_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    ["https://", "http://", "ssh://", "git://"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
        || is_scp_like(url)
}

/// Clones `url` into `dir` (which must not exist): sparse to `path` when
/// given. Returns the commit checked out and the branch followed.
fn clone(
    sx: &SkillsEnv<'_>,
    git: &Path,
    url: &str,
    git_ref: Option<&str>,
    path: Option<&str>,
    dir: &Path,
) -> Result<(String, Option<String>), CoreError> {
    check_url(url)?;
    if let Some(git_ref) = git_ref {
        check_ref(git_ref)?;
    }
    let parent = dir.parent().expect("a clone folder has a parent");
    confine(sx.fs, dir)?;
    sx.fs
        .create_dir_all(parent)
        .map_err(|err| io_error("cannot create directory", parent, &err))?;
    let mut args: Vec<&OsStr> = vec![OsStr::new("clone"), OsStr::new("--quiet")];
    if path.is_some() {
        args.push(OsStr::new("--no-checkout"));
    }
    if is_remote_url(url) {
        args.extend([OsStr::new("--depth"), OsStr::new("1")]);
    }
    if let Some(git_ref) = git_ref {
        args.extend([OsStr::new("--branch"), OsStr::new(git_ref)]);
    }
    args.extend([OsStr::new("--"), OsStr::new(url), dir.as_os_str()]);
    let output = run_git(sx, git, parent, &args)?;
    if !output.success {
        let _ = remove_dir(sx.fs, dir);
        return Err(unreachable(url, &output));
    }
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
    let commit = rev_parse(sx, git, dir, "HEAD")?;
    let branch = match git_ref {
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
    Ok((commit, branch))
}

fn rev_parse(sx: &SkillsEnv<'_>, git: &Path, dir: &Path, rev: &str) -> Result<String, CoreError> {
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
) -> Result<Fetched, CoreError> {
    let git = git_path(sx)?;
    let path = path.as_deref().map(check_repo_path).transpose()?;
    let temp = sources_dir(sx).join(format!(
        ".tmp-{}",
        ids::new_ulid(sx.env.now())?.to_ascii_lowercase()
    ));
    let (commit, branch) = clone(sx, &git, url, git_ref.as_deref(), path.as_deref(), &temp)?;
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
            None => match find_skill_dirs(sx.fs, &temp)?.as_slice() {
                [] => Err(missing_skill_file(&temp)),
                [one] => Ok((
                    read_in_clone(sx.fs, &temp, &join_rel(&temp, one))?,
                    Some(one.clone()),
                )),
                many => Err(several(many)),
            },
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
            git_ref: branch,
            commit: Some(commit),
            extra: toml::Table::new(),
        },
        clone: Some(temp),
    })
}

fn join_rel(base: &Path, relative: &str) -> PathBuf {
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

fn remove_dir(fs: &dyn Fs, dir: &Path) -> Result<(), CoreError> {
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

/// One file that differs between the store copy and the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
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
}

impl UpdateStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            UpdateStatus::UpToDate => "up_to_date",
            UpdateStatus::Available => "available",
            UpdateStatus::NotTracked => "not_tracked",
            UpdateStatus::Unreachable => "source_unreachable",
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
}

/// The latest content of a git skill: the cache clone fetched (re-cloned
/// when missing) and checked out at the source's head.
fn latest(
    sx: &SkillsEnv<'_>,
    id: &str,
    source: &SkillSource,
) -> Result<(String, SkillFiles), CoreError> {
    let url = source
        .url
        .as_deref()
        .ok_or_else(|| CoreError::new(ErrorCode::MissingField, "the git source has no url"))?;
    let git = git_path(sx)?;
    let path = source.path.as_deref().map(check_repo_path).transpose()?;
    let dir = sources_dir(sx).join(id);
    let commit = if sx.fs.exists(&dir.join(".git")) {
        let mut args: Vec<&OsStr> = vec![OsStr::new("fetch"), OsStr::new("--quiet")];
        if is_remote_url(url) {
            args.extend([OsStr::new("--depth"), OsStr::new("1")]);
        }
        args.extend([OsStr::new("--"), OsStr::new(url)]);
        if let Some(git_ref) = &source.git_ref {
            check_ref(git_ref)?;
            args.push(OsStr::new(git_ref));
        }
        let output = run_git(sx, &git, &dir, &args)?;
        if !output.success {
            return Err(unreachable(url, &output));
        }
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
        commit
    } else {
        remove_dir(sx.fs, &dir)?;
        clone(
            sx,
            &git,
            url,
            source.git_ref.as_deref(),
            path.as_deref(),
            &dir,
        )?
        .0
    };
    let skill_dir = match &path {
        Some(path) => join_rel(&dir, path),
        None => dir.clone(),
    };
    if !sx.fs.exists(&skill_dir.join(SKILL_FILE)) {
        return Err(CoreError::new(
            ErrorCode::SourceUnreachable,
            format!("the source {url} no longer holds the skill"),
        ));
    }
    Ok((commit, read_in_clone(sx.fs, &dir, &skill_dir)?))
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
        match old.get(path) {
            None => out.push(FileChange {
                path: path.clone(),
                kind: ChangeKind::Added,
            }),
            Some(before) if before != bytes => out.push(FileChange {
                path: path.clone(),
                kind: ChangeKind::Modified,
            }),
            Some(_) => {}
        }
    }
    for path in old.keys() {
        if !new.contains_key(path) {
            out.push(FileChange {
                path: path.clone(),
                kind: ChangeKind::Removed,
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
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
/// clone; an unreachable source is reported, not an error.
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
        });
    };
    let current = source.commit.clone();
    let (commit, files) = match latest(sx, id, &source) {
        Ok(latest) => latest,
        Err(err) if err.code() == ErrorCode::SourceUnreachable => {
            return Ok(UpdateCheck {
                id: id.to_owned(),
                status: UpdateStatus::Unreachable,
                current,
                latest: None,
                changes: Vec::new(),
                error: Some(err),
            });
        }
        Err(err) => return Err(err),
    };
    let stored = store::read_content(store, id)?;
    let changes = changes(&stored, &files);
    let status = if changes.is_empty() && current.as_deref() == Some(commit.as_str()) {
        UpdateStatus::UpToDate
    } else {
        UpdateStatus::Available
    };
    Ok(UpdateCheck {
        id: id.to_owned(),
        status,
        current,
        latest: Some(commit),
        changes,
        error: None,
    })
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
}

/// Updates a git skill: the source's latest content replaces the store
/// copy in one commit (`skill: update <id> to <sha>`), then owned copies
/// follow ([`sync`]). `source_unreachable` leaves everything as it is;
/// `not_found`/`invalid_value` for skills that are not git skills.
pub fn update(store: &Store<'_>, sx: &SkillsEnv<'_>, id: &str) -> Result<Updated, CoreError> {
    let Some(source) = git_source(store, id)? else {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("skill {id:?} has no git source to update from; add it again to replace it"),
        ));
    };
    let (commit, files) = latest(sx, id, &source)?;
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
    })
}

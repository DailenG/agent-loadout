//! MCP servers from a Git repository or a local folder (#5).
//!
//! [`resolve`] turns a source into **drafts**: proposed store servers read
//! from the folder's files (`server.json` of the MCP Registry, an MCPB
//! manifest, committed client configs, or the folder as a package). It
//! runs nothing: the system git clones shallow at the ref (no submodules,
//! no hooks from the repository) into `cache/mcp-sources/.tmp-<ulid>`, the
//! files are read (dependency and build folders left out, [`SKIP_DIRS`])
//! and scanned (M40), and the clone is removed when the [`Resolved`] is
//! dropped ([`sweep_temp_clones`] removes what a crash left). [`save`]
//! stores the drafts the user picked and edited in one commit, after the
//! source trust gate (M39) and the scan gate; a draft that runs from the
//! repository's files keeps the clone as the machine-local
//! `<LOADOUT_HOME>/servers/<id>` (never synced), placed under the store
//! lock and marked with the commit it holds.
//!
//! Only provenance goes into the synced server file (`[meta.source]`), so
//! [`check_update`] can read the source again and show what changed. A
//! server runs only where its code is what the file names ([`runnable`]):
//! a checkout at another commit, or a folder of another machine, blocks its
//! entries. Secrets: a declared secret becomes a `${secret:<alias>}`
//! reference for the user to set; a literal the classifier flags is
//! replaced by a reference and its value dropped: no value is kept or
//! returned. Every check covers the base definition and each `[os.*]`
//! override alike.

mod convert;
pub mod snippet;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::apply::{ApplyEnv, ApplyReport, Touched, instant_apply};
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::ids;
use crate::model::server::{Meta, Server, ServerFields, ServerSource, ServerSourceKind};
use crate::paths::STATE_DIR;
use crate::scan::{self, ScanReport, Scanner};
use crate::secrets::catalog::Catalog;
use crate::secrets::extract::{Aliases, Extractor};
use crate::skills::install::{
    self, ChangeKind, Decisions, FileChange, Source, UpdateStatus, check_ref, check_repo_path,
    git_path, is_remote_url, join_rel, looks_executable, remove_dir, rev_parse, run_git,
};
use crate::skills::{SkillFiles, SkillsEnv, confine, content_hash, io_error, read_folder_skipping};
use crate::sources::{self, SourceChange, SourceChangeKind, SourceRef, TrustCheck};
use crate::store::{CommitId, Machine, Operation, Store, server_path};

use convert::{CHECKOUT_TOKEN, Ctx, GitPin, ID_TOKEN, IMAGE_PREFIX, RawDraft};

/// Machine-local clones servers run from: `<LOADOUT_HOME>/servers/<id>`.
pub const CHECKOUTS_DIR: &str = "servers";
/// Temporary clones of a resolve, below `<LOADOUT_HOME>/cache`.
pub const TEMP_DIR: &str = "mcp-sources";
/// `meta.origin` of a server drafted from a git source.
pub const ORIGIN_GIT: &str = "source:git";
/// `meta.origin` of a server drafted from a local folder.
pub const ORIGIN_LOCAL: &str = "source:local";
/// Dependency and build folders a source read leaves out (scan, size cap,
/// diffs): installed or built on this machine, not part of the source.
pub const SKIP_DIRS: [&str; 8] = [
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    ".git",
    "target",
    "dist",
    "build",
];
/// The checkout's marker, inside its `.git`: the commit and URL it holds.
const MARKER_FILE: &str = "loadout-checkout";
/// A checkout moved aside while its replacement is committed.
const ASIDE_PREFIX: &str = ".old-";

/// A store id from a server or entry name: lower case, other characters
/// become `-`, at most 64 characters.
pub fn id_from_name(name: &str) -> String {
    let mut id: String = name
        .trim()
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    id = id.trim_matches(|c| matches!(c, '-' | '.' | '_')).to_owned();
    id.truncate(64);
    if id.is_empty() {
        "server".to_owned()
    } else {
        id
    }
}

/// Which converter made a draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DraftKind {
    /// `server.json` (MCP Registry).
    Registry,
    /// An MCPB/DXT `manifest.json`.
    Bundle,
    /// A committed client config.
    ClientConfig,
    /// The folder is the server (`package.json`, `pyproject.toml`,
    /// `Dockerfile`).
    Package,
}

impl DraftKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            DraftKind::Registry => "registry",
            DraftKind::Bundle => "bundle",
            DraftKind::ClientConfig => "client_config",
            DraftKind::Package => "package",
        }
    }
}

/// A value the user provides: a secret to set under `alias`, or a
/// variable the draft reads from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftInput {
    /// `env.NAME`, `headers.NAME`, `args[N]` (1-based), `url`, `command`,
    /// each also as `os.<name>.<field>` for an override, or
    /// `client.<id>.<key>` for a native passthrough value.
    pub field: String,
    pub description: Option<String>,
    /// The draft holds `${secret:<alias>}` (or, for a passthrough value,
    /// the value was left out).
    pub secret: bool,
    pub required: bool,
    pub alias: Option<String>,
    /// The file held a literal value here; it was replaced by the
    /// reference (or left out) and not kept.
    pub value_dropped: bool,
}

/// One launch line: the base definition (`os` `None`) or an `[os.<name>]`
/// override merged over it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchLine {
    pub os: Option<String>,
    /// The command and arguments joined by spaces, or the URL.
    pub line: String,
}

/// One proposed server.
#[derive(Debug, Clone)]
pub struct Draft {
    /// Names the draft in [`Pick::key`]: `<file>#<entry>` or `<file>`.
    pub key: String,
    pub kind: DraftKind,
    /// The proposed store server: `meta.origin` and `meta.source` set, no
    /// enablement.
    pub server: Server,
    pub inputs: Vec<DraftInput>,
    /// It runs code from the source (the folder's files, or the
    /// repository as a package) every time a client starts it.
    pub runs_repo_code: bool,
    /// It runs from `<LOADOUT_HOME>/servers/<id>` (git sources).
    pub checkout: bool,
    /// The command names a version or commit; `false` for a registry
    /// package without a concrete version (the scan flags it too).
    pub pinned: bool,
    /// Steps the user runs before it works (`docker build ...`); never run
    /// by Loadout. They may hold `${home}` and `${loadout_home}`
    /// ([`setup_text`]).
    pub setup: Vec<String>,
    /// Other files that held an identical definition.
    pub also_in: Vec<String>,
    /// The folder's files and every launch line of this draft.
    pub scan: ScanReport,
}

impl Draft {
    /// The draft's provenance.
    pub fn provenance(&self) -> &ServerSource {
        self.server
            .body
            .meta
            .as_ref()
            .and_then(|meta| meta.source.as_ref())
            .expect("a draft has its source")
    }
}

/// A definition the folder holds that cannot be drafted.
#[derive(Debug, Clone)]
pub struct Skipped {
    pub file: String,
    /// The entry inside the file, when it holds several.
    pub entry: Option<String>,
    pub error: CoreError,
}

/// What to resolve.
#[derive(Debug, Clone)]
pub struct ResolveRequest {
    pub source: Source,
    /// Branch, tag or commit of a git source (overrides the URL's).
    pub git_ref: Option<String>,
    /// The folder inside a git source (overrides the URL's).
    pub path: Option<String>,
}

impl ResolveRequest {
    pub fn new(source: Source) -> Self {
        Self {
            source,
            git_ref: None,
            path: None,
        }
    }
}

/// A resolved source waiting for the user's picks ([`resolve`], then
/// [`save`]). The temporary clone is removed when this is dropped without
/// a save that keeps it.
#[derive(Debug)]
pub struct Resolved {
    base: ServerSource,
    drafts: Vec<Draft>,
    skipped: Vec<Skipped>,
    skipped_folders: Vec<String>,
    files: SkillFiles,
    files_scan: ScanReport,
    trust_source: Option<SourceRef>,
    moved_source: Option<SourceRef>,
    source_change: Option<SourceChange>,
    /// The temporary clone (git sources).
    clone: Option<PathBuf>,
    spent: bool,
}

impl Resolved {
    pub fn drafts(&self) -> &[Draft] {
        &self.drafts
    }

    pub fn skipped(&self) -> &[Skipped] {
        &self.skipped
    }

    /// Dependency and build folders left out of the read and the scan
    /// (relative paths, [`SKIP_DIRS`]).
    pub fn skipped_folders(&self) -> &[String] {
        &self.skipped_folders
    }

    /// Where the drafts come from (`file` and `converter` empty).
    pub fn source(&self) -> &ServerSource {
        &self.base
    }

    /// The scan of the folder's files.
    pub fn files_scan(&self) -> &ScanReport {
        &self.files_scan
    }

    /// The source the trust gate admits with [`Decisions::trust`]: the
    /// repository the URL names, or the local folder.
    pub fn trust_source(&self) -> Option<&SourceRef> {
        self.trust_source.as_ref()
    }

    /// Where the host redirected the repository (another key): gated too,
    /// with [`Decisions::moved_trust`].
    pub fn moved_source(&self) -> Option<&SourceRef> {
        self.moved_source.as_ref()
    }

    /// The repository the URL names moved.
    pub fn source_change(&self) -> Option<&SourceChange> {
        self.source_change.as_ref()
    }

    /// The trust gate (#24): the requested source with the user's answer
    /// for it, and a redirect target with the answer for that one. A
    /// redirect only ever adds a question.
    fn admit(&self, store: &Store<'_>, decisions: &Decisions) -> Result<(), CoreError> {
        if let Some(source) = &self.trust_source {
            sources::admit(store, source, decisions.trust, decisions.via)?;
        }
        if let Some(moved) = &self.moved_source {
            sources::admit(store, moved, decisions.moved_trust, decisions.via)?;
        }
        Ok(())
    }
}

impl Drop for Resolved {
    fn drop(&mut self) {
        if let Some(clone) = self.clone.take() {
            // Below `LOADOUT_HOME/cache`; a leftover is swept later.
            let _ = std::fs::remove_dir_all(clone);
        }
    }
}

/// What [`fetch`] read.
struct Fetched {
    base: ServerSource,
    files: SkillFiles,
    skipped_folders: Vec<String>,
    /// How commands name the folder.
    root: String,
    pin: Option<GitPin>,
    name: String,
    clone: Option<PathBuf>,
    trust_source: SourceRef,
    moved_source: Option<SourceRef>,
    source_change: Option<SourceChange>,
}

/// `<LOADOUT_HOME>/servers/<id>`.
pub fn checkout_dir(sx: &SkillsEnv<'_>, id: &str) -> PathBuf {
    sx.home.path.join(CHECKOUTS_DIR).join(id)
}

/// A ref that names a commit (7 to 40 hex characters).
fn is_commit_ref(git_ref: &str) -> bool {
    (7..=40).contains(&git_ref.len()) && git_ref.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The URL kept in the store (and in a checkout's `.git/config`): a
/// password or token in an `http(s)` URL is removed; ssh users (`git@`)
/// stay, the clone needs them.
fn provenance_url(url: &str) -> String {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    for scheme in ["https://", "http://"] {
        if lower.starts_with(scheme) {
            let rest = &url[scheme.len()..];
            let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
            if let Some((_, host)) = authority.rsplit_once('@') {
                return format!("{}{host}/{path}", &url[..scheme.len()]);
            }
        }
    }
    url.to_owned()
}

/// A folder as commands name it: `${home}/...` below the home folder, else
/// the absolute path, `/`-separated.
fn folder_template(sx: &SkillsEnv<'_>, folder: &Path) -> String {
    let home = sx.env.home();
    let text = match folder.strip_prefix(home) {
        Ok(rest) if !rest.as_os_str().is_empty() => {
            format!("${{home}}/{}", rest.to_string_lossy())
        }
        _ => folder.to_string_lossy().into_owned(),
    };
    convert::escape(&text.replace('\\', "/")).replace("$${home}", "${home}")
}

/// The folder a [`folder_template`] names.
fn folder_of(sx: &SkillsEnv<'_>, template: &str) -> PathBuf {
    let text = template.replace("$${", "${");
    match text.strip_prefix("${home}/") {
        Some(rest) => rest
            .split('/')
            .fold(sx.env.home().to_path_buf(), |path, part| path.join(part)),
        None => PathBuf::from(text),
    }
}

fn name_of(text: &str) -> String {
    text.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\', ':'])
        .next()
        .unwrap_or("server")
        .trim_end_matches(".git")
        .to_owned()
}

/// A setup step for a person to run: `${loadout_home}` and `${home}` (store
/// templates, not shell variables) written as `loadout_home` and `home`.
pub fn setup_text(step: &str, home: &str, loadout_home: &str) -> String {
    step.replace("${loadout_home}", loadout_home)
        .replace("${home}", home)
        .replace("$${", "${")
}

/// Reads a source folder ([`SKIP_DIRS`] left out).
fn read_source(fs: &dyn Fs, dir: &Path) -> Result<(SkillFiles, Vec<String>), CoreError> {
    read_folder_skipping(fs, dir, &SKIP_DIRS)
}

/// Reads a folder inside a clone, refused when it (or a link on its way)
/// resolves outside the clone.
fn read_in(
    sx: &SkillsEnv<'_>,
    clone: &Path,
    dir: &Path,
) -> Result<(SkillFiles, Vec<String>), CoreError> {
    let resolve = |path: &Path| {
        sx.fs
            .canonicalize(path)
            .map_err(|err| io_error("cannot resolve folder", path, &err))
    };
    if !resolve(dir)?.starts_with(resolve(clone)?) {
        return Err(CoreError::new(
            ErrorCode::InvalidPath,
            "the folder points outside the repository",
        )
        .with_path(dir));
    }
    read_source(sx.fs, dir)
}

/// Clones `url` into a new temporary folder at `git_ref` (a branch, tag or
/// commit), sparse to `path`, and points its `origin` at the URL without
/// credentials. Returns the clone, the commit, the branch followed and
/// where the host redirected the URL.
fn clone_at(
    sx: &SkillsEnv<'_>,
    url: &str,
    git_ref: Option<&str>,
    path: Option<&str>,
) -> Result<(PathBuf, String, Option<String>, Option<String>), CoreError> {
    let git = git_path(sx)?;
    let temp = sx.home.cache_dir().join(TEMP_DIR).join(format!(
        ".tmp-{}",
        ids::new_ulid(sx.env.now())?.to_ascii_lowercase()
    ));
    let commit_ref = git_ref.filter(|r| is_commit_ref(r));
    let branch_ref = git_ref.filter(|r| !is_commit_ref(r));
    let cloned = install::clone(sx, &git, url, branch_ref, path, &temp)?;
    let run = |args: &[&str]| {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        run_git(sx, &git, &temp, &args)
    };
    let fail = |err: CoreError| {
        let _ = remove_dir(sx.fs, &temp);
        err
    };
    // A token in the URL never stays in the clone's `.git/config`, which
    // a checkout keeps for good.
    let clean = provenance_url(url);
    let output = run(&["remote", "set-url", "origin", &clean]).map_err(fail)?;
    if !output.success {
        return Err(fail(CoreError::new(
            ErrorCode::GitError,
            "cannot set the clone's remote URL",
        )));
    }
    let mut commit = cloned.commit;
    if let Some(wanted) = commit_ref {
        check_ref(wanted).map_err(fail)?;
        let mut output = run(&["checkout", "--quiet", "--detach", wanted]).map_err(fail)?;
        if !output.success && is_remote_url(url) {
            let fetched = run(&["fetch", "--quiet", "--depth", "1", url, wanted]).map_err(fail)?;
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
        commit = rev_parse(sx, &git, &temp, "HEAD").map_err(fail)?;
    }
    let branch = if commit_ref.is_some() {
        None
    } else {
        cloned.branch
    };
    Ok((temp, commit, branch, cloned.redirect))
}

fn fetch(sx: &SkillsEnv<'_>, request: &ResolveRequest) -> Result<Fetched, CoreError> {
    match &request.source {
        Source::Local(folder) => {
            if request.git_ref.is_some() || request.path.is_some() {
                return Err(CoreError::new(
                    ErrorCode::InvalidValue,
                    "a ref and a path apply to git sources only",
                ));
            }
            if !sx.fs.is_dir(folder) {
                return Err(
                    CoreError::new(ErrorCode::NotFound, "the folder does not exist")
                        .with_path(folder),
                );
            }
            let root = folder_template(sx, folder);
            let (files, skipped_folders) = read_source(sx.fs, folder)?;
            Ok(Fetched {
                base: ServerSource {
                    kind: ServerSourceKind::Local,
                    url: None,
                    folder: Some(root.clone()),
                    path: None,
                    git_ref: None,
                    commit: None,
                    file: String::new(),
                    converter: String::new(),
                    entry: None,
                    checkout: None,
                    machine: None,
                    extra: toml::Table::new(),
                },
                files,
                skipped_folders,
                root,
                pin: None,
                name: name_of(&folder.to_string_lossy()),
                clone: None,
                trust_source: SourceRef::from_local_path(folder)?,
                moved_source: None,
                source_change: None,
            })
        }
        Source::Git { url, git_ref, path } => {
            let requested = SourceRef::from_git_url(url)?;
            let git_ref = request.git_ref.clone().or_else(|| git_ref.clone());
            let path = request
                .path
                .clone()
                .or_else(|| path.clone())
                .map(|path| check_repo_path(&path))
                .transpose()?;
            let (clone, commit, branch, redirect) =
                clone_at(sx, url, git_ref.as_deref(), path.as_deref())?;
            let read = || -> Result<(SkillFiles, Vec<String>), CoreError> {
                let dir = match &path {
                    Some(path) => join_rel(&clone, path),
                    None => clone.clone(),
                };
                if !sx.fs.is_dir(&dir) {
                    return Err(CoreError::new(
                        ErrorCode::NotFound,
                        format!(
                            "the repository has no folder {:?}",
                            path.as_deref().unwrap_or("")
                        ),
                    ));
                }
                read_in(sx, &clone, &dir)
            };
            let (files, skipped_folders) = match read() {
                Ok(read) => read,
                Err(err) => {
                    let _ = remove_dir(sx.fs, &clone);
                    return Err(err);
                }
            };
            let moved_to = redirect
                .as_deref()
                .and_then(|target| SourceRef::from_git_url(target).ok())
                .filter(|target| target.key() != requested.key());
            let source_change = moved_to.as_ref().map(|target| SourceChange {
                kind: SourceChangeKind::Moved,
                detail: Some(target.key().to_owned()),
            });
            let url = provenance_url(url);
            let followed = match git_ref.as_deref() {
                Some(r) if is_commit_ref(r) => Some(r.to_owned()),
                _ => branch,
            };
            let name = path
                .as_deref()
                .and_then(|path| path.rsplit('/').next())
                .map_or_else(|| name_of(&url), str::to_owned);
            Ok(Fetched {
                base: ServerSource {
                    kind: ServerSourceKind::Git,
                    url: Some(url.clone()),
                    folder: None,
                    path: path.clone(),
                    git_ref: followed,
                    commit: Some(commit.clone()),
                    file: String::new(),
                    converter: String::new(),
                    entry: None,
                    checkout: None,
                    machine: None,
                    extra: toml::Table::new(),
                },
                files,
                skipped_folders,
                root: CHECKOUT_TOKEN.to_owned(),
                pin: Some(GitPin { url, commit, path }),
                name,
                clone: Some(clone),
                trust_source: requested,
                moved_source: moved_to,
                source_change,
            })
        }
    }
}

/// `${loadout_home}/servers/<id>[/<path>]`.
fn checkout_root(id: &str, path: Option<&str>) -> String {
    match path {
        Some(path) => format!("${{loadout_home}}/{CHECKOUTS_DIR}/{id}/{path}"),
        None => format!("${{loadout_home}}/{CHECKOUTS_DIR}/{id}"),
    }
}

/// Every launch string of `fields` through `edit`.
fn edit_fields(fields: &mut ServerFields, edit: &dyn Fn(&str) -> String) {
    let apply = |value: &mut String| *value = edit(value);
    fields.command.iter_mut().for_each(apply);
    fields.cwd.iter_mut().for_each(apply);
    fields.url.iter_mut().for_each(apply);
    fields.args.iter_mut().flatten().for_each(apply);
    fields
        .env
        .iter_mut()
        .flatten()
        .for_each(|(_, value)| apply(value));
    fields
        .headers
        .iter_mut()
        .flatten()
        .for_each(|(_, value)| apply(value));
}

/// Every launch string of `server`, base and overrides, through `edit`.
fn edit_server(server: &mut Server, edit: &dyn Fn(&str) -> String) {
    let mut fields = server.body.fields();
    edit_fields(&mut fields, edit);
    set_launch(server, &fields);
    for over in server.body.os.iter_mut().flatten().map(|(_, over)| over) {
        edit_fields(&mut over.fields, edit);
    }
}

/// Writes the launch fields of `fields` into `server`.
fn set_launch(server: &mut Server, fields: &ServerFields) {
    let body = &mut server.body;
    body.transport = fields.transport;
    body.command.clone_from(&fields.command);
    body.args.clone_from(&fields.args);
    body.cwd.clone_from(&fields.cwd);
    body.url.clone_from(&fields.url);
    body.headers.clone_from(&fields.headers);
    body.env.clone_from(&fields.env);
}

/// `value` with each whole `from` (followed by the end, `/` or `:`)
/// replaced by `to`.
fn replace_whole(value: &str, from: &str, to: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(at) = rest.find(from) {
        let after = &rest[at + from.len()..];
        out.push_str(&rest[..at]);
        if after.is_empty() || after.starts_with(['/', ':']) {
            out.push_str(to);
        } else {
            out.push_str(from);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Points what a draft names by its id (its checkout folder, its local
/// image tag) at `to` instead of `from`.
fn rename_id(server: &mut Server, from: &str, to: &str) {
    if from == to {
        return;
    }
    let pairs = [
        (
            format!("${{loadout_home}}/{CHECKOUTS_DIR}/{from}"),
            format!("${{loadout_home}}/{CHECKOUTS_DIR}/{to}"),
        ),
        (
            format!("{IMAGE_PREFIX}{from}"),
            format!("{IMAGE_PREFIX}{to}"),
        ),
    ];
    edit_server(server, &|value| {
        pairs.iter().fold(value.to_owned(), |value, (old, new)| {
            replace_whole(&value, old, new)
        })
    });
}

fn rename_steps(steps: &[String], from: &str, to: &str) -> Vec<String> {
    let mut server = Server::new(to.to_owned());
    server.body.args = Some(steps.to_vec());
    rename_id(&mut server, from, to);
    server.body.args.unwrap_or_default()
}

fn line_of(fields: &ServerFields) -> String {
    match (&fields.url, &fields.command) {
        (Some(url), _) => url.clone(),
        (None, Some(command)) => std::iter::once(command)
            .chain(fields.args.iter().flatten())
            .cloned()
            .collect::<Vec<_>>()
            .join(" "),
        (None, None) => String::new(),
    }
}

/// The launch fields as each OS sees them: the base, then each
/// `[os.<name>]` override merged over it.
fn effective(server: &Server) -> Vec<(Option<String>, ServerFields)> {
    let base = server.body.fields();
    let mut out = vec![(None, base.clone())];
    for (name, over) in server.body.os.iter().flatten() {
        out.push((Some(name.clone()), base.merged(&over.fields)));
    }
    out
}

/// Every launch line of `server`: the base and each OS override.
pub fn launch_lines(server: &Server) -> Vec<LaunchLine> {
    effective(server)
        .into_iter()
        .map(|(os, fields)| LaunchLine {
            os,
            line: line_of(&fields),
        })
        .collect()
}

/// The scan of every command line `server` can run, base and overrides.
fn scan_commands(scanner: &Scanner<'_>, server: &Server) -> ScanReport {
    let mut lines = effective(server)
        .into_iter()
        .map(|(_, fields)| match &fields.command {
            Some(command) => scanner.scan_command(command, &fields.args.unwrap_or_default()),
            None => scanner.scan_command(&fields.url.unwrap_or_default(), &[]),
        });
    let mut report = lines.next().expect("the base definition");
    for other in lines {
        report.merge(other);
    }
    report
}

fn store_ids(store: Option<&Store<'_>>) -> Result<BTreeSet<String>, CoreError> {
    Ok(match store {
        Some(store) => store.servers()?.into_iter().map(|entry| entry.id).collect(),
        None => BTreeSet::new(),
    })
}

fn aliases(store: Option<&Store<'_>>, sx: &SkillsEnv<'_>) -> Result<Aliases, CoreError> {
    let Some(store) = store else {
        return Ok(Aliases::default());
    };
    let machine = Machine::load(sx.fs, &sx.home.state_dir())?;
    let catalog = Catalog::load(store, machine.as_ref().map(|m| m.id.as_str()))?;
    Ok(Aliases::new(
        catalog.iter().map(|alias| alias.alias.clone()),
    ))
}

/// `base`, or `base-2`, `base-3`, ... free of `taken`.
fn free_id(base: &str, taken: &BTreeSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_owned();
    }
    let stem: String = base.chars().take(60).collect();
    (2..)
        .map(|n| format!("{stem}-{n}"))
        .find(|id| !taken.contains(id))
        .expect("a free id")
}

/// Replaces the literals of `fields` the classifier flags by references
/// (the values are dropped), listing each as an input named
/// `<prefix><field>`.
fn extract(
    fields: &mut ServerFields,
    prefix: &str,
    id: &str,
    aliases: &mut Aliases,
    inputs: &mut Vec<DraftInput>,
) -> Result<(), CoreError> {
    let mut extractor = Extractor::new(id, aliases);
    if let Some(args) = fields.args.take() {
        fields.args = Some(extractor.args(args));
    }
    if let Some(url) = fields.url.take() {
        fields.url = Some(extractor.url(url)?);
    }
    if let Some(env) = fields.env.take() {
        fields.env = Some(extractor.map("env", env));
    }
    if let Some(headers) = fields.headers.take() {
        fields.headers = Some(extractor.map("headers", headers));
    }
    for extracted in extractor.finish() {
        let extraction = extracted.extraction;
        // The value (`extracted.value`) is dropped here.
        inputs.push(DraftInput {
            field: format!("{prefix}{}", extraction.field),
            description: None,
            secret: true,
            required: true,
            alias: Some(extraction.alias),
            value_dropped: true,
        });
    }
    Ok(())
}

/// Renames the provisional secret references of `raw` to free aliases of
/// server `id`, then replaces every other literal the classifier flags, in
/// the base definition and in each override, by a reference.
fn secure(raw: &mut RawDraft, id: &str, aliases: &mut Aliases) -> Result<(), CoreError> {
    let mut renames = BTreeMap::new();
    for input in raw.inputs.iter_mut().filter(|input| input.secret) {
        if let Some(key) = input.alias.clone() {
            let alias = renames
                .entry(key)
                .or_insert_with_key(|key| aliases.new_alias(id, key))
                .clone();
            input.alias = Some(alias);
        }
    }
    if !renames.is_empty() {
        let edit = |value: &str| {
            let mut out = value.to_owned();
            for (key, alias) in &renames {
                out = out.replace(
                    &format!("${{secret:{key}}}"),
                    &format!("${{secret:{alias}}}"),
                );
            }
            out
        };
        edit_fields(&mut raw.fields, &edit);
        for over in raw.os.iter_mut().flatten().map(|(_, over)| over) {
            edit_fields(&mut over.fields, &edit);
        }
    }
    extract(&mut raw.fields, "", id, aliases, &mut raw.inputs)?;
    for (name, over) in raw.os.iter_mut().flatten() {
        let prefix = format!("os.{name}.");
        extract(&mut over.fields, &prefix, id, aliases, &mut raw.inputs)?;
    }
    Ok(())
}

/// Resolves a source into drafts: fetches (a git source into a temporary
/// clone), converts and scans, without touching the store. `store` (when
/// there is one) keeps ids and secret aliases free of those in use, and a
/// source it blocks is refused before it is contacted.
pub fn resolve(
    store: Option<&Store<'_>>,
    sx: &SkillsEnv<'_>,
    request: &ResolveRequest,
) -> Result<Resolved, CoreError> {
    if let Some(store) = store {
        // A blocked source is never contacted (#24).
        let requested = match &request.source {
            Source::Git { url, .. } => SourceRef::from_git_url(url)?,
            Source::Local(folder) => SourceRef::from_local_path(folder)?,
        };
        sources::refuse_blocked(store, &requested)?;
    }
    let fetched = fetch(sx, request)?;
    build(store, sx, fetched)
}

fn build(
    store: Option<&Store<'_>>,
    sx: &SkillsEnv<'_>,
    fetched: Fetched,
) -> Result<Resolved, CoreError> {
    let Fetched {
        base,
        files,
        skipped_folders,
        root,
        pin,
        name,
        clone,
        trust_source,
        moved_source,
        source_change,
    } = fetched;
    let ctx = Ctx {
        files: &files,
        skipped: &skipped_folders,
        root: root.clone(),
        git: pin.as_ref(),
        os: sx.env.os(),
        name,
        content_hash: content_hash(&files),
    };
    let (raws, mut skipped) = convert::all(&ctx);
    // Identical definitions collapse into the first.
    let mut kept: Vec<(RawDraft, Vec<String>)> = Vec::new();
    for raw in raws {
        match kept
            .iter_mut()
            .find(|(first, _)| first.fields == raw.fields && first.os == raw.os)
        {
            Some((_, also_in)) => also_in.push(raw.file),
            None => kept.push((raw, Vec::new())),
        }
    }
    let mut taken = store_ids(store)?;
    let mut aliases = aliases(store, sx)?;
    let scanner = Scanner::new(sx.fs, sx.home);
    let files_scan = scanner.scan_files(&files);
    let mut drafts = Vec::new();
    for (mut raw, also_in) in kept {
        let id = free_id(&id_from_name(&raw.name), &taken);
        if let Err(error) = secure(&mut raw, &id, &mut aliases) {
            skipped.push(Skipped {
                file: raw.file,
                entry: raw.entry,
                error,
            });
            continue;
        }
        let checkout = pin.is_some() && convert::uses_root(&raw, CHECKOUT_TOKEN);
        let root_text = match &pin {
            Some(pin) => checkout_root(&id, pin.path.as_deref()),
            None => root.clone(),
        };
        let mut server = Server::new(id.clone());
        set_launch(&mut server, &raw.fields);
        let body = &mut server.body;
        body.display_name = raw.display_name.clone();
        body.description = raw.description.clone();
        body.os = raw.os.clone();
        body.tools = raw.tools.clone();
        body.client = raw
            .passthrough
            .clone()
            .map(|(client, table)| BTreeMap::from([(client, table)]));
        let mut source = base.clone();
        source.file.clone_from(&raw.file);
        source.converter.clone_from(&raw.converter);
        source.entry.clone_from(&raw.entry);
        source.checkout = checkout.then_some(true);
        body.meta = Some(Meta {
            homepage: raw.homepage.clone(),
            origin: Some(
                match base.kind {
                    ServerSourceKind::Git => ORIGIN_GIT,
                    ServerSourceKind::Local => ORIGIN_LOCAL,
                }
                .to_owned(),
            ),
            source: Some(source),
            ..Meta::default()
        });
        let fill = |value: &str| {
            value
                .replace(CHECKOUT_TOKEN, &root_text)
                .replace(ID_TOKEN, &id)
        };
        edit_server(&mut server, &fill);
        let setup = raw.setup.iter().map(|step| fill(step)).collect();
        if let Some(error) = server.validate(Some(&id)).first_error(None) {
            skipped.push(Skipped {
                file: raw.file,
                entry: raw.entry,
                error,
            });
            continue;
        }
        taken.insert(id);
        let mut scan = files_scan.clone();
        scan.merge(scan_commands(&scanner, &server));
        let key = match &raw.entry {
            Some(entry) => format!("{}#{entry}", raw.file),
            None => raw.file.clone(),
        };
        drafts.push(Draft {
            key,
            kind: raw.kind,
            server,
            inputs: raw.inputs,
            runs_repo_code: raw.runs_repo_code || checkout,
            checkout,
            pinned: raw.pinned,
            setup,
            also_in,
            scan,
        });
    }
    Ok(Resolved {
        base,
        drafts,
        skipped,
        skipped_folders,
        files,
        files_scan,
        trust_source: Some(trust_source),
        moved_source,
        source_change,
        clone,
        spent: false,
    })
}

// --------------------------------------------------------------- checkouts

/// `<checkout>/.git/loadout-checkout`.
fn marker_path(dir: &Path) -> PathBuf {
    dir.join(".git").join(MARKER_FILE)
}

fn marker_text(commit: &str, url: &str) -> String {
    format!("{commit}\n{url}\n")
}

/// Whether the checkout of server `id` below `loadout_home` holds what
/// `source` names (its marker: commit and URL).
fn checkout_current(fs: &dyn Fs, loadout_home: &Path, id: &str, source: &ServerSource) -> bool {
    let dir = loadout_home.join(CHECKOUTS_DIR).join(id);
    let Ok(bytes) = fs.read(&marker_path(&dir)) else {
        return false;
    };
    let expected = marker_text(
        source.commit.as_deref().unwrap_or_default(),
        source.url.as_deref().unwrap_or_default(),
    );
    String::from_utf8_lossy(&bytes) == expected
}

/// Whether `server` may run on this machine (checked when its entries are
/// rendered): a server from a Git repository that runs from a checkout
/// needs this machine's checkout at the commit and URL its file names
/// (`checkout_out_of_date` otherwise, so the old code never runs with the
/// new command); a server from a local folder runs only on the machine
/// that added it (`folder_on_other_machine`).
pub fn runnable(fs: &dyn Fs, loadout_home: &Path, server: &Server) -> Result<(), CoreError> {
    let Some(source) = server
        .body
        .meta
        .as_ref()
        .and_then(|meta| meta.source.as_ref())
    else {
        return Ok(());
    };
    match source.kind {
        ServerSourceKind::Git if source.uses_checkout() => {
            if checkout_current(fs, loadout_home, &server.id, source) {
                Ok(())
            } else {
                Err(CoreError::new(
                    ErrorCode::CheckoutOutOfDate,
                    format!(
                        "checkout out of date: run `loadout server checkout {}`",
                        server.id
                    ),
                ))
            }
        }
        ServerSourceKind::Local => match &source.machine {
            Some(machine) => {
                let here = Machine::load(fs, &loadout_home.join(STATE_DIR))?;
                if here.as_ref().is_some_and(|here| &here.id == machine) {
                    Ok(())
                } else {
                    Err(CoreError::new(
                        ErrorCode::FolderOnOtherMachine,
                        "added from a folder on another machine; it does not run here",
                    ))
                }
            }
            None => Ok(()),
        },
        ServerSourceKind::Git => Ok(()),
    }
}

/// A checkout being put in place: the new folder placed (moved from a
/// clone, or copied), the folder it replaces moved aside. [`Swap::finish`]
/// keeps the new one (and deletes the old) or undoes it all.
#[derive(Debug)]
struct Swap {
    target: PathBuf,
    aside: Option<PathBuf>,
    /// Where the new folder was moved from, to move it back on undo.
    moved_from: Option<PathBuf>,
}

impl Swap {
    fn finish(self, fs: &dyn Fs, keep: bool) {
        if keep {
            if let Some(aside) = &self.aside {
                let _ = remove_dir(fs, aside);
            }
            return;
        }
        match &self.moved_from {
            Some(from) => {
                let _ = std::fs::rename(&self.target, from);
            }
            None => {
                let _ = remove_dir(fs, &self.target);
            }
        }
        if let Some(aside) = &self.aside {
            let _ = std::fs::rename(aside, &self.target);
        }
    }
}

/// `<LOADOUT_HOME>/servers/.old-<id>-<ulid>`.
fn aside_path(sx: &SkillsEnv<'_>, id: &str) -> Result<PathBuf, CoreError> {
    Ok(sx.home.path.join(CHECKOUTS_DIR).join(format!(
        "{ASIDE_PREFIX}{id}-{}",
        ids::new_ulid(sx.env.now())?.to_ascii_lowercase()
    )))
}

/// Moves the checkout of `id` aside, if there is one.
fn set_aside(sx: &SkillsEnv<'_>, id: &str) -> Result<Option<(PathBuf, PathBuf)>, CoreError> {
    let target = checkout_dir(sx, id);
    confine(sx.fs, &target)?;
    if !sx.fs.exists(&target) {
        return Ok(None);
    }
    let aside = aside_path(sx, id)?;
    std::fs::rename(&target, &aside)
        .map_err(|err| io_error("cannot move the old checkout aside", &target, &err))?;
    Ok(Some((aside, target)))
}

/// Copies the tree `from` into `to` (a second checkout of one clone).
fn copy_tree(fs: &dyn Fs, from: &Path, to: &Path) -> Result<(), CoreError> {
    confine(fs, from)?;
    confine(fs, to)?;
    std::fs::create_dir_all(to).map_err(|err| io_error("cannot create directory", to, &err))?;
    let entries =
        std::fs::read_dir(from).map_err(|err| io_error("cannot list directory", from, &err))?;
    for entry in entries {
        let entry = entry.map_err(|err| io_error("cannot list directory", from, &err))?;
        let kind = entry
            .file_type()
            .map_err(|err| io_error("cannot read file metadata", &entry.path(), &err))?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(fs, &entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)
                .map_err(|err| io_error("cannot copy file", &target, &err))?;
        }
    }
    Ok(())
}

/// Puts `clone` in place as the checkout of `id` (moved, or copied with
/// `copy`), marked with `commit` and `url`; an existing checkout is moved
/// aside. On an error nothing is left changed.
fn place_checkout(
    sx: &SkillsEnv<'_>,
    clone: &Path,
    id: &str,
    copy: bool,
    commit: &str,
    url: &str,
) -> Result<Swap, CoreError> {
    let target = checkout_dir(sx, id);
    confine(sx.fs, clone)?;
    confine(sx.fs, &target)?;
    let aside = set_aside(sx, id)?.map(|(aside, _)| aside);
    let mut swap = Swap {
        target: target.clone(),
        aside,
        moved_from: None,
    };
    let place = |swap: &mut Swap| -> Result<(), CoreError> {
        if let Some(parent) = target.parent() {
            sx.fs
                .create_dir_all(parent)
                .map_err(|err| io_error("cannot create directory", parent, &err))?;
        }
        if copy {
            copy_tree(sx.fs, clone, &target)?;
        } else {
            std::fs::rename(clone, &target)
                .map_err(|err| io_error("cannot move the clone", &target, &err))?;
            swap.moved_from = Some(clone.to_path_buf());
        }
        let marker = marker_path(&target);
        sx.fs
            .write(&marker, marker_text(commit, url).as_bytes())
            .map_err(|err| io_error("cannot mark the checkout", &marker, &err))
    };
    if let Err(err) = place(&mut swap) {
        swap.finish(sx.fs, false);
        return Err(err);
    }
    Ok(swap)
}

/// Checkouts of removed servers, moved aside under the store lock until
/// the removal commits ([`CheckoutRemoval::finish`]).
#[derive(Debug, Default)]
pub struct CheckoutRemoval {
    moved: Vec<(PathBuf, PathBuf)>,
}

impl CheckoutRemoval {
    /// Moves the checkouts of `ids` aside (call it inside the removal's
    /// transaction).
    pub fn begin(sx: &SkillsEnv<'_>, ids: &[String]) -> Result<Self, CoreError> {
        let mut removal = Self::default();
        for id in ids {
            ids::check(ids::IdKind::Server, id)?;
            match set_aside(sx, id) {
                Ok(Some(pair)) => removal.moved.push(pair),
                Ok(None) => {}
                Err(err) => {
                    removal.finish(sx.fs, false);
                    return Err(err);
                }
            }
        }
        Ok(removal)
    }

    /// Deletes the checkouts (`committed`) or puts them back.
    pub fn finish(self, fs: &dyn Fs, committed: bool) {
        for (aside, target) in self.moved {
            if committed {
                let _ = remove_dir(fs, &aside);
            } else {
                let _ = std::fs::rename(&aside, &target);
            }
        }
    }
}

/// Removes temporary clones of resolves and checkouts moved aside that are
/// older than `older_than` (left by a crash or an app that ended with
/// reviews open). Returns how many were removed.
pub fn sweep_temp_clones(sx: &SkillsEnv<'_>, older_than: Duration) -> usize {
    let now = sx.env.now();
    let sweep = |dir: PathBuf, prefix: &str| {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return 0;
        };
        entries
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
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
    };
    sweep(sx.home.cache_dir().join(TEMP_DIR), ".tmp-")
        + sweep(sx.home.path.join(CHECKOUTS_DIR), ASIDE_PREFIX)
}

// -------------------------------------------------------------------- save

/// One draft to save, as the user edited it.
#[derive(Debug, Clone)]
pub struct Pick {
    /// [`Draft::key`].
    pub key: String,
    /// The server to store: its id, launch fields, enablement and names as
    /// edited; `meta.origin` and `meta.source` are set from the draft.
    pub server: Server,
}

/// What [`save`] did.
#[derive(Debug, Clone)]
pub struct Saved {
    pub ids: Vec<String>,
    pub commit: Option<CommitId>,
    pub apply: Option<ApplyReport>,
    /// The scan the gate checked: the folder's files and every saved
    /// launch line.
    pub scan: ScanReport,
}

/// The server `pick` stores for `draft`.
fn picked_server(draft: &Draft, pick: &Pick) -> Result<Server, CoreError> {
    let mut server = pick.server.clone();
    ids::check(ids::IdKind::Server, &server.id)?;
    rename_id(&mut server, &draft.server.id, &pick.server.id);
    let meta = server.body.meta.get_or_insert_with(Meta::default);
    let draft_meta = draft.server.body.meta.as_ref();
    meta.origin = draft_meta.and_then(|m| m.origin.clone());
    let mut source = draft.provenance().clone();
    if draft.checkout {
        source.checkout = Some(true);
    }
    meta.source = Some(source);
    if meta.homepage.is_none() {
        meta.homepage = draft_meta.and_then(|m| m.homepage.clone());
    }
    if let Some(error) = server.validate(Some(&server.id)).first_error(None) {
        return Err(error);
    }
    Ok(server)
}

/// Where a commit message says the servers come from: the URL, or the
/// folder (`${home}/...`, or only its name when it is outside the home
/// folder: no absolute path goes into history).
fn from_text(source: &ServerSource) -> String {
    let place = match (&source.url, &source.folder) {
        (Some(url), _) => url.clone(),
        (None, Some(folder)) if folder.starts_with("${home}") => folder.clone(),
        (None, Some(folder)) => format!("folder {}", name_of(folder)),
        (None, None) => String::new(),
    };
    match &source.commit {
        Some(commit) => format!("{place}@{}", &commit[..commit.len().min(7)]),
        None => place,
    }
}

/// Stores the picked drafts (one commit, with the instant apply) when the
/// decisions let them through: the source must be trusted or
/// [`Decisions::trust`] carries the user's answer (`source_trust_required`,
/// `source_blocked`); then a `dangerous` scan of the folder's files and
/// every saved launch line needs [`Decisions::allow_dangerous`]
/// (`scan_blocked`). A taken id is `already_exists`. Checkouts are placed
/// under the store lock and undone when the commit fails; a local folder's
/// servers are bound to this machine. On an error `resolved` stays usable;
/// once saved it is spent.
pub fn save(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    ax: &ApplyEnv<'_>,
    resolved: &mut Resolved,
    picks: &[Pick],
    decisions: &Decisions,
) -> Result<Saved, CoreError> {
    if resolved.spent {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            "these drafts are already saved",
        ));
    }
    if picks.is_empty() {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            "pick at least one draft to save",
        ));
    }
    let mut servers: Vec<(Server, bool)> = Vec::new();
    for pick in picks {
        let draft = resolved
            .drafts
            .iter()
            .find(|draft| draft.key == pick.key)
            .ok_or_else(|| {
                CoreError::new(ErrorCode::NotFound, format!("no draft {:?}", pick.key))
            })?;
        let server = picked_server(draft, pick)?;
        if servers.iter().any(|(other, _)| other.id == server.id)
            || store.server(&server.id)?.is_some()
        {
            return Err(CoreError::new(
                ErrorCode::AlreadyExists,
                format!("server {:?} already exists", server.id),
            ));
        }
        servers.push((server, draft.checkout));
    }
    resolved.admit(store, decisions)?;
    let scanner = Scanner::new(sx.fs, sx.home);
    let mut report = resolved.files_scan.clone();
    for (server, _) in &servers {
        report.merge(scan_commands(&scanner, server));
    }
    scan::gate(&report, decisions.allow_dangerous)?;

    let ids: Vec<String> = servers
        .iter()
        .map(|(server, _)| server.id.clone())
        .collect();
    let from = from_text(&resolved.base);
    let summary = match ids.as_slice() {
        [one] => format!("server: add {one} from {from}"),
        many => format!(
            "server: add {} servers ({}) from {from}",
            many.len(),
            many.join(", ")
        ),
    };
    let operation = ids
        .iter()
        .fold(Operation::new("server.add", summary), |operation, id| {
            operation.entity(format!("server/{id}"))
        });
    let clone = resolved.clone.clone();
    let commit = resolved.base.commit.clone().unwrap_or_default();
    let url = resolved.base.url.clone().unwrap_or_default();
    let mut swaps: Vec<Swap> = Vec::new();
    let result = store.transaction(operation, |tx| {
        // Under the store lock: no other save or removal of these ids
        // runs between placing the checkouts and the commit.
        let wanted: Vec<&str> = servers
            .iter()
            .filter(|(_, checkout)| *checkout)
            .map(|(server, _)| server.id.as_str())
            .collect();
        if !wanted.is_empty() {
            let clone = clone.as_deref().ok_or_else(|| {
                CoreError::new(
                    ErrorCode::InvalidValue,
                    "the clone of this source is gone; resolve it again",
                )
            })?;
            // Copies first, then the move of the clone itself.
            for (index, id) in wanted.iter().enumerate() {
                let last = index + 1 == wanted.len();
                swaps.push(place_checkout(sx, clone, id, !last, &commit, &url)?);
            }
        }
        let machine = tx.machine().id.clone();
        for (server, _) in &servers {
            let mut server = server.clone();
            if let Some(source) = server.body.meta.as_mut().and_then(|m| m.source.as_mut())
                && source.kind == ServerSourceKind::Local
            {
                source.machine = Some(machine.clone());
            }
            tx.add_server(server)?;
        }
        instant_apply(tx, ax, &Touched::Servers(ids.clone()))
    });
    let keep = result.is_ok();
    let moved = swaps.iter().any(|swap| swap.moved_from.is_some());
    for swap in swaps {
        swap.finish(sx.fs, keep);
    }
    let committed = result?;
    resolved.spent = true;
    if moved {
        resolved.clone = None;
    }
    Ok(Saved {
        ids,
        commit: committed.commit,
        apply: committed.value,
        scan: report,
    })
}

// ------------------------------------------------------------------ update

/// One launch field that differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldChange {
    /// `transport`, `command`, `args`, `url`, `cwd`, `env.NAME`,
    /// `headers.NAME`, each also as `os.<name>.<field>`.
    pub field: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

/// What [`check_update`] found.
#[derive(Debug, Clone)]
pub struct UpdateCheck {
    pub id: String,
    pub status: UpdateStatus,
    /// The commit the server came from.
    pub current: Option<String>,
    /// The source's commit now; for a local folder, the content hash of
    /// what was read (pass it to [`update`] as `expected`).
    pub latest: Option<String>,
    pub changes: Vec<FieldChange>,
    /// Changed files of a checkout ([`SKIP_DIRS`] left out).
    pub files: Vec<FileChange>,
    /// The server an update stores.
    pub server: Option<Server>,
    /// The folder's files and every new launch line, when an update is
    /// available.
    pub scan: Option<ScanReport>,
    pub trust: Option<TrustCheck>,
    /// The trust check of where the host redirected the repository, when
    /// that is another source; an update needs its answer too.
    pub moved_trust: Option<TrustCheck>,
    pub source_change: Option<SourceChange>,
    pub runs_repo_code: bool,
    /// The setup steps of the new version ([`setup_text`]).
    pub setup: Vec<String>,
    /// The update replaces the checkout: what its setup steps installed
    /// there (dependencies, a build) has to be done again.
    pub setup_again: bool,
    /// This machine's checkout is missing or not at the commit the server
    /// file names; its entries are blocked until [`restore_checkout`].
    pub checkout_out_of_date: bool,
    pub error: Option<CoreError>,
}

impl UpdateCheck {
    fn new(id: &str, status: UpdateStatus, current: Option<String>) -> Self {
        Self {
            id: id.to_owned(),
            status,
            current,
            latest: None,
            changes: Vec::new(),
            files: Vec::new(),
            server: None,
            scan: None,
            trust: None,
            moved_trust: None,
            source_change: None,
            runs_repo_code: false,
            setup: Vec::new(),
            setup_again: false,
            checkout_out_of_date: false,
            error: None,
        }
    }
}

fn stored(store: &Store<'_>, id: &str) -> Result<(Server, Option<ServerSource>), CoreError> {
    ids::check(ids::IdKind::Server, id)?;
    let doc = store.server(id)?.ok_or_else(|| {
        CoreError::new(ErrorCode::NotFound, format!("server {id:?} does not exist"))
    })?;
    let server = doc.into_value();
    let source = server
        .body
        .meta
        .as_ref()
        .and_then(|meta| meta.source.clone());
    Ok((server, source))
}

fn request_of(sx: &SkillsEnv<'_>, source: &ServerSource) -> Result<ResolveRequest, CoreError> {
    match source.kind {
        ServerSourceKind::Git => {
            let url = source.url.clone().ok_or_else(|| {
                CoreError::new(ErrorCode::MissingField, "the git source has no url")
            })?;
            Ok(ResolveRequest {
                source: Source::Git {
                    url,
                    git_ref: None,
                    path: None,
                },
                git_ref: source.git_ref.clone(),
                path: source.path.clone(),
            })
        }
        ServerSourceKind::Local => {
            let folder = source.folder.as_deref().ok_or_else(|| {
                CoreError::new(ErrorCode::MissingField, "the local source has no folder")
            })?;
            Ok(ResolveRequest::new(Source::Local(folder_of(sx, folder))))
        }
    }
}

/// The value at `field` (`command`, `url`, `cwd`, `env.X`, `headers.X`,
/// `args[N]`, each optionally `os.<name>.`-prefixed) of `server`.
fn field_value<'a>(server: &'a Server, field: &str) -> Option<&'a str> {
    let (fields_command, fields_args, fields_url, fields_cwd, fields_env, fields_headers, rest) =
        match field.strip_prefix("os.") {
            Some(rest) => {
                let (os, rest) = rest.split_once('.')?;
                let over = &server.body.os.as_ref()?.get(os)?.fields;
                (
                    &over.command,
                    &over.args,
                    &over.url,
                    &over.cwd,
                    &over.env,
                    &over.headers,
                    rest,
                )
            }
            None => {
                let body = &server.body;
                (
                    &body.command,
                    &body.args,
                    &body.url,
                    &body.cwd,
                    &body.env,
                    &body.headers,
                    field,
                )
            }
        };
    match rest {
        "command" => fields_command.as_deref(),
        "url" => fields_url.as_deref(),
        "cwd" => fields_cwd.as_deref(),
        _ => {
            if let Some(name) = rest.strip_prefix("env.") {
                return fields_env.as_ref()?.get(name).map(String::as_str);
            }
            if let Some(name) = rest.strip_prefix("headers.") {
                return fields_headers.as_ref()?.get(name).map(String::as_str);
            }
            let index: usize = rest
                .strip_prefix("args[")?
                .strip_suffix(']')?
                .parse()
                .ok()?;
            fields_args
                .as_ref()?
                .get(index.checked_sub(1)?)
                .map(String::as_str)
        }
    }
}

/// The first `${secret:<alias>}` alias in `text`.
fn secret_alias(text: &str) -> Option<&str> {
    let start = text.find("${secret:")? + "${secret:".len();
    let end = text[start..].find('}')?;
    Some(&text[start..start + end])
}

/// Keeps the secret aliases `stored` uses: a re-read draft names its
/// secrets after a fresh id, so each one whose field already held a
/// reference gets that reference's alias back.
fn keep_aliases(proposed: &mut Server, stored: &Server, inputs: &[DraftInput]) {
    let renames: Vec<(String, String)> = inputs
        .iter()
        .filter(|input| input.secret)
        .filter_map(|input| {
            let new = input.alias.clone()?;
            let old = secret_alias(field_value(stored, &input.field)?)?.to_owned();
            (old != new).then_some((new, old))
        })
        .collect();
    if renames.is_empty() {
        return;
    }
    edit_server(proposed, &|value| {
        renames.iter().fold(value.to_owned(), |value, (new, old)| {
            value.replace(&format!("${{secret:{new}}}"), &format!("${{secret:{old}}}"))
        })
    });
}

/// The fields of `a` and `b` that differ, named with `prefix`.
fn diff_fields(prefix: &str, a: &ServerFields, b: &ServerFields, changes: &mut Vec<FieldChange>) {
    let mut push = |field: &str, old: Option<String>, new: Option<String>| {
        if old != new {
            changes.push(FieldChange {
                field: format!("{prefix}{field}"),
                old,
                new,
            });
        }
    };
    push(
        "transport",
        a.transport.map(|t| t.as_str().to_owned()),
        b.transport.map(|t| t.as_str().to_owned()),
    );
    push("command", a.command.clone(), b.command.clone());
    let join = |args: &Option<Vec<String>>| args.as_ref().map(|args| args.join(" "));
    push("args", join(&a.args), join(&b.args));
    push("url", a.url.clone(), b.url.clone());
    push("cwd", a.cwd.clone(), b.cwd.clone());
    for (label, old, new) in [("env", &a.env, &b.env), ("headers", &a.headers, &b.headers)] {
        let keys: BTreeSet<&String> = old
            .iter()
            .flatten()
            .chain(new.iter().flatten())
            .map(|(k, _)| k)
            .collect();
        for key in keys {
            let get = |map: &Option<BTreeMap<String, String>>| {
                map.as_ref().and_then(|m| m.get(key).cloned())
            };
            push(&format!("{label}.{key}"), get(old), get(new));
        }
    }
}

/// The launch fields that differ between `old` and `new`, base and each
/// override.
fn field_changes(old: &Server, new: &Server) -> Vec<FieldChange> {
    let mut changes = Vec::new();
    diff_fields("", &old.body.fields(), &new.body.fields(), &mut changes);
    let names: BTreeSet<&String> = old
        .body
        .os
        .iter()
        .flatten()
        .chain(new.body.os.iter().flatten())
        .map(|(name, _)| name)
        .collect();
    let empty = ServerFields::default();
    for name in names {
        let get = |server: &Server| {
            server
                .body
                .os
                .as_ref()
                .and_then(|os| os.get(name.as_str()))
                .map_or(empty.clone(), |over| over.fields.clone())
        };
        diff_fields(&format!("os.{name}."), &get(old), &get(new), &mut changes);
    }
    changes
}

/// `stored` with `draft`'s launch fields and provenance under the stored
/// id: `env` and `headers` keys present before and after keep the stored
/// value, secret references keep their aliases, the machine a local folder
/// is bound to stays.
fn updated_server(stored: &Server, draft: &Draft) -> Server {
    let mut server = stored.clone();
    let mut fields = draft.server.body.fields();
    let keep = |new: &mut Option<BTreeMap<String, String>>,
                old: &Option<BTreeMap<String, String>>| {
        if let (Some(new), Some(old)) = (new.as_mut(), old.as_ref()) {
            for (key, value) in new.iter_mut() {
                if let Some(kept) = old.get(key) {
                    value.clone_from(kept);
                }
            }
        }
    };
    keep(&mut fields.env, &stored.body.env);
    keep(&mut fields.headers, &stored.body.headers);
    set_launch(&mut server, &fields);
    server.body.os = draft.server.body.os.clone();
    rename_id(&mut server, &draft.server.id, &stored.id);
    keep_aliases(&mut server, stored, &draft.inputs);
    let machine = stored
        .body
        .meta
        .as_ref()
        .and_then(|meta| meta.source.as_ref())
        .and_then(|source| source.machine.clone());
    let meta = server.body.meta.get_or_insert_with(Meta::default);
    let mut source = draft.provenance().clone();
    source.machine = machine;
    meta.source = Some(source);
    server
}

/// Files that differ between a checkout and the source now.
fn file_changes(old: &SkillFiles, new: &SkillFiles) -> Vec<FileChange> {
    let paths: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    paths
        .into_iter()
        .filter_map(|path| {
            let kind = match (old.get(path), new.get(path)) {
                (Some(a), Some(b)) if a == b => return None,
                (Some(_), Some(_)) => ChangeKind::Modified,
                (None, Some(_)) => ChangeKind::Added,
                (Some(_), None) => ChangeKind::Removed,
                (None, None) => return None,
            };
            let script = kind != ChangeKind::Removed
                && new
                    .get(path)
                    .is_some_and(|bytes| looks_executable(path, bytes));
            Some(FileChange {
                path: path.clone(),
                kind,
                script,
            })
        })
        .collect()
}

/// The folder of `id`'s checkout that holds the server's files.
fn checkout_folder(sx: &SkillsEnv<'_>, id: &str, source: &ServerSource) -> PathBuf {
    let dir = checkout_dir(sx, id);
    match &source.path {
        Some(path) => join_rel(&dir, path),
        None => dir,
    }
}

/// This machine's id, if it has one yet.
fn this_machine(sx: &SkillsEnv<'_>) -> Result<Option<String>, CoreError> {
    Ok(Machine::load(sx.fs, &sx.home.state_dir())?.map(|machine| machine.id))
}

/// The check and what an update would store.
struct Latest {
    check: UpdateCheck,
    resolved: Option<Resolved>,
}

fn latest(store: &Store<'_>, sx: &SkillsEnv<'_>, id: &str) -> Result<Latest, CoreError> {
    let (server, source) = stored(store, id)?;
    let Some(source) = source else {
        return Ok(Latest {
            check: UpdateCheck::new(id, UpdateStatus::NotTracked, None),
            resolved: None,
        });
    };
    let current = source.commit.clone();
    let mut check = UpdateCheck::new(id, UpdateStatus::UpToDate, current.clone());
    check.checkout_out_of_date =
        source.uses_checkout() && !checkout_current(sx.fs, &sx.home.path, id, &source);
    let gated = match source.kind {
        ServerSourceKind::Git => source
            .url
            .as_deref()
            .and_then(|url| SourceRef::from_git_url(url).ok()),
        ServerSourceKind::Local => source
            .folder
            .as_deref()
            .map(|f| folder_of(sx, f))
            .and_then(|folder| SourceRef::from_local_path(&folder).ok()),
    };
    let not_fetched = |mut check: UpdateCheck,
                       status: UpdateStatus,
                       err: CoreError|
     -> Result<Latest, CoreError> {
        check.status = status;
        check.trust = gated
            .as_ref()
            .map(|source| sources::check(store, source))
            .transpose()?;
        check.error = Some(err);
        Ok(Latest {
            check,
            resolved: None,
        })
    };
    if source.kind == ServerSourceKind::Local
        && let Some(machine) = &source.machine
        && this_machine(sx)?.as_ref() != Some(machine)
    {
        let err = CoreError::new(
            ErrorCode::FolderOnOtherMachine,
            "added from a folder on another machine; check it there",
        );
        return not_fetched(check, UpdateStatus::Unreachable, err);
    }
    if source.kind == ServerSourceKind::Git && source.git_ref.as_deref().is_some_and(is_commit_ref)
    {
        // Pinned to a commit: nothing to follow.
        check.latest = current;
        return Ok(Latest {
            check,
            resolved: None,
        });
    }
    let request = request_of(sx, &source)?;
    let resolved = match resolve(Some(store), sx, &request) {
        Ok(resolved) => resolved,
        Err(err) if err.code() == ErrorCode::SourceBlocked => {
            return not_fetched(check, UpdateStatus::Blocked, err);
        }
        // Whatever stops the read (unreachable, gone, too large) is this
        // server's result, not an error of the whole check.
        Err(err) => return not_fetched(check, UpdateStatus::Unreachable, err),
    };
    let latest_commit = match source.kind {
        ServerSourceKind::Git => resolved.base.commit.clone(),
        ServerSourceKind::Local => Some(content_hash(&resolved.files)),
    };
    let mut change = resolved.source_change.clone();
    if change.is_none()
        && let (Some(old), Some(new), Some(clone), Some(url)) = (
            source.commit.as_deref(),
            resolved.base.commit.as_deref(),
            resolved.clone.as_deref(),
            source.url.as_deref(),
        )
    {
        let git = git_path(sx)?;
        let in_clone = |args: &[&OsStr]| run_git(sx, &git, clone, args);
        if sources::detect::history_rewritten(&in_clone, url, source.git_ref.as_deref(), old, new)?
        {
            change = Some(SourceChange {
                kind: SourceChangeKind::HistoryRewritten,
                detail: None,
            });
        }
    }
    if let (Some(gated), Some(change)) = (resolved.trust_source(), &change) {
        sources::lapse(store, gated, change)?;
    }
    check.trust = resolved
        .trust_source()
        .map(|source| sources::check(store, source))
        .transpose()?;
    check.moved_trust = resolved
        .moved_source()
        .map(|source| sources::check(store, source))
        .transpose()?;
    check.source_change = change;
    check.latest.clone_from(&latest_commit);
    let Some(draft) = resolved.drafts.iter().find(|draft| {
        let p = draft.provenance();
        p.file == source.file && p.converter == source.converter && p.entry == source.entry
    }) else {
        let err = CoreError::new(
            ErrorCode::SourceUnreachable,
            format!(
                "the source no longer holds this server ({}{})",
                source.file,
                source
                    .entry
                    .as_deref()
                    .map(|entry| format!(" {entry}"))
                    .unwrap_or_default()
            ),
        );
        return not_fetched(check, UpdateStatus::Unreachable, err);
    };
    let proposed = updated_server(&server, draft);
    check.changes = field_changes(&server, &proposed);
    check.setup = rename_steps(&draft.setup, &draft.server.id, id);
    if draft.checkout {
        let folder = checkout_folder(sx, id, &source);
        if sx.fs.is_dir(&folder) {
            match read_source(sx.fs, &folder) {
                Ok((files, _)) => check.files = file_changes(&files, &resolved.files),
                Err(err) => return not_fetched(check, UpdateStatus::Unreachable, err),
            }
        }
    }
    let moved_on = source.kind == ServerSourceKind::Git
        && current.is_some()
        && resolved.base.commit != current;
    if !check.changes.is_empty() || moved_on || !check.files.is_empty() {
        check.status = UpdateStatus::Available;
        check.setup_again = draft.checkout && !check.setup.is_empty();
        let scanner = Scanner::new(sx.fs, sx.home);
        let mut scan = resolved.files_scan.clone();
        scan.merge(scan_commands(&scanner, &proposed));
        check.scan = Some(scan);
    }
    check.runs_repo_code = draft.runs_repo_code;
    check.server = Some(proposed);
    Ok(Latest {
        check,
        resolved: Some(resolved),
    })
}

/// Checks a source server for an update: reads its source again (a fresh
/// temporary clone, or the folder) and compares the definition it gives
/// now with the stored one. Whatever stops the read (unreachable, gone,
/// too large, a folder of another machine) is reported in the check, not
/// returned as an error. A force-push or a move of a trusted source makes
/// its trust lapse (one store commit).
pub fn check_update(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    id: &str,
) -> Result<UpdateCheck, CoreError> {
    Ok(latest(store, sx, id)?.check)
}

/// What [`update`] did.
#[derive(Debug, Clone)]
pub struct Updated {
    pub id: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub changes: Vec<FieldChange>,
    pub commit: Option<CommitId>,
    pub apply: Option<ApplyReport>,
    /// `None` when nothing was updated (up to date).
    pub scan: Option<ScanReport>,
}

/// Updates a source server to what its source gives now (one commit,
/// `server: update <id> to <sha7>`, with the instant apply): the launch
/// fields are replaced, `env` and `headers` values of keys kept keep the
/// stored value, secret references keep their aliases, a checkout is
/// replaced: the new one is built beside the old, committed, then swapped
/// (the old stays when anything fails). `expected` is what the user
/// reviewed (the commit, or a folder's content hash): another one is
/// `entry_changed`. The gates are those of [`save`]. Nothing is written
/// when it is up to date.
pub fn update(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    ax: &ApplyEnv<'_>,
    id: &str,
    expected: Option<&str>,
    decisions: &Decisions,
) -> Result<Updated, CoreError> {
    let Latest { check, resolved } = latest(store, sx, id)?;
    match check.status {
        UpdateStatus::NotTracked => {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!("server {id:?} does not come from a git repository or folder"),
            ));
        }
        UpdateStatus::Unreachable | UpdateStatus::Blocked => {
            return Err(check.error.unwrap_or_else(|| {
                CoreError::new(ErrorCode::SourceUnreachable, "the source cannot be reached")
            }));
        }
        _ => {}
    }
    if let Some(expected) = expected
        && check.latest.as_deref() != Some(expected)
    {
        return Err(CoreError::new(
            ErrorCode::EntryChanged,
            "the source changed since it was checked; check again and review it",
        ));
    }
    let (Some(mut resolved), Some(server), UpdateStatus::Available) =
        (resolved, check.server.clone(), check.status)
    else {
        return Ok(Updated {
            id: id.to_owned(),
            from: check.current.clone(),
            to: check.latest.clone(),
            changes: Vec::new(),
            commit: None,
            apply: None,
            scan: check.scan,
        });
    };
    resolved.admit(store, decisions)?;
    let scan = check.scan.clone().expect("an available update is scanned");
    scan::gate(&scan, decisions.allow_dangerous)?;
    let source = server
        .body
        .meta
        .as_ref()
        .and_then(|meta| meta.source.clone())
        .expect("a source server");
    let to = match source.kind {
        ServerSourceKind::Git => source.commit.clone(),
        ServerSourceKind::Local => None,
    };
    let summary = match &to {
        Some(commit) => format!("server: update {id} to {}", &commit[..commit.len().min(7)]),
        None => format!("server: update {id} from its folder"),
    };
    let mut doc = store.server(id)?.ok_or_else(|| {
        CoreError::new(ErrorCode::NotFound, format!("server {id:?} does not exist"))
    })?;
    *doc.value_mut() = server;
    let text = doc.to_text()?;
    let operation = Operation::new("server.update", summary).entity(format!("server/{id}"));
    let clone = resolved.clone.clone();
    let mut swap: Option<Swap> = None;
    let result = store.transaction(operation, |tx| {
        if source.uses_checkout() {
            let clone = clone.as_deref().ok_or_else(|| {
                CoreError::new(ErrorCode::InvalidValue, "the clone of this source is gone")
            })?;
            swap = Some(place_checkout(
                sx,
                clone,
                id,
                false,
                source.commit.as_deref().unwrap_or_default(),
                source.url.as_deref().unwrap_or_default(),
            )?);
        }
        tx.write(&server_path(id)?, text)?;
        instant_apply(tx, ax, &Touched::Servers(vec![id.to_owned()]))
    });
    if let Some(swap) = swap {
        if result.is_ok() && swap.moved_from.is_some() {
            resolved.clone = None;
        }
        swap.finish(sx.fs, result.is_ok());
    }
    let committed = result?;
    Ok(Updated {
        id: id.to_owned(),
        from: check.current,
        to: check.latest,
        changes: check.changes,
        commit: committed.commit,
        apply: committed.value,
        scan: Some(scan),
    })
}

// ----------------------------------------------------------------- restore

/// A checkout fetched for [`restore_checkout`]: the repository at the
/// commit the server file names, scanned with the stored launch lines.
#[derive(Debug)]
pub struct PreparedCheckout {
    id: String,
    resolved: Resolved,
    scan: ScanReport,
    commit: String,
    url: String,
}

impl PreparedCheckout {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn commit(&self) -> &str {
        &self.commit
    }

    /// The repository's files and every launch line of the stored server.
    pub fn scan(&self) -> &ScanReport {
        &self.scan
    }

    pub fn trust_source(&self) -> Option<&SourceRef> {
        self.resolved.trust_source()
    }

    /// Where the host redirected the repository (asked about too).
    pub fn moved_source(&self) -> Option<&SourceRef> {
        self.resolved.moved_source()
    }

    pub fn source_change(&self) -> Option<&SourceChange> {
        self.resolved.source_change()
    }
}

/// Fetches what this machine's checkout of source server `id` should hold
/// (another machine saved or updated it, or it was removed): the
/// repository at the recorded commit, into a temporary clone. `None` when
/// the checkout is already current. A blocked source is not contacted.
pub fn prepare_checkout(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    id: &str,
) -> Result<Option<PreparedCheckout>, CoreError> {
    let (server, source) = stored(store, id)?;
    let source = source.filter(ServerSource::uses_checkout).ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidValue,
            format!("server {id:?} does not run from a checkout"),
        )
    })?;
    if checkout_current(sx.fs, &sx.home.path, id, &source) {
        return Ok(None);
    }
    let (Some(url), Some(commit)) = (source.url.clone(), source.commit.clone()) else {
        return Err(CoreError::new(
            ErrorCode::MissingField,
            "the git source has no url or commit",
        ));
    };
    let request = ResolveRequest {
        source: Source::Git {
            url: url.clone(),
            git_ref: None,
            path: None,
        },
        git_ref: Some(commit.clone()),
        path: source.path.clone(),
    };
    let resolved = resolve(Some(store), sx, &request)?;
    let scanner = Scanner::new(sx.fs, sx.home);
    let mut scan = resolved.files_scan.clone();
    scan.merge(scan_commands(&scanner, &server));
    Ok(Some(PreparedCheckout {
        id: id.to_owned(),
        resolved,
        scan,
        commit,
        url,
    }))
}

/// Puts a prepared checkout in place under the store lock, after the same
/// gates as a save: trust in the source (and a redirect target), then the
/// scan of its files and the stored launch lines (`allow_dangerous`). A
/// stale checkout is replaced atomically; the server's entries are applied
/// again.
pub fn restore_checkout(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    ax: &ApplyEnv<'_>,
    mut prepared: PreparedCheckout,
    decisions: &Decisions,
) -> Result<Option<ApplyReport>, CoreError> {
    prepared.resolved.admit(store, decisions)?;
    scan::gate(&prepared.scan, decisions.allow_dangerous)?;
    let clone = prepared.resolved.clone.clone().ok_or_else(|| {
        CoreError::new(ErrorCode::InvalidValue, "the clone of this source is gone")
    })?;
    let id = prepared.id.clone();
    let operation = Operation::new(
        "server.checkout",
        format!(
            "server: check out {id} at {}",
            &prepared.commit[..7.min(prepared.commit.len())]
        ),
    )
    .entity(format!("server/{id}"));
    let mut swap: Option<Swap> = None;
    let result = store.transaction(operation, |tx| {
        swap = Some(place_checkout(
            sx,
            &clone,
            &id,
            false,
            &prepared.commit,
            &prepared.url,
        )?);
        instant_apply(tx, ax, &Touched::Servers(vec![id.clone()]))
    });
    if let Some(swap) = swap {
        if result.is_ok() && swap.moved_from.is_some() {
            prepared.resolved.clone = None;
        }
        swap.finish(sx.fs, result.is_ok());
    }
    Ok(result?.value)
}

/// Removes the machine-local checkout of `id`, if any.
pub fn remove_checkout(sx: &SkillsEnv<'_>, id: &str) -> Result<(), CoreError> {
    ids::check(ids::IdKind::Server, id)?;
    let dir = checkout_dir(sx, id);
    confine(sx.fs, &dir)?;
    remove_dir(sx.fs, &dir)
}

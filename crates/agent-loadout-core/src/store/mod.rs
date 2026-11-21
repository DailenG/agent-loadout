//! The versioned store `<LOADOUT_HOME>/store` (SPEC 9.1): a normal git
//! repository on branch `main`, readable by any git client.
//!
//! Local operations use `git2` with a vendored libgit2 and no network
//! features (SPEC 4.2), so the store works offline without a git binary.
//! Commits carry an explicit author and committer (`loadout <label>` /
//! `<machine-id>@loadout.invalid`); Loadout never asks git for an identity
//! and never writes git configuration. Every mutation goes through
//! [`Store::transaction`], which validates, writes and commits once.
//!
//! The working tree is the source of truth for reads, so hand edits are
//! seen before they are committed. git types stay private to this module.
//!
//! History views live in [`diff`] (semantic and raw diffs) and [`tags`];
//! [`restore`] restores a past revision as a new commit and client files
//! from apply backups (SPEC 9.3 to 9.5).

pub mod diff;
mod lock;
mod log;
pub mod restore;
mod revision;
pub mod tags;
mod trailers;
mod transaction;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use git2::{Commit, Repository, RepositoryInitOptions, Signature};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use toml::value::{Date, Datetime, Offset, Time};

use crate::brand;
use crate::env::Env;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::ids::{self, IdKind};
use crate::model::server::Server;
use crate::model::{SCHEMA_VERSION, StoreModel, TomlDocument};
use crate::paths::LoadoutHome;
use crate::secrets::scanner::SecretMemory;

pub use lock::{LOCK_FILE, LOCK_TIMEOUT, StoreLock};
pub use log::{HistoryFinding, LogEntry, LogFilter, entity_paths};
pub use revision::Revision;
pub use trailers::{ApplyRef, MachineRef, Scope, Trailers, compose_message, message_summary};
pub use transaction::{Committed, Operation, Transaction};

/// Read access to store files: the working tree ([`Store`]), the working
/// tree plus staged changes ([`Transaction`]) or a past commit
/// ([`Revision`]).
pub trait StoreRead {
    /// A store file as text; `None` if it does not exist.
    fn read_file(&self, relative: &str) -> Result<Option<String>, CoreError>;

    /// Names of the files (not directories) directly in the store-relative
    /// directory `dir`, sorted; empty if it does not exist.
    fn list_files(&self, dir: &str) -> Result<Vec<String>, CoreError>;
}

impl StoreRead for Store<'_> {
    fn read_file(&self, relative: &str) -> Result<Option<String>, CoreError> {
        self.read_to_string(relative)
    }

    fn list_files(&self, dir: &str) -> Result<Vec<String>, CoreError> {
        let path = self.path(dir)?;
        if !self.fs.is_dir(&path) {
            return Ok(Vec::new());
        }
        let entries = self
            .fs
            .read_dir(&path)
            .map_err(|err| io_error("cannot list directory", &path, &err))?;
        let mut names: Vec<String> = entries
            .iter()
            .filter(|entry| !self.fs.is_dir(entry))
            .filter_map(|entry| entry.file_name()?.to_str().map(str::to_owned))
            .collect();
        names.sort();
        Ok(names)
    }
}

/// The store branch.
pub const BRANCH: &str = "main";
/// Store identity file at the store root.
pub const IDENTITY_FILE: &str = "loadout.toml";
/// Machine identity file in `state/`.
pub const MACHINE_FILE: &str = "machine.toml";
/// Written by `init`: store files are versioned byte for byte, whatever
/// `core.autocrlf` a git client uses on the store.
const GITATTRIBUTES: (&str, &str) = (".gitattributes", "* -text\n");

/// `store/loadout.toml`: ties remotes and machines to one logical store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "Loadout store identity (store/loadout.toml)")]
pub struct StoreIdentity {
    /// Schema version; always 1.
    pub schema: i64,
    /// ULID.
    pub store_id: String,
    #[schemars(with = "String")]
    pub created_at: Datetime,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

impl StoreModel for StoreIdentity {
    const SCHEMA_NAME: &'static str = "store-identity";
}

/// This machine (`state/machine.toml`): the commit author and the
/// `Loadout-Machine` trailer. Created on the first mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    /// ULID; never changes.
    pub id: String,
    /// Human label, cleaned so it fits a git signature and a trailer.
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "Loadout machine identity (state/machine.toml)")]
struct MachineFile {
    schema: i64,
    machine_id: String,
    label: String,
    #[serde(flatten)]
    #[schemars(skip)]
    extra: toml::Table,
}

impl StoreModel for MachineFile {
    const SCHEMA_NAME: &'static str = "machine-identity";
}

/// Label used when the host name is unknown.
const DEFAULT_LABEL: &str = "machine";

impl Machine {
    /// Reads `state/machine.toml`, if it exists.
    pub fn load(fs: &dyn Fs, state_dir: &Path) -> Result<Option<Self>, CoreError> {
        let path = state_dir.join(MACHINE_FILE);
        if !fs.exists(&path) {
            return Ok(None);
        }
        let file = TomlDocument::<MachineFile>::load(fs, &path)?.into_value();
        ids::check(IdKind::Machine, &file.machine_id).map_err(|err| err.with_path(&path))?;
        Ok(Some(Self {
            id: file.machine_id,
            label: clean_label(&file.label),
        }))
    }

    /// Reads `state/machine.toml`, creating it with a new ULID and the host
    /// name as label on first use.
    pub fn load_or_create(env: &Env, fs: &dyn Fs, state_dir: &Path) -> Result<Self, CoreError> {
        if let Some(machine) = Self::load(fs, state_dir)? {
            return Ok(machine);
        }
        let machine = Self {
            id: ids::new_ulid(env.now())?,
            label: clean_label(env.hostname().unwrap_or(DEFAULT_LABEL)),
        };
        let path = state_dir.join(MACHINE_FILE);
        fs.create_dir_all(state_dir)
            .map_err(|err| io_error("cannot create directory", state_dir, &err))?;
        TomlDocument::new(MachineFile {
            schema: SCHEMA_VERSION,
            machine_id: machine.id.clone(),
            label: machine.label.clone(),
            extra: toml::Table::new(),
        })
        .save(fs, &path)?;
        Ok(machine)
    }

    /// `loadout <label>` / `<machine-id>@loadout.invalid` at `time`.
    fn signature(&self, time: SystemTime) -> Result<Signature<'static>, CoreError> {
        let seconds = time
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        Signature::new(
            &format!("{} {}", brand::COMMIT_AUTHOR_NAME, self.label),
            &format!("{}@{}", self.id, brand::COMMIT_EMAIL_DOMAIN),
            &git2::Time::new(i64::try_from(seconds).unwrap_or(i64::MAX), 0),
        )
        .map_err(git_error)
    }
}

/// Drops characters a git signature or a trailer cannot hold (`<`, `>`,
/// control characters) and collapses whitespace.
fn clean_label(raw: &str) -> String {
    let cleaned = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .filter(|c| !matches!(c, '<' | '>'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.is_empty() {
        DEFAULT_LABEL.to_owned()
    } else {
        cleaned
    }
}

/// A store commit id (full lower-case hex).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitId(String);

impl CommitId {
    fn from_oid(oid: git2::Oid) -> Self {
        Self(oid.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first 7 hex digits.
    pub fn short(&self) -> &str {
        &self.0[..self.0.len().min(7)]
    }
}

impl fmt::Display for CommitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One server file as listed by [`Store::servers`]: the parsed and
/// validated document, or why it could not be used.
#[derive(Debug)]
pub struct ServerEntry {
    /// The file name without `.toml`.
    pub id: String,
    pub document: Result<TomlDocument<Server>, CoreError>,
}

/// An open store.
pub struct Store<'a> {
    env: Env,
    fs: &'a dyn Fs,
    root: PathBuf,
    state_dir: PathBuf,
    repo: Repository,
    identity: StoreIdentity,
    client: String,
    /// Values resolved in this process; every commit is scanned for them.
    secrets: Arc<SecretMemory>,
}

impl fmt::Debug for Store<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store")
            .field("root", &self.root)
            .field("store_id", &self.identity.store_id)
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}

impl<'a> Store<'a> {
    /// Opens the store in `home`, or `None` when it has not been created
    /// yet. Creates nothing. `client` names the calling program and version
    /// for the `Loadout-Client` trailer (`loadout-cli/0.1.0`).
    pub fn open(
        env: &Env,
        fs: &'a dyn Fs,
        home: &LoadoutHome,
        client: &str,
    ) -> Result<Option<Self>, CoreError> {
        let root = home.store_dir();
        if !fs.exists(&root.join(".git")) {
            return Ok(None);
        }
        let repo = Repository::open(&root).map_err(git_error)?;
        if repo.head().is_err() {
            // Initialized but never committed (an interrupted init).
            return Ok(None);
        }
        let path = root.join(IDENTITY_FILE);
        let identity = TomlDocument::<StoreIdentity>::load(fs, &path)?.into_value();
        ids::check(IdKind::Store, &identity.store_id).map_err(|err| err.with_path(&path))?;
        Ok(Some(Self {
            env: env.clone(),
            fs,
            root,
            state_dir: home.state_dir(),
            repo,
            identity,
            client: client.to_owned(),
            secrets: Arc::new(SecretMemory::new()?),
        }))
    }

    /// Opens the store in `home`, creating it first if needed: a git
    /// repository on `main` whose initial commit `init: store <store_id>`
    /// holds `loadout.toml`.
    pub fn open_or_init(
        env: &Env,
        fs: &'a dyn Fs,
        home: &LoadoutHome,
        client: &str,
    ) -> Result<Self, CoreError> {
        if let Some(store) = Self::open(env, fs, home, client)? {
            return Ok(store);
        }
        let state_dir = home.state_dir();
        let _lock = StoreLock::acquire(fs, &state_dir, LOCK_TIMEOUT)?;
        // Another process may have created it while this one waited.
        if let Some(store) = Self::open(env, fs, home, client)? {
            return Ok(store);
        }
        let machine = Machine::load_or_create(env, fs, &state_dir)?;
        let root = home.store_dir();
        // Through `fs` first, so a confined test fails before libgit2 writes.
        fs.create_dir_all(&root)
            .map_err(|err| io_error("cannot create directory", &root, &err))?;
        let repo = if fs.exists(&root.join(".git")) {
            Repository::open(&root)
        } else {
            let mut options = RepositoryInitOptions::new();
            options
                .bare(false)
                .no_reinit(true)
                .mkdir(true)
                .external_template(false)
                .initial_head(BRANCH);
            Repository::init_opts(&root, &options)
        }
        .map_err(git_error)?;

        // Reuse the identity of an interrupted init, if it is intact.
        let identity_path = root.join(IDENTITY_FILE);
        let identity = TomlDocument::<StoreIdentity>::load(fs, &identity_path)
            .ok()
            .map(TomlDocument::into_value)
            .filter(|identity| ids::is_valid(IdKind::Store, &identity.store_id));
        let identity = match identity {
            Some(identity) => identity,
            None => StoreIdentity {
                schema: SCHEMA_VERSION,
                store_id: ids::new_ulid(env.now())?,
                created_at: utc_datetime(env.now()),
                extra: toml::Table::new(),
            },
        };
        let identity_text = TomlDocument::new(identity.clone()).to_text()?;
        let store = Self {
            env: env.clone(),
            fs,
            root,
            state_dir,
            repo,
            identity,
            client: client.to_owned(),
            secrets: Arc::new(SecretMemory::new()?),
        };
        let operation = Operation::new(
            "store.init",
            format!("init: store {}", store.identity.store_id),
        );
        let changes = [
            (IDENTITY_FILE.to_owned(), Some(identity_text.into_bytes())),
            (GITATTRIBUTES.0.to_owned(), Some(GITATTRIBUTES.1.into())),
        ]
        .into_iter()
        .collect();
        store.commit_changes(&machine, &operation, changes)?;
        Ok(store)
    }

    /// The store working tree.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The environment the store was opened with.
    pub fn env(&self) -> &Env {
        &self.env
    }

    /// Hashes of the secret values this process resolved. Share it with
    /// every [`crate::secrets::providers::Resolver`] of the process, so
    /// commits holding a resolved value are refused (SPEC 11.8).
    pub fn secret_memory(&self) -> &Arc<SecretMemory> {
        &self.secrets
    }

    pub fn identity(&self) -> &StoreIdentity {
        &self.identity
    }

    pub fn store_id(&self) -> &str {
        &self.identity.store_id
    }

    /// The commit `main` points at.
    pub fn head(&self) -> Result<CommitId, CoreError> {
        Ok(CommitId::from_oid(self.head_commit()?.id()))
    }

    /// Absolute path of a store-relative path (`servers/github.toml`).
    pub fn path(&self, relative: &str) -> Result<PathBuf, CoreError> {
        check_relative(relative)?;
        Ok(relative
            .split('/')
            .fold(self.root.clone(), |path, part| path.join(part)))
    }

    /// Reads a store file from the working tree; `None` if it does not exist.
    pub fn read_to_string(&self, relative: &str) -> Result<Option<String>, CoreError> {
        let path = self.path(relative)?;
        if !self.fs.exists(&path) {
            return Ok(None);
        }
        self.fs
            .read_to_string(&path)
            .map(Some)
            .map_err(|err| io_error("cannot read file", &path, &err))
    }

    /// The server `id`, parsed but not validated; `None` if it does not exist.
    pub fn server(&self, id: &str) -> Result<Option<TomlDocument<Server>>, CoreError> {
        let path = self.path(&server_path(id)?)?;
        if !self.fs.exists(&path) {
            return Ok(None);
        }
        TomlDocument::load(self.fs, &path).map(Some)
    }

    /// Every `servers/*.toml`, sorted by id, parsed and validated.
    pub fn servers(&self) -> Result<Vec<ServerEntry>, CoreError> {
        let dir = self.root.join("servers");
        if !self.fs.is_dir(&dir) {
            return Ok(Vec::new());
        }
        let paths = self
            .fs
            .read_dir(&dir)
            .map_err(|err| io_error("cannot list directory", &dir, &err))?;
        let entries = paths
            .into_iter()
            .filter_map(|path| {
                let id = path
                    .file_name()?
                    .to_str()?
                    .strip_suffix(".toml")?
                    .to_owned();
                Some((id, path))
            })
            .map(|(id, path)| {
                let document =
                    TomlDocument::<Server>::load(self.fs, &path).and_then(|doc| {
                        match doc.value().validate(Some(&id)).first_error(Some(&path)) {
                            Some(err) => Err(err),
                            None => Ok(doc),
                        }
                    });
                ServerEntry { id, document }
            })
            .collect();
        Ok(entries)
    }

    fn head_commit(&self) -> Result<Commit<'_>, CoreError> {
        self.repo
            .head()
            .and_then(|head| head.peel_to_commit())
            .map_err(git_error)
    }
}

/// `servers/<id>.toml`, after checking `id`.
pub fn server_path(id: &str) -> Result<String, CoreError> {
    ids::check(IdKind::Server, id)?;
    Ok(format!("servers/{id}.toml"))
}

/// Store-relative paths use `/`, name no parent or current directory and
/// never reach into `.git`.
fn check_relative(relative: &str) -> Result<(), CoreError> {
    let valid = !relative.is_empty()
        && relative.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && !part.contains(['\\', ':'])
        })
        && !relative
            .split('/')
            .next()
            .is_some_and(|first| first.eq_ignore_ascii_case(".git"));
    if valid {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorCode::InvalidPath,
            format!("{relative:?} is not a store-relative path"),
        ))
    }
}

/// `time` as a TOML offset date-time in UTC, whole seconds.
pub fn utc_datetime(time: SystemTime) -> Datetime {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let seconds = i64::try_from(seconds).unwrap_or(i64::MAX);
    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    let of_day = seconds.rem_euclid(86_400);
    Datetime {
        date: Some(Date {
            year: u16::try_from(year).unwrap_or(u16::MAX),
            month,
            day,
        }),
        time: Some(Time {
            hour: (of_day / 3600) as u8,
            minute: (of_day % 3600 / 60) as u8,
            second: Some((of_day % 60) as u8),
            nanosecond: None,
        }),
        offset: Some(Offset::Z),
    }
}

/// Proleptic Gregorian date of a day count since 1970-01-01 (Howard
/// Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn git_error(err: git2::Error) -> CoreError {
    CoreError::new(ErrorCode::GitError, err.message().to_owned())
}

fn io_error(what: &str, path: &Path, err: &std::io::Error) -> CoreError {
    CoreError::new(ErrorCode::IoError, format!("{what}: {err}")).with_path(path)
}

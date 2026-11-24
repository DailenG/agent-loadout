//! Client skill folders as they are (SPEC 12.5): each entry is `managed`
//! (a link or copy the ledger owns), `unmanaged` (a real folder or file)
//! or `mismatch` (a link the ledger does not own). Actions: import an
//! unmanaged folder into the store, and move an entry to
//! `backups/trash/` (never a hard delete).
//!
//! Operations take a client, a scope, a project folder and an entry name,
//! never a free path, so they only reach client skill folders.

use std::path::{Path, PathBuf};

use crate::apply::backup::timestamp;
use crate::error::{CoreError, ErrorCode};
use crate::model::applied::Scope;
use crate::model::skill_meta::{SkillSource, SkillSourceKind};
use crate::skills::install::{Installed, Naming, install_files};
use crate::skills::link::{CLIENT_SKILLS, SkillLedger, skills_dir};
use crate::skills::{
    Entry, SKILL_FILE, SkillsEnv, confine, entry, frontmatter, io_error, read_folder,
};
use crate::store::{LOCK_TIMEOUT, Store, StoreLock};

/// Trash folder below `<LOADOUT_HOME>/backups`.
pub const TRASH_DIR: &str = "trash";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryClass {
    /// A link or copy the ledger owns.
    Managed,
    /// A real folder or file.
    Unmanaged,
    /// A link the ledger does not own (to another folder, or to the store
    /// without being recorded).
    Mismatch,
}

impl EntryClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            EntryClass::Managed => "managed",
            EntryClass::Unmanaged => "unmanaged",
            EntryClass::Mismatch => "mismatch",
        }
    }
}

/// One entry of a client skill folder.
#[derive(Debug, Clone)]
pub struct ToolEntry {
    pub name: String,
    pub path: PathBuf,
    pub class: EntryClass,
    /// Where a link points.
    pub link_target: Option<PathBuf>,
    /// The store skill of a managed entry.
    pub skill_id: Option<String>,
    /// `description` from the entry's `SKILL.md`, when readable.
    pub description: Option<String>,
}

/// One client's skill folder in one scope.
#[derive(Debug, Clone)]
pub struct ClientDir {
    pub client: &'static str,
    pub scope: Scope,
    pub dir: PathBuf,
    pub exists: bool,
    pub entries: Vec<ToolEntry>,
}

/// Every Tier A and B client's skill folder in `scope` (`project` for the
/// project scope), classified. Reads only.
pub fn scan(
    sx: &SkillsEnv<'_>,
    scope: Scope,
    project: Option<&Path>,
) -> Result<Vec<ClientDir>, CoreError> {
    let ledger = SkillLedger::load(sx.fs, &sx.home.state_dir())?;
    let mut out: Vec<ClientDir> = Vec::new();
    for client in &CLIENT_SKILLS {
        let dir = skills_dir(sx.env, client.client, scope, project)?;
        let exists = sx.fs.is_dir(&dir);
        let entries = if exists {
            classify_dir(sx, &ledger, &dir)?
        } else {
            Vec::new()
        };
        out.push(ClientDir {
            client: client.client,
            scope,
            dir,
            exists,
            entries,
        });
    }
    Ok(out)
}

/// The entries of one folder, sorted by name; names starting with `.` are
/// left out.
pub fn classify_dir(
    sx: &SkillsEnv<'_>,
    ledger: &SkillLedger,
    dir: &Path,
) -> Result<Vec<ToolEntry>, CoreError> {
    let mut out = Vec::new();
    for path in sx
        .fs
        .read_dir(dir)
        .map_err(|err| io_error("cannot list directory", dir, &err))?
    {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        out.push(classify(sx, ledger, &path, name.to_owned())?);
    }
    Ok(out)
}

fn classify(
    sx: &SkillsEnv<'_>,
    ledger: &SkillLedger,
    path: &Path,
    name: String,
) -> Result<ToolEntry, CoreError> {
    let found = entry(sx.fs, path)?;
    let owned = ledger.get(path);
    let link_target = match &found {
        Entry::Link(target) => Some(target.clone()),
        _ => None,
    };
    let class = match (&found, owned) {
        (_, Some(_)) => EntryClass::Managed,
        (Entry::Link(_), None) => EntryClass::Mismatch,
        _ => EntryClass::Unmanaged,
    };
    let description = sx
        .fs
        .read_to_string(&path.join(SKILL_FILE))
        .ok()
        .and_then(|text| frontmatter::parse(&text).ok().flatten())
        .and_then(|fm| fm.description().map(str::to_owned));
    Ok(ToolEntry {
        name,
        path: path.to_path_buf(),
        class,
        link_target,
        skill_id: owned.map(|link| link.skill_id.clone()),
        description,
    })
}

/// The entry `name` of `client`'s folder; `not_found` when absent.
fn locate(
    sx: &SkillsEnv<'_>,
    client: &str,
    scope: Scope,
    project: Option<&Path>,
    name: &str,
) -> Result<(PathBuf, Entry), CoreError> {
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\', ':']) {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{name:?} is not an entry name"),
        ));
    }
    let path = skills_dir(sx.env, client, scope, project)?.join(name);
    match entry(sx.fs, &path)? {
        Entry::Missing => Err(CoreError::new(
            ErrorCode::NotFound,
            format!("{client} has no skill entry {name:?}"),
        )
        .with_path(&path)),
        found => Ok((path, found)),
    }
}

/// Imports an unmanaged folder of a client into the store (one commit).
/// The folder itself stays as it is.
pub fn import(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    client: &str,
    scope: Scope,
    project: Option<&Path>,
    name: &str,
    naming: &Naming,
) -> Result<Installed, CoreError> {
    let (path, found) = locate(sx, client, scope, project, name)?;
    let ledger = SkillLedger::load(sx.fs, &sx.home.state_dir())?;
    if found != Entry::Dir || ledger.get(&path).is_some() {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{name:?} is not an unmanaged folder; only those can be imported"),
        )
        .with_path(&path));
    }
    let files = read_folder(sx.fs, &path)?;
    let source = SkillSource {
        kind: SkillSourceKind::Local,
        url: None,
        path: None,
        git_ref: None,
        commit: None,
        catalog: None,
        extra: toml::Table::new(),
    };
    install_files(store, sx, naming, files, name.to_owned(), source)
}

/// Moves an entry that is not managed to `backups/trash/<time>-<client>-<name>`
/// (a link moves as a link; what it points to is untouched). Managed
/// entries are disabled instead. Returns the trash path.
pub fn trash(
    sx: &SkillsEnv<'_>,
    client: &str,
    scope: Scope,
    project: Option<&Path>,
    name: &str,
) -> Result<PathBuf, CoreError> {
    let state = sx.home.state_dir();
    let _lock = StoreLock::acquire(sx.fs, &state, LOCK_TIMEOUT)?;
    let (path, _) = locate(sx, client, scope, project, name)?;
    let ledger = SkillLedger::load(sx.fs, &state)?;
    if ledger.get(&path).is_some() {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{name:?} is managed by Loadout; disable the skill instead"),
        )
        .with_path(&path));
    }
    let target = trash_target(sx, client, name)?;
    confine(sx.fs, &path)?;
    std::fs::rename(&path, &target)
        .map_err(|err| io_error("cannot move the entry to the trash", &path, &err))?;
    Ok(target)
}

/// A free `backups/trash/<time>-<client>-<name>` for an entry, its folder
/// created; `already_exists` when that name is taken this second.
pub(crate) fn trash_target(
    sx: &SkillsEnv<'_>,
    client: &str,
    name: &str,
) -> Result<PathBuf, CoreError> {
    let dir = sx.home.backups_dir().join(TRASH_DIR);
    let target = dir.join(format!("{}-{client}-{name}", timestamp(sx.env.now())));
    confine(sx.fs, &target)?;
    if sx.fs.exists(&target) {
        return Err(
            CoreError::new(ErrorCode::AlreadyExists, "the trash entry exists already")
                .with_path(&target),
        );
    }
    sx.fs
        .create_dir_all(&dir)
        .map_err(|err| io_error("cannot create directory", &dir, &err))?;
    Ok(target)
}

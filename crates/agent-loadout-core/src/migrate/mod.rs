//! Migration from SkillDock (SPEC 15.1): `~/.skilldock` into the store.
//!
//! `~/.skilldock` is read only; nothing in it is modified or deleted.
//! [`plan`] is the dry run: it reads SkillDock's data, maps it, runs the
//! post-migration scan ([`scan`]) and returns the [`Report`] without
//! creating or writing anything. [`apply`] tags the store
//! (`auto/pre-migrate/<ts>`), maps again inside one transaction and commits
//! everything as one commit (`migrate: skilldock (...)`, tagged
//! `auto/post-migrate/<ts>`), then records the
//! skill links found pointing into SkillDock's library in the ledger (the
//! next skills sync re-points them to the store copy) and bare-clones
//! SkillDock's backup repository to `<LOADOUT_HOME>/imports`. No client
//! file is written: the first apply is a separate, previewed step.
//!
//! What maps where:
//!
//! - MCP registry records: one `servers/<id>.toml` each; `enabledAppIds`
//!   to `enable.clients`; disabled tools to `[tools] deny`; SkillDock's
//!   `mcp-filter` / `mcp-remote` wrappers unwrapped; plaintext values that
//!   may be secrets replaced by `${secret:<alias>}` and listed in
//!   [`Report::extractions`] for the extraction wizard (the values are
//!   never kept); absolute commands kept and flagged. Plugin-owned records
//!   are skipped.
//! - Skills in `~/.skilldock/skills/<name>`: content without `.git` into
//!   `skills/<id>/`, metadata (tags, git remote and HEAD as `[source]`,
//!   clients from the links on disk) into `skill-meta/<id>.toml`.
//! - Settings: theme, agents CLI compatibility and install activation into
//!   `settings.toml`; the rest listed as skipped.
//! - Credentials, the backup repository, plugins and SkillHub data: listed
//!   with what to do; secrets are never copied.

pub mod scan;
pub mod skilldock;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::apply::ApplyEnv;
use crate::apply::ledger::{Ledger, LedgerKey};
use crate::apply::render::DesiredState;
use crate::brand;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::TomlDocument;
use crate::model::clients::ClientSelection;
use crate::model::server::Server;
use crate::model::settings::{NewItemEnable, Settings, Theme};
use crate::model::skill_meta::{
    DistributionMethod, SkillEnable, SkillMeta, SkillSource, SkillSourceKind,
};
use crate::secrets::classify::{classify_import, find_in_text};
use crate::secrets::scanner::SecretMemory;
use crate::skills::link::{SkillLedger, SkillLink};
use crate::skills::store::{SKILLS_DIR, meta_path};
use crate::skills::{SKILL_FILE, frontmatter, read_folder, slug};
use crate::store::{
    CommitId, LOCK_TIMEOUT, Machine, Operation, Store, StoreLock, StoreRead, server_path,
    utc_datetime,
};

use scan::{FoundLink, MigratedServer};
use skilldock::{Aliases, Extraction, Record, SkillDockSettings};

/// `auto/<kind>/<ts>` tag made before a migration.
pub const TAG_KIND: &str = "pre-migrate";
/// `auto/<kind>/<ts>` tag on the migration commit.
pub const POST_TAG_KIND: &str = "post-migrate";
/// Where SkillDock's backup repository is bare-cloned, under
/// `<LOADOUT_HOME>/imports`.
pub const BACKUP_IMPORT: &str = "skilldock-backup.git";
const SETTINGS_FILE: &str = "settings.toml";

/// What a report item is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// An MCP registry record.
    Server,
    /// A skill folder in SkillDock's library.
    Skill,
    /// A `settings.json` field.
    Setting,
    /// A client entry of a migrated server (`<client>/<name>`).
    ClientEntry,
    /// A link in a client skill folder into SkillDock's library.
    SkillLink,
    Credential,
    Backup,
    Plugin,
    Skillhub,
    /// Any other file or folder of `~/.skilldock`.
    File,
}

impl ItemKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            ItemKind::Server => "server",
            ItemKind::Skill => "skill",
            ItemKind::Setting => "setting",
            ItemKind::ClientEntry => "client_entry",
            ItemKind::SkillLink => "skill_link",
            ItemKind::Credential => "credential",
            ItemKind::Backup => "backup",
            ItemKind::Plugin => "plugin",
            ItemKind::Skillhub => "skillhub",
            ItemKind::File => "file",
        }
    }
}

/// What happened to an item (SPEC 15.1: migrated, needs review, skipped
/// with a reason).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    Migrated,
    NeedsReview,
    Skipped,
}

impl ItemStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            ItemStatus::Migrated => "migrated",
            ItemStatus::NeedsReview => "needs_review",
            ItemStatus::Skipped => "skipped",
        }
    }
}

/// A follow-up the wizard offers for an item. Commands name ids and
/// entries, never paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Offer {
    /// "Replace SkillDock wrapper": overwrite the client's wrapped entry
    /// with the native definition (adopting it).
    ReplaceWrapper {
        client: String,
        name: String,
        command: String,
    },
    /// A different native entry: overwrite it, or keep the client's version.
    ReviewEntry {
        client: String,
        name: String,
        command: String,
    },
    /// A value replaced by a reference: store it (extraction wizard).
    SetSecret { alias: String, command: String },
    /// An absolute command: move it into `[os.*]` or the machine overlay.
    MachinePath { server: String },
    /// SkillDock's GitHub token: move it into the OS keyring for a `token`
    /// remote. Never copied into the store.
    KeyringToken { command: String },
    /// SkillDock's GitHub backup repository: add it as a git remote.
    AddRemote { url: String },
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    pub kind: ItemKind,
    /// Record id, skill folder, setting key, `<client>/<name>`, or a
    /// `~/.skilldock/...` path.
    pub name: String,
    pub status: ItemStatus,
    pub detail: String,
    /// The error code behind a review or skip, if any.
    pub code: Option<&'static str>,
    /// The store id it became (server or skill), if any.
    pub id: Option<String>,
    pub offers: Vec<Offer>,
}

impl Item {
    pub fn new(
        kind: ItemKind,
        name: impl Into<String>,
        status: ItemStatus,
        detail: impl Into<String>,
        code: Option<&'static str>,
    ) -> Self {
        Self {
            kind,
            name: name.into(),
            status,
            detail: detail.into(),
            code,
            id: None,
            offers: Vec::new(),
        }
    }

    fn skipped(kind: ItemKind, name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(kind, name, ItemStatus::Skipped, detail, None)
    }

    fn error(kind: ItemKind, name: impl Into<String>, status: ItemStatus, err: &CoreError) -> Self {
        Self::new(kind, name, status, err.message(), Some(err.code().as_str()))
    }
}

/// The migration report: every item found, with its outcome.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    /// `~/.skilldock`.
    pub source: String,
    pub items: Vec<Item>,
    /// Plaintext values routed to the extraction wizard.
    pub extractions: Vec<Extraction>,
    /// Store server ids created.
    pub servers: Vec<String>,
    /// Store skill ids created.
    pub skills: Vec<String>,
}

impl Report {
    /// `(migrated, needs review, skipped)`.
    pub fn counts(&self) -> (usize, usize, usize) {
        let count = |status| self.items.iter().filter(|i| i.status == status).count();
        (
            count(ItemStatus::Migrated),
            count(ItemStatus::NeedsReview),
            count(ItemStatus::Skipped),
        )
    }
}

/// A computed migration: the report and what [`apply`] writes.
#[derive(Debug, Clone)]
pub struct Migration {
    pub report: Report,
    /// Store-relative path to content.
    files: BTreeMap<String, Vec<u8>>,
    links: Vec<SkillLink>,
    backup: Option<PathBuf>,
}

impl Migration {
    /// Store-relative paths the migration writes, sorted.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }
}

/// What [`apply`] did.
#[derive(Debug, Clone)]
pub struct Applied {
    pub report: Report,
    /// `auto/pre-migrate/<ts>`, on the store as it was.
    pub tag: String,
    /// `None` when nothing was left to migrate.
    pub commit: Option<CommitId>,
    /// `auto/post-migrate/<ts>` on the migration commit (SPEC 9.4).
    pub post_tag: Option<String>,
    /// `imports/skilldock-backup.git`, when the backup was cloned now.
    pub backup_import: Option<String>,
}

/// `~/.skilldock`.
pub fn source_dir(ax: &ApplyEnv<'_>) -> PathBuf {
    ax.env.home().join(skilldock::DIR)
}

/// Whether there is SkillDock data to migrate (`~/.skilldock`, or the
/// registry of `~/.skillm`).
pub fn source_exists(ax: &ApplyEnv<'_>) -> bool {
    ax.fs.is_dir(&source_dir(ax))
        || ax
            .fs
            .exists(&skilldock::resolve(ax.env.home(), skilldock::MCP_FILES[2]))
}

fn require_source(ax: &ApplyEnv<'_>) -> Result<(), CoreError> {
    if source_exists(ax) {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorCode::NotFound,
            "no SkillDock data at ~/.skilldock",
        ))
    }
}

/// The dry run: the migration as [`apply`] would make it now. Creates and
/// writes nothing (no store, ledger key, lock or file).
pub fn plan(store: Option<&Store<'_>>, ax: &ApplyEnv<'_>) -> Result<Migration, CoreError> {
    require_source(ax)?;
    let memory = match store {
        Some(store) => store.secret_memory().clone(),
        None => Arc::new(SecretMemory::new()?),
    };
    build(ax, store.map(|s| s as &dyn StoreRead), memory)
}

/// Migrates: tags the store `auto/pre-migrate/<ts>`, commits the mapped
/// files as one commit (tagged `auto/post-migrate/<ts>`), records the skill
/// links into SkillDock's library
/// as owned (re-pointed by the next skills sync) and bare-clones the
/// backup repository. Confirmation is the caller's (CLI `--yes`, wizard).
pub fn apply(store: &Store<'_>, ax: &ApplyEnv<'_>) -> Result<Applied, CoreError> {
    require_source(ax)?;
    let tag = store.create_auto_tag(TAG_KIND)?;
    let operation = Operation::new("migrate.skilldock", "migrate: skilldock");
    let committed = store.transaction(operation, |tx| {
        let migration = build(ax, Some(&*tx), store.secret_memory().clone())?;
        for (relative, bytes) in &migration.files {
            tx.write(relative, bytes.clone())?;
        }
        for id in &migration.report.servers {
            tx.add_entity(format!("server/{id}"));
        }
        for id in &migration.report.skills {
            tx.add_entity(format!("skill/{id}"));
        }
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let (servers, skills) = (
            migration.report.servers.len(),
            migration.report.skills.len(),
        );
        tx.set_summary(format!(
            "migrate: skilldock ({servers} server{}, {skills} skill{})",
            plural(servers),
            plural(skills)
        ));
        Ok(migration)
    })?;
    let mut migration = committed.value;
    let post_tag = match &committed.commit {
        Some(commit) => Some(store.create_auto_tag_at(POST_TAG_KIND, commit)?),
        None => None,
    };
    if !migration.links.is_empty() {
        let state = ax.home.state_dir();
        let _lock = StoreLock::acquire(ax.fs, &state, LOCK_TIMEOUT)?;
        let mut ledger = SkillLedger::load(ax.fs, &state)?;
        for link in &migration.links {
            if ledger.get(Path::new(&link.path)).is_none() {
                ledger.links.push(link.clone());
            }
        }
        ledger.save(ax.fs, &state)?;
    }
    let mut backup_import = None;
    if let Some(source) = &migration.backup {
        let item = migration
            .report
            .items
            .iter_mut()
            .find(|item| item.kind == ItemKind::Backup && item.status == ItemStatus::Migrated);
        match clone_backup(ax, source) {
            Ok(()) => backup_import = Some(format!("imports/{BACKUP_IMPORT}")),
            Err(err) => {
                if let Some(item) = item {
                    item.status = ItemStatus::NeedsReview;
                    item.detail = format!("the bare clone failed: {}", err.message());
                    item.code = Some(err.code().as_str());
                }
            }
        }
    }
    Ok(Applied {
        report: migration.report,
        tag,
        commit: committed.commit,
        post_tag,
        backup_import,
    })
}

/// `source` bare-cloned (history kept, objects copied, no hard links) to
/// `<LOADOUT_HOME>/imports/skilldock-backup.git`.
fn clone_backup(ax: &ApplyEnv<'_>, source: &Path) -> Result<(), CoreError> {
    let imports = ax.home.imports_dir();
    let dest = imports.join(BACKUP_IMPORT);
    crate::skills::confine(ax.fs, source)?;
    crate::skills::confine(ax.fs, &dest)?;
    ax.fs.create_dir_all(&imports).map_err(|err| {
        CoreError::new(
            ErrorCode::IoError,
            format!("cannot create directory: {err}"),
        )
        .with_path(&imports)
    })?;
    let git = |err: git2::Error| CoreError::new(ErrorCode::GitError, err.message().to_owned());
    let url = source.to_string_lossy().into_owned();
    git2::build::RepoBuilder::new()
        .bare(true)
        .clone_local(git2::build::CloneLocal::NoLinks)
        .clone(&url, &dest)
        .map(|_| ())
        .map_err(git)
}

/// Store files as the base plus the migration's staged ones.
struct Overlay<'a> {
    base: Option<&'a dyn StoreRead>,
    files: &'a BTreeMap<String, Vec<u8>>,
}

impl StoreRead for Overlay<'_> {
    fn read_file(&self, relative: &str) -> Result<Option<String>, CoreError> {
        match self.files.get(relative) {
            Some(bytes) => Ok(Some(String::from_utf8_lossy(bytes).into_owned())),
            None => match self.base {
                Some(base) => base.read_file(relative),
                None => Ok(None),
            },
        }
    }

    fn list_files(&self, dir: &str) -> Result<Vec<String>, CoreError> {
        let mut names = match self.base {
            Some(base) => base.list_files(dir)?,
            None => Vec::new(),
        };
        let prefix = format!("{dir}/");
        for relative in self.files.keys() {
            if let Some(name) = relative.strip_prefix(&prefix)
                && !name.contains('/')
                && !names.iter().any(|known| known == name)
            {
                names.push(name.to_owned());
            }
        }
        names.sort();
        Ok(names)
    }
}

fn read_base(base: Option<&dyn StoreRead>, relative: &str) -> Result<Option<String>, CoreError> {
    match base {
        Some(base) => base.read_file(relative),
        None => Ok(None),
    }
}

/// Maps everything; `base` is the store as it is (or the transaction).
fn build(
    ax: &ApplyEnv<'_>,
    base: Option<&dyn StoreRead>,
    memory: Arc<SecretMemory>,
) -> Result<Migration, CoreError> {
    let fs: &dyn Fs = ax.fs;
    let home = ax.env.home();
    let root = source_dir(ax);
    let mut report = Report {
        source: format!("~/{}", skilldock::DIR),
        ..Report::default()
    };
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    // Paths (`~/...`) the mapping consumed; everything else is listed.
    let mut consumed: BTreeSet<String> = BTreeSet::new();

    let machine = Machine::load(fs, &ax.home.state_dir())?.map(|m| m.id);
    let base_desired = match base {
        Some(base) => DesiredState::load(base, machine.as_deref())?,
        None => DesiredState::empty(),
    };
    let mut aliases = Aliases::new(base_desired.catalog.iter().map(|alias| alias.alias.clone()));

    // MCP registry.
    let mut migrated: BTreeMap<String, MigratedServer> = BTreeMap::new();
    let mut servers: Vec<Server> = Vec::new();
    let mcp = skilldock::first_existing(fs, home, &skilldock::MCP_FILES);
    for display in skilldock::MCP_FILES {
        if mcp.as_ref().is_some_and(|(used, _)| used != display)
            && fs.exists(&skilldock::resolve(home, display))
        {
            consumed.insert(display.to_owned());
            report.items.push(Item::skipped(
                ItemKind::File,
                display,
                format!(
                    "an older copy of the MCP registry; {} is used",
                    mcp.as_ref().expect("some").0
                ),
            ));
        }
    }
    if let Some((display, path)) = &mcp {
        consumed.insert(display.clone());
        let records =
            skilldock::read_json(fs, path, display).and_then(|doc| skilldock::parse_records(&doc));
        match records {
            Err(err) => report.items.push(Item::error(
                ItemKind::File,
                display.clone(),
                ItemStatus::NeedsReview,
                &err,
            )),
            Ok(records) => {
                report.items.push(Item::new(
                    ItemKind::File,
                    display.clone(),
                    ItemStatus::Migrated,
                    format!(
                        "the MCP registry: {} record{}",
                        records.len(),
                        if records.len() == 1 { "" } else { "s" }
                    ),
                    None,
                ));
                for record in records {
                    map_server(
                        record,
                        base,
                        &mut aliases,
                        &mut report,
                        &mut migrated,
                        &mut servers,
                    )?;
                }
            }
        }
    }

    // Skills.
    let mut tags: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut installed = BTreeMap::new();
    for (list, is_state) in [
        (&skilldock::TAG_FILES[..], false),
        (&skilldock::STATE_FILES[..], true),
    ] {
        let used = skilldock::first_existing(fs, home, list);
        for display in list {
            if !fs.exists(&skilldock::resolve(home, display)) {
                continue;
            }
            consumed.insert((*display).to_owned());
            if used.as_ref().is_some_and(|(first, _)| first != display) {
                report.items.push(Item::skipped(
                    ItemKind::File,
                    *display,
                    "an older copy; the newer file is used",
                ));
            }
        }
        if let Some((display, path)) = used {
            match skilldock::read_json(fs, &path, &display) {
                Ok(doc) => {
                    report.items.push(Item::new(
                        ItemKind::File,
                        display.clone(),
                        ItemStatus::Migrated,
                        if is_state {
                            "installed skills (sources) and skill tags"
                        } else {
                            "skill tags"
                        },
                        None,
                    ));
                    skilldock::parse_tags(&doc, &mut tags);
                    if is_state {
                        installed = skilldock::parse_installed(&doc);
                    }
                }
                Err(err) => report.items.push(Item::error(
                    ItemKind::File,
                    display,
                    ItemStatus::NeedsReview,
                    &err,
                )),
            }
        }
    }
    let library = skilldock::resolve(home, skilldock::SKILLS_DIR);
    let mut links: Vec<SkillLink> = Vec::new();
    let mut skill_ids: Vec<String> = Vec::new();
    if fs.is_dir(&library) {
        consumed.insert(skilldock::SKILLS_DIR.to_owned());
        let found = scan::library_links(ax, &library)?;
        let mut dirs = fs.read_dir(&library).map_err(|err| {
            CoreError::new(
                ErrorCode::IoError,
                format!("cannot list ~/.skilldock/skills: {err}"),
            )
        })?;
        dirs.sort();
        let now = utc_datetime(ax.env.now()).to_string();
        for dir in dirs {
            let Some(name) = dir.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            let skill = SkillInput {
                name: &name,
                dir: &dir,
                tags: tags.remove(&name).unwrap_or_default(),
                installed: installed.get(&name),
                links: found.get(&name).map(Vec::as_slice).unwrap_or_default(),
            };
            map_skill(
                ax,
                base,
                &skill,
                &now,
                &mut report,
                &mut files,
                &mut links,
                &mut skill_ids,
            )?;
        }
    }
    for (name, list) in &tags {
        report.items.push(Item::skipped(
            ItemKind::Skill,
            name.clone(),
            format!(
                "tag{} {} for a skill that is not in ~/.skilldock/skills",
                if list.len() == 1 { "" } else { "s" },
                list.join(", ")
            ),
        ));
    }

    // Settings.
    let settings = skilldock::first_existing(fs, home, &skilldock::SETTINGS_FILES);
    for display in skilldock::SETTINGS_FILES {
        if fs.exists(&skilldock::resolve(home, display)) {
            consumed.insert(display.to_owned());
            if settings.as_ref().is_some_and(|(used, _)| used != display) {
                report.items.push(Item::skipped(
                    ItemKind::File,
                    display,
                    "an older copy of the settings; the newer file is used",
                ));
            }
        }
    }
    if let Some((display, path)) = &settings {
        match skilldock::read_json(fs, path, display).and_then(|doc| SkillDockSettings::parse(&doc))
        {
            Ok(parsed) => {
                report.items.push(Item::new(
                    ItemKind::File,
                    display.clone(),
                    ItemStatus::Migrated,
                    "settings; one item per field below",
                    None,
                ));
                map_settings(&parsed, base, &mut report, &mut files)?;
            }
            Err(err) => report.items.push(Item::error(
                ItemKind::File,
                display.clone(),
                ItemStatus::NeedsReview,
                &err,
            )),
        }
    }

    // Credentials, backup and the rest of `~/.skilldock`.
    let backup = list_rest(ax, &root, &consumed, &mut report)?;

    // The post-migration scan.
    for server in &servers {
        let text = TomlDocument::new(server.clone()).to_text()?;
        files.insert(server_path(&server.id)?, text.into_bytes());
    }
    let mut desired = DesiredState::load(
        &Overlay {
            base,
            files: &files,
        },
        machine.as_deref(),
    )?;
    let ledger = Ledger::load(fs, &ax.home.state_dir())?;
    let key = LedgerKey::load(fs, &ax.home.state_dir())?.unwrap_or(LedgerKey::from_bytes([0; 32]));
    let (candidates, warnings) = scan::candidates(ax, &desired, &ledger)?;
    for warning in &warnings {
        report.items.push(Item::error(
            ItemKind::ClientEntry,
            warning
                .path()
                .map(|path| crate::apply::placeholder_path(ax.env, path, None))
                .unwrap_or_default(),
            ItemStatus::NeedsReview,
            warning,
        ));
    }
    let adopted = scan::adopt_identical(ax, memory.clone(), &mut desired, &migrated, &candidates);
    for (id, client) in &adopted {
        let server = desired.servers[id].clone();
        let text = TomlDocument::new(server).to_text()?;
        files.insert(server_path(id)?, text.into_bytes());
        if let Some(item) = report
            .items
            .iter_mut()
            .find(|item| item.kind == ItemKind::Server && item.id.as_deref() == Some(id))
        {
            item.detail
                .push_str(&format!("; enabled for {client} (identical entry found)"));
        }
    }
    let entries = scan::mcp_entries(
        ax,
        memory,
        &desired,
        &ledger,
        &key,
        &migrated,
        &candidates,
        &adopted,
    )?;
    report.items.extend(entries);
    for link in &links {
        report.items.push(Item {
            id: Some(link.skill_id.clone()),
            ..Item::new(
                ItemKind::SkillLink,
                format!("{}/{}", link.client, link_name(link)),
                ItemStatus::Migrated,
                format!(
                    "{} points into ~/.skilldock/skills; re-pointed to the store copy by the next \
                     skills sync (planned as an update)",
                    crate::apply::placeholder_path(ax.env, Path::new(&link.path), None)
                ),
                None,
            )
        });
    }
    report.servers = servers.iter().map(|s| s.id.clone()).collect();
    report.skills = skill_ids;
    Ok(Migration {
        report,
        files,
        links,
        backup,
    })
}

fn link_name(link: &SkillLink) -> String {
    Path::new(&link.path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn map_server(
    record: Result<Record, CoreError>,
    base: Option<&dyn StoreRead>,
    aliases: &mut Aliases,
    report: &mut Report,
    migrated: &mut BTreeMap<String, MigratedServer>,
    servers: &mut Vec<Server>,
) -> Result<(), CoreError> {
    let record = match record {
        Ok(record) => record,
        Err(err) => {
            report.items.push(Item::error(
                ItemKind::Server,
                "?",
                ItemStatus::NeedsReview,
                &err,
            ));
            return Ok(());
        }
    };
    let name = record.id.clone();
    if record.plugin_owned() {
        report.items.push(Item::skipped(
            ItemKind::Server,
            name,
            "installed by a SkillDock plugin; plugins are not migrated",
        ));
        return Ok(());
    }
    let Some(id) = slug(&record.id) else {
        report.items.push(Item::new(
            ItemKind::Server,
            name,
            ItemStatus::NeedsReview,
            "no server id can be made from the record id; add it by hand",
            Some(ErrorCode::InvalidId.as_str()),
        ));
        return Ok(());
    };
    if migrated.contains_key(&id) || read_base(base, &server_path(&id)?)?.is_some() {
        report.items.push(Item::new(
            ItemKind::Server,
            name,
            ItemStatus::Skipped,
            format!("the store already has server {id:?}; left as it is"),
            Some(ErrorCode::AlreadyExists.as_str()),
        ));
        return Ok(());
    }
    let mapped = match skilldock::map_record(&record, &id, aliases) {
        Ok(mapped) => mapped,
        Err(err) => {
            report.items.push(Item::error(
                ItemKind::Server,
                name,
                ItemStatus::NeedsReview,
                &err,
            ));
            return Ok(());
        }
    };
    let mut notes = Vec::new();
    let mut offers = Vec::new();
    let mut review = false;
    if let Some(kind) = mapped.unwrapped {
        notes.push(format!("unwrapped from SkillDock's {kind} wrapper"));
    }
    if let Some(tools) = &mapped.server.body.tools {
        if let Some(deny) = &tools.deny {
            notes.push(format!("tools denied: {}", deny.join(", ")));
        }
        if let Some(allow) = &tools.allow {
            notes.push(format!("tools allowed: {}", allow.join(", ")));
        }
    }
    match mapped
        .server
        .enable
        .as_ref()
        .and_then(|e| e.clients.as_ref())
    {
        Some(ClientSelection::List(ids)) => notes.push(format!("enabled for {}", ids.join(", "))),
        _ => notes.push("SkillDock enabled it for no client".to_owned()),
    }
    if !mapped.unknown_clients.is_empty() {
        review = true;
        notes.push(format!(
            "no {} adapter for {}",
            brand::PRODUCT_NAME,
            mapped.unknown_clients.join(", ")
        ));
    }
    if mapped.machine_path {
        review = true;
        notes.push(
            "machine-specific path: the command is absolute; consider moving it into [os.*] or \
             the machine overlay"
                .to_owned(),
        );
        offers.push(Offer::MachinePath { server: id.clone() });
    }
    for extraction in &mapped.extractions {
        review = true;
        notes.push(format!(
            "{} may be a secret; replaced by ${{secret:{}}}",
            extraction.field, extraction.alias
        ));
        offers.push(Offer::SetSecret {
            alias: extraction.alias.clone(),
            command: format!(
                "{} secrets set {} --keyring",
                brand::BIN_NAME,
                extraction.alias
            ),
        });
    }
    if !mapped.dropped.is_empty() {
        review = true;
        notes.push(format!(
            "fields with no store equivalent dropped: {}",
            mapped.dropped.join(", ")
        ));
    }
    let mut item = Item::new(
        ItemKind::Server,
        name,
        if review {
            ItemStatus::NeedsReview
        } else {
            ItemStatus::Migrated
        },
        notes.join("; "),
        None,
    );
    item.id = Some(id.clone());
    item.offers = offers;
    report.items.push(item);
    report.extractions.extend(mapped.extractions);
    migrated.insert(
        id,
        MigratedServer {
            managed: record.managed_app_ids.clone(),
            imported_from: record.imported_from_app_ids.clone(),
        },
    );
    servers.push(mapped.server);
    Ok(())
}

struct SkillInput<'a> {
    name: &'a str,
    dir: &'a Path,
    tags: Vec<String>,
    installed: Option<&'a skilldock::InstalledSkill>,
    links: &'a [FoundLink],
}

#[allow(clippy::too_many_arguments)]
fn map_skill(
    ax: &ApplyEnv<'_>,
    base: Option<&dyn StoreRead>,
    skill: &SkillInput<'_>,
    now: &str,
    report: &mut Report,
    files: &mut BTreeMap<String, Vec<u8>>,
    links: &mut Vec<SkillLink>,
    skill_ids: &mut Vec<String>,
) -> Result<(), CoreError> {
    let fs = ax.fs;
    let name = skill.name;
    let display = format!("{}/{name}", skilldock::SKILLS_DIR);
    match crate::skills::entry(fs, skill.dir)? {
        crate::skills::Entry::Dir => {}
        crate::skills::Entry::Link(_) => {
            report.items.push(Item::new(
                ItemKind::Skill,
                name,
                ItemStatus::NeedsReview,
                format!("{display} is a link; install the folder it points to with `skills add`"),
                Some(ErrorCode::InvalidPath.as_str()),
            ));
            return Ok(());
        }
        _ => {
            report
                .items
                .push(Item::skipped(ItemKind::File, display, "not a skill folder"));
            return Ok(());
        }
    }
    let content = match read_folder(fs, skill.dir) {
        Ok(content) => content,
        Err(err) => {
            report.items.push(Item::error(
                ItemKind::Skill,
                name,
                ItemStatus::NeedsReview,
                &err,
            ));
            return Ok(());
        }
    };
    let Some(skill_md) = content.get(SKILL_FILE) else {
        report.items.push(Item::new(
            ItemKind::Skill,
            name,
            ItemStatus::Skipped,
            format!("no {SKILL_FILE}"),
            Some(ErrorCode::NotFound.as_str()),
        ));
        return Ok(());
    };
    if let Some(file) = content.iter().find_map(|(relative, bytes)| {
        std::str::from_utf8(bytes)
            .ok()
            .filter(|text| !find_in_text(text).is_empty())
            .map(|_| relative)
    }) {
        report.items.push(Item::new(
            ItemKind::Skill,
            name,
            ItemStatus::NeedsReview,
            format!(
                "{file} looks like it holds a secret, which the store refuses; remove it, then \
                 install the folder with `skills add`"
            ),
            Some(ErrorCode::LeakDetected.as_str()),
        ));
        return Ok(());
    }
    let mut notes = Vec::new();
    let parsed = match frontmatter::parse(&String::from_utf8_lossy(skill_md)) {
        Ok(parsed) => parsed,
        Err(err) => {
            notes.push(format!("frontmatter: {}", err.message()));
            None
        }
    };
    let skill_name = parsed
        .as_ref()
        .and_then(|fm| fm.name())
        .unwrap_or(name)
        .to_owned();
    let Some(id) = slug(&skill_name).or_else(|| slug(name)) else {
        report.items.push(Item::new(
            ItemKind::Skill,
            name,
            ItemStatus::NeedsReview,
            "no skill id can be made from the name; install it by hand",
            Some(ErrorCode::InvalidId.as_str()),
        ));
        return Ok(());
    };
    let taken = skill_ids.contains(&id)
        || read_base(base, &meta_path(&id)?)?.is_some()
        || read_base(base, &format!("{SKILLS_DIR}/{id}/{SKILL_FILE}"))?.is_some();
    if taken {
        report.items.push(Item::new(
            ItemKind::Skill,
            name,
            ItemStatus::Skipped,
            format!("the store already has skill {id:?}; left as it is"),
            Some(ErrorCode::AlreadyExists.as_str()),
        ));
        return Ok(());
    }
    let mut review = false;
    let mut meta = SkillMeta::new(&id);
    meta.name = Some(skill_name);
    if !skill.tags.is_empty() {
        notes.push(format!("tags: {}", skill.tags.join(", ")));
        meta.tags = Some(skill.tags.clone());
    }
    let (source, source_note) = skill_source(skill.dir, skill.installed);
    if let Some(note) = source_note {
        review = true;
        notes.push(note);
    }
    match &source {
        SkillSource {
            kind: SkillSourceKind::Git,
            url: Some(url),
            ..
        } => notes.push(format!("source: {url}")),
        _ => notes.push("source: local".to_owned()),
    }
    meta.source = Some(source);
    let mut clients: Vec<String> = Vec::new();
    for link in skill.links {
        if !clients.iter().any(|known| known == link.client) {
            clients.push(link.client.to_owned());
        }
        links.push(SkillLink {
            kind: "skill_link".to_owned(),
            client: link.client.to_owned(),
            scope: crate::model::applied::Scope::User,
            project_id: None,
            skill_id: id.clone(),
            path: link.path.to_string_lossy().into_owned(),
            method: link.method,
            target: link.target.to_string_lossy().into_owned(),
            written_hash: None,
            written_at: Some(now.to_owned()),
            extra: serde_json::Map::new(),
        });
    }
    if clients.is_empty() {
        notes.push("not linked into any client".to_owned());
    } else {
        notes.push(format!(
            "enabled for {} (links on disk)",
            clients.join(", ")
        ));
        meta.enable = Some(SkillEnable {
            clients: Some(ClientSelection::List(clients)),
            method: Some(DistributionMethod::Link),
            extra: toml::Table::new(),
        });
    }
    if let Some(err) = meta.validate(Some(&id)).first_error(None) {
        report.items.push(Item::error(
            ItemKind::Skill,
            name,
            ItemStatus::NeedsReview,
            &err,
        ));
        return Ok(());
    }
    for (relative, bytes) in &content {
        files.insert(format!("{SKILLS_DIR}/{id}/{relative}"), bytes.clone());
    }
    files.insert(
        meta_path(&id)?,
        TomlDocument::new(meta).to_text()?.into_bytes(),
    );
    let mut item = Item::new(
        ItemKind::Skill,
        name,
        if review {
            ItemStatus::NeedsReview
        } else {
            ItemStatus::Migrated
        },
        notes.join("; "),
        None,
    );
    item.id = Some(id.clone());
    report.items.push(item);
    skill_ids.push(id);
    Ok(())
}

/// `[source]` from the skill folder's git repository (remote `origin` and
/// HEAD, read only), else from `state.json`, else `local`. A URL that may
/// hold a credential is left out (with a note).
fn skill_source(
    dir: &Path,
    installed: Option<&skilldock::InstalledSkill>,
) -> (SkillSource, Option<String>) {
    let mut source = SkillSource {
        kind: SkillSourceKind::Local,
        url: None,
        path: None,
        git_ref: None,
        commit: None,
        extra: toml::Table::new(),
    };
    let mut url = None;
    if dir.join(".git").exists()
        && let Ok(repo) = git2::Repository::open_ext(
            dir,
            git2::RepositoryOpenFlags::NO_SEARCH,
            std::iter::empty::<&std::ffi::OsStr>(),
        )
    {
        url = repo
            .find_remote("origin")
            .ok()
            .and_then(|remote| remote.url().ok().map(str::to_owned));
        if let Ok(head) = repo.head() {
            source.commit = head.peel_to_commit().ok().map(|c| c.id().to_string());
            if head.is_branch() {
                source.git_ref = head.shorthand().ok().map(str::to_owned);
            }
        }
    }
    if url.is_none()
        && let Some(installed) = installed
        && !installed.source_url.is_empty()
        && installed.source_type != "local"
    {
        url = Some(installed.source_url.clone());
        if source.git_ref.is_none() && !installed.branch.is_empty() {
            source.git_ref = Some(installed.branch.clone());
        }
    }
    let mut note = None;
    match url {
        Some(url) if classify_import(None, &url).is_some() => {
            note = Some(
                "the source URL may hold a credential and was left out; reinstall from a clean URL \
                 for updates"
                    .to_owned(),
            );
            source.git_ref = None;
            source.commit = None;
        }
        Some(url) => {
            source.kind = SkillSourceKind::Git;
            source.url = Some(url);
        }
        None => {
            source.git_ref = None;
            source.commit = None;
        }
    }
    (source, note)
}

/// Maps `settings.json` onto `settings.toml`. A field the store already
/// sets differently is kept and listed as skipped.
fn map_settings(
    parsed: &SkillDockSettings,
    base: Option<&dyn StoreRead>,
    report: &mut Report,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), CoreError> {
    let mut doc = match read_base(base, SETTINGS_FILE)? {
        Some(text) => TomlDocument::<Settings>::parse(&text).map_err(|d| d.to_core_error(None))?,
        None => TomlDocument::new(Settings::new()),
    };
    let before = doc.value().clone();
    let settings = doc.value_mut();
    for (key, value) in &parsed.fields {
        let item = |status, detail: String| {
            Item::new(ItemKind::Setting, key.clone(), status, detail, None)
        };
        match key.as_str() {
            "theme" => {
                let theme = match parsed.text(key) {
                    Some("light") => Some(Theme::Light),
                    Some("dark") => Some(Theme::Dark),
                    Some("system") => Some(Theme::System),
                    _ => None,
                };
                report.items.push(match theme {
                    None => item(ItemStatus::Skipped, "not a theme Loadout knows".to_owned()),
                    Some(theme) => set(key, &mut settings.theme, theme, "theme", |t| format!("{t:?}").to_lowercase()),
                });
            }
            "agentSkillsCompatibilityEnabled" => {
                let Some(on) = parsed.bool(key) else {
                    report.items.push(item(ItemStatus::Skipped, "not a true/false value".to_owned()));
                    continue;
                };
                // The agents CLI library as SkillDock's library also means compatibility.
                let on = on || parsed.text("skillLibraryProvider") == Some("agent-skills");
                report.items.push(set(key, &mut settings.agents_cli_compat, on, "agents_cli_compat", |v| v.to_string()));
            }
            "skillInstallActivation" | "mcpInstallActivation" => {
                let target = if key == "skillInstallActivation" {
                    (&mut settings.new_skill_enable, "new_skill_enable")
                } else {
                    (&mut settings.new_server_enable, "new_server_enable")
                };
                let value = match parsed.text(key) {
                    Some("apply-all-tools") => Some(NewItemEnable::AllDetected),
                    Some("disable-all-tools") => Some(NewItemEnable::None),
                    _ => None,
                };
                report.items.push(match value {
                    None => item(ItemStatus::Skipped, "not an activation mode Loadout knows".to_owned()),
                    Some(value) => set(key, target.0, value, target.1, |v| match v {
                        NewItemEnable::AllDetected => "all-detected".to_owned(),
                        NewItemEnable::None => "none".to_owned(),
                    }),
                });
            }
            "skillLibraryProvider" => report.items.push(item(
                ItemStatus::Skipped,
                match parsed.text(key) {
                    Some("agent-skills") => {
                        "the library was the agents CLI folder; it is scanned read-only with agents CLI compatibility".to_owned()
                    }
                    _ => "SkillDock's own library is migrated from ~/.skilldock/skills".to_owned(),
                },
            )),
            "defaultOpenToolId" => report.items.push(item(
                ItemStatus::Skipped,
                "no portable setting; the default editor is chosen per machine".to_owned(),
            )),
            "language" | "languageSource" => report.items.push(item(
                ItemStatus::Skipped,
                "Loadout is English only".to_owned(),
            )),
            "skillSourceViewStyle" | "skillTagFilterLayout" => report.items.push(item(
                ItemStatus::Skipped,
                "a SkillDock layout preference".to_owned(),
            )),
            "githubToken" => {
                if parsed.text(key).is_some() {
                    let mut review = item(
                        ItemStatus::NeedsReview,
                        "a GitHub token; never copied into the store. Move it into the OS keyring \
                         for a git remote with token auth"
                            .to_owned(),
                    );
                    review.kind = ItemKind::Credential;
                    review.offers.push(token_offer());
                    report.items.push(review);
                }
            }
            "githubConnection" => report.items.push(item(
                ItemStatus::Skipped,
                "GitHub account details; remotes are added in Loadout".to_owned(),
            )),
            "githubBackup" => {
                let url = value
                    .get("repositoryUrl")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|url| !url.is_empty() && classify_import(None, url).is_none());
                let enabled = value.get("enabled").and_then(serde_json::Value::as_bool) == Some(true);
                report.items.push(match url {
                    Some(url) if enabled => {
                        let mut review = item(
                            ItemStatus::NeedsReview,
                            format!("SkillDock backed up to {url}; add it as a backup remote if wanted"),
                        );
                        review.offers.push(Offer::AddRemote { url: url.to_owned() });
                        review
                    }
                    _ => item(ItemStatus::Skipped, "GitHub backup was not enabled".to_owned()),
                });
            }
            _ => report.items.push(item(ItemStatus::Skipped, "a setting with no Loadout equivalent".to_owned())),
        }
    }
    if *doc.value() != before {
        files.insert(SETTINGS_FILE.to_owned(), doc.to_text()?.into_bytes());
    }
    Ok(())
}

/// Sets `slot` to `value` unless the store already holds another value;
/// the item is named after SkillDock's `key`.
fn set<T: PartialEq + Copy>(
    key: &str,
    slot: &mut Option<T>,
    value: T,
    field: &str,
    show: impl Fn(T) -> String,
) -> Item {
    let item = |status, detail| Item::new(ItemKind::Setting, key, status, detail, None);
    match *slot {
        Some(current) if current != value => item(
            ItemStatus::Skipped,
            format!("the store already sets {field} = {}; kept", show(current)),
        ),
        Some(_) => item(
            ItemStatus::Migrated,
            format!("{field} = {} (already set)", show(value)),
        ),
        None => {
            *slot = Some(value);
            item(ItemStatus::Migrated, format!("{field} = {}", show(value)))
        }
    }
}

fn token_offer() -> Offer {
    Offer::KeyringToken {
        command: format!(
            "{bin} remote add git <NAME> <URL> --auth token, then pipe the token to `{bin} remote \
             token <NAME>`",
            bin = brand::BIN_NAME
        ),
    }
}

/// Lists every entry of `~/.skilldock` the mapping did not consume; returns
/// the backup repository to clone, if there is one to clone.
fn list_rest(
    ax: &ApplyEnv<'_>,
    root: &Path,
    consumed: &BTreeSet<String>,
    report: &mut Report,
) -> Result<Option<PathBuf>, CoreError> {
    let fs = ax.fs;
    let mut backup = None;
    if !fs.is_dir(root) {
        return Ok(None);
    }
    let list = |dir: &Path| -> Vec<PathBuf> {
        let mut paths = fs.read_dir(dir).unwrap_or_default();
        paths.sort();
        paths
    };
    for path in list(root) {
        let top = file_name(&path);
        let display = format!("~/{}/{top}", skilldock::DIR);
        if consumed.contains(&display) {
            continue;
        }
        match top.as_str() {
            "config" | "data" | "credentials" | "backup" if fs.is_dir(&path) => {
                for child in list(&path) {
                    let child_display = format!("{display}/{}", file_name(&child));
                    if consumed.contains(&child_display) {
                        continue;
                    }
                    if child_display == skilldock::BACKUP_REPO {
                        backup = backup_item(ax, &child, report);
                        continue;
                    }
                    report
                        .items
                        .push(rest_item(fs, &child, &child_display, top.as_str()));
                }
            }
            _ => report.items.push(rest_item(fs, &path, &display, "")),
        }
    }
    Ok(backup)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn rest_item(fs: &dyn Fs, path: &Path, display: &str, parent: &str) -> Item {
    let name = file_name(path);
    match (parent, name.as_str()) {
        ("credentials", "github.json") | ("", "github-credentials.json") => {
            let has_token = fs
                .read_to_string(path)
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|doc| {
                    doc.get("token")
                        .and_then(serde_json::Value::as_str)
                        .map(|token| !token.trim().is_empty())
                })
                .unwrap_or(false);
            if has_token {
                let mut item = Item::new(
                    ItemKind::Credential,
                    display,
                    ItemStatus::NeedsReview,
                    "SkillDock's GitHub token; never copied into the store. Move it into the OS \
                     keyring for a git remote with token auth",
                    None,
                );
                item.offers.push(token_offer());
                item
            } else {
                Item::skipped(ItemKind::Credential, display, "holds no token")
            }
        }
        ("credentials", "skillhub.json") | ("", "skillhub-auth.json") => Item::skipped(
            ItemKind::Skillhub,
            display,
            "SkillHub sign-in; SkillHub is not migrated",
        ),
        ("credentials", _) => Item::skipped(
            ItemKind::Credential,
            display,
            "a SkillDock credential with no Loadout use; never copied",
        ),
        ("data", "publishing") => Item::skipped(
            ItemKind::Skillhub,
            display,
            "SkillHub publishing state; SkillHub is not migrated",
        ),
        ("", "plugins") => Item::skipped(
            ItemKind::Plugin,
            display,
            "SkillDock plugins are not migrated",
        ),
        ("", "repositories" | "repo-cache" | "cache" | "logs" | "imports") => Item::skipped(
            ItemKind::File,
            display,
            "SkillDock caches and logs; nothing to migrate",
        ),
        ("backup", _) => Item::skipped(
            ItemKind::Backup,
            display,
            "SkillDock backup bookkeeping; the repository itself is imported",
        ),
        _ => Item::skipped(ItemKind::File, display, "not used by the migration"),
    }
}

/// The backup repository's item; the path to clone when it is a git
/// repository not imported yet.
fn backup_item(ax: &ApplyEnv<'_>, repo: &Path, report: &mut Report) -> Option<PathBuf> {
    let dest = ax.home.imports_dir().join(BACKUP_IMPORT);
    let display = skilldock::BACKUP_REPO;
    let is_repo = ax.fs.exists(&repo.join(".git")) || ax.fs.exists(&repo.join("HEAD"));
    if !is_repo {
        report.items.push(Item::skipped(
            ItemKind::Backup,
            display,
            "not a git repository",
        ));
        return None;
    }
    if ax.fs.exists(&dest) {
        report.items.push(Item::new(
            ItemKind::Backup,
            display,
            ItemStatus::Skipped,
            format!("already imported as imports/{BACKUP_IMPORT}"),
            Some(ErrorCode::AlreadyExists.as_str()),
        ));
        return None;
    }
    report.items.push(Item::new(
        ItemKind::Backup,
        display,
        ItemStatus::Migrated,
        format!(
            "bare-cloned to imports/{BACKUP_IMPORT} (history kept, read-only); one backup's \
             servers and skills can be imported from there"
        ),
        None,
    ));
    Some(repo.to_path_buf())
}

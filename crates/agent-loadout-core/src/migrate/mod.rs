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
use crate::secrets::SecretValue;
use crate::secrets::catalog::{self, AliasUpdate, SetScope};
use crate::secrets::classify::{classify_import, find_in_text};
use crate::secrets::grammar::SecretRef;
use crate::secrets::providers::Resolver;
use crate::secrets::providers::onepassword::OpCli;
use crate::secrets::scanner::{ALLOWLIST_FILE, Allowlist, SecretMemory, scan_text};
use crate::skills::install::Source;
use crate::skills::link::{SkillLedger, SkillLink};
use crate::skills::store::{SKILLS_DIR, meta_path, read_meta};
use crate::skills::{SKILL_FILE, frontmatter, read_folder, slug};
use crate::store::{
    CommitId, LOCK_TIMEOUT, Machine, Operation, Store, StoreLock, StoreRead, server_path,
    utc_datetime,
};

use scan::{FoundLink, MigratedServer};
use skilldock::{Aliases, Extraction, InstalledSkill, Record, SkillDockSettings, TagEntry};

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

/// Where the wizard sends a value the migration replaced by a reference
/// (SPEC 11.7).
pub enum ExtractionTarget {
    /// The OS keyring, as `keyring://loadout/<alias>`; the value goes one
    /// way and is never stored or returned.
    Keyring(SecretValue),
    /// A 1Password or environment reference (`op://...`, `op-env://...`,
    /// `env:NAME`).
    Reference(SecretRef),
}

/// The destination of one extraction, by alias.
pub struct ExtractionChoice {
    pub alias: String,
    pub target: ExtractionTarget,
}

/// [`apply_with_choices`] without extraction choices (the CLI: values are
/// set afterwards with `loadout secrets set`).
pub fn apply(store: &Store<'_>, ax: &ApplyEnv<'_>) -> Result<Applied, CoreError> {
    apply_with_choices(store, ax, &[])
}

/// Migrates: tags the store `auto/pre-migrate/<ts>`, commits the mapped
/// files as one commit (tagged `auto/post-migrate/<ts>`), records the skill
/// links into SkillDock's library
/// as owned (re-pointed by the next skills sync) and bare-clones the
/// backup repository. Confirmation is the caller's (CLI `--yes`, wizard).
///
/// `choices` (the wizard's extraction step) become `secrets.toml` aliases
/// in the same commit; keyring values are written through the resolver
/// (so the scanner refuses them in the store) right before the commit.
/// Each alias must be one of this migration's extractions. No client file
/// is written (no apply).
pub fn apply_with_choices(
    store: &Store<'_>,
    ax: &ApplyEnv<'_>,
    choices: &[ExtractionChoice],
) -> Result<Applied, CoreError> {
    require_source(ax)?;
    let tag = store.create_auto_tag(TAG_KIND)?;
    let operation = Operation::new("migrate.skilldock", "migrate: skilldock");
    let committed = store.transaction(operation, |tx| {
        let migration = build(ax, Some(&*tx), store.secret_memory().clone())?;
        for choice in choices {
            if !migration
                .report
                .extractions
                .iter()
                .any(|extraction| extraction.alias == choice.alias)
            {
                return Err(CoreError::new(
                    ErrorCode::InvalidValue,
                    format!("{:?} is not a value this migration extracts", choice.alias),
                ));
            }
            if matches!(
                choice.target,
                ExtractionTarget::Reference(SecretRef::Keyring { .. })
            ) {
                return Err(CoreError::new(
                    ErrorCode::InvalidValue,
                    "a keyring destination takes the value, not a reference",
                ));
            }
        }
        for (relative, bytes) in &migration.files {
            tx.write(relative, bytes.clone())?;
        }
        for id in &migration.report.servers {
            tx.add_entity(format!("server/{id}"));
        }
        for id in &migration.report.skills {
            tx.add_entity(format!("skill/{id}"));
        }
        if !choices.is_empty() {
            let resolver = Resolver::new(
                ax.env,
                ax.delivery.keyring(),
                &OpCli::NotFound,
                store.secret_memory().clone(),
            );
            for choice in choices {
                let reference = match &choice.target {
                    ExtractionTarget::Keyring(value) => {
                        resolver
                            .set_keyring(brand::KEYRING_SERVICE, &choice.alias, value)
                            .map_err(|resolution| {
                                resolution.into_error(&format!("cannot store {}", choice.alias))
                            })?;
                        SecretRef::Keyring {
                            service: brand::KEYRING_SERVICE.to_owned(),
                            account: choice.alias.clone(),
                        }
                    }
                    ExtractionTarget::Reference(reference) => reference.clone(),
                };
                let update = AliasUpdate {
                    reference,
                    description: None,
                    env_name: None,
                };
                catalog::stage_set(tx, &choice.alias, &update, SetScope::Store)?;
            }
            tx.add_entity("secrets".to_owned());
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
    let mut tags: Vec<TagEntry> = Vec::new();
    let mut installed: Vec<InstalledSkill> = Vec::new();
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
                        installed = skilldock::parse_installed(&doc, &mut tags);
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
    let mut tag_used = vec![false; tags.len()];
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
        let cx = SkillCx {
            ax,
            base,
            now: utc_datetime(ax.env.now()).to_string(),
            memory: &memory,
            allow: Allowlist::parse(&read_base(base, ALLOWLIST_FILE)?.unwrap_or_default()),
            ledger: SkillLedger::load(fs, &ax.home.state_dir())?,
            tags: &tags,
            installed: &installed,
        };
        let mut out = SkillOut {
            report: &mut report,
            files: &mut files,
            links: &mut links,
            skill_ids: &mut skill_ids,
            tag_used: &mut tag_used,
        };
        for dir in dirs {
            let name = file_name(&dir);
            if name.is_empty() {
                continue;
            }
            let found = found.get(&name).map(Vec::as_slice).unwrap_or_default();
            map_skill(&cx, &name, &dir, found, &mut out)?;
        }
    }
    // Tags of skills that are not in the library (the item names the
    // skill, never SkillDock's key, which holds an absolute path).
    let mut unmatched: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (entry, used) in tags.iter().zip(&tag_used) {
        if !used {
            let list = unmatched.entry(entry.name.clone()).or_default();
            if !list.contains(&entry.tag) {
                list.push(entry.tag.clone());
            }
        }
    }
    for (name, list) in &unmatched {
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
    notes.extend(mapped.notes.iter().cloned());
    if !mapped.review.is_empty() {
        review = true;
        notes.extend(mapped.review.iter().cloned());
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

/// What every skill folder is mapped against.
struct SkillCx<'a> {
    ax: &'a ApplyEnv<'a>,
    base: Option<&'a dyn StoreRead>,
    now: String,
    /// The store's secret memory and allowlist: skill files get the same
    /// leak scan as the commit (SPEC 11.8), so one file cannot fail the
    /// whole migration.
    memory: &'a SecretMemory,
    allow: Allowlist,
    /// Skill links this machine owns already.
    ledger: SkillLedger,
    tags: &'a [TagEntry],
    installed: &'a [InstalledSkill],
}

/// What the skill mapping adds to.
struct SkillOut<'a> {
    report: &'a mut Report,
    files: &'a mut BTreeMap<String, Vec<u8>>,
    links: &'a mut Vec<SkillLink>,
    skill_ids: &'a mut Vec<String>,
    /// Per [`SkillCx::tags`] entry: matched a skill folder.
    tag_used: &'a mut Vec<bool>,
}

fn skill_link(cx: &SkillCx<'_>, link: &FoundLink, id: &str) -> SkillLink {
    SkillLink {
        kind: "skill_link".to_owned(),
        client: link.client.to_owned(),
        scope: crate::model::applied::Scope::User,
        project_id: None,
        skill_id: id.to_owned(),
        path: link.path.to_string_lossy().into_owned(),
        method: link.method,
        target: link.target.to_string_lossy().into_owned(),
        written_hash: None,
        written_at: Some(cx.now.clone()),
        extra: serde_json::Map::new(),
    }
}

/// A link the sync could not re-point: its name is not the skill id, so
/// the sync would add a second link at `<id>` and remove this one.
fn misnamed_link(link: &FoundLink, id: &str) -> Item {
    Item {
        id: Some(id.to_owned()),
        ..Item::new(
            ItemKind::SkillLink,
            format!("{}/{}", link.client, file_name(&link.path)),
            ItemStatus::NeedsReview,
            format!(
                "{} points into ~/.skilldock/skills but is not named {id:?}, the skill's id; left \
                 as it is and not enabled for {} (rename it to {id:?} or remove it, then enable \
                 the skill)",
                link.display, link.client
            ),
            None,
        )
    }
}

fn map_skill(
    cx: &SkillCx<'_>,
    name: &str,
    dir: &Path,
    found: &[FoundLink],
    out: &mut SkillOut<'_>,
) -> Result<(), CoreError> {
    let fs = cx.ax.fs;
    let report = &mut *out.report;
    let display = format!("{}/{name}", skilldock::SKILLS_DIR);
    match crate::skills::entry(fs, dir)? {
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
    let content = match read_folder(fs, dir) {
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
    // SkillDock's tags and install records name the skill by folder (its
    // path) or by name.
    let names = [name, skill_name.as_str()];
    let mut tags: Vec<String> = Vec::new();
    for (index, entry) in cx.tags.iter().enumerate() {
        if entry.matches(name, &names) {
            out.tag_used[index] = true;
            if !tags.contains(&entry.tag) {
                tags.push(entry.tag.clone());
            }
        }
    }
    let installed = cx.installed.iter().find(|s| s.matches(name, &names));
    if let Some(file) = content.iter().find_map(|(relative, bytes)| {
        let findings = scan_text(
            relative,
            bytes,
            relative.ends_with(".toml"),
            cx.memory,
            &cx.allow,
        );
        (!findings.is_empty()).then_some(relative)
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
    // Only a link named like the id is the entry the sync re-points.
    let (named, misnamed): (Vec<&FoundLink>, Vec<&FoundLink>) =
        found.iter().partition(|link| file_name(&link.path) == id);
    for link in &misnamed {
        report.items.push(misnamed_link(link, &id));
    }
    if out.skill_ids.contains(&id) {
        report.items.push(Item::new(
            ItemKind::Skill,
            name,
            ItemStatus::NeedsReview,
            format!("another SkillDock skill already became {id:?}; install this one by hand"),
            Some(ErrorCode::AlreadyExists.as_str()),
        ));
        for link in &named {
            report.items.push(misnamed_link(link, &id));
        }
        return Ok(());
    }
    let existing = match cx.base {
        Some(base) => read_meta(base, &id)?
            .map(|meta| meta.enable.and_then(|e| e.clients))
            .or_else(|| {
                read_base(Some(base), &format!("{SKILLS_DIR}/{id}/{SKILL_FILE}"))
                    .ok()
                    .flatten()
                    .map(|_| None)
            }),
        None => None,
    };
    if let Some(enabled) = existing {
        // Already in the store (an earlier run, or the user's own skill):
        // its links are still owned now, so a run after a failed
        // post-commit step repairs them. A client the store copy is not
        // enabled for is left to the user (the sync would remove it).
        let mut recorded = 0;
        for link in &named {
            if cx.ledger.get(&link.path).is_some() {
                continue;
            }
            let wanted = match &enabled {
                Some(ClientSelection::AllDetected) => true,
                Some(ClientSelection::List(ids)) => ids.iter().any(|c| c == link.client),
                None => false,
            };
            if wanted {
                out.links.push(skill_link(cx, link, &id));
                recorded += 1;
            } else {
                report.items.push(Item {
                    id: Some(id.clone()),
                    ..Item::new(
                        ItemKind::SkillLink,
                        format!("{}/{}", link.client, file_name(&link.path)),
                        ItemStatus::NeedsReview,
                        format!(
                            "{} points into ~/.skilldock/skills; the store's skill {id:?} is not \
                             enabled for {}, so it is left as it is",
                            link.display, link.client
                        ),
                        None,
                    )
                });
            }
        }
        report.items.push(Item {
            id: Some(id.clone()),
            ..Item::new(
                ItemKind::Skill,
                name,
                ItemStatus::Skipped,
                if recorded == 0 {
                    format!("the store already has skill {id:?}; left as it is")
                } else {
                    format!(
                        "the store already has skill {id:?}; left as it is, its {recorded} \
                         link{} recorded",
                        if recorded == 1 { "" } else { "s" }
                    )
                },
                Some(ErrorCode::AlreadyExists.as_str()),
            )
        });
        return Ok(());
    }
    let mut review = false;
    let mut meta = SkillMeta::new(&id);
    meta.name = Some(skill_name);
    if !tags.is_empty() {
        notes.push(format!("tags: {}", tags.join(", ")));
        meta.tags = Some(tags);
    }
    let (source, source_note) = skill_source(fs, dir, installed);
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
    let mut new_links = Vec::new();
    for link in &named {
        if !clients.iter().any(|known| known == link.client) {
            clients.push(link.client.to_owned());
        }
        new_links.push(skill_link(cx, link, &id));
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
    if !misnamed.is_empty() {
        review = true;
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
        out.files
            .insert(format!("{SKILLS_DIR}/{id}/{relative}"), bytes.clone());
    }
    out.files.insert(
        meta_path(&id)?,
        TomlDocument::new(meta).to_text()?.into_bytes(),
    );
    out.links.extend(new_links);
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
    out.skill_ids.push(id);
    Ok(())
}

/// `[source]` from the skill folder's git repository (remote `origin` and
/// HEAD, read only), else from `state.json` (`git`, and `github` with a
/// `/tree/<ref>/<path>` URL split into repository, ref and path), else
/// `local`. Other SkillDock sources (`marketplace`, `well-known`) have no
/// update path here and stay local, with a note. A URL that may hold a
/// credential is left out (with a note).
fn skill_source(
    fs: &dyn Fs,
    dir: &Path,
    installed: Option<&InstalledSkill>,
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
    let mut note = None;
    if url.is_none()
        && let Some(installed) = installed
        && !installed.source_url.is_empty()
    {
        match installed.source_type.as_str() {
            "" | "local" => {}
            "git" | "github" => {
                if let Source::Git {
                    url: repo,
                    git_ref,
                    path,
                } = Source::parse(&installed.source_url, fs)
                {
                    url = Some(repo);
                    source.path = path;
                    source.git_ref = git_ref.filter(|r| r != "HEAD").or_else(|| {
                        (!installed.branch.is_empty()).then(|| installed.branch.clone())
                    });
                } else {
                    note = Some(
                        "SkillDock's git source is not a repository URL; kept as a local skill"
                            .to_owned(),
                    );
                }
            }
            other => {
                note = Some(format!(
                    "SkillDock's {other} source has no update path in Loadout; kept as a local \
                     skill (reinstall it from its repository for updates)"
                ));
            }
        }
    }
    match url {
        Some(url) if classify_import(None, &url).is_some() || !find_in_text(&url).is_empty() => {
            note = Some(
                "the source URL may hold a credential and was left out; reinstall from a clean URL \
                 for updates"
                    .to_owned(),
            );
            source.git_ref = None;
            source.commit = None;
            source.path = None;
        }
        Some(url) => {
            source.kind = SkillSourceKind::Git;
            source.url = Some(url);
        }
        None => {
            source.git_ref = None;
            source.commit = None;
            source.path = None;
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
            "skillLibraryProvider" => match parsed.text(key) {
                // SkillDock computes compatibility as the field or this
                // provider (`state.rs` `load_app_settings`) and omits the
                // field when unset.
                Some("agent-skills")
                    if !parsed.fields.contains_key("agentSkillsCompatibilityEnabled") =>
                {
                    report.items.push(set(
                        key,
                        &mut settings.agents_cli_compat,
                        true,
                        "agents_cli_compat",
                        |v| v.to_string(),
                    ));
                }
                Some("agent-skills") => report.items.push(item(
                    ItemStatus::Skipped,
                    "the library was the agents CLI folder; it is scanned read-only with agents CLI compatibility".to_owned(),
                )),
                _ => report.items.push(item(
                    ItemStatus::Skipped,
                    "SkillDock's own library is migrated from ~/.skilldock/skills".to_owned(),
                )),
            },
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

//! Catalogs (#26): places that list MCP servers and skills, searched and
//! browsed on the user's request, installed through the paths that exist.
//!
//! - Sources live in `store/catalogs.toml` ([`CatalogsFile`]); built-ins
//!   ([`BUILTIN_REGISTRY`], [`BUILTIN_SKILLS`], [`BUILTIN_SKILLS_SH`]) are
//!   always there and can be turned off.
//! - Nothing is contacted unless "Online catalogs" is on
//!   (`state/local-settings.toml`, off until the user turns it on), and
//!   only on a request: search, show, refresh, install. Never in the
//!   background.
//! - MCP registries through [`http::Http`] ([`registry`]); git taps through
//!   the system git (`tap`). Results are cached in
//!   `cache/catalogs/<id>` for [`TTL`]; when a fetch fails the old results
//!   come back labeled stale with their age. Catalogs are searched each on
//!   its own, so one slow or failing catalog never blocks the others.
//! - Installing hands the entry to the M41 review (MCP servers:
//!   [`McpHandoff`]) or the git skill install review ([`skill_request`]),
//!   with `catalog = "<catalog>/<entry>"` recorded as provenance; trust
//!   (M39) and the scan (M40) apply exactly as there.

mod cache;
pub mod http;
pub mod registry;
mod tap;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::adapters::detect::NoRegistry;
use crate::env::Env;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::local_settings::LocalSettings;
use crate::mcp_source::{self, RegistryRequest, ResolveRequest, Resolved};
use crate::model::TomlDocument;
pub use crate::model::catalogs::{
    BUILTIN_IDS, CatalogRecord, CatalogSourceKind, CatalogsFile, is_catalog_id,
    registry_url_problem,
};
use crate::model::server::ServerSource;
use crate::paths::LoadoutHome;
use crate::remote::git::Runner;
use crate::skills::install::{InstallRequest, Source};
use crate::skills::{LinkKind, SkillsEnv};
use crate::sources::SourceRef;
use crate::store::{CommitId, Operation, Store, StoreRead};
pub use http::{Http, HttpLimits, HttpResponse, UreqHttp};

/// `store/catalogs.toml`.
pub const CATALOGS_FILE: &str = "catalogs.toml";
/// The `Loadout-Entity` of catalog commits.
pub const ENTITY: &str = "catalogs";
/// The cache folder, below `<LOADOUT_HOME>/cache`.
pub const CACHE_DIR: &str = "catalogs";
/// How long cached results answer without a request.
pub const TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// The shortest registry search (the official registry refuses shorter
/// ones with HTTP 422).
pub const MIN_QUERY: usize = 3;
/// `catalog:<source>/<id>`.
pub const REF_PREFIX: &str = "catalog:";
/// Wording the official registry's terms ask for; no endorsement implied.
pub const REGISTRY_ATTRIBUTION: &str = "Data from the Official MCP Registry";
pub const BUILTIN_REGISTRY: &str = "mcp-registry";
pub const BUILTIN_SKILLS: &str = "anthropic-skills";
pub const BUILTIN_SKILLS_SH: &str = "skills-sh";
/// The official MCP Registry.
pub const REGISTRY_URL: &str = "https://registry.modelcontextprotocol.io";

/// What a catalog is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogKind {
    McpRegistry,
    GitTap,
    /// A web page Loadout links to (skills.sh); never contacted.
    Link,
}

impl CatalogKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            CatalogKind::McpRegistry => "mcp_registry",
            CatalogKind::GitTap => "git_tap",
            CatalogKind::Link => "link",
        }
    }
}

impl From<CatalogSourceKind> for CatalogKind {
    fn from(kind: CatalogSourceKind) -> Self {
        match kind {
            CatalogSourceKind::McpRegistry => CatalogKind::McpRegistry,
            CatalogSourceKind::GitTap => CatalogKind::GitTap,
        }
    }
}

/// One catalog, built-in or user-added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub id: String,
    pub kind: CatalogKind,
    pub title: String,
    pub url: String,
    /// A git tap's branch, tag or commit.
    pub git_ref: Option<String>,
    pub enabled: bool,
    pub builtin: bool,
    /// Verifiably first-party: a tap of `github.com/anthropics`.
    pub official: bool,
    /// What the catalog's terms ask to show with its data.
    pub attribution: Option<String>,
}

impl Catalog {
    /// Top-level folders left out of a tap's index.
    fn exclude(&self) -> &'static [&'static str] {
        if self.builtin && self.id == BUILTIN_SKILLS {
            &["template", "spec"]
        } else {
            &[]
        }
    }
}

fn builtins() -> [Catalog; 3] {
    [
        Catalog {
            id: BUILTIN_REGISTRY.to_owned(),
            kind: CatalogKind::McpRegistry,
            title: "Official MCP Registry".to_owned(),
            url: REGISTRY_URL.to_owned(),
            git_ref: None,
            enabled: true,
            builtin: true,
            official: false,
            attribution: Some(REGISTRY_ATTRIBUTION.to_owned()),
        },
        Catalog {
            id: BUILTIN_SKILLS.to_owned(),
            kind: CatalogKind::GitTap,
            title: "Anthropic skills".to_owned(),
            url: "https://github.com/anthropics/skills".to_owned(),
            git_ref: None,
            enabled: true,
            builtin: true,
            official: true,
            attribution: None,
        },
        Catalog {
            id: BUILTIN_SKILLS_SH.to_owned(),
            kind: CatalogKind::Link,
            title: "skills.sh".to_owned(),
            url: "https://skills.sh".to_owned(),
            git_ref: None,
            enabled: true,
            builtin: true,
            official: false,
            attribution: None,
        },
    ]
}

// ------------------------------------------------------------------ store

fn load_doc(read: &dyn StoreRead) -> Result<TomlDocument<CatalogsFile>, CoreError> {
    let Some(text) = read.read_file(CATALOGS_FILE)? else {
        return Ok(TomlDocument::new(CatalogsFile::new()));
    };
    let path = Path::new(CATALOGS_FILE);
    let doc = TomlDocument::<CatalogsFile>::parse(&text)
        .map_err(|diagnostic| diagnostic.to_core_error(Some(path)))?;
    if let Some(err) = doc.value().validate().first_error(Some(path)) {
        return Err(err);
    }
    Ok(doc)
}

/// Every catalog: the built-ins first, then the user's by id. `read` is
/// the store (`None`: none yet, only the built-ins).
pub fn list(read: Option<&dyn StoreRead>) -> Result<Vec<Catalog>, CoreError> {
    let file = match read {
        Some(read) => load_doc(read)?.into_value(),
        None => CatalogsFile::new(),
    };
    let mut catalogs: Vec<Catalog> = builtins().into_iter().collect();
    for catalog in &mut catalogs {
        if let Some(record) = file.catalogs.get(&catalog.id) {
            catalog.enabled = record.enabled.unwrap_or(true);
        }
    }
    for (id, record) in &file.catalogs {
        if BUILTIN_IDS.contains(&id.as_str()) {
            continue;
        }
        let (Some(kind), Some(url)) = (record.kind, record.url.clone()) else {
            continue;
        };
        let official = kind == CatalogSourceKind::GitTap && tap::is_anthropic(&url);
        catalogs.push(Catalog {
            id: id.clone(),
            kind: kind.into(),
            title: record.title.clone().unwrap_or_else(|| id.clone()),
            url: url.trim_end_matches('/').to_owned(),
            git_ref: record.git_ref.clone(),
            enabled: record.enabled.unwrap_or(true),
            builtin: false,
            official,
            attribution: None,
        });
    }
    Ok(catalogs)
}

/// The catalog `id`, or `not_found`.
fn find(read: Option<&dyn StoreRead>, id: &str) -> Result<Catalog, CoreError> {
    list(read)?
        .into_iter()
        .find(|catalog| catalog.id == id)
        .ok_or_else(|| CoreError::new(ErrorCode::NotFound, format!("there is no catalog {id:?}")))
}

/// Runs one change of `catalogs.toml`; `None` when nothing changed.
fn change(
    store: &Store<'_>,
    op: &str,
    summary: String,
    edit: impl FnOnce(&mut CatalogsFile) -> Result<bool, CoreError>,
) -> Result<Option<CommitId>, CoreError> {
    let operation = Operation::new(op, summary).entity(ENTITY);
    let committed = store.transaction(operation, |tx| {
        let mut doc = load_doc(tx)?;
        if !edit(doc.value_mut())? {
            return Ok(());
        }
        let path = Path::new(CATALOGS_FILE);
        if let Some(err) = doc.value().validate().first_error(Some(path)) {
            return Err(err);
        }
        let text = doc.to_text()?;
        tx.write(CATALOGS_FILE, text)
    })?;
    Ok(committed.commit)
}

/// A catalog the user adds.
#[derive(Debug, Clone)]
pub struct NewCatalog {
    pub kind: CatalogSourceKind,
    pub url: String,
    /// The id [default: from the URL's host (registry) or repository name
    /// (tap)].
    pub id: Option<String>,
    /// A tap's branch, tag or commit.
    pub git_ref: Option<String>,
    pub title: Option<String>,
}

/// A catalog id from `text`: lower case, other characters `-`.
fn id_from(text: &str) -> String {
    let id: String = text
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let id = id.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let id: String = id.chars().take(64).collect();
    if id.is_empty() {
        "catalog".to_owned()
    } else {
        id
    }
}

/// The id a new catalog gets from its URL.
fn default_id(new: &NewCatalog) -> String {
    match new.kind {
        CatalogSourceKind::McpRegistry => url::Url::parse(&new.url)
            .ok()
            .and_then(|url| url.host_str().map(id_from))
            .unwrap_or_else(|| "registry".to_owned()),
        CatalogSourceKind::GitTap => id_from(
            new.url
                .trim_end_matches(['/', '\\'])
                .rsplit(['/', '\\', ':'])
                .next()
                .unwrap_or("tap")
                .trim_end_matches(".git"),
        ),
    }
}

/// Adds a catalog (one commit): returns its id. A given id that is taken
/// is `already_exists`; one made from the URL gets `-2`, `-3`.
pub fn add(store: &Store<'_>, new: &NewCatalog) -> Result<(String, Option<CommitId>), CoreError> {
    let url = new.url.trim().trim_end_matches('/').to_owned();
    let existing = list(Some(store))?;
    if let Some(same) = existing
        .iter()
        .find(|catalog| catalog.url.eq_ignore_ascii_case(&url))
    {
        return Err(CoreError::new(
            ErrorCode::AlreadyExists,
            format!("this catalog is already there as {:?}", same.id),
        ));
    }
    let taken = |id: &str| existing.iter().any(|catalog| catalog.id == id);
    let id = match &new.id {
        Some(id) => {
            if !is_catalog_id(id) {
                return Err(CoreError::new(
                    ErrorCode::InvalidId,
                    format!(
                        "{id:?} is not a catalog id (1 to 64 characters of a-z, 0-9, '.', '_', \
                         '-', starting with a letter or digit)"
                    ),
                ));
            }
            if taken(id) {
                return Err(CoreError::new(
                    ErrorCode::AlreadyExists,
                    format!("there is already a catalog {id:?}"),
                ));
            }
            id.clone()
        }
        None => {
            let base = default_id(new);
            let mut id = base.clone();
            let mut n = 2;
            while taken(&id) {
                id = format!("{base}-{n}");
                n += 1;
            }
            id
        }
    };
    let record = CatalogRecord {
        kind: Some(new.kind),
        url: Some(url),
        git_ref: new
            .git_ref
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_owned),
        title: new
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned),
        enabled: None,
        extra: toml::Table::new(),
    };
    let commit = change(store, "catalog.add", format!("catalog: add {id}"), |file| {
        file.catalogs.insert(id.clone(), record);
        Ok(true)
    })?;
    Ok((id, commit))
}

/// Removes a user-added catalog (one commit) and drops its cache on this
/// machine. A built-in is turned off instead ([`set_enabled`]).
pub fn remove(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    id: &str,
) -> Result<Option<CommitId>, CoreError> {
    if BUILTIN_IDS.contains(&id) {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{id:?} is a built-in catalog: turn it off instead"),
        ));
    }
    find(Some(store), id)?;
    let commit = change(
        store,
        "catalog.remove",
        format!("catalog: remove {id}"),
        |file| Ok(file.catalogs.remove(id).is_some()),
    )?;
    cache::clear(sx, id)?;
    Ok(commit)
}

/// Turns a catalog on or off (one commit; `None` when it already was).
pub fn set_enabled(
    store: &Store<'_>,
    id: &str,
    enabled: bool,
) -> Result<Option<CommitId>, CoreError> {
    find(Some(store), id)?;
    let verb = if enabled { "enable" } else { "disable" };
    change(
        store,
        &format!("catalog.{verb}"),
        format!("catalog: {verb} {id}"),
        |file| {
            let builtin = BUILTIN_IDS.contains(&id);
            let current = file
                .catalogs
                .get(id)
                .and_then(|record| record.enabled)
                .unwrap_or(true);
            if current == enabled {
                return Ok(false);
            }
            if builtin {
                if enabled {
                    file.catalogs.remove(id);
                } else {
                    file.catalogs
                        .insert(id.to_owned(), CatalogRecord::switch(false));
                }
            } else if let Some(record) = file.catalogs.get_mut(id) {
                record.enabled = (!enabled).then_some(false);
            }
            Ok(true)
        },
    )
}

/// The hosts searching `catalogs` contacts (the enabled ones that are
/// not links; a tap in a local repository contacts no host), for the
/// sentence the app shows before turning catalogs on.
pub fn contacts(catalogs: &[Catalog]) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for catalog in catalogs
        .iter()
        .filter(|catalog| catalog.enabled && catalog.kind != CatalogKind::Link)
    {
        let host = match catalog.kind {
            CatalogKind::GitTap => match SourceRef::from_git_url(&catalog.url) {
                Ok(source) if source.key().starts_with("file:") => continue,
                Ok(source) => crate::sources::without_insecure_prefix(source.key())
                    .split('/')
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                Err(_) => continue,
            },
            _ => http::host_of(&catalog.url),
        };
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    hosts
}

// ----------------------------------------------------------------- setting

/// The "Online catalogs" switch of this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnlineSetting {
    pub on: bool,
    /// The user answered once (on or off): the app does not ask again.
    pub decided: bool,
}

pub fn online(fs: &dyn Fs, home: &LoadoutHome) -> Result<OnlineSetting, CoreError> {
    let settings = LocalSettings::load(fs, &home.state_dir())?;
    Ok(OnlineSetting {
        on: settings.online_catalogs == Some(true),
        decided: settings.online_catalogs.is_some(),
    })
}

pub fn set_online(fs: &dyn Fs, home: &LoadoutHome, on: bool) -> Result<(), CoreError> {
    let state = home.state_dir();
    let mut settings = LocalSettings::load(fs, &state)?;
    settings.online_catalogs = Some(on);
    settings.save(fs, &state)
}

fn off_error() -> CoreError {
    CoreError::new(
        ErrorCode::CatalogsOff,
        "online catalogs are off on this machine, so no catalog was contacted; turn them on in \
         Install > Browse or with `loadout catalog online on`",
    )
}

fn require_online(fs: &dyn Fs, home: &LoadoutHome) -> Result<(), CoreError> {
    if online(fs, home)?.on {
        Ok(())
    } else {
        Err(off_error())
    }
}

// ------------------------------------------------------------------ entries

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Server,
    Skill,
    Plugin,
}

impl EntryKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            EntryKind::Server => "server",
            EntryKind::Skill => "skill",
            EntryKind::Plugin => "plugin",
        }
    }
}

/// Who published an entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Publisher {
    pub name: String,
    /// What the catalog checked about it, when it checks anything.
    pub checked: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryPackage {
    pub registry_type: String,
    pub identifier: String,
    pub version: Option<String>,
    pub transport: Option<String>,
    /// Names exactly what runs (a concrete version, a digest, a hash).
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryRemote {
    pub transport: String,
    /// May hold `{variables}`.
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginSourceKind {
    Relative,
    Github,
    Url,
    GitSubdir,
    Npm,
    Archive,
    Command,
    Unknown,
}

impl PluginSourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            PluginSourceKind::Relative => "relative",
            PluginSourceKind::Github => "github",
            PluginSourceKind::Url => "url",
            PluginSourceKind::GitSubdir => "git_subdir",
            PluginSourceKind::Npm => "npm",
            PluginSourceKind::Archive => "archive",
            PluginSourceKind::Command => "command",
            PluginSourceKind::Unknown => "unknown",
        }
    }
}

/// Where a marketplace plugin's files are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSource {
    pub kind: PluginSourceKind,
    /// The repository to clone (userinfo masked).
    pub url: Option<String>,
    /// The folder in that repository.
    pub path: Option<String>,
    pub git_ref: Option<String>,
    pub sha: Option<String>,
}

/// One entry of a catalog. Text from the catalog (names, descriptions)
/// is untrusted data: show it as plain text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub catalog: String,
    /// The registry name, the skill folder's name, or `plugin/<name>`.
    pub id: String,
    pub kind: EntryKind,
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub updated_at: Option<String>,
    pub deprecated: bool,
    pub status_message: Option<String>,
    pub publisher: Option<Publisher>,
    pub official: bool,
    pub warning: Option<String>,
    pub repository: Option<String>,
    pub website: Option<String>,
    pub license: Option<String>,
    pub transports: Vec<String>,
    pub packages: Vec<EntryPackage>,
    pub remotes: Vec<EntryRemote>,
    pub required_secrets: Vec<String>,
    pub pinned: bool,
    pub git_ref: Option<String>,
    pub commit: Option<String>,
    pub path: Option<String>,
    pub skills: Vec<String>,
    pub servers: Vec<String>,
    pub not_imported: Vec<String>,
    pub source: Option<PluginSource>,
    pub install_mcp: bool,
    pub install_skill: bool,
    pub problem: Option<String>,
    /// A registry entry's `server.json` (what an install drafts from).
    pub server_json: Option<String>,
    /// A marketplace plugin's own `mcpServers` (an object or a path in
    /// the plugin's folder) as JSON: the review drafts these too.
    #[serde(default)]
    pub mcp_servers: Option<String>,
}

impl CatalogEntry {
    /// `catalog:<catalog>/<id>`.
    pub fn reference(&self) -> String {
        format!("{REF_PREFIX}{}/{}", self.catalog, self.id)
    }
}

/// A marketplace plugin name: letters, digits, `.`, `_`, `-`, starting
/// with a letter or digit, no `..`.
pub(crate) fn is_plugin_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !name.contains("..")
}

// ------------------------------------------------------------------ search

/// Where a catalog's results come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResultStatus {
    /// Fetched just now.
    Fresh,
    /// From the cache, younger than [`TTL`].
    Cached,
    /// From an older cache, because the fetch failed (`error`).
    Stale,
    /// Online catalogs are off: the cache only (any age), nothing fetched.
    Off,
    /// The catalog is turned off.
    Disabled,
    /// A registry search needs [`MIN_QUERY`] characters.
    QueryTooShort,
    /// A link-out catalog: open `link`.
    Link,
    /// The fetch failed and nothing is cached.
    Error,
}

impl ResultStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            ResultStatus::Fresh => "fresh",
            ResultStatus::Cached => "cached",
            ResultStatus::Stale => "stale",
            ResultStatus::Off => "off",
            ResultStatus::Disabled => "disabled",
            ResultStatus::QueryTooShort => "query_too_short",
            ResultStatus::Link => "link",
            ResultStatus::Error => "error",
        }
    }
}

/// One catalog's answer.
#[derive(Debug, Clone)]
pub struct Results {
    pub catalog: String,
    pub status: ResultStatus,
    pub fetched_at: Option<SystemTime>,
    pub age: Option<Duration>,
    pub error: Option<CoreError>,
    pub entries: Vec<CatalogEntry>,
    pub next_cursor: Option<String>,
    pub link: Option<String>,
    pub attribution: Option<String>,
}

impl Results {
    fn new(catalog: &Catalog, status: ResultStatus) -> Self {
        Self {
            catalog: catalog.id.clone(),
            status,
            fetched_at: None,
            age: None,
            error: None,
            entries: Vec::new(),
            next_cursor: None,
            link: (catalog.kind == CatalogKind::Link).then(|| catalog.url.clone()),
            attribution: catalog.attribution.clone(),
        }
    }
}

/// What catalog requests run against.
pub struct CatalogEnv<'a> {
    pub sx: &'a SkillsEnv<'a>,
    pub http: &'a dyn Http,
}

static NO_REGISTRY: NoRegistry = NoRegistry;

/// The parts of a [`SkillsEnv`] a catalog request uses, shareable across
/// threads (a catalog never probes for installed programs).
#[derive(Clone, Copy)]
struct Parts<'a> {
    env: &'a Env,
    fs: &'a dyn Fs,
    home: &'a LoadoutHome,
    runner: &'a dyn Runner,
    link_kind: LinkKind,
    http: &'a dyn Http,
}

impl<'a> Parts<'a> {
    fn of(cx: &CatalogEnv<'a>) -> Self {
        Self {
            env: cx.sx.env,
            fs: cx.sx.fs,
            home: cx.sx.home,
            runner: cx.sx.runner,
            link_kind: cx.sx.link_kind,
            http: cx.http,
        }
    }

    fn sx(&self) -> SkillsEnv<'a> {
        SkillsEnv {
            env: self.env,
            fs: self.fs,
            home: self.home,
            registry: &NO_REGISTRY,
            runner: self.runner,
            link_kind: self.link_kind,
        }
    }
}

/// A value from the cache or a fetch, with how fresh it is.
struct Got<T> {
    data: Option<T>,
    status: ResultStatus,
    fetched_at: Option<SystemTime>,
    age: Option<Duration>,
    error: Option<CoreError>,
}

/// The cache under `name`/`key` when it is younger than [`TTL`] (and
/// `refetch` is false), else `fetch` (cached on success); a failed fetch
/// falls back to any cached value, labeled stale. With online catalogs
/// off, only the cache answers.
fn cached_or<T: Serialize + serde::de::DeserializeOwned>(
    parts: &Parts<'_>,
    online: bool,
    refetch: bool,
    catalog: &str,
    name: &str,
    key: &str,
    fetch: impl FnOnce() -> Result<T, CoreError>,
) -> Got<T> {
    let sx = parts.sx();
    let now = parts.env.now();
    let hit = (!refetch)
        .then(|| cache::read::<T>(&sx, catalog, name, key))
        .flatten();
    let from_hit = |hit: cache::Hit<T>, status: ResultStatus, error: Option<CoreError>| Got {
        age: Some(hit.age(now)),
        fetched_at: Some(hit.fetched_at),
        data: Some(hit.data),
        status,
        error,
    };
    if !online {
        return match hit {
            Some(hit) => from_hit(hit, ResultStatus::Off, None),
            None => Got {
                data: None,
                status: ResultStatus::Off,
                fetched_at: None,
                age: None,
                error: Some(off_error()),
            },
        };
    }
    let hit = match hit {
        Some(hit) if hit.age(now) < TTL => return from_hit(hit, ResultStatus::Cached, None),
        hit => hit,
    };
    match fetch() {
        Ok(data) => {
            let fetched_at = cache::write(&sx, catalog, name, key, &data).ok();
            Got {
                data: Some(data),
                status: ResultStatus::Fresh,
                fetched_at: fetched_at.or(Some(now)),
                age: Some(Duration::ZERO),
                error: None,
            }
        }
        Err(err) => {
            let stale = hit.or_else(|| cache::read::<T>(&sx, catalog, name, key));
            match stale {
                Some(hit) => from_hit(hit, ResultStatus::Stale, Some(err)),
                None => Got {
                    data: None,
                    status: ResultStatus::Error,
                    fetched_at: None,
                    age: None,
                    error: Some(err),
                },
            }
        }
    }
}

/// The cache key of a tap's index.
fn tap_key(catalog: &Catalog) -> String {
    format!(
        "tap\n{}\n{}",
        catalog.url,
        catalog.git_ref.as_deref().unwrap_or_default()
    )
}

/// A tap's index (cache or a fresh clone).
fn tap_index(
    parts: &Parts<'_>,
    online: bool,
    refetch: bool,
    catalog: &Catalog,
) -> Got<tap::TapIndex> {
    cached_or(
        parts,
        online,
        refetch,
        &catalog.id,
        "index",
        &tap_key(catalog),
        || tap::index(&parts.sx(), catalog, catalog.exclude()),
    )
}

fn with<T>(
    mut results: Results,
    got: Got<T>,
    entries: impl FnOnce(T) -> (Vec<CatalogEntry>, Option<String>),
) -> Results {
    results.status = got.status;
    results.fetched_at = got.fetched_at;
    results.age = got.age;
    results.error = got.error;
    if let Some(data) = got.data {
        (results.entries, results.next_cursor) = entries(data);
    }
    results
}

fn search_catalog(
    parts: &Parts<'_>,
    online: bool,
    catalog: &Catalog,
    query: &str,
    cursor: Option<&str>,
) -> Results {
    if !catalog.enabled {
        return Results::new(catalog, ResultStatus::Disabled);
    }
    let needle = query.trim().to_lowercase();
    match catalog.kind {
        CatalogKind::Link => Results::new(catalog, ResultStatus::Link),
        CatalogKind::McpRegistry => {
            if query.trim().chars().count() < MIN_QUERY {
                let mut results = Results::new(catalog, ResultStatus::QueryTooShort);
                results.error = Some(CoreError::new(
                    ErrorCode::InvalidValue,
                    format!(
                        "search {} with at least {MIN_QUERY} characters",
                        catalog.title
                    ),
                ));
                return results;
            }
            let key = format!(
                "search\n{}\n{needle}\n{}",
                catalog.url,
                cursor.unwrap_or_default()
            );
            let name = format!("search-{}", cache::digest(&key));
            let got = cached_or(parts, online, false, &catalog.id, &name, &key, || {
                registry::search(parts.http, catalog, query, cursor)
                    .map(|page| (page.entries, page.next_cursor))
            });
            with(Results::new(catalog, ResultStatus::Fresh), got, |data| data)
        }
        CatalogKind::GitTap => {
            let got = tap_index(parts, online, false, catalog);
            with(Results::new(catalog, ResultStatus::Fresh), got, |index| {
                let entries = index
                    .entries
                    .into_iter()
                    .filter(|entry| registry::matches(entry, &needle))
                    .collect();
                (entries, None)
            })
        }
    }
}

/// Searches every enabled catalog (or only `only`), each on its own
/// thread: one slow or failing catalog never holds the others' results
/// back beyond its own timeouts, and fails alone (its `status`).
pub fn search(
    cx: &CatalogEnv<'_>,
    read: Option<&dyn StoreRead>,
    query: &str,
    only: Option<&str>,
) -> Result<Vec<Results>, CoreError> {
    let catalogs: Vec<Catalog> = match only {
        Some(id) => vec![find(read, id)?],
        None => list(read)?
            .into_iter()
            .filter(|catalog| catalog.enabled)
            .collect(),
    };
    let parts = Parts::of(cx);
    let on = online(parts.fs, parts.home)?.on;
    Ok(std::thread::scope(|scope| {
        let handles: Vec<_> = catalogs
            .iter()
            .map(|catalog| scope.spawn(move || search_catalog(&parts, on, catalog, query, None)))
            .collect();
        handles
            .into_iter()
            .zip(&catalogs)
            .map(|(handle, catalog)| {
                handle.join().unwrap_or_else(|_| {
                    let mut results = Results::new(catalog, ResultStatus::Error);
                    results.error = Some(CoreError::new(
                        ErrorCode::CatalogUnreachable,
                        "the search of this catalog stopped unexpectedly",
                    ));
                    results
                })
            })
            .collect()
    }))
}

/// Searches one catalog; `cursor` is a registry's `next_cursor`.
pub fn search_one(
    cx: &CatalogEnv<'_>,
    read: Option<&dyn StoreRead>,
    catalog: &str,
    query: &str,
    cursor: Option<&str>,
) -> Result<Results, CoreError> {
    let catalog = find(read, catalog)?;
    let parts = Parts::of(cx);
    let on = online(parts.fs, parts.home)?.on;
    Ok(search_catalog(&parts, on, &catalog, query, cursor))
}

/// One entry, with how fresh it is.
#[derive(Debug, Clone)]
pub struct Shown {
    pub entry: CatalogEntry,
    pub status: ResultStatus,
    pub age: Option<Duration>,
    pub error: Option<CoreError>,
}

/// An entry from `got`, or the error that kept it from being read.
fn shown(got: Got<CatalogEntry>) -> Result<Shown, CoreError> {
    match got.data {
        Some(entry) => Ok(Shown {
            entry,
            status: got.status,
            age: got.age,
            error: got.error,
        }),
        None => Err(got.error.unwrap_or_else(off_error)),
    }
}

fn not_in(catalog: &Catalog, id: &str) -> CoreError {
    CoreError::new(
        ErrorCode::NotFound,
        format!("{} lists no entry {id:?}", catalog.title),
    )
}

/// One entry of a catalog: a registry entry at its latest version, or a
/// tap's skill or plugin.
pub fn show(
    cx: &CatalogEnv<'_>,
    read: Option<&dyn StoreRead>,
    catalog: &str,
    entry: &str,
) -> Result<Shown, CoreError> {
    let catalog = find(read, catalog)?;
    let parts = Parts::of(cx);
    let on = online(parts.fs, parts.home)?.on;
    match catalog.kind {
        CatalogKind::Link => Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{} is a link: open {}", catalog.title, catalog.url),
        )),
        CatalogKind::McpRegistry => {
            let key = format!("entry\n{}\n{entry}", catalog.url);
            let name = format!("entry-{}", cache::digest(&key));
            shown(cached_or(
                &parts,
                on,
                false,
                &catalog.id,
                &name,
                &key,
                || registry::entry(parts.http, &catalog, entry, None),
            ))
        }
        CatalogKind::GitTap => {
            let got = tap_index(&parts, on, false, &catalog);
            let found = got
                .data
                .as_ref()
                .map(|index| index.entries.iter().find(|e| e.id == entry).cloned());
            match found {
                Some(Some(found)) => Ok(Shown {
                    entry: found,
                    status: got.status,
                    age: got.age,
                    error: got.error,
                }),
                Some(None) => Err(not_in(&catalog, entry)),
                None => Err(got.error.unwrap_or_else(off_error)),
            }
        }
    }
}

/// An entry as cached (by a search or a show), without any request: what
/// `open_catalog_link` reads.
pub fn cached_entry(
    sx: &SkillsEnv<'_>,
    read: Option<&dyn StoreRead>,
    catalog: &str,
    entry: &str,
) -> Result<CatalogEntry, CoreError> {
    let catalog = find(read, catalog)?;
    match catalog.kind {
        CatalogKind::Link => Err(not_in(&catalog, entry)),
        CatalogKind::GitTap => {
            cache::read::<tap::TapIndex>(sx, &catalog.id, "index", &tap_key(&catalog))
                .and_then(|hit| hit.data.entries.into_iter().find(|e| e.id == entry))
                .ok_or_else(|| not_in(&catalog, entry))
        }
        CatalogKind::McpRegistry => {
            let key = format!("entry\n{}\n{entry}", catalog.url);
            if let Some(hit) = cache::read::<CatalogEntry>(
                sx,
                &catalog.id,
                &format!("entry-{}", cache::digest(&key)),
                &key,
            ) {
                return Ok(hit.data);
            }
            // Any cached search page that holds it.
            let dir = cache::root(sx).join(&catalog.id);
            let pages = sx.fs.read_dir(&dir).unwrap_or_default();
            // Search pages of this catalog's address only: a catalog added
            // again under the same id at another address never answers
            // from the old one's pages.
            let prefix = format!("search\n{}\n", catalog.url);
            pages
                .iter()
                .filter_map(|path| sx.fs.read(path).ok())
                .filter_map(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .filter(|file| {
                    file.get("key")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|key| key.starts_with(&prefix))
                })
                .filter_map(|file| {
                    serde_json::from_value::<(Vec<CatalogEntry>, Option<String>)>(
                        file.get("data")?.clone(),
                    )
                    .ok()
                })
                .flat_map(|(entries, _)| entries)
                .find(|e| e.id == entry && e.catalog == catalog.id)
                .ok_or_else(|| not_in(&catalog, entry))
        }
    }
}

/// Fetches every enabled catalog (or `only`) again: a tap is indexed again
/// (its entries come back), a registry is asked whether it answers and its
/// cached pages are dropped only then. A refresh that fails keeps what is
/// cached (shown stale). Needs online catalogs.
pub fn refresh(
    cx: &CatalogEnv<'_>,
    read: Option<&dyn StoreRead>,
    only: Option<&str>,
) -> Result<Vec<Results>, CoreError> {
    let catalogs: Vec<Catalog> = match only {
        Some(id) => vec![find(read, id)?],
        None => list(read)?
            .into_iter()
            .filter(|catalog| catalog.enabled)
            .collect(),
    };
    let parts = Parts::of(cx);
    require_online(parts.fs, parts.home)?;
    Ok(std::thread::scope(|scope| {
        let handles: Vec<_> = catalogs
            .iter()
            .map(|catalog| {
                scope.spawn(move || {
                    let sx = parts.sx();
                    let base = Results::new(catalog, ResultStatus::Fresh);
                    match catalog.kind {
                        CatalogKind::Link => Results::new(catalog, ResultStatus::Link),
                        CatalogKind::McpRegistry => match registry::ping(parts.http, catalog) {
                            Ok(()) => {
                                let _ = cache::clear(&sx, &catalog.id);
                                Results {
                                    fetched_at: Some(parts.env.now()),
                                    age: Some(Duration::ZERO),
                                    ..base
                                }
                            }
                            Err(err) => Results {
                                status: ResultStatus::Error,
                                error: Some(err),
                                ..base
                            },
                        },
                        CatalogKind::GitTap => {
                            let got = tap_index(&parts, true, true, catalog);
                            with(base, got, |index| (index.entries, None))
                        }
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .zip(&catalogs)
            .map(|(handle, catalog)| {
                handle.join().unwrap_or_else(|_| {
                    let mut results = Results::new(catalog, ResultStatus::Error);
                    results.error = Some(CoreError::new(
                        ErrorCode::CatalogUnreachable,
                        "the refresh of this catalog stopped unexpectedly",
                    ));
                    results
                })
            })
            .collect()
    }))
}

// ----------------------------------------------------------------- install

/// `catalog:<source>/<id>` split into the catalog id and the entry id.
pub fn parse_ref(text: &str) -> Result<(String, String), CoreError> {
    let bad = || {
        CoreError::new(
            ErrorCode::InvalidValue,
            format!("{text:?} is not a catalog entry (catalog:<source>/<id>)"),
        )
    };
    let rest = text.trim().strip_prefix(REF_PREFIX).ok_or_else(bad)?;
    let (catalog, id) = rest.split_once('/').ok_or_else(bad)?;
    if !is_catalog_id(catalog) || id.is_empty() || id.chars().any(char::is_control) {
        return Err(bad());
    }
    Ok((catalog.to_owned(), id.to_owned()))
}

/// The catalog and entry a reference names, for an install: online
/// catalogs on, the catalog on, the entry read (a tap's from its index).
fn install_entry(
    parts: &Parts<'_>,
    read: Option<&dyn StoreRead>,
    text: &str,
) -> Result<(Catalog, CatalogEntry, Option<tap::TapIndex>), CoreError> {
    let (catalog_id, entry_id) = parse_ref(text)?;
    let catalog = find(read, &catalog_id)?;
    if !catalog.enabled {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("the catalog {catalog_id:?} is turned off"),
        ));
    }
    require_online(parts.fs, parts.home)?;
    match catalog.kind {
        CatalogKind::Link => Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!(
                "{} is a link; paste a https://skills.sh/<owner>/<repo>/<skill> address instead",
                catalog.title
            ),
        )),
        CatalogKind::McpRegistry => {
            // Read live at its latest version: what gets installed.
            let entry = registry::entry(parts.http, &catalog, &entry_id, None);
            entry.map(|entry| (catalog, entry, None))
        }
        CatalogKind::GitTap => {
            let got = tap_index(parts, true, false, &catalog);
            let index = got
                .data
                .ok_or_else(|| got.error.unwrap_or_else(off_error))?;
            let entry = index
                .entries
                .iter()
                .find(|e| e.id == entry_id)
                .cloned()
                .ok_or_else(|| not_in(&catalog, &entry_id))?;
            Ok((catalog, entry, Some(index)))
        }
    }
}

/// How an MCP server entry is reviewed.
#[derive(Debug, Clone)]
pub enum McpRequest {
    /// A registry entry's `server.json`.
    Registry(RegistryRequest),
    /// A tap plugin: its repository and folder (its `.mcp.json`).
    Git(ResolveRequest),
}

/// An MCP install from a catalog: the entry (for the review to show) and
/// what the M41 review resolves.
#[derive(Debug, Clone)]
pub struct McpHandoff {
    pub entry: CatalogEntry,
    pub request: McpRequest,
}

impl McpHandoff {
    /// The source the trust gate asks about before anything is fetched: a
    /// tap plugin's repository. `None` for a registry entry: each draft is
    /// gated on what it runs ([`registry::trust_keys`],
    /// [`mcp_source::Draft::trust_source`]), asked after the review.
    pub fn trust_source(&self) -> Result<Option<SourceRef>, CoreError> {
        match &self.request {
            McpRequest::Registry(_) => Ok(None),
            McpRequest::Git(request) => match &request.source {
                Source::Git { url, .. } => SourceRef::from_git_url(url).map(Some),
                Source::Local(folder) => SourceRef::from_local_path(folder).map(Some),
            },
        }
    }

    /// The M41 review of the entry.
    pub fn resolve(
        &self,
        store: Option<&Store<'_>>,
        sx: &SkillsEnv<'_>,
    ) -> Result<Resolved, CoreError> {
        match &self.request {
            McpRequest::Registry(request) => mcp_source::resolve_registry(store, sx, request),
            McpRequest::Git(request) => mcp_source::resolve(store, sx, request),
        }
    }
}

/// The git URL a tap plugin is cloned from: the tap itself (relative
/// plugins), or its source.
fn plugin_url(catalog: &Catalog, source: &PluginSource) -> Result<String, CoreError> {
    match source.kind {
        PluginSourceKind::Relative => Ok(catalog.url.clone()),
        _ => source.url.clone().ok_or_else(|| {
            CoreError::new(ErrorCode::InvalidValue, "this plugin names no repository")
        }),
    }
}

/// The MCP install of `catalog:<source>/<id>`: a registry entry at its
/// latest version (or `version`), or a tap plugin at its pin (or
/// `version` as a branch, tag or commit). Needs online catalogs.
pub fn mcp_handoff(
    cx: &CatalogEnv<'_>,
    read: Option<&dyn StoreRead>,
    text: &str,
    version: Option<&str>,
) -> Result<McpHandoff, CoreError> {
    let parts = Parts::of(cx);
    let (catalog_id, entry_id) = parse_ref(text)?;
    let catalog = find(read, &catalog_id)?;
    if catalog.kind == CatalogKind::McpRegistry && version.is_some() {
        // Read the version asked for, not the latest.
        if !catalog.enabled {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!("the catalog {catalog_id:?} is turned off"),
            ));
        }
        require_online(parts.fs, parts.home)?;
        let entry = registry::entry(parts.http, &catalog, &entry_id, version)?;
        return registry_handoff(&catalog, entry, true);
    }
    let (catalog, entry, _) = install_entry(&parts, read, text)?;
    match catalog.kind {
        CatalogKind::McpRegistry => registry_handoff(&catalog, entry, false),
        _ => {
            let source = match (&entry.kind, &entry.source) {
                (EntryKind::Plugin, Some(source)) if entry.install_mcp => source.clone(),
                (EntryKind::Skill, _) => {
                    return Err(CoreError::new(
                        ErrorCode::InvalidValue,
                        format!(
                            "{} is a skill: install it with `loadout skills add {}`",
                            entry.id,
                            entry.reference()
                        ),
                    ));
                }
                _ => {
                    return Err(CoreError::new(
                        ErrorCode::InvalidValue,
                        entry.problem.clone().unwrap_or_else(|| {
                            format!("{} holds no MCP server Loadout imports", entry.id)
                        }),
                    ));
                }
            };
            let mut request = ResolveRequest::new(Source::Git {
                url: plugin_url(&catalog, &source)?,
                git_ref: None,
                path: None,
            });
            request.git_ref = version
                .map(str::to_owned)
                .or_else(|| source.sha.clone())
                .or_else(|| source.git_ref.clone());
            request.path.clone_from(&source.path);
            request.catalog = Some(format!("{}/{}", catalog.id, entry.id));
            request.mcp_servers.clone_from(&entry.mcp_servers);
            Ok(McpHandoff {
                entry,
                request: McpRequest::Git(request),
            })
        }
    }
}

fn registry_handoff(
    catalog: &Catalog,
    entry: CatalogEntry,
    pinned: bool,
) -> Result<McpHandoff, CoreError> {
    if !entry.install_mcp {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            entry
                .problem
                .clone()
                .unwrap_or_else(|| "this entry cannot be installed".to_owned()),
        ));
    }
    let server_json = entry.server_json.clone().ok_or_else(|| {
        CoreError::new(ErrorCode::InvalidValue, "the entry carries no server.json")
    })?;
    let version = entry
        .version
        .clone()
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidValue, "the entry names no version"))?;
    Ok(McpHandoff {
        request: McpRequest::Registry(RegistryRequest {
            catalog: format!("{}/{}", catalog.id, entry.id),
            registry: catalog.url.clone(),
            name: entry.id.clone(),
            version,
            pinned,
            server_json,
        }),
        entry,
    })
}

/// The git skill install of `catalog:<source>/<id>`: a tap skill (at its
/// folder), or a tap plugin's one skill (several need `path`). `git_ref`
/// and `path` override the entry's. Needs online catalogs.
pub fn skill_request(
    cx: &CatalogEnv<'_>,
    read: Option<&dyn StoreRead>,
    text: &str,
    git_ref: Option<&str>,
    path: Option<&str>,
) -> Result<(InstallRequest, CatalogEntry), CoreError> {
    let parts = Parts::of(cx);
    let (catalog, entry, index) = install_entry(&parts, read, text)?;
    let catalog_ref = Some(format!("{}/{}", catalog.id, entry.id));
    let pin = |entry: &CatalogEntry| -> Option<String> {
        if entry.pinned {
            entry.commit.clone()
        } else {
            entry.git_ref.clone()
        }
    };
    let (url, entry_ref, entry_path) = match entry.kind {
        EntryKind::Server => {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!(
                    "{} is an MCP server: add it with `loadout server add --from {}`",
                    entry.id,
                    entry.reference()
                ),
            ));
        }
        EntryKind::Skill => (catalog.url.clone(), pin(&entry), entry.path.clone()),
        EntryKind::Plugin => {
            let source = entry
                .source
                .clone()
                .filter(|_| entry.install_skill)
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::InvalidValue,
                        entry.problem.clone().unwrap_or_else(|| {
                            format!("{} holds no skill Loadout installs", entry.id)
                        }),
                    )
                })?;
            let folder = match (source.kind, entry.skills.as_slice()) {
                (PluginSourceKind::Relative, _) if path.is_some() => None,
                (PluginSourceKind::Relative, [one]) => index
                    .as_ref()
                    .and_then(|index| index.entries.iter().find(|e| &e.id == one))
                    .and_then(|skill| skill.path.clone()),
                (PluginSourceKind::Relative, many) => {
                    return Err(CoreError::new(
                        ErrorCode::InvalidValue,
                        format!(
                            "{} holds {} skills; install one of them: {}",
                            entry.id,
                            many.len(),
                            many.iter()
                                .map(|id| format!("{REF_PREFIX}{}/{id}", catalog.id))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                }
                _ => source.path.clone(),
            };
            (
                plugin_url(&catalog, &source)?,
                source.sha.clone().or_else(|| source.git_ref.clone()),
                folder,
            )
        }
    };
    let mut request = InstallRequest::new(Source::Git {
        url,
        git_ref: None,
        path: None,
    });
    request.git_ref = git_ref.map(str::to_owned).or(entry_ref);
    request.path = path.map(str::to_owned).or(entry_path);
    request.catalog = catalog_ref;
    Ok((request, entry))
}

/// The git skill install a skills.sh address stands for (no request to
/// skills.sh): `https://skills.sh/<owner>/<repo>[/<skill>]` is
/// `https://github.com/<owner>/<repo>` with `<skill>` picked. `None` for
/// any other text; an error for skills.sh addresses that name no GitHub
/// repository.
pub fn skills_sh_request(text: &str) -> Option<Result<InstallRequest, CoreError>> {
    let url = url::Url::parse(text.trim()).ok()?;
    if !matches!(url.host_str(), Some("skills.sh" | "www.skills.sh")) {
        return None;
    }
    let parts: Vec<&str> = url
        .path_segments()
        .map(|segments| segments.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let valid = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    Some(match parts.as_slice() {
        ["site", domain, ..] => Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!(
                "skills.sh lists this skill from {domain}, not from a GitHub repository; Loadout \
                 installs skills.sh entries from GitHub repositories only"
            ),
        )),
        [owner, repo, rest @ ..]
            if valid(owner) && valid(repo) && rest.iter().all(|p| valid(p)) =>
        {
            let mut request = InstallRequest::new(Source::Git {
                url: format!("https://github.com/{owner}/{repo}"),
                git_ref: None,
                path: None,
            });
            request.skill = rest.first().map(|skill| (*skill).to_owned());
            Ok(request)
        }
        _ => Err(CoreError::new(
            ErrorCode::InvalidValue,
            "this skills.sh address names no skill (https://skills.sh/<owner>/<repo>/<skill>)",
        )),
    })
}

/// The latest version of a registry server (`server update`): the entry
/// its `meta.source` names, read again from its registry with the real
/// HTTP client. Needs online catalogs, and the catalog turned on at the
/// same address: nothing is contacted otherwise.
pub(crate) fn registry_latest(
    read: &dyn StoreRead,
    sx: &SkillsEnv<'_>,
    source: &ServerSource,
) -> Result<RegistryRequest, CoreError> {
    require_online(sx.fs, sx.home)?;
    let missing = |what: &str| {
        CoreError::new(
            ErrorCode::MissingField,
            format!("the registry source has no {what}"),
        )
    };
    let reference = source
        .catalog
        .as_deref()
        .ok_or_else(|| missing("catalog"))?;
    let (catalog_id, name) = reference
        .split_once('/')
        .ok_or_else(|| missing("catalog entry"))?;
    let url = source.url.clone().ok_or_else(|| missing("url"))?;
    if let Some(problem) = registry_url_problem(&url) {
        return Err(CoreError::new(ErrorCode::InvalidValue, problem));
    }
    let catalog = find(Some(read), catalog_id)
        .ok()
        .filter(|catalog| catalog.kind == CatalogKind::McpRegistry && catalog.url == url)
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::NotFound,
                format!(
                    "the catalog {catalog_id:?} is no longer in your catalogs at this address: \
                     add it again to check for updates"
                ),
            )
        })?;
    if !catalog.enabled {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("the catalog {catalog_id:?} is turned off: turn it on to check for updates"),
        ));
    }
    let http = UreqHttp::new(sx.env, HttpLimits::default());
    let entry = registry::entry(&http, &catalog, name, None)?;
    match registry_handoff(&catalog, entry, false)?.request {
        McpRequest::Registry(request) => Ok(request),
        McpRequest::Git(_) => unreachable!("a registry entry"),
    }
}

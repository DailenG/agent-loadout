//! Catalogs for Install > Browse (#26): the catalog sources and the
//! "Online catalogs" switch, a search per catalog (so one slow catalog
//! never holds the others back), an entry's details, and the handoff of an
//! entry to the existing reviews: MCP servers to the M41 review
//! (`save_mcp_source_drafts`), skills to the git skill install review
//! (`install_reviewed_skill`). Nothing is contacted while the switch is
//! off. No absolute path, secret value or credential crosses IPC: URLs are
//! masked; catalog text is passed on as data.

use std::ffi::OsStr;
use std::time::{Duration, SystemTime};

use agent_loadout_core::catalog::{
    self, Catalog, CatalogEntry as CoreEntry, CatalogEnv, CatalogKind as CoreCatalogKind,
    CatalogSourceKind, EntryKind, HttpLimits, NewCatalog, PluginSourceKind, ResultStatus, Results,
    UreqHttp,
};
use agent_loadout_core::remote::git::{IDLE_TIMEOUT, SystemRunner};
use agent_loadout_core::skills::{LinkKind, SkillsEnv};
use agent_loadout_core::store::{Store, StoreRead, utc_datetime};
use agent_loadout_core::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use specta::Type;

use super::mcp_source::{self, McpSourceReview};
use super::skills::{self, SkillInstallReview};
use super::{AppState, CmdResult, Core, IpcError, NO_REGISTRY, blocking, scrub, scrubbed};

static RUNNER: SystemRunner = SystemRunner {
    idle_timeout: IDLE_TIMEOUT,
};

fn sx(core: &Core) -> SkillsEnv<'_> {
    SkillsEnv {
        env: &core.env,
        fs: core.fs.as_ref(),
        home: &core.home,
        registry: &NO_REGISTRY,
        runner: &RUNNER,
        link_kind: LinkKind::Junction,
    }
}

fn ipc(core: &Core, err: &CoreError) -> IpcError {
    scrubbed(&core.env, err, None)
}

fn masked(core: &Core, url: &str) -> String {
    scrub(&core.env, &super::remotes::mask_url(url), None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CatalogKind {
    McpRegistry,
    GitTap,
    Link,
}

impl From<CoreCatalogKind> for CatalogKind {
    fn from(kind: CoreCatalogKind) -> Self {
        match kind {
            CoreCatalogKind::McpRegistry => Self::McpRegistry,
            CoreCatalogKind::GitTap => Self::GitTap,
            CoreCatalogKind::Link => Self::Link,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogInfo {
    pub id: String,
    pub kind: CatalogKind,
    pub title: String,
    pub url: String,
    pub git_ref: Option<String>,
    pub enabled: bool,
    pub builtin: bool,
    pub official: bool,
    pub attribution: Option<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogsOverview {
    /// The "Online catalogs" switch of this machine.
    pub online: bool,
    /// The user answered once (on or off): Browse does not ask again.
    pub decided: bool,
    /// Hosts the enabled catalogs contact (the sentence before turning
    /// catalogs on).
    pub contacts: Vec<String>,
    pub catalogs: Vec<CatalogInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CatalogEntryKind {
    Server,
    Skill,
    Plugin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CatalogResultStatus {
    Fresh,
    Cached,
    Stale,
    Off,
    Disabled,
    QueryTooShort,
    Link,
    Error,
}

impl From<ResultStatus> for CatalogResultStatus {
    fn from(status: ResultStatus) -> Self {
        match status {
            ResultStatus::Fresh => Self::Fresh,
            ResultStatus::Cached => Self::Cached,
            ResultStatus::Stale => Self::Stale,
            ResultStatus::Off => Self::Off,
            ResultStatus::Disabled => Self::Disabled,
            ResultStatus::QueryTooShort => Self::QueryTooShort,
            ResultStatus::Link => Self::Link,
            ResultStatus::Error => Self::Error,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPublisher {
    pub name: String,
    /// What the catalog checked ("namespace ownership checked by the
    /// registry"); null when it checks nothing.
    pub checked: Option<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPackage {
    pub registry_type: String,
    pub identifier: String,
    pub version: Option<String>,
    pub transport: Option<String>,
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogRemote {
    pub transport: String,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CatalogPluginSourceKind {
    Relative,
    Github,
    Url,
    GitSubdir,
    Npm,
    Archive,
    Command,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPluginSource {
    pub kind: CatalogPluginSourceKind,
    pub url: Option<String>,
    pub path: Option<String>,
    pub git_ref: Option<String>,
    pub sha: Option<String>,
}

/// One catalog entry. Text from the catalog is untrusted: render it as
/// plain text; icons are never sent.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub catalog: String,
    pub id: String,
    /// `catalog:<catalog>/<id>`.
    #[serde(rename = "ref")]
    pub reference: String,
    pub kind: CatalogEntryKind,
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub updated_at: Option<String>,
    pub deprecated: bool,
    pub status_message: Option<String>,
    pub publisher: Option<CatalogPublisher>,
    pub official: bool,
    pub warning: Option<String>,
    pub repository: Option<String>,
    pub website: Option<String>,
    pub license: Option<String>,
    pub transports: Vec<String>,
    pub packages: Vec<CatalogPackage>,
    pub remotes: Vec<CatalogRemote>,
    pub required_secrets: Vec<String>,
    pub pinned: bool,
    pub git_ref: Option<String>,
    pub commit: Option<String>,
    pub path: Option<String>,
    pub skills: Vec<String>,
    pub servers: Vec<String>,
    pub not_imported: Vec<String>,
    pub source: Option<CatalogPluginSource>,
    pub install_mcp: bool,
    pub install_skill: bool,
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogResults {
    pub catalog: String,
    pub status: CatalogResultStatus,
    pub age_secs: Option<u32>,
    /// RFC 3339 (UTC).
    pub fetched_at: Option<String>,
    pub error: Option<IpcError>,
    pub entries: Vec<CatalogEntry>,
    pub next_cursor: Option<String>,
    pub link: Option<String>,
    pub attribution: Option<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntryDetail {
    pub entry: CatalogEntry,
    pub status: CatalogResultStatus,
    pub age_secs: Option<u32>,
    pub error: Option<IpcError>,
}

/// What `add_catalog` adds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum NewCatalogKind {
    McpRegistry,
    GitTap,
}

/// Which link `open_catalog_link` opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum CatalogLink {
    Repository,
    Website,
    Catalog,
}

fn info(core: &Core, catalog: &Catalog) -> CatalogInfo {
    CatalogInfo {
        id: catalog.id.clone(),
        kind: catalog.kind.into(),
        title: catalog.title.clone(),
        url: masked(core, &catalog.url),
        git_ref: catalog.git_ref.clone(),
        enabled: catalog.enabled,
        builtin: catalog.builtin,
        official: catalog.official,
        attribution: catalog.attribution.clone(),
    }
}

fn entry_info(core: &Core, entry: &CoreEntry) -> CatalogEntry {
    let url = |text: &Option<String>| text.as_deref().map(|url| masked(core, url));
    CatalogEntry {
        catalog: entry.catalog.clone(),
        id: entry.id.clone(),
        reference: entry.reference(),
        kind: match entry.kind {
            EntryKind::Server => CatalogEntryKind::Server,
            EntryKind::Skill => CatalogEntryKind::Skill,
            EntryKind::Plugin => CatalogEntryKind::Plugin,
        },
        name: entry.name.clone(),
        description: entry.description.clone(),
        version: entry.version.clone(),
        updated_at: entry.updated_at.clone(),
        deprecated: entry.deprecated,
        status_message: entry.status_message.clone(),
        publisher: entry.publisher.as_ref().map(|p| CatalogPublisher {
            name: p.name.clone(),
            checked: p.checked.clone(),
        }),
        official: entry.official,
        warning: entry.warning.clone(),
        repository: url(&entry.repository),
        website: url(&entry.website),
        license: entry.license.clone(),
        transports: entry.transports.clone(),
        packages: entry
            .packages
            .iter()
            .map(|p| CatalogPackage {
                registry_type: p.registry_type.clone(),
                identifier: p.identifier.clone(),
                version: p.version.clone(),
                transport: p.transport.clone(),
                pinned: p.pinned,
            })
            .collect(),
        remotes: entry
            .remotes
            .iter()
            .map(|r| CatalogRemote {
                transport: r.transport.clone(),
                url: masked(core, &r.url),
            })
            .collect(),
        required_secrets: entry.required_secrets.clone(),
        pinned: entry.pinned,
        git_ref: entry.git_ref.clone(),
        commit: entry.commit.clone(),
        path: entry.path.clone(),
        skills: entry.skills.clone(),
        servers: entry.servers.clone(),
        not_imported: entry.not_imported.clone(),
        source: entry.source.as_ref().map(|s| CatalogPluginSource {
            kind: match s.kind {
                PluginSourceKind::Relative => CatalogPluginSourceKind::Relative,
                PluginSourceKind::Github => CatalogPluginSourceKind::Github,
                PluginSourceKind::Url => CatalogPluginSourceKind::Url,
                PluginSourceKind::GitSubdir => CatalogPluginSourceKind::GitSubdir,
                PluginSourceKind::Npm => CatalogPluginSourceKind::Npm,
                PluginSourceKind::Archive => CatalogPluginSourceKind::Archive,
                PluginSourceKind::Command => CatalogPluginSourceKind::Command,
                PluginSourceKind::Unknown => CatalogPluginSourceKind::Unknown,
            },
            url: url(&s.url),
            path: s.path.clone(),
            git_ref: s.git_ref.clone(),
            sha: s.sha.clone(),
        }),
        install_mcp: entry.install_mcp,
        install_skill: entry.install_skill,
        problem: entry.problem.clone(),
    }
}

fn age_secs(age: Option<Duration>) -> Option<u32> {
    age.map(|age| u32::try_from(age.as_secs()).unwrap_or(u32::MAX))
}

fn rfc3339(at: Option<SystemTime>) -> Option<String> {
    at.map(|at| utc_datetime(at).to_string())
}

fn results_info(core: &Core, results: &Results) -> CatalogResults {
    CatalogResults {
        catalog: results.catalog.clone(),
        status: results.status.into(),
        age_secs: age_secs(results.age),
        fetched_at: rfc3339(results.fetched_at),
        error: results.error.as_ref().map(|err| ipc(core, err)),
        entries: results
            .entries
            .iter()
            .map(|entry| entry_info(core, entry))
            .collect(),
        next_cursor: results.next_cursor.clone(),
        link: results.link.clone(),
        attribution: results.attribution.clone(),
    }
}

fn read<'a>(store: &'a Option<Store<'_>>) -> Option<&'a dyn StoreRead> {
    store.as_ref().map(|store| store as &dyn StoreRead)
}

/// The catalogs and the switch.
pub fn overview(core: &Core) -> CmdResult<CatalogsOverview> {
    let store = core.open_store()?;
    let catalogs = catalog::list(read(&store)).map_err(|err| ipc(core, &err))?;
    let setting = catalog::online(core.fs.as_ref(), &core.home)?;
    Ok(CatalogsOverview {
        online: setting.on,
        decided: setting.decided,
        contacts: catalog::contacts(&catalogs),
        catalogs: catalogs.iter().map(|c| info(core, c)).collect(),
    })
}

pub fn set_online(core: &Core, on: bool) -> CmdResult<CatalogsOverview> {
    catalog::set_online(core.fs.as_ref(), &core.home, on)?;
    overview(core)
}

pub fn add(
    core: &Core,
    kind: NewCatalogKind,
    url: &str,
    id: Option<String>,
    git_ref: Option<String>,
    title: Option<String>,
) -> CmdResult<CatalogsOverview> {
    let store = core.open_store_for_change()?;
    let new = NewCatalog {
        kind: match kind {
            NewCatalogKind::McpRegistry => CatalogSourceKind::McpRegistry,
            NewCatalogKind::GitTap => CatalogSourceKind::GitTap,
        },
        url: url.to_owned(),
        id: id.filter(|id| !id.trim().is_empty()),
        git_ref,
        title,
    };
    catalog::add(&store, &new).map_err(|err| ipc(core, &err))?;
    overview(core)
}

pub fn remove(core: &Core, id: &str) -> CmdResult<CatalogsOverview> {
    let store = core.open_store_for_change()?;
    catalog::remove(&store, &sx(core), id).map_err(|err| ipc(core, &err))?;
    overview(core)
}

pub fn set_enabled(core: &Core, id: &str, enabled: bool) -> CmdResult<CatalogsOverview> {
    let store = core.open_store_for_change()?;
    catalog::set_enabled(&store, id, enabled).map_err(|err| ipc(core, &err))?;
    overview(core)
}

/// What catalog requests run with.
fn http(core: &Core) -> UreqHttp {
    UreqHttp::new(&core.env, HttpLimits::default())
}

pub fn search(
    core: &Core,
    catalog_id: &str,
    query: &str,
    cursor: Option<&str>,
) -> CmdResult<CatalogResults> {
    let store = core.open_store()?;
    let sx = sx(core);
    let http = http(core);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };
    let results = catalog::search_one(&cx, read(&store), catalog_id, query, cursor)
        .map_err(|err| ipc(core, &err))?;
    Ok(results_info(core, &results))
}

pub fn show(core: &Core, catalog_id: &str, entry: &str) -> CmdResult<CatalogEntryDetail> {
    let store = core.open_store()?;
    let sx = sx(core);
    let http = http(core);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };
    let shown =
        catalog::show(&cx, read(&store), catalog_id, entry).map_err(|err| ipc(core, &err))?;
    Ok(CatalogEntryDetail {
        entry: entry_info(core, &shown.entry),
        status: shown.status.into(),
        age_secs: age_secs(shown.age),
        error: shown.error.as_ref().map(|err| ipc(core, err)),
    })
}

pub fn refresh(core: &Core, catalog_id: &str) -> CmdResult<CatalogResults> {
    let store = core.open_store()?;
    let sx = sx(core);
    let http = http(core);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };
    let results =
        catalog::refresh(&cx, read(&store), Some(catalog_id)).map_err(|err| ipc(core, &err))?;
    let results = results.into_iter().next().ok_or_else(|| IpcError {
        code: ErrorCode::NotFound,
        message: format!("there is no catalog {catalog_id:?}"),
    })?;
    Ok(results_info(core, &results))
}

fn entry_ref(catalog_id: &str, entry: &str) -> String {
    format!("{}{catalog_id}/{entry}", catalog::REF_PREFIX)
}

/// Resolves an MCP entry into the M41 review (nothing saved).
pub fn review_mcp(
    core: &Core,
    catalog_id: &str,
    entry: &str,
    version: Option<String>,
) -> CmdResult<(agent_loadout_core::mcp_source::Resolved, McpSourceReview)> {
    let store = core.open_store()?;
    let sx = sx(core);
    let http = http(core);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };
    let version = version.filter(|v| !v.trim().is_empty());
    let handoff = catalog::mcp_handoff(
        &cx,
        read(&store),
        &entry_ref(catalog_id, entry),
        version.as_deref(),
    )
    .map_err(|err| ipc(core, &err))?;
    let resolved = handoff
        .resolve(store.as_ref(), &sx)
        .map_err(|err| ipc(core, &err))?;
    mcp_source::review_resolved(core, resolved)
}

/// Reads a skill entry into the git skill install review (nothing
/// installed). `skill` picks one skill of a plugin (a skill entry id).
pub fn review_skill(
    core: &Core,
    catalog_id: &str,
    entry: &str,
    skill: Option<String>,
) -> CmdResult<(
    agent_loadout_core::skills::install::Prepared,
    SkillInstallReview,
)> {
    let store = core.open_store()?;
    let sx = sx(core);
    let http = http(core);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };
    // A plugin's skill is its own entry in the tap.
    let target = match skill.filter(|s| !s.trim().is_empty()) {
        Some(skill) => skill,
        None => entry.to_owned(),
    };
    let (request, _) = catalog::skill_request(
        &cx,
        read(&store),
        &entry_ref(catalog_id, &target),
        None,
        None,
    )
    .map_err(|err| ipc(core, &err))?;
    skills::review_request(core, &request)
}

/// The http(s) address `link` names for an entry (from the cache, no
/// request) or the catalog itself.
pub fn link_of(
    core: &Core,
    catalog_id: &str,
    entry: Option<&str>,
    link: CatalogLink,
) -> CmdResult<String> {
    let store = core.open_store()?;
    let url = match (entry, link) {
        (None, CatalogLink::Catalog) => {
            catalog::list(read(&store))
                .map_err(|err| ipc(core, &err))?
                .into_iter()
                .find(|c| c.id == catalog_id)
                .ok_or_else(|| IpcError {
                    code: ErrorCode::NotFound,
                    message: format!("there is no catalog {catalog_id:?}"),
                })?
                .url
        }
        (Some(entry), CatalogLink::Repository | CatalogLink::Website) => {
            let found = catalog::cached_entry(&sx(core), read(&store), catalog_id, entry)
                .map_err(|err| ipc(core, &err))?;
            let url = if link == CatalogLink::Repository {
                found.repository
            } else {
                found.website
            };
            url.ok_or_else(|| IpcError {
                code: ErrorCode::NotFound,
                message: "this entry has no such link".to_owned(),
            })?
        }
        _ => {
            return Err(IpcError {
                code: ErrorCode::InvalidValue,
                message: "an entry has a repository or website link; a catalog has its own"
                    .to_owned(),
            });
        }
    };
    openable(&url).ok_or_else(|| IpcError {
        code: ErrorCode::InvalidValue,
        message: "only https addresses are opened".to_owned(),
    })
}

/// `url` as handed to the OS opener: https with a host and no user name or
/// password, normalized by the URL parser (quotes, spaces and control
/// characters percent-encoded), with commas encoded too (explorer.exe
/// splits its argument at commas). `None` for anything else.
fn openable(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .filter(|u| {
            u.scheme() == "https"
                && u.username().is_empty()
                && u.password().is_none()
                && u.host_str().is_some_and(|host| !host.is_empty())
        })
        .map(|u| u.as_str().replace(',', "%2C"))
}

#[tauri::command]
#[specta::specta]
pub async fn catalogs_overview(state: tauri::State<'_, AppState>) -> CmdResult<CatalogsOverview> {
    blocking(&state, overview).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_online_catalogs(
    state: tauri::State<'_, AppState>,
    on: bool,
) -> CmdResult<CatalogsOverview> {
    blocking(&state, move |core| set_online(core, on)).await
}

#[tauri::command]
#[specta::specta]
pub async fn add_catalog(
    state: tauri::State<'_, AppState>,
    kind: NewCatalogKind,
    url: String,
    id: Option<String>,
    git_ref: Option<String>,
    title: Option<String>,
) -> CmdResult<CatalogsOverview> {
    blocking(&state, move |core| {
        add(core, kind, &url, id, git_ref, title)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn remove_catalog(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CmdResult<CatalogsOverview> {
    blocking(&state, move |core| remove(core, &id)).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_catalog_enabled(
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> CmdResult<CatalogsOverview> {
    blocking(&state, move |core| set_enabled(core, &id, enabled)).await
}

/// Searches one catalog: call once per catalog so each renders when it
/// answers.
#[tauri::command]
#[specta::specta]
pub async fn search_catalog(
    state: tauri::State<'_, AppState>,
    catalog: String,
    query: String,
    cursor: Option<String>,
) -> CmdResult<CatalogResults> {
    blocking(&state, move |core| {
        search(core, &catalog, &query, cursor.as_deref())
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn show_catalog_entry(
    state: tauri::State<'_, AppState>,
    catalog: String,
    entry: String,
) -> CmdResult<CatalogEntryDetail> {
    blocking(&state, move |core| show(core, &catalog, &entry)).await
}

#[tauri::command]
#[specta::specta]
pub async fn refresh_catalog(
    state: tauri::State<'_, AppState>,
    catalog: String,
) -> CmdResult<CatalogResults> {
    blocking(&state, move |core| refresh(core, &catalog)).await
}

/// The M41 review of a catalog's MCP entry; save it with
/// `save_mcp_source_drafts`, drop it with `discard_mcp_source_review`.
#[tauri::command]
#[specta::specta]
pub async fn review_catalog_mcp(
    state: tauri::State<'_, AppState>,
    catalog: String,
    entry: String,
    version: Option<String>,
) -> CmdResult<McpSourceReview> {
    let (resolved, mut review) = blocking(&state, move |core| {
        review_mcp(core, &catalog, &entry, version)
    })
    .await?;
    review.token = mcp_source::remember_resolved(resolved);
    Ok(review)
}

/// The git skill install review of a catalog's skill entry (or one skill
/// of a plugin); install with `install_reviewed_skill`, drop with
/// `discard_skill_review`.
#[tauri::command]
#[specta::specta]
pub async fn review_catalog_skill(
    state: tauri::State<'_, AppState>,
    catalog: String,
    entry: String,
    skill: Option<String>,
) -> CmdResult<SkillInstallReview> {
    let (prepared, review) = blocking(&state, move |core| {
        review_skill(core, &catalog, &entry, skill)
    })
    .await?;
    Ok(skills::remember_review(&state, prepared, review))
}

/// Opens an entry's repository or website, or the catalog's own page, in
/// the browser. The address is looked up here, never taken from the
/// caller; only https addresses open; nothing is fetched.
#[tauri::command]
#[specta::specta]
pub async fn open_catalog_link(
    state: tauri::State<'_, AppState>,
    catalog: String,
    entry: Option<String>,
    link: CatalogLink,
) -> CmdResult<()> {
    blocking(&state, move |core| {
        let url = link_of(core, &catalog, entry.as_deref(), link)?;
        super::tools::os_open(OsStr::new(&url)).map_err(IpcError::from)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::openable;

    #[test]
    fn only_plain_https_addresses_reach_the_opener() {
        assert_eq!(
            openable("https://github.com/acme/weather").as_deref(),
            Some("https://github.com/acme/weather")
        );
        assert_eq!(
            openable("https://acme.example/a,b c\"d").as_deref(),
            Some("https://acme.example/a%2Cb%20c%22d")
        );
        for refused in [
            "http://acme.example/",
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "ms-settings:privacy",
            "\\\\server\\share",
            "https://user:secret@acme.example/",
            "https://token@acme.example/",
            "C:\\Windows\\notepad.exe",
            "-help",
            "",
        ] {
            assert_eq!(openable(refused), None, "{refused}");
        }
    }
}

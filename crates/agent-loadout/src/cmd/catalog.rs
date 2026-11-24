//! `loadout catalog ls|add|rm|enable|disable|online|search|show|refresh`
//! (#26). Catalogs are contacted only when online catalogs are on (`catalog
//! online on`) and only by these commands and the installs that name a
//! catalog entry (`server add --from catalog:<source>/<id>`, `skills add
//! catalog:<source>/<id>`). Catalog text is data from a third party.

use std::time::Duration;

use agent_loadout_core::catalog::{
    self, Catalog, CatalogEntry, CatalogEnv, CatalogSourceKind, HttpLimits, NewCatalog,
    ResultStatus, Results, UreqHttp,
};
use agent_loadout_core::sources::SourceRef;
use agent_loadout_core::store::StoreRead;
use agent_loadout_core::{CoreError, ErrorCode};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Value, json};

use super::{Context, JSON_SCHEMA, error_json, print_json, require_store};

#[derive(Debug, Args)]
pub struct CatalogArgs {
    #[command(subcommand)]
    command: CatalogCommand,
}

#[derive(Debug, Subcommand)]
enum CatalogCommand {
    /// List catalogs (built-in and added) and the online catalogs setting.
    Ls,
    /// Add an MCP registry (any server of the MCP Registry API) or a git
    /// tap (skill folders and/or a Claude Code plugin marketplace).
    Add {
        /// The registry's base URL, or the tap's git URL.
        url: String,
        /// What the URL is [default: tap for git URLs (ssh, file://, .git,
        /// github.com, gitlab.com, bitbucket.org), else registry].
        #[arg(long, value_enum)]
        kind: Option<KindArg>,
        /// Catalog id [default: from the URL].
        #[arg(long)]
        id: Option<String>,
        /// A tap's branch, tag or commit (a commit pins it).
        #[arg(long = "ref", value_name = "REF")]
        git_ref: Option<String>,
        /// Display name.
        #[arg(long)]
        title: Option<String>,
    },
    /// Remove an added catalog (built-ins are turned off instead).
    Rm { id: String },
    /// Turn a catalog on.
    Enable { id: String },
    /// Turn a catalog off: it is not searched or contacted.
    Disable { id: String },
    /// Show or set the online catalogs setting of this machine (off until
    /// turned on: nothing contacts a catalog before that).
    Online {
        #[arg(value_enum)]
        state: Option<OnOff>,
    },
    /// Search every enabled catalog (or one). An MCP registry needs at
    /// least 3 characters; a tap lists everything for an empty query.
    Search {
        query: String,
        /// Search only this catalog.
        #[arg(long, value_name = "ID")]
        catalog: Option<String>,
        /// The next page of a registry's results (the `next_cursor` of
        /// the last search).
        #[arg(long, requires = "catalog")]
        cursor: Option<String>,
    },
    /// Show one entry: <SOURCE>/<ID> (or catalog:<SOURCE>/<ID>).
    Show {
        #[arg(value_name = "SOURCE/ID")]
        entry: String,
    },
    /// Drop the cache and fetch again (every enabled catalog, or one).
    Refresh { id: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum KindArg {
    Registry,
    Tap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OnOff {
    On,
    Off,
}

/// The HTTP client of catalog commands.
pub(super) fn http(ctx: &Context) -> UreqHttp {
    UreqHttp::new(&ctx.env, HttpLimits::default())
}

pub fn run(ctx: &Context, args: &CatalogArgs) -> Result<u8, CoreError> {
    match &args.command {
        CatalogCommand::Ls => list(ctx),
        CatalogCommand::Add {
            url,
            kind,
            id,
            git_ref,
            title,
        } => {
            let kind = match kind.unwrap_or_else(|| guess_kind(url)) {
                KindArg::Registry => CatalogSourceKind::McpRegistry,
                KindArg::Tap => CatalogSourceKind::GitTap,
            };
            let store = ctx.open_store_for_change()?;
            let new = NewCatalog {
                kind,
                url: url.clone(),
                id: id.clone(),
                git_ref: git_ref.clone(),
                title: title.clone(),
            };
            let (id, commit) = catalog::add(&store, &new)?;
            changed(ctx, &format!("Added catalog {id}"), &id, commit.as_ref())
        }
        CatalogCommand::Rm { id } => {
            let store = require_store(ctx, "there is no catalog to remove")?;
            let sx = super::skills::skills_env(ctx);
            let commit = catalog::remove(&store, &sx, id)?;
            changed(ctx, &format!("Removed catalog {id}"), id, commit.as_ref())
        }
        CatalogCommand::Enable { id } | CatalogCommand::Disable { id } => {
            let enabled = matches!(args.command, CatalogCommand::Enable { .. });
            let store = ctx.open_store_for_change()?;
            let commit = catalog::set_enabled(&store, id, enabled)?;
            let verb = if enabled { "Turned on" } else { "Turned off" };
            changed(ctx, &format!("{verb} catalog {id}"), id, commit.as_ref())
        }
        CatalogCommand::Online { state } => online(ctx, *state),
        CatalogCommand::Search {
            query,
            catalog: only,
            cursor,
        } => search(ctx, query, only.as_deref(), cursor.as_deref()),
        CatalogCommand::Show { entry } => show(ctx, entry),
        CatalogCommand::Refresh { id } => refresh(ctx, id.as_deref()),
    }
}

/// `tap` for URLs git reads (ssh, scp-like, `file://`, `.git`, the big
/// git hosts), else `registry`.
fn guess_kind(url: &str) -> KindArg {
    let lower = url.trim().to_ascii_lowercase();
    let git_host = ["github.com", "gitlab.com", "bitbucket.org"]
        .iter()
        .any(|host| {
            lower.starts_with(&format!("https://{host}/"))
                || lower.starts_with(&format!("http://{host}/"))
        });
    let scp = lower
        .split_once(':')
        .is_some_and(|(user_host, rest)| user_host.contains('@') && !rest.starts_with("//"));
    if git_host
        || scp
        || lower.starts_with("ssh://")
        || lower.starts_with("git://")
        || lower.starts_with("file://")
        || lower.trim_end_matches('/').ends_with(".git")
    {
        KindArg::Tap
    } else {
        KindArg::Registry
    }
}

fn changed(
    ctx: &Context,
    text: &str,
    id: &str,
    commit: Option<&agent_loadout_core::store::CommitId>,
) -> Result<u8, CoreError> {
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "catalog": id,
            "commit": commit.map(ToString::to_string),
        }));
    } else {
        match commit {
            Some(commit) => println!("{text} (commit {}).", commit.short()),
            None => println!("Nothing changed."),
        }
    }
    Ok(0)
}

fn catalog_json(catalog: &Catalog) -> Value {
    json!({
        "id": catalog.id,
        "kind": catalog.kind.as_str(),
        "title": catalog.title,
        "url": SourceRef::redact_userinfo(&catalog.url),
        "ref": catalog.git_ref,
        "enabled": catalog.enabled,
        "builtin": catalog.builtin,
        "official": catalog.official,
        "attribution": catalog.attribution,
    })
}

fn list(ctx: &Context) -> Result<u8, CoreError> {
    let store = ctx.open_store()?;
    let catalogs = catalog::list(store.as_ref().map(|s| s as &dyn StoreRead))?;
    let setting = catalog::online(ctx.fs(), &ctx.home)?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "online": setting.on,
            "decided": setting.decided,
            "contacts": catalog::contacts(&catalogs),
            "catalogs": catalogs.iter().map(catalog_json).collect::<Vec<_>>(),
        }));
        return Ok(0);
    }
    println!("Online catalogs: {}", on_text(setting.on));
    for catalog in &catalogs {
        let mut notes = Vec::new();
        notes.push(catalog.kind.as_str().to_owned());
        if catalog.builtin {
            notes.push("built-in".to_owned());
        }
        if catalog.official {
            notes.push("official".to_owned());
        }
        if !catalog.enabled {
            notes.push("off".to_owned());
        }
        if let Some(git_ref) = &catalog.git_ref {
            notes.push(format!("ref {git_ref}"));
        }
        println!(
            "{:<20} {}  {}  ({})",
            catalog.id,
            catalog.title,
            SourceRef::redact_userinfo(&catalog.url),
            notes.join(", ")
        );
    }
    Ok(0)
}

fn on_text(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off (nothing is contacted; turn on with `loadout catalog online on`)"
    }
}

fn online(ctx: &Context, state: Option<OnOff>) -> Result<u8, CoreError> {
    if let Some(state) = state {
        catalog::set_online(ctx.fs(), &ctx.home, state == OnOff::On)?;
    }
    let setting = catalog::online(ctx.fs(), &ctx.home)?;
    let store = ctx.open_store()?;
    let catalogs = catalog::list(store.as_ref().map(|s| s as &dyn StoreRead))?;
    let contacts = catalog::contacts(&catalogs);
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "online": setting.on,
            "decided": setting.decided,
            "contacts": contacts,
        }));
    } else {
        println!("Online catalogs: {}", on_text(setting.on));
        if setting.on {
            println!(
                "Searches, refreshes and catalog installs contact: {}.",
                contacts.join(", ")
            );
        }
    }
    Ok(0)
}

fn age_secs(age: Option<Duration>) -> Option<u64> {
    age.map(|age| age.as_secs())
}

fn age_text(age: Duration) -> String {
    let secs = age.as_secs();
    match secs {
        0..=59 => "just now".to_owned(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// An entry for `--json` (snake_case of the IPC keys).
pub(super) fn entry_json(entry: &CatalogEntry) -> Value {
    json!({
        "catalog": entry.catalog,
        "id": entry.id,
        "ref": entry.reference(),
        "kind": entry.kind.as_str(),
        "name": entry.name,
        "description": entry.description,
        "version": entry.version,
        "updated_at": entry.updated_at,
        "deprecated": entry.deprecated,
        "status_message": entry.status_message,
        "publisher": entry.publisher.as_ref().map(|p| json!({"name": p.name, "checked": p.checked})),
        "official": entry.official,
        "warning": entry.warning,
        "repository": entry.repository,
        "website": entry.website,
        "license": entry.license,
        "transports": entry.transports,
        "packages": entry.packages.iter().map(|p| json!({
            "registry_type": p.registry_type,
            "identifier": p.identifier,
            "version": p.version,
            "transport": p.transport,
            "pinned": p.pinned,
        })).collect::<Vec<_>>(),
        "remotes": entry.remotes.iter().map(|r| json!({"transport": r.transport, "url": r.url})).collect::<Vec<_>>(),
        "required_secrets": entry.required_secrets,
        "pinned": entry.pinned,
        "git_ref": entry.git_ref,
        "commit": entry.commit,
        "path": entry.path,
        "skills": entry.skills,
        "servers": entry.servers,
        "not_imported": entry.not_imported,
        "source": entry.source.as_ref().map(|s| json!({
            "kind": s.kind.as_str(),
            "url": s.url,
            "path": s.path,
            "git_ref": s.git_ref,
            "sha": s.sha,
        })),
        "install_mcp": entry.install_mcp,
        "install_skill": entry.install_skill,
        "problem": entry.problem,
    })
}

fn results_json(results: &Results) -> Value {
    json!({
        "catalog": results.catalog,
        "status": results.status.as_str(),
        "fetched_at": results.fetched_at.map(|at| agent_loadout_core::store::utc_datetime(at).to_string()),
        "age_secs": age_secs(results.age),
        "error": results.error.as_ref().map(error_json),
        "next_cursor": results.next_cursor,
        "link": results.link,
        "attribution": results.attribution,
        "entries": results.entries.iter().map(entry_json).collect::<Vec<_>>(),
    })
}

/// One line of text from a catalog: control characters dropped, at most
/// `max` characters.
pub(super) fn plain(text: &str, max: usize) -> String {
    let clean: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() > max {
        let cut: String = clean.chars().take(max.saturating_sub(3)).collect();
        format!("{cut}...")
    } else {
        clean
    }
}

fn print_results(results: &Results) {
    let status = match (results.status, results.age) {
        (ResultStatus::Cached, Some(age)) => format!("cached, {}", age_text(age)),
        (ResultStatus::Stale, Some(age)) => {
            format!("offline: results from {}", age_text(age))
        }
        (ResultStatus::Off, Some(age)) => format!("online catalogs off: cached {}", age_text(age)),
        (status, _) => status.as_str().replace('_', " "),
    };
    println!("== {} ({status})", results.catalog);
    if let Some(attribution) = &results.attribution {
        println!("   {attribution}");
    }
    if let Some(link) = &results.link {
        println!("   Open {link} in a browser; paste a skill's address into `loadout skills add`.");
    }
    if let Some(err) = &results.error {
        println!("   [{}] {}", err.code(), err.message());
    }
    for entry in &results.entries {
        let version = entry
            .version
            .as_deref()
            .map(|v| format!(" {}", plain(v, 24)))
            .unwrap_or_default();
        let mut marks = Vec::new();
        if entry.official {
            marks.push("official");
        }
        if entry.deprecated {
            marks.push("deprecated");
        }
        if entry.problem.is_some() {
            marks.push("not installable");
        }
        let marks = if marks.is_empty() {
            String::new()
        } else {
            format!(" [{}]", marks.join(", "))
        };
        println!(
            "  {}  {}{version}{marks}",
            plain(&entry.reference(), 120),
            plain(&entry.name, 60)
        );
        if let Some(description) = &entry.description {
            println!("      {}", plain(description, 100));
        }
    }
    if let Some(cursor) = &results.next_cursor {
        println!(
            "   More: --catalog {} --cursor {}",
            results.catalog,
            plain(cursor, 200)
        );
    }
}

fn search(
    ctx: &Context,
    query: &str,
    only: Option<&str>,
    cursor: Option<&str>,
) -> Result<u8, CoreError> {
    let store = ctx.open_store()?;
    let read = store.as_ref().map(|s| s as &dyn StoreRead);
    let sx = super::skills::skills_env(ctx);
    let http = http(ctx);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };
    let results = match (only, cursor) {
        (Some(only), Some(_)) => vec![catalog::search_one(&cx, read, only, query, cursor)?],
        _ => catalog::search(&cx, read, query, only)?,
    };
    let answered = results.iter().any(|results| {
        !results.entries.is_empty()
            || matches!(
                results.status,
                ResultStatus::Fresh
                    | ResultStatus::Cached
                    | ResultStatus::Stale
                    | ResultStatus::Link
                    | ResultStatus::QueryTooShort
                    | ResultStatus::Disabled
            )
    });
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "query": query,
            "results": results.iter().map(results_json).collect::<Vec<_>>(),
        }));
    } else {
        for results in &results {
            print_results(results);
        }
    }
    Ok(if answered { 0 } else { 1 })
}

/// `<source>/<id>` or `catalog:<source>/<id>`.
fn reference(text: &str) -> String {
    if text.starts_with(catalog::REF_PREFIX) {
        text.to_owned()
    } else {
        format!("{}{text}", catalog::REF_PREFIX)
    }
}

fn show(ctx: &Context, text: &str) -> Result<u8, CoreError> {
    let (source, id) = catalog::parse_ref(&reference(text))?;
    let store = ctx.open_store()?;
    let read = store.as_ref().map(|s| s as &dyn StoreRead);
    let sx = super::skills::skills_env(ctx);
    let http = http(ctx);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };
    let shown = catalog::show(&cx, read, &source, &id)?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "status": shown.status.as_str(),
            "age_secs": age_secs(shown.age),
            "error": shown.error.as_ref().map(error_json),
            "entry": entry_json(&shown.entry),
        }));
        return Ok(0);
    }
    print_entry(&shown.entry);
    match (shown.status, shown.age) {
        (ResultStatus::Stale, Some(age)) => println!("(offline: from {})", age_text(age)),
        (ResultStatus::Off, Some(age)) => {
            println!("(online catalogs off: cached {})", age_text(age))
        }
        _ => {}
    }
    if let Some(err) = &shown.error {
        eprintln!("warning[{}]: {}", err.code(), err.message());
    }
    Ok(0)
}

/// An entry for people. Catalog text is third-party data: printed as
/// plain text.
pub(super) fn print_entry(entry: &CatalogEntry) {
    let line = |label: &str, value: &str| println!("{label:<18} {}", plain(value, 200));
    line("Entry:", &entry.reference());
    line("Name:", &entry.name);
    line("Kind:", entry.kind.as_str());
    if let Some(text) = &entry.description {
        line("Description:", text);
    }
    if let Some(publisher) = &entry.publisher {
        match &publisher.checked {
            Some(checked) => line("Publisher:", &format!("{} ({checked})", publisher.name)),
            None => line("Publisher:", &publisher.name),
        }
    }
    if entry.official {
        line("Official:", "yes (a repository of github.com/anthropics)");
    }
    if let Some(version) = &entry.version {
        line("Version:", version);
    }
    if let Some(updated) = &entry.updated_at {
        line("Updated:", updated);
    }
    if let Some(repo) = &entry.repository {
        // A registry's `repository.url` is the publisher's word: the
        // registry does not check it, and trust is decided on what runs.
        let label = if entry.server_json.is_some() {
            "Repository (claimed by the publisher, not verified):"
        } else {
            "Repository:"
        };
        line(label, repo);
    }
    if let Some(site) = &entry.website {
        line("Website:", site);
    }
    if let Some(license) = &entry.license {
        line("License:", license);
    }
    if !entry.transports.is_empty() {
        line("Transports:", &entry.transports.join(", "));
    }
    for package in &entry.packages {
        let version = package.version.as_deref().unwrap_or("no version");
        let pin = if package.pinned { "" } else { " (not pinned)" };
        line(
            "Package:",
            &format!(
                "{} {} {version}{pin}",
                package.registry_type, package.identifier
            ),
        );
    }
    for remote in &entry.remotes {
        line("Remote:", &format!("{} {}", remote.transport, remote.url));
    }
    if !entry.required_secrets.is_empty() {
        line("Needs secrets:", &entry.required_secrets.join(", "));
    }
    if entry.kind != agent_loadout_core::catalog::EntryKind::Server {
        match (&entry.commit, &entry.git_ref, entry.pinned) {
            (Some(commit), _, true) => line("Pinned to:", commit),
            (_, Some(git_ref), false) => line("Follows:", &format!("branch or tag {git_ref}")),
            (Some(commit), None, false) => {
                line("Follows:", &format!("the default branch (now at {commit})"))
            }
            _ => {}
        }
    }
    if let Some(path) = &entry.path {
        line("Folder:", path);
    }
    if let Some(source) = &entry.source {
        let mut text = source.kind.as_str().to_owned();
        if let Some(url) = &source.url {
            text.push_str(&format!(" {url}"));
        }
        if let Some(path) = &source.path {
            text.push_str(&format!(" ({path})"));
        }
        line("Source:", &text);
    }
    if !entry.skills.is_empty() {
        line("Skills:", &entry.skills.join(", "));
    }
    if !entry.servers.is_empty() {
        line("MCP servers:", &entry.servers.join(", "));
    }
    if !entry.not_imported.is_empty() {
        line("Not imported:", &entry.not_imported.join(", "));
    }
    if entry.deprecated {
        line(
            "Deprecated:",
            entry.status_message.as_deref().unwrap_or("yes"),
        );
    }
    if let Some(warning) = &entry.warning {
        line("Warning:", warning);
    }
    match &entry.problem {
        Some(problem) => line("Cannot install:", problem),
        None => {
            if entry.install_mcp {
                line(
                    "Add with:",
                    &format!("loadout server add --from {}", entry.reference()),
                );
            }
            if entry.install_skill {
                line(
                    "Install with:",
                    &format!("loadout skills add {}", entry.reference()),
                );
            }
        }
    }
}

fn refresh(ctx: &Context, only: Option<&str>) -> Result<u8, CoreError> {
    let store = ctx.open_store()?;
    let read = store.as_ref().map(|s| s as &dyn StoreRead);
    let sx = super::skills::skills_env(ctx);
    let http = http(ctx);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };
    let results = catalog::refresh(&cx, read, only)?;
    let failed = results
        .iter()
        .any(|results| results.status == ResultStatus::Error);
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "query": Value::Null,
            "results": results.iter().map(results_json).collect::<Vec<_>>(),
        }));
    } else {
        for results in &results {
            match &results.error {
                Some(err) => println!(
                    "{}: failed\n  [{}] {}",
                    results.catalog,
                    err.code(),
                    err.message()
                ),
                None if results.status == ResultStatus::Link => {
                    println!("{}: a link, nothing to fetch", results.catalog)
                }
                None => println!(
                    "{}: refreshed ({} entries)",
                    results.catalog,
                    results.entries.len()
                ),
            }
        }
    }
    Ok(u8::from(failed))
}

/// A catalog reference given where it cannot be used.
pub(super) fn not_here(what: &str) -> CoreError {
    CoreError::new(
        ErrorCode::InvalidValue,
        format!("{what} does not apply to a catalog entry"),
    )
}

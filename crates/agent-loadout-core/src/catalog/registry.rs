//! MCP Registry client (#26): `GET /v0.1/servers` (search and paging) and
//! `GET /v0.1/servers/<name>/versions/<version>`, with `/v0` for
//! registries that answer 404 on `/v0.1`. Every `server.json` schema from
//! `2025-07-09` on is read (camelCase and the older snake_case keys,
//! wrapped `{server, _meta}` items or bare ones). Deleted entries are
//! dropped; `isLatest` alone is never trusted.

use serde_json::Value;

use super::http::{Http, unreachable};
use super::{Catalog, CatalogEntry, EntryKind, EntryPackage, EntryRemote, Publisher};
use crate::error::{CoreError, ErrorCode};
use crate::mcp_source::custom_registry_url;
use crate::sources::SourceRef;

/// The key of the official registry's metadata in `_meta`.
const OFFICIAL_META: &str = "io.modelcontextprotocol.registry/official";
/// Results per page.
const PAGE: &str = "50";
/// What the built-in registry proves about a publisher.
pub(crate) const NAMESPACE_CHECKED: &str = "namespace ownership checked by the registry";

/// One page of results.
#[derive(Debug, Clone, Default)]
pub(crate) struct Page {
    pub entries: Vec<CatalogEntry>,
    pub next_cursor: Option<String>,
}

/// What a request gave.
enum Answer {
    Json(Value),
    /// A 4xx/5xx the caller may act on (400/422: a parameter refused).
    Status(u16),
}

/// `<base>/<api>/<segments...>?<query>`, each segment percent-encoded (a
/// server name's `/` becomes `%2F`).
fn api_url(
    base: &str,
    api: &str,
    segments: &[&str],
    query: &[(&str, &str)],
) -> Result<String, CoreError> {
    let mut url = url::Url::parse(base).map_err(|_| {
        CoreError::new(
            ErrorCode::InvalidValue,
            format!("{base:?} is not a registry URL"),
        )
    })?;
    {
        let mut path = url.path_segments_mut().map_err(|()| {
            CoreError::new(
                ErrorCode::InvalidValue,
                format!("{base:?} is not a registry URL"),
            )
        })?;
        path.pop_if_empty();
        path.push(api);
        path.extend(segments);
    }
    if !query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            pairs.append_pair(key, value);
        }
    }
    Ok(url.into())
}

/// GET `/v0.1/...`, then `/v0/...` when the first answers 404.
fn get(
    http: &dyn Http,
    base: &str,
    segments: &[&str],
    query: &[(&str, &str)],
) -> Result<Answer, CoreError> {
    let mut last = 404;
    for api in ["v0.1", "v0"] {
        let url = api_url(base, api, segments, query)?;
        let response = http.get(&url)?;
        match response.status {
            200..=299 => {
                let json = serde_json::from_slice(&response.body)
                    .map_err(|_| unreachable(&url, "the answer is not JSON"))?;
                return Ok(Answer::Json(json));
            }
            404 => last = 404,
            429 | 403 => {
                return Err(unreachable(
                    &url,
                    "it is limiting requests from this network; try again later",
                ));
            }
            status => return Ok(Answer::Status(status)),
        }
    }
    Ok(Answer::Status(last))
}

fn status_error(base: &str, status: u16) -> CoreError {
    unreachable(base, format!("it answered HTTP {status}"))
}

/// A string at the first of `keys` in `value`.
fn text<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn flag(value: &Value, keys: &[&str]) -> bool {
    keys.iter()
        .find_map(|key| value.get(key).and_then(Value::as_bool))
        .unwrap_or(false)
}

fn array<'a>(value: &'a Value, keys: &[&str]) -> &'a [Value] {
    keys.iter()
        .find_map(|key| value.get(key).and_then(Value::as_array))
        .map_or(&[], Vec::as_slice)
}

/// A concrete package version: starts with a digit, no range or tag.
fn concrete_version(version: &str) -> bool {
    version.starts_with(|c: char| c.is_ascii_digit())
        && !version.contains(['^', '~', '<', '>', '*', '|', ' ', ','])
        && !version.split('.').any(|part| part == "x" || part == "X")
}

/// Whether a package names exactly what runs: a concrete version, an OCI
/// image by digest, an MCPB file with its hash.
fn package_pinned(registry_type: &str, identifier: &str, package: &Value) -> bool {
    match registry_type {
        "oci" => identifier.contains("@sha256:"),
        "mcpb" => text(package, &["fileSha256", "file_sha256"]).is_some(),
        _ => text(package, &["version"]).is_some_and(concrete_version),
    }
}

/// An input asks the user for a secret, or for a value it cannot do
/// without.
fn needs_value(input: &Value) -> bool {
    flag(input, &["isSecret", "is_secret"])
        || (flag(input, &["isRequired", "is_required"])
            && input.get("default").is_none()
            && input.get("value").is_none())
}

/// The names of the secrets and required values an entry asks for.
fn required_secrets(server: &Value) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut add = |name: Option<&str>| {
        if let Some(name) = name
            && !names.iter().any(|known| known == name)
        {
            names.push(name.to_owned());
        }
    };
    for package in array(server, &["packages"]) {
        for input in array(package, &["environmentVariables", "environment_variables"]) {
            if needs_value(input) {
                add(text(input, &["name"]));
            }
        }
    }
    for remote in array(server, &["remotes"]) {
        for header in array(remote, &["headers"]) {
            if needs_value(header) {
                add(text(header, &["name"]));
            }
        }
        if let Some(variables) = remote.get("variables").and_then(Value::as_object) {
            for (name, input) in variables {
                if needs_value(input) {
                    add(Some(name));
                }
            }
        }
    }
    names
}

/// The registry's own metadata of an item: `item._meta`, or the older
/// `server._meta`.
fn official<'a>(item: &'a Value, server: &'a Value) -> Option<&'a Value> {
    item.get("_meta")
        .and_then(|meta| meta.get(OFFICIAL_META))
        .or_else(|| server.get("_meta").and_then(|meta| meta.get(OFFICIAL_META)))
}

/// One parsed item and whether the registry marks it latest; `None` for a
/// deleted entry or one without a name.
fn parse_item(catalog: &Catalog, item: &Value) -> Option<(CatalogEntry, bool)> {
    let server = item.get("server").filter(|s| s.is_object()).unwrap_or(item);
    let meta = official(item, server);
    let status = meta
        .and_then(|meta| text(meta, &["status"]))
        .or_else(|| text(server, &["status"]))
        .unwrap_or("active");
    if status == "deleted" {
        return None;
    }
    let is_latest = meta.is_some_and(|meta| flag(meta, &["isLatest", "is_latest"]));
    let name = text(server, &["name"])?.to_owned();
    let version = text(server, &["version"])
        .or_else(|| {
            server
                .get("version_detail")
                .and_then(|detail| text(detail, &["version"]))
        })
        .map(str::to_owned);
    let mut packages = Vec::new();
    let mut transports: Vec<String> = Vec::new();
    let mut add_transport = |transport: &str| {
        if !transports.iter().any(|known| known == transport) {
            transports.push(transport.to_owned());
        }
    };
    for package in array(server, &["packages"]) {
        let registry_type = text(package, &["registryType", "registry_type", "registry_name"])
            .unwrap_or("npm")
            .to_owned();
        let Some(identifier) = text(package, &["identifier", "name"]) else {
            continue;
        };
        let transport = package
            .get("transport")
            .and_then(|transport| text(transport, &["type"]))
            .unwrap_or("stdio")
            .to_owned();
        add_transport(&transport);
        packages.push(EntryPackage {
            pinned: package_pinned(&registry_type, identifier, package),
            registry_type,
            identifier: identifier.to_owned(),
            version: text(package, &["version"]).map(str::to_owned),
            transport: Some(transport),
        });
    }
    let mut remotes = Vec::new();
    for remote in array(server, &["remotes"]) {
        let (Some(kind), Some(url)) = (
            text(remote, &["type", "transport_type"]),
            text(remote, &["url"]),
        ) else {
            continue;
        };
        add_transport(kind);
        remotes.push(EntryRemote {
            transport: kind.to_owned(),
            url: url.to_owned(),
        });
    }
    let namespace = name.split_once('/').map(|(ns, _)| ns.to_owned());
    let deprecated = status == "deprecated";
    let installable = !packages.is_empty() || !remotes.is_empty();
    let entry = CatalogEntry {
        catalog: catalog.id.clone(),
        id: name.clone(),
        kind: EntryKind::Server,
        name: text(server, &["title"]).unwrap_or(&name).to_owned(),
        description: text(server, &["description"]).map(str::to_owned),
        version,
        updated_at: meta
            .and_then(|meta| {
                text(
                    meta,
                    &["updatedAt", "updated_at", "publishedAt", "published_at"],
                )
            })
            .map(str::to_owned),
        deprecated,
        status_message: meta
            .and_then(|meta| text(meta, &["statusMessage", "status_message"]))
            .map(str::to_owned),
        publisher: namespace.map(|name| Publisher {
            name,
            checked: catalog.builtin.then(|| NAMESPACE_CHECKED.to_owned()),
        }),
        official: false,
        warning: None,
        repository: server
            .get("repository")
            .and_then(|repo| text(repo, &["url"]))
            .map(str::to_owned),
        website: text(server, &["websiteUrl", "website_url"]).map(str::to_owned),
        license: None,
        pinned: !packages.is_empty() && packages.iter().all(|package| package.pinned),
        transports,
        packages,
        remotes,
        required_secrets: required_secrets(server),
        git_ref: None,
        commit: None,
        path: None,
        skills: Vec::new(),
        servers: Vec::new(),
        not_imported: Vec::new(),
        source: None,
        install_mcp: installable,
        install_skill: false,
        problem: (!installable)
            .then(|| "this entry lists no package or remote Loadout can run".to_owned()),
        server_json: serde_json::to_string(server).ok(),
        mcp_servers: None,
    };
    Some((entry, is_latest))
}

/// Whether `entry` matches `query` (lowercase): name, title or
/// description.
pub(crate) fn matches(entry: &CatalogEntry, query: &str) -> bool {
    query.is_empty()
        || [
            Some(entry.id.as_str()),
            Some(entry.name.as_str()),
            entry.description.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|text| text.to_lowercase().contains(query))
}

/// The entries of a list answer, filtered on `query` and de-duplicated by
/// name (the one marked latest, else the last seen).
fn page_of(catalog: &Catalog, json: &Value, query: &str) -> Page {
    let mut entries: Vec<(CatalogEntry, bool)> = Vec::new();
    for item in array(json, &["servers"]) {
        let Some((entry, latest)) = parse_item(catalog, item) else {
            continue;
        };
        if !matches(&entry, query) {
            continue;
        }
        match entries.iter_mut().find(|(known, _)| known.id == entry.id) {
            Some(slot) if latest || !slot.1 => *slot = (entry, latest),
            Some(_) => {}
            None => entries.push((entry, latest)),
        }
    }
    let next_cursor = json
        .get("metadata")
        .and_then(|meta| text(meta, &["nextCursor", "next_cursor"]))
        .map(str::to_owned);
    Page {
        entries: entries.into_iter().map(|(entry, _)| entry).collect(),
        next_cursor,
    }
}

/// One page of search results for `query` (3 characters or more, checked
/// by the caller). A registry that refuses `search` (400/422) is listed
/// and filtered here instead.
pub(crate) fn search(
    http: &dyn Http,
    catalog: &Catalog,
    query: &str,
    cursor: Option<&str>,
) -> Result<Page, CoreError> {
    let needle = query.trim().to_lowercase();
    let mut params = vec![
        ("search", query.trim()),
        ("version", "latest"),
        ("limit", PAGE),
    ];
    if let Some(cursor) = cursor {
        params.push(("cursor", cursor));
    }
    match get(http, &catalog.url, &["servers"], &params)? {
        Answer::Json(json) => Ok(page_of(catalog, &json, &needle)),
        Answer::Status(400 | 422) => {
            params.retain(|(key, _)| *key == "limit" || *key == "cursor");
            match get(http, &catalog.url, &["servers"], &params)? {
                Answer::Json(json) => Ok(page_of(catalog, &json, &needle)),
                Answer::Status(status) => Err(status_error(&catalog.url, status)),
            }
        }
        Answer::Status(status) => Err(status_error(&catalog.url, status)),
    }
}

/// Whether the registry answers (a one-entry list).
pub(crate) fn ping(http: &dyn Http, catalog: &Catalog) -> Result<(), CoreError> {
    match get(http, &catalog.url, &["servers"], &[("limit", "1")])? {
        Answer::Json(_) => Ok(()),
        Answer::Status(status) => Err(status_error(&catalog.url, status)),
    }
}

/// The entry `name` at `version` (`latest` when `None`). A deleted entry
/// is `not_found`.
pub(crate) fn entry(
    http: &dyn Http,
    catalog: &Catalog,
    name: &str,
    version: Option<&str>,
) -> Result<CatalogEntry, CoreError> {
    let version = version.unwrap_or("latest");
    let missing = || {
        CoreError::new(
            ErrorCode::NotFound,
            format!(
                "{} lists no server {name:?} at version {version}",
                catalog.title
            ),
        )
    };
    match get(
        http,
        &catalog.url,
        &["servers", name, "versions", version],
        &[],
    )? {
        Answer::Json(json) => match parse_item(catalog, &json) {
            Some((entry, _)) if entry.id == name => Ok(entry),
            Some(_) => Err(missing()),
            None if text(&json, &["name"]).is_some() || json.get("server").is_some() => {
                Err(CoreError::new(
                    ErrorCode::NotFound,
                    format!(
                        "{name:?} was removed from {} (status deleted); Loadout does not install it",
                        catalog.title
                    ),
                ))
            }
            None => Err(missing()),
        },
        Answer::Status(404) => Err(missing()),
        Answer::Status(status) => Err(status_error(&catalog.url, status)),
    }
}

fn unkeyed(why: impl Into<String>) -> CoreError {
    CoreError::new(ErrorCode::InvalidValue, why)
}

/// `scheme` and `host[:port]` of a registry other than a package type's
/// default (`registryBaseUrl`), as the `registry` converter passes it.
fn custom_host(package: &Value) -> Result<Option<(&'static str, String)>, CoreError> {
    let Some(url) = custom_registry_url(text(package, &["registryBaseUrl", "registry_base_url"]))
    else {
        return Ok(None);
    };
    let parsed = url::Url::parse(url)
        .map_err(|_| unkeyed(format!("the package registry {url:?} is not a URL")))?;
    let scheme = match parsed.scheme() {
        "https" => "https",
        "http" => "http",
        _ => {
            return Err(unkeyed(format!(
                "the package registry {url:?} is not http(s)"
            )));
        }
    };
    let host = parsed
        .host_str()
        .ok_or_else(|| unkeyed(format!("the package registry {url:?} names no host")))?;
    Ok(Some((
        scheme,
        match parsed.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        },
    )))
}

/// PEP 503: lowercase, runs of `-`, `_`, `.` as one `-`.
fn pypi_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !out.ends_with('-') {
                out.push('-');
            }
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// What a package runs from, keyed like a repository (M39): npm
/// `<registry host>/<name>` (`registry.npmjs.org/%40scope/name`, so its
/// owner scope is the npm scope), `pypi.org/<name>`,
/// `api.nuget.org/<id>`, `crates.io/<name>`, an OCI image
/// `<registry host>/<namespace>/<repository>`, an MCPB bundle the
/// repository of its release URL. A `registryBaseUrl` other than the
/// type's default replaces the host: the code comes from there.
fn package_key(package: &Value) -> Result<SourceRef, CoreError> {
    let kind = text(package, &["registryType", "registry_type", "registry_name"]).unwrap_or("npm");
    let identifier = text(package, &["identifier", "name"])
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| unkeyed("the package has no identifier"))?;
    let custom = custom_host(package)?;
    let (default_host, path) = match kind {
        "npm" => ("registry.npmjs.org", identifier.replace('@', "%40")),
        "pypi" => ("pypi.org", pypi_name(identifier)),
        "nuget" => ("api.nuget.org", identifier.to_ascii_lowercase()),
        "cargo" => ("crates.io", identifier.to_ascii_lowercase()),
        "oci" | "docker" => {
            let image = identifier.split('@').next().unwrap_or(identifier);
            let image = match image.rsplit_once(':') {
                Some((name, tag)) if !tag.contains('/') => name,
                _ => image,
            };
            let first = image.split('/').next().unwrap_or_default();
            if first.contains('.') || first.contains(':') || first == "localhost" {
                // The image names its registry.
                let (host, rest) = image.split_once('/').unwrap_or((image, ""));
                let host = if host == "registry-1.docker.io" {
                    "docker.io"
                } else {
                    host
                };
                return SourceRef::from_key_or_url(&format!("https://{host}/{rest}"));
            }
            let path = if image.contains('/') {
                image.to_owned()
            } else {
                format!("library/{image}")
            };
            ("docker.io", path)
        }
        "mcpb" if identifier.starts_with("https://") => {
            return SourceRef::from_git_url(identifier);
        }
        other => return Err(unkeyed(format!("{other} packages have no trust key"))),
    };
    let (scheme, host) = custom.unwrap_or(("https", default_host.to_owned()));
    SourceRef::from_key_or_url(&format!("{scheme}://{host}/{path}"))
}

/// Each definition's entry name with its trust source, or why it has none.
pub type TrustKeys = Vec<(String, Result<SourceRef, CoreError>)>;

/// The source the trust gate (M39) admits for each definition of an
/// entry's `server.json`, by the `registry` converter's entry name
/// (`packages[0]`, `remotes[1]`): what that definition runs, never the
/// publisher-written `repository.url` (the registry does not check it).
/// A package is keyed by its registry and name ([`package_key`]), a
/// remote by its host ([`SourceRef::whole_host`]). A definition without
/// a usable key carries the error.
pub fn trust_keys(server_json: &str) -> Result<TrustKeys, CoreError> {
    let server: Value = serde_json::from_str(server_json).map_err(|_| {
        CoreError::new(
            ErrorCode::InvalidValue,
            "the entry's server.json is not JSON",
        )
    })?;
    let mut keys = Vec::new();
    for (index, package) in array(&server, &["packages"]).iter().enumerate() {
        keys.push((format!("packages[{index}]"), package_key(package)));
    }
    for (index, remote) in array(&server, &["remotes"]).iter().enumerate() {
        let key = text(remote, &["url"])
            .ok_or_else(|| unkeyed("the remote has no URL"))
            .and_then(SourceRef::whole_host);
        keys.push((format!("remotes[{index}]"), key));
    }
    Ok(keys)
}

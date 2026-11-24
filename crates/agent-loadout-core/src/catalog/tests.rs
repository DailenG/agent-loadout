//! Unit tests of the catalog parsers: registry items from recorded answers
//! (every schema, status and wrapping), the client-side search fallback,
//! trust keys, references, skills.sh addresses and reserved marketplace
//! names. No network: a recorded [`Http`].

use std::path::PathBuf;
use std::sync::Mutex;

use super::http::{Http, HttpResponse};
use super::registry::{self, NAMESPACE_CHECKED};
use super::*;
use crate::model::sources::SourceScope;

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/catalogs/registry")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

/// Answers by URL substring, in order; records every URL asked for.
struct Recorded {
    routes: Vec<(&'static str, u16, Vec<u8>)>,
    asked: Mutex<Vec<String>>,
}

impl Recorded {
    fn new(routes: Vec<(&'static str, u16, Vec<u8>)>) -> Self {
        Self {
            routes,
            asked: Mutex::new(Vec::new()),
        }
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl Http for Recorded {
    fn get(&self, url: &str) -> Result<HttpResponse, CoreError> {
        self.asked.lock().unwrap().push(url.to_owned());
        let (_, status, body) = self
            .routes
            .iter()
            .find(|(part, ..)| url.contains(part))
            .cloned()
            .unwrap_or(("", 404, b"{}".to_vec()));
        Ok(HttpResponse { status, body })
    }
}

fn registry_catalog(builtin: bool) -> Catalog {
    let mut catalog = builtins()[0].clone();
    if !builtin {
        catalog.id = "acme".to_owned();
        catalog.builtin = false;
        catalog.url = "https://registry.acme.example/sub".to_owned();
        catalog.attribution = None;
    }
    catalog
}

#[test]
fn a_search_page_keeps_the_latest_live_entries_of_every_schema() {
    let http = Recorded::new(vec![(
        "/v0.1/servers?",
        200,
        fixture("search-weather.json"),
    )]);
    let page = registry::search(&http, &registry_catalog(true), "weather", None).unwrap();
    let ids: Vec<&str> = page.entries.iter().map(|e| e.id.as_str()).collect();
    // Deleted dropped; the unrelated entry filtered on the client; one row
    // per name (the latest, not the first).
    assert_eq!(
        ids,
        [
            "io.github.acme/weather",
            "io.github.acme/weather-old",
            "com.example/weather-legacy"
        ]
    );
    assert_eq!(
        page.next_cursor.as_deref(),
        Some("io.github.acme/weather-old:0.5.0")
    );
    let asked = http.asked();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].contains("search=weather"), "{asked:?}");
    assert!(asked[0].contains("version=latest"), "{asked:?}");

    let weather = &page.entries[0];
    assert_eq!(weather.version.as_deref(), Some("1.2.0"));
    assert_eq!(weather.name, "Acme Weather");
    assert_eq!(weather.kind, EntryKind::Server);
    assert_eq!(
        weather.reference(),
        "catalog:mcp-registry/io.github.acme/weather"
    );
    assert_eq!(
        weather.publisher,
        Some(Publisher {
            name: "io.github.acme".to_owned(),
            checked: Some(NAMESPACE_CHECKED.to_owned()),
        })
    );
    assert!(!weather.official, "the registry has no official badge");
    assert_eq!(weather.transports, ["stdio", "streamable-http"]);
    assert_eq!(
        weather.required_secrets,
        ["WEATHER_API_KEY", "Authorization", "tenant"]
    );
    assert!(weather.pinned);
    assert!(weather.install_mcp);
    assert_eq!(
        weather.repository.as_deref(),
        Some("https://github.com/acme/weather")
    );
    assert_eq!(weather.updated_at.as_deref(), Some("2026-11-05T23:35:33Z"));
    let json = weather.server_json.as_deref().unwrap();
    assert!(
        json.contains("\"icons\""),
        "the server.json is kept as served"
    );

    let old = &page.entries[1];
    assert!(old.deprecated);
    assert_eq!(
        old.status_message.as_deref(),
        Some("Use io.github.acme/weather instead.")
    );
    assert!(!old.pinned, "an OCI tag without a digest is not pinned");

    let legacy = &page.entries[2];
    assert_eq!(legacy.version.as_deref(), Some("0.9.0"));
    assert_eq!(legacy.packages[0].identifier, "@example/weather");
    assert!(!legacy.packages[0].pinned, "a dist-tag is not a version");
    assert_eq!(legacy.required_secrets, ["LEGACY_TOKEN"]);
}

#[test]
fn a_user_registry_has_no_publisher_check_and_falls_back_to_v0_and_client_filtering() {
    let catalog = registry_catalog(false);
    // `/v0.1` is missing (404), `/v0` refuses `search` (422): listed and
    // filtered here.
    let http = Recorded::new(vec![
        ("/v0.1/", 404, b"{}".to_vec()),
        ("search=", 422, b"{}".to_vec()),
        ("/sub/v0/servers?limit", 200, fixture("search-weather.json")),
    ]);
    let page = registry::search(&http, &catalog, "legacy", None).unwrap();
    let ids: Vec<&str> = page.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["com.example/weather-legacy"]);
    assert!(
        page.entries[0]
            .publisher
            .as_ref()
            .unwrap()
            .checked
            .is_none()
    );
    let asked = http.asked();
    assert!(
        asked.iter().any(|url| url.contains("/sub/v0.1/servers")),
        "{asked:?}"
    );
    assert!(
        asked
            .last()
            .unwrap()
            .starts_with("https://registry.acme.example/sub/v0/servers?limit=50"),
        "{asked:?}"
    );
}

#[test]
fn an_entry_is_read_by_its_encoded_name_and_a_deleted_one_is_refused() {
    let catalog = registry_catalog(true);
    let http = Recorded::new(vec![
        (
            "/v0.1/servers/io.github.acme%2Fweather/versions/latest",
            200,
            fixture("entry-weather-1.2.0.json"),
        ),
        ("weather-gone", 200, fixture("entry-deleted.json")),
    ]);
    let entry = registry::entry(&http, &catalog, "io.github.acme/weather", None).unwrap();
    assert_eq!(entry.version.as_deref(), Some("1.2.0"));
    let err = registry::entry(&http, &catalog, "io.github.acme/weather-gone", None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);
    assert!(err.message().contains("deleted"), "{err}");
    let err = registry::entry(&http, &catalog, "io.github.acme/none", Some("9.9.9")).unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);
}

#[test]
fn rate_limits_and_bad_answers_are_catalog_unreachable() {
    let catalog = registry_catalog(true);
    for (status, body) in [
        (429, b"{}".to_vec()),
        (500, b"{}".to_vec()),
        (200, b"<html>".to_vec()),
    ] {
        let http = Recorded::new(vec![("/v0.1/", status, body)]);
        let err = registry::search(&http, &catalog, "weather", None).unwrap_err();
        assert_eq!(err.code(), ErrorCode::CatalogUnreachable, "{status}: {err}");
    }
}

/// The keys of an entry's definitions, by converter entry name.
fn keys(json: &str) -> Vec<(String, Result<String, ErrorCode>)> {
    registry::trust_keys(json)
        .unwrap()
        .into_iter()
        .map(|(entry, key)| (entry, key.map(|s| s.key().to_owned()).map_err(|e| e.code())))
        .collect()
}

fn key_of(json: &str) -> String {
    keys(json).remove(0).1.unwrap()
}

#[test]
fn trust_keys_name_what_runs_never_the_claimed_repository() {
    // P1: a publisher claims a trusted repository but ships its own package.
    assert_eq!(
        keys(
            r#"{"repository":{"url":"https://github.com/microsoft/playwright-mcp"},
               "packages":[{"registryType":"npm","identifier":"evil-mcp","version":"1.0.0"}]}"#
        ),
        vec![(
            "packages[0]".to_owned(),
            Ok("registry.npmjs.org/evil-mcp".to_owned())
        )]
    );
    // The npm scope is the owner scope.
    let scoped = registry::trust_keys(
        r#"{"packages":[{"registryType":"npm","identifier":"@acme/weather-mcp"}]}"#,
    )
    .unwrap()
    .remove(0)
    .1
    .unwrap();
    assert_eq!(scoped.key(), "registry.npmjs.org/%40acme/weather-mcp");
    assert_eq!(
        scoped.scope_key(SourceScope::Owner).as_deref(),
        Some("registry.npmjs.org/%40acme/*")
    );
    // A custom registry is where the code comes from.
    assert_eq!(
        key_of(
            r#"{"packages":[{"registryType":"npm","identifier":"weather","registryBaseUrl":"https://npm.evil.example/"}]}"#
        ),
        "npm.evil.example/weather"
    );
    // The default registry written out is the default.
    assert_eq!(
        key_of(
            r#"{"packages":[{"registryType":"npm","identifier":"weather","registryBaseUrl":"https://registry.npmjs.org"}]}"#
        ),
        "registry.npmjs.org/weather"
    );
    assert_eq!(
        key_of(r#"{"packages":[{"registryType":"pypi","identifier":"Acme_Weather.MCP"}]}"#),
        "pypi.org/acme-weather-mcp"
    );
    assert_eq!(
        key_of(r#"{"packages":[{"registryType":"nuget","identifier":"Acme.Weather"}]}"#),
        "api.nuget.org/acme.weather"
    );
    assert_eq!(
        key_of(r#"{"packages":[{"registryType":"cargo","identifier":"weather"}]}"#),
        "crates.io/weather"
    );
    assert_eq!(
        key_of(r#"{"packages":[{"registryType":"oci","identifier":"ghcr.io/acme/weather:1.0"}]}"#),
        "ghcr.io/acme/weather"
    );
    assert_eq!(
        key_of(r#"{"packages":[{"registryType":"oci","identifier":"acme/weather@sha256:abc"}]}"#),
        "docker.io/acme/weather"
    );
    assert_eq!(
        key_of(r#"{"packages":[{"registryType":"oci","identifier":"weather"}]}"#),
        "docker.io/library/weather"
    );
    assert_eq!(
        key_of(
            r#"{"packages":[{"registryType":"mcpb","identifier":"https://github.com/Acme/weather/releases/download/v1/w.mcpb"}]}"#
        ),
        "github.com/acme/weather"
    );
}

#[test]
fn every_definition_has_its_own_key_and_remotes_are_keyed_by_host() {
    // Two remotes on two hosts are two keys; a package is a third.
    assert_eq!(
        keys(
            r#"{"repository":{"url":"https://github.com/acme/weather"},
               "packages":[{"registryType":"pypi","identifier":"acme-weather"}],
               "remotes":[{"type":"sse","url":"https://mcp.acme.example/sse"},
                          {"type":"streamable-http","url":"https://mcp.evil.example:8443/mcp"},
                          {"url":"https://{tenant}.acme.example/mcp"}]}"#
        ),
        vec![
            (
                "packages[0]".to_owned(),
                Ok("pypi.org/acme-weather".to_owned())
            ),
            ("remotes[0]".to_owned(), Ok("mcp.acme.example/*".to_owned())),
            (
                "remotes[1]".to_owned(),
                Ok("mcp.evil.example:8443/*".to_owned())
            ),
            ("remotes[2]".to_owned(), Err(ErrorCode::InvalidValue)),
        ]
    );
    let host = SourceRef::whole_host("https://mcp.acme.example/sse").unwrap();
    assert_eq!(host.scopes(), vec![SourceScope::Host]);
    assert_eq!(host.matching_keys(), vec!["mcp.acme.example/*".to_owned()]);
    // An entry without packages or remotes has nothing to key.
    assert!(
        registry::trust_keys(r#"{"name":"x/y"}"#)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        registry::trust_keys("not json").unwrap_err().code(),
        ErrorCode::InvalidValue
    );
}

#[test]
fn references_and_skills_sh_addresses() {
    assert_eq!(
        parse_ref("catalog:mcp-registry/io.github.acme/weather").unwrap(),
        (
            "mcp-registry".to_owned(),
            "io.github.acme/weather".to_owned()
        )
    );
    assert_eq!(
        parse_ref("catalog:anthropic-skills/plugin/document-skills")
            .unwrap()
            .1,
        "plugin/document-skills"
    );
    for bad in [
        "catalog:",
        "catalog:x",
        "catalog:/y",
        "catalog:Bad/y",
        "mcp-registry/x",
    ] {
        assert_eq!(
            parse_ref(bad).unwrap_err().code(),
            ErrorCode::InvalidValue,
            "{bad}"
        );
    }

    let request = skills_sh_request("https://skills.sh/anthropics/skills/pdf")
        .unwrap()
        .unwrap();
    assert_eq!(
        request.source,
        Source::Git {
            url: "https://github.com/anthropics/skills".to_owned(),
            git_ref: None,
            path: None,
        }
    );
    assert_eq!(request.skill.as_deref(), Some("pdf"));
    let err = skills_sh_request("https://skills.sh/site/mintlify.com/docs")
        .unwrap()
        .unwrap_err();
    assert!(err.message().contains("GitHub"), "{err}");
    assert!(skills_sh_request("https://skills.sh/").unwrap().is_err());
    assert!(skills_sh_request("https://github.com/anthropics/skills").is_none());
    assert!(skills_sh_request("not a url").is_none());
}

#[test]
fn reserved_marketplace_names_and_look_alikes() {
    for name in [
        "claude-plugins-official",
        "Claude_Plugins.Official",
        "anthropic agent skills",
        "anthrоpic-plugins", // Cyrillic o
    ] {
        assert!(tap::reserved_like(name), "{name}");
    }
    assert!(!tap::reserved_like("acme-skills"));
    assert!(tap::is_anthropic("https://github.com/anthropics/skills"));
    assert!(tap::is_anthropic(
        "git@github.com:anthropics/claude-code.git"
    ));
    assert!(!tap::is_anthropic(
        "https://github.com/anthropics-fan/skills"
    ));
}

#[test]
fn catalog_ids_and_registry_urls() {
    assert!(is_catalog_id("mcp-registry"));
    assert!(is_catalog_id("registry.acme.example"));
    assert!(!is_catalog_id("a/b"));
    assert!(!is_catalog_id("-x"));
    assert!(!is_catalog_id(""));
    assert!(registry_url_problem("https://registry.acme.example/sub").is_none());
    assert!(registry_url_problem("http://127.0.0.1:8080").is_none());
    assert!(registry_url_problem("http://registry.acme.example").is_some());
    assert!(registry_url_problem("https://user:pw@registry.acme.example").is_some());
    assert!(registry_url_problem("https://registry.acme.example/?q=1").is_some());
    assert!(registry_url_problem("ftp://registry.acme.example").is_some());
}

#[test]
fn contacts_name_the_hosts_of_enabled_catalogs_only() {
    let mut catalogs: Vec<Catalog> = builtins().into_iter().collect();
    assert_eq!(
        contacts(&catalogs),
        ["registry.modelcontextprotocol.io", "github.com"]
    );
    catalogs[0].enabled = false;
    assert_eq!(contacts(&catalogs), ["github.com"]);
}

//! `store/catalogs.toml`: the catalogs the user browses and searches
//! (#26). Built-in catalogs are always there (a table for one may only turn
//! it off); the user adds MCP registries and git taps. Synced like every
//! store file.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;
use crate::model::validate::{Diagnostics, join};
use crate::model::{SCHEMA_VERSION, StoreModel};

/// Ids of the built-in catalogs: reserved, never added or removed.
pub const BUILTIN_IDS: [&str; 3] = ["mcp-registry", "anthropic-skills", "skills-sh"];

/// What a user-added catalog is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CatalogSourceKind {
    /// A server of the MCP Registry API (`/v0.1/servers`).
    McpRegistry,
    /// A git repository of skill folders and/or a Claude Code plugin
    /// marketplace (`.claude-plugin/marketplace.json`).
    GitTap,
}

impl CatalogSourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            CatalogSourceKind::McpRegistry => "mcp_registry",
            CatalogSourceKind::GitTap => "git_tap",
        }
    }
}

/// One catalog: a user-added one (`kind` and `url`), or a built-in's
/// switch (`enabled` only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CatalogRecord {
    /// `mcp_registry` or `git_tap` (user-added catalogs only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<CatalogSourceKind>,
    /// The registry's base URL (https; http only for localhost), or the
    /// tap's git URL (user-added catalogs only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Branch, tag or commit of a git tap; a commit pins it.
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Searched and browsed; default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

impl CatalogRecord {
    /// A built-in's switch.
    pub fn switch(enabled: bool) -> Self {
        Self {
            kind: None,
            url: None,
            git_ref: None,
            title: None,
            enabled: Some(enabled),
            extra: toml::Table::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "Loadout catalogs (store/catalogs.toml)")]
pub struct CatalogsFile {
    /// Schema version; always 1.
    #[schemars(extend("const" = 1))]
    pub schema: i64,
    /// Catalogs by id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub catalogs: BTreeMap<String, CatalogRecord>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

impl StoreModel for CatalogsFile {
    const SCHEMA_NAME: &'static str = "catalogs";
}

impl Default for CatalogsFile {
    fn default() -> Self {
        Self::new()
    }
}

/// A catalog id: 1 to 64 characters of `a-z 0-9 . _ -`, starting with a
/// letter or digit.
pub fn is_catalog_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

/// Why `url` is not a registry base URL: https (http only for
/// `localhost`, `127.0.0.1` or `[::1]`), no userinfo, query or fragment.
pub fn registry_url_problem(url: &str) -> Option<String> {
    let Ok(parsed) = url::Url::parse(url) else {
        return Some(format!("{url:?} is not a URL"));
    };
    let loopback = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match parsed.scheme() {
        "https" => {}
        "http" if loopback => {}
        "http" => {
            return Some("a registry URL must use https (http only for localhost)".to_owned());
        }
        _ => return Some(format!("{url:?} is not an https URL")),
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Some(format!("{url:?} has no host"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Some("a registry URL must not hold a user name or password".to_owned());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Some("a registry URL must not hold a query or fragment".to_owned());
    }
    None
}

/// Why `url` cannot be a tap: a user name or token in an http(s) URL, or
/// a password in any URL, would be committed to the synced store. An ssh
/// user (`git@host:`, `ssh://git@host/`) is fine.
pub fn tap_url_problem(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url.trim()).ok()?;
    let web = matches!(parsed.scheme(), "http" | "https");
    if parsed.password().is_some() || (web && !parsed.username().is_empty()) {
        return Some(
            "a tap URL must not hold a user name, password or token (it would be saved in your \
             store): use your git credential helper instead"
                .to_owned(),
        );
    }
    None
}

impl CatalogsFile {
    pub fn new() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            catalogs: BTreeMap::new(),
            extra: toml::Table::new(),
        }
    }

    /// Ids are catalog ids; a built-in's table holds only `enabled`; a
    /// user-added catalog has a kind and a URL of that kind (a ref only for
    /// a git tap).
    pub fn validate(&self) -> Diagnostics {
        let mut d = Diagnostics::new();
        for (id, record) in &self.catalogs {
            let path = join("catalogs", id);
            if !is_catalog_id(id) {
                d.error(
                    ErrorCode::InvalidValue,
                    &path,
                    format!(
                        "{id:?} is not a catalog id (1 to 64 characters of a-z, 0-9, '.', '_', '-', \
                         starting with a letter or digit)"
                    ),
                );
                continue;
            }
            if BUILTIN_IDS.contains(&id.as_str()) {
                let extra = [
                    ("kind", record.kind.is_some()),
                    ("url", record.url.is_some()),
                    ("ref", record.git_ref.is_some()),
                    ("title", record.title.is_some()),
                ];
                for (key, set) in extra {
                    if set {
                        d.error(
                            ErrorCode::FieldNotAllowed,
                            join(&path, key),
                            format!("{id:?} is a built-in catalog: only `enabled` can be set"),
                        );
                    }
                }
                continue;
            }
            let Some(kind) = record.kind else {
                d.error(
                    ErrorCode::MissingField,
                    join(&path, "kind"),
                    "a catalog needs its kind (mcp_registry or git_tap)",
                );
                continue;
            };
            let Some(url) = record.url.as_deref().filter(|url| !url.is_empty()) else {
                d.error(
                    ErrorCode::MissingField,
                    join(&path, "url"),
                    "a catalog needs its url",
                );
                continue;
            };
            match kind {
                CatalogSourceKind::McpRegistry => {
                    if let Some(problem) = registry_url_problem(url) {
                        d.error(ErrorCode::InvalidValue, join(&path, "url"), problem);
                    }
                    if record.git_ref.is_some() {
                        d.error(
                            ErrorCode::FieldNotAllowed,
                            join(&path, "ref"),
                            "only a git tap has a ref",
                        );
                    }
                }
                CatalogSourceKind::GitTap => {
                    if let Some(problem) = tap_url_problem(url) {
                        d.error(ErrorCode::InvalidValue, join(&path, "url"), problem);
                    } else if let Err(err) = crate::sources::SourceRef::from_git_url(url) {
                        d.error(ErrorCode::InvalidValue, join(&path, "url"), err.message());
                    }
                    if let Some(git_ref) = &record.git_ref
                        && let Err(err) = crate::skills::install::check_ref(git_ref)
                    {
                        d.error(ErrorCode::InvalidValue, join(&path, "ref"), err.message());
                    }
                }
            }
        }
        d
    }
}

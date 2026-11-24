//! `store/sources.toml`: trust decisions about install sources (SPEC 17,
//! #24). One table per source key (`github.com/acme/skills` for one
//! repository, `github.com/acme/*` for an owner, `git.acme.corp/*` for a
//! host, `file:///C:/repos/x` for a local repository; `http://` or `git://`
//! before the host for a source fetched without encryption), so a pull
//! merges decisions about different sources field by field.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;
use crate::model::validate::{Diagnostics, join};
use crate::model::{SCHEMA_VERSION, StoreModel};
use crate::sources::SourceRef;

/// Trust a source, or never trust it (a block wins over any trust).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SourceDecision {
    Trust,
    Block,
}

impl SourceDecision {
    pub const fn as_str(self) -> &'static str {
        match self {
            SourceDecision::Trust => "trust",
            SourceDecision::Block => "block",
        }
    }
}

/// How much a record covers: one repository, every repository of one owner
/// (user or organization), or every repository on one host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SourceScope {
    Repo,
    Owner,
    Host,
}

impl SourceScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            SourceScope::Repo => "repo",
            SourceScope::Owner => "owner",
            SourceScope::Host => "host",
        }
    }

    /// Reads `repo`, `owner` or `host`.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "repo" => Some(SourceScope::Repo),
            "owner" => Some(SourceScope::Owner),
            "host" => Some(SourceScope::Host),
            _ => None,
        }
    }
}

/// Where a decision was made: never by an agent on its own (#24).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TrustVia {
    /// The app's trust dialog or Settings > Sources.
    #[default]
    App,
    /// An interactive `loadout` prompt.
    CliPrompt,
    /// `--trust-source` on a non-interactive command.
    CliFlag,
    /// `loadout source trust|block`.
    CliCommand,
}

impl TrustVia {
    pub const fn as_str(self) -> &'static str {
        match self {
            TrustVia::App => "app",
            TrustVia::CliPrompt => "cli-prompt",
            TrustVia::CliFlag => "cli-flag",
            TrustVia::CliCommand => "cli-command",
        }
    }
}

/// Why trust lapsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LapseReason {
    /// The followed branch no longer contains the commit installed last
    /// (force-push).
    HistoryRewritten,
    /// The repository was renamed or transferred (the host redirects).
    Moved,
}

impl LapseReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            LapseReason::HistoryRewritten => "history_rewritten",
            LapseReason::Moved => "moved",
        }
    }
}

/// A trust that no longer admits the source until the user decides again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Lapse {
    pub reason: LapseReason,
    #[schemars(with = "String", extend("format" = "date-time"))]
    pub at: toml::value::Datetime,
    /// `moved`: the source key the repository moved to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

/// One decision about one source key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SourceRecord {
    pub decision: SourceDecision,
    /// Must fit the key: `repo` (`host/owner/repo`, `file:///...`),
    /// `owner` (`host/owner`), `host` (`host`).
    pub scope: SourceScope,
    #[schemars(with = "String", extend("format" = "date-time"))]
    pub decided_at: toml::value::Datetime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<TrustVia>,
    /// Label of the machine that decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// The URL the decision was made for, without userinfo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lapsed: Option<Lapse>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "Loadout source trust (store/sources.toml)")]
pub struct SourcesFile {
    /// Schema version; always 1.
    #[schemars(extend("const" = 1))]
    pub schema: i64,
    /// Decisions by source key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sources: BTreeMap<String, SourceRecord>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

impl StoreModel for SourcesFile {
    const SCHEMA_NAME: &'static str = "sources";
}

impl Default for SourcesFile {
    fn default() -> Self {
        Self::new()
    }
}

impl SourcesFile {
    pub fn new() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            sources: BTreeMap::new(),
            extra: toml::Table::new(),
        }
    }

    /// Every key is a canonical source key (the key its own URL gets)
    /// whose shape fits its record's scope.
    pub fn validate(&self) -> Diagnostics {
        let mut diagnostics = Diagnostics::new();
        for (key, record) in &self.sources {
            let path = join("sources", key);
            match SourceRef::canonical_key(key) {
                Err(_) => diagnostics.error(
                    ErrorCode::InvalidValue,
                    &path,
                    format!(
                        "{key:?} is not a source key (host/path for a repository, host/owner/* \
                         for an owner, host/* for a host, file:///path for a local repository)"
                    ),
                ),
                Ok((canonical, _)) if canonical != *key => diagnostics.error(
                    ErrorCode::InvalidValue,
                    &path,
                    format!(
                        "{key:?} is not written the way it is matched; write it as {canonical:?}"
                    ),
                ),
                Ok((_, shape)) if shape != record.scope => diagnostics.error(
                    ErrorCode::InvalidValue,
                    join(&path, "scope"),
                    format!(
                        "scope {:?} does not fit the key {key:?} (a {} key)",
                        record.scope.as_str(),
                        shape.as_str()
                    ),
                ),
                Ok(_) => {}
            }
        }
        diagnostics
    }
}

/// Key prefixes of sources fetched without encryption: their keys are not
/// the keys of the same repository over HTTPS or SSH.
pub const INSECURE_KEY_PREFIXES: [&str; 2] = ["http://", "git://"];

/// `key` without an [`INSECURE_KEY_PREFIXES`] prefix.
pub fn without_insecure_prefix(key: &str) -> &str {
    INSECURE_KEY_PREFIXES
        .iter()
        .find_map(|prefix| key.strip_prefix(prefix))
        .unwrap_or(key)
}

/// The scope a source key's shape stands for: `file:///<path>` and
/// `<host>/<path>` are repositories, `<host>/<owner>/*` an owner,
/// `<host>/*` a host (each may start with `http://` or `git://`). `None`
/// for text that is not a key. Shape only: [`SourcesFile::validate`] also
/// requires the canonical spelling.
pub fn key_shape(key: &str) -> Option<SourceScope> {
    if let Some(path) = key.strip_prefix("file:///") {
        let ok = !path.is_empty()
            && !path.ends_with('/')
            && !path.contains(['*', '\\'])
            && !path.chars().any(char::is_control);
        return ok.then_some(SourceScope::Repo);
    }
    let bad_char = |c: char| c.is_whitespace() || c.is_control() || matches!(c, '\\' | '"' | '@');
    if key.is_empty() || key.chars().any(bad_char) {
        return None;
    }
    let key = without_insecure_prefix(key);
    if key.contains("://") {
        return None;
    }
    let parts: Vec<&str> = key.split('/').collect();
    let host = parts[0];
    let host_ok = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".-:[]_".contains(&b));
    let (path, wildcard) = match parts[1..].split_last() {
        Some((&"*", rest)) => (rest, true),
        _ => (&parts[1..], false),
    };
    let path_ok = path
        .iter()
        .all(|part| !part.is_empty() && *part != "." && *part != ".." && !part.contains('*'));
    if !host_ok || !path_ok {
        return None;
    }
    match (path.len(), wildcard) {
        (0, true) => Some(SourceScope::Host),
        (1, true) => Some(SourceScope::Owner),
        (n, false) if n >= 1 => Some(SourceScope::Repo),
        _ => None,
    }
}

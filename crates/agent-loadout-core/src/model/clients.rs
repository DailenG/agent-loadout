//! Client ids and `store/clients.toml` preferences (SPEC 5.4, 7.3, 7.5).

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;
use crate::model::validate::{Diagnostics, at, join};
use crate::model::{SCHEMA_VERSION, StoreModel};

/// Key accepted in every `client.<id>` table: the native entry name for that
/// client when it differs from the server id (SPEC 5.1).
pub const NATIVE_NAME_KEY: &str = "native_name";

/// A client id the adapter table knows, with the native fields a server may
/// pass through to it in `[client.<id>]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownClient {
    pub id: &'static str,
    /// Native fields with no canonical equivalent (SPEC 7.3). Fields Loadout
    /// renders itself (transport, env, headers, tool filters, enablement,
    /// delivery helpers) are not listed.
    pub passthrough: &'static [&'static str],
}

const fn client(id: &'static str, passthrough: &'static [&'static str]) -> KnownClient {
    KnownClient { id, passthrough }
}

/// Every client id of SPEC 7.3 and 7.5, in table order, with its
/// passthrough allowlist. Validation reads ids from here because most
/// clients have no adapter yet; adapters (`crate::adapters`) take their
/// allowlist from here through [`passthrough_of`], so there is one list.
pub const KNOWN_CLIENTS: &[KnownClient] = &[
    // Tier A (Pi is Tier B since 2026-11-21; built with Tier A in M5)
    client("claude-code", &[]),
    client("omp", &["timeout", "instructions", "auth", "oauth"]),
    client("pi", &["exposure", "timeout", "oauth"]),
    client("codex", &["startup_timeout_sec", "tool_timeout_sec"]),
    client("claude-desktop", &[]),
    // Tier B
    client("copilot", &["oauthClientId"]),
    client("cursor", &["envFile"]),
    // Tier C
    client("gemini", &["timeout", "trust"]),
    client("antigravity", &["timeoutSeconds"]),
    client("opencode", &[]),
    client("grok", &["startup_timeout_sec", "tool_timeout_sec"]),
    // Tier D
    client("vscode-legacy", &[]),
    client("windsurf", &[]),
    client("devin", &[]),
    client("augment", &[]),
    client("cline", &[]),
    client("roo-code", &[]),
    client("kilo-code", &[]),
    client("kiro", &[]),
    client("junie", &[]),
    client("droid", &[]),
    client("commandcode", &[]),
    client("crush", &[]),
    client("qoder", &[]),
    client("trae", &[]),
    client("trae-cn", &[]),
    client("codebuddy", &[]),
    client("iflow", &[]),
    // A Gemini CLI fork, written through Gemini's mapper.
    client("qwen-code", &["timeout", "trust"]),
    client("zencoder", &[]),
    client("openclaw", &[]),
    client("mimo-code", &[]),
    client("workbuddy", &[]),
    client("zcode", &[]),
    client("continue", &[]),
    client("goose", &[]),
    client("hermes", &[]),
    client("intellij", &[]),
];

/// The table entry for `id`, if the client is known.
pub fn known_client(id: &str) -> Option<&'static KnownClient> {
    KNOWN_CLIENTS.iter().find(|client| client.id == id)
}

/// The passthrough allowlist of known client `id`, for the adapter table.
/// Used in constants, where an unknown id fails the build.
pub const fn passthrough_of(id: &str) -> &'static [&'static str] {
    let mut index = 0;
    while index < KNOWN_CLIENTS.len() {
        if const_str_eq(KNOWN_CLIENTS[index].id, id) {
            return KNOWN_CLIENTS[index].passthrough;
        }
        index += 1;
    }
    panic!("passthrough_of: unknown client id");
}

const fn const_str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut index = 0;
    while index < a.len() {
        if a[index] != b[index] {
            return false;
        }
        index += 1;
    }
    true
}

const ALL_DETECTED: &str = "all-detected";

/// Which clients an item is enabled for: `"all-detected"` or a list of ids.
/// Unknown ids are kept as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientSelection {
    /// Every detected client whose capabilities fit.
    AllDetected,
    List(Vec<String>),
}

impl Serialize for ClientSelection {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::AllDetected => serializer.serialize_str(ALL_DETECTED),
            Self::List(ids) => ids.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ClientSelection {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(KeywordOrList {
            keywords: &[ALL_DETECTED],
            keyword: |_| ClientSelection::AllDetected,
            list: ClientSelection::List,
        })
    }
}

impl JsonSchema for ClientSelection {
    fn schema_name() -> Cow<'static, str> {
        "ClientSelection".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "\"all-detected\" or a list of client ids",
            "oneOf": [
                { "const": ALL_DETECTED },
                { "type": "array", "items": { "type": "string" } }
            ]
        })
    }
}

/// VS Code profiles to import: `"default"`, `"all"` or a list of profile ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileSelection {
    Default,
    All,
    List(Vec<String>),
}

impl Serialize for ProfileSelection {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Default => serializer.serialize_str("default"),
            Self::All => serializer.serialize_str("all"),
            Self::List(ids) => ids.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ProfileSelection {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(KeywordOrList {
            keywords: &["default", "all"],
            keyword: |keyword| match keyword {
                "default" => ProfileSelection::Default,
                _ => ProfileSelection::All,
            },
            list: ProfileSelection::List,
        })
    }
}

impl JsonSchema for ProfileSelection {
    fn schema_name() -> Cow<'static, str> {
        "ProfileSelection".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "\"default\", \"all\" or a list of profile ids",
            "oneOf": [
                { "enum": ["default", "all"] },
                { "type": "array", "items": { "type": "string" } }
            ]
        })
    }
}

/// Accepts one of `keywords` or a list of strings.
struct KeywordOrList<T> {
    keywords: &'static [&'static str],
    /// Builds the value for a matched keyword.
    keyword: fn(&str) -> T,
    list: fn(Vec<String>) -> T,
}

impl<'de, T> Visitor<'de> for KeywordOrList<T> {
    type Value = T;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let keywords: Vec<String> = self.keywords.iter().map(|k| format!("{k:?}")).collect();
        write!(f, "{} or a list of ids", keywords.join(", "))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<T, E> {
        match self.keywords.iter().find(|keyword| **keyword == value) {
            Some(keyword) => Ok((self.keyword)(keyword)),
            None => Err(E::invalid_value(de::Unexpected::Str(value), &self)),
        }
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<T, A::Error> {
        let mut ids = Vec::new();
        while let Some(id) = seq.next_element::<String>()? {
            ids.push(id);
        }
        Ok((self.list)(ids))
    }
}

/// `store/clients.toml`: one table per client id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "Loadout client preferences (store/clients.toml)")]
pub struct ClientPrefsFile {
    /// Schema version; always 1.
    #[schemars(extend("const" = 1))]
    pub schema: i64,
    /// Preferences keyed by client id.
    #[serde(flatten)]
    pub clients: BTreeMap<String, ClientPrefs>,
}

impl StoreModel for ClientPrefsFile {
    const SCHEMA_NAME: &'static str = "clients";
}

/// Per-client preferences. Keys a client does not use are ignored by it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClientPrefs {
    /// `false` opts the client out of everything; `true` forces it on even
    /// when it is not detected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// `true`: an owned entry that is no longer desired is disabled through
    /// the client's native mechanism (`enabled = false`) instead of being
    /// removed, when the client has one (SPEC 8.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_disable: Option<bool>,
    /// Where the shared project layer goes (Claude Code: `.mcp.json`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_shared_target: Option<String>,
    /// Where the private project layer goes (Claude Code: `local-scope`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_local_target: Option<String>,
    /// Named profiles to target (omp).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profiles: Option<Vec<String>>,
    /// VS Code variants to import from (`vscode-legacy`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_variants: Option<Vec<String>>,
    /// VS Code profiles to import from (`vscode-legacy`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_profiles: Option<ProfileSelection>,
    /// Keys this build does not model, kept on rewrite.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

/// VS Code variants `vscode-legacy` can import from.
pub const VSCODE_VARIANTS: [&str; 3] = ["Code", "Code - Insiders", "VSCodium"];

impl ClientPrefsFile {
    pub fn new() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            clients: BTreeMap::new(),
        }
    }

    pub fn validate(&self) -> Diagnostics {
        let mut d = Diagnostics::new();
        for (id, prefs) in &self.clients {
            let base = join("", id);
            if known_client(id).is_none() {
                d.warning(
                    ErrorCode::UnknownClient,
                    &base,
                    format!("unknown client {id:?}; kept as written"),
                );
            }
            for (index, variant) in prefs.import_variants.iter().flatten().enumerate() {
                if !VSCODE_VARIANTS.contains(&variant.as_str()) {
                    d.error(
                        ErrorCode::InvalidValue,
                        at(&join(&base, "import_variants"), index),
                        format!(
                            "unknown VS Code variant {variant:?}; expected one of {VSCODE_VARIANTS:?}"
                        ),
                    );
                }
            }
        }
        d
    }
}

impl Default for ClientPrefsFile {
    fn default() -> Self {
        Self::new()
    }
}

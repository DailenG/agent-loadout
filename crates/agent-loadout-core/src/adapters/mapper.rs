//! Entry mappers (SPEC 7.1 `EntryMapper`): one server between Loadout's
//! rendered form and a client's native entry, plus the canonical form used
//! for equality and hashing.
//!
//! [`McpServersJson`] maps the common `mcpServers.<name>` JSON shape used by
//! the Claude clients, omp and Pi; what differs between them comes from
//! their [`Caps`], a few shape switches and a name rule. Codex has its own
//! TOML mapper (`crate::adapters::codex`) built on the helpers here.

use std::collections::BTreeMap;
use std::fmt;

use crate::adapters::caps::{Caps, CommandRef, DisableMech, ToolFilterMech};
use crate::env::Os;
use crate::error::{CoreError, ErrorCode};
use crate::formats::{NativeMap, NativeValue};
use crate::model::server::{ServerFields, Transport};

/// A server ready to write for one client: layers merged, OS overrides
/// applied and secret references rendered for the client's delivery mode
/// (SPEC 6, 11.4). Values are written as they are.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedServer {
    /// The native entry key (the server id or its `native_name`).
    pub name: String,
    pub launch: Launch,
    /// `false` writes the entry natively disabled (`enabled: false`, SPEC
    /// 8.4, 11.9). A client without that mechanism blocks the target
    /// (`unsupported_field`).
    pub enabled: bool,
    /// The tool filter (`[tools]`), written through the client's native
    /// fields ([`Caps::tool_filter`]). A client without them blocks the
    /// target; dropping the filter instead is the renderer's choice.
    pub tools: ToolLists,
    /// `[client.<id>]` fields other than `native_name`, already checked
    /// against the client's passthrough allowlist.
    pub passthrough: NativeMap,
}

impl RenderedServer {
    /// An enabled server without a tool filter or passthrough fields.
    pub fn new(name: impl Into<String>, launch: Launch) -> Self {
        Self {
            name: name.into(),
            launch,
            enabled: true,
            tools: ToolLists::default(),
            passthrough: NativeMap::new(),
        }
    }
}

/// How the client starts or reaches the server.
#[derive(Debug, Clone, PartialEq)]
pub enum Launch {
    Stdio(StdioLaunch),
    Http(RemoteLaunch),
    Sse(RemoteLaunch),
}

impl Launch {
    pub const fn transport(&self) -> Transport {
        match self {
            Launch::Stdio(_) => Transport::Stdio,
            Launch::Http(_) => Transport::Http,
            Launch::Sse(_) => Transport::Sse,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct StdioLaunch {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// Variables the client copies from its own environment into the
    /// server's under the same name (`env` delivery on a client with
    /// `EnvRefSyntax::CodexForward`: Codex `env_vars`).
    pub forward_env: Vec<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RemoteLaunch {
    pub url: String,
    pub headers: BTreeMap<String, String>,
    /// A command printing a JSON object of headers, run by the client at
    /// connect time (`command` delivery on Claude Code, SPEC 11.4).
    pub headers_helper: Option<String>,
    /// A variable whose value the client sends as `Authorization: Bearer
    /// <value>` (Codex `bearer_token_env_var`).
    pub bearer_env: Option<String>,
    /// Header name to the variable whose value the client sends as that
    /// header (Codex `env_http_headers`).
    pub headers_env: BTreeMap<String, String>,
}

/// A tool filter: only `allow` (when set), never `deny`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolLists {
    pub allow: Option<Vec<String>>,
    pub deny: Option<Vec<String>>,
}

impl ToolLists {
    pub fn is_empty(&self) -> bool {
        self.allow.is_none() && self.deny.is_none()
    }
}

/// Context of one write.
#[derive(Debug, Clone, Copy)]
pub struct WriteCx<'a> {
    pub os: Os,
    /// The entry currently in the file under the same key, if any. Keys the
    /// mapper does not model are carried over from it.
    pub existing: Option<&'a NativeValue>,
}

/// A native entry read back as a server definition (import, adopt, the
/// post-write self check).
///
/// Values a client forwards from its environment by name (Codex
/// `env_vars`, `bearer_token_env_var`, `env_http_headers`) read back as
/// `${env:NAME}` references in `env` and `headers` (SPEC 11.2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerDef {
    /// Launch and connection fields; `transport` is always set.
    pub fields: ServerFields,
    pub headers_helper: Option<String>,
    /// The entry is natively disabled (`enabled: false`).
    pub disabled: bool,
    /// The native tool filter.
    pub tools: ToolLists,
    /// Allowlisted native fields, for `[client.<id>]`.
    pub passthrough: NativeMap,
    /// Native fields with no canonical or passthrough equivalent. Reported
    /// on import; carried over by later writes.
    pub unmapped: NativeMap,
}

/// The canonical text of a native entry: object keys sorted, client
/// defaults dropped. Equal canonical forms mean the client sees the same
/// server.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Canonical(String);

impl Canonical {
    /// Canonical text of `value` as it is (keys sorted, nothing dropped).
    pub fn of(value: &NativeValue) -> Self {
        let mut text = String::new();
        write_canonical(value, &mut text);
        Self(text)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a server cannot be written to, or read from, a client. A write
/// failure blocks that target; nothing is dropped silently (SPEC 7.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapError {
    /// The client cannot run this transport (`http` on Claude Desktop,
    /// `ws` read from Claude Code).
    UnsupportedTransport { transport: String },
    /// The client has no native field for a set field (`cwd` on Claude).
    UnsupportedField { field: String },
    /// The entry key breaks the client's naming rule; set `native_name`.
    InvalidName { name: String },
    /// A native value has another kind than the client documents.
    UnexpectedShape {
        field: String,
        expected: &'static str,
    },
}

impl MapError {
    pub const fn code(&self) -> ErrorCode {
        match self {
            MapError::UnsupportedTransport { .. } => ErrorCode::UnsupportedTransport,
            MapError::UnsupportedField { .. } => ErrorCode::UnsupportedField,
            MapError::InvalidName { .. } => ErrorCode::InvalidId,
            MapError::UnexpectedShape { .. } => ErrorCode::UnexpectedShape,
        }
    }
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MapError::UnsupportedTransport { transport } => {
                write!(f, "the client cannot run {transport:?} servers")
            }
            MapError::UnsupportedField { field } => {
                write!(f, "the client has no native field for `{field}`")
            }
            MapError::InvalidName { name } => {
                write!(
                    f,
                    "{name:?} is not a valid entry name for the client; set `native_name`"
                )
            }
            MapError::UnexpectedShape { field, expected } => {
                write!(f, "`{field}` must be {expected}")
            }
        }
    }
}

impl std::error::Error for MapError {}

impl From<MapError> for CoreError {
    fn from(err: MapError) -> Self {
        CoreError::new(err.code(), err.to_string())
    }
}

/// Maps one server to and from a client's native entry.
pub trait EntryMapper: Sync {
    fn to_native(&self, server: &RenderedServer, cx: &WriteCx<'_>)
    -> Result<NativeValue, MapError>;

    /// Reads the entry `key` back. Named as in SPEC 7.1, hence the lint.
    #[allow(clippy::wrong_self_convention)]
    fn from_native(&self, key: &str, value: &NativeValue) -> Result<ServerDef, MapError>;

    /// Sorted, client defaults dropped; used for equality and hashing.
    fn canonical(&self, value: &NativeValue) -> Canonical;

    /// Why the client itself does not load this entry, when it does not
    /// (gate V1 CC-5: Claude Code skips a `url` entry without `type`). Such
    /// an entry still reads back, so import can take it, but importing it
    /// starts a server the client never ran.
    fn skipped_by_client(&self, _value: &NativeValue) -> Option<&'static str> {
        None
    }
}

/// Keys every `mcpServers` entry may have.
const MODELED_KEYS: [&str; 6] = ["type", "command", "args", "env", "url", "headers"];
const HEADERS_HELPER: &str = "headersHelper";
const TYPE: &str = "type";
const CWD: &str = "cwd";
const ENABLED: &str = "enabled";

/// Commands that are `.cmd` or `.bat` shims on Windows (npm-installed
/// launchers), which a client spawning without a shell cannot start.
const WINDOWS_SHIMS: [&str; 7] = ["npx", "npm", "pnpm", "pnpx", "yarn", "yarnpkg", "corepack"];

/// The `mcpServers.<name>` JSON entry: `{type, command, args, env, cwd}` or
/// `{type, url, headers, headersHelper}`, plus `enabled` for clients that
/// disable natively (SPEC 7.3).
///
/// Stdio entries are written with `command` and `args`, plus `env` when it
/// has variables (or the existing entry has the key).
#[derive(Debug)]
pub struct McpServersJson {
    pub caps: &'static Caps,
    /// Write `type: "stdio"` on stdio entries.
    pub type_field: bool,
    /// Write `type` on http entries. SSE entries always get it, since a
    /// bare `url` reads as http.
    pub remote_type: bool,
    /// The client has a native `cwd` field. Without it a server with `cwd`
    /// blocks, and a native `cwd` is unmapped and kept.
    pub cwd: bool,
    /// Other native `type` spellings of http (Pi `streamable-http`).
    pub http_aliases: &'static [&'static str],
    /// The client runs an untyped `url` entry as http, so a missing `type`
    /// equals `http` in the canonical form (omp, Pi). Without it, an
    /// untyped remote entry is not the one Loadout writes (Claude Code
    /// requires `type`).
    pub url_implies_http: bool,
    /// Whether an entry key is valid for the client.
    pub valid_name: fn(&str) -> bool,
}

impl McpServersJson {
    fn headers_helper(&self) -> bool {
        self.caps.command_ref == Some(CommandRef::HeadersHelper)
    }

    fn is_modeled(&self, key: &str) -> bool {
        MODELED_KEYS.contains(&key)
            || (key == CWD && self.cwd)
            || (key == HEADERS_HELPER && self.headers_helper())
            || (key == ENABLED && disables_natively(self.caps))
            || is_tool_key(self.caps, key)
            || self.caps.passthrough_allowlist.contains(&key)
    }

    /// The type the client gives an entry without `type`.
    fn inferred_type(&self, entry: &NativeMap) -> Option<&'static str> {
        if entry.contains_key("command") {
            Some("stdio")
        } else if entry.contains_key("url") && self.url_implies_http {
            Some("http")
        } else {
            None
        }
    }
}

impl EntryMapper for McpServersJson {
    fn to_native(
        &self,
        server: &RenderedServer,
        cx: &WriteCx<'_>,
    ) -> Result<NativeValue, MapError> {
        check_name(self.valid_name, &server.name)?;
        let transport = check_transport(self.caps, &server.launch)?;
        let existing = existing_entry(cx);
        let keep_empty = |key: &str| existing.is_some_and(|entry| entry.contains_key(key));
        let mut entry = NativeMap::new();
        match &server.launch {
            Launch::Stdio(stdio) => {
                if !stdio.forward_env.is_empty() {
                    return unsupported("env_vars");
                }
                if self.type_field {
                    entry.insert(TYPE, "stdio".into());
                }
                let (command, args) = if self.caps.windows_shell_shim && cx.os == Os::Windows {
                    shell_shim(&stdio.command, &stdio.args)
                } else {
                    (stdio.command.clone(), stdio.args.clone())
                };
                entry.insert("command", command.into());
                entry.insert("args", strings(args));
                if !stdio.env.is_empty() || keep_empty("env") {
                    entry.insert("env", string_map(&stdio.env));
                }
                if let Some(cwd) = &stdio.cwd {
                    if !self.cwd {
                        return unsupported(CWD);
                    }
                    entry.insert(CWD, cwd.as_str().into());
                }
            }
            Launch::Http(remote) | Launch::Sse(remote) => {
                if remote.bearer_env.is_some() {
                    return unsupported("bearer_token_env_var");
                }
                if !remote.headers_env.is_empty() {
                    return unsupported("env_http_headers");
                }
                if self.remote_type || transport == Transport::Sse {
                    entry.insert(TYPE, transport.as_str().into());
                }
                entry.insert("url", remote.url.as_str().into());
                if !remote.headers.is_empty() || keep_empty("headers") {
                    entry.insert("headers", string_map(&remote.headers));
                }
                if let Some(helper) = &remote.headers_helper {
                    if !self.headers_helper() {
                        return unsupported(HEADERS_HELPER);
                    }
                    entry.insert(HEADERS_HELPER, helper.as_str().into());
                }
            }
        }
        write_enabled(&mut entry, self.caps, server, existing)?;
        write_tools(&mut entry, self.caps, &server.tools)?;
        write_passthrough(&mut entry, self.caps, server)?;
        carry_over(&mut entry, existing, |key| self.is_modeled(key));
        Ok(NativeValue::Object(entry))
    }

    fn from_native(&self, key: &str, value: &NativeValue) -> Result<ServerDef, MapError> {
        let entry = value.as_object().ok_or_else(|| MapError::UnexpectedShape {
            field: key.to_owned(),
            expected: "an object",
        })?;
        let transport = match entry.get(TYPE) {
            Some(NativeValue::String(kind)) => match kind.as_str() {
                "stdio" => Transport::Stdio,
                "http" => Transport::Http,
                "sse" => Transport::Sse,
                alias if self.http_aliases.contains(&alias) => Transport::Http,
                other => {
                    return Err(MapError::UnsupportedTransport {
                        transport: other.to_owned(),
                    });
                }
            },
            Some(_) => return shape(TYPE, "a string"),
            // SPEC 7.5: `command` means stdio, a bare `url` means http.
            None if entry.contains_key("command") => Transport::Stdio,
            None if entry.contains_key("url") => Transport::Http,
            None => return shape("command", "set (or `url` for a remote server)"),
        };
        let mut fields = ServerFields {
            transport: Some(transport),
            command: string_field(entry, "command")?,
            args: string_list(entry, "args")?,
            cwd: if self.cwd {
                string_field(entry, CWD)?
            } else {
                None
            },
            url: string_field(entry, "url")?,
            headers: string_map_field(entry, "headers")?,
            env: string_map_field(entry, "env")?,
        };
        if self.caps.windows_shell_shim
            && let (Some(command), Some(args)) = (&fields.command, &fields.args)
            && let Some((inner, rest)) = shell_unshim(command, args)
        {
            fields.command = Some(inner);
            fields.args = Some(rest);
        }
        let mut def = ServerDef {
            fields,
            disabled: read_disabled(entry, self.caps)?,
            tools: read_tools(entry, self.caps)?,
            ..ServerDef::default()
        };
        if self.headers_helper() {
            def.headers_helper = string_field(entry, HEADERS_HELPER)?;
        }
        split_rest(&mut def, entry, self.caps, |key| self.is_modeled(key));
        Ok(def)
    }

    fn canonical(&self, value: &NativeValue) -> Canonical {
        let Some(entry) = value.as_object() else {
            return Canonical::of(value);
        };
        let mut entry = entry.clone();
        if let Some(kind) = entry.get(TYPE).and_then(NativeValue::as_str) {
            let kind = if self.http_aliases.contains(&kind) {
                "http"
            } else {
                kind
            };
            if Some(kind) == self.inferred_type(&entry) {
                entry.remove(TYPE);
            } else {
                entry.insert(TYPE, kind.into());
            }
        }
        drop_empty(&mut entry, &["args", "env", "headers"]);
        drop_default_enabled(&mut entry, self.caps);
        Canonical::of(&NativeValue::Object(entry))
    }

    fn skipped_by_client(&self, value: &NativeValue) -> Option<&'static str> {
        let entry = value.as_object()?;
        // An entry with a `command` is stdio whatever else it holds.
        (entry.contains_key("url")
            && !entry.contains_key(TYPE)
            && self.inferred_type(entry).is_none())
        .then_some("has a `url` but no `type`, so the client skips it")
    }
}

// ---------------------------------------------------------------------------
// Shared by the mappers

/// `[A-Za-z0-9_-]+`, the entry name rule of Pi (SPEC 5.1; V2 verifies it).
pub fn word_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// `[A-Za-z0-9_.-]+`: Claude Code and Codex also load names with `.`
/// (gate V1, CC-7 and CX-6, on Windows and Linux).
pub fn dotted_word_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' || byte == b'.'
        })
}

/// The existing entry of a write, when it is an object.
pub(crate) fn existing_entry<'a>(cx: &WriteCx<'a>) -> Option<&'a NativeMap> {
    cx.existing.and_then(NativeValue::as_object)
}

pub(crate) fn check_name(valid: fn(&str) -> bool, name: &str) -> Result<(), MapError> {
    if valid(name) {
        Ok(())
    } else {
        Err(MapError::InvalidName {
            name: name.to_owned(),
        })
    }
}

pub(crate) fn check_transport(caps: &Caps, launch: &Launch) -> Result<Transport, MapError> {
    let transport = launch.transport();
    if caps.transports.contains(transport) {
        Ok(transport)
    } else {
        Err(MapError::UnsupportedTransport {
            transport: transport.as_str().to_owned(),
        })
    }
}

fn disables_natively(caps: &Caps) -> bool {
    caps.native_disable == Some(DisableMech::EnabledFalse)
}

/// `enabled: false` for a disabled server; `enabled: true` only when the
/// existing entry has the key, so an update does not churn it.
pub(crate) fn write_enabled(
    entry: &mut NativeMap,
    caps: &Caps,
    server: &RenderedServer,
    existing: Option<&NativeMap>,
) -> Result<(), MapError> {
    if !server.enabled {
        if !disables_natively(caps) {
            return unsupported(ENABLED);
        }
        entry.insert(ENABLED, NativeValue::Bool(false));
    } else if disables_natively(caps) && existing.is_some_and(|entry| entry.contains_key(ENABLED)) {
        entry.insert(ENABLED, NativeValue::Bool(true));
    }
    Ok(())
}

pub(crate) fn read_disabled(entry: &NativeMap, caps: &Caps) -> Result<bool, MapError> {
    if !disables_natively(caps) {
        return Ok(false);
    }
    match entry.get(ENABLED) {
        None => Ok(false),
        Some(NativeValue::Bool(enabled)) => Ok(!enabled),
        Some(_) => shape(ENABLED, "a boolean"),
    }
}

/// Removes `enabled: true`, the default, when the client models `enabled`.
pub(crate) fn drop_default_enabled(entry: &mut NativeMap, caps: &Caps) {
    if disables_natively(caps) && entry.get(ENABLED) == Some(&NativeValue::Bool(true)) {
        entry.remove(ENABLED);
    }
}

/// Whether `key` is one of the client's native tool filter fields.
pub(crate) fn is_tool_key(caps: &Caps, key: &str) -> bool {
    match caps.tool_filter {
        Some(ToolFilterMech::AllowDeny { allow, deny }) => key == allow || key == deny,
        Some(ToolFilterMech::Allow(allow)) => key == allow,
        None => false,
    }
}

pub(crate) fn write_tools(
    entry: &mut NativeMap,
    caps: &Caps,
    tools: &ToolLists,
) -> Result<(), MapError> {
    let (allow_key, deny_key) = match caps.tool_filter {
        _ if tools.is_empty() => return Ok(()),
        Some(ToolFilterMech::AllowDeny { allow, deny }) => (allow, Some(deny)),
        Some(ToolFilterMech::Allow(allow)) => (allow, None),
        None => return unsupported("tools"),
    };
    if let Some(allow) = &tools.allow {
        entry.insert(allow_key, strings(allow.clone()));
    }
    if let Some(deny) = &tools.deny {
        let Some(deny_key) = deny_key else {
            return unsupported("tools.deny");
        };
        entry.insert(deny_key, strings(deny.clone()));
    }
    Ok(())
}

pub(crate) fn read_tools(entry: &NativeMap, caps: &Caps) -> Result<ToolLists, MapError> {
    Ok(match caps.tool_filter {
        Some(ToolFilterMech::AllowDeny { allow, deny }) => ToolLists {
            allow: string_list(entry, allow)?,
            deny: string_list(entry, deny)?,
        },
        Some(ToolFilterMech::Allow(allow)) => ToolLists {
            allow: string_list(entry, allow)?,
            deny: None,
        },
        None => ToolLists::default(),
    })
}

/// Writes the allowlisted passthrough fields; a timeout must be a
/// non-negative number in the client's unit.
pub(crate) fn write_passthrough(
    entry: &mut NativeMap,
    caps: &Caps,
    server: &RenderedServer,
) -> Result<(), MapError> {
    for (key, value) in server.passthrough.iter() {
        if !caps.passthrough_allowlist.contains(&key) {
            return unsupported(key);
        }
        if let Some(timeout) = caps.timeouts.iter().find(|timeout| timeout.key == key)
            && timeout.duration(value).is_none()
        {
            return shape(key, timeout.expected());
        }
        entry.insert(key, value.clone());
    }
    Ok(())
}

/// Copies the keys of `existing` the mapper does not model.
pub(crate) fn carry_over(
    entry: &mut NativeMap,
    existing: Option<&NativeMap>,
    is_modeled: impl Fn(&str) -> bool,
) {
    for (key, value) in existing.into_iter().flat_map(NativeMap::iter) {
        if !is_modeled(key) {
            entry.insert(key, value.clone());
        }
    }
}

/// Files the keys of `entry` that are passthrough or unmodeled.
pub(crate) fn split_rest(
    def: &mut ServerDef,
    entry: &NativeMap,
    caps: &Caps,
    is_modeled: impl Fn(&str) -> bool,
) {
    for (key, value) in entry.iter() {
        if caps.passthrough_allowlist.contains(&key) {
            def.passthrough.insert(key, value.clone());
        } else if !is_modeled(key) {
            def.unmapped.insert(key, value.clone());
        }
    }
}

/// Removes keys holding an empty array or object.
pub(crate) fn drop_empty(entry: &mut NativeMap, keys: &[&str]) {
    for key in keys {
        let empty = match entry.get(key) {
            Some(NativeValue::Array(items)) => items.is_empty(),
            Some(NativeValue::Object(map)) => map.is_empty(),
            _ => false,
        };
        if empty {
            entry.remove(key);
        }
    }
}

pub(crate) fn unsupported<T>(field: &str) -> Result<T, MapError> {
    Err(MapError::UnsupportedField {
        field: field.to_owned(),
    })
}

pub(crate) fn shape<T>(field: &str, expected: &'static str) -> Result<T, MapError> {
    Err(MapError::UnexpectedShape {
        field: field.to_owned(),
        expected,
    })
}

pub(crate) fn strings(items: Vec<String>) -> NativeValue {
    NativeValue::Array(items.into_iter().map(NativeValue::String).collect())
}

pub(crate) fn string_map(map: &BTreeMap<String, String>) -> NativeValue {
    NativeValue::Object(
        map.iter()
            .map(|(key, value)| (key.as_str(), NativeValue::from(value.as_str())))
            .collect(),
    )
}

pub(crate) fn string_field(entry: &NativeMap, key: &str) -> Result<Option<String>, MapError> {
    match entry.get(key) {
        None => Ok(None),
        Some(NativeValue::String(value)) => Ok(Some(value.clone())),
        Some(_) => shape(key, "a string"),
    }
}

pub(crate) fn string_list(entry: &NativeMap, key: &str) -> Result<Option<Vec<String>>, MapError> {
    let Some(value) = entry.get(key) else {
        return Ok(None);
    };
    value
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        })
        .map(Some)
        .ok_or_else(|| MapError::UnexpectedShape {
            field: key.to_owned(),
            expected: "an array of strings",
        })
}

pub(crate) fn string_map_field(
    entry: &NativeMap,
    key: &str,
) -> Result<Option<BTreeMap<String, String>>, MapError> {
    let Some(value) = entry.get(key) else {
        return Ok(None);
    };
    value
        .as_object()
        .and_then(|map| {
            map.iter()
                .map(|(name, value)| {
                    value
                        .as_str()
                        .map(|value| (name.to_owned(), value.to_owned()))
                })
                .collect::<Option<BTreeMap<_, _>>>()
        })
        .map(Some)
        .ok_or_else(|| MapError::UnexpectedShape {
            field: key.to_owned(),
            expected: "an object of strings",
        })
}

/// Whether `command` is a Windows `.cmd`/`.bat` shim: a known npm launcher
/// named without extension, or any `.cmd`/`.bat` file.
fn is_windows_shim(command: &str) -> bool {
    let name = command
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(command)
        .to_ascii_lowercase();
    name.ends_with(".cmd") || name.ends_with(".bat") || WINDOWS_SHIMS.contains(&name.as_str())
}

/// `npx args...` as `cmd /c npx args...` (SPEC 5.2); other commands as they
/// are.
fn shell_shim(command: &str, args: &[String]) -> (String, Vec<String>) {
    if !is_windows_shim(command) {
        return (command.to_owned(), args.to_vec());
    }
    let mut shimmed = Vec::with_capacity(args.len() + 2);
    shimmed.push("/c".to_owned());
    shimmed.push(command.to_owned());
    shimmed.extend_from_slice(args);
    ("cmd".to_owned(), shimmed)
}

/// The inverse of [`shell_shim`]: `cmd /c <shim> args...` as `<shim>
/// args...`. A `cmd /c` running anything else is kept as written, so it
/// reads back as it would be written.
fn shell_unshim(command: &str, args: &[String]) -> Option<(String, Vec<String>)> {
    let is_cmd = command.eq_ignore_ascii_case("cmd") || command.eq_ignore_ascii_case("cmd.exe");
    match args {
        [flag, inner, rest @ ..]
            if is_cmd && flag.eq_ignore_ascii_case("/c") && is_windows_shim(inner) =>
        {
            Some((inner.clone(), rest.to_vec()))
        }
        _ => None,
    }
}

fn write_canonical(value: &NativeValue, out: &mut String) {
    match value {
        NativeValue::Null => out.push_str("null"),
        NativeValue::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        NativeValue::Integer(value) => out.push_str(&value.to_string()),
        NativeValue::Float(value) => out.push_str(&format!("{value:?}")),
        NativeValue::String(value) => push_string(value, out),
        NativeValue::Datetime(value) => {
            out.push_str("datetime(");
            push_string(value, out);
            out.push(')');
        }
        NativeValue::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        NativeValue::Object(map) => {
            let mut members: Vec<(&str, &NativeValue)> = map.iter().collect();
            members.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (index, (key, item)) in members.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                push_string(key, out);
                out.push(':');
                write_canonical(item, out);
            }
            out.push('}');
        }
    }
}

fn push_string(value: &str, out: &mut String) {
    out.push_str(&serde_json::to_string(value).expect("a string serializes"));
}

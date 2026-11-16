//! Entry mappers (SPEC 7.1 `EntryMapper`): one server between Loadout's
//! rendered form and a client's native entry, plus the canonical form used
//! for equality and hashing.
//!
//! [`McpServersJson`] maps the common `mcpServers.<name>` JSON shape used by
//! both Claude clients; what differs between them comes from their
//! [`Caps`] and name rule.

use std::collections::BTreeMap;
use std::fmt;

use crate::adapters::caps::{Caps, CommandRef};
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
    /// `[client.<id>]` fields other than `native_name`, already checked
    /// against the client's passthrough allowlist.
    pub passthrough: NativeMap,
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
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RemoteLaunch {
    pub url: String,
    pub headers: BTreeMap<String, String>,
    /// A command printing a JSON object of headers, run by the client at
    /// connect time (`command` delivery on Claude Code, SPEC 11.4).
    pub headers_helper: Option<String>,
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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerDef {
    /// Launch and connection fields; `transport` is always set.
    pub fields: ServerFields,
    pub headers_helper: Option<String>,
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
}

/// Keys the `mcpServers` mapper models. `cwd` is not one: neither Claude
/// client has it, so a native `cwd` is unmapped and kept.
const MODELED_KEYS: [&str; 6] = ["type", "command", "args", "env", "url", "headers"];
const HEADERS_HELPER: &str = "headersHelper";
const TYPE: &str = "type";

/// Commands that are `.cmd` or `.bat` shims on Windows (npm-installed
/// launchers), which a client spawning without a shell cannot start.
const WINDOWS_SHIMS: [&str; 7] = ["npx", "npm", "pnpm", "pnpx", "yarn", "yarnpkg", "corepack"];

/// The `mcpServers.<name>` JSON entry: `{type, command, args, env}` or
/// `{type, url, headers, headersHelper}` (SPEC 7.3).
///
/// Stdio entries are written with `command` and `args`, plus `env` when it
/// has variables (or the existing entry has the key). With `type_field`,
/// every stdio entry gets `type`; remote entries always do, since the type
/// is what tells them apart.
#[derive(Debug)]
pub struct McpServersJson {
    pub caps: &'static Caps,
    /// Write `type: "stdio"` on stdio entries.
    pub type_field: bool,
    /// Whether an entry key is valid for the client.
    pub valid_name: fn(&str) -> bool,
}

impl McpServersJson {
    fn headers_helper(&self) -> bool {
        self.caps.command_ref == Some(CommandRef::HeadersHelper)
    }

    fn is_modeled(&self, key: &str) -> bool {
        MODELED_KEYS.contains(&key)
            || (key == HEADERS_HELPER && self.headers_helper())
            || self.caps.passthrough_allowlist.contains(&key)
    }
}

impl EntryMapper for McpServersJson {
    fn to_native(
        &self,
        server: &RenderedServer,
        cx: &WriteCx<'_>,
    ) -> Result<NativeValue, MapError> {
        if !(self.valid_name)(&server.name) {
            return Err(MapError::InvalidName {
                name: server.name.clone(),
            });
        }
        let transport = server.launch.transport();
        if !self.caps.transports.contains(transport) {
            return Err(MapError::UnsupportedTransport {
                transport: transport.as_str().to_owned(),
            });
        }
        let existing = cx.existing.and_then(NativeValue::as_object);
        // An empty map is written only when the entry already has the key,
        // so an update does not churn `"env": {}` (equal canonically).
        let keep_empty = |key: &str| existing.is_some_and(|entry| entry.contains_key(key));
        let mut entry = NativeMap::new();
        match &server.launch {
            Launch::Stdio(stdio) => {
                if stdio.cwd.is_some() {
                    return unsupported("cwd");
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
            }
            Launch::Http(remote) | Launch::Sse(remote) => {
                entry.insert(TYPE, transport.as_str().into());
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
        for (key, value) in server.passthrough.iter() {
            if !self.caps.passthrough_allowlist.contains(&key) {
                return unsupported(key);
            }
            entry.insert(key, value.clone());
        }
        for (key, value) in existing.into_iter().flat_map(NativeMap::iter) {
            if !self.is_modeled(key) {
                entry.insert(key, value.clone());
            }
        }
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
            cwd: None,
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
            ..ServerDef::default()
        };
        if self.headers_helper() {
            def.headers_helper = string_field(entry, HEADERS_HELPER)?;
        }
        for (key, value) in entry.iter() {
            if self.caps.passthrough_allowlist.contains(&key) {
                def.passthrough.insert(key, value.clone());
            } else if !self.is_modeled(key) {
                def.unmapped.insert(key, value.clone());
            }
        }
        Ok(def)
    }

    fn canonical(&self, value: &NativeValue) -> Canonical {
        let Some(entry) = value.as_object() else {
            return Canonical::of(value);
        };
        let mut entry = entry.clone();
        if entry.get(TYPE).and_then(NativeValue::as_str) == Some("stdio")
            && entry.contains_key("command")
        {
            entry.remove(TYPE);
        }
        for key in ["args", "env", "headers"] {
            let empty = match entry.get(key) {
                Some(NativeValue::Array(items)) => items.is_empty(),
                Some(NativeValue::Object(map)) => map.is_empty(),
                _ => false,
            };
            if empty {
                entry.remove(key);
            }
        }
        Canonical::of(&NativeValue::Object(entry))
    }
}

fn unsupported<T>(field: &str) -> Result<T, MapError> {
    Err(MapError::UnsupportedField {
        field: field.to_owned(),
    })
}

fn shape<T>(field: &str, expected: &'static str) -> Result<T, MapError> {
    Err(MapError::UnexpectedShape {
        field: field.to_owned(),
        expected,
    })
}

fn strings(items: Vec<String>) -> NativeValue {
    NativeValue::Array(items.into_iter().map(NativeValue::String).collect())
}

fn string_map(map: &BTreeMap<String, String>) -> NativeValue {
    NativeValue::Object(
        map.iter()
            .map(|(key, value)| (key.as_str(), NativeValue::from(value.as_str())))
            .collect(),
    )
}

fn string_field(entry: &NativeMap, key: &str) -> Result<Option<String>, MapError> {
    match entry.get(key) {
        None => Ok(None),
        Some(NativeValue::String(value)) => Ok(Some(value.clone())),
        Some(_) => shape(key, "a string"),
    }
}

fn string_list(entry: &NativeMap, key: &str) -> Result<Option<Vec<String>>, MapError> {
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

fn string_map_field(
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

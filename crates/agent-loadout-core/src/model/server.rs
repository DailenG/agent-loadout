//! `store/servers/<id>.toml` (SPEC 5.2).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::env::Os;
use crate::error::ErrorCode;
use crate::model::clients::{ClientSelection, NATIVE_NAME_KEY, known_client};
use crate::model::ids::IdKind;
use crate::model::validate::{Diagnostics, at, join};
use crate::model::{SCHEMA_VERSION, StoreModel};

/// OS names accepted in `[os.<name>]` and machine overlays.
pub const OS_NAMES: [&str; 3] = [Os::Windows.as_str(), Os::Linux.as_str(), Os::MacOs.as_str()];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Stdio,
    Http,
    Sse,
}

impl Transport {
    pub const fn as_str(self) -> &'static str {
        match self {
            Transport::Stdio => "stdio",
            Transport::Http => "http",
            Transport::Sse => "sse",
        }
    }
}

/// How secret references reach the server (SPEC 11.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryMode {
    Auto,
    Env,
    Launch,
    Command,
    Inline,
}

/// `store/servers/<id>.toml`: one MCP server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "Loadout MCP server (store/servers/<id>.toml)")]
pub struct Server {
    /// Schema version; always 1.
    #[schemars(extend("const" = 1))]
    pub schema: i64,
    /// Server id; equals the file name.
    #[schemars(extend("pattern" = crate::model::ids::SLUG_PATTERN))]
    pub id: String,
    #[serde(flatten)]
    pub body: ServerBody,
    /// System scope (user-wide) enablement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable: Option<Enable>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

impl StoreModel for Server {
    const SCHEMA_NAME: &'static str = "server";
}

/// A server definition without identity: shared by store servers and
/// project-only servers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ServerBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// `stdio`, `http` or `sse`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<Transport>,
    /// Executable (stdio).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Arguments (stdio).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Working directory (stdio); may use `${home}`, `${project}`, `${loadout_home}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Endpoint (http, sse).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// HTTP headers (http, sse); values may hold references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
    /// Environment (stdio); values are references or literal non-secrets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<Delivery>,
    /// Tool filters, written only through native fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolFilter>,
    /// Per-OS overrides, deep-merged over the definition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<BTreeMap<String, ServerOverride>>,
    /// Native passthrough fields per client id, checked against that
    /// client's allowlist; `native_name` is accepted for every client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<BTreeMap<String, BTreeMap<String, serde_json::Value>>>")]
    pub client: Option<BTreeMap<String, toml::Table>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Launch and connection fields. Used for `[os.<name>]` overrides and
/// machine overlay server overrides.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ServerFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<Transport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
}

/// Field overrides deep-merged over a server (`[os.<name>]`, machine overlay).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ServerOverride {
    #[serde(flatten)]
    pub fields: ServerFields,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Enable {
    /// Client ids, or "all-detected". Unknown ids are a warning and are kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clients: Option<ClientSelection>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Delivery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<DeliveryMode>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolFilter {
    /// Only these tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,
    /// Never these tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<String>>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Meta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// `manual`, `imported:<client>`, `migrated:skilldock` or `catalog:<source>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

impl Server {
    /// A minimal server; fill `body` before saving.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            schema: SCHEMA_VERSION,
            id: id.into(),
            body: ServerBody::default(),
            enable: None,
            extra: toml::Table::new(),
        }
    }

    /// Validates per SPEC 5.2. `file_id` is the file name without `.toml`.
    pub fn validate(&self, file_id: Option<&str>) -> Diagnostics {
        let mut d = Diagnostics::new();
        d.check_named_id(IdKind::Server, &self.id, "id", file_id);
        self.body.validate_into("", true, &mut d);
        if let Some(clients) = self.enable.as_ref().and_then(|e| e.clients.as_ref()) {
            d.check_client_selection(clients, "enable.clients");
        }
        d
    }
}

impl ServerBody {
    /// The launch and connection fields of this definition.
    pub fn fields(&self) -> ServerFields {
        ServerFields {
            transport: self.transport,
            command: self.command.clone(),
            args: self.args.clone(),
            cwd: self.cwd.clone(),
            url: self.url.clone(),
            headers: self.headers.clone(),
            env: self.env.clone(),
        }
    }

    /// Checks templates, OS overrides, client passthrough and metadata;
    /// with `complete`, also the transport rules for every OS.
    pub(crate) fn validate_into(&self, prefix: &str, complete: bool, d: &mut Diagnostics) {
        let base = self.fields();
        base.check_templates(prefix, d);
        let overrides = self.os.as_ref();
        for (os, over) in overrides.into_iter().flatten() {
            let path = join(&join(prefix, "os"), os);
            if !OS_NAMES.contains(&os.as_str()) {
                d.warning(
                    ErrorCode::UnknownOs,
                    &path,
                    format!("unknown OS {os:?}; expected one of {OS_NAMES:?}; kept as written"),
                );
            }
            over.fields.check_templates(&path, d);
        }
        if complete {
            check_transport(&base, overrides, prefix, d);
        }
        for (client_id, table) in self.client.iter().flatten() {
            let path = join(&join(prefix, "client"), client_id);
            check_passthrough(client_id, table, &path, d);
        }
        if let Some(origin) = self.meta.as_ref().and_then(|meta| meta.origin.as_ref())
            && !is_valid_origin(origin)
        {
            d.error(
                ErrorCode::InvalidValue,
                join(&join(prefix, "meta"), "origin"),
                format!(
                    "invalid origin {origin:?}; expected manual, imported:<client>, \
                     migrated:skilldock or catalog:<source>"
                ),
            );
        }
    }
}

impl ServerFields {
    /// `over` deep-merged over `self`: `env` and `headers` merge key by key,
    /// everything else is replaced (SPEC 6.2).
    pub fn merged(&self, over: &ServerFields) -> ServerFields {
        let merge_map = |base: &Option<BTreeMap<String, String>>,
                         over: &Option<BTreeMap<String, String>>| {
            match (base, over) {
                (Some(base), Some(over)) => {
                    let mut merged = base.clone();
                    merged.extend(over.iter().map(|(k, v)| (k.clone(), v.clone())));
                    Some(merged)
                }
                (base, None) => base.clone(),
                (None, over) => over.clone(),
            }
        };
        ServerFields {
            transport: over.transport.or(self.transport),
            command: over.command.clone().or_else(|| self.command.clone()),
            args: over.args.clone().or_else(|| self.args.clone()),
            cwd: over.cwd.clone().or_else(|| self.cwd.clone()),
            url: over.url.clone().or_else(|| self.url.clone()),
            headers: merge_map(&self.headers, &over.headers),
            env: merge_map(&self.env, &over.env),
        }
    }

    fn has(&self, field: &str) -> bool {
        match field {
            "transport" => self.transport.is_some(),
            "command" => self.command.is_some(),
            "args" => self.args.is_some(),
            "cwd" => self.cwd.is_some(),
            "url" => self.url.is_some(),
            "headers" => self.headers.is_some(),
            "env" => self.env.is_some(),
            _ => false,
        }
    }

    /// Every string value goes through the template grammar (SPEC 11.2).
    pub(crate) fn check_templates(&self, prefix: &str, d: &mut Diagnostics) {
        for (field, value) in [
            ("command", &self.command),
            ("cwd", &self.cwd),
            ("url", &self.url),
        ] {
            if let Some(value) = value {
                d.check_template(value, &join(prefix, field));
            }
        }
        for (index, arg) in self.args.iter().flatten().enumerate() {
            d.check_template(arg, &at(&join(prefix, "args"), index));
        }
        for (field, map) in [("headers", &self.headers), ("env", &self.env)] {
            for (key, value) in map.iter().flatten() {
                d.check_template(value, &join(&join(prefix, field), key));
            }
        }
    }
}

/// `stdio` requires `command` and forbids `url`/`headers`; `http`/`sse`
/// require `url` and forbid `command`/`args`/`env`. Checked on the definition
/// as each OS sees it, so a field supplied only through `[os.<name>]` counts
/// for that OS.
fn check_transport(
    base: &ServerFields,
    overrides: Option<&BTreeMap<String, ServerOverride>>,
    prefix: &str,
    d: &mut Diagnostics,
) {
    let has_os_override = overrides
        .is_some_and(|overrides| overrides.keys().any(|os| OS_NAMES.contains(&os.as_str())));
    let views: Vec<(Option<&str>, ServerFields)> = if has_os_override {
        OS_NAMES
            .iter()
            .map(|os| {
                let over = overrides.and_then(|overrides| overrides.get(*os));
                let view = over.map_or_else(|| base.clone(), |over| base.merged(&over.fields));
                (Some(*os), view)
            })
            .collect()
    } else {
        vec![(None, base.clone())]
    };

    // (code, path, message) -> OSes where it applies, in first-seen order.
    let mut problems: Vec<(ErrorCode, String, String, Vec<&'static str>)> = Vec::new();
    let mut report = |code: ErrorCode, field: &str, os: Option<&'static str>, message: String| {
        let path = match os {
            Some(os) if !base.has(field) && code == ErrorCode::FieldNotAllowed => {
                join(&join(&join(prefix, "os"), os), field)
            }
            _ => join(prefix, field),
        };
        match problems
            .iter_mut()
            .find(|(c, p, m, _)| *c == code && *p == path && *m == message)
        {
            Some((.., oses)) => oses.extend(os),
            None => problems.push((code, path, message, os.into_iter().collect())),
        }
    };
    for (os, view) in &views {
        let Some(transport) = view.transport else {
            report(
                ErrorCode::MissingField,
                "transport",
                *os,
                "`transport` is required: stdio, http or sse".to_owned(),
            );
            continue;
        };
        let (required, forbidden): (&str, &[&str]) = match transport {
            Transport::Stdio => ("command", &["url", "headers"]),
            Transport::Http | Transport::Sse => ("url", &["command", "args", "env"]),
        };
        let name = transport.as_str();
        if !view.has(required) {
            report(
                ErrorCode::MissingField,
                required,
                *os,
                format!("{name} servers need `{required}`"),
            );
        }
        for field in forbidden {
            if view.has(field) {
                report(
                    ErrorCode::FieldNotAllowed,
                    field,
                    *os,
                    format!("{name} servers must not set `{field}`"),
                );
            }
        }
    }
    for (code, path, message, oses) in problems {
        let message = if oses.is_empty() || oses.len() == views.len() {
            message
        } else {
            format!("{message} (on {})", oses.join(", "))
        };
        d.error(code, path, message);
    }
}

fn check_passthrough(client_id: &str, table: &toml::Table, path: &str, d: &mut Diagnostics) {
    let known = known_client(client_id);
    if known.is_none() {
        d.warning(
            ErrorCode::UnknownClient,
            path,
            format!("unknown client {client_id:?}; kept as written"),
        );
    }
    for (key, value) in table {
        let key_path = join(path, key);
        if key == NATIVE_NAME_KEY {
            if !value.as_str().is_some_and(|name| !name.trim().is_empty()) {
                d.error(
                    ErrorCode::InvalidValue,
                    key_path,
                    "`native_name` must be a non-empty string",
                );
            }
            continue;
        }
        if let Some(known) = known
            && !known.passthrough.contains(&key.as_str())
        {
            let allowed = if known.passthrough.is_empty() {
                format!("{client_id} accepts only `{NATIVE_NAME_KEY}`")
            } else {
                format!(
                    "{client_id} accepts `{NATIVE_NAME_KEY}`, `{}`",
                    known.passthrough.join("`, `")
                )
            };
            d.error(
                ErrorCode::PassthroughNotAllowed,
                &key_path,
                format!("`{key}` is not a passthrough field: {allowed}"),
            );
            continue;
        }
        check_value_templates(value, &key_path, d);
    }
}

fn check_value_templates(value: &toml::Value, path: &str, d: &mut Diagnostics) {
    match value {
        toml::Value::String(text) => d.check_template(text, path),
        toml::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                check_value_templates(item, &at(path, index), d);
            }
        }
        toml::Value::Table(table) => {
            for (key, item) in table {
                check_value_templates(item, &join(path, key), d);
            }
        }
        _ => {}
    }
}

fn is_valid_origin(origin: &str) -> bool {
    match origin.split_once(':') {
        None => origin == "manual",
        Some(("imported", client)) => !client.is_empty(),
        Some(("migrated", source)) => source == "skilldock",
        Some(("catalog", source)) => !source.is_empty(),
        Some(_) => false,
    }
}

//! Tier D ported clients (SPEC 7.5): carried over from SkillDock's tables,
//! user scope only, best effort.
//!
//! Every client here has one user file (or a first-of list) and no project
//! files; `[verify]` marks what no real client has confirmed yet. The
//! shapes:
//!
//! - Most JSON clients keep SkillDock's unified entry under `mcpServers`:
//!   `{type: "stdio", command, args, env}` or `{type: "http"|"sse", url,
//!   headers}` (`augment`, `kiro`, `junie`, `droid`, `commandcode`,
//!   `crush` (under `mcp`), `qoder`, `trae`/`trae-cn`, `codebuddy`,
//!   `iflow`, `zencoder` (under `zencoder.mcpServers`, nested), `workbuddy`,
//!   `zcode` (under `mcp.servers`)). No `cwd` and no tool filter.
//! - `cline`, `roo-code` and `kilo-code` (Cline and its forks) disable an
//!   entry with `disabled: true` and spell Streamable HTTP as
//!   `streamableHttp` (Cline) or `streamable-http` (Roo, Kilo).
//! - `zcode` disables an entry with `enable: false`. The planner's native
//!   disable knows only `enabled` and `disabled`, so the mapper reads and
//!   writes the flag but the caps declare none: an entry Loadout no longer
//!   wants is removed rather than disabled.
//! - `trae` and `trae-cn` also write every existing
//!   `User/profiles/<id>/mcp.json`, as SkillDock did.
//! - `codebuddy` and `openclaw` files are JSON5: written only when they
//!   also parse as JSONC, else `read_only_format`.
//! - `openclaw` (`mcp.servers`) and `mimo-code` (`mcp`) use OpenCode's
//!   agent shape (`{type: "local", command: [cmd, ...args], environment,
//!   enabled}` or `{type: "remote", url, headers, enabled}`), through
//!   OpenCode's mapper.
//! - `qwen-code` (a Gemini CLI fork) uses Gemini's mapper: `httpUrl` is
//!   Streamable HTTP, a bare `url` SSE, `includeTools`/`excludeTools` the
//!   tool filter.
//! - `continue`: YAML list `mcpServers`, matched by `name` (the
//!   `[name]` container segment): `{name, command, args, env, cwd}` or
//!   `{name, type: "streamable-http"|"sse", url, requestOptions: {headers}}`.
//!   SkillDock's `transport` key reads as `type`.
//! - `goose`: YAML `extensions.<name>`: `{enabled, name, type: "stdio",
//!   cmd, args, envs}` or `{enabled, name, type: "streamable_http", uri,
//!   headers}`. SSE is rejected; built-in extensions read as
//!   `unsupported_transport`. Goose needs `enabled`, so it is always
//!   written.
//! - `hermes`: YAML `mcp_servers.<name>`: `{command, args, env}`, `{url,
//!   headers}`, and `transport: "sse"` for SSE.
//! - `intellij` has skills only (shared with Junie): no MCP target.
//!
//! Assumptions left for Tier D verification (`scripts/verify-clients.md`,
//! TD-*): `[verify]` every path per OS (TD-1; `qoder` uses
//! `~/.config/Qoder/SharedClientCache` on every OS as SkillDock did, and
//! `goose` honors `GOOSE_PATH_ROOT`), that the unified JSON entry with
//! `type` loads in each JSON client (TD-2), the Cline-family disable flag
//! and http spelling (TD-3), Kiro's `disabled` flag (TD-4), `zcode`'s
//! `enable` flag (TD-5), `zencoder`'s nested container rather than a dotted
//! key (TD-6), the Trae profile files (TD-7), the first-of files of
//! `codebuddy` and `iflow` (TD-8), the agent shape of `openclaw` and
//! `mimo-code` and MiMo's `{env:NAME}` (TD-9), Qwen Code's Gemini shape
//! (TD-10), Continue's `type` spellings and `requestOptions.headers`
//! (TD-11), Goose's `uri`, `headers` and required keys (TD-12), Hermes's
//! `transport: sse` (TD-13), that none of these clients expands `${...}`
//! references Loadout could rely on (TD-14), and the detection probes
//! (TD-15).

use std::borrow::Cow;
use std::path::PathBuf;

use crate::adapters::caps::{
    Caps, DisableMech, EnvRefSyntax, FieldSet, ProjectSemantics, Source, TimeUnit, TimeoutField,
    ToolFilterMech, TransportSet,
};
use crate::adapters::detect::Probe;
use crate::adapters::location::Location;
use crate::adapters::mapper::{
    Canonical, EntryMapper, Launch, MapError, McpServersJson, RenderedServer, ServerDef, WriteCx,
    carry_over, check_name, check_transport, drop_default_enabled, drop_empty, existing_entry,
    read_disabled, shape, split_rest, string_field, string_list, string_map, string_map_field,
    strings, unsupported, write_enabled, write_passthrough, write_tools,
};
use crate::adapters::spec::{AdapterSpec, ContainerPath, McpSpec, Tier, UserFiles, exact_names};
use crate::adapters::{gemini, opencode};
use crate::env::{Env, Os};
use crate::error::{CoreError, ErrorCode};
use crate::formats::{Format, NativeMap, NativeValue};
use crate::fs::Fs;
use crate::model::clients::{ClientPrefs, passthrough_of};
use crate::model::server::{ServerFields, Transport};

const ALL_OSES: &[Os] = &[Os::Windows, Os::Linux, Os::MacOs];
const MCP_SERVERS: ContainerPath = ContainerPath(&["mcpServers"]);
/// Other `type` spellings of Streamable HTTP, read as `http`.
const HTTP_ALIASES: &[&str] = &["streamable-http", "streamableHttp"];

fn non_empty(name: &str) -> bool {
    !name.is_empty()
}

/// The caps of a ported client: user scope only, nothing expanded that
/// Loadout relies on (TD-14).
const fn ported_caps(
    id: &str,
    transports: TransportSet,
    native_disable: Option<DisableMech>,
) -> Caps {
    Caps {
        transports,
        env_ref: None,
        env_ref_fields: FieldSet::NONE,
        command_ref: None,
        native_disable,
        project_disable: None,
        project_semantics: ProjectSemantics::Unknown,
        precedence: &[Source::User],
        compat_reads: &[],
        trust_gate: None,
        rewrites_file_at_runtime: false,
        restart_required: false,
        tool_filter: None,
        windows_shell_shim: false,
        timeouts: &[],
        passthrough_allowlist: passthrough_of(id),
    }
}

/// A user-scope-only MCP spec.
const fn user_only(
    user: &'static [Location],
    user_files: Option<UserFiles>,
    format: Format,
    container: ContainerPath,
    mapper: &'static dyn EntryMapper,
    caps: Caps,
) -> McpSpec {
    McpSpec {
        oses: ALL_OSES,
        read_only: false,
        user,
        user_files,
        user_mirrors: &[],
        profiles: None,
        project: &[],
        project_local: &[],
        format,
        skeleton: "",
        container,
        project_local_container: container,
        legacy_containers: &[],
        same_name: exact_names,
        replaced_by: &[],
        mapper,
        caps,
    }
}

/// SkillDock's unified JSON entry: `type` on every entry.
const fn unified(caps: &'static Caps) -> McpServersJson {
    McpServersJson {
        caps,
        type_field: true,
        remote_type: true,
        cwd: false,
        http_aliases: HTTP_ALIASES,
        url_implies_http: true,
        valid_name: non_empty,
    }
}

// ---------------------------------------------------------------------------
// Mapper wrappers

/// [`McpServersJson`] with some top-level keys spelled differently in the
/// file (`(ours, native)`: Windsurf `url` as `serverUrl`, Devin and Hermes
/// `type` as `transport`). A native entry that uses our spelling reads the
/// same.
pub(crate) struct Renamed {
    pub(crate) inner: McpServersJson,
    pub(crate) renames: &'static [(&'static str, &'static str)],
}

impl Renamed {
    /// `value` with native spellings turned into ours (where ours is
    /// absent).
    fn ours<'v>(&self, value: &'v NativeValue) -> Cow<'v, NativeValue> {
        let Some(entry) = value.as_object() else {
            return Cow::Borrowed(value);
        };
        if !self
            .renames
            .iter()
            .any(|(ours, native)| entry.contains_key(native) && !entry.contains_key(ours))
        {
            return Cow::Borrowed(value);
        }
        Cow::Owned(NativeValue::Object(rename(entry, self.renames, false)))
    }
}

/// `entry` with keys renamed `ours -> native` (or back, with `back`),
/// keeping their position; a key whose new name is taken stays.
fn rename(entry: &NativeMap, renames: &[(&str, &str)], to_native: bool) -> NativeMap {
    entry
        .iter()
        .map(|(key, value)| {
            let renamed = renames.iter().find_map(|(ours, native)| {
                let (from, to) = if to_native {
                    (*ours, *native)
                } else {
                    (*native, *ours)
                };
                (key == from && !entry.contains_key(to)).then_some(to)
            });
            (renamed.unwrap_or(key).to_owned(), value.clone())
        })
        .collect()
}

impl EntryMapper for Renamed {
    fn to_native(
        &self,
        server: &RenderedServer,
        cx: &WriteCx<'_>,
    ) -> Result<NativeValue, MapError> {
        let existing = cx.existing.map(|value| self.ours(value));
        let value = self.inner.to_native(
            server,
            &WriteCx {
                os: cx.os,
                existing: existing.as_deref(),
            },
        )?;
        Ok(match value {
            NativeValue::Object(entry) => NativeValue::Object(rename(&entry, self.renames, true)),
            other => other,
        })
    }

    fn from_native(&self, key: &str, value: &NativeValue) -> Result<ServerDef, MapError> {
        self.inner.from_native(key, &self.ours(value))
    }

    fn canonical(&self, value: &NativeValue) -> Canonical {
        self.inner.canonical(&self.ours(value))
    }

    fn skipped_by_client(&self, value: &NativeValue) -> Option<&'static str> {
        self.inner.skipped_by_client(&self.ours(value))
    }
}

/// [`McpServersJson`] writing another `type` for Streamable HTTP (Cline
/// `streamableHttp`, Roo and Kilo `streamable-http`).
struct HttpSpelling {
    inner: McpServersJson,
    http: &'static str,
}

impl EntryMapper for HttpSpelling {
    fn to_native(
        &self,
        server: &RenderedServer,
        cx: &WriteCx<'_>,
    ) -> Result<NativeValue, MapError> {
        let mut value = self.inner.to_native(server, cx)?;
        if let NativeValue::Object(entry) = &mut value
            && entry.get(TYPE).and_then(NativeValue::as_str) == Some("http")
        {
            entry.insert(TYPE, self.http.into());
        }
        Ok(value)
    }

    fn from_native(&self, key: &str, value: &NativeValue) -> Result<ServerDef, MapError> {
        self.inner.from_native(key, value)
    }

    fn canonical(&self, value: &NativeValue) -> Canonical {
        self.inner.canonical(value)
    }

    fn skipped_by_client(&self, value: &NativeValue) -> Option<&'static str> {
        self.inner.skipped_by_client(value)
    }
}

const TYPE: &str = "type";
const ENABLE: &str = "enable";

/// [`McpServersJson`] plus ZCode's `enable` flag: `enable: false` disables
/// the entry; `enable: true` is written only when the entry had the key.
struct EnableFlag(McpServersJson);

impl EnableFlag {
    fn without_flag(value: &NativeValue) -> Cow<'_, NativeValue> {
        match value.as_object() {
            Some(entry) if entry.contains_key(ENABLE) => {
                let mut entry = entry.clone();
                entry.remove(ENABLE);
                Cow::Owned(NativeValue::Object(entry))
            }
            _ => Cow::Borrowed(value),
        }
    }
}

impl EntryMapper for EnableFlag {
    fn to_native(
        &self,
        server: &RenderedServer,
        cx: &WriteCx<'_>,
    ) -> Result<NativeValue, MapError> {
        let had_flag = existing_entry(cx).is_some_and(|entry| entry.contains_key(ENABLE));
        let existing = cx.existing.map(Self::without_flag);
        let enabled = RenderedServer {
            enabled: true,
            ..server.clone()
        };
        let mut value = self.0.to_native(
            &enabled,
            &WriteCx {
                os: cx.os,
                existing: existing.as_deref(),
            },
        )?;
        if let NativeValue::Object(entry) = &mut value
            && (!server.enabled || had_flag)
        {
            entry.insert(ENABLE, NativeValue::Bool(server.enabled));
        }
        Ok(value)
    }

    fn from_native(&self, key: &str, value: &NativeValue) -> Result<ServerDef, MapError> {
        let disabled = match value.as_object().and_then(|entry| entry.get(ENABLE)) {
            None => false,
            Some(NativeValue::Bool(enable)) => !enable,
            Some(_) => return shape(ENABLE, "a boolean"),
        };
        let mut def = self.0.from_native(key, &Self::without_flag(value))?;
        def.disabled = disabled;
        Ok(def)
    }

    fn canonical(&self, value: &NativeValue) -> Canonical {
        match value.as_object() {
            Some(entry) if entry.get(ENABLE) == Some(&NativeValue::Bool(true)) => {
                self.0.canonical(&Self::without_flag(value))
            }
            _ => self.0.canonical(value),
        }
    }

    fn skipped_by_client(&self, value: &NativeValue) -> Option<&'static str> {
        self.0.skipped_by_client(&Self::without_flag(value))
    }
}

// ---------------------------------------------------------------------------
// Continue

const REQUEST_OPTIONS: &str = "requestOptions";
const HEADERS: &str = "headers";

/// An item of Continue's `mcpServers` list, without its `name` (the
/// container key).
struct ContinueYaml {
    caps: &'static Caps,
}

impl ContinueYaml {
    fn is_modeled(&self, key: &str) -> bool {
        [
            "type",
            "transport",
            "command",
            "args",
            "env",
            "cwd",
            "url",
            REQUEST_OPTIONS,
        ]
        .contains(&key)
            || self.caps.passthrough_allowlist.contains(&key)
    }

    /// The transport of an entry: `type` (or SkillDock's `transport`), else
    /// `command` means stdio and `url` Streamable HTTP.
    fn transport(entry: &NativeMap) -> Result<Transport, MapError> {
        let kind = match (entry.get("type"), entry.get("transport")) {
            (Some(NativeValue::String(kind)), _) | (None, Some(NativeValue::String(kind))) => {
                Some(kind.as_str())
            }
            (Some(_), _) => return shape("type", "a string"),
            (None, Some(_)) => return shape("transport", "a string"),
            (None, None) => None,
        };
        match kind {
            Some("stdio") => Ok(Transport::Stdio),
            Some("sse") => Ok(Transport::Sse),
            Some("streamable-http" | "streamableHttp" | "http") => Ok(Transport::Http),
            Some(other) => Err(MapError::UnsupportedTransport {
                transport: other.to_owned(),
            }),
            None if entry.contains_key("command") => Ok(Transport::Stdio),
            None if entry.contains_key("url") => Ok(Transport::Http),
            None => shape("command", "set (or `url` for a remote server)"),
        }
    }

    /// `requestOptions` as an object, or empty.
    fn request_options(entry: &NativeMap) -> Result<NativeMap, MapError> {
        match entry.get(REQUEST_OPTIONS) {
            None => Ok(NativeMap::new()),
            Some(NativeValue::Object(options)) => Ok(options.clone()),
            Some(_) => shape(REQUEST_OPTIONS, "a mapping"),
        }
    }
}

impl EntryMapper for ContinueYaml {
    fn to_native(
        &self,
        server: &RenderedServer,
        cx: &WriteCx<'_>,
    ) -> Result<NativeValue, MapError> {
        check_name(non_empty, &server.name)?;
        let transport = check_transport(self.caps, &server.launch)?;
        let existing = existing_entry(cx);
        let keep_empty = |key: &str| existing.is_some_and(|entry| entry.contains_key(key));
        let mut entry = NativeMap::new();
        match &server.launch {
            Launch::Stdio(stdio) => {
                if !stdio.forward_env.is_empty() {
                    return unsupported("env_vars");
                }
                entry.insert("command", stdio.command.as_str().into());
                entry.insert("args", strings(stdio.args.clone()));
                if !stdio.env.is_empty() || keep_empty("env") {
                    entry.insert("env", string_map(&stdio.env));
                }
                if let Some(cwd) = &stdio.cwd {
                    entry.insert("cwd", cwd.as_str().into());
                }
            }
            Launch::Http(remote) | Launch::Sse(remote) => {
                if remote.headers_helper.is_some() {
                    return unsupported("headersHelper");
                }
                if remote.bearer_env.is_some() {
                    return unsupported("bearer_token_env_var");
                }
                if !remote.headers_env.is_empty() {
                    return unsupported("env_http_headers");
                }
                let kind = if transport == Transport::Sse {
                    "sse"
                } else {
                    "streamable-http"
                };
                entry.insert("type", kind.into());
                entry.insert("url", remote.url.as_str().into());
                let mut options = existing
                    .map(Self::request_options)
                    .transpose()?
                    .unwrap_or_default();
                if remote.headers.is_empty() {
                    options.remove(HEADERS);
                } else {
                    options.insert(HEADERS, string_map(&remote.headers));
                }
                if !options.is_empty() {
                    entry.insert(REQUEST_OPTIONS, NativeValue::Object(options));
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
            expected: "a mapping",
        })?;
        let transport = Self::transport(entry)?;
        let mut options = Self::request_options(entry)?;
        let headers = match options.remove(HEADERS) {
            None => None,
            Some(headers) => {
                let holder: NativeMap = [(HEADERS, headers)].into_iter().collect();
                string_map_field(&holder, HEADERS)?
            }
        };
        let fields = if transport == Transport::Stdio {
            ServerFields {
                transport: Some(transport),
                command: string_field(entry, "command")?,
                args: string_list(entry, "args")?,
                env: string_map_field(entry, "env")?,
                cwd: string_field(entry, "cwd")?,
                ..ServerFields::default()
            }
        } else {
            ServerFields {
                transport: Some(transport),
                url: string_field(entry, "url")?,
                headers,
                ..ServerFields::default()
            }
        };
        let mut def = ServerDef {
            fields,
            ..ServerDef::default()
        };
        split_rest(&mut def, entry, self.caps, |key| self.is_modeled(key));
        if !options.is_empty() {
            def.unmapped
                .insert(REQUEST_OPTIONS, NativeValue::Object(options));
        }
        Ok(def)
    }

    fn canonical(&self, value: &NativeValue) -> Canonical {
        let Some(entry) = value.as_object() else {
            return Canonical::of(value);
        };
        let mut entry = entry.clone();
        if !entry.contains_key("type")
            && let Some(kind) = entry.remove("transport")
        {
            entry.insert("type", kind);
        }
        match entry.get("type").and_then(NativeValue::as_str) {
            Some("stdio") if entry.contains_key("command") => {
                entry.remove("type");
            }
            Some("http" | "streamableHttp") => {
                entry.insert("type", "streamable-http".into());
            }
            _ => {}
        }
        if let Some(NativeValue::Object(options)) = entry.get_mut(REQUEST_OPTIONS) {
            drop_empty(options, &[HEADERS]);
        }
        drop_empty(&mut entry, &["args", "env", REQUEST_OPTIONS]);
        Canonical::of(&NativeValue::Object(entry))
    }
}

// ---------------------------------------------------------------------------
// Goose

const GOOSE_STDIO: &str = "stdio";
const GOOSE_HTTP: &str = "streamable_http";

/// A Goose `extensions.<name>` entry.
struct GooseYaml {
    caps: &'static Caps,
}

impl GooseYaml {
    fn is_modeled(&self, key: &str) -> bool {
        [
            "type", "cmd", "args", "envs", "uri", "url", "headers", "enabled", "name",
        ]
        .contains(&key)
            || self.caps.passthrough_allowlist.contains(&key)
    }
}

impl EntryMapper for GooseYaml {
    fn to_native(
        &self,
        server: &RenderedServer,
        cx: &WriteCx<'_>,
    ) -> Result<NativeValue, MapError> {
        check_name(non_empty, &server.name)?;
        check_transport(self.caps, &server.launch)?;
        let existing = existing_entry(cx);
        let keep_empty = |key: &str| existing.is_some_and(|entry| entry.contains_key(key));
        // Goose's own key order (it writes keys sorted).
        let mut entry = NativeMap::new();
        match &server.launch {
            Launch::Stdio(stdio) => {
                if !stdio.forward_env.is_empty() {
                    return unsupported("env_vars");
                }
                if stdio.cwd.is_some() {
                    return unsupported("cwd");
                }
                entry.insert("args", strings(stdio.args.clone()));
                entry.insert("cmd", stdio.command.as_str().into());
                entry.insert("enabled", NativeValue::Bool(server.enabled));
                if !stdio.env.is_empty() || keep_empty("envs") {
                    entry.insert("envs", string_map(&stdio.env));
                }
                entry.insert("name", server.name.as_str().into());
                entry.insert("type", GOOSE_STDIO.into());
            }
            Launch::Http(remote) => {
                if remote.headers_helper.is_some() {
                    return unsupported("headersHelper");
                }
                if remote.bearer_env.is_some() {
                    return unsupported("bearer_token_env_var");
                }
                if !remote.headers_env.is_empty() {
                    return unsupported("env_http_headers");
                }
                entry.insert("enabled", NativeValue::Bool(server.enabled));
                if !remote.headers.is_empty() || keep_empty(HEADERS) {
                    entry.insert(HEADERS, string_map(&remote.headers));
                }
                entry.insert("name", server.name.as_str().into());
                entry.insert("type", GOOSE_HTTP.into());
                entry.insert("uri", remote.url.as_str().into());
            }
            Launch::Sse(_) => unreachable!("check_transport rejects SSE"),
        }
        write_tools(&mut entry, self.caps, &server.tools)?;
        write_passthrough(&mut entry, self.caps, server)?;
        carry_over(&mut entry, existing, |key| self.is_modeled(key));
        Ok(NativeValue::Object(entry))
    }

    fn from_native(&self, key: &str, value: &NativeValue) -> Result<ServerDef, MapError> {
        let entry = value.as_object().ok_or_else(|| MapError::UnexpectedShape {
            field: key.to_owned(),
            expected: "a mapping",
        })?;
        let fields = match entry.get("type") {
            Some(NativeValue::String(kind)) if kind == GOOSE_STDIO => ServerFields {
                transport: Some(Transport::Stdio),
                command: string_field(entry, "cmd")?,
                args: string_list(entry, "args")?,
                env: string_map_field(entry, "envs")?,
                ..ServerFields::default()
            },
            Some(NativeValue::String(kind)) if kind == GOOSE_HTTP => ServerFields {
                transport: Some(Transport::Http),
                // SkillDock wrote `url`.
                url: match string_field(entry, "uri")? {
                    Some(uri) => Some(uri),
                    None => string_field(entry, "url")?,
                },
                headers: string_map_field(entry, HEADERS)?,
                ..ServerFields::default()
            },
            Some(NativeValue::String(other)) => {
                return Err(MapError::UnsupportedTransport {
                    transport: other.clone(),
                });
            }
            Some(_) => return shape("type", "a string"),
            None => return shape("type", "set (`stdio` or `streamable_http`)"),
        };
        if fields.transport == Some(Transport::Stdio) && fields.command.is_none() {
            return shape("cmd", "set");
        }
        let mut def = ServerDef {
            fields,
            disabled: read_disabled(entry, self.caps)?,
            ..ServerDef::default()
        };
        split_rest(&mut def, entry, self.caps, |key| self.is_modeled(key));
        Ok(def)
    }

    fn canonical(&self, value: &NativeValue) -> Canonical {
        let Some(entry) = value.as_object() else {
            return Canonical::of(value);
        };
        let mut entry = entry.clone();
        if !entry.contains_key("uri")
            && let Some(url) = entry.remove("url")
        {
            entry.insert("uri", url);
        }
        drop_empty(&mut entry, &["args", "envs", HEADERS]);
        drop_default_enabled(&mut entry, self.caps);
        Canonical::of(&NativeValue::Object(entry))
    }
}

// ---------------------------------------------------------------------------
// User files

/// `<AppData>/<product>/User/mcp.json`, then each existing
/// `User/profiles/<id>/mcp.json` (sorted), labeled `profiles/<id>`.
fn trae_files(
    env: &Env,
    fs: &dyn Fs,
    product: &str,
) -> Result<Vec<(PathBuf, Option<String>)>, CoreError> {
    let user = Location::AppData("")
        .resolve(env, None)
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::NotSupportedOnOs,
                format!("{product} has no user directory on {}", env.os().as_str()),
            )
        })?
        .join(product)
        .join("User");
    let mut files = vec![(user.join("mcp.json"), None)];
    let profiles = user.join("profiles");
    if fs.is_dir(&profiles) {
        let mut ids: Vec<String> = fs
            .read_dir(&profiles)
            .unwrap_or_default()
            .into_iter()
            .filter(|dir| fs.exists(&dir.join("mcp.json")))
            .filter_map(|dir| dir.file_name()?.to_str().map(str::to_owned))
            .collect();
        ids.sort();
        for id in ids {
            files.push((
                profiles.join(&id).join("mcp.json"),
                Some(format!("profiles/{id}")),
            ));
        }
    }
    Ok(files)
}

fn trae_user_files(
    env: &Env,
    fs: &dyn Fs,
    _prefs: &ClientPrefs,
) -> Result<Vec<(PathBuf, Option<String>)>, CoreError> {
    trae_files(env, fs, "Trae")
}

fn trae_cn_user_files(
    env: &Env,
    fs: &dyn Fs,
    _prefs: &ClientPrefs,
) -> Result<Vec<(PathBuf, Option<String>)>, CoreError> {
    trae_files(env, fs, "Trae CN")
}

/// `$GOOSE_PATH_ROOT/config/config.yaml` when the variable holds an
/// absolute path; else `%APPDATA%\Block\goose\config\config.yaml` on
/// Windows and `$XDG_CONFIG_HOME/goose/config.yaml` (default `~/.config`)
/// elsewhere.
fn goose_user_files(
    env: &Env,
    _fs: &dyn Fs,
    _prefs: &ClientPrefs,
) -> Result<Vec<(PathBuf, Option<String>)>, CoreError> {
    let root = Location::EnvDir {
        var: "GOOSE_PATH_ROOT",
        fallback: &Location::Home(""),
        rel: "",
        tilde: false,
    };
    let file = match env.var("GOOSE_PATH_ROOT").map(PathBuf::from) {
        Some(path) if path.is_absolute() => root
            .resolve(env, None)
            .map(|root| root.join("config").join("config.yaml")),
        _ if env.os() == Os::Windows => {
            Location::AppData("Block/goose/config/config.yaml").resolve(env, None)
        }
        _ => Location::XdgConfigAllOs("goose/config.yaml").resolve(env, None),
    };
    let file = file.ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotSupportedOnOs,
            format!("Goose has no config path on {}", env.os().as_str()),
        )
    })?;
    Ok(vec![(file, None)])
}

// ---------------------------------------------------------------------------
// The table

macro_rules! unified_client {
    (
        $spec:ident, $caps:ident, $mapper:ident, $id:literal, $name:literal,
        detect: $detect:expr,
        user: $user:expr,
        format: $format:expr,
        container: $container:expr,
        disable: $disable:expr $(,)?
    ) => {
        const $caps: Caps = ported_caps($id, TransportSet::ALL, $disable);
        static $mapper: McpServersJson = unified(&$caps);
        pub static $spec: AdapterSpec = AdapterSpec {
            id: $id,
            display_name: $name,
            tier: Tier::D,
            detect: $detect,
            mcp: Some(user_only($user, None, $format, $container, &$mapper, $caps)),
        };
    };
}

unified_client!(
    AUGMENT, AUGMENT_CAPS, AUGMENT_MAPPER, "augment", "Augment",
    detect: &[Probe::Path(Location::Home(".augment"))],
    user: &[Location::Home(".augment/settings.json")],
    format: Format::Json,
    container: MCP_SERVERS,
    disable: None,
);

const CLINE_CAPS: Caps = ported_caps("cline", TransportSet::ALL, Some(DisableMech::DisabledTrue));
static CLINE_MAPPER: HttpSpelling = HttpSpelling {
    inner: unified(&CLINE_CAPS),
    http: "streamableHttp",
};
pub static CLINE: AdapterSpec = AdapterSpec {
    id: "cline",
    display_name: "Cline",
    tier: Tier::D,
    detect: &[
        Probe::Path(Location::Home(".cline")),
        Probe::Executable("cline"),
    ],
    mcp: Some(user_only(
        &[Location::Home(
            ".cline/data/settings/cline_mcp_settings.json",
        )],
        None,
        Format::Json,
        MCP_SERVERS,
        &CLINE_MAPPER,
        CLINE_CAPS,
    )),
};

const ROO_CODE_CAPS: Caps = ported_caps(
    "roo-code",
    TransportSet::ALL,
    Some(DisableMech::DisabledTrue),
);
static ROO_CODE_MAPPER: HttpSpelling = HttpSpelling {
    inner: unified(&ROO_CODE_CAPS),
    http: "streamable-http",
};
pub static ROO_CODE: AdapterSpec = AdapterSpec {
    id: "roo-code",
    display_name: "Roo Code",
    tier: Tier::D,
    detect: &[
        Probe::Path(Location::AppData(
            "Code/User/globalStorage/RooVeterinaryInc.roo-cline",
        )),
        Probe::Path(Location::Glob {
            base: &Location::Home(".vscode/extensions"),
            pattern: "rooveterinaryinc.roo-cline-*",
        }),
    ],
    mcp: Some(user_only(
        &[Location::AppData(
            "Code/User/globalStorage/RooVeterinaryInc.roo-cline/settings/mcp_settings.json",
        )],
        None,
        Format::Json,
        MCP_SERVERS,
        &ROO_CODE_MAPPER,
        ROO_CODE_CAPS,
    )),
};

const KILO_CODE_CAPS: Caps = ported_caps(
    "kilo-code",
    TransportSet::ALL,
    Some(DisableMech::DisabledTrue),
);
static KILO_CODE_MAPPER: HttpSpelling = HttpSpelling {
    inner: unified(&KILO_CODE_CAPS),
    http: "streamable-http",
};
pub static KILO_CODE: AdapterSpec = AdapterSpec {
    id: "kilo-code",
    display_name: "Kilo Code",
    tier: Tier::D,
    detect: &[
        Probe::Path(Location::AppData(
            "Code/User/globalStorage/kilocode.kilo-code",
        )),
        Probe::Path(Location::Glob {
            base: &Location::Home(".vscode/extensions"),
            pattern: "kilocode.kilo-code-*",
        }),
    ],
    mcp: Some(user_only(
        &[Location::AppData(
            "Code/User/globalStorage/kilocode.kilo-code/settings/mcp_settings.json",
        )],
        None,
        Format::Json,
        MCP_SERVERS,
        &KILO_CODE_MAPPER,
        KILO_CODE_CAPS,
    )),
};

unified_client!(
    KIRO, KIRO_CAPS, KIRO_MAPPER, "kiro", "Kiro",
    detect: &[Probe::Path(Location::Home(".kiro")), Probe::Executable("kiro")],
    user: &[Location::Home(".kiro/settings/mcp.json")],
    format: Format::Json,
    container: MCP_SERVERS,
    disable: Some(DisableMech::DisabledTrue),
);

unified_client!(
    JUNIE, JUNIE_CAPS, JUNIE_MAPPER, "junie", "Junie",
    detect: &[Probe::Path(Location::Home(".junie")), Probe::Executable("junie")],
    user: &[Location::Home(".junie/mcp/mcp.json")],
    format: Format::Json,
    container: MCP_SERVERS,
    disable: None,
);

unified_client!(
    DROID, DROID_CAPS, DROID_MAPPER, "droid", "Factory Droid",
    detect: &[Probe::Path(Location::Home(".factory")), Probe::Executable("droid")],
    user: &[Location::Home(".factory/mcp.json")],
    format: Format::Json,
    container: MCP_SERVERS,
    disable: None,
);

unified_client!(
    COMMANDCODE, COMMANDCODE_CAPS, COMMANDCODE_MAPPER, "commandcode", "Command Code",
    detect: &[Probe::Path(Location::Home(".commandcode"))],
    user: &[Location::Home(".commandcode/mcp.json")],
    format: Format::Json,
    container: MCP_SERVERS,
    disable: None,
);

unified_client!(
    CRUSH, CRUSH_CAPS, CRUSH_MAPPER, "crush", "Crush",
    detect: &[Probe::Path(Location::XdgConfigAllOs("crush")), Probe::Executable("crush")],
    user: &[Location::XdgConfigAllOs("crush/crush.json")],
    format: Format::Json,
    container: ContainerPath(&["mcp"]),
    disable: None,
);

unified_client!(
    QODER, QODER_CAPS, QODER_MAPPER, "qoder", "Qoder",
    detect: &[
        Probe::Path(Location::Home(".config/Qoder")),
        Probe::Path(Location::Home(".qoder")),
        Probe::Executable("qoder"),
    ],
    user: &[Location::Home(".config/Qoder/SharedClientCache/mcp.json")],
    format: Format::Json,
    container: MCP_SERVERS,
    disable: None,
);

const TRAE_CAPS: Caps = ported_caps("trae", TransportSet::ALL, None);
static TRAE_MAPPER: McpServersJson = unified(&TRAE_CAPS);
pub static TRAE: AdapterSpec = AdapterSpec {
    id: "trae",
    display_name: "Trae",
    tier: Tier::D,
    detect: &[
        Probe::Path(Location::AppData("Trae")),
        Probe::Executable("trae"),
    ],
    mcp: Some(user_only(
        &[Location::AppData("Trae/User/mcp.json")],
        Some(trae_user_files),
        Format::Json,
        MCP_SERVERS,
        &TRAE_MAPPER,
        TRAE_CAPS,
    )),
};

const TRAE_CN_CAPS: Caps = ported_caps("trae-cn", TransportSet::ALL, None);
static TRAE_CN_MAPPER: McpServersJson = unified(&TRAE_CN_CAPS);
pub static TRAE_CN: AdapterSpec = AdapterSpec {
    id: "trae-cn",
    display_name: "Trae CN",
    tier: Tier::D,
    detect: &[Probe::Path(Location::AppData("Trae CN"))],
    mcp: Some(user_only(
        &[Location::AppData("Trae CN/User/mcp.json")],
        Some(trae_cn_user_files),
        Format::Json,
        MCP_SERVERS,
        &TRAE_CN_MAPPER,
        TRAE_CN_CAPS,
    )),
};

unified_client!(
    CODEBUDDY, CODEBUDDY_CAPS, CODEBUDDY_MAPPER, "codebuddy", "CodeBuddy",
    detect: &[Probe::Path(Location::Home(".codebuddy")), Probe::Executable("codebuddy")],
    user: &[
        Location::Home(".codebuddy/.mcp.json"),
        Location::Home(".codebuddy/mcp.json"),
        Location::Home(".codebuddy.json"),
    ],
    format: Format::Json5,
    container: MCP_SERVERS,
    disable: None,
);

unified_client!(
    IFLOW, IFLOW_CAPS, IFLOW_MAPPER, "iflow", "iFlow CLI",
    detect: &[Probe::Path(Location::Home(".iflow")), Probe::Executable("iflow")],
    user: &[
        Location::Home(".iflow/settings.json"),
        Location::Home(".iflow/mcp/config.json"),
    ],
    format: Format::Json,
    container: MCP_SERVERS,
    disable: None,
);

/// Qwen Code is a Gemini CLI fork: Gemini's entry shape and filters.
const QWEN_CODE_CAPS: Caps = Caps {
    env_ref: Some(EnvRefSyntax::DollarBrace { default: false }),
    env_ref_fields: FieldSet::COMMAND
        .union(FieldSet::ARGS)
        .union(FieldSet::ENV)
        .union(FieldSet::CWD)
        .union(FieldSet::URL)
        .union(FieldSet::HEADERS),
    tool_filter: Some(ToolFilterMech::AllowDeny {
        allow: "includeTools",
        deny: "excludeTools",
    }),
    timeouts: &[TimeoutField {
        key: "timeout",
        unit: TimeUnit::Milliseconds,
    }],
    ..ported_caps("qwen-code", TransportSet::ALL, None)
};
static QWEN_CODE_MAPPER: gemini::GeminiJson = gemini::GeminiJson;
pub static QWEN_CODE: AdapterSpec = AdapterSpec {
    id: "qwen-code",
    display_name: "Qwen Code",
    tier: Tier::D,
    detect: &[
        Probe::Path(Location::Home(".qwen")),
        Probe::Executable("qwen"),
    ],
    mcp: Some(user_only(
        &[Location::Home(".qwen/settings.json")],
        None,
        Format::Json,
        MCP_SERVERS,
        &QWEN_CODE_MAPPER,
        QWEN_CODE_CAPS,
    )),
};

unified_client!(
    ZENCODER, ZENCODER_CAPS, ZENCODER_MAPPER, "zencoder", "Zencoder",
    detect: &[Probe::Path(Location::Home(".zencoder"))],
    user: &[Location::Home(".zencoder/settings.json")],
    format: Format::Json,
    container: ContainerPath(&["zencoder", "mcpServers"]),
    disable: None,
);

const OPENCLAW_CAPS: Caps = ported_caps(
    "openclaw",
    TransportSet::ALL,
    Some(DisableMech::EnabledFalse),
);
static OPENCLAW_MAPPER: opencode::OpenCodeJson = opencode::OpenCodeJson;
pub static OPENCLAW: AdapterSpec = AdapterSpec {
    id: "openclaw",
    display_name: "OpenClaw",
    tier: Tier::D,
    detect: &[
        Probe::Path(Location::Home(".openclaw")),
        Probe::Executable("openclaw"),
    ],
    mcp: Some(user_only(
        &[Location::Home(".openclaw/openclaw.json")],
        None,
        Format::Json5,
        ContainerPath(&["mcp", "servers"]),
        &OPENCLAW_MAPPER,
        OPENCLAW_CAPS,
    )),
};

/// MiMo Code is an OpenCode fork: OpenCode's entry shape and `{env:NAME}`.
const MIMO_CODE_CAPS: Caps = Caps {
    env_ref: Some(EnvRefSyntax::BraceEnv),
    env_ref_fields: FieldSet::COMMAND
        .union(FieldSet::ARGS)
        .union(FieldSet::ENV)
        .union(FieldSet::URL)
        .union(FieldSet::HEADERS),
    ..ported_caps(
        "mimo-code",
        TransportSet::ALL,
        Some(DisableMech::EnabledFalse),
    )
};
static MIMO_CODE_MAPPER: opencode::OpenCodeJson = opencode::OpenCodeJson;
pub static MIMO_CODE: AdapterSpec = AdapterSpec {
    id: "mimo-code",
    display_name: "MiMo Code",
    tier: Tier::D,
    detect: &[
        Probe::Path(Location::Home(".config/mimocode")),
        Probe::Path(Location::Home(".mimocode")),
        Probe::Executable("mimo"),
    ],
    mcp: Some(user_only(
        &[Location::Home(".config/mimocode/mimocode.json")],
        None,
        Format::Json,
        ContainerPath(&["mcp"]),
        &MIMO_CODE_MAPPER,
        MIMO_CODE_CAPS,
    )),
};

unified_client!(
    WORKBUDDY, WORKBUDDY_CAPS, WORKBUDDY_MAPPER, "workbuddy", "WorkBuddy",
    detect: &[Probe::Path(Location::Home(".workbuddy"))],
    user: &[Location::Home(".workbuddy/.mcp.json")],
    format: Format::Json,
    container: MCP_SERVERS,
    disable: None,
);

const ZCODE_CAPS: Caps = ported_caps("zcode", TransportSet::ALL, None);
static ZCODE_MAPPER: EnableFlag = EnableFlag(unified(&ZCODE_CAPS));
pub static ZCODE: AdapterSpec = AdapterSpec {
    id: "zcode",
    display_name: "ZCode",
    tier: Tier::D,
    detect: &[Probe::Path(Location::Home(".zcode"))],
    mcp: Some(user_only(
        &[Location::Home(".zcode/cli/config.json")],
        None,
        Format::Json,
        ContainerPath(&["mcp", "servers"]),
        &ZCODE_MAPPER,
        ZCODE_CAPS,
    )),
};

const CONTINUE_CAPS: Caps = ported_caps("continue", TransportSet::ALL, None);
static CONTINUE_MAPPER: ContinueYaml = ContinueYaml {
    caps: &CONTINUE_CAPS,
};
pub static CONTINUE: AdapterSpec = AdapterSpec {
    id: "continue",
    display_name: "Continue",
    tier: Tier::D,
    detect: &[
        Probe::Path(Location::Home(".continue")),
        Probe::Executable("cn"),
    ],
    mcp: Some(user_only(
        &[Location::Home(".continue/config.yaml")],
        None,
        Format::Yaml,
        ContainerPath(&["mcpServers", "[name]"]),
        &CONTINUE_MAPPER,
        CONTINUE_CAPS,
    )),
};

const GOOSE_CAPS: Caps = ported_caps(
    "goose",
    TransportSet::STDIO_HTTP,
    Some(DisableMech::EnabledFalse),
);
static GOOSE_MAPPER: GooseYaml = GooseYaml { caps: &GOOSE_CAPS };
pub static GOOSE: AdapterSpec = AdapterSpec {
    id: "goose",
    display_name: "Goose",
    tier: Tier::D,
    detect: &[
        Probe::Path(Location::XdgConfigAllOs("goose")),
        Probe::Path(Location::AppData("Block/goose")),
        Probe::Executable("goose"),
    ],
    mcp: Some(user_only(
        &[Location::XdgConfigAllOs("goose/config.yaml")],
        Some(goose_user_files),
        Format::Yaml,
        ContainerPath(&["extensions"]),
        &GOOSE_MAPPER,
        GOOSE_CAPS,
    )),
};

const HERMES_CAPS: Caps = ported_caps("hermes", TransportSet::ALL, None);
static HERMES_MAPPER: Renamed = Renamed {
    inner: McpServersJson {
        caps: &HERMES_CAPS,
        type_field: false,
        remote_type: false,
        cwd: false,
        http_aliases: HTTP_ALIASES,
        url_implies_http: true,
        valid_name: non_empty,
    },
    renames: &[("type", "transport")],
};
pub static HERMES: AdapterSpec = AdapterSpec {
    id: "hermes",
    display_name: "Hermes Agent",
    tier: Tier::D,
    detect: &[
        Probe::Path(Location::Home(".hermes")),
        Probe::Executable("hermes"),
    ],
    mcp: Some(user_only(
        &[Location::Home(".hermes/config.yaml")],
        None,
        Format::Yaml,
        ContainerPath(&["mcp_servers"]),
        &HERMES_MAPPER,
        HERMES_CAPS,
    )),
};

/// IntelliJ's AI assistant shares Junie's skills directory and has no MCP
/// file Loadout writes.
pub static INTELLIJ: AdapterSpec = AdapterSpec {
    id: "intellij",
    display_name: "IntelliJ IDEA",
    tier: Tier::D,
    detect: &[
        Probe::Path(Location::AppData("JetBrains")),
        Probe::Executable("idea"),
    ],
    mcp: None,
};

//! Gemini CLI (`gemini`): SPEC 7.3, 7.4 and 7.5.
//!
//! - User scope: `~/.gemini/settings.json`.
//! - Project scope: `<P>/.gemini/settings.json`, loaded by Gemini only in
//!   trusted folders.
//! - Entries: `mcpServers.<name>` without `type`: `{command, args, env,
//!   cwd}` (stdio), `{httpUrl, headers}` (Streamable HTTP) or `{url,
//!   headers}` (SSE: a bare `url` is SSE for Gemini, unlike every other
//!   client). Gemini's newer explicit `type` (`http`, `sse`) next to `url`
//!   is read too, and `{"type": "http", "url": ...}` compares equal to the
//!   `httpUrl` entry Loadout writes. `includeTools`/`excludeTools` are the
//!   tool filter; `timeout` (milliseconds) and `trust` are passthrough
//!   fields.
//! - A project hides a user server by listing its name in the project
//!   file's `mcp.excluded` ([`ProjectDisable::ListMember`] in the project
//!   file, as Gemini has no private project file).
//! - Variables: Gemini expands `${NAME}` and a bare `$NAME` in every
//!   string value of its settings file, with no escape; Loadout writes
//!   `${NAME}` and refuses literal text (and inline secret values) holding
//!   `${` or `$` before a name character (`DollarBrace { bare: true }`).
//!
//! Assumptions left for Tier C verification (`scripts/verify-clients.md`,
//! GEM-*):
//! `[verify]` that `mcp.excluded` in the project file hides a same-named
//! user server (GEM-4), that a project entry merges over the user entry
//! (GEM-5, `ProjectSemantics::DeepMerge` as SPEC 7.4 says), that an entry
//! holding both `httpUrl` and `url` (or a URL key and `command`) runs as
//! Streamable HTTP, then SSE, then stdio (GEM-3), the fields `${NAME}` is
//! expanded in and what a bare `$NAME` in a literal value does (GEM-6),
//! that there is no native disable field (GEM-7), that any non-empty name
//! loads (GEM-8), that Windows starts `npx` without `cmd /c` (GEM-9) and
//! the detection probes (GEM-10). `~/.gemini` alone does not detect
//! Gemini, since Antigravity keeps its files there too.

use crate::adapters::caps::{
    Caps, EnvRefSyntax, FieldSet, ProjectDisable, ProjectSemantics, Source, TimeUnit, TimeoutField,
    ToolFilterMech, TransportSet, TrustGate,
};
use crate::adapters::detect::Probe;
use crate::adapters::location::Location;
use crate::adapters::mapper::{
    Canonical, EntryMapper, Launch, MapError, RenderedServer, ServerDef, WriteCx, carry_over,
    check_name, check_transport, drop_empty, existing_entry, is_tool_key, read_disabled,
    read_tools, shape, split_rest, string_field, string_list, string_map, string_map_field,
    strings, unsupported, write_enabled, write_passthrough, write_tools,
};
use crate::adapters::spec::{AdapterSpec, ContainerPath, McpSpec, Tier, exact_names};
use crate::env::Os;
use crate::formats::{Format, NativeMap, NativeValue};
use crate::model::clients::passthrough_of;
use crate::model::server::{ServerFields, Transport};

pub const ID: &str = "gemini";

const TYPE: &str = "type";
const URL: &str = "url";
const HTTP_URL: &str = "httpUrl";
const INCLUDE_TOOLS: &str = "includeTools";
const EXCLUDE_TOOLS: &str = "excludeTools";

/// The list a project hides user servers with, in the project file.
pub const EXCLUDED: ContainerPath = ContainerPath(&["mcp", "excluded"]);

const CAPS: Caps = Caps {
    transports: TransportSet::ALL,
    env_ref: Some(EnvRefSyntax::DollarBrace {
        default: false,
        bare: true,
    }),
    env_ref_fields: FieldSet::COMMAND
        .union(FieldSet::ARGS)
        .union(FieldSet::ENV)
        .union(FieldSet::CWD)
        .union(FieldSet::URL)
        .union(FieldSet::HEADERS),
    command_ref: None,
    native_disable: None,
    project_disable: Some(ProjectDisable::ListMember(EXCLUDED)),
    project_semantics: ProjectSemantics::DeepMerge,
    precedence: &[Source::Project, Source::User],
    compat_reads: &[],
    trust_gate: Some(TrustGate::TrustedFolder),
    rewrites_file_at_runtime: false,
    restart_required: false,
    tool_filter: Some(ToolFilterMech::AllowDeny {
        allow: INCLUDE_TOOLS,
        deny: EXCLUDE_TOOLS,
    }),
    windows_shell_shim: false,
    timeouts: &[TimeoutField {
        key: "timeout",
        unit: TimeUnit::Milliseconds,
    }],
    passthrough_allowlist: passthrough_of(ID),
};

/// Keys the Gemini mapper models besides the tool lists and passthrough.
const MODELED_KEYS: [&str; 8] = [
    TYPE, "command", "args", "env", "cwd", URL, HTTP_URL, "headers",
];

fn is_modeled(key: &str) -> bool {
    MODELED_KEYS.contains(&key)
        || is_tool_key(&CAPS, key)
        || CAPS.passthrough_allowlist.contains(&key)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
}

/// The transport Gemini gives an entry without `type`, in the order it
/// checks the keys: `httpUrl`, then `url`, then `command`.
fn inferred(entry: &NativeMap) -> Option<Transport> {
    if entry.contains_key(HTTP_URL) {
        Some(Transport::Http)
    } else if entry.contains_key(URL) {
        Some(Transport::Sse)
    } else if entry.contains_key("command") {
        Some(Transport::Stdio)
    } else {
        None
    }
}

/// The `mcpServers.<name>` entry of Gemini's settings file (see the
/// module docs).
#[derive(Debug)]
pub struct GeminiJson;

impl EntryMapper for GeminiJson {
    fn to_native(
        &self,
        server: &RenderedServer,
        cx: &WriteCx<'_>,
    ) -> Result<NativeValue, MapError> {
        check_name(valid_name, &server.name)?;
        let transport = check_transport(&CAPS, &server.launch)?;
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
                let key = match transport {
                    Transport::Http => HTTP_URL,
                    Transport::Sse | Transport::Stdio => URL,
                };
                entry.insert(key, remote.url.as_str().into());
                if !remote.headers.is_empty() || keep_empty("headers") {
                    entry.insert("headers", string_map(&remote.headers));
                }
            }
        }
        write_enabled(&mut entry, &CAPS, server, existing)?;
        write_tools(&mut entry, &CAPS, &server.tools)?;
        write_passthrough(&mut entry, &CAPS, server)?;
        carry_over(&mut entry, existing, is_modeled);
        Ok(NativeValue::Object(entry))
    }

    fn from_native(&self, key: &str, value: &NativeValue) -> Result<ServerDef, MapError> {
        let entry = value.as_object().ok_or_else(|| MapError::UnexpectedShape {
            field: key.to_owned(),
            expected: "an object",
        })?;
        // SPEC 7.5: an explicit `type` wins; else `httpUrl` is http and a
        // bare `url` SSE.
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
            None => match inferred(entry) {
                Some(transport) => transport,
                None => return shape("command", "set (or `httpUrl` or `url` for a remote server)"),
            },
        };
        let http_url = string_field(entry, HTTP_URL)?;
        let url = string_field(entry, URL)?;
        let url = match transport {
            Transport::Http => http_url.or(url),
            Transport::Sse | Transport::Stdio => url.or(http_url),
        };
        let fields = ServerFields {
            transport: Some(transport),
            command: string_field(entry, "command")?,
            args: string_list(entry, "args")?,
            cwd: string_field(entry, "cwd")?,
            url,
            headers: string_map_field(entry, "headers")?,
            env: string_map_field(entry, "env")?,
        };
        let mut def = ServerDef {
            fields,
            disabled: read_disabled(entry, &CAPS)?,
            tools: read_tools(entry, &CAPS)?,
            ..ServerDef::default()
        };
        split_rest(&mut def, entry, &CAPS, is_modeled);
        Ok(def)
    }

    fn canonical(&self, value: &NativeValue) -> Canonical {
        let Some(entry) = value.as_object() else {
            return Canonical::of(value);
        };
        let mut entry = entry.clone();
        // `{"type": "http", "url": u}` is Loadout's `{"httpUrl": u}`; a
        // `type` naming the transport its keys imply is dropped.
        let kind = entry
            .get(TYPE)
            .and_then(NativeValue::as_str)
            .map(str::to_owned);
        match kind.as_deref() {
            Some("http") if !entry.contains_key(HTTP_URL) && entry.contains_key(URL) => {
                if let Some(url) = entry.remove(URL) {
                    entry.insert(HTTP_URL, url);
                }
                entry.remove(TYPE);
            }
            Some(kind) if inferred(&entry).map(Transport::as_str) == Some(kind) => {
                entry.remove(TYPE);
            }
            _ => {}
        }
        // Gemini runs an untyped entry with `httpUrl` over Streamable HTTP
        // and ignores its `url` (`[verify]` GEM-3).
        if entry.contains_key(HTTP_URL) && !entry.contains_key(TYPE) {
            entry.remove(URL);
        }
        // An empty `includeTools` allows no tool, so it is not a default.
        drop_empty(&mut entry, &["args", "env", "headers", EXCLUDE_TOOLS]);
        Canonical::of(&NativeValue::Object(entry))
    }
}

static MAPPER: GeminiJson = GeminiJson;

pub static SPEC: AdapterSpec = AdapterSpec {
    id: ID,
    display_name: "Gemini CLI",
    tier: Tier::C,
    detect: &[
        Probe::Path(Location::Home(".gemini/settings.json")),
        Probe::Executable("gemini"),
    ],
    mcp: Some(McpSpec {
        oses: &[Os::Windows, Os::Linux, Os::MacOs],
        read_only: false,
        user: &[Location::Home(".gemini/settings.json")],
        user_files: None,
        user_mirrors: &[],
        profiles: None,
        project: &[Location::Project(".gemini/settings.json")],
        project_local: &[],
        format: Format::Json,
        skeleton: "",
        container: ContainerPath(&["mcpServers"]),
        project_local_container: ContainerPath(&["mcpServers"]),
        legacy_containers: &[],
        same_name: exact_names,
        replaced_by: &[],
        mapper: &MAPPER,
        caps: CAPS,
    }),
};

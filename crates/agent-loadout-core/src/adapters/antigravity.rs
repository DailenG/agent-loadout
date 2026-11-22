//! Antigravity (`antigravity`): SPEC 7.3, 7.4 and 7.5.
//!
//! - User scope: `~/.gemini/config/mcp_config.json`.
//! - Project scope: `<P>/.agents/mcp_config.json`.
//! - Entries: `mcpServers.<name>` without `type`: `{command, args, env}`
//!   (stdio) or `{serverUrl, headers}` (Streamable HTTP; `serverUrl` means
//!   http, SPEC 7.5). No SSE, no `httpUrl`, no `cwd`. `timeoutSeconds` is a
//!   passthrough field.
//! - No variable expansion is assumed: values are written literally, so
//!   `env` delivery is not available and secrets use another mode.
//!
//! Assumptions left for Tier C verification (`scripts/verify-clients.md`,
//! ANT-*): `[verify]` the user file (ANT-1, SPEC 7.3 names
//! `~/.gemini/config/`; record whether the app reads
//! `~/.gemini/antigravity/` instead) and the project file (ANT-2), whether
//! any variable syntax is expanded (ANT-3, `env_ref = None` until then),
//! that there is no native disable field (ANT-4: an existing `disabled` or
//! `disabledTools` key is kept as an unmapped field), how a project entry
//! combines with a same-named user entry and whether a project can hide one
//! (ANT-5, `ProjectSemantics::Unknown`, `cannot_suppress_in_project`), any
//! trust prompt (ANT-6), that a `url` key is not read (ANT-7), that any
//! non-empty name loads (ANT-8), that Windows starts `npx` without `cmd /c`
//! (ANT-9) and the detection probes (ANT-10).

use crate::adapters::caps::{
    Caps, FieldSet, ProjectSemantics, Source, TimeUnit, TimeoutField, TransportSet,
};
use crate::adapters::detect::Probe;
use crate::adapters::location::Location;
use crate::adapters::mapper::{
    Canonical, EntryMapper, Launch, MapError, RenderedServer, ServerDef, WriteCx, carry_over,
    check_name, check_transport, drop_empty, existing_entry, read_disabled, read_tools, shape,
    split_rest, string_field, string_list, string_map, string_map_field, strings, unsupported,
    write_enabled, write_passthrough, write_tools,
};
use crate::adapters::spec::{AdapterSpec, ContainerPath, McpSpec, Tier, exact_names};
use crate::env::Os;
use crate::formats::{Format, NativeMap, NativeValue};
use crate::model::clients::passthrough_of;
use crate::model::server::{ServerFields, Transport};

pub const ID: &str = "antigravity";

const SERVER_URL: &str = "serverUrl";

const CAPS: Caps = Caps {
    transports: TransportSet::STDIO_HTTP,
    env_ref: None,
    env_ref_fields: FieldSet::NONE,
    command_ref: None,
    native_disable: None,
    project_disable: None,
    project_semantics: ProjectSemantics::Unknown,
    precedence: &[Source::Project, Source::User],
    compat_reads: &[],
    trust_gate: None,
    rewrites_file_at_runtime: false,
    restart_required: false,
    tool_filter: None,
    windows_shell_shim: false,
    timeouts: &[TimeoutField {
        key: "timeoutSeconds",
        unit: TimeUnit::Seconds,
    }],
    passthrough_allowlist: passthrough_of(ID),
};

/// Keys the Antigravity mapper models besides passthrough fields.
const MODELED_KEYS: [&str; 5] = ["command", "args", "env", SERVER_URL, "headers"];

fn is_modeled(key: &str) -> bool {
    MODELED_KEYS.contains(&key) || CAPS.passthrough_allowlist.contains(&key)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
}

/// The `mcpServers.<name>` entry of Antigravity's `mcp_config.json`:
/// `{command, args, env}` or `{serverUrl, headers}`.
#[derive(Debug)]
pub struct AntigravityJson;

impl EntryMapper for AntigravityJson {
    fn to_native(
        &self,
        server: &RenderedServer,
        cx: &WriteCx<'_>,
    ) -> Result<NativeValue, MapError> {
        check_name(valid_name, &server.name)?;
        check_transport(&CAPS, &server.launch)?;
        let existing = existing_entry(cx);
        let keep_empty = |key: &str| existing.is_some_and(|entry| entry.contains_key(key));
        let mut entry = NativeMap::new();
        match &server.launch {
            Launch::Stdio(stdio) => {
                if !stdio.forward_env.is_empty() {
                    return unsupported("env_vars");
                }
                if stdio.cwd.is_some() {
                    return unsupported("cwd");
                }
                entry.insert("command", stdio.command.as_str().into());
                entry.insert("args", strings(stdio.args.clone()));
                if !stdio.env.is_empty() || keep_empty("env") {
                    entry.insert("env", string_map(&stdio.env));
                }
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
                entry.insert(SERVER_URL, remote.url.as_str().into());
                if !remote.headers.is_empty() || keep_empty("headers") {
                    entry.insert("headers", string_map(&remote.headers));
                }
            }
            Launch::Sse(_) => unreachable!("check_transport rejects SSE for Antigravity"),
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
        let fields = match (
            entry.contains_key("command"),
            entry.contains_key(SERVER_URL),
        ) {
            (true, true) => return shape(SERVER_URL, "absent when `command` is set"),
            (true, false) => ServerFields {
                transport: Some(Transport::Stdio),
                command: string_field(entry, "command")?,
                args: string_list(entry, "args")?,
                env: string_map_field(entry, "env")?,
                ..ServerFields::default()
            },
            (false, true) => ServerFields {
                transport: Some(Transport::Http),
                url: string_field(entry, SERVER_URL)?,
                headers: string_map_field(entry, "headers")?,
                ..ServerFields::default()
            },
            (false, false) => {
                return shape("command", "set (or `serverUrl` for a remote server)");
            }
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
        drop_empty(&mut entry, &["args", "env", "headers"]);
        Canonical::of(&NativeValue::Object(entry))
    }
}

static MAPPER: AntigravityJson = AntigravityJson;

pub static SPEC: AdapterSpec = AdapterSpec {
    id: ID,
    display_name: "Antigravity",
    tier: Tier::C,
    detect: &[
        Probe::Path(Location::Home(".gemini/config")),
        Probe::Path(Location::Home(".antigravity")),
        Probe::Path(Location::AppData("Antigravity")),
        Probe::Executable("antigravity"),
    ],
    mcp: Some(McpSpec {
        oses: &[Os::Windows, Os::Linux, Os::MacOs],
        read_only: false,
        user: &[Location::Home(".gemini/config/mcp_config.json")],
        user_files: None,
        user_mirrors: &[],
        profiles: None,
        project: &[Location::Project(".agents/mcp_config.json")],
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

//! OpenCode (`opencode`): SPEC 7.3, 7.4 and 7.5.
//!
//! - User scope: `$XDG_CONFIG_HOME/opencode/opencode.json` (default
//!   `~/.config`, on Windows too), or `opencode.jsonc` there when only that
//!   one exists. Both are JSONC; a new file starts with OpenCode's
//!   `$schema` line.
//! - Project scope: `<P>/opencode.json[c]`, else an existing
//!   `<P>/.opencode/opencode.json[c]`.
//! - Entries: `mcp.<name>`: `{type: "local", command: [cmd, ...args],
//!   environment, enabled}` or `{type: "remote", url, headers, enabled}`.
//!   `command` is one array holding the command and its arguments. A
//!   remote entry reads as http unless its URL path ends in `/sse` (SPEC
//!   7.5); an SSE server is written as a remote entry too, since OpenCode
//!   tries Streamable HTTP first and falls back to SSE. No `cwd`, no
//!   per-server tool filter.
//! - `enabled: false` disables an entry; a project hides a user server
//!   with the whole entry and `enabled: false` (OpenCode merges project
//!   over user, SPEC 7.4).
//! - Variables: OpenCode substitutes `{env:NAME}` anywhere in the file
//!   before parsing it; Loadout writes `{env:NAME}`.
//!
//! Assumptions left for Tier C verification (`scripts/verify-clients.md`,
//! OC-*): `[verify]` the user and project files (OC-1, OC-2, including
//! which file wins when `opencode.json` and `opencode.jsonc` both exist),
//! the remote fallback to SSE (OC-3), that a project entry with `enabled:
//! false` hides the user entry (OC-4; Loadout writes the whole entry, as
//! OpenCode validates each file on its own and a partial entry has no
//! `type`), the deep merge of project over user entries (OC-5), that an
//! entry without `type` is rejected (OC-6), that `{env:NAME}` is expanded
//! in every field (OC-7), that any non-empty name loads (OC-8), that
//! Windows starts `npx` without `cmd /c` (OC-9) and the detection probes
//! (OC-10).

use crate::adapters::caps::{
    Caps, DisableMech, EnvRefSyntax, FieldSet, ProjectDisable, ProjectSemantics, Source,
    TransportSet,
};
use crate::adapters::detect::Probe;
use crate::adapters::location::Location;
use crate::adapters::mapper::{
    Canonical, EntryMapper, Launch, MapError, RenderedServer, ServerDef, WriteCx, carry_over,
    check_name, check_transport, drop_default_enabled, drop_empty, existing_entry, read_disabled,
    read_tools, shape, split_rest, string_field, string_map, string_map_field, unsupported,
    write_enabled, write_passthrough, write_tools,
};
use crate::adapters::spec::{AdapterSpec, ContainerPath, McpSpec, Tier, exact_names};
use crate::env::Os;
use crate::formats::{Format, NativeMap, NativeValue};
use crate::model::clients::passthrough_of;
use crate::model::server::{ServerFields, Transport};

pub const ID: &str = "opencode";

const TYPE: &str = "type";
const LOCAL: &str = "local";
const REMOTE: &str = "remote";
const COMMAND: &str = "command";
const ENVIRONMENT: &str = "environment";
const ENABLED: &str = "enabled";

/// The text of a new OpenCode config file.
pub const SKELETON: &str = "{\n  \"$schema\": \"https://opencode.ai/config.json\"\n}\n";

const CAPS: Caps = Caps {
    transports: TransportSet::ALL,
    env_ref: Some(EnvRefSyntax::BraceEnv),
    env_ref_fields: FieldSet::COMMAND
        .union(FieldSet::ARGS)
        .union(FieldSet::ENV)
        .union(FieldSet::URL)
        .union(FieldSet::HEADERS),
    command_ref: None,
    native_disable: Some(DisableMech::EnabledFalse),
    project_disable: Some(ProjectDisable::EnabledFalse { partial: false }),
    project_semantics: ProjectSemantics::DeepMerge,
    precedence: &[Source::Project, Source::User],
    compat_reads: &[],
    trust_gate: None,
    rewrites_file_at_runtime: false,
    restart_required: false,
    tool_filter: None,
    windows_shell_shim: false,
    timeouts: &[],
    passthrough_allowlist: passthrough_of(ID),
};

/// Keys the OpenCode mapper models besides passthrough fields.
const MODELED_KEYS: [&str; 6] = [TYPE, COMMAND, ENVIRONMENT, "url", "headers", ENABLED];

fn is_modeled(key: &str) -> bool {
    MODELED_KEYS.contains(&key) || CAPS.passthrough_allowlist.contains(&key)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
}

/// Whether the path of `url` ends in `/sse` (query and fragment ignored,
/// trailing slashes too), which makes a remote entry SSE (SPEC 7.5).
pub fn is_sse_url(url: &str) -> bool {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    let path = url[..end].trim_end_matches('/');
    let path = path.split_once("://").map_or(path, |(_, rest)| {
        rest.find('/').map_or("", |at| &rest[at..])
    });
    path.ends_with("/sse")
}

/// The type OpenCode needs for an entry missing it, from its keys.
fn inferred_type(entry: &NativeMap) -> Option<&'static str> {
    if entry.contains_key(COMMAND) {
        Some(LOCAL)
    } else if entry.contains_key("url") {
        Some(REMOTE)
    } else {
        None
    }
}

/// The `mcp.<name>` entry of an OpenCode config file (see the module
/// docs).
#[derive(Debug)]
pub struct OpenCodeJson;

impl EntryMapper for OpenCodeJson {
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
                entry.insert(TYPE, LOCAL.into());
                let command = std::iter::once(&stdio.command)
                    .chain(&stdio.args)
                    .map(|part| NativeValue::from(part.as_str()))
                    .collect();
                entry.insert(COMMAND, NativeValue::Array(command));
                if !stdio.env.is_empty() || keep_empty(ENVIRONMENT) {
                    entry.insert(ENVIRONMENT, string_map(&stdio.env));
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
                entry.insert(TYPE, REMOTE.into());
                entry.insert("url", remote.url.as_str().into());
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
        let kind = match entry.get(TYPE) {
            Some(NativeValue::String(kind)) => kind.as_str(),
            Some(_) => return shape(TYPE, "a string"),
            None => match inferred_type(entry) {
                Some(kind) => kind,
                None => return shape(TYPE, "`local` or `remote`"),
            },
        };
        let fields = match kind {
            LOCAL => {
                let (command, args) = command_array(entry)?;
                ServerFields {
                    transport: Some(Transport::Stdio),
                    command: Some(command),
                    args: Some(args),
                    env: string_map_field(entry, ENVIRONMENT)?,
                    ..ServerFields::default()
                }
            }
            REMOTE => {
                let url = string_field(entry, "url")?;
                let transport = match &url {
                    Some(url) if is_sse_url(url) => Transport::Sse,
                    _ => Transport::Http,
                };
                ServerFields {
                    transport: Some(transport),
                    url,
                    headers: string_map_field(entry, "headers")?,
                    ..ServerFields::default()
                }
            }
            other => {
                return Err(MapError::UnsupportedTransport {
                    transport: other.to_owned(),
                });
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
        drop_empty(&mut entry, &[ENVIRONMENT, "headers"]);
        drop_default_enabled(&mut entry, &CAPS);
        Canonical::of(&NativeValue::Object(entry))
    }

    fn skipped_by_client(&self, value: &NativeValue) -> Option<&'static str> {
        let entry = value.as_object()?;
        (!entry.contains_key(TYPE) && inferred_type(entry).is_some())
            .then_some("has no `type` (`local` or `remote`), so the client rejects it")
    }
}

/// `command`: a non-empty array of strings, the command then its
/// arguments.
fn command_array(entry: &NativeMap) -> Result<(String, Vec<String>), MapError> {
    const EXPECTED: &str = "a non-empty array of strings (the command, then its arguments)";
    let parts = entry
        .get(COMMAND)
        .and_then(NativeValue::as_array)
        .and_then(|items| {
            items
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect::<Option<Vec<String>>>()
        });
    match parts {
        Some(mut parts) if !parts.is_empty() => {
            let command = parts.remove(0);
            Ok((command, parts))
        }
        _ => shape(COMMAND, EXPECTED),
    }
}

static MAPPER: OpenCodeJson = OpenCodeJson;

pub static SPEC: AdapterSpec = AdapterSpec {
    id: ID,
    display_name: "OpenCode",
    tier: Tier::C,
    detect: &[
        Probe::Path(Location::XdgConfigAllOs("opencode")),
        Probe::Executable("opencode"),
    ],
    mcp: Some(McpSpec {
        oses: &[Os::Windows, Os::Linux, Os::MacOs],
        read_only: false,
        user: &[
            Location::XdgConfigAllOs("opencode/opencode.json"),
            Location::XdgConfigAllOs("opencode/opencode.jsonc"),
        ],
        user_files: None,
        user_mirrors: &[],
        profiles: None,
        project: &[
            Location::Project("opencode.json"),
            Location::Project("opencode.jsonc"),
            Location::Project(".opencode/opencode.json"),
            Location::Project(".opencode/opencode.jsonc"),
        ],
        project_local: &[],
        format: Format::Jsonc,
        skeleton: SKELETON,
        container: ContainerPath(&["mcp"]),
        project_local_container: ContainerPath(&["mcp"]),
        legacy_containers: &[],
        same_name: exact_names,
        replaced_by: &[],
        mapper: &MAPPER,
        caps: CAPS,
    }),
};

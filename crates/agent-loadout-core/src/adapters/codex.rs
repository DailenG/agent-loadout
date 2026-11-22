//! Codex (`codex`): SPEC 7.3 and 7.4.
//!
//! - User scope: `$CODEX_HOME/config.toml` when `CODEX_HOME` is an absolute
//!   path, else `~/.codex/config.toml`.
//! - Project scope: `<P>/.codex/config.toml`, loaded by Codex only for
//!   trusted projects.
//! - Entries: `[mcp_servers.<name>]`. The legacy `[mcp.servers.<name>]`
//!   table is read (import) and never edited or deleted.
//! - Codex expands no `${...}` references: `env` delivery forwards a parent
//!   variable by name (`env_vars`, `bearer_token_env_var`,
//!   `env_http_headers`). No SSE.

use std::collections::BTreeMap;

use crate::adapters::caps::{
    Caps, DisableMech, EnvRefSyntax, FieldSet, ProjectDisable, ProjectSemantics, Source, TimeUnit,
    TimeoutField, ToolFilterMech, TransportSet, TrustGate,
};
use crate::adapters::detect::Probe;
use crate::adapters::location::Location;
use crate::adapters::mapper::{
    Canonical, EntryMapper, Launch, MapError, RenderedServer, ServerDef, WriteCx, carry_over,
    check_name, check_transport, dotted_word_name, drop_default_enabled, drop_empty,
    existing_entry, is_tool_key, read_disabled, read_tools, shape, split_rest, string_field,
    string_list, string_map, string_map_field, strings, unsupported, write_enabled,
    write_passthrough, write_tools,
};
use crate::adapters::spec::{AdapterSpec, ContainerPath, McpSpec, Tier, exact_names};
use crate::env::Os;
use crate::formats::{Format, NativeMap, NativeValue};
use crate::model::clients::passthrough_of;
use crate::model::server::{ServerFields, Transport};

pub const ID: &str = "codex";

/// `~/.codex`, or `$CODEX_HOME`.
const CODEX_DIR: Location = Location::EnvDir {
    var: "CODEX_HOME",
    fallback: &Location::Home(".codex"),
    rel: "",
};

/// `~/.codex/config.toml`, or `$CODEX_HOME/config.toml`.
const USER_FILE: Location = Location::EnvDir {
    var: "CODEX_HOME",
    fallback: &Location::Home(".codex"),
    rel: "config.toml",
};

const CAPS: Caps = Caps {
    transports: TransportSet::STDIO_HTTP,
    env_ref: Some(EnvRefSyntax::CodexForward),
    env_ref_fields: FieldSet::ENV.union(FieldSet::HEADERS),
    command_ref: None,
    native_disable: Some(DisableMech::EnabledFalse),
    // `[verify]` that a project `enabled = false` merges over the user
    // entry of the same name.
    project_disable: Some(ProjectDisable::EnabledFalse { partial: true }),
    // `[verify]` deep merge of project over user entries.
    project_semantics: ProjectSemantics::DeepMerge,
    precedence: &[Source::Project, Source::User],
    compat_reads: &[],
    trust_gate: Some(TrustGate::TrustedProject),
    rewrites_file_at_runtime: false,
    restart_required: false,
    tool_filter: Some(ToolFilterMech::AllowDeny {
        allow: ENABLED_TOOLS,
        deny: DISABLED_TOOLS,
    }),
    // `[verify]` whether Codex on Windows starts `npx` without `cmd /c`.
    windows_shell_shim: false,
    timeouts: &[
        TimeoutField {
            key: "startup_timeout_sec",
            unit: TimeUnit::Seconds,
        },
        TimeoutField {
            key: "tool_timeout_sec",
            unit: TimeUnit::Seconds,
        },
    ],
    passthrough_allowlist: passthrough_of(ID),
};

const ENABLED_TOOLS: &str = "enabled_tools";
const DISABLED_TOOLS: &str = "disabled_tools";
const ENV_VARS: &str = "env_vars";
const BEARER: &str = "bearer_token_env_var";
const HTTP_HEADERS: &str = "http_headers";
const ENV_HTTP_HEADERS: &str = "env_http_headers";

/// Keys the Codex mapper models besides the tool lists and passthrough.
const MODELED_KEYS: [&str; 10] = [
    "command",
    "args",
    "env",
    ENV_VARS,
    "cwd",
    "url",
    BEARER,
    HTTP_HEADERS,
    ENV_HTTP_HEADERS,
    "enabled",
];

/// The `[mcp_servers.<name>]` table: `command, args, env, env_vars, cwd` or
/// `url, bearer_token_env_var, http_headers, env_http_headers`, plus
/// `enabled`, `enabled_tools`, `disabled_tools` and the timeouts. Names
/// are `[A-Za-z0-9_.-]+` (gate V1, CX-6).
#[derive(Debug)]
pub struct CodexToml;

fn is_modeled(key: &str) -> bool {
    MODELED_KEYS.contains(&key)
        || is_tool_key(&CAPS, key)
        || CAPS.passthrough_allowlist.contains(&key)
}

impl EntryMapper for CodexToml {
    fn to_native(
        &self,
        server: &RenderedServer,
        cx: &WriteCx<'_>,
    ) -> Result<NativeValue, MapError> {
        check_name(dotted_word_name, &server.name)?;
        check_transport(&CAPS, &server.launch)?;
        let existing = existing_entry(cx);
        let keep_empty = |key: &str| existing.is_some_and(|entry| entry.contains_key(key));
        let mut entry = NativeMap::new();
        match &server.launch {
            Launch::Stdio(stdio) => {
                entry.insert("command", stdio.command.as_str().into());
                entry.insert("args", strings(stdio.args.clone()));
                if !stdio.env.is_empty() || keep_empty("env") {
                    entry.insert("env", string_map(&stdio.env));
                }
                if !stdio.forward_env.is_empty() || keep_empty(ENV_VARS) {
                    entry.insert(ENV_VARS, strings(stdio.forward_env.clone()));
                }
                if let Some(cwd) = &stdio.cwd {
                    entry.insert("cwd", cwd.as_str().into());
                }
            }
            Launch::Http(remote) => {
                if remote.headers_helper.is_some() {
                    return unsupported("headersHelper");
                }
                entry.insert("url", remote.url.as_str().into());
                if let Some(variable) = &remote.bearer_env {
                    entry.insert(BEARER, variable.as_str().into());
                }
                if !remote.headers.is_empty() || keep_empty(HTTP_HEADERS) {
                    entry.insert(HTTP_HEADERS, string_map(&remote.headers));
                }
                if !remote.headers_env.is_empty() || keep_empty(ENV_HTTP_HEADERS) {
                    entry.insert(ENV_HTTP_HEADERS, string_map(&remote.headers_env));
                }
            }
            Launch::Sse(_) => unreachable!("check_transport rejects SSE for Codex"),
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
            expected: "a table",
        })?;
        let fields = match (entry.contains_key("command"), entry.contains_key("url")) {
            (true, true) => return shape("url", "absent when `command` is set"),
            (true, false) => {
                // Forwarded variables read as references; an explicit `env`
                // value for the same name wins, as it is the literal one.
                let mut env = forwarded(env_var_names(entry)?);
                env.extend(string_map_field(entry, "env")?.unwrap_or_default());
                ServerFields {
                    transport: Some(Transport::Stdio),
                    command: string_field(entry, "command")?,
                    args: string_list(entry, "args")?,
                    cwd: string_field(entry, "cwd")?,
                    env: (!env.is_empty()).then_some(env),
                    ..ServerFields::default()
                }
            }
            (false, true) => {
                let mut headers = BTreeMap::new();
                if let Some(variable) = string_field(entry, BEARER)? {
                    headers.insert(
                        "Authorization".to_owned(),
                        format!("Bearer {}", env_ref(&variable)),
                    );
                }
                for (header, variable) in
                    string_map_field(entry, ENV_HTTP_HEADERS)?.unwrap_or_default()
                {
                    headers.insert(header, env_ref(&variable));
                }
                headers.extend(string_map_field(entry, HTTP_HEADERS)?.unwrap_or_default());
                ServerFields {
                    transport: Some(Transport::Http),
                    url: string_field(entry, "url")?,
                    headers: (!headers.is_empty()).then_some(headers),
                    ..ServerFields::default()
                }
            }
            (false, false) => return shape("command", "set (or `url` for a remote server)"),
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
        // An empty `enabled_tools` allows no tool, so it is not a default.
        drop_empty(
            &mut entry,
            &[
                "args",
                "env",
                ENV_VARS,
                HTTP_HEADERS,
                ENV_HTTP_HEADERS,
                DISABLED_TOOLS,
            ],
        );
        drop_default_enabled(&mut entry, &CAPS);
        Canonical::of(&NativeValue::Object(entry))
    }
}

/// `${env:NAME}` (SPEC 11.2).
fn env_ref(name: &str) -> String {
    format!("${{env:{name}}}")
}

/// `NAME = "${env:NAME}"` for each forwarded variable.
fn forwarded(names: Vec<String>) -> BTreeMap<String, String> {
    names
        .into_iter()
        .map(|name| {
            let value = env_ref(&name);
            (name, value)
        })
        .collect()
}

/// The names in `env_vars`: strings, or tables with a `name` and an
/// optional `source = "local"`.
fn env_var_names(entry: &NativeMap) -> Result<Vec<String>, MapError> {
    const EXPECTED: &str = "an array of variable names (or `{ name, source = \"local\" }` tables)";
    let Some(value) = entry.get(ENV_VARS) else {
        return Ok(Vec::new());
    };
    let items = value.as_array().ok_or_else(|| MapError::UnexpectedShape {
        field: ENV_VARS.to_owned(),
        expected: EXPECTED,
    })?;
    items
        .iter()
        .map(|item| match item {
            NativeValue::String(name) => Some(name.clone()),
            NativeValue::Object(table) => {
                let local = table
                    .get("source")
                    .is_none_or(|source| source.as_str() == Some("local"));
                let only_known = table.keys().all(|key| key == "name" || key == "source");
                table
                    .get("name")
                    .and_then(NativeValue::as_str)
                    .filter(|_| local && only_known)
                    .map(str::to_owned)
            }
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| MapError::UnexpectedShape {
            field: ENV_VARS.to_owned(),
            expected: EXPECTED,
        })
}

static MAPPER: CodexToml = CodexToml;

pub static SPEC: AdapterSpec = AdapterSpec {
    id: ID,
    display_name: "Codex",
    tier: Tier::A,
    detect: &[Probe::Path(CODEX_DIR), Probe::Executable("codex")],
    mcp: Some(McpSpec {
        oses: &[Os::Windows, Os::Linux, Os::MacOs],
        user: &[USER_FILE],
        user_mirrors: &[],
        profiles: None,
        project: &[Location::Project(".codex/config.toml")],
        project_local: &[],
        format: Format::Toml,
        skeleton: "",
        container: ContainerPath(&["mcp_servers"]),
        project_local_container: ContainerPath(&["mcp_servers"]),
        legacy_containers: &[ContainerPath(&["mcp", "servers"])],
        same_name: exact_names,
        replaced_by: &[],
        mapper: &MAPPER,
        caps: CAPS,
    }),
};

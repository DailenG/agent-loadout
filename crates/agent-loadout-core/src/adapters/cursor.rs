//! Cursor (`cursor`): SPEC 7.3 and 7.4.
//!
//! - User scope: `~/.cursor/mcp.json` (every OS).
//! - Project scope: `<P>/.cursor/mcp.json`.
//! - Entries: `mcpServers.<name>` without `type`: `{command, args, env,
//!   envFile}` (stdio) or `{url, headers}` (a bare `url` is Streamable
//!   HTTP). SSE entries carry `"type": "sse"`. `envFile` is a passthrough
//!   field (`[client.cursor] envFile`).
//! - Variables: Cursor expands `${env:NAME}` (and its own `${userHome}`,
//!   `${workspaceFolder}`) in `command`, `args`, `env`, `url` and
//!   `headers`; Loadout writes `${env:NAME}` for `env` delivery.
//! - Project servers: Cursor lists each new server of a project file as
//!   Disabled until it is enabled once in Customize > MCPs (gate V2 CUR-2);
//!   that toggle is Cursor's own state, so plans and `effective` carry a
//!   `client_enable_required` hint ([`TrustGate::ClientEnable`]).
//! - Same-named entries: a user and a project entry both run, as separate
//!   servers listed per source (gate V2 CUR-5, FB-17), so the layers are
//!   [`ProjectSemantics::Coexist`]: `effective` lists one loaded row per
//!   file with `duplicate_across_layers`, and a project cannot hide or
//!   replace a user server (`cannot_suppress_in_project`).
//!
//! Verified at gate V2 (Windows Sandbox, Cursor 3.31; `docs/verify/cursor.md`):
//! `"disabled": true` is ignored and the UI toggle lives in Cursor's own
//! storage, so there is no native disable field (CUR-4); a project entry
//! cannot hide a user server, and a same-named user and project entry both
//! run as separate servers (CUR-5); `${env:NAME}` expands in command, args,
//! env, url and headers (CUR-3); SSE with `"type": "sse"`, bare-url SSE and
//! bare-url Streamable HTTP all load (CUR-6); dotted and spaced names load
//! (CUR-7); plain `npx` starts without `cmd /c` (CUR-8); the per-user
//! installer's uninstall entry is `Cursor (User)` (CUR-9).

use crate::adapters::caps::{
    Caps, EnvRefSyntax, FieldSet, ProjectSemantics, Source, TransportSet, TrustGate,
};
use crate::adapters::detect::Probe;
use crate::adapters::location::Location;
use crate::adapters::mapper::McpServersJson;
use crate::adapters::spec::{AdapterSpec, ContainerPath, McpSpec, Tier, exact_names};
use crate::env::Os;
use crate::formats::Format;
use crate::model::clients::passthrough_of;

pub const ID: &str = "cursor";

const CAPS: Caps = Caps {
    transports: TransportSet::ALL,
    env_ref: Some(EnvRefSyntax::DollarBraceEnv),
    env_ref_fields: FieldSet::COMMAND
        .union(FieldSet::ARGS)
        .union(FieldSet::ENV)
        .union(FieldSet::URL)
        .union(FieldSet::HEADERS),
    command_ref: None,
    native_disable: None,
    project_disable: None,
    project_semantics: ProjectSemantics::Coexist,
    // Only the order `effective` lists same-named entries in.
    precedence: &[Source::Project, Source::User],
    compat_reads: &[],
    trust_gate: Some(TrustGate::ClientEnable("Cursor > Customize > MCPs")),
    rewrites_file_at_runtime: false,
    restart_required: false,
    tool_filter: None,
    windows_shell_shim: false,
    timeouts: &[],
    passthrough_allowlist: passthrough_of(ID),
};

static MAPPER: McpServersJson = McpServersJson {
    caps: &CAPS,
    type_field: false,
    remote_type: false,
    cwd: false,
    http_aliases: &[],
    url_implies_http: true,
    valid_name: |name| !name.is_empty(),
};

pub static SPEC: AdapterSpec = AdapterSpec {
    id: ID,
    display_name: "Cursor",
    tier: Tier::B,
    detect: &[
        Probe::Path(Location::Home(".cursor")),
        Probe::Path(Location::AppData("Cursor")),
        Probe::Executable("cursor"),
        // The uninstall `DisplayName`: "Cursor (User)" for the per-user
        // installer (gate V2 CUR-9), "Cursor" for a machine-wide install.
        Probe::UninstallEntry("Cursor"),
        Probe::UninstallEntry("Cursor (User)"),
    ],
    mcp: Some(McpSpec {
        oses: &[Os::Windows, Os::Linux, Os::MacOs],
        read_only: false,
        user: &[Location::Home(".cursor/mcp.json")],
        user_files: None,
        user_mirrors: &[],
        profiles: None,
        project: &[Location::Project(".cursor/mcp.json")],
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

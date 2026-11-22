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
//!
//! Assumptions left for gate V2 (`scripts/verify-clients.md`, CUR-*):
//! `[verify]` that there is no native disable field (CUR-4) and no way to
//! hide a user server in a project (CUR-5), that a project entry replaces
//! a same-named user entry (CUR-5), the fields Cursor expands `${env:NAME}`
//! in (CUR-3), that `"type": "sse"` is accepted for SSE (CUR-6), that any
//! non-empty name loads (CUR-7), that Windows starts `npx` without `cmd /c`
//! (CUR-8) and the detection probes (CUR-9).

use crate::adapters::caps::{Caps, EnvRefSyntax, FieldSet, ProjectSemantics, Source, TransportSet};
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
    project_semantics: ProjectSemantics::Replace,
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
        // `[verify]` the uninstall `DisplayName` of the Windows installer.
        Probe::UninstallEntry("Cursor"),
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

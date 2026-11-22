//! Windsurf, legacy Cascade (`windsurf`): SPEC 7.3 and 7.4. Best effort;
//! Devin Desktop (`devin`) is its successor.
//!
//! - User scope: `~/.codeium/windsurf/mcp_config.json` (every OS).
//! - Project scope: `<P>/.windsurf/mcp_config.json`.
//! - Entries: `mcpServers.<name>` without `type`: `{command, args, env}`
//!   (stdio) or `{serverUrl, headers}` (Streamable HTTP). SSE entries carry
//!   `"type": "sse"`. An entry with `url` instead of `serverUrl` reads the
//!   same.
//! - `disabled: true` disables an entry.
//! - Variables: Windsurf expands `${env:NAME}`; Loadout writes it for `env`
//!   delivery.
//!
//! Assumptions left for Tier D verification (`scripts/verify-clients.md`,
//! WSF-*): `[verify]` the project file (WSF-2), `serverUrl` rather than
//! `url` for remote servers and `"type": "sse"` for SSE (WSF-3), the fields
//! `${env:NAME}` is expanded in (WSF-4), the `disabled` flag (WSF-5), that
//! a project cannot hide a user server and how a same-named project entry
//! combines with the user one (WSF-6, `ProjectSemantics::Unknown`), that
//! Windows starts `npx` without `cmd /c` (WSF-7) and the detection probes
//! (WSF-8).

use crate::adapters::caps::{
    Caps, DisableMech, EnvRefSyntax, FieldSet, ProjectSemantics, Source, TransportSet,
};
use crate::adapters::detect::Probe;
use crate::adapters::location::Location;
use crate::adapters::mapper::McpServersJson;
use crate::adapters::ported::Renamed;
use crate::adapters::spec::{AdapterSpec, ContainerPath, McpSpec, Tier, exact_names};
use crate::env::Os;
use crate::formats::Format;
use crate::model::clients::passthrough_of;

pub const ID: &str = "windsurf";

const CAPS: Caps = Caps {
    transports: TransportSet::ALL,
    env_ref: Some(EnvRefSyntax::DollarBraceEnv),
    env_ref_fields: FieldSet::COMMAND
        .union(FieldSet::ARGS)
        .union(FieldSet::ENV)
        .union(FieldSet::URL)
        .union(FieldSet::HEADERS),
    command_ref: None,
    native_disable: Some(DisableMech::DisabledTrue),
    project_disable: None,
    project_semantics: ProjectSemantics::Unknown,
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

/// `url` is spelled `serverUrl`.
static MAPPER: Renamed = Renamed {
    inner: McpServersJson {
        caps: &CAPS,
        type_field: false,
        remote_type: false,
        cwd: false,
        http_aliases: &[],
        url_implies_http: true,
        valid_name: |name| !name.is_empty(),
    },
    renames: &[("url", "serverUrl")],
};

pub static SPEC: AdapterSpec = AdapterSpec {
    id: ID,
    display_name: "Windsurf",
    tier: Tier::D,
    detect: &[
        Probe::Path(Location::Home(".codeium/windsurf")),
        Probe::Executable("windsurf"),
        Probe::UninstallEntry("Windsurf"),
    ],
    mcp: Some(McpSpec {
        oses: &[Os::Windows, Os::Linux, Os::MacOs],
        read_only: false,
        user: &[Location::Home(".codeium/windsurf/mcp_config.json")],
        user_files: None,
        user_mirrors: &[],
        profiles: None,
        project: &[Location::Project(".windsurf/mcp_config.json")],
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

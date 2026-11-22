//! Pi (`pi`): SPEC 7.3 and 7.4.
//!
//! - User scope: `~/.pi/agent/mcp.json`, or `$PI_CODING_AGENT_DIR/mcp.json`
//!   when that is an absolute path.
//! - Project scope: `<P>/.pi/mcp.json`, read by Pi only for trusted
//!   projects.
//! - Entries: `mcpServers.<name>` without `type` (`command` means stdio, a
//!   `url` streamable HTTP); `timeout` is in seconds; no SSE. Names are
//!   `[A-Za-z0-9_-]+`, and names differing only in `-` and `_` are the
//!   same server (reported as `collision`).
//! - An installed `pi-mcp-adapter` replaces Pi's MCP support
//!   (`client_mcp_replaced_by_extension`).

use crate::adapters::caps::{
    Caps, CommandRef, DisableMech, EnvRefSyntax, FieldSet, ProjectDisable, ProjectSemantics,
    Source, TimeUnit, TimeoutField, TransportSet, TrustGate,
};
use crate::adapters::detect::{McpExtension, Probe};
use crate::adapters::location::Location;
use crate::adapters::mapper::{McpServersJson, word_name};
use crate::adapters::spec::{AdapterSpec, ContainerPath, McpSpec, Tier};
use crate::env::Os;
use crate::formats::Format;
use crate::model::clients::passthrough_of;

pub const ID: &str = "pi";

/// A path in Pi's agent directory: `~/.pi/agent/<rel>`, or
/// `$PI_CODING_AGENT_DIR/<rel>`.
const fn agent(rel: &'static str) -> Location {
    Location::EnvDir {
        var: "PI_CODING_AGENT_DIR",
        fallback: &Location::Home(".pi/agent"),
        rel,
    }
}

const CAPS: Caps = Caps {
    transports: TransportSet::STDIO_HTTP,
    env_ref: Some(EnvRefSyntax::DollarBrace { default: false }),
    env_ref_fields: FieldSet::ENV.union(FieldSet::HEADERS),
    command_ref: Some(CommandRef::Bang),
    native_disable: Some(DisableMech::EnabledFalse),
    // A project entry without `command`, `url` or `type` overrides only
    // `enabled` (and exposure) of the user entry.
    project_disable: Some(ProjectDisable::EnabledFalse { partial: true }),
    project_semantics: ProjectSemantics::Replace,
    precedence: &[Source::Project, Source::User],
    compat_reads: &[],
    trust_gate: Some(TrustGate::TrustedProject),
    rewrites_file_at_runtime: false,
    restart_required: false,
    // `toolExposure` is not mapped from `[tools]` yet; it is kept as an
    // unmodeled native field.
    tool_filter: None,
    // `[verify]` whether Pi on Windows starts `npx` without `cmd /c`.
    windows_shell_shim: false,
    timeouts: &[TimeoutField {
        key: "timeout",
        unit: TimeUnit::Seconds,
    }],
    passthrough_allowlist: passthrough_of(ID),
};

static MAPPER: McpServersJson = McpServersJson {
    caps: &CAPS,
    type_field: false,
    remote_type: false,
    cwd: true,
    http_aliases: &["streamable-http"],
    url_implies_http: true,
    valid_name: word_name,
};

/// `pi-mcp-adapter`, installed as an auto-discovered extension or as a
/// package in the user or project settings (`[verify]` install shapes).
const MCP_ADAPTER: McpExtension = McpExtension {
    package: "pi-mcp-adapter",
    paths: &[
        agent("extensions/pi-mcp-adapter"),
        agent("extensions/pi-mcp-adapter.ts"),
        agent("extensions/pi-mcp-adapter.js"),
        Location::Project(".pi/extensions/pi-mcp-adapter"),
        Location::Project(".pi/extensions/pi-mcp-adapter.ts"),
        Location::Project(".pi/extensions/pi-mcp-adapter.js"),
    ],
    package_lists: &[
        agent("settings.json"),
        Location::Project(".pi/settings.json"),
    ],
};

pub static SPEC: AdapterSpec = AdapterSpec {
    id: ID,
    display_name: "Pi",
    tier: Tier::B,
    detect: &[Probe::Path(agent("")), Probe::Executable("pi")],
    mcp: Some(McpSpec {
        oses: &[Os::Windows, Os::Linux, Os::MacOs],
        read_only: false,
        user: &[agent("mcp.json")],
        user_files: None,
        user_mirrors: &[],
        profiles: None,
        project: &[Location::Project(".pi/mcp.json")],
        project_local: &[],
        format: Format::Json,
        skeleton: "",
        container: ContainerPath(&["mcpServers"]),
        project_local_container: ContainerPath(&["mcpServers"]),
        legacy_containers: &[],
        same_name,
        replaced_by: &[MCP_ADAPTER],
        mapper: &MAPPER,
        caps: CAPS,
    }),
};

/// Pi counts names that differ only in `-` and `_` as one server.
fn same_name(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes().zip(b.bytes()).all(|(x, y)| {
            let fold = |byte: u8| if byte == b'-' { b'_' } else { byte };
            fold(x) == fold(y)
        })
}

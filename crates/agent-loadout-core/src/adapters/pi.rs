//! Pi (`pi`): SPEC 7.3 and 7.4.
//!
//! - User scope: `~/.pi/agent/mcp.json`, or `$PI_CODING_AGENT_DIR/mcp.json`
//!   when that is an absolute path once a leading `~/` (or `~\`) is
//!   expanded, as Pi does.
//! - Project scope: `<P>/.pi/mcp.json`, read by Pi only for trusted
//!   projects.
//! - Entries: `mcpServers.<name>`, written with `type` (`stdio` or `http`)
//!   so that they read the same as omp's when `PI_CODING_AGENT_DIR` points
//!   both clients at one file; Pi itself takes `command` as stdio and a
//!   `url` as streamable HTTP. `timeout` is in seconds; no SSE. Names are
//!   `[A-Za-z0-9_-]+`, and names differing only in `-` and `_` are the
//!   same server (reported as `collision`).
//! - `env` and `headers` values expand `${NAME}` and bare `$NAME`; `$$` is
//!   a literal `$`, `$!` a literal `!`, and a value starting with `!` runs
//!   as a command (`EnvRefSyntax::Dollar`).
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
/// `$PI_CODING_AGENT_DIR/<rel>` (with a leading `~` expanded).
const fn agent(rel: &'static str) -> Location {
    Location::EnvDir {
        var: "PI_CODING_AGENT_DIR",
        fallback: &Location::Home(".pi/agent"),
        rel,
        tilde: true,
    }
}

const CAPS: Caps = Caps {
    transports: TransportSet::STDIO_HTTP,
    env_ref: Some(EnvRefSyntax::Dollar),
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
    // Pi accepts `type`; writing it makes entries agree with omp's in a file
    // both read (gate V2 OMP-10).
    type_field: true,
    remote_type: true,
    cwd: true,
    http_aliases: &["streamable-http"],
    url_implies_http: true,
    valid_name: word_name,
};

/// `pi-mcp-adapter`, installed as a package (`pi install npm:pi-mcp-adapter`
/// puts it under `npm/node_modules` and adds `npm:pi-mcp-adapter` to the
/// settings' `packages`; `-l` does the same under `<P>/.pi`; gate V2 PI-8)
/// or as an auto-discovered extension under `extensions/`.
const MCP_ADAPTER: McpExtension = McpExtension {
    package: "pi-mcp-adapter",
    paths: &[
        agent("npm/node_modules/pi-mcp-adapter"),
        agent("extensions/pi-mcp-adapter"),
        agent("extensions/pi-mcp-adapter.ts"),
        agent("extensions/pi-mcp-adapter.js"),
        Location::Project(".pi/npm/node_modules/pi-mcp-adapter"),
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

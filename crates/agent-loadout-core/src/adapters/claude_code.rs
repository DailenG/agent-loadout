//! Claude Code (`claude-code`): SPEC 7.3 and 7.4.
//!
//! - User scope: `mcpServers` in `~/.claude.json`, or in
//!   `$CLAUDE_CONFIG_DIR/.claude.json` when that is set (`[verify]`).
//! - Project scope: `mcpServers` in `<P>/.mcp.json`.
//! - Project-local scope: `projects["<P>"].mcpServers` in the user file. A
//!   user server is hidden in a project by listing it in
//!   `projects["<P>"].disabledMcpServers`.

use crate::adapters::caps::{
    Caps, CommandRef, EnvRefSyntax, FieldSet, ProjectDisable, ProjectSemantics, Source,
    TransportSet, TrustGate,
};
use crate::adapters::detect::Probe;
use crate::adapters::location::Location;
use crate::adapters::mapper::{McpServersJson, dotted_word_name};
use crate::adapters::spec::{AdapterSpec, ContainerPath, McpSpec, Tier, exact_names};
use crate::env::Os;
use crate::formats::Format;
use crate::model::clients::passthrough_of;

pub const ID: &str = "claude-code";

/// `~/.claude.json`, or `$CLAUDE_CONFIG_DIR/.claude.json`.
const USER_FILE: Location = Location::EnvDir {
    var: "CLAUDE_CONFIG_DIR",
    fallback: &Location::Home(""),
    rel: ".claude.json",
    tilde: false,
};

/// `~/.claude`, or `$CLAUDE_CONFIG_DIR`.
const CONFIG_DIR: Location = Location::EnvDir {
    var: "CLAUDE_CONFIG_DIR",
    fallback: &Location::Home(".claude"),
    rel: "",
    tilde: false,
};

const CAPS: Caps = Caps {
    // `ws` entries are read as unsupported: the store has no `ws` transport.
    transports: TransportSet::ALL,
    env_ref: Some(EnvRefSyntax::DollarBrace { default: true }),
    env_ref_fields: FieldSet::COMMAND
        .union(FieldSet::ARGS)
        .union(FieldSet::ENV)
        .union(FieldSet::URL)
        .union(FieldSet::HEADERS),
    command_ref: Some(CommandRef::HeadersHelper),
    native_disable: None,
    project_disable: Some(ProjectDisable::ListMember(ContainerPath(&[
        "projects",
        ContainerPath::PROJECT_KEY,
        "disabledMcpServers",
    ]))),
    project_semantics: ProjectSemantics::Replace,
    precedence: &[Source::ProjectLocal, Source::Project, Source::User],
    compat_reads: &[],
    trust_gate: Some(TrustGate::ApprovalPrompt),
    rewrites_file_at_runtime: true,
    restart_required: false,
    tool_filter: None,
    windows_shell_shim: false,
    timeouts: &[],
    passthrough_allowlist: passthrough_of(ID),
};

/// Names are `[A-Za-z0-9_.-]+` (gate V1, CC-7; SPEC 5.1 had no `.`).
static MAPPER: McpServersJson = McpServersJson {
    caps: &CAPS,
    type_field: true,
    remote_type: true,
    cwd: false,
    http_aliases: &[],
    url_implies_http: false,
    valid_name: dotted_word_name,
};

pub static SPEC: AdapterSpec = AdapterSpec {
    id: ID,
    display_name: "Claude Code",
    tier: Tier::A,
    detect: &[
        Probe::Path(USER_FILE),
        Probe::Path(CONFIG_DIR),
        Probe::Executable("claude"),
    ],
    mcp: Some(McpSpec {
        oses: &[Os::Windows, Os::Linux, Os::MacOs],
        read_only: false,
        user: &[USER_FILE],
        user_files: None,
        user_mirrors: &[],
        profiles: None,
        project: &[Location::Project(".mcp.json")],
        project_local: &[USER_FILE],
        format: Format::Json,
        skeleton: "",
        container: ContainerPath(&["mcpServers"]),
        project_local_container: ContainerPath(&[
            "projects",
            ContainerPath::PROJECT_KEY,
            "mcpServers",
        ]),
        legacy_containers: &[],
        same_name: exact_names,
        replaced_by: &[],
        mapper: &MAPPER,
        caps: CAPS,
    }),
};

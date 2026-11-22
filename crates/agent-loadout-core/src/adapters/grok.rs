//! Grok Build (`grok`): SPEC 7.3 and 7.4.
//!
//! - User scope: `~/.grok/config.toml`.
//! - Project scope: `<P>/.grok/config.toml` (Grok walks from the working
//!   directory up to the git root; Loadout writes at the project root),
//!   loaded only in trusted folders.
//! - Entries: `[mcp_servers.<name>]` with `command, args, env` (stdio) or
//!   `url, headers` (Streamable HTTP), no `type`; `startup_timeout_sec` and
//!   `tool_timeout_sec` are passthrough fields in seconds. No SSE, no
//!   `cwd`. The shape is the `mcpServers` one of the JSON clients, so the
//!   shared [`McpServersJson`] mapper writes it (the TOML editor spells
//!   the table).
//! - Grok also loads Claude Code's `~/.claude.json` and `<P>/.mcp.json`
//!   and Cursor's `<P>/.cursor/mcp.json`: the effective view lists them as
//!   `client-compat` sources and flags names Grok also has natively
//!   (`duplicate_via_compat`).
//! - A same-named project entry replaces the user one, and nothing hides a
//!   user server in a project (`cannot_suppress_in_project`).
//!
//! Assumptions left for Tier C verification (`scripts/verify-clients.md`,
//! GRK-*): `[verify]` the user and project files and the walk to the git
//! root (GRK-1, GRK-2), the folder trust prompt (GRK-3), the compatibility
//! sources and their order below Grok's own files (GRK-4: `~/.claude.json`
//! top-level `mcpServers` only, then `<P>/.cursor/mcp.json`, then
//! `<P>/.mcp.json`), that there is no native disable field (GRK-5), that
//! a project entry replaces the user entry (GRK-6), the fields `${NAME}`
//! and `${NAME:-default}` are expanded in (GRK-7, assumed `env` and
//! header values only), that any non-empty name loads (GRK-8), that
//! Windows starts `npx` without `cmd /c` (GRK-9) and the detection probes
//! (GRK-10).

use crate::adapters::caps::{
    Caps, CompatSource, EnvRefSyntax, FieldSet, ProjectSemantics, Source, TimeUnit, TimeoutField,
    TransportSet, TrustGate,
};
use crate::adapters::detect::Probe;
use crate::adapters::location::Location;
use crate::adapters::mapper::McpServersJson;
use crate::adapters::spec::{AdapterSpec, ContainerPath, McpSpec, Tier, exact_names};
use crate::adapters::{claude_code, cursor};
use crate::env::Os;
use crate::formats::Format;
use crate::model::clients::passthrough_of;

pub const ID: &str = "grok";

const CAPS: Caps = Caps {
    transports: TransportSet::STDIO_HTTP,
    env_ref: Some(EnvRefSyntax::DollarBrace {
        default: true,
        bare: false,
    }),
    env_ref_fields: FieldSet::ENV.union(FieldSet::HEADERS),
    command_ref: None,
    native_disable: None,
    project_disable: None,
    project_semantics: ProjectSemantics::Replace,
    precedence: &[Source::Project, Source::User],
    compat_reads: &[
        CompatSource {
            client: claude_code::ID,
            source: Source::User,
        },
        CompatSource {
            client: cursor::ID,
            source: Source::Project,
        },
        CompatSource {
            client: claude_code::ID,
            source: Source::Project,
        },
    ],
    trust_gate: Some(TrustGate::TrustedFolder),
    rewrites_file_at_runtime: false,
    restart_required: false,
    tool_filter: None,
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
    display_name: "Grok Build",
    tier: Tier::C,
    detect: &[
        Probe::Path(Location::Home(".grok")),
        Probe::Executable("grok"),
    ],
    mcp: Some(McpSpec {
        oses: &[Os::Windows, Os::Linux, Os::MacOs],
        read_only: false,
        user: &[Location::Home(".grok/config.toml")],
        user_files: None,
        user_mirrors: &[],
        profiles: None,
        project: &[Location::Project(".grok/config.toml")],
        project_local: &[],
        format: Format::Toml,
        skeleton: "",
        container: ContainerPath(&["mcp_servers"]),
        project_local_container: ContainerPath(&["mcp_servers"]),
        legacy_containers: &[],
        same_name: exact_names,
        replaced_by: &[],
        mapper: &MAPPER,
        caps: CAPS,
    }),
};

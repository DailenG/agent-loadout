//! oh-my-pi (`omp`): SPEC 7.3 and 7.4.
//!
//! - Root: `~/.omp`, or `~/$PI_CONFIG_DIR` (a directory name under home).
//! - User scope: `<root>/agent/mcp.json` for the default profile, or
//!   `$PI_CODING_AGENT_DIR/mcp.json` when that is set; a named profile
//!   `<name>` uses `<root>/profiles/<name>/agent/mcp.json` and ignores
//!   `PI_CODING_AGENT_DIR`. `profiles` in `clients.toml` picks the profiles.
//! - Project scope: `<P>/.omp/mcp.json`.
//! - Entries: `mcpServers.<name>` with an explicit `type`; `timeout` is in
//!   milliseconds. A new file starts with the `$schema` line omp writes
//!   itself. Top-level `disabledServers` and `enabledServers` are never
//!   touched.

use crate::adapters::caps::{
    Caps, CommandRef, CompatSource, DisableMech, EnvRefSyntax, FieldSet, ProjectDisable,
    ProjectSemantics, Source, TimeUnit, TimeoutField, TransportSet,
};
use crate::adapters::detect::Probe;
use crate::adapters::location::Location;
use crate::adapters::mapper::McpServersJson;
use crate::adapters::spec::{AdapterSpec, ContainerPath, McpSpec, Profiles, Tier, exact_names};
use crate::env::Os;
use crate::formats::Format;
use crate::model::clients::passthrough_of;

pub const ID: &str = "omp";

/// The profile name meaning the default user file.
pub const DEFAULT_PROFILE: &str = "default";

/// `~/.omp`, or `~/$PI_CONFIG_DIR`.
const ROOT: Location = Location::HomeNamedDir {
    var: "PI_CONFIG_DIR",
    default: ".omp",
    rel: "",
};

/// The default profile's user file: `<root>/agent/mcp.json`, or
/// `$PI_CODING_AGENT_DIR/mcp.json`. Unlike Pi, omp does not expand a
/// leading `~` in the variable (omp 19.1 resolves it with `path.resolve`,
/// relative to the working directory), so no `tilde` here.
const USER_FILE: Location = Location::EnvDir {
    var: "PI_CODING_AGENT_DIR",
    fallback: &Location::HomeNamedDir {
        var: "PI_CONFIG_DIR",
        default: ".omp",
        rel: "agent",
    },
    rel: "mcp.json",
    tilde: false,
};

/// What omp writes at the top of a new MCP file.
const SKELETON: &str = "{\n  \"$schema\": \"https://raw.githubusercontent.com/can1357/oh-my-pi/main/packages/coding-agent/src/config/mcp-schema.json\"\n}\n";

const CAPS: Caps = Caps {
    transports: TransportSet::ALL,
    env_ref: Some(EnvRefSyntax::DollarBrace { default: true }),
    env_ref_fields: FieldSet::COMMAND
        .union(FieldSet::ARGS)
        .union(FieldSet::ENV)
        .union(FieldSet::CWD)
        .union(FieldSet::URL)
        .union(FieldSet::HEADERS),
    command_ref: Some(CommandRef::Bang),
    native_disable: Some(DisableMech::EnabledFalse),
    // The project entry replaces the user one, so the suppression is the
    // whole entry with `enabled: false` (`[verify]` whether an
    // `enabled`-only entry is enough).
    project_disable: Some(ProjectDisable::EnabledFalse { partial: false }),
    project_semantics: ProjectSemantics::Replace,
    precedence: &[Source::Project, Source::User],
    compat_reads: &[
        CompatSource {
            client: "claude-code",
            source: Source::User,
        },
        CompatSource {
            client: "claude-code",
            source: Source::Project,
        },
        CompatSource {
            client: "codex",
            source: Source::User,
        },
        CompatSource {
            client: "codex",
            source: Source::Project,
        },
        CompatSource {
            client: "gemini",
            source: Source::User,
        },
        CompatSource {
            client: "gemini",
            source: Source::Project,
        },
        CompatSource {
            client: "opencode",
            source: Source::User,
        },
        CompatSource {
            client: "opencode",
            source: Source::Project,
        },
        CompatSource {
            client: "cursor",
            source: Source::User,
        },
        CompatSource {
            client: "cursor",
            source: Source::Project,
        },
        CompatSource {
            client: "windsurf",
            source: Source::User,
        },
        CompatSource {
            client: "windsurf",
            source: Source::Project,
        },
        CompatSource {
            client: "vscode-legacy",
            source: Source::Project,
        },
    ],
    trust_gate: None,
    rewrites_file_at_runtime: false,
    restart_required: false,
    tool_filter: None,
    // `[verify]` whether omp on Windows starts `npx` without `cmd /c`.
    windows_shell_shim: false,
    timeouts: &[TimeoutField {
        key: "timeout",
        unit: TimeUnit::Milliseconds,
    }],
    passthrough_allowlist: passthrough_of(ID),
};

static MAPPER: McpServersJson = McpServersJson {
    caps: &CAPS,
    type_field: true,
    remote_type: true,
    cwd: true,
    http_aliases: &[],
    url_implies_http: true,
    valid_name,
};

pub static SPEC: AdapterSpec = AdapterSpec {
    id: ID,
    display_name: "oh-my-pi",
    tier: Tier::A,
    detect: &[Probe::Path(ROOT), Probe::Executable("omp")],
    mcp: Some(McpSpec {
        oses: &[Os::Windows, Os::Linux, Os::MacOs],
        read_only: false,
        user: &[USER_FILE],
        user_files: None,
        user_mirrors: &[],
        profiles: Some(Profiles {
            default: DEFAULT_PROFILE,
            dir: Location::HomeNamedDir {
                var: "PI_CONFIG_DIR",
                default: ".omp",
                rel: "profiles",
            },
            rel: "agent/mcp.json",
            valid_name: valid_profile,
        }),
        project: &[Location::Project(".omp/mcp.json")],
        project_local: &[],
        format: Format::Json,
        skeleton: SKELETON,
        container: ContainerPath(&["mcpServers"]),
        project_local_container: ContainerPath(&["mcpServers"]),
        legacy_containers: &[],
        same_name: exact_names,
        replaced_by: &[],
        mapper: &MAPPER,
        caps: CAPS,
    }),
};

/// omp server names: up to 100 characters of letters, digits, `_`, `-`,
/// `.` and `:`, with single spaces between non-space segments.
fn valid_name(name: &str) -> bool {
    name.chars().count() <= 100
        && !name.is_empty()
        && name.split(' ').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
        })
}

/// omp profile names: `[a-z0-9][a-z0-9._-]{0,63}`, not ending in `.`, and
/// not a Windows reserved device name (`CON`, `COM1`, `nul.txt`, ...).
fn valid_profile(name: &str) -> bool {
    let bytes = name.as_bytes();
    let pattern = matches!(bytes.first(), Some(b'a'..=b'z' | b'0'..=b'9'))
        && bytes.len() <= 64
        && bytes
            .iter()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'));
    let base = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base.as_bytes()[3].is_ascii_digit());
    pattern && !name.ends_with('.') && !reserved
}

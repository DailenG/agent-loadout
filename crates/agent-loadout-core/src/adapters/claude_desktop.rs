//! Claude Desktop (`claude-desktop`): SPEC 7.3 and 7.4, Decision 12.
//!
//! User scope only, stdio only (remote servers are Connectors in the app).
//! Windows: `%APPDATA%\Claude\claude_desktop_config.json`, plus the MSIX
//! copy under `%LOCALAPPDATA%\Packages\Claude_*\LocalCache\Roaming\Claude`
//! when it exists (both are written, with a warning). macOS: best effort.
//! Linux: no target (`not_supported_on_os`). The app reads its file only at
//! launch and spawns servers without a shell, so `npx` is written as
//! `cmd /c npx` on Windows.

use crate::adapters::caps::{Caps, FieldSet, ProjectSemantics, Source, TransportSet};
use crate::adapters::detect::Probe;
use crate::adapters::location::Location;
use crate::adapters::mapper::McpServersJson;
use crate::adapters::spec::{AdapterSpec, ContainerPath, McpSpec, Tier, exact_names};
use crate::env::Os;
use crate::formats::Format;
use crate::model::clients::passthrough_of;

pub const ID: &str = "claude-desktop";

const PACKAGES: Location = Location::LocalAppData("Packages");

const CONFIG: Location = Location::AppData("Claude/claude_desktop_config.json");

/// The MSIX virtualized copy of [`CONFIG`].
const MSIX_CONFIG: Location = Location::Glob {
    base: &PACKAGES,
    pattern: "Claude_*/LocalCache/Roaming/Claude/claude_desktop_config.json",
};

const CAPS: Caps = Caps {
    transports: TransportSet::STDIO,
    env_ref: None,
    env_ref_fields: FieldSet::NONE,
    command_ref: None,
    native_disable: None,
    project_disable: None,
    project_semantics: ProjectSemantics::Unknown,
    precedence: &[Source::User],
    compat_reads: &[],
    trust_gate: None,
    // The app re-serializes the whole file at launch (V1 CD-6).
    rewrites_file_at_runtime: true,
    restart_required: true,
    tool_filter: None,
    windows_shell_shim: true,
    timeouts: &[],
    passthrough_allowlist: passthrough_of(ID),
};

static MAPPER: McpServersJson = McpServersJson {
    caps: &CAPS,
    type_field: false,
    remote_type: false,
    cwd: false,
    http_aliases: &[],
    url_implies_http: false,
    valid_name: |name| !name.is_empty(),
};

pub static SPEC: AdapterSpec = AdapterSpec {
    id: ID,
    display_name: "Claude Desktop",
    tier: Tier::A,
    detect: &[
        Probe::Path(Location::AppData("Claude")),
        // `[verify]` the uninstall `DisplayName` of the Windows installer.
        Probe::UninstallEntry("Claude"),
        Probe::MsixPackage(Location::Glob {
            base: &PACKAGES,
            pattern: "Claude_*",
        }),
    ],
    mcp: Some(McpSpec {
        oses: &[Os::Windows, Os::MacOs],
        read_only: false,
        user: &[CONFIG],
        user_files: None,
        user_mirrors: &[MSIX_CONFIG],
        profiles: None,
        project: &[],
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

//! Devin Desktop, Windsurf's successor (`devin`): SPEC 7.3 and 7.4. Best
//! effort.
//!
//! - User scope: `%APPDATA%\devin\mcp_config.json` on Windows,
//!   `~/.config/devin/mcp_config.json` on Linux and macOS.
//! - Project scope: `<P>/.devin/mcp_config.json`; project-local (private):
//!   `<P>/.devin/mcp_config.local.json`, which Loadout never puts in a file
//!   the project's git repository tracks (SPEC 6.5).
//! - Entries: `mcpServers.<name>`: `{command, args, env}` (stdio) or
//!   `{url, transport, headers}` with `transport` `http` or `sse`. A remote
//!   entry without `transport` reads as http; `streamable-http` reads as
//!   http too.
//! - Variables: Devin expands `${env:NAME}`; Loadout writes it for `env`
//!   delivery.
//! - No native disable and no way to hide a user server in a project:
//!   entries are removed, and a suppression reports
//!   `cannot_suppress_in_project`.
//!
//! Assumptions left for Tier D verification (`scripts/verify-clients.md`,
//! DEV-*): `[verify]` the user path per OS (DEV-1), the project and
//! project-local files and their precedence local > project > user (DEV-2,
//! DEV-3; `ProjectSemantics::Unknown` until then), the `transport` key and
//! its values and `headers` (DEV-4), the fields `${env:NAME}` is expanded in
//! (DEV-5), that there is no disable flag or project suppression (DEV-6),
//! that Windows starts `npx` without `cmd /c` (DEV-7) and the detection
//! probes (DEV-8).

use std::path::PathBuf;

use crate::adapters::caps::{Caps, EnvRefSyntax, FieldSet, ProjectSemantics, Source, TransportSet};
use crate::adapters::detect::Probe;
use crate::adapters::location::Location;
use crate::adapters::mapper::McpServersJson;
use crate::adapters::ported::Renamed;
use crate::adapters::spec::{AdapterSpec, ContainerPath, McpSpec, Tier, exact_names};
use crate::env::{Env, Os};
use crate::error::{CoreError, ErrorCode};
use crate::formats::Format;
use crate::fs::Fs;
use crate::model::clients::{ClientPrefs, passthrough_of};

pub const ID: &str = "devin";

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
    project_semantics: ProjectSemantics::Unknown,
    precedence: &[Source::ProjectLocal, Source::Project, Source::User],
    compat_reads: &[],
    trust_gate: None,
    rewrites_file_at_runtime: false,
    restart_required: false,
    tool_filter: None,
    windows_shell_shim: false,
    timeouts: &[],
    passthrough_allowlist: passthrough_of(ID),
};

/// `type` is spelled `transport`, written on remote entries only.
static MAPPER: Renamed = Renamed {
    inner: McpServersJson {
        caps: &CAPS,
        type_field: false,
        remote_type: true,
        cwd: false,
        http_aliases: &["streamable-http"],
        url_implies_http: true,
        valid_name: |name| !name.is_empty(),
    },
    renames: &[("type", "transport")],
};

/// The user file: `%APPDATA%\devin` on Windows, `~/.config/devin`
/// elsewhere (not `$XDG_CONFIG_HOME`, and not `~/Library` on macOS).
fn user_files(
    env: &Env,
    _fs: &dyn Fs,
    _prefs: &ClientPrefs,
) -> Result<Vec<(PathBuf, Option<String>)>, CoreError> {
    let location = match env.os() {
        Os::Windows => Location::AppData("devin/mcp_config.json"),
        Os::Linux | Os::MacOs => Location::Home(".config/devin/mcp_config.json"),
    };
    let file = location.resolve(env, None).ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotSupportedOnOs,
            format!("Devin has no config path on {}", env.os().as_str()),
        )
    })?;
    Ok(vec![(file, None)])
}

pub static SPEC: AdapterSpec = AdapterSpec {
    id: ID,
    display_name: "Devin Desktop",
    tier: Tier::D,
    detect: &[
        Probe::Path(Location::AppData("devin")),
        Probe::Path(Location::Home(".config/devin")),
        Probe::Executable("devin"),
        Probe::UninstallEntry("Devin"),
    ],
    mcp: Some(McpSpec {
        oses: &[Os::Windows, Os::Linux, Os::MacOs],
        read_only: false,
        user: &[Location::Home(".config/devin/mcp_config.json")],
        user_files: Some(user_files),
        user_mirrors: &[],
        profiles: None,
        project: &[Location::Project(".devin/mcp_config.json")],
        project_local: &[Location::Project(".devin/mcp_config.local.json")],
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

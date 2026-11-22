//! VS Code's editor-managed MCP format, legacy (`vscode-legacy`): SPEC 7.3
//! and 7.4. **Read and import only; Loadout never writes these files.**
//!
//! - User scope: `<AppData>/<variant>/User/mcp.json` (`%APPDATA%` on
//!   Windows, `~/Library/Application Support` on macOS, `$XDG_CONFIG_HOME`
//!   on Linux), plus `User/profiles/<id>/mcp.json` per profile. The
//!   variants (`Code`, `Code - Insiders`, `VSCodium`) and profiles come
//!   from `import_variants` and `import_profiles` in `clients.toml`
//!   (default: `Code`, default profile only).
//! - Project scope: `<P>/.vscode/mcp.json`.
//! - Entries: JSONC `servers.<name>` (`{type, command, args, env, envFile,
//!   cwd, dev, sandbox}` or `{type, url, headers}`), next to a top-level
//!   `inputs[]`. `envFile`, `dev` and `sandbox` read as unmapped. Import
//!   converts `${env:NAME}` to store references; a `${input:...}` value is
//!   one VS Code prompts for and has no store form, so it is
//!   `needs_manual_secret`.
//! - VS Code deprecated this Local-harness format (removal February 2027)
//!   but can forward its servers into Copilot sessions, so the effective
//!   view lists these files as `client-compat` sources of `copilot`; omp
//!   reads `<P>/.vscode/mcp.json` too.
//!
//! The adapter is [`McpSpec::read_only`]: the planner never selects it,
//! naming it in an enable list reports `read_only_client`, and `all-detected`
//! skips it.
//!
//! Assumptions left for gate V2 (`scripts/verify-clients.md`, VSL-*):
//! `[verify]` the per-OS user paths and the profile layout (VSL-1, VSL-2),
//! that VS Code forwards these servers into Copilot sessions (VSL-3), that
//! a missing `type` is inferred from `command` or `url` (VSL-4), and the
//! detection probes (VSL-5).

use std::path::{Component, Path, PathBuf};

use crate::adapters::caps::{
    Caps, EnvRefSyntax, FieldSet, ProjectSemantics, Source, TransportSet, TrustGate,
};
use crate::adapters::detect::Probe;
use crate::adapters::location::Location;
use crate::adapters::mapper::McpServersJson;
use crate::adapters::spec::{AdapterSpec, ContainerPath, McpSpec, Tier, exact_names};
use crate::env::{Env, Os};
use crate::error::{CoreError, ErrorCode};
use crate::formats::Format;
use crate::fs::Fs;
use crate::model::clients::{ClientPrefs, ProfileSelection, VSCODE_VARIANTS, passthrough_of};

pub const ID: &str = "vscode-legacy";

/// The variant read when `clients.toml` names none.
const DEFAULT_VARIANT: &str = "Code";

/// The profile name meaning `User/mcp.json` in an `import_profiles` list.
const DEFAULT_PROFILE: &str = "default";

const CAPS: Caps = Caps {
    transports: TransportSet::ALL,
    env_ref: Some(EnvRefSyntax::DollarBraceEnv),
    env_ref_fields: FieldSet::COMMAND
        .union(FieldSet::ARGS)
        .union(FieldSet::ENV)
        .union(FieldSet::CWD)
        .union(FieldSet::URL)
        .union(FieldSet::HEADERS),
    command_ref: None,
    native_disable: None,
    project_disable: None,
    project_semantics: ProjectSemantics::Unknown,
    precedence: &[Source::Project, Source::User],
    compat_reads: &[],
    trust_gate: Some(TrustGate::WorkspaceTrust),
    rewrites_file_at_runtime: false,
    restart_required: false,
    tool_filter: None,
    windows_shell_shim: false,
    timeouts: &[],
    passthrough_allowlist: passthrough_of(ID),
};

static MAPPER: McpServersJson = McpServersJson {
    caps: &CAPS,
    type_field: true,
    remote_type: true,
    cwd: true,
    http_aliases: &[],
    url_implies_http: true,
    valid_name: |name| !name.is_empty(),
};

pub static SPEC: AdapterSpec = AdapterSpec {
    id: ID,
    display_name: "VS Code (legacy mcp.json)",
    tier: Tier::D,
    detect: &[
        Probe::Path(Location::AppData("Code/User")),
        Probe::Path(Location::AppData("Code - Insiders/User")),
        Probe::Path(Location::AppData("VSCodium/User")),
        Probe::Executable("code"),
        Probe::Executable("code-insiders"),
        Probe::Executable("codium"),
    ],
    mcp: Some(McpSpec {
        oses: &[Os::Windows, Os::Linux, Os::MacOs],
        read_only: true,
        user: &[Location::AppData("Code/User/mcp.json")],
        user_files: Some(user_files),
        user_mirrors: &[],
        profiles: None,
        project: &[Location::Project(".vscode/mcp.json")],
        project_local: &[],
        format: Format::Jsonc,
        skeleton: "",
        container: ContainerPath(&["servers"]),
        project_local_container: ContainerPath(&["servers"]),
        legacy_containers: &[],
        same_name: exact_names,
        replaced_by: &[],
        mapper: &MAPPER,
        caps: CAPS,
    }),
};

/// The `mcp.json` files of the selected variants and profiles: for each
/// variant, its default profile file, then the selected profiles' files.
/// `all` lists the profiles that hold an `mcp.json`, by id.
fn user_files(
    env: &Env,
    fs: &dyn Fs,
    prefs: &ClientPrefs,
) -> Result<Vec<(PathBuf, Option<String>)>, CoreError> {
    let base = Location::AppData("").resolve(env, None).ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotSupportedOnOs,
            format!("VS Code has no user directory on {}", env.os().as_str()),
        )
    })?;
    let default_variants = [DEFAULT_VARIANT.to_owned()];
    let variants = prefs
        .import_variants
        .as_deref()
        .unwrap_or(&default_variants);
    let selection = prefs
        .import_profiles
        .clone()
        .unwrap_or(ProfileSelection::Default);
    let mut files: Vec<(PathBuf, Option<String>)> = Vec::new();
    let mut push = |file: PathBuf, label: Option<String>| {
        if !files.iter().any(|(known, _)| *known == file) {
            files.push((file, label));
        }
    };
    for variant in variants {
        if !VSCODE_VARIANTS.contains(&variant.as_str()) {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!("unknown VS Code variant {variant:?}; expected one of {VSCODE_VARIANTS:?}"),
            ));
        }
        let user = base.join(variant).join("User");
        let default_label = (variant != DEFAULT_VARIANT).then(|| variant.clone());
        let profile = |id: &str| {
            (
                user.join("profiles").join(id).join("mcp.json"),
                Some(format!("{variant}/profiles/{id}")),
            )
        };
        match &selection {
            ProfileSelection::Default => push(user.join("mcp.json"), default_label),
            ProfileSelection::All => {
                push(user.join("mcp.json"), default_label);
                for id in profile_ids(fs, &user.join("profiles")) {
                    let (file, label) = profile(&id);
                    push(file, label);
                }
            }
            ProfileSelection::List(ids) => {
                for id in ids {
                    if id == DEFAULT_PROFILE {
                        push(user.join("mcp.json"), default_label.clone());
                    } else if is_plain_name(id) {
                        let (file, label) = profile(id);
                        push(file, label);
                    } else {
                        return Err(CoreError::new(
                            ErrorCode::InvalidValue,
                            format!("{id:?} is not a VS Code profile id"),
                        ));
                    }
                }
            }
        }
    }
    Ok(files)
}

/// The ids of the profile directories under `dir` that hold an
/// `mcp.json`, sorted.
fn profile_ids(fs: &dyn Fs, dir: &Path) -> Vec<String> {
    if !fs.is_dir(dir) {
        return Vec::new();
    }
    let Ok(entries) = fs.read_dir(dir) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .into_iter()
        .filter(|entry| fs.exists(&entry.join("mcp.json")))
        .filter_map(|entry| {
            entry
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .collect();
    ids.sort();
    ids
}

/// One plain path component: no separators, `.` or `..`.
fn is_plain_name(id: &str) -> bool {
    let mut parts = Path::new(id).components();
    matches!(parts.next(), Some(Component::Normal(_)))
        && parts.next().is_none()
        && !id.contains(['/', '\\'])
}

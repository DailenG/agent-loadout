//! VS Code Stable with GitHub Copilot, and the Copilot CLI (`copilot`):
//! SPEC 7.3 and 7.4.
//!
//! - User scope: `$COPILOT_HOME/mcp-config.json` when `COPILOT_HOME` is an
//!   absolute path, else `~/.copilot/mcp-config.json`.
//! - Project scope: `<P>/.mcp.json` at the workspace root. Claude Code
//!   reads the same file and container, so the planner joins both clients
//!   into one target whose entries are the union of theirs (SPEC 6.5); a
//!   name both want with different entries is a `collision`.
//! - Entries: `mcpServers.<name>` with an explicit `type` (`stdio`, `http`,
//!   `sse`): `{type, command, args, env, cwd, tools}` or `{type, url,
//!   headers, tools, oauthClientId}`. The Copilot CLI's own spelling
//!   `"type": "local"` reads as stdio, and its `"tools": ["*"]` means no
//!   filter. `tools` is an allow list only, so `[tools] deny` blocks.
//! - Never writes VS Code's editor-only fields (`inputs`, `envFile`, `dev`,
//!   `sandbox`): they are not carried over from an existing entry either
//!   (an entry holding them reads them as unmapped, and the plan shows the
//!   difference before overwriting it).
//! - VS Code can also forward servers from its legacy `mcp.json` files into
//!   Copilot sessions, so the effective view lists `vscode-legacy` as a
//!   `client-compat` source and flags duplicates.
//!
//! Assumptions left for gate V2 (`scripts/verify-clients.md`, COP-*):
//! `[verify]` that VS Code's Copilot harness reads the user file above
//! (COP-1, COP-2), that Copilot expands `${NAME}` and `${NAME:-default}`
//! in `command`, `args`, `env` and header values but not in `url` (COP-4),
//! that `.mcp.json` sits behind workspace trust (COP-3), that there is no
//! native disable field (COP-6), that a project entry replaces a same-named
//! user entry (COP-5, `ProjectSemantics::Unknown` until then), that
//! `"type": "local"` and `"tools": ["*"]` mean what is read here (COP-7),
//! that names follow Claude Code's `[A-Za-z0-9_.-]+` (COP-8), and that
//! Windows starts `npx` without `cmd /c` (COP-9). Detection (COP-10) looks
//! for the Copilot CLI, `~/.copilot` and the Copilot Chat extension.

use crate::adapters::caps::{
    Caps, CompatSource, EnvRefSyntax, FieldSet, ProjectSemantics, Source, ToolFilterMech,
    TransportSet, TrustGate,
};
use crate::adapters::detect::Probe;
use crate::adapters::location::Location;
use crate::adapters::mapper::{
    Canonical, EntryMapper, MapError, McpServersJson, RenderedServer, ServerDef, WriteCx,
    dotted_word_name,
};
use crate::adapters::spec::{AdapterSpec, ContainerPath, McpSpec, Tier, exact_names};
use crate::env::Os;
use crate::formats::{Format, NativeMap, NativeValue};
use crate::model::clients::passthrough_of;

pub const ID: &str = "copilot";

/// `$COPILOT_HOME/<rel>`, or `~/.copilot/<rel>`.
const fn copilot_home(rel: &'static str) -> Location {
    Location::EnvDir {
        var: "COPILOT_HOME",
        fallback: &Location::Home(".copilot"),
        rel,
        tilde: false,
    }
}

/// VS Code's editor-only entry fields, never written for Copilot.
pub const EDITOR_ONLY: [&str; 4] = ["inputs", "envFile", "dev", "sandbox"];

const TYPE: &str = "type";
const TOOLS: &str = "tools";

const CAPS: Caps = Caps {
    transports: TransportSet::ALL,
    env_ref: Some(EnvRefSyntax::DollarBrace {
        default: true,
        bare: false,
    }),
    env_ref_fields: FieldSet::COMMAND
        .union(FieldSet::ARGS)
        .union(FieldSet::ENV)
        .union(FieldSet::HEADERS),
    command_ref: None,
    native_disable: None,
    project_disable: None,
    project_semantics: ProjectSemantics::Unknown,
    precedence: &[Source::Project, Source::User],
    compat_reads: &[
        CompatSource {
            client: "vscode-legacy",
            source: Source::Project,
        },
        CompatSource {
            client: "vscode-legacy",
            source: Source::User,
        },
    ],
    trust_gate: Some(TrustGate::WorkspaceTrust),
    rewrites_file_at_runtime: false,
    restart_required: false,
    tool_filter: Some(ToolFilterMech::Allow(TOOLS)),
    windows_shell_shim: false,
    timeouts: &[],
    passthrough_allowlist: passthrough_of(ID),
};

/// The `mcpServers` JSON entry with Copilot's spellings on read and
/// without VS Code's editor-only fields on write.
struct CopilotJson(McpServersJson);

static MAPPER: CopilotJson = CopilotJson(McpServersJson {
    caps: &CAPS,
    type_field: true,
    remote_type: true,
    cwd: true,
    http_aliases: &[],
    url_implies_http: true,
    valid_name: dotted_word_name,
});

impl CopilotJson {
    /// `value` with `"type": "local"` as `stdio` and `"tools": ["*"]`
    /// dropped; `None` when neither is there.
    fn normalized(value: &NativeValue) -> Option<NativeValue> {
        let entry = value.as_object()?;
        let local = entry.get(TYPE).and_then(NativeValue::as_str) == Some("local");
        let all_tools = matches!(
            entry.get(TOOLS),
            Some(NativeValue::Array(items))
                if items.len() == 1 && items[0].as_str() == Some("*")
        );
        if !local && !all_tools {
            return None;
        }
        let mut entry: NativeMap = entry.clone();
        if local {
            entry.insert(TYPE, "stdio".into());
        }
        if all_tools {
            entry.remove(TOOLS);
        }
        Some(NativeValue::Object(entry))
    }
}

impl EntryMapper for CopilotJson {
    fn to_native(
        &self,
        server: &RenderedServer,
        cx: &WriteCx<'_>,
    ) -> Result<NativeValue, MapError> {
        let mut value = self.0.to_native(server, cx)?;
        if let NativeValue::Object(entry) = &mut value {
            for key in EDITOR_ONLY {
                entry.remove(key);
            }
        }
        Ok(value)
    }

    fn from_native(&self, key: &str, value: &NativeValue) -> Result<ServerDef, MapError> {
        match Self::normalized(value) {
            Some(normalized) => self.0.from_native(key, &normalized),
            None => self.0.from_native(key, value),
        }
    }

    fn canonical(&self, value: &NativeValue) -> Canonical {
        match Self::normalized(value) {
            Some(normalized) => self.0.canonical(&normalized),
            None => self.0.canonical(value),
        }
    }

    fn skipped_by_client(&self, value: &NativeValue) -> Option<&'static str> {
        self.0.skipped_by_client(value)
    }
}

pub static SPEC: AdapterSpec = AdapterSpec {
    id: ID,
    display_name: "GitHub Copilot",
    tier: Tier::B,
    detect: &[
        Probe::Path(copilot_home("")),
        Probe::Executable("copilot"),
        Probe::Path(Location::Glob {
            base: &Location::Home(".vscode/extensions"),
            pattern: "github.copilot-chat-*",
        }),
    ],
    mcp: Some(McpSpec {
        oses: &[Os::Windows, Os::Linux, Os::MacOs],
        read_only: false,
        user: &[copilot_home("mcp-config.json")],
        user_files: None,
        user_mirrors: &[],
        profiles: None,
        project: &[Location::Project(".mcp.json")],
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

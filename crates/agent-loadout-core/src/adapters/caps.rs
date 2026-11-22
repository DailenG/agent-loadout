//! Client capabilities (SPEC 7.1 `Caps`, table 7.4): what a client can
//! express natively. The planner and the delivery renderer read these;
//! mappers enforce the ones that shape an entry.

use std::time::Duration;

use crate::formats::NativeValue;
use crate::model::server::Transport;

/// The transports a client can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportSet {
    pub stdio: bool,
    pub http: bool,
    pub sse: bool,
}

impl TransportSet {
    pub const STDIO: Self = Self {
        stdio: true,
        http: false,
        sse: false,
    };
    pub const STDIO_HTTP: Self = Self {
        stdio: true,
        http: true,
        sse: false,
    };
    pub const ALL: Self = Self {
        stdio: true,
        http: true,
        sse: true,
    };

    pub const fn contains(self, transport: Transport) -> bool {
        match transport {
            Transport::Stdio => self.stdio,
            Transport::Http => self.http,
            Transport::Sse => self.sse,
        }
    }
}

/// How a client spells an environment variable reference it expands itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvRefSyntax {
    /// `${NAME}`; `default` when `${NAME:-default}` is understood too.
    DollarBrace { default: bool },
    /// `${NAME}` and bare `$NAME`, no default; `$$` is a literal `$` and
    /// `$!` a literal `!`, so literal text is written with every `$`
    /// doubled and a leading `!` as `$!` (Pi; gate V2 PI-7).
    Dollar,
    /// `${env:NAME}` (Cursor, Windsurf, Devin).
    DollarBraceEnv,
    /// `{env:NAME}` (OpenCode).
    BraceEnv,
    /// No expansion; a parent variable is forwarded by name (Codex
    /// `env_vars`, `bearer_token_env_var`, `env_http_headers`).
    CodexForward,
}

/// A set of server fields (which fields a client expands references in).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FieldSet(u8);

impl FieldSet {
    pub const NONE: Self = Self(0);
    pub const COMMAND: Self = Self(1);
    pub const ARGS: Self = Self(1 << 1);
    pub const ENV: Self = Self(1 << 2);
    pub const CWD: Self = Self(1 << 3);
    pub const URL: Self = Self(1 << 4);
    pub const HEADERS: Self = Self(1 << 5);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// A native way to get a value by running a command at connect time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandRef {
    /// A whole value of `!command` in env or headers (Pi, omp).
    Bang,
    /// An entry-level `headersHelper` command printing a JSON object of
    /// headers (Claude Code, http and sse).
    HeadersHelper,
}

/// How a client disables an entry without removing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisableMech {
    /// `enabled: false` in the entry.
    EnabledFalse,
    /// `disabled: true` in the entry.
    DisabledTrue,
    /// Membership of the entry name in a list at this path from the root.
    ListMember(&'static [&'static str]),
}

/// How a project file hides a same-named user-scope server (SPEC 6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectDisable {
    /// The server name is added to a list in the client's project-local
    /// file at this container path (Claude Code
    /// `projects["<P>"].disabledMcpServers`).
    ListMember(crate::adapters::spec::ContainerPath),
    /// A project entry with `enabled: false` under the same name. With
    /// `partial`, the entry holds nothing else: the client merges it over
    /// the user entry (Pi, Codex). Otherwise it is the whole user entry with
    /// `enabled: false`, because the project entry replaces the user one
    /// (omp).
    EnabledFalse { partial: bool },
}

/// How a client combines a project entry with a same-named user entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectSemantics {
    /// The higher layer replaces the whole entry.
    Replace,
    /// Fields of the higher layer merge over the lower one.
    DeepMerge,
    Unknown,
}

/// A configuration layer a client loads entries from, for effective-config
/// computation. A precedence list is ordered highest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The private project layer (Claude Code local scope).
    ProjectLocal,
    /// The shared project file.
    Project,
    /// The user-scope file.
    User,
}

/// Another tool's file a client also loads entries from (SPEC 6.4
/// `client-compat:<file>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatSource {
    /// The client whose file it is.
    pub client: &'static str,
    pub source: Source,
}

/// A client's approval gate for project files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustGate {
    /// Servers in the project file need an approval prompt (Claude Code
    /// `.mcp.json`).
    ApprovalPrompt,
    /// The project must be marked trusted (Codex, Pi).
    TrustedProject,
    /// The folder must be trusted (Gemini, Grok).
    TrustedFolder,
    /// The editor workspace must be trusted (VS Code).
    WorkspaceTrust,
}

/// Native tool filtering fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolFilterMech {
    /// Separate allow and deny lists (Codex `enabled_tools`/`disabled_tools`,
    /// Gemini `includeTools`/`excludeTools`).
    AllowDeny {
        allow: &'static str,
        deny: &'static str,
    },
    /// An allow list only (Copilot `tools`).
    Allow(&'static str),
}

/// The unit of a native timeout field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeUnit {
    Seconds,
    Milliseconds,
}

impl TimeUnit {
    /// The unit in messages.
    pub const fn name(self) -> &'static str {
        match self {
            TimeUnit::Seconds => "seconds",
            TimeUnit::Milliseconds => "milliseconds",
        }
    }
}

/// A native timeout field and its unit (omp `timeout` in milliseconds, Pi
/// `timeout` in seconds). Timeouts are passthrough fields, so a value keeps
/// the unit of the client it is written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeoutField {
    pub key: &'static str,
    pub unit: TimeUnit,
}

impl TimeoutField {
    /// The duration a native value stands for: a non-negative, finite
    /// number in this field's unit. `None` for anything else.
    pub fn duration(&self, value: &NativeValue) -> Option<Duration> {
        let amount = match *value {
            NativeValue::Integer(amount) => u64::try_from(amount).ok()? as f64,
            NativeValue::Float(amount) if amount.is_finite() && amount >= 0.0 => amount,
            _ => return None,
        };
        let seconds = match self.unit {
            TimeUnit::Seconds => amount,
            TimeUnit::Milliseconds => amount / 1000.0,
        };
        Duration::try_from_secs_f64(seconds).ok()
    }

    /// What a value of this field must be, for error messages.
    pub const fn expected(&self) -> &'static str {
        match self.unit {
            TimeUnit::Seconds => "a non-negative number of seconds",
            TimeUnit::Milliseconds => "a non-negative number of milliseconds",
        }
    }
}

/// A client's MCP capabilities (SPEC 7.1, table 7.4).
#[derive(Debug, Clone, Copy)]
pub struct Caps {
    pub transports: TransportSet,
    pub env_ref: Option<EnvRefSyntax>,
    /// Fields the client expands `env_ref` references in.
    pub env_ref_fields: FieldSet,
    pub command_ref: Option<CommandRef>,
    pub native_disable: Option<DisableMech>,
    pub project_disable: Option<ProjectDisable>,
    pub project_semantics: ProjectSemantics,
    /// Layers the client loads, highest precedence first.
    pub precedence: &'static [Source],
    pub compat_reads: &'static [CompatSource],
    pub trust_gate: Option<TrustGate>,
    /// The client rewrites its file while running (`~/.claude.json`): writes
    /// need a compare-and-swap check and a retry.
    pub rewrites_file_at_runtime: bool,
    /// The client reads its file only at launch.
    pub restart_required: bool,
    pub tool_filter: Option<ToolFilterMech>,
    /// The client spawns stdio servers without a shell, so Windows `.cmd`
    /// shims (`npx`) run through `cmd /c` (SPEC 5.2).
    pub windows_shell_shim: bool,
    /// Native timeout fields (all passthrough), with their units.
    pub timeouts: &'static [TimeoutField],
    /// Native fields a server may pass through in `[client.<id>]`.
    pub passthrough_allowlist: &'static [&'static str],
}

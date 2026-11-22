//! Client adapters (SPEC 7): a declarative table of clients, each with
//! detection probes, MCP target locations, capabilities and an entry mapper.

pub mod antigravity;
pub mod caps;
pub mod claude_code;
pub mod claude_desktop;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod detect;
pub mod devin;
pub mod gemini;
pub mod grok;
pub mod location;
pub mod mapper;
pub mod omp;
pub mod opencode;
pub mod pi;
pub mod ported;
pub mod spec;
pub mod vscode_legacy;
pub mod windsurf;

pub use spec::{AdapterSpec, ContainerPath, McpSpec, Profiles, Target, Tier, UserFiles};

/// Every client with an adapter, in table order (SPEC 7.3, then the ported
/// clients of SPEC 7.5). `vscode-legacy` is read-only
/// ([`McpSpec::read_only`]): import and the effective view read it, the
/// planner never writes it. `intellij` has no MCP target (skills only).
pub static ADAPTERS: [&AdapterSpec; 38] = [
    &claude_code::SPEC,
    &omp::SPEC,
    &pi::SPEC,
    &codex::SPEC,
    &claude_desktop::SPEC,
    &copilot::SPEC,
    &cursor::SPEC,
    &gemini::SPEC,
    &antigravity::SPEC,
    &opencode::SPEC,
    &grok::SPEC,
    &vscode_legacy::SPEC,
    &windsurf::SPEC,
    &devin::SPEC,
    &ported::AUGMENT,
    &ported::CLINE,
    &ported::ROO_CODE,
    &ported::KILO_CODE,
    &ported::KIRO,
    &ported::JUNIE,
    &ported::DROID,
    &ported::COMMANDCODE,
    &ported::CRUSH,
    &ported::QODER,
    &ported::TRAE,
    &ported::TRAE_CN,
    &ported::CODEBUDDY,
    &ported::IFLOW,
    &ported::QWEN_CODE,
    &ported::ZENCODER,
    &ported::OPENCLAW,
    &ported::MIMO_CODE,
    &ported::WORKBUDDY,
    &ported::ZCODE,
    &ported::CONTINUE,
    &ported::GOOSE,
    &ported::HERMES,
    &ported::INTELLIJ,
];

/// The adapter for client `id`.
pub fn adapter(id: &str) -> Option<&'static AdapterSpec> {
    ADAPTERS.iter().copied().find(|spec| spec.id == id)
}

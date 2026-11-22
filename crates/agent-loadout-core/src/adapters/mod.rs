//! Client adapters (SPEC 7): a declarative table of clients, each with
//! detection probes, MCP target locations, capabilities and an entry mapper.

pub mod caps;
pub mod claude_code;
pub mod claude_desktop;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod detect;
pub mod location;
pub mod mapper;
pub mod omp;
pub mod pi;
pub mod spec;
pub mod vscode_legacy;

pub use spec::{AdapterSpec, ContainerPath, McpSpec, Profiles, Target, Tier, UserFiles};

/// Every client with an adapter, in table order (SPEC 7.3). `vscode-legacy`
/// is read-only ([`McpSpec::read_only`]): import and the effective view
/// read it, the planner never writes it.
pub static ADAPTERS: [&AdapterSpec; 8] = [
    &claude_code::SPEC,
    &omp::SPEC,
    &pi::SPEC,
    &codex::SPEC,
    &claude_desktop::SPEC,
    &copilot::SPEC,
    &cursor::SPEC,
    &vscode_legacy::SPEC,
];

/// The adapter for client `id`.
pub fn adapter(id: &str) -> Option<&'static AdapterSpec> {
    ADAPTERS.iter().copied().find(|spec| spec.id == id)
}

//! Client adapters (SPEC 7): a declarative table of clients, each with
//! detection probes, MCP target locations, capabilities and an entry mapper.

pub mod caps;
pub mod claude_code;
pub mod claude_desktop;
pub mod detect;
pub mod location;
pub mod mapper;
pub mod spec;

pub use spec::{AdapterSpec, ContainerPath, McpSpec, Target, Tier};

/// Every client with an adapter, in table order (SPEC 7.3).
pub static ADAPTERS: [&AdapterSpec; 2] = [&claude_code::SPEC, &claude_desktop::SPEC];

/// The adapter for client `id`.
pub fn adapter(id: &str) -> Option<&'static AdapterSpec> {
    ADAPTERS.iter().copied().find(|spec| spec.id == id)
}

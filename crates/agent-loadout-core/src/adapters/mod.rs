//! Client adapters (SPEC 7): a declarative table of clients, each with
//! detection probes, MCP target locations, capabilities and an entry mapper.

pub mod caps;
pub mod detect;
pub mod location;
pub mod mapper;
pub mod spec;

pub use spec::{AdapterSpec, ContainerPath, McpSpec, Target, Tier};

//! Hermetic test helpers (SPEC 19 items 1 and 7). Nothing here touches the
//! real profile, the real OS keyring, a real 1Password or GitHub CLI, or
//! the user's git configuration: every path lives under a fresh temp root,
//! the keyring is [`MemKeyring`], `op` is the `op-stub` binary
//! ([`op_stub::OpStub`]), `gh` is `gh-stub` ([`gh_stub::GhStub`]), Snyk
//! Agent Scan is `agent-scan-stub` ([`agent_scan_stub::AgentScanStub`]),
//! and a system git runs with [`git_env`]'s isolated configuration.

pub mod agent_scan_stub;
pub mod gh_stub;
pub mod git_env;
mod keyring_mem;
pub mod op_stub;
mod rooted_fs;
mod temp_home;

pub use keyring_mem::{MemKeyring, MemKeyringMode};
pub use rooted_fs::RootedFs;
pub use temp_home::TempHome;

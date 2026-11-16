//! Hermetic test helpers (SPEC 19 items 1 and 7). Nothing here touches the
//! real profile, the real OS keyring or a real 1Password CLI: every path
//! lives under a fresh temp root, the keyring is [`MemKeyring`] and `op` is
//! the `op-stub` binary ([`op_stub::OpStub`]).

mod keyring_mem;
pub mod op_stub;
mod rooted_fs;
mod temp_home;

pub use keyring_mem::{MemKeyring, MemKeyringMode};
pub use rooted_fs::RootedFs;
pub use temp_home::TempHome;

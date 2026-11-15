//! Hermetic test helpers (SPEC 19 item 1). Nothing here touches the real
//! profile: every path lives under a fresh temp root.

mod rooted_fs;
mod temp_home;

pub use rooted_fs::RootedFs;
pub use temp_home::TempHome;

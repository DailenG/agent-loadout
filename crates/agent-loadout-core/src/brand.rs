//! Single source of brand strings (SPEC Appendix C). A rename touches this
//! module, the crate names and the docs, nothing else.

/// Product display name and window title.
pub const PRODUCT_NAME: &str = "Loadout";
/// Project, repository and CLI package name (`cargo install agent-loadout`).
pub const PACKAGE_NAME: &str = "agent-loadout";
/// Typed CLI binary name.
pub const BIN_NAME: &str = "loadout";
/// GitHub repository (`owner/name`).
pub const REPO: &str = "DailenG/agent-loadout";
/// Tauri identifier; also what `loadout --identify` prints.
pub const APP_ID: &str = "dev.dailen.loadout";

/// Data directory name under `%LOCALAPPDATA%` on Windows.
pub const DATA_DIR_WINDOWS: &str = "Loadout";
/// Data directory name under `${XDG_DATA_HOME:-~/.local/share}` on Linux.
pub const DATA_DIR_LINUX: &str = "loadout";
/// Data directory name under `~/Library/Application Support` on macOS.
pub const DATA_DIR_MACOS: &str = "Loadout";
/// Environment variable that overrides the data directory.
pub const HOME_ENV_VAR: &str = "LOADOUT_HOME";

/// Per-project manifest directory (`<P>/.loadout/`).
pub const PROJECT_DIR: &str = ".loadout";
/// Prefix of git commit trailers written to the store (`Loadout-Op: ...`).
pub const TRAILER_PREFIX: &str = "Loadout-";
/// OS keyring service name.
pub const KEYRING_SERVICE: &str = "loadout";

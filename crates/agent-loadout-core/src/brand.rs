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
/// Debug builds of the CLI read a test keyring file from this variable
/// instead of the OS keyring; release builds refuse keyring calls while it
/// is set.
pub const TEST_KEYRING_ENV_VAR: &str = "LOADOUT_TEST_KEYRING";
/// Variable names `loadout exec` stage 1 hands to stage 2 under `op run`.
pub const EXEC_ENV_VAR: &str = "LOADOUT_EXEC_ENV";
/// Prefix of every message `loadout exec` writes (stderr only).
pub const EXEC_PREFIX: &str = "loadout-exec:";

/// Per-project manifest directory (`<P>/.loadout/`).
pub const PROJECT_DIR: &str = ".loadout";
/// Prefix of git commit trailers written to the store (`Loadout-Op: ...`).
pub const TRAILER_PREFIX: &str = "Loadout-";
/// OS keyring service name.
pub const KEYRING_SERVICE: &str = "loadout";

/// Store commit author and committer name prefix (`loadout <machine-label>`).
pub const COMMIT_AUTHOR_NAME: &str = "loadout";
/// Domain of store commit e-mail addresses (`<machine-id>@loadout.invalid`).
pub const COMMIT_EMAIL_DOMAIN: &str = "loadout.invalid";
/// `Loadout-Client` trailer name of the CLI (`loadout-cli/<version>`).
pub const CLI_CLIENT_NAME: &str = "loadout-cli";

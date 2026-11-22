//! Tauri commands: thin wrappers over `agent-loadout-core` (SPEC 4.1). Every
//! value that crosses IPC is a typed struct or enum; display strings live in
//! the frontend catalog (`src/i18n/en.ts`), keyed by these enums and by
//! [`ErrorCode`].

use agent_loadout_core::paths::{self, HomeSource};
use agent_loadout_core::{CoreError, Env, ErrorCode, Os, RealFs};
use serde::Serialize;
use specta::Type;

/// Process-wide state the commands read. The environment is built once at
/// startup; a failure is kept and reported by every command that needs it.
pub struct AppState {
    env: Result<Env, CoreError>,
}

impl AppState {
    pub fn from_process() -> Self {
        Self {
            env: Env::from_process(),
        }
    }

    fn env(&self) -> Result<&Env, IpcError> {
        self.env.as_ref().map_err(|err| IpcError::from(err.clone()))
    }
}

/// A failed command: the stable code the frontend localizes from, plus
/// English diagnostic detail (never shown as state).
#[derive(Debug, Clone, Serialize, Type)]
pub struct IpcError {
    pub code: ErrorCode,
    pub message: String,
}

impl From<CoreError> for IpcError {
    fn from(err: CoreError) -> Self {
        Self {
            code: err.code(),
            message: err.message().to_owned(),
        }
    }
}

/// Operating system the app runs on; selects the window chrome (SPEC 13.1:
/// custom controls on Windows, native decorations on Linux, Overlay title
/// bar on macOS).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Windows,
    Linux,
    Macos,
}

impl From<Os> for Platform {
    fn from(os: Os) -> Self {
        match os {
            Os::Windows => Platform::Windows,
            Os::Linux => Platform::Linux,
            Os::MacOs => Platform::Macos,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    /// The app crate's version (`CARGO_PKG_VERSION`).
    pub version: String,
    pub platform: Platform,
}

/// Version and platform. Brand strings are constants in the bindings.
#[tauri::command]
#[specta::specta]
pub fn app_info() -> AppInfo {
    let os = if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    };
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        platform: os.into(),
    }
}

/// A configuration scope the sidebar switcher offers (SPEC 13.1). Projects
/// join in M16.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    /// User-wide configuration of every client.
    System,
}

#[tauri::command]
#[specta::specta]
pub fn list_scopes() -> Vec<Scope> {
    vec![Scope::System]
}

/// Where the data directory setting came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum DataHomeSource {
    /// The `--home` flag (CLI only; listed so every core source maps).
    Flag,
    /// The `LOADOUT_HOME` environment variable.
    EnvVar,
    /// The per-OS default (SPEC 4.5).
    Default,
}

impl From<HomeSource> for DataHomeSource {
    fn from(source: HomeSource) -> Self {
        match source {
            HomeSource::Flag => DataHomeSource::Flag,
            HomeSource::EnvVar => DataHomeSource::EnvVar,
            HomeSource::Default => DataHomeSource::Default,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DataHome {
    /// Absolute path, for display.
    pub path: String,
    pub source: DataHomeSource,
}

/// The resolved `LOADOUT_HOME` (SPEC 4.5). Reads nothing inside it.
#[tauri::command]
#[specta::specta]
pub fn data_home(state: tauri::State<'_, AppState>) -> Result<DataHome, IpcError> {
    let home = paths::loadout_home(state.env()?, &RealFs, None)?;
    Ok(DataHome {
        path: home.path.display().to_string(),
        source: home.source.into(),
    })
}

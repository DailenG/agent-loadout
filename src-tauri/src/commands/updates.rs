//! In-app updates (SPEC 16.4). The updater plugin is part of the app only
//! when the build's configuration carries `plugins.updater` (the release
//! job merges it in from repository variables; committed configuration has
//! no endpoint and no public key) and the app runs as the Windows NSIS
//! install or as an AppImage. AUR, deb, rpm, source and macOS builds never
//! have it. Settings > Updates turns checking off (`check_for_updates` in
//! `local-settings.toml`); with it off, or without the updater, nothing
//! here touches the network.

use std::sync::Mutex;

use agent_loadout_core::error::{CoreError, ErrorCode};
use agent_loadout_core::local_settings::LocalSettings;
use serde::Serialize;
use specta::Type;

use super::{AppState, CmdResult, Core, IpcError, blocking};

/// Whether this process runs with the updater plugin: a build configured
/// for it, on Windows, or on Linux inside an AppImage (`APPIMAGE` set by
/// the AppImage runtime).
pub fn updater_supported(configured: bool, windows: bool, appimage: bool) -> bool {
    configured && (windows || (cfg!(target_os = "linux") && appimage))
}

/// Process-wide updater state: whether the plugin is registered, and the
/// update the last check found (installed by [`install_update`]).
pub struct Updates {
    available: bool,
    pending: Mutex<Option<tauri_plugin_updater::Update>>,
}

impl Updates {
    pub fn new(available: bool) -> Self {
        Self {
            available,
            pending: Mutex::new(None),
        }
    }
}

/// What Settings > Updates shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdaterStatus {
    /// This build carries the updater (else the section says updates come
    /// from the package manager or the releases page).
    pub available: bool,
    /// The Settings switch; on unless turned off.
    pub enabled: bool,
}

/// A newer release the check found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOffer {
    pub version: String,
    pub current_version: String,
    /// Release notes from the update feed, if any.
    pub notes: Option<String>,
}

fn failed(message: impl Into<String>) -> IpcError {
    IpcError {
        code: ErrorCode::UpdateFailed,
        message: message.into(),
    }
}

fn load_settings(core: &Core) -> Result<LocalSettings, CoreError> {
    LocalSettings::load(core.fs.as_ref(), &core.home.state_dir())
}

/// The switch and whether the updater exists in this build.
pub fn status(core: &Core, available: bool) -> CmdResult<UpdaterStatus> {
    let settings = load_settings(core)?;
    Ok(UpdaterStatus {
        available,
        enabled: settings.check_for_updates.unwrap_or(true),
    })
}

/// Records the switch. Turning it on writes nothing but the setting; the
/// next check runs when the user asks or at the next start.
pub fn set_enabled(core: &Core, available: bool, enabled: bool) -> CmdResult<UpdaterStatus> {
    let state_dir = core.home.state_dir();
    let mut settings = load_settings(core)?;
    settings.check_for_updates = Some(enabled);
    settings.save(core.fs.as_ref(), &state_dir)?;
    status(core, available)
}

/// Refuses a check when the build has no updater or the switch is off.
pub fn ensure_may_check(status: UpdaterStatus) -> CmdResult<()> {
    if !status.available {
        return Err(failed(
            "this build has no updater; update through the package manager or the releases page",
        ));
    }
    if !status.enabled {
        return Err(failed("checking for updates is turned off in Settings"));
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn updater_status(
    state: tauri::State<'_, AppState>,
    updates: tauri::State<'_, Updates>,
) -> CmdResult<UpdaterStatus> {
    let available = updates.available;
    blocking(&state, move |core| status(core, available)).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_check_for_updates(
    state: tauri::State<'_, AppState>,
    updates: tauri::State<'_, Updates>,
    enabled: bool,
) -> CmdResult<UpdaterStatus> {
    let available = updates.available;
    blocking(&state, move |core| set_enabled(core, available, enabled)).await
}

/// Asks the update feed for a newer release. `None` when this is the
/// newest. Only runs when the build has the updater and the switch is on.
#[tauri::command]
#[specta::specta]
pub async fn check_for_update(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    updates: tauri::State<'_, Updates>,
) -> CmdResult<Option<UpdateOffer>> {
    use tauri_plugin_updater::UpdaterExt;

    let available = updates.available;
    let current = blocking(&state, move |core| status(core, available)).await?;
    ensure_may_check(current)?;
    let updater = app
        .updater()
        .map_err(|err| failed(format!("updater: {err}")))?;
    let found = updater
        .check()
        .await
        .map_err(|err| failed(format!("update check: {err}")))?;
    let offer = found.as_ref().map(|update| UpdateOffer {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: update.body.clone(),
    });
    *updates
        .pending
        .lock()
        .unwrap_or_else(|err| err.into_inner()) = found;
    Ok(offer)
}

/// Downloads the update the last check found, verifies its signature
/// against the build's public key, installs it and restarts the app. On
/// Windows the NSIS installer closes the app itself.
#[tauri::command]
#[specta::specta]
pub async fn install_update(
    app: tauri::AppHandle,
    updates: tauri::State<'_, Updates>,
) -> CmdResult<()> {
    let update = updates
        .pending
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .take()
        .ok_or_else(|| failed("no update is pending; check for updates first"))?;
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|err| failed(format!("update install: {err}")))?;
    app.restart();
}

#[cfg(test)]
mod tests {
    use agent_loadout_core::local_settings::LOCAL_SETTINGS_FILE;

    use super::*;
    use crate::commands::tests::Fx;

    #[test]
    fn the_updater_needs_a_configured_build_on_windows_or_an_appimage() {
        assert!(
            !updater_supported(false, true, false),
            "no feed, no updater"
        );
        assert!(!updater_supported(false, false, true));
        assert!(updater_supported(true, true, false));
        assert!(!updater_supported(true, false, false), "deb, rpm, AUR");
        assert_eq!(
            updater_supported(true, false, true),
            cfg!(target_os = "linux"),
            "AppImage only on Linux"
        );
    }

    #[test]
    fn checking_is_on_until_turned_off_and_the_switch_persists() {
        let fx = Fx::new();
        let file = fx.core.home.state_dir().join(LOCAL_SETTINGS_FILE);
        let initial = status(&fx.core, true).unwrap();
        assert_eq!(
            initial,
            UpdaterStatus {
                available: true,
                enabled: true
            }
        );
        assert!(!file.exists(), "reading the status writes nothing");

        let off = set_enabled(&fx.core, true, false).unwrap();
        assert!(!off.enabled);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("check_for_updates = false"), "{text}");
        assert!(!status(&fx.core, true).unwrap().enabled);

        assert!(set_enabled(&fx.core, true, true).unwrap().enabled);
    }

    #[test]
    fn no_check_runs_without_the_updater_or_with_the_switch_off() {
        let err = ensure_may_check(UpdaterStatus {
            available: false,
            enabled: true,
        })
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::UpdateFailed);
        let err = ensure_may_check(UpdaterStatus {
            available: true,
            enabled: false,
        })
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::UpdateFailed);
        assert!(
            ensure_may_check(UpdaterStatus {
                available: true,
                enabled: true
            })
            .is_ok()
        );
    }

    /// Committed configuration never carries an updater endpoint or key
    /// (the release job adds them), so local and package builds have none.
    #[test]
    fn committed_configs_have_no_updater_section() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !(name.starts_with("tauri.") && name.ends_with(".json")) {
                continue;
            }
            let config: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert!(
                config.pointer("/plugins/updater").is_none(),
                "{name} carries plugins.updater"
            );
            assert!(
                config.pointer("/bundle/createUpdaterArtifacts").is_none(),
                "{name} asks for updater artifacts"
            );
        }
    }
}

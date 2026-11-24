; Loadout's English installer strings (SPEC 16.1), used through
; bundle.windows.nsis.customLanguageFiles: Tauri 2.12.6's English.nsh with the
; upgrade page reworded. An upgrade over an older version is routine, and the
; Loadout data folder %LOCALAPPDATA%\Loadout is kept by either choice
; (windows/installer.nsi preselects "Upgrade in place" for upgrades).
; Keep every key the template uses; when upgrading @tauri-apps/cli, compare
; with that version's languages/English.nsh ($R4 and the WebView2 variables
; must match the template in windows/installer.nsi).
LangString addOrReinstall ${LANG_ENGLISH} "Add/Reinstall components"
LangString alreadyInstalled ${LANG_ENGLISH} "Already Installed"
LangString alreadyInstalledLong ${LANG_ENGLISH} "${PRODUCTNAME} ${VERSION} is already installed. Select the operation you want to perform and click Next to continue."
LangString appRunning ${LANG_ENGLISH} "{{product_name}} is running! Please close it first then try again."
LangString noPermissionToCloseApp ${LANG_ENGLISH} "Insufficient permissions to close {{product_name}}, please close it and try again"
LangString appRunningOkKill ${LANG_ENGLISH} "{{product_name}} is running!$\nClick OK to kill it"
LangString chooseMaintenanceOption ${LANG_ENGLISH} "Choose the maintenance option to perform."
LangString chooseHowToInstall ${LANG_ENGLISH} "Choose how to install ${PRODUCTNAME} ${VERSION}."
LangString createDesktop ${LANG_ENGLISH} "Create desktop shortcut"
LangString dontUninstall ${LANG_ENGLISH} "Upgrade in place (recommended)"
LangString dontUninstallDowngrade ${LANG_ENGLISH} "Do not uninstall (Downgrading without uninstall is disabled for this installer)"
LangString failedToKillApp ${LANG_ENGLISH} "Failed to kill {{product_name}}. Please close it first then try again"
LangString installingWebview2 ${LANG_ENGLISH} "Installing WebView2..."
LangString newerVersionInstalled ${LANG_ENGLISH} "A newer version of ${PRODUCTNAME} is already installed, and this installer has ${VERSION}. To go back to ${VERSION}, replace the installed version. Your Loadout data folder is kept either way. Choose an option and click Next."
LangString older ${LANG_ENGLISH} "older"
LangString olderOrUnknownVersionInstalled ${LANG_ENGLISH} "An $R4 version of ${PRODUCTNAME} is installed. Choose how to upgrade to ${VERSION}. Your Loadout data, settings and history are kept either way, and your AI clients' config files are not touched. Choose an option and click Next."
LangString silentDowngrades ${LANG_ENGLISH} "Downgrades are disabled for this installer, can't proceed with the silent installer, please use the graphical interface installer instead.$\n"
LangString unableToUninstall ${LANG_ENGLISH} "Unable to uninstall!"
LangString uninstallApp ${LANG_ENGLISH} "Uninstall ${PRODUCTNAME}"
LangString uninstallBeforeInstalling ${LANG_ENGLISH} "Replace the installed version (removes it first)"
LangString unknown ${LANG_ENGLISH} "unknown"
LangString webview2AbortError ${LANG_ENGLISH} "Failed to install WebView2! The app can't run without it. Try restarting the installer."
LangString webview2DownloadError ${LANG_ENGLISH} "Error: Downloading WebView2 Failed - $0"
LangString webview2DownloadSuccess ${LANG_ENGLISH} "WebView2 bootstrapper downloaded successfully"
LangString webview2Downloading ${LANG_ENGLISH} "Downloading WebView2 bootstrapper..."
LangString webview2InstallError ${LANG_ENGLISH} "Error: Installing WebView2 failed with exit code $1"
LangString webview2InstallSuccess ${LANG_ENGLISH} "WebView2 installed successfully"
LangString deleteAppData ${LANG_ENGLISH} "Delete the app's window cache (your Loadout data folder is kept)"

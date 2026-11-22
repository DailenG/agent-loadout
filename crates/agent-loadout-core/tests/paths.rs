use std::path::Path;

use agent_loadout_core::paths::{HomeSource, LoadoutHome, SyncRoot, SyncRootKind, loadout_home};
use agent_loadout_core::{Env, ErrorCode, Os};
use agent_loadout_testkit::TempHome;

fn resolve(th: &TempHome, env: &Env, flag: Option<&Path>) -> LoadoutHome {
    loadout_home(env, &th.fs(), flag).expect("resolve LOADOUT_HOME")
}

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"").unwrap();
}

#[test]
fn windows_default_is_local_app_data() {
    let th = TempHome::new();
    let home = resolve(&th, &th.env(Os::Windows), None);
    assert_eq!(home.path, th.local_appdata().join("Loadout"));
    assert_eq!(home.source, HomeSource::Default);
    assert_eq!(home.sync_root, None);
    assert_eq!(
        home.store_dir(),
        th.local_appdata().join("Loadout").join("store")
    );
}

#[test]
fn windows_ignores_home_and_xdg_vars() {
    let th = TempHome::new();
    let elsewhere = th.root().join("gitbash-home");
    let env = th
        .env_builder(Os::Windows)
        .var("HOME", &elsewhere)
        .var("XDG_DATA_HOME", &elsewhere)
        .build();
    assert_eq!(
        resolve(&th, &env, None).path,
        th.local_appdata().join("Loadout")
    );

    // Without the LocalAppData Known Folder the fallback is under the
    // profile (FOLDERID_Profile), still not under HOME.
    let env = Env::builder(Os::Windows, th.home())
        .var("HOME", &elsewhere)
        .build();
    assert_eq!(
        resolve(&th, &env, None).path,
        th.home().join("AppData").join("Local").join("Loadout")
    );
}

#[test]
fn linux_default_follows_xdg_data_home() {
    let th = TempHome::new();
    assert_eq!(
        resolve(&th, &th.env(Os::Linux), None).path,
        th.home().join(".local").join("share").join("loadout")
    );

    let xdg_data = th.xdg().join("data");
    let env = th
        .env_builder(Os::Linux)
        .var("XDG_DATA_HOME", &xdg_data)
        .build();
    assert_eq!(resolve(&th, &env, None).path, xdg_data.join("loadout"));

    let env = th
        .env_builder(Os::Linux)
        .var("XDG_DATA_HOME", "relative/data")
        .build();
    assert_eq!(
        resolve(&th, &env, None).path,
        th.home().join(".local").join("share").join("loadout")
    );
}

#[test]
fn macos_default_is_application_support() {
    let th = TempHome::new();
    assert_eq!(
        resolve(&th, &th.env(Os::MacOs), None).path,
        th.home()
            .join("Library")
            .join("Application Support")
            .join("Loadout")
    );
}

#[test]
fn loadout_home_var_overrides_default_and_flag_overrides_var() {
    let th = TempHome::new();
    let from_var = th.root().join("custom").join("data");
    // Windows variable names are case-insensitive.
    let env = th
        .env_builder(Os::Windows)
        .var(
            "loadout_home",
            th.root()
                .join("custom")
                .join(".")
                .join("x")
                .join("..")
                .join("data"),
        )
        .build();
    let home = resolve(&th, &env, None);
    assert_eq!(home.path, from_var);
    assert_eq!(home.source, HomeSource::EnvVar);

    let from_flag = th.root().join("flag");
    let home = resolve(&th, &env, Some(&from_flag));
    assert_eq!(home.path, from_flag);
    assert_eq!(home.source, HomeSource::Flag);

    let env = th.env_builder(Os::Linux).var("LOADOUT_HOME", "").build();
    assert_eq!(resolve(&th, &env, None).source, HomeSource::Default);
}

#[test]
fn relative_override_is_rejected() {
    let th = TempHome::new();
    let env = th
        .env_builder(Os::Linux)
        .var("LOADOUT_HOME", "data")
        .build();
    let err = loadout_home(&env, &th.fs(), None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidPath);
    assert_eq!(err.path(), Some(Path::new("data")));
}

#[test]
fn resilio_ancestor_is_sync_root_detected() {
    let th = TempHome::new();
    let sync = th.root().join("Sync");
    touch(&sync.join(".sync").join("ID"));
    let target = sync.join("tools").join("loadout");

    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let home = resolve(&th, &th.env(os), Some(&target));
        assert_eq!(
            home.sync_root,
            Some(SyncRoot {
                kind: SyncRootKind::Resilio,
                root: sync.clone()
            }),
            "{os:?}"
        );
    }
}

#[test]
fn resilio_needs_id_or_ignore_list() {
    let th = TempHome::new();
    let sync = th.root().join("Sync");
    std::fs::create_dir_all(sync.join(".sync")).unwrap();
    let target = sync.join("loadout");
    let env = th.env(Os::Linux);
    assert_eq!(resolve(&th, &env, Some(&target)).sync_root, None);

    touch(&sync.join(".sync").join("IgnoreList"));
    assert_eq!(
        resolve(&th, &env, Some(&target)).sync_root.map(|r| r.kind),
        Some(SyncRootKind::Resilio)
    );
}

#[test]
fn syncthing_folder_marker_is_detected() {
    let th = TempHome::new();
    let shared = th.home().join("Shared");
    std::fs::create_dir_all(shared.join(".stfolder")).unwrap();
    let home = resolve(&th, &th.env(Os::Linux), Some(&shared.join("loadout")));
    assert_eq!(
        home.sync_root,
        Some(SyncRoot {
            kind: SyncRootKind::Syncthing,
            root: shared
        })
    );
}

#[test]
fn onedrive_root_from_variable_is_detected() {
    let th = TempHome::new();
    let onedrive = th.home().join("OneDrive - Contoso");
    let env = th
        .env_builder(Os::Windows)
        .var("OneDriveCommercial", &onedrive)
        .build();
    let home = resolve(&th, &env, Some(&onedrive.join("Loadout")));
    assert_eq!(
        home.sync_root,
        Some(SyncRoot {
            kind: SyncRootKind::OneDrive,
            root: onedrive
        })
    );
    // The default stays outside it.
    assert_eq!(resolve(&th, &env, None).sync_root, None);
}

#[test]
fn dropbox_root_from_info_json_is_detected() {
    let th = TempHome::new();
    let dropbox = th.home().join("Dropbox (Personal)");
    let info = serde_json_string(&dropbox);
    let info_path = th.home().join(".dropbox").join("info.json");
    touch(&info_path);
    std::fs::write(&info_path, info).unwrap();

    let home = resolve(
        &th,
        &th.env(Os::Linux),
        Some(&dropbox.join("apps").join("loadout")),
    );
    assert_eq!(
        home.sync_root,
        Some(SyncRoot {
            kind: SyncRootKind::Dropbox,
            root: dropbox
        })
    );
}

#[test]
fn google_drive_mirror_folder_is_detected() {
    let th = TempHome::new();
    let drive = th.home().join("My Drive");
    let home = resolve(&th, &th.env(Os::Windows), Some(&drive.join("Loadout")));
    assert_eq!(
        home.sync_root,
        Some(SyncRoot {
            kind: SyncRootKind::GoogleDrive,
            root: drive
        })
    );
}

fn serde_json_string(dropbox_root: &Path) -> String {
    let escaped = dropbox_root
        .to_str()
        .unwrap()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!(r#"{{"personal": {{"path": "{escaped}", "host": 1, "is_team": false}}}}"#)
}

#[cfg(windows)]
#[test]
fn extended_paths_round_trip_through_plain() {
    use agent_loadout_core::paths::{extended, normalize, plain};
    let long_claude = r"\\?\C:\Users\me\.claude.json";
    for (path, long) in [
        (r"C:\Users\me\.claude.json", long_claude),
        (r"C:/Users/me/./a/../.claude.json", long_claude),
        (r"C:\", r"\\?\C:\"),
        (r"\\srv\share\f.json", r"\\?\UNC\srv\share\f.json"),
    ] {
        let made = extended(Path::new(path));
        assert_eq!(made.as_os_str(), long, "{path}");
        assert_eq!(plain(&made).as_ref(), normalize(Path::new(path)));
    }
    // Relative and already-extended paths are left alone.
    assert_eq!(extended(Path::new(r"a\b")).as_os_str(), r"a\b");
    assert_eq!(extended(Path::new(r"\\?\C:\x")).as_os_str(), r"\\?\C:\x");
    assert_eq!(plain(Path::new(r"C:\x")).as_os_str(), r"C:\x");
}

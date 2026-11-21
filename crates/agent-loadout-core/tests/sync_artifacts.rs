//! Sync tool artifact names (SPEC 9.6 table): reported, never read, never
//! deleted. Names of the folder remote grammar are never artifacts.

use agent_loadout_core::remote::sync_artifacts::artifact_tool;

#[test]
fn known_conflict_and_temp_names_are_artifacts() {
    let table = [
        ("machine (conflicted copy 2026-01-01).json", "Dropbox"),
        ("machine (Case Conflict).json", "Dropbox"),
        ("machine (Selective Sync Conflict).json", "Dropbox"),
        (".dropbox", "Dropbox"),
        (".dropbox.attr", "Dropbox"),
        (".dropbox.cache", "Dropbox"),
        ("v1.sync-conflict-20260101-000000-ABCDEFG", "Syncthing"),
        (
            "machine.sync-conflict-20260101-000000-ABCDEFG.json",
            "Syncthing",
        ),
        (".syncthing.machine.json.tmp", "Syncthing"),
        ("~syncthing~machine.json.tmp", "Syncthing"),
        (".stfolder", "Syncthing"),
        (".stversions", "Syncthing"),
        (".stignore", "Syncthing"),
        ("01JCX5B7K3M9N1P5Q7R3S9T1V5.Conflict", "Resilio Sync"),
        ("machine.Conflict.json", "Resilio Sync"),
        ("machine.Conflict1", "Resilio Sync"),
        ("machine.json.!sync", "Resilio Sync"),
        ("store.rsls", "Resilio Sync"),
        (".sync", "Resilio Sync"),
        ("machine (1).json", "Google Drive"),
        ("store (12)", "Google Drive"),
        (".tmp.drivedownload", "Google Drive"),
        (".tmp.driveupload", "Google Drive"),
        (".DS_Store", "OS"),
        ("Thumbs.db", "OS"),
        ("desktop.ini", "OS"),
        ("~$notes.txt", "OS"),
    ];
    for (name, tool) in table {
        assert_eq!(artifact_tool(name), Some(tool), "{name}");
    }
}

#[test]
fn folder_remote_names_are_not_artifacts() {
    for name in [
        "01JCX5B7K3M9N1P5Q7R3S9T1V5",
        "machine.json",
        "store.git",
        "latest",
        "archives",
        "EXPORT.json",
        "loadout-20260101-000000-0123456789ab.zip",
        "HEAD",
        "objects",
        "refs",
        "main",
        "pre-reset",
        "auto",
        "servers",
        "github.toml",
        "notes (draft)",
        "(1)",
        // OneDrive's `name-<computer>.ext` copies have no fixed pattern; the
        // readers' allowlist skips them without a report.
        "machine-DESKTOP.json",
    ] {
        assert_eq!(artifact_tool(name), None, "{name}");
    }
}

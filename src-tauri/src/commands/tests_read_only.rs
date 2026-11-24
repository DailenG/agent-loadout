//! Read-only contexts in the app's data (M37, #4): files this account
//! cannot write carry `readOnly` with their kind, a project whose files are
//! all read only is read only as a whole, and the drawer blocks the writes
//! into them with `read_only_target`.

use std::path::Path;

use agent_loadout_core::ErrorCode;
use agent_loadout_core::access::ReadOnlyKind;

use super::tests::Fx;
use super::{EntryStateKind, ReadOnly, apply, projects, tools};

fn set_readonly(path: &Path, readonly: bool) {
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(readonly);
    std::fs::set_permissions(path, permissions).unwrap();
}

/// Root on Unix writes through a read-only mode: nothing to test then.
fn writes_anyway(path: &Path) -> bool {
    std::fs::OpenOptions::new().write(true).open(path).is_ok()
}

const ATTRIBUTE: Option<ReadOnly> = Some(ReadOnly {
    kind: ReadOnlyKind::Attribute,
});

#[test]
fn tools_cards_mark_a_read_only_user_file() {
    let fx = Fx::new();
    let file = fx.claude();
    std::fs::write(&file, "{}\n").unwrap();
    set_readonly(&file, true);
    if writes_anyway(&file) {
        set_readonly(&file, false);
        return;
    }
    let list = tools::list(&fx.core).unwrap();
    set_readonly(&file, false);
    let claude = list.iter().find(|tool| tool.id == "claude-code").unwrap();
    assert_eq!(claude.user_files[0].read_only, ATTRIBUTE);
    assert!(!claude.read_only, "the client is still one Loadout writes");
    let codex = list.iter().find(|tool| tool.id == "codex").unwrap();
    assert!(codex.user_files.iter().all(|file| file.read_only.is_none()));
    let json = serde_json::to_value(&claude.user_files[0]).unwrap();
    assert_eq!(json["readOnly"], serde_json::json!({ "kind": "attribute" }));
}

/// A permission change alone keeps the file's content, so the cached probe
/// stays until Refresh rechecks (#23); then the new state shows.
#[test]
fn refresh_rechecks_a_permission_change_without_a_content_change() {
    let fx = Fx::new();
    let file = fx.claude();
    std::fs::write(&file, "{}\n").unwrap();
    let claude = |list: &[tools::ToolInfo]| {
        list.iter()
            .find(|tool| tool.id == "claude-code")
            .unwrap()
            .user_files[0]
            .read_only
    };
    assert_eq!(claude(&tools::list(&fx.core).unwrap()), None);

    set_readonly(&file, true);
    if writes_anyway(&file) {
        set_readonly(&file, false);
        return;
    }
    let cached = tools::list(&fx.core).unwrap();
    apply::recheck(&fx.core);
    let rechecked = tools::list(&fx.core).unwrap();
    set_readonly(&file, false);
    assert_eq!(claude(&cached), None, "same content: the cached probe");
    assert_eq!(claude(&rechecked), ATTRIBUTE);

    // And back: writable again after the next recheck.
    apply::recheck(&fx.core);
    assert_eq!(claude(&tools::list(&fx.core).unwrap()), None);
}

#[test]
fn a_project_whose_files_are_all_read_only_is_read_only() {
    let fx = Fx::new();
    drop(fx.store());
    let root = fx.th.projects().join("locked");
    std::fs::create_dir_all(&root).unwrap();
    projects::init(&fx.core, &root, true, Some("locked")).unwrap();
    let manifest = root.join(".loadout/project.toml");
    std::fs::write(
        &manifest,
        format!(
            "{}\n[defaults]\nclients = [\"claude-code\"]\n\n[servers.docs]\ntransport = \"stdio\"\n\
             command = \"docs-mcp\"\n",
            std::fs::read_to_string(&manifest).unwrap()
        ),
    )
    .unwrap();
    let mcp = root.join(".mcp.json");
    std::fs::write(&mcp, "{\n  \"mcpServers\": {}\n}\n").unwrap();
    let rows = projects::list(&fx.core).unwrap();
    let id = rows[0].id.clone();
    // Trust the edited manifest, so the project plans its files.
    let review = projects::review(&fx.core, &id).unwrap();
    projects::trust(&fx.core, &id, &review.hash).unwrap();

    set_readonly(&manifest, true);
    if writes_anyway(&manifest) {
        set_readonly(&manifest, false);
        return;
    }
    // Only the manifest: partly read only.
    fx.core.plan_cache.invalidate();
    let rows = projects::list(&fx.core).unwrap();
    let row = rows.iter().find(|row| row.id == id).unwrap();
    assert_eq!(row.read_only, None, "{row:#?}");
    let files: Vec<&str> = row
        .read_only_files
        .iter()
        .map(|f| f.file.as_str())
        .collect();
    assert_eq!(files, ["<P>/.loadout/project.toml"]);

    // The client file too: the whole project, and the drawer blocks it.
    // (The trust's apply wrote `docs` there; deleting it by hand leaves a
    // re-create pending.)
    std::fs::write(&mcp, "{\n  \"mcpServers\": {}\n}\n").unwrap();
    set_readonly(&mcp, true);
    fx.core.plan_cache.invalidate();
    let rows = projects::list(&fx.core).unwrap();
    let pending = apply::pending(&fx.core).unwrap();
    set_readonly(&mcp, false);
    set_readonly(&manifest, false);
    let row = rows.iter().find(|row| row.id == id).unwrap();
    assert_eq!(row.read_only, ATTRIBUTE, "{row:#?}");
    assert_eq!(row.read_only_files.len(), 2);
    let json = serde_json::to_value(row).unwrap();
    assert_eq!(json["readOnly"], serde_json::json!({ "kind": "attribute" }));
    assert_eq!(
        json["readOnlyFiles"][0],
        serde_json::json!({ "file": "<P>/.loadout/project.toml", "kind": "attribute" })
    );
    let file = pending
        .files
        .iter()
        .find(|file| file.file == "<P>/.mcp.json")
        .unwrap_or_else(|| panic!("{:#?}", pending.files));
    assert_eq!(file.read_only, ATTRIBUTE);
    assert!(file.diff.is_empty());
    let docs = file.entries.iter().find(|e| e.note.name == "docs").unwrap();
    assert_eq!(docs.note.state, EntryStateKind::Blocked);
    assert_eq!(
        docs.note.problem.as_ref().unwrap().code,
        ErrorCode::ReadOnlyTarget
    );
}

/// Makes `folder` refuse new files for everyone (Windows: a deny ACE for
/// Everyone through `icacls`; Unix: mode 0555). `false` when this process
/// writes anyway (root).
fn refuse_new_files(folder: &Path) -> bool {
    if cfg!(windows) {
        let status = std::process::Command::new("icacls")
            .arg(folder)
            .args(["/deny", "*S-1-1-0:(WD,AD)"])
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        true
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        std::fs::write(folder.join(".writes-anyway"), "").is_err()
    }
}

fn allow_new_files(folder: &Path) {
    if cfg!(windows) {
        let _ = std::process::Command::new("icacls")
            .arg(folder)
            .args(["/remove:d", "*S-1-1-0"])
            .stdout(std::process::Stdio::null())
            .status();
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o755));
        }
        let _ = std::fs::remove_file(folder.join(".writes-anyway"));
    }
}

/// A shared project whose private manifest does not exist yet and cannot
/// be created: Import's "add to manifest" and the private layer read it
/// from `readOnlyFiles`.
#[test]
fn a_missing_private_manifest_in_a_read_only_folder_is_listed() {
    let fx = Fx::new();
    drop(fx.store());
    let root = fx.th.projects().join("shared");
    std::fs::create_dir_all(&root).unwrap();
    projects::init(&fx.core, &root, true, Some("shared")).unwrap();
    let dir = root.join(".loadout");
    assert!(!dir.join("project.local.toml").exists());
    if !refuse_new_files(&dir) {
        allow_new_files(&dir);
        return;
    }
    fx.core.plan_cache.invalidate();
    let rows = projects::list(&fx.core).unwrap();
    allow_new_files(&dir);
    let files: Vec<&str> = rows[0]
        .read_only_files
        .iter()
        .map(|file| file.file.as_str())
        .collect();
    assert!(
        files.contains(&"<P>/.loadout/project.local.toml"),
        "{files:?}"
    );
}

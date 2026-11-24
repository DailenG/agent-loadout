//! Skill folder entries in a folder this account cannot write (SPEC 8.2
//! `read_only_target`, M37 review): removing or re-pointing a link,
//! removing a copy and moving a folder aside (adopt) probe the folder that
//! holds the entry, never the entry (a link leads into the always-writable
//! store); creating an entry asks for a new subfolder, not a new file. Each
//! test runs in its own temp folder with the real file system.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::access::{ReadOnlyKind, WriteAccess};
use crate::error::ErrorCode;
use crate::fs::RealFs;
use crate::skills::link::remove_link;
use crate::skills::{new_folder_access, removal_error};

static REAL: RealFs = RealFs;

/// A temp folder with `store/skill` (a file inside) and `skills/` holding
/// a link to it and a copy of it; removed on drop.
struct Fx {
    root: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        unlock(&self.skills());
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fx {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "loadout-ro-skills-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let store = root.join("store").join("skill");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join("SKILL.md"), "# skill\n").unwrap();
        let fx = Self { root };
        std::fs::create_dir_all(fx.skills()).unwrap();
        link(&store, &fx.link());
        std::fs::create_dir_all(fx.copy()).unwrap();
        std::fs::write(fx.copy().join("SKILL.md"), "# skill\n").unwrap();
        fx
    }

    fn skills(&self) -> PathBuf {
        self.root.join("skills")
    }

    fn link(&self) -> PathBuf {
        self.skills().join("linked")
    }

    fn copy(&self) -> PathBuf {
        self.skills().join("copied")
    }
}

#[cfg(windows)]
fn link(target: &Path, at: &Path) {
    junction::create(target, at).unwrap();
}

#[cfg(unix)]
fn link(target: &Path, at: &Path) {
    std::os::unix::fs::symlink(target, at).unwrap();
}

/// Makes `folder` and what it holds read only for this account, as a
/// managed folder is: Windows, a protected DACL granting the owner only
/// read and execute, inherited by the entries; Unix, mode 0555. `false`
/// when this process writes anyway (root).
#[cfg(windows)]
fn lock(folder: &Path) -> bool {
    set_dacl(folder, "D:P(A;OICI;0x1200a9;;;{owner})");
    true
}

#[cfg(windows)]
fn unlock(folder: &Path) {
    if folder.exists() {
        set_dacl(folder, "D:P(A;OICI;FA;;;{owner})");
    }
}

/// Sets `template` (with `{owner}` replaced by the folder's owner) as the
/// DACL of `folder`, propagated to what it holds. The owner keeps the
/// right to change it back.
#[cfg(windows)]
fn set_dacl(folder: &Path, template: &str) {
    use windows_permissions::constants::{SeObjectType, SecurityInformation};
    use windows_permissions::{LocalBox, SecurityDescriptor, wrappers};
    let sd = wrappers::GetNamedSecurityInfo(
        folder.as_os_str(),
        SeObjectType::SE_FILE_OBJECT,
        SecurityInformation::Owner,
    )
    .unwrap();
    let owner = sd.owner().unwrap().to_string();
    let wanted: LocalBox<SecurityDescriptor> = template.replace("{owner}", &owner).parse().unwrap();
    wrappers::SetNamedSecurityInfo(
        folder.as_os_str(),
        SeObjectType::SE_FILE_OBJECT,
        SecurityInformation::Dacl | SecurityInformation::ProtectedDacl,
        None,
        None,
        Some(wanted.dacl().unwrap()),
        None,
    )
    .unwrap();
}

#[cfg(unix)]
fn lock(folder: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    std::fs::write(folder.join(".writes-anyway"), "").is_err()
}

#[cfg(unix)]
fn unlock(folder: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o755));
    let _ = std::fs::remove_file(folder.join(".writes-anyway"));
}

fn assert_read_only(result: Result<(), crate::error::CoreError>, path: &Path) {
    let err = result.unwrap_err();
    assert_eq!(err.code(), ErrorCode::ReadOnlyTarget, "{err:?}");
    assert!(err.read_only().is_some());
    assert_eq!(err.path(), Some(path));
}

/// Retiring a link and re-pointing it both start with removing it.
#[test]
fn removing_a_link_in_a_read_only_folder_is_read_only() {
    let fx = Fx::new();
    if !lock(&fx.skills()) {
        return;
    }
    assert_read_only(remove_link(&REAL, &fx.link()), &fx.link());
    assert!(fx.link().exists());
}

#[test]
fn removing_a_copy_in_a_read_only_folder_is_read_only() {
    let fx = Fx::new();
    if !lock(&fx.skills()) {
        return;
    }
    let path = fx.copy();
    let removed = std::fs::remove_dir_all(&path)
        .map_err(|err| removal_error(&REAL, "cannot remove folder", &path, &err));
    assert_read_only(removed, &path);
}

/// Adopt's first step moves the folder aside in its own folder.
#[test]
fn moving_a_folder_aside_in_a_read_only_folder_is_read_only() {
    let fx = Fx::new();
    if !lock(&fx.skills()) {
        return;
    }
    let path = fx.copy();
    let aside = fx.skills().join(".copied.loadout-adopt");
    let moved = std::fs::rename(&path, &aside)
        .map_err(|err| removal_error(&REAL, "cannot move the folder aside", &path, &err));
    assert_read_only(moved, &path);
}

#[test]
fn a_new_entry_needs_a_new_subfolder() {
    let fx = Fx::new();
    let entry = fx.skills().join("new-skill");
    assert_eq!(new_folder_access(&REAL, &entry), WriteAccess::Writable);
    assert!(!entry.exists(), "the probe creates nothing");
    if !lock(&fx.skills()) {
        return;
    }
    assert!(matches!(
        new_folder_access(&REAL, &entry),
        WriteAccess::ReadOnly(ReadOnlyKind::Acl | ReadOnlyKind::Attribute)
    ));
}

/// A folder that allows new files but not new subfolders: junctions and
/// copies cannot be made there, though a file could.
#[cfg(windows)]
#[test]
fn a_folder_without_add_subdirectory_blocks_new_entries() {
    let fx = Fx::new();
    // FILE_ADD_SUBDIRECTORY denied, nothing inherited.
    set_dacl(&fx.skills(), "D:P(D;;0x4;;;{owner})(A;OICI;FA;;;{owner})");
    let entry = fx.skills().join("new-skill");
    assert_eq!(
        crate::access::probe_path(&REAL, &entry),
        WriteAccess::Writable
    );
    assert_eq!(
        new_folder_access(&REAL, &entry),
        WriteAccess::ReadOnly(ReadOnlyKind::Acl)
    );
    assert!(!entry.exists());
}

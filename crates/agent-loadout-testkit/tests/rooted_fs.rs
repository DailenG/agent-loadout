use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use agent_loadout_core::Fs;
use agent_loadout_testkit::TempHome;

fn assert_panics(what: &str, f: impl FnOnce()) {
    let result = panic::catch_unwind(AssertUnwindSafe(f));
    assert!(result.is_err(), "{what} did not panic");
}

/// A sibling of the temp root: real, outside the root, unique per test.
fn outside(th: &TempHome) -> PathBuf {
    let name = th.root().file_name().unwrap().to_string_lossy();
    th.root().parent().unwrap().join(format!("{name}-outside"))
}

#[test]
fn access_inside_root_works() {
    let th = TempHome::new();
    let fs = th.fs();
    let dir = th.home().join(".config").join("tool");
    fs.create_dir_all(&dir).unwrap();
    fs.write(&dir.join("a.json"), b"{}").unwrap();

    assert!(fs.is_dir(&dir));
    assert!(fs.exists(&dir.join("a.json")));
    assert_eq!(fs.read_to_string(&dir.join("a.json")).unwrap(), "{}");
    assert_eq!(fs.root(), Some(th.root()));
}

#[test]
fn access_outside_root_panics_without_touching_disk() {
    let th = TempHome::new();
    let fs = th.fs();
    let outside = outside(&th);
    let escape = th
        .home()
        .join("..")
        .join("..")
        .join(outside.file_name().unwrap());

    assert_panics("exists", || {
        fs.exists(&outside);
    });
    assert_panics("is_dir", || {
        fs.is_dir(&outside);
    });
    assert_panics("read_to_string", || {
        let _ = fs.read_to_string(&outside.join("config.json"));
    });
    assert_panics("write", || {
        let _ = fs.write(&outside, b"x");
    });
    assert_panics("create_dir_all", || {
        let _ = fs.create_dir_all(&outside.join("nested"));
    });
    assert_panics("remove_file", || {
        let _ = fs.remove_file(&outside.join("config.json"));
    });
    assert_panics("read_dir", || {
        let _ = fs.read_dir(&outside);
    });
    assert_panics("open_lock_file", || {
        let _ = fs.open_lock_file(&outside);
    });
    assert_panics("write through ..", || {
        let _ = fs.write(&escape, b"x");
    });
    assert_panics("relative path", || {
        fs.exists(Path::new("home"));
    });

    assert!(!outside.exists(), "a rejected write reached the disk");
}

//! M35 review follow-up: removing the backups that hold old secret values.
//! The zeros are written in place (never through a truncating write), a
//! zeroed backup is recorded as removed even when it cannot be deleted (and
//! `restore --files` never writes it back), and the run result on disk is
//! never trusted to name what to overwrite: a recorded path outside this
//! run's folder, under another file's name, or a link is left alone.

mod support;

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use agent_loadout_core::adapters::detect::NoRegistry;
use agent_loadout_core::apply::journal::APPLIES_DIR;
use agent_loadout_core::apply::{ApplyEnv, backup};
use agent_loadout_core::secrets::SecretValue;
use agent_loadout_core::store::restore::{FileOutcome, restore_files};
use agent_loadout_core::{ErrorCode, Fs, RealFs};
use agent_loadout_testkit::{RootedFs, TempHome};
use support::{Fx, stdio};

const OLD_VALUE: &str = "planted-old-value-9e41b7";

/// `~/.claude.json` holding the value and `~/.omp/agent/mcp.json` without
/// it, both written by one apply; the claude backup is marked. Returns the
/// apply id.
fn applied_with_old_value(fx: &Fx) -> String {
    fx.write(
        &fx.claude(),
        &format!("{{\n  \"note\": \"{OLD_VALUE}\",\n  \"mcpServers\": {{}}\n}}\n"),
    );
    fx.write(&fx.omp(), "{\n  \"mcpServers\": {}\n}\n");
    fx.put_server(stdio(
        "notes",
        &["claude-code", "omp"],
        &["-y", "@example/notes-mcp"],
    ));
    let (report, _) = fx.apply();
    let marked = backup::mark_old_values(
        &fx.fs,
        &fx.home.state_dir(),
        &report.apply_id,
        &[SecretValue::new(OLD_VALUE)],
    )
    .unwrap();
    assert_eq!(marked, [fx.claude()]);
    report.apply_id
}

/// The backup of `file` in the run of `apply_id`.
fn backup_of(fx: &Fx, apply_id: &str, file: &Path) -> PathBuf {
    let runs = backup::list_runs(&fx.fs, &fx.home).unwrap();
    let run = runs.iter().find(|run| run.apply_id == apply_id).unwrap();
    fx.home
        .backups_dir()
        .join(backup::FILES_DIR)
        .join(&run.dir)
        .join(backup::encode_path(file))
}

/// Points the run result's record of `file` at `backup`.
fn tamper(fx: &Fx, apply_id: &str, file: &Path, backup: &Path) {
    let path = fx
        .home
        .state_dir()
        .join(APPLIES_DIR)
        .join(format!("{apply_id}.json"));
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for record in doc["files"].as_array_mut().unwrap() {
        if record["file"] == file.to_string_lossy().as_ref() {
            record["backup"] = serde_json::Value::String(backup.to_string_lossy().into_owned());
        }
    }
    std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
}

fn ax_with<'a>(fx: &'a Fx, fs: &'a dyn Fs) -> ApplyEnv<'a> {
    ApplyEnv {
        env: &fx.env,
        fs,
        home: &fx.home,
        registry: &NoRegistry,
        delivery: &fx.delivery,
    }
}

#[test]
fn overwrite_with_zeros_writes_in_place_and_creates_nothing() {
    let th = TempHome::new();
    let path = th.root().join("backup.bin");
    std::fs::write(&path, vec![0x5a_u8; 200_003]).unwrap();
    RealFs.overwrite_with_zeros(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len(), 200_003, "the length is unchanged");
    assert!(bytes.iter().all(|byte| *byte == 0));

    let missing = th.root().join("missing.bin");
    assert!(RealFs.overwrite_with_zeros(&missing).is_err());
    assert!(!missing.exists(), "nothing is created");
    let folder = th.root().join("folder");
    std::fs::create_dir(&folder).unwrap();
    assert!(RealFs.overwrite_with_zeros(&folder).is_err());
    assert!(RealFs.is_regular_file(&path));
    assert!(!RealFs.is_regular_file(&folder));
    assert!(!RealFs.is_regular_file(&missing));
}

/// A view where no backup can be written through a truncating write or
/// deleted: only an in-place overwrite reaches it.
struct StuckBackups {
    inner: RootedFs,
    backups: PathBuf,
}

impl StuckBackups {
    fn refuse(&self, path: &Path, what: &str) -> io::Result<()> {
        if path.starts_with(&self.backups) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{what} refused in this test"),
            ));
        }
        Ok(())
    }
}

impl Fs for StuckBackups {
    fn root(&self) -> Option<&Path> {
        self.inner.root()
    }
    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }
    fn is_dir(&self, path: &Path) -> bool {
        self.inner.is_dir(path)
    }
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.inner.read_to_string(path)
    }
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read(path)
    }
    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.refuse(path, "a truncating write")?;
        self.inner.write(path, contents)
    }
    fn write_new(&self, path: &Path, contents: &[u8], like: Option<&Path>) -> io::Result<()> {
        self.inner.write_new(path, contents, like)
    }
    fn replace(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.inner.replace(from, to)
    }
    fn write_private(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.refuse(path, "a truncating write")?;
        self.inner.write_private(path, contents)
    }
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.create_dir_all(path)
    }
    fn create_private_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.create_private_dir_all(path)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.refuse(path, "a delete")?;
        self.inner.remove_file(path)
    }
    fn is_regular_file(&self, path: &Path) -> bool {
        self.inner.is_regular_file(path)
    }
    fn overwrite_with_zeros(&self, path: &Path) -> io::Result<()> {
        self.inner.overwrite_with_zeros(path)
    }
    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        self.refuse(path, "a delete")?;
        self.inner.remove_dir_all(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        self.inner.read_dir(path)
    }
    fn open_lock_file(&self, path: &Path) -> io::Result<File> {
        self.inner.open_lock_file(path)
    }
}

/// The zeros land in the backup itself (its length unchanged), the backup
/// counts as removed as soon as they do, and a zeroed backup that cannot be
/// deleted is never written back over the client file.
#[test]
fn a_zeroed_backup_that_cannot_be_deleted_is_removed_and_never_restored() {
    let fx = Fx::new();
    let apply_id = applied_with_old_value(&fx);
    let saved = backup_of(&fx, &apply_id, &fx.claude());
    let len = std::fs::metadata(&saved).unwrap().len();
    let stuck = StuckBackups {
        inner: fx.fs.clone(),
        backups: fx.home.backups_dir(),
    };

    let purge = backup::purge_old_values(&ax_with(&fx, &stuck), &apply_id).unwrap();
    assert!(purge.failed.is_empty(), "{:?}", purge.failed);
    assert_eq!(purge.removed, [fx.claude()]);
    assert_eq!(purge.not_deleted.len(), 1);
    assert_eq!(purge.not_deleted[0].0, fx.claude());
    let bytes = std::fs::read(&saved).unwrap();
    assert_eq!(bytes.len() as u64, len, "overwritten in place");
    assert!(bytes.iter().all(|byte| *byte == 0), "every byte is zero");
    assert_eq!(
        backup::removed_backups(&fx.fs, &fx.home.state_dir(), &apply_id).unwrap(),
        [fx.claude()]
    );

    let before = fx.read(&fx.claude());
    let results = restore_files(&fx.ax(), &apply_id, Some(&fx.claude())).unwrap();
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].outcome, FileOutcome::Removed));
    assert_eq!(fx.read(&fx.claude()), before, "no zeros written back");
    let all = restore_files(&fx.ax(), &apply_id, None).unwrap();
    assert!(
        all.iter()
            .any(|result| result.file == fx.claude()
                && matches!(result.outcome, FileOutcome::Removed))
    );
    assert_eq!(fx.read(&fx.claude()), before);
}

/// A recorded backup outside this run's folder, in it under another
/// file's name, or in another run's folder is left alone and reported.
#[test]
fn a_tampered_run_result_never_names_what_to_overwrite() {
    let fx = Fx::new();
    let apply_id = applied_with_old_value(&fx);
    let claude_before = std::fs::read(fx.claude()).unwrap();
    let omp_backup = backup_of(&fx, &apply_id, &fx.omp());
    let omp_backup_before = std::fs::read(&omp_backup).unwrap();
    let elsewhere = fx
        .home
        .backups_dir()
        .join(backup::FILES_DIR)
        .join("20200101T000000Z-01OTHERAPPLY00000000000000")
        .join(backup::encode_path(&fx.claude()));
    std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
    std::fs::write(&elsewhere, OLD_VALUE).unwrap();

    for target in [fx.claude(), omp_backup.clone(), elsewhere.clone()] {
        tamper(&fx, &apply_id, &fx.claude(), &target);
        let purge = backup::purge_old_values(&fx.ax(), &apply_id).unwrap();
        assert!(purge.removed.is_empty(), "{target:?}: {purge:?}");
        assert_eq!(purge.failed.len(), 1, "{target:?}");
        assert_eq!(purge.failed[0].0, fx.claude());
        assert_eq!(purge.failed[0].1.code(), ErrorCode::InvalidValue);
    }
    assert_eq!(std::fs::read(fx.claude()).unwrap(), claude_before);
    assert_eq!(std::fs::read(&omp_backup).unwrap(), omp_backup_before);
    assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), OLD_VALUE);
    assert!(
        backup::removed_backups(&fx.fs, &fx.home.state_dir(), &apply_id)
            .unwrap()
            .is_empty()
    );
}

/// The backup replaced by a link (a symbolic link to the client file on
/// Unix, a junction to a folder on Windows): left alone, never followed.
#[test]
fn a_backup_that_is_a_link_is_left_alone() {
    let fx = Fx::new();
    let apply_id = applied_with_old_value(&fx);
    let saved = backup_of(&fx, &apply_id, &fx.claude());
    std::fs::remove_file(&saved).unwrap();
    let target = link(&fx, &saved);
    let target_before = std::fs::read(&target).unwrap();

    let purge = backup::purge_old_values(&fx.ax(), &apply_id).unwrap();
    assert!(purge.removed.is_empty(), "{purge:?}");
    assert_eq!(purge.failed.len(), 1);
    assert_eq!(purge.failed[0].1.code(), ErrorCode::InvalidValue);
    assert_eq!(std::fs::read(&target).unwrap(), target_before);
    assert!(std::fs::symlink_metadata(&saved).is_ok(), "the link stays");
}

/// Makes `at` a symbolic link to the client file; returns the file a
/// follower would overwrite.
#[cfg(unix)]
fn link(fx: &Fx, at: &Path) -> PathBuf {
    std::os::unix::fs::symlink(fx.claude(), at).unwrap();
    fx.claude()
}

/// Makes `at` a junction to a folder holding a file; returns that file.
#[cfg(windows)]
fn link(fx: &Fx, at: &Path) -> PathBuf {
    let folder = fx.th.root().join("junction-target");
    std::fs::create_dir_all(&folder).unwrap();
    let inside = folder.join("inside.txt");
    std::fs::write(&inside, OLD_VALUE).unwrap();
    junction::create(&folder, at).unwrap();
    inside
}

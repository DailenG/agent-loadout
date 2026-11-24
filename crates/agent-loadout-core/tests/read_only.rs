//! Read-only targets (SPEC 8.2 `read_only_target`, #4): the write probe
//! blocks writes into files this account cannot write (read-only attribute
//! or mode, deny-write ACL, read-only folder), never modifies or creates
//! what it probes, keeps a sharing violation apart, and gives up in time;
//! a write the OS refuses is mapped to `read_only_target` and the run goes
//! on with the other targets.

mod support;

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use agent_loadout_core::access::{
    self, ProbeCache, ReadOnlyKind, WriteAccess, WriteProbe, fingerprint, probe_files,
};
use agent_loadout_core::adapters::detect::NoRegistry;
use agent_loadout_core::apply::cache::PlanCache;
use agent_loadout_core::apply::{
    self, Action, ApplyEnv, ApplyRequest, ApplyStatus, EntryState, PlanOptions,
};
use agent_loadout_core::model::applied::{ApplyOutcome, ApplyTrigger};
use agent_loadout_core::store::Store;
use agent_loadout_core::{ErrorCode, Fs};
use agent_loadout_testkit::RootedFs;
use support::{Fx, stdio};

const NOTES_ARGS: [&str; 2] = ["-y", "@example/notes-mcp"];
const EMPTY: &str = "{\n  \"mcpServers\": {}\n}\n";

static NO_REGISTRY: NoRegistry = NoRegistry;

/// Bytes and modification time of a file, to prove a probe left it alone.
fn snapshot(path: &Path) -> (Vec<u8>, SystemTime) {
    (
        std::fs::read(path).unwrap(),
        std::fs::metadata(path).unwrap().modified().unwrap(),
    )
}

fn set_readonly(path: &Path, readonly: bool) {
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(readonly);
    std::fs::set_permissions(path, permissions).unwrap();
}

/// Whether this process writes through mode bits anyway (root on Unix):
/// then a read-only mode proves nothing and the test is skipped.
fn writes_anyway(path: &Path) -> bool {
    std::fs::OpenOptions::new().write(true).open(path).is_ok()
}

/// The probe of `path` through the confined test view.
fn probe(fx: &Fx, path: &Path) -> WriteAccess {
    access::probe_path(&fx.fs, path)
}

/// Claude Code's file (read only in the tests) and omp's (writable), both
/// with a server to write.
fn two_targets() -> Fx {
    let fx = Fx::new();
    fx.write(&fx.claude(), EMPTY);
    fx.put_server(stdio("notes", &["claude-code", "omp"], &NOTES_ARGS));
    fx
}

/// The plan blocks the read-only file's entry, plans the other, and the
/// apply writes only the other; the read-only file is untouched.
fn assert_blocked_then_rest_applies(fx: &Fx, file: &Path, kind: ReadOnlyKind) {
    let plan = fx.plan();
    let target = plan.targets.iter().find(|t| t.file == file).unwrap();
    assert_eq!(target.read_only, Some(kind), "{target:#?}");
    assert!(target.after.is_none());
    let entry = target.entries.iter().find(|e| e.name == "notes").unwrap();
    assert_eq!(entry.state, EntryState::Blocked);
    assert_eq!(entry.action, Action::None);
    assert!(!entry.unresolved);
    let problem = entry.problem.as_ref().unwrap();
    assert_eq!(problem.code(), ErrorCode::ReadOnlyTarget);
    assert_eq!(problem.read_only(), Some(kind));
    let omp = plan.targets.iter().find(|t| t.file == fx.omp()).unwrap();
    assert_eq!(omp.read_only, None);
    assert!(omp.after.is_some());

    let before = file.exists().then(|| snapshot(file));
    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");
    assert_eq!(report.outcome, ApplyOutcome::Partial, "{report:#?}");
    assert_eq!(report.status(), ApplyStatus::Done);
    let blocked = report.targets.iter().find(|t| t.file == file).unwrap();
    assert_eq!(blocked.outcome, ApplyOutcome::Blocked);
    assert_eq!(blocked.read_only, Some(kind));
    assert!(!blocked.written);
    let written = report.targets.iter().find(|t| t.file == fx.omp()).unwrap();
    assert!(written.written);
    assert_eq!(file.exists().then(|| snapshot(file)), before);
}

#[test]
fn read_only_file_is_blocked_and_the_run_goes_on() {
    let fx = two_targets();
    let file = fx.claude();
    set_readonly(&file, true);
    if writes_anyway(&file) {
        set_readonly(&file, false);
        return;
    }
    assert_blocked_then_rest_applies(&fx, &file, ReadOnlyKind::Attribute);
    // The file is listed (status), with nothing to write.
    let plan = fx.plan();
    assert!(plan.targets.iter().any(|t| t.file == file));
    set_readonly(&file, false);
    // Writable again: the next apply writes it.
    let (report, _) = fx.apply();
    assert_eq!(report.outcome, ApplyOutcome::Ok, "{report:#?}");
    assert!(fx.read(&file).contains("notes"));
}

#[test]
fn probe_never_modifies_or_creates() {
    let fx = Fx::new();
    let file = fx.claude();
    fx.write(&file, EMPTY);
    let before = snapshot(&file);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(probe(&fx, &file), WriteAccess::Writable);
    assert_eq!(snapshot(&file), before);

    set_readonly(&file, true);
    let before = snapshot(&file);
    let found = probe(&fx, &file);
    assert_eq!(snapshot(&file), before);
    set_readonly(&file, false);
    if !writes_anyway(&file) {
        assert_eq!(found, WriteAccess::ReadOnly(ReadOnlyKind::Attribute));
    }

    let missing = fx.th.home().join(".cursor").join("mcp.json");
    assert_eq!(probe(&fx, &missing), WriteAccess::Writable);
    assert!(!missing.exists());
    assert!(!missing.parent().unwrap().exists());
}

/// The confined test view with hooks: a slow (counted) probe or none, a
/// folder whose writes fail with `PermissionDenied`, and a file made read
/// only once its temp file is written (after planning, before the
/// replace).
#[derive(Default)]
struct Hooks {
    probe: bool,
    probe_delay: Duration,
    calls: Arc<AtomicUsize>,
    deny_folder: Option<PathBuf>,
    readonly_on_temp: Option<PathBuf>,
    gate: Option<Gate>,
}

struct Hooked {
    inner: RootedFs,
    hooks: Hooks,
}

impl Hooked {
    fn check(&self, path: &Path) -> io::Result<()> {
        if self
            .hooks
            .deny_folder
            .as_deref()
            .is_some_and(|folder| path.parent() == Some(folder))
        {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        } else {
            Ok(())
        }
    }
}

impl Fs for Hooked {
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
        self.check(path)?;
        self.inner.write(path, contents)
    }
    fn write_new(&self, path: &Path, contents: &[u8], like: Option<&Path>) -> io::Result<()> {
        self.check(path)?;
        self.inner.write_new(path, contents, like)?;
        if let Some(target) = &self.hooks.readonly_on_temp
            && path.parent() == target.parent()
            && path.to_string_lossy().ends_with(".tmp")
        {
            set_readonly(target, true);
        }
        Ok(())
    }
    fn replace(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.check(to)?;
        self.inner.replace(from, to)
    }
    fn write_private(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.inner.write_private(path, contents)
    }
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.create_dir_all(path)
    }
    fn create_private_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.create_private_dir_all(path)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_file(path)
    }
    fn is_regular_file(&self, path: &Path) -> bool {
        self.inner.is_regular_file(path)
    }
    fn overwrite_with_zeros(&self, path: &Path) -> io::Result<()> {
        self.inner.overwrite_with_zeros(path)
    }
    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_dir_all(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        self.inner.read_dir(path)
    }
    fn open_lock_file(&self, path: &Path) -> io::Result<File> {
        self.inner.open_lock_file(path)
    }
    fn write_probe(&self) -> Option<WriteProbe> {
        if !self.hooks.probe {
            return None;
        }
        let (delay, calls) = (self.hooks.probe_delay, self.hooks.calls.clone());
        let gate = self.hooks.gate.clone();
        let real = self.inner.write_probe()?;
        Some(Arc::new(move |path: &Path| {
            calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(delay);
            if let Some(gate) = &gate {
                let (open, opened) = &**gate;
                let mut open = open.lock().unwrap();
                while !*open {
                    open = opened.wait(open).unwrap();
                }
            }
            real(path)
        }))
    }
}

/// A probe that waits until the test opens it.
type Gate = Arc<(Mutex<bool>, Condvar)>;

#[test]
fn invalidating_keeps_a_stalled_probe_and_its_late_answer() {
    let fx = Fx::new();
    let file = fx.claude();
    fx.write(&file, EMPTY);
    let gate: Gate = Arc::default();
    let stalled = hooked(
        &fx,
        Hooks {
            probe: true,
            gate: Some(gate.clone()),
            ..Hooks::default()
        },
    );
    let calls = stalled.hooks.calls.clone();
    let cache = ProbeCache::with_budget(Duration::from_millis(30));
    let files = [(file.clone(), fingerprint(Some(EMPTY.as_bytes())))];
    for _ in 0..3 {
        assert_eq!(
            probe_files(&stalled, &files, Some(&cache))[&file],
            WriteAccess::Unknown
        );
        // The app's invalidate (watcher event) between refreshes.
        cache.clear();
    }
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(calls.load(Ordering::SeqCst), 1, "one probe thread per path");

    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let started = Instant::now();
    loop {
        let found = probe_files(&stalled, &files, Some(&cache))[&file];
        if found == WriteAccess::Writable {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the late answer is kept"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_failed_ledger_save_is_an_io_error_not_read_only() {
    let fx = two_targets();
    // Loadout's own state folder refuses writes; no probe, so a mapping
    // would say `acl`.
    let denied = hooked(
        &fx,
        Hooks {
            deny_folder: Some(fx.home.state_dir()),
            ..Hooks::default()
        },
    );
    let request = ApplyRequest {
        options: PlanOptions::default(),
        trigger: ApplyTrigger::Review,
    };
    match apply::run(&fx.store(), &ax(&fx, &denied), &request) {
        Err(err) => {
            assert_eq!(err.code(), ErrorCode::IoError, "{err:?}");
            assert_eq!(err.read_only(), None);
        }
        Ok((report, _)) => {
            let claude = report
                .targets
                .iter()
                .find(|t| t.file == fx.claude())
                .unwrap();
            assert!(claude.written, "the client file was written: {report:#?}");
            let error = claude.error.as_ref().unwrap();
            assert_eq!(error.code(), ErrorCode::IoError, "{report:#?}");
            assert_eq!(claude.read_only, None);
        }
    }
    assert!(fx.read(&fx.claude()).contains("notes"));
}

/// Makes `folder` refuse new entries for this account (Windows: a deny ACE
/// for adding files and folders, not inherited; Unix: mode 0555). `None`
/// when this process writes anyway (root); else what restores it.
fn refuse_new_entries(folder: &Path) -> Option<Box<dyn FnOnce()>> {
    #[cfg(windows)]
    {
        let original = windows::deny(folder, 0x6);
        let folder = folder.to_path_buf();
        Some(Box::new(move || windows::restore(&folder, &original)))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let set = |mode| {
            std::fs::set_permissions(folder, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        set(0o555);
        if std::fs::write(folder.join(".writes-anyway"), "").is_ok() {
            let _ = std::fs::remove_file(folder.join(".writes-anyway"));
            set(0o755);
            return None;
        }
        let folder = folder.to_path_buf();
        Some(Box::new(move || {
            std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        }))
    }
}

/// A read-only `info/exclude` blocks the private project file it would
/// hide, but does not mark that file read only.
#[test]
fn a_read_only_info_exclude_blocks_without_blaming_the_client_file() {
    let fx = Fx::new();
    let root = fx.project_from("web-app", "web-app");
    let repo = support::git_init(&root);
    fx.know_projects(&[("web-app-a1b2", &root)]);
    fx.trust(&root);
    let info = repo.path().join("info");
    std::fs::create_dir_all(&info).unwrap();
    let Some(restore) = refuse_new_entries(&info) else {
        return;
    };
    let (report, _) = fx.apply();
    restore();
    let omp = root.join(".omp").join("mcp.json");
    let target = report.targets.iter().find(|t| t.file == omp).unwrap();
    assert_eq!(target.outcome, ApplyOutcome::Blocked, "{report:#?}");
    let error = target.error.as_ref().unwrap();
    assert_eq!(error.code(), ErrorCode::ReadOnlyTarget);
    assert!(error.read_only().is_some());
    assert_eq!(target.read_only, None, "the client file is not read only");
    assert!(!omp.exists());
}

fn hooked(fx: &Fx, hooks: Hooks) -> Hooked {
    Hooked {
        inner: fx.th.fs(),
        hooks,
    }
}

fn ax<'a>(fx: &'a Fx, fs: &'a Hooked) -> ApplyEnv<'a> {
    ApplyEnv {
        env: &fx.env,
        fs,
        home: &fx.home,
        registry: &NO_REGISTRY,
        delivery: &fx.delivery,
    }
}

fn run(fx: &Fx, fs: &Hooked) -> apply::ApplyReport {
    let request = ApplyRequest {
        options: PlanOptions::default(),
        trigger: ApplyTrigger::Review,
    };
    apply::run(&fx.store(), &ax(fx, fs), &request).unwrap().0
}

#[test]
fn probe_past_the_deadline_is_unknown_then_cached() {
    let fx = Fx::new();
    let file = fx.claude();
    fx.write(&file, EMPTY);
    let slow = hooked(
        &fx,
        Hooks {
            probe: true,
            probe_delay: Duration::from_millis(400),
            ..Hooks::default()
        },
    );
    let calls = slow.hooks.calls.clone();
    let cache = ProbeCache::with_budget(Duration::from_millis(50));
    let files = [(file.clone(), fingerprint(Some(EMPTY.as_bytes())))];

    let started = Instant::now();
    let found = probe_files(&slow, &files, Some(&cache));
    assert!(started.elapsed() < Duration::from_millis(350));
    assert_eq!(found[&file], WriteAccess::Unknown);
    // Asked again while the probe still runs: no second probe.
    assert_eq!(
        probe_files(&slow, &files, Some(&cache))[&file],
        WriteAccess::Unknown
    );

    // The late answer is kept for the same fingerprint.
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        probe_files(&slow, &files, Some(&cache))[&file],
        WriteAccess::Writable
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // Another fingerprint (the file changed) probes again.
    let changed = [(file.clone(), fingerprint(Some(b"{}")))];
    let _ = probe_files(&slow, &changed, Some(&cache));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // So does a cleared cache (the app's invalidate), once that probe
    // answered (a running one is kept, not started again).
    std::thread::sleep(Duration::from_millis(600));
    cache.clear();
    let _ = probe_files(&slow, &changed, Some(&cache));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[test]
fn a_plan_whose_probe_stalls_counts_the_file_writable() {
    let fx = Fx::new();
    let file = fx.claude();
    fx.write(&file, EMPTY);
    fx.put_server(stdio("notes", &["claude-code"], &NOTES_ARGS));
    let slow = hooked(
        &fx,
        Hooks {
            probe: true,
            probe_delay: Duration::from_millis(500),
            ..Hooks::default()
        },
    );
    let cache = PlanCache::with_probe_budget(Duration::from_millis(50));
    let started = Instant::now();
    let cached = cache
        .plan(&ax(&fx, &slow), &|fs| {
            Store::open(&fx.env, fs, &fx.home, "loadout-test/0")
        })
        .unwrap();
    assert!(started.elapsed() < Duration::from_millis(450));
    let target = cached.plan.targets.iter().find(|t| t.file == file).unwrap();
    assert_eq!(target.read_only, None);
    assert!(target.after.is_some());
}

#[test]
fn a_write_refused_as_read_only_is_blocked_and_the_run_goes_on() {
    let fx = two_targets();
    // No probe: only the write-time mapping can tell.
    let denied = hooked(
        &fx,
        Hooks {
            deny_folder: Some(fx.th.home().to_path_buf()),
            ..Hooks::default()
        },
    );
    let before = snapshot(&fx.claude());
    let report = run(&fx, &denied);
    assert!(report.recorded, "{report:#?}");
    assert_eq!(report.outcome, ApplyOutcome::Partial, "{report:#?}");
    assert_eq!(report.status(), ApplyStatus::Done);
    let claude = report
        .targets
        .iter()
        .find(|t| t.file == fx.claude())
        .unwrap();
    assert_eq!(claude.outcome, ApplyOutcome::Blocked);
    assert_eq!(claude.read_only, Some(ReadOnlyKind::Acl));
    let error = claude.error.as_ref().unwrap();
    assert_eq!(error.code(), ErrorCode::ReadOnlyTarget);
    assert_eq!(error.read_only(), Some(ReadOnlyKind::Acl));
    let notes = claude.entries.iter().find(|e| e.name == "notes").unwrap();
    assert_eq!(notes.state, EntryState::Blocked);
    let claude_file = fx.claude().to_string_lossy().into_owned();
    assert!(!fx.ledger().entries.iter().any(|e| e.file == claude_file));
    let omp = report.targets.iter().find(|t| t.file == fx.omp()).unwrap();
    assert!(omp.written);
    assert_eq!(snapshot(&fx.claude()), before);
}

#[cfg(windows)]
#[test]
fn read_only_attribute_at_replace_is_not_waited_for() {
    // Set after planning: the replace's access denied is checked by the
    // probe at once and reported read only, not retried as a lock.
    let fx = two_targets();
    let file = fx.claude();
    let late = hooked(
        &fx,
        Hooks {
            probe: true,
            readonly_on_temp: Some(file.clone()),
            ..Hooks::default()
        },
    );
    let started = Instant::now();
    let report = run(&fx, &late);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "waited as for a lock"
    );
    set_readonly(&file, false);
    let claude = report.targets.iter().find(|t| t.file == file).unwrap();
    assert_eq!(claude.outcome, ApplyOutcome::Blocked, "{report:#?}");
    assert_eq!(claude.read_only, Some(ReadOnlyKind::Attribute));
    assert_eq!(fx.read(&file), EMPTY);
    let temps = std::fs::read_dir(fx.th.home())
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        })
        .count();
    assert_eq!(temps, 0, "the temp file is removed");
}

#[cfg(windows)]
mod windows {
    use std::os::windows::fs::OpenOptionsExt;

    use windows_permissions::constants::{SeObjectType, SecurityInformation};
    use windows_permissions::{LocalBox, SecurityDescriptor, wrappers};

    use super::*;

    /// Puts a deny ACE for the current user (`mask`) in front of `path`'s
    /// DACL; returns the SDDL to restore.
    pub(super) fn deny(path: &Path, mask: u32) -> String {
        let sd = wrappers::GetNamedSecurityInfo(
            path.as_os_str(),
            SeObjectType::SE_FILE_OBJECT,
            SecurityInformation::Dacl | SecurityInformation::Owner,
        )
        .unwrap();
        let original = sd.as_sddl().unwrap().to_string_lossy().into_owned();
        let owner = sd.owner().unwrap().to_string();
        let denied: LocalBox<SecurityDescriptor> =
            format!("D:(D;;0x{mask:x};;;{owner})(A;;FA;;;{owner})")
                .parse()
                .unwrap();
        set_dacl(path, &denied);
        original
    }

    fn set_dacl(path: &Path, sd: &SecurityDescriptor) {
        wrappers::SetNamedSecurityInfo(
            path.as_os_str(),
            SeObjectType::SE_FILE_OBJECT,
            SecurityInformation::Dacl | SecurityInformation::UnprotectedDacl,
            None,
            None,
            Some(sd.dacl().unwrap()),
            None,
        )
        .unwrap();
    }

    pub(super) fn restore(path: &Path, sddl: &str) {
        let sd: LocalBox<SecurityDescriptor> = sddl.parse().unwrap();
        set_dacl(path, &sd);
    }

    /// FILE_WRITE_DATA, FILE_APPEND_DATA, FILE_WRITE_EA,
    /// FILE_WRITE_ATTRIBUTES and DELETE: what "Read & execute" lacks.
    const DENY_WRITE: u32 = 0x1_0116;
    /// FILE_ADD_FILE and FILE_ADD_SUBDIRECTORY on a folder.
    const DENY_CREATE: u32 = 0x6;
    /// DELETE on a file.
    const DENY_DELETE: u32 = 0x1_0000;
    /// FILE_DELETE_CHILD on a folder.
    const DENY_DELETE_CHILD: u32 = 0x40;

    /// "Read & Write" without "Modify": the file can be written in place
    /// but not deleted, so the save's rename over it fails. Read only,
    /// found by the probe, never waited for as a lock.
    #[test]
    fn write_without_delete_is_acl() {
        let fx = two_targets();
        let file = fx.claude();
        let folder = fx.th.home().to_path_buf();
        let file_acl = deny(&file, DENY_DELETE);
        let folder_acl = deny(&folder, DENY_DELETE_CHILD);
        assert!(std::fs::OpenOptions::new().write(true).open(&file).is_ok());
        assert_eq!(probe(&fx, &file), WriteAccess::ReadOnly(ReadOnlyKind::Acl));
        let started = Instant::now();
        let (report, _) = fx.apply();
        let elapsed = started.elapsed();
        restore(&file, &file_acl);
        restore(&folder, &folder_acl);
        assert!(elapsed < Duration::from_secs(2), "waited as for a lock");
        let claude = report.targets.iter().find(|t| t.file == file).unwrap();
        assert_eq!(claude.outcome, ApplyOutcome::Blocked, "{report:#?}");
        assert_eq!(claude.read_only, Some(ReadOnlyKind::Acl));
        assert_eq!(fx.read(&file), EMPTY);
    }

    #[test]
    fn deny_write_ace_is_acl() {
        let fx = two_targets();
        let file = fx.claude();
        let before = snapshot(&file);
        let original = deny(&file, DENY_WRITE);
        assert_eq!(probe(&fx, &file), WriteAccess::ReadOnly(ReadOnlyKind::Acl));
        assert_eq!(snapshot(&file), before);
        assert_blocked_then_rest_applies(&fx, &file, ReadOnlyKind::Acl);
        restore(&file, &original);
    }

    #[test]
    fn folder_that_denies_creating_is_acl_and_nothing_is_created() {
        let fx = Fx::new();
        let folder = fx.th.home().join(".omp").join("agent");
        std::fs::create_dir_all(&folder).unwrap();
        fx.write(&fx.claude(), EMPTY);
        fx.put_server(stdio("notes", &["claude-code", "omp"], &NOTES_ARGS));
        let original = deny(&folder, DENY_CREATE);
        assert_eq!(
            probe(&fx, &fx.omp()),
            WriteAccess::ReadOnly(ReadOnlyKind::Acl)
        );
        let plan = fx.plan();
        let omp = plan.targets.iter().find(|t| t.file == fx.omp()).unwrap();
        assert_eq!(omp.read_only, Some(ReadOnlyKind::Acl));
        let (report, _) = fx.apply();
        assert_eq!(report.outcome, ApplyOutcome::Partial, "{report:#?}");
        assert!(!fx.omp().exists());
        assert!(fx.read(&fx.claude()).contains("notes"));
        restore(&folder, &original);
    }

    #[test]
    fn sharing_violation_is_not_read_only() {
        const FILE_SHARE_READ: u32 = 1;
        let fx = two_targets();
        let file = fx.claude();
        let held = File::options()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&file)
            .unwrap();
        assert_eq!(probe(&fx, &file), WriteAccess::Unknown);
        let plan = fx.plan();
        let target = plan.targets.iter().find(|t| t.file == file).unwrap();
        assert_eq!(target.read_only, None);
        assert!(target.after.is_some());
        let (report, _) = fx.apply();
        let claude = report.targets.iter().find(|t| t.file == file).unwrap();
        assert_eq!(claude.outcome, ApplyOutcome::Failed);
        assert_eq!(claude.error.as_ref().unwrap().code(), ErrorCode::FileLocked);
        assert_eq!(claude.read_only, None);
        drop(held);
    }
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn mode_0444_file_is_attribute() {
        let fx = two_targets();
        let file = fx.claude();
        chmod(&file, 0o444);
        if writes_anyway(&file) {
            chmod(&file, 0o644);
            return;
        }
        assert_blocked_then_rest_applies(&fx, &file, ReadOnlyKind::Attribute);
        chmod(&file, 0o644);
    }

    #[test]
    fn mode_0555_folder_blocks_a_new_file_and_nothing_is_created() {
        let fx = Fx::new();
        let folder = fx.th.home().join(".omp").join("agent");
        std::fs::create_dir_all(&folder).unwrap();
        fx.write(&fx.claude(), EMPTY);
        fx.put_server(stdio("notes", &["claude-code", "omp"], &NOTES_ARGS));
        chmod(&folder, 0o555);
        if writes_anyway(&folder.join("probe")) || std::fs::write(folder.join("x"), "").is_ok() {
            let _ = std::fs::remove_file(folder.join("x"));
            chmod(&folder, 0o755);
            return;
        }
        assert_eq!(
            probe(&fx, &fx.omp()),
            WriteAccess::ReadOnly(ReadOnlyKind::Attribute)
        );
        let (report, _) = fx.apply();
        assert_eq!(report.outcome, ApplyOutcome::Partial, "{report:#?}");
        let omp = report.targets.iter().find(|t| t.file == fx.omp()).unwrap();
        assert_eq!(omp.read_only, Some(ReadOnlyKind::Attribute));
        assert!(!fx.omp().exists());
        chmod(&folder, 0o755);
    }
}

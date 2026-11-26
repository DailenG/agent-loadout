//! M50: NVIDIA SkillSpector in the app (#30), hermetic: a stub stands in
//! for `skillspector`, `uvx` and `uv` (testkit `skillspector_stub`) on a
//! `PATH` of its own; nothing is downloaded. Status per availability,
//! Prepare (progress, failure, cancel, one at a time), the switch
//! dropping reports, scans of a local review, the gates
//! (`scan_blocked`, `external_scan_required`) and not-scanned reports
//! that never block.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use agent_loadout_core::scan::external::ExternalReport;
use agent_loadout_core::scan::external::skillspector;
use agent_loadout_core::skills::install::Prepared;
use agent_loadout_core::skills::{content_hash, install};
use agent_loadout_core::{CoreError, ErrorCode};
use agent_loadout_testkit::git_env::{git_vars, init_bare, system_git_or_skip};
use agent_loadout_testkit::skillspector_stub::{SkillSpectorStub, StubConfig, output};

use super::scan::ScanVerdict;
use super::scanner::{
    self, ExternalNotScanned, ExternalScanResult, ExternalScanStatus, ExternalScanner,
    ScannerAvailability, ScannerRuntime, ScannerState,
};
use super::skills;
use super::sources::{SourceScope, TrustGrantArg};
use super::tests::Fx;
use super::{AppState, CmdResult, Reviews, scrub};

const CLEAN: &str = "---\nname: tidy\ndescription: Tidies.\n---\n\n# Tidy\n\nSort the imports.\n";

/// The app is a workspace of its own, so its test build does not make the
/// testkit's binaries: build the stub once in the root workspace (its own
/// target folder, so no build lock is shared) and copy it where
/// `built_binary` looks (beside the test executable's `deps`).
fn ensure_stub_binary() {
    static BUILT: LazyLock<()> = LazyLock::new(build_stub_binary);
    LazyLock::force(&BUILT);
}

fn build_stub_binary() {
    let name = format!("skillspector-stub{}", std::env::consts::EXE_SUFFIX);
    let exe = std::env::current_exe().unwrap();
    let profile_dir = exe.parent().and_then(Path::parent).unwrap();
    let wanted = profile_dir.join(&name);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let target = root.join("target").join("app-test-stubs");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let status = std::process::Command::new(cargo)
        .current_dir(&root)
        .args([
            "build",
            "--quiet",
            "-p",
            "agent-loadout-testkit",
            "--bin",
            "skillspector-stub",
            "--target-dir",
        ])
        .arg(&target)
        .status()
        .expect("run cargo build for the stub");
    assert!(status.success(), "building skillspector-stub failed");
    std::fs::copy(target.join("debug").join(&name), &wanted)
        .expect("copy the stub beside the tests");
}

/// A stub `bin` folder holding `programs`, alone on `PATH` (in front of
/// what `PATH` held, with `keep_path`).
fn stub(fx: &mut Fx, programs: &[&str], keep_path: bool) -> SkillSpectorStub {
    ensure_stub_binary();
    let stub = SkillSpectorStub::new(&fx.th.root().join("stub"));
    for program in programs {
        stub.add(program);
    }
    let mut paths = vec![stub.bin().to_path_buf()];
    if keep_path && let Some(path) = fx.core.env.var("PATH") {
        paths.extend(std::env::split_paths(path));
    }
    let path = std::env::join_paths(paths).unwrap();
    fx.core.env = fx.core.env.with_var("PATH", path);
    stub
}

/// uvx with SkillSpector already prepared: Ready.
fn ready(fx: &mut Fx) -> SkillSpectorStub {
    let stub = stub(fx, &["uvx"], false);
    stub.set_prepared(true);
    stub
}

fn folder(fx: &Fx, name: &str) -> PathBuf {
    let dir = fx.th.projects().join("downloads").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), CLEAN.replace("tidy", name)).unwrap();
    dir
}

fn dangerous(root: &Path) -> ExternalReport {
    skillspector::parse(
        output("do-not-install.json").as_bytes(),
        b"",
        Some(1),
        root,
        skillspector::VERSION,
    )
}

/// A review scan with no switch-off meanwhile.
fn review_scan(
    fx: &Fx,
    prepared: &mut Prepared,
    cancel: &AtomicBool,
) -> CmdResult<ExternalScanResult> {
    scanner::scan_review(&fx.core, prepared, cancel, &AtomicU64::new(0), 0)
}

fn app_state() -> AppState {
    AppState {
        core: Err(CoreError::new(ErrorCode::NotFound, "no core in this test")),
        picked: Mutex::new(HashMap::new()),
        reviews: Mutex::new(Reviews::default()),
        scanner: ScannerState::default(),
    }
}

/// The home folder goes in every form a message may carry it: as is,
/// with forward slashes, and with backslashes doubled (JSON, debug output).
#[test]
fn scrub_replaces_every_form_of_the_home_folder() {
    let fx = Fx::new();
    let home = fx.core.env.home().to_string_lossy().into_owned();
    let forward = home.replace('\\', "/");
    let doubled = home.replace('\\', "\\\\");
    let text = format!("a {home}/x b {forward}/y c {doubled}\\\\z");
    let out = scrub(&fx.core.env, &text, None);
    assert_eq!(out, "a ~/x b ~/y c ~\\\\z");
    assert_eq!(
        scrub(&fx.core.env, r"C:\\Elsewhere\\file", None),
        r"C:\\Elsewhere\\file"
    );
}

#[test]
fn status_follows_availability() {
    let mut fx = Fx::new();
    let stub = stub(&mut fx, &[], false);
    let status = scanner::status(&fx.core).unwrap();
    assert_eq!(status.scanner, ExternalScanner::SkillSpector);
    assert_eq!(status.tested_version, "2.13.7");
    assert_eq!(status.python, "3.13");
    assert_eq!(status.availability, ScannerAvailability::NotAvailable);
    assert_eq!((status.runtime, status.version.as_deref()), (None, None));
    assert!(status.version_tested && !status.enabled && !status.snyk_installed);
    assert_eq!(status.timeout_secs, 120);

    stub.add("uvx");
    let status = scanner::status(&fx.core).unwrap();
    assert_eq!(status.availability, ScannerAvailability::NeedsPrepare);
    assert_eq!(status.runtime, Some(ScannerRuntime::Uvx));
    assert_eq!(status.version, None);

    stub.set_prepared(true);
    let status = scanner::status(&fx.core).unwrap();
    assert_eq!(status.availability, ScannerAvailability::Ready);
    assert_eq!(status.runtime, Some(ScannerRuntime::Uvx));
    assert_eq!(status.version.as_deref(), Some("2.13.7"));
    assert!(status.version_tested);
    let json = serde_json::to_string(&status).unwrap();
    for key in [
        "\"scanner\":\"skillspector\"",
        "\"testedVersion\"",
        "\"versionTested\":true",
        "\"timeoutSecs\":120",
        "\"snykInstalled\":false",
        "\"availability\":\"ready\"",
        "\"runtime\":\"uvx\"",
    ] {
        assert!(json.contains(key), "{key} in {json}");
    }
    fx.assert_no_root(&json);
}

#[test]
fn an_untested_skillspector_on_path_is_ready() {
    let mut fx = Fx::new();
    let stub = stub(&mut fx, &["skillspector"], false);
    stub.set(&StubConfig {
        version: "skillspector 2.14.0".to_owned(),
        ..StubConfig::default()
    });
    let status = scanner::status(&fx.core).unwrap();
    assert_eq!(status.availability, ScannerAvailability::Ready);
    assert_eq!(status.runtime, Some(ScannerRuntime::Path));
    assert_eq!(status.version.as_deref(), Some("2.14.0"));
    assert!(!status.version_tested);
}

#[test]
fn prepare_reports_progress_scrubbed_and_does_not_turn_it_on() {
    let mut fx = Fx::new();
    let stub = stub(&mut fx, &["uvx"], false);
    let home = fx.core.env.home().display().to_string();
    stub.set(&StubConfig {
        prepare_lines: vec![
            "Resolved 42 packages".to_owned(),
            format!("Installed into {home}\u{1b}[0m cache"),
        ],
        ..StubConfig::default()
    });
    let cancel = AtomicBool::new(false);
    let mut lines = Vec::new();
    let status =
        scanner::prepare(&fx.core, &mut |line| lines.push(line.to_owned()), &cancel).unwrap();
    assert_eq!(status.availability, ScannerAvailability::Ready);
    assert!(!status.enabled, "Prepare does not turn it on");
    assert!(stub.prepared());
    assert!(
        lines.iter().any(|line| line == "Resolved 42 packages"),
        "{lines:?}"
    );
    for line in &lines {
        assert!(!line.contains(&home), "{line}");
        assert!(!line.chars().any(char::is_control), "{line:?}");
    }
    fx.assert_no_root(&lines.join("\n"));
}

#[test]
fn prepare_fails_without_uv_or_when_uv_fails() {
    let mut fx = Fx::new();
    let stub = stub(&mut fx, &[], false);
    let cancel = AtomicBool::new(false);
    let err = scanner::prepare(&fx.core, &mut |_| {}, &cancel).unwrap_err();
    assert_eq!(err.code, ErrorCode::ScannerUnavailable);

    stub.add("uvx");
    stub.set(&StubConfig {
        prepare_exit: 1,
        ..StubConfig::default()
    });
    let err = scanner::prepare(&fx.core, &mut |_| {}, &cancel).unwrap_err();
    assert_eq!(err.code, ErrorCode::ScannerDownloadFailed);
    assert!(!stub.prepared());
    fx.assert_no_root(&err.message);
}

#[test]
fn prepare_is_canceled() {
    let mut fx = Fx::new();
    let stub = stub(&mut fx, &["uvx"], false);
    stub.set(&StubConfig {
        prepare_hang: true,
        heartbeat: Some(fx.th.root().join("heartbeat")),
        ..StubConfig::default()
    });
    let state = ScannerState::default();
    let running = state.begin_prepare().unwrap();
    let err = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(500));
            // `cancel_external_scan(null)`.
            state.cancel(None);
        });
        scanner::prepare(&fx.core, &mut |_| {}, &running.flag).unwrap_err()
    });
    assert_eq!(err.code, ErrorCode::Canceled);
    assert!(!stub.prepared());
}

#[test]
fn one_prepare_at_a_time() {
    let state = ScannerState::default();
    let first = state.begin_prepare().unwrap();
    assert_eq!(
        state.begin_prepare().err().map(|err| err.code),
        Some(ErrorCode::Locked)
    );
    drop(first);
    assert!(state.begin_prepare().is_ok());
}

/// The GUI's Cancel may reach the backend before `prepare_scanner` does:
/// the next Prepare is then canceled before it starts, once.
#[test]
fn a_cancel_before_prepare_starts_cancels_it() {
    let state = ScannerState::default();
    state.cancel(None);
    assert_eq!(
        state.begin_prepare().err().map(|err| err.code),
        Some(ErrorCode::Canceled)
    );
    let preparing = state.begin_prepare().unwrap();
    // While one runs, a cancel reaches it and is not kept for later.
    state.cancel(None);
    assert!(preparing.flag.load(Ordering::SeqCst));
    drop(preparing);
    assert!(state.begin_prepare().is_ok());
}

#[test]
fn cancel_reaches_one_run_or_every_run() {
    let state = ScannerState::default();
    let first = state.start(Some("run-1".to_owned()));
    let second = state.start(Some("run-2".to_owned()));
    let prepare = state.begin_prepare().unwrap();
    state.cancel(Some("run-1"));
    assert!(first.flag.load(Ordering::SeqCst));
    assert!(!second.flag.load(Ordering::SeqCst), "another run goes on");
    assert!(!prepare.flag.load(Ordering::SeqCst));
    state.cancel(None);
    assert!(second.flag.load(Ordering::SeqCst));
    assert!(
        prepare.flag.load(Ordering::SeqCst),
        "null cancels Prepare too"
    );
    drop((first, second, prepare));
    let later = state.start(Some("run-1".to_owned()));
    assert!(
        !later.flag.load(Ordering::SeqCst),
        "a new run starts uncanceled"
    );
}

/// Off cancels every run (not a Prepare) and drops remembered reports.
#[test]
fn turning_the_scanner_off_cancels_runs_and_forgets_update_reports() {
    let fx = Fx::new();
    let status = scanner::set_enabled(&fx.core, true).unwrap();
    assert!(status.enabled);
    let state = ScannerState::default();
    let run = state.start(Some("run-1".to_owned()));
    let prepare = state.begin_prepare().unwrap();
    state.remember_update("tidy", dangerous(&fx.th.root().join("run")));
    let status = scanner::set_enabled(&fx.core, false).unwrap();
    assert!(!status.enabled);
    state.turned_off();
    assert!(run.flag.load(Ordering::SeqCst), "the run was canceled");
    assert!(!prepare.flag.load(Ordering::SeqCst), "Prepare goes on");
    assert!(state.take_update("tidy").is_none());
    // A run that began before the switch-off remembers nothing.
    let since = state.generation();
    state.turned_off();
    assert!(!state.remember_update_since("tidy", dangerous(&fx.th.root().join("run")), since));
    assert!(state.take_update("tidy").is_none());
    assert!(state.remember_update_since(
        "tidy",
        dangerous(&fx.th.root().join("run")),
        state.generation()
    ));
}

/// Switching off drops the report attached to an open review.
#[test]
fn turning_the_scanner_off_drops_attached_review_reports() {
    let mut fx = Fx::new();
    let stub = ready(&mut fx);
    stub.answer(&output("safe.json"), 0);
    scanner::set_enabled(&fx.core, true).unwrap();
    let (mut prepared, _) = skills::review_local(&fx.core, &folder(&fx, "tidy")).unwrap();
    review_scan(&fx, &mut prepared, &AtomicBool::new(false)).unwrap();
    assert!(prepared.external().is_some());
    let state = app_state();
    let (token, _) = state.remember_review(prepared);
    scanner::turned_off(&state);
    let review = state.review(&token).unwrap();
    assert!(review.lock().unwrap().external().is_none());
}

/// Switched off while a review scan runs (holding the review's lock):
/// the run is canceled and nothing stays attached, not even an earlier
/// report.
#[test]
fn a_switch_off_during_a_review_scan_leaves_nothing_attached() {
    let mut fx = Fx::new();
    let stub = ready(&mut fx);
    stub.set(&StubConfig {
        hang: true,
        heartbeat: Some(fx.th.root().join("heartbeat")),
        ..StubConfig::default()
    });
    scanner::set_enabled(&fx.core, true).unwrap();
    let (mut prepared, _) = skills::review_local(&fx.core, &folder(&fx, "tidy")).unwrap();
    let mut earlier = dangerous(&fx.th.root().join("run"));
    earlier.content_hash = Some(content_hash(prepared.files()));
    prepared.set_external(Some(earlier));
    let state = app_state();
    let (token, _) = state.remember_review(prepared);
    let review = state.review(&token).unwrap();
    let err = std::thread::scope(|scope| {
        let scan = scope.spawn(|| {
            // What `external_scan_review` does.
            let since = state.scanner.generation();
            let generation = state.scanner.generation_counter();
            let running = state.scanner.start(Some("run-1".to_owned()));
            let mut prepared = review.lock().unwrap();
            scanner::scan_review(&fx.core, &mut prepared, &running.flag, &generation, since)
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while stub.scans().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        scanner::turned_off(&state);
        scan.join().unwrap().unwrap_err()
    });
    assert_eq!(err.code, ErrorCode::ScannerUnavailable);
    assert!(review.lock().unwrap().external().is_none());
}

/// Off: every run is refused and nothing starts.
#[test]
fn runs_are_refused_while_off() {
    let mut fx = Fx::new();
    let stub = ready(&mut fx);
    let (mut prepared, _) = skills::review_local(&fx.core, &folder(&fx, "tidy")).unwrap();
    let cancel = AtomicBool::new(false);
    let err = review_scan(&fx, &mut prepared, &cancel).unwrap_err();
    assert_eq!(err.code, ErrorCode::ScannerUnavailable);
    assert!(prepared.external().is_none());
    skills::install_reviewed(&fx.core, &mut prepared, None, false, false, None, None).unwrap();
    let err = scanner::scan_library(&fx.core, "tidy", &cancel).unwrap_err();
    assert_eq!(err.code, ErrorCode::ScannerUnavailable);
    assert!(stub.scans().is_empty());
}

#[test]
fn a_local_review_is_scanned_and_installed() {
    let mut fx = Fx::new();
    let stub = ready(&mut fx);
    stub.answer(&output("safe.json"), 0);
    scanner::set_enabled(&fx.core, true).unwrap();
    let (mut prepared, _) = skills::review_local(&fx.core, &folder(&fx, "tidy")).unwrap();
    let cancel = AtomicBool::new(false);
    let result = review_scan(&fx, &mut prepared, &cancel).unwrap();
    assert_eq!(result.scanner, ExternalScanner::SkillSpector);
    assert_eq!(result.status, ExternalScanStatus::Scanned);
    assert_eq!(result.version, "2.13.7");
    assert!(!result.incomplete);
    let scans = stub.scans();
    assert_eq!(scans.len(), 1);
    assert_eq!(scans[0].files, vec!["SKILL.md".to_owned()]);
    assert!(scans[0].argv.iter().any(|arg| arg == "--offline"));
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"scanner\":\"skillspector\""), "{json}");
    assert!(json.contains("\"incomplete\":false"), "{json}");
    fx.assert_no_root(&json);
    assert!(prepared.external().is_some());
    let installed =
        skills::install_reviewed(&fx.core, &mut prepared, None, false, false, None, None).unwrap();
    assert!(installed.external.is_some());

    // A library skill's store content, report only.
    stub.answer(&output("incomplete.json"), 0);
    let result = scanner::scan_library(&fx.core, "tidy", &cancel).unwrap();
    assert!(result.incomplete);
    assert!(
        result
            .findings
            .iter()
            .any(|finding| finding.rule == "incomplete")
    );
}

#[test]
fn a_dangerous_report_needs_install_anyway() {
    let mut fx = Fx::new();
    let stub = ready(&mut fx);
    stub.answer(&output("do-not-install.json"), 1);
    scanner::set_enabled(&fx.core, true).unwrap();
    let (mut prepared, review) = skills::review_local(&fx.core, &folder(&fx, "tidy")).unwrap();
    assert_eq!(review.scan.verdict, ScanVerdict::Clean);
    let cancel = AtomicBool::new(false);
    let result = review_scan(&fx, &mut prepared, &cancel).unwrap();
    assert_eq!(result.status, ExternalScanStatus::Scanned);
    assert_eq!(result.verdict, ScanVerdict::Dangerous);
    assert!(result.findings.iter().all(|f| f.scanner == "skillspector"));

    let err = skills::install_reviewed(&fx.core, &mut prepared, None, false, false, None, None)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ScanBlocked);
    assert!(
        err.message.starts_with("SkillSpector found"),
        "{}",
        err.message
    );
    let installed =
        skills::install_reviewed(&fx.core, &mut prepared, None, false, true, None, None).unwrap();
    assert_eq!(
        installed.external.map(|external| external.verdict),
        Some(ScanVerdict::Dangerous)
    );
}

/// On but not ready: `not_prepared` without running anything; a failed
/// run: `failed`. Neither blocks.
#[test]
fn not_scanned_never_blocks() {
    let mut fx = Fx::new();
    let stub = stub(&mut fx, &["uvx"], false);
    scanner::set_enabled(&fx.core, true).unwrap();
    let cancel = AtomicBool::new(false);
    let (mut prepared, _) = skills::review_local(&fx.core, &folder(&fx, "tidy")).unwrap();
    let result = review_scan(&fx, &mut prepared, &cancel).unwrap();
    assert_eq!(result.status, ExternalScanStatus::NotScanned);
    assert_eq!(result.reason, Some(ExternalNotScanned::NotPrepared));
    assert!(stub.scans().is_empty(), "nothing ran");
    skills::install_reviewed(&fx.core, &mut prepared, None, false, false, None, None).unwrap();

    stub.set_prepared(true);
    stub.answer("", 2);
    let (mut prepared, _) = skills::review_local(&fx.core, &folder(&fx, "neat")).unwrap();
    let result = review_scan(&fx, &mut prepared, &cancel).unwrap();
    assert_eq!(result.reason, Some(ExternalNotScanned::Failed));
    fx.assert_no_root(&serde_json::to_string(&result).unwrap());
    skills::install_reviewed(&fx.core, &mut prepared, None, false, false, None, None).unwrap();

    let report = ExternalScanResult::of(&fx.core.env, prepared.external().unwrap());
    assert_eq!(report.status, ExternalScanStatus::NotScanned);
}

/// On and ready: every review (a local folder too) must carry its report,
/// and an install without a review is refused. Off, or not ready: no
/// report is needed.
#[test]
fn installs_need_a_report_while_on_and_ready() {
    let mut fx = Fx::new();
    let stub = stub(&mut fx, &["uvx"], false);
    scanner::set_enabled(&fx.core, true).unwrap();
    let (mut prepared, _) = skills::review_local(&fx.core, &folder(&fx, "tidy")).unwrap();
    // Not ready yet: nothing required.
    assert!(scanner::require_report(&fx.core, false).is_ok());

    stub.set_prepared(true);
    let err = skills::install_reviewed(&fx.core, &mut prepared, None, false, false, None, None)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ExternalScanRequired);
    let err = skills::install_git(
        &fx.core,
        "https://github.com/acme/skills",
        None,
        None,
        None,
        false,
        None,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::ExternalScanRequired);
    let err = skills::install_local(&fx.core, &folder(&fx, "plain"), None, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::ExternalScanRequired);
    let err = skills::update(&fx.core, "tidy", "0123abc", false, None, None, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::ExternalScanRequired);

    stub.answer(&output("safe.json"), 0);
    let cancel = AtomicBool::new(false);
    review_scan(&fx, &mut prepared, &cancel).unwrap();
    skills::install_reviewed(&fx.core, &mut prepared, None, false, false, None, None).unwrap();

    scanner::set_enabled(&fx.core, false).unwrap();
    let (mut prepared, _) = skills::review_local(&fx.core, &folder(&fx, "neat")).unwrap();
    skills::install_reviewed(&fx.core, &mut prepared, None, false, false, None, None).unwrap();
    skills::install_local(&fx.core, &folder(&fx, "plain"), None, false).unwrap();
}

fn git_run(fx: &Fx, git: &Path, cwd: &Path, args: &[&str]) {
    let status = fx
        .th
        .command(git)
        .envs(git_vars(&fx.th, git))
        .current_dir(cwd)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// `update_skill` with a remembered report, driven as the command does
/// (take, update, restore on failure): with SkillSpector on and ready an
/// update needs a report, a dangerous one blocks until allowed and is kept
/// for the retry, a report of other content is `entry_changed`, and once
/// the scanner is turned off the old report is gone and the update goes
/// through.
#[test]
fn update_skill_gates_on_the_remembered_report() {
    let Some(git) = system_git_or_skip("update_skill_gates_on_the_remembered_report") else {
        return;
    };
    let mut fx = Fx::with_git("test-desk", &git);
    let home = fx.th.home();
    let bare = home.join("up.git");
    init_bare(&fx.th, &git, &bare);
    git_run(
        &fx,
        &git,
        &home,
        &["init", "--quiet", "--initial-branch=main", "work"],
    );
    let work = home.join("work");
    git_run(&fx, &git, &work, &["remote", "add", "origin", "../up.git"]);
    let skill = work.join("skills").join("alpha");
    std::fs::create_dir_all(&skill).unwrap();
    let push = |fx: &Fx, text: &str, message: &str| {
        std::fs::write(skill.join("SKILL.md"), CLEAN.replace("tidy", text)).unwrap();
        git_run(fx, &git, &work, &["add", "-A"]);
        git_run(fx, &git, &work, &["commit", "--quiet", "-m", message]);
        git_run(fx, &git, &work, &["push", "--quiet", "origin", "HEAD:main"]);
    };
    push(&fx, "alpha", "initial");
    let url = format!(
        "file:///{}",
        bare.to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches('/')
    );
    skills::install_git(
        &fx.core,
        &url,
        Some("skills/alpha".to_owned()),
        None,
        Some("alpha".to_owned()),
        false,
        Some(TrustGrantArg::Always {
            scope: SourceScope::Repo,
        }),
    )
    .unwrap();
    push(&fx, "alpha v2", "v2");
    let checks = skills::check_updates(&fx.core, &["alpha".to_owned()]).unwrap();
    let latest = checks[0].latest.clone().unwrap();
    let files = {
        let store = fx.core.open_store().unwrap().unwrap();
        install::cached_latest(&store, &skills::sx(&fx.core), "alpha")
            .unwrap()
            .unwrap()
    };
    let run_root = fx.th.root().join("run");
    let scanned = |hash: String| {
        let mut report = dangerous(&run_root);
        report.content_hash = Some(hash);
        report
    };

    // On and ready (git stays on PATH).
    let _stub = {
        let stub = stub(&mut fx, &["uvx"], true);
        stub.set_prepared(true);
        stub
    };
    scanner::set_enabled(&fx.core, true).unwrap();
    // What `update_skill` does around `skills::update`.
    let state = ScannerState::default();
    let update_skill = |fx: &Fx, allow: bool| {
        let external = state.take_update("alpha");
        let kept = external.clone();
        let result = skills::update(&fx.core, "alpha", &latest, allow, None, None, external);
        if result.is_err()
            && let Some(report) = kept
        {
            state.restore_update("alpha", report);
        }
        result
    };

    assert_eq!(
        update_skill(&fx, false).unwrap_err().code,
        ErrorCode::ExternalScanRequired
    );
    state.remember_update("alpha", scanned(content_hash(&files)));
    let err = update_skill(&fx, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::ScanBlocked);
    let err = update_skill(&fx, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::ScanBlocked, "kept for the retry");
    // A report of other content does not gate this update.
    state.remember_update("alpha", scanned("sha256:other".to_owned()));
    assert_eq!(
        update_skill(&fx, false).unwrap_err().code,
        ErrorCode::EntryChanged
    );

    // Dangerous again, then the scanner turned off: the old report is
    // gone and the update goes through.
    state.remember_update("alpha", scanned(content_hash(&files)));
    scanner::set_enabled(&fx.core, false).unwrap();
    state.turned_off();
    let updated = update_skill(&fx, false).unwrap();
    assert_eq!(updated.to, latest);
    assert!(updated.external.is_none());
}

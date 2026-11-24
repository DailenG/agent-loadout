//! M47: Snyk Agent Scan in the app (#27), disabled in this build since
//! 0.7.1 (`external::AVAILABLE`): settings round-trip without a token
//! value, every run and the install refused with `scanner_unavailable`
//! even when a 0.7.0 setting turned it on, no `external_scan_required`,
//! Remove still cleaning up, cancel flags. Hermetic: nothing is
//! downloaded or run; a report comes from a recorded output.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use agent_loadout_core::ErrorCode;
use agent_loadout_core::scan::external::{self, Tool};
use agent_loadout_core::skills::{content_hash, install};
use agent_loadout_testkit::agent_scan_stub::output;
use agent_loadout_testkit::git_env::{git_vars, init_bare, system_git_or_skip};

use super::scan::ScanVerdict;
use super::scanner::{
    self, ExternalNotScanned, ExternalScanResult, ExternalScanStatus, ScannerState,
    ScannerTokenKind,
};
use super::skills;
use super::sources::{SourceScope, TrustGrantArg};
use super::tests::Fx;

const CLEAN: &str = "---\nname: tidy\ndescription: Tidies.\n---\n\n# Tidy\n\nSort the imports.\n";

fn folder(fx: &Fx, name: &str) -> PathBuf {
    let dir = fx.th.projects().join("downloads").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), CLEAN).unwrap();
    dir
}

#[test]
fn settings_round_trip_with_a_reference_only() {
    let fx = Fx::new();
    let status = scanner::status(&fx.core).unwrap();
    assert_eq!(status.scanner, "snyk-agent-scan");
    assert_eq!(status.version, "0.7.1");
    assert!(!status.enabled && !status.installed);
    assert_eq!(status.token_ref, None);
    assert_eq!(status.size_bytes.is_some(), status.supported);

    let status = scanner::set_token(&fx.core, Some("keyring://loadout/snyk".to_owned())).unwrap();
    assert_eq!(status.token_ref.as_deref(), Some("keyring://loadout/snyk"));
    assert_eq!(status.token_kind, Some(ScannerTokenKind::Keyring));
    let err =
        scanner::set_token(&fx.core, Some("snyk_live_value with spaces".to_owned())).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidRef);
    let status = scanner::set_enabled(&fx.core, true).unwrap();
    assert!(status.enabled);
    let json = serde_json::to_string(&status).unwrap();
    assert!(json.contains("\"tokenRef\""), "{json}");
    let status = scanner::set_token(&fx.core, None).unwrap();
    assert_eq!(status.token_ref, None);
    let check = scanner::check_token(&fx.core).unwrap();
    assert!(!check.ok);
    assert_eq!(check.reason, Some(ExternalNotScanned::NoToken));
}

/// Disabled in this build: a 0.7.0 setting that turned an installed
/// scanner on runs nothing, and nothing is downloaded.
#[test]
fn runs_and_installs_are_refused_in_this_build() {
    let fx = Fx::new();
    turn_on_installed(&fx);
    let (mut prepared, _) = skills::review_local(&fx.core, &folder(&fx, "tidy")).unwrap();
    let cancel = AtomicBool::new(false);
    let err = scanner::scan_review(&fx.core, &mut prepared, &cancel).unwrap_err();
    assert_eq!(err.code, ErrorCode::ScannerUnavailable);
    assert!(prepared.external().is_none());
    let installed =
        skills::install_reviewed(&fx.core, &mut prepared, None, false, false, None, None).unwrap();
    assert!(installed.external.is_none());
    let err = scanner::scan_library(&fx.core, "tidy", &cancel).unwrap_err();
    assert_eq!(err.code, ErrorCode::ScannerUnavailable);
    let err = scanner::install(&fx.core, &mut |_, _| {}, &cancel).unwrap_err();
    assert_eq!(err.code, ErrorCode::ScannerUnavailable);
}

#[test]
fn a_dangerous_external_verdict_needs_install_anyway() {
    let fx = Fx::new();
    let (mut prepared, review) = skills::review_local(&fx.core, &folder(&fx, "tidy")).unwrap();
    assert_eq!(review.scan.verdict, ScanVerdict::Clean);
    let root = fx.th.root().join("run").join("skill");
    let mut report = dangerous(&root);
    report.content_hash = Some(content_hash(prepared.files()));
    let result = ExternalScanResult::of(&fx.core.env, &report);
    assert_eq!(result.status, ExternalScanStatus::Scanned);
    assert_eq!(result.verdict, ScanVerdict::Dangerous);
    assert_eq!(result.findings[0].scanner, "snyk-agent-scan");
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"status\":\"scanned\""), "{json}");
    prepared.set_external(Some(report));

    let err = skills::install_reviewed(&fx.core, &mut prepared, None, false, false, None, None)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ScanBlocked);
    assert!(
        err.message.starts_with("Snyk Agent Scan found"),
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

#[test]
fn cancel_reaches_one_run_or_every_run() {
    let state = ScannerState::default();
    let first = state.start(Some("run-1".to_owned()));
    let second = state.start(Some("run-2".to_owned()));
    let install = state.start(None);
    state.cancel(Some("run-1"));
    assert!(first.flag.load(Ordering::SeqCst));
    assert!(!second.flag.load(Ordering::SeqCst), "another run goes on");
    assert!(!install.flag.load(Ordering::SeqCst));
    state.cancel(None);
    assert!(second.flag.load(Ordering::SeqCst));
    assert!(install.flag.load(Ordering::SeqCst));
    drop((first, second, install));
    let later = state.start(Some("run-1".to_owned()));
    assert!(
        !later.flag.load(Ordering::SeqCst),
        "a new run starts uncanceled"
    );
}

#[test]
fn one_install_at_a_time() {
    let state = ScannerState::default();
    let first = state.begin_install().unwrap();
    assert_eq!(
        state.begin_install().err().map(|err| err.code),
        Some(ErrorCode::Locked)
    );
    drop(first);
    assert!(state.begin_install().is_ok());
}

fn dangerous(root: &Path) -> external::ExternalReport {
    external::parse(
        output("dangerous.json").as_bytes(),
        b"",
        Some(0),
        root,
        external::VERSION,
    )
}

/// The pinned scanner as `status` sees it installed (the marker and a file
/// of the pinned size; never run here), turned on with a token reference,
/// as 0.7.0 left it; not installed where Snyk has no build.
fn turn_on_installed(fx: &Fx) {
    scanner::set_token(&fx.core, Some("keyring://loadout/snyk".to_owned())).unwrap();
    scanner::set_enabled(&fx.core, true).unwrap();
    let Some(asset) = external::pinned_asset() else {
        return;
    };
    let tool = Tool::pinned(&fx.core.home).unwrap();
    std::fs::create_dir_all(tool.dir()).unwrap();
    std::fs::File::create(tool.binary())
        .unwrap()
        .set_len(asset.size)
        .unwrap();
    let marker = serde_json::json!({
        "version": external::VERSION,
        "asset": asset.name,
        "sha256": asset.sha256,
        "size": asset.size,
    });
    std::fs::write(tool.dir().join("installed.json"), marker.to_string()).unwrap();
    let status = scanner::status(&fx.core).unwrap();
    assert!(status.installed && status.enabled, "{status:?}");
}

/// Review F7: turning the scanner off cancels every run, not the install.
#[test]
fn turning_the_scanner_off_cancels_runs_and_forgets_update_reports() {
    let fx = Fx::new();
    let state = ScannerState::default();
    let run = state.start(Some("run-1".to_owned()));
    let install = state.start(None);
    state.remember_update("tidy", dangerous(&fx.th.root().join("run")));
    state.turned_off();
    assert!(run.flag.load(Ordering::SeqCst), "the run goes on");
    assert!(
        !install.flag.load(Ordering::SeqCst),
        "the install was canceled"
    );
    assert!(state.take_update("tidy").is_none());
}

/// Off, or on but disabled in this build: a review scan attaches nothing.
#[test]
fn a_review_scan_attaches_nothing() {
    let fx = Fx::new();
    let (mut prepared, _) = skills::review_local(&fx.core, &folder(&fx, "tidy")).unwrap();
    let cancel = AtomicBool::new(false);
    for on in [false, true] {
        scanner::set_enabled(&fx.core, on).unwrap();
        let err = scanner::scan_review(&fx.core, &mut prepared, &cancel).unwrap_err();
        assert_eq!(err.code, ErrorCode::ScannerUnavailable);
        assert!(prepared.external().is_none());
    }
}

/// Review F1: a pasted token is refused, never echoed, never stored.
#[test]
fn a_pasted_token_is_refused_without_echo() {
    let fx = Fx::new();
    for pasted in [
        "3f2a9c1e-7b4d-4e8a-9c0f-1a2b3c4d5e6f",
        "deadbeef0123456789abcdef01234567",
    ] {
        let err = scanner::set_token(&fx.core, Some(pasted.to_owned())).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidRef);
        assert!(!err.message.contains(pasted), "{}", err.message);
        assert_eq!(scanner::status(&fx.core).unwrap().token_ref, None);
    }
}

/// Remove deletes a 0.7.0 install and, in this build, its settings too.
#[test]
fn removing_the_scanner_cleans_up() {
    let fx = Fx::new();
    turn_on_installed(&fx);
    let status = scanner::remove(&fx.core).unwrap();
    assert!(!status.enabled && !status.installed);
    assert_eq!(status.token_ref, None);
}

/// Review F10, inverted for this build: with a 0.7.0 setting that turned
/// an installed scanner on, no install or update waits for an external
/// report (`external_scan_required` never fires).
#[test]
fn installs_never_need_an_external_report_in_this_build() {
    let fx = Fx::new();
    turn_on_installed(&fx);
    let (mut prepared, _) = skills::review_local(&fx.core, &folder(&fx, "tidy")).unwrap();
    skills::install_reviewed(&fx.core, &mut prepared, None, false, false, None, None).unwrap();
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
    assert_ne!(err.code, ErrorCode::ExternalScanRequired);
    let err = skills::update(&fx.core, "tidy", "0123abc", false, None, None, None).unwrap_err();
    assert_ne!(err.code, ErrorCode::ExternalScanRequired);
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

/// Test gap and review M49 F1: `update_skill` with a remembered report,
/// driven as the command does (take, update, restore on failure): a
/// dangerous report blocks until allowed and is kept for the retry, a
/// report of other content is `entry_changed`, no report is needed in
/// this build, and once the scanner is turned off (after a review was
/// closed) the old report is gone and the update goes through.
#[test]
fn update_skill_gates_on_the_remembered_report() {
    let Some(git) = system_git_or_skip("update_skill_gates_on_the_remembered_report") else {
        return;
    };
    let fx = Fx::with_git("test-desk", &git);
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
    let push = |text: &str, message: &str| {
        std::fs::write(skill.join("SKILL.md"), CLEAN.replace("tidy", text)).unwrap();
        git_run(&fx, &git, &work, &["add", "-A"]);
        git_run(&fx, &git, &work, &["commit", "--quiet", "-m", message]);
        git_run(
            &fx,
            &git,
            &work,
            &["push", "--quiet", "origin", "HEAD:main"],
        );
    };
    push("alpha", "initial");
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
    push("alpha v2", "v2");
    let checks = skills::check_updates(&fx.core, &["alpha".to_owned()]).unwrap();
    let latest = checks[0].latest.clone().unwrap();
    let files = {
        let store = fx.core.open_store().unwrap().unwrap();
        install::cached_latest(&store, &skills::sx(&fx.core), "alpha")
            .unwrap()
            .unwrap()
    };
    let scanned = |hash: String| {
        let mut report = dangerous(&fx.th.root().join("run"));
        report.content_hash = Some(hash);
        report
    };
    // What `update_skill` does around `skills::update`.
    let state = ScannerState::default();
    let update_skill = |allow: bool| {
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

    // A 0.7.0 setting turned it on; a remembered report found the update
    // dangerous.
    turn_on_installed(&fx);
    state.remember_update("alpha", scanned(content_hash(&files)));
    let err = update_skill(false).unwrap_err();
    assert_eq!(err.code, ErrorCode::ScanBlocked);
    let err = update_skill(false).unwrap_err();
    assert_eq!(err.code, ErrorCode::ScanBlocked, "kept for the retry");
    // A report of other content does not gate this update.
    state.remember_update("alpha", scanned("sha256:other".to_owned()));
    assert_eq!(
        update_skill(false).unwrap_err().code,
        ErrorCode::EntryChanged
    );

    // Dangerous again, the review closed, the scanner turned off: the old
    // report is gone and the update goes through.
    state.remember_update("alpha", scanned(content_hash(&files)));
    scanner::set_enabled(&fx.core, false).unwrap();
    state.turned_off();
    let updated = update_skill(false).unwrap();
    assert_eq!(updated.to, latest);
    assert!(updated.external.is_none());
}

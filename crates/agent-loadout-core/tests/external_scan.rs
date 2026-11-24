//! The external scanner (Snyk Agent Scan, #27) against `agent-scan-stub`:
//! exactly `scan --json --storage-file <file> <skill>` is passed (never a
//! dangerous flag), the token only in a scrubbed environment, stdin closed;
//! recorded outputs become reports; a timeout or cancel kills the whole
//! process tree; a dangerous external verdict gates installs and updates
//! like the built-in one, while not-scanned never blocks. No network.

mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use agent_loadout_core::adapters::detect::NoRegistry;
use agent_loadout_core::remote::git::SystemRunner;
use agent_loadout_core::scan::Verdict;
use agent_loadout_core::scan::external::{
    self, ExternalReport, ExternalStatus, NotScanned, Runner, SCANNER,
};
use agent_loadout_core::secrets::SecretValue;
use agent_loadout_core::skills::install::{
    self, Decisions, InstallRequest, Naming, Source, UpdateStatus,
};
use agent_loadout_core::skills::{LinkKind, SkillFiles, SkillsEnv, content_hash, store};
use agent_loadout_core::sources::{self, SourceRef, SourceScope, TrustVia};
use agent_loadout_core::{Env, ErrorCode};
use agent_loadout_testkit::agent_scan_stub::{AgentScanStub, StubConfig, output};
use agent_loadout_testkit::git_env::{git_vars, init_bare, system_git_or_skip, with_git_vars};
use support::{Fx, host_os};

static RUNNER: SystemRunner = SystemRunner {
    idle_timeout: Duration::from_secs(60),
};
static NO_REGISTRY: NoRegistry = NoRegistry;

const TOKEN: &str = "snyk-test-token-5f1c2a";
const SKILL: &str = "---\nname: tidy\ndescription: Tidies.\n---\n\n# Tidy\n\nSort the imports.\n";
/// Never passed to the scanner.
const FORBIDDEN_FLAGS: &[&str] = &[
    "--dangerously-run-mcp-servers",
    "--ci",
    "--verbose",
    "--scan-all-users",
    "--skip-ssl-verify",
    "--push-key",
];

fn sx<'a>(fx: &'a Fx, env: &'a Env) -> SkillsEnv<'a> {
    SkillsEnv {
        env,
        fs: &fx.fs,
        home: &fx.home,
        registry: &NO_REGISTRY,
        runner: &RUNNER,
        link_kind: LinkKind::Junction,
    }
}

fn files() -> SkillFiles {
    let mut files = SkillFiles::new();
    files.insert("SKILL.md".to_owned(), SKILL.as_bytes().to_vec());
    files.insert(
        "scripts/run.sh".to_owned(),
        b"#!/bin/sh\necho tidy\n".to_vec(),
    );
    files
}

/// An env with variables that must never reach the scanner.
fn noisy_env(fx: &Fx) -> Env {
    fx.th
        .env_builder(host_os())
        .var("MACHINE_SERIAL", "SERIAL-42")
        .var("SNYK_API", "https://evil.example")
        .var("AGENT_SCAN_ENVIRONMENT", "dev")
        .var("SNYK_CLI_USE", "true")
        .var("GITHUB_TOKEN", "ghp_not_for_snyk")
        .var("SNYK_TOKEN", "from-the-process")
        .var("TMPDIR", fx.th.root().join("tmp"))
        .build()
}

fn runner(stub: &AgentScanStub, timeout: Duration) -> Runner {
    Runner::new(stub.tool().clone(), SecretValue::new(TOKEN), timeout)
}

fn scan(fx: &Fx, stub: &AgentScanStub) -> ExternalReport {
    runner(stub, Duration::from_secs(60)).scan_files(&noisy_env(fx), &fx.home, &files(), None)
}

#[test]
fn the_scanner_gets_one_skill_path_and_the_token_only_in_its_environment() {
    let fx = Fx::new();
    let stub = AgentScanStub::install(&fx.home);
    stub.answer(&output("clean.json"));
    let report = scan(&fx, &stub);
    assert_eq!(report.status, ExternalStatus::Scanned, "{report:?}");
    assert_eq!(report.verdict, Verdict::Clean);
    assert_eq!(report.scanner, SCANNER);
    assert_eq!(
        report.content_hash.as_deref(),
        Some(content_hash(&files()).as_str())
    );

    let calls = stub.calls();
    assert_eq!(calls.len(), 1, "one bounded run");
    let call = &calls[0];
    let argv: Vec<&str> = call.argv.iter().map(String::as_str).collect();
    assert_eq!(argv[..3], ["scan", "--json", "--storage-file"], "{argv:?}");
    assert_eq!(argv.len(), 5, "{argv:?}");
    let storage = Path::new(argv[3]);
    let skill = Path::new(argv[4]);
    let runs = fx.home.cache_dir().join(external::RUNS_DIR);
    for path in [storage, skill] {
        assert!(path.is_absolute(), "{path:?}");
        assert!(path.starts_with(&runs), "{path:?} under {runs:?}");
    }
    assert!(!argv[4].contains(':') || cfg!(windows), "no prefixed path");
    for flag in FORBIDDEN_FLAGS {
        assert!(!argv.contains(flag), "{flag} passed: {argv:?}");
    }
    assert!(argv.iter().all(|arg| !arg.contains(TOKEN)), "token in argv");
    // The staged copy is exactly the content.
    assert_eq!(call.files, ["SKILL.md", "scripts/run.sh"]);
    // Stdin is closed: every consent prompt reads EOF (declined).
    assert_eq!(call.stdin_bytes, 0);
    // The token from the reference, never the process's SNYK_TOKEN; a
    // scrubbed environment otherwise.
    assert_eq!(call.env.get("SNYK_TOKEN").map(String::as_str), Some(TOKEN));
    for name in [
        "MACHINE_SERIAL",
        "SNYK_API",
        "AGENT_SCAN_ENVIRONMENT",
        "SNYK_CLI_USE",
        "GITHUB_TOKEN",
    ] {
        assert!(
            !call.env.keys().any(|key| key.eq_ignore_ascii_case(name)),
            "{name} reached the scanner"
        );
    }
    let allowed: Vec<String> = external::PASSED_VARS
        .iter()
        .chain(external::TEMP_VARS.iter())
        .chain(["SNYK_TOKEN", "PYTHONIOENCODING", "PYTHONUTF8"].iter())
        .map(|name| name.to_ascii_uppercase())
        .collect();
    for key in call.env.keys() {
        // Windows adds a few variables to every process it starts.
        let windows_added = cfg!(windows) && key.starts_with('=');
        assert!(
            windows_added || allowed.contains(&key.to_ascii_uppercase()),
            "{key} reached the scanner"
        );
    }
    // The run folder is gone; the token is not in the report.
    assert!(!skill.exists() && !storage.exists());
    assert!(!format!("{report:?}").contains(TOKEN));
}

#[test]
fn recorded_outputs_become_reports() {
    let fx = Fx::new();
    let stub = AgentScanStub::install(&fx.home);
    for (name, status, reason, verdict) in [
        ("clean.json", ExternalStatus::Scanned, None, Verdict::Clean),
        (
            "warning.json",
            ExternalStatus::Scanned,
            None,
            Verdict::Warning,
        ),
        (
            "dangerous.json",
            ExternalStatus::Scanned,
            None,
            Verdict::Dangerous,
        ),
        (
            "limit.json",
            ExternalStatus::NotScanned,
            Some(NotScanned::LimitReached),
            Verdict::Clean,
        ),
        (
            "too-large.json",
            ExternalStatus::NotScanned,
            Some(NotScanned::TooLarge),
            Verdict::Clean,
        ),
        (
            "unauthorized.json",
            ExternalStatus::NotScanned,
            Some(NotScanned::TokenRejected),
            Verdict::Clean,
        ),
        (
            "malformed.txt",
            ExternalStatus::NotUnderstood,
            None,
            Verdict::Clean,
        ),
    ] {
        stub.answer(&output(name));
        let report = scan(&fx, &stub);
        assert_eq!(report.status, status, "{name}: {report:?}");
        assert_eq!(report.reason, reason, "{name}");
        assert_eq!(report.verdict, verdict, "{name}");
        assert_eq!(
            external::gate(&report, false).is_err(),
            verdict == Verdict::Dangerous,
            "{name}"
        );
    }
}

/// Review F8: `prepare_run` already guaranteed a token, so empty output is
/// a failure; only token wording on stderr makes it `token_rejected`.
#[test]
fn empty_output_is_failed_unless_stderr_names_the_token() {
    let fx = Fx::new();
    let stub = AgentScanStub::install(&fx.home);
    stub.set(&StubConfig {
        stdout: String::new(),
        exit: 1,
        ..StubConfig::default()
    });
    let report = scan(&fx, &stub);
    assert_eq!(report.status, ExternalStatus::NotScanned);
    assert_eq!(report.reason, Some(NotScanned::Failed), "{report:?}");
    assert!(
        report
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("scan finished")),
        "the stderr tail is the detail: {report:?}"
    );
    assert!(external::gate(&report, false).is_ok());

    stub.set(&StubConfig {
        stdout: String::new(),
        exit: 1,
        stderr: Some(format!(
            "Traceback ...\nSnykTokenError: the token {TOKEN} was not accepted\n"
        )),
        ..StubConfig::default()
    });
    let report = scan(&fx, &stub);
    assert_eq!(report.reason, Some(NotScanned::TokenRejected), "{report:?}");
    assert!(!format!("{report:?}").contains(TOKEN), "{report:?}");
}

/// Review F3: the scanner unpacks itself into the run folder (removed
/// afterwards), never into the user's temp folder.
#[test]
fn the_scanner_gets_the_run_folder_as_its_temp_folder() {
    let fx = Fx::new();
    let stub = AgentScanStub::install(&fx.home);
    stub.answer(&output("clean.json"));
    let report = scan(&fx, &stub);
    assert_eq!(report.status, ExternalStatus::Scanned, "{report:?}");
    let call = &stub.calls()[0];
    let runs = fx.home.cache_dir().join(external::RUNS_DIR);
    for name in ["TEMP", "TMP", "TMPDIR"] {
        let value = call
            .env
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| PathBuf::from(value))
            .unwrap_or_else(|| panic!("{name} not set: {:?}", call.env));
        assert!(value.starts_with(&runs), "{name}={value:?}");
        assert!(value.ends_with("tmp"), "{name}={value:?}");
    }
}

/// Test gap: stdout past the cap is truncated, so it is never trusted.
#[test]
fn output_past_the_cap_is_not_understood() {
    let fx = Fx::new();
    let stub = AgentScanStub::install(&fx.home);
    stub.set(&StubConfig {
        stdout: output("clean.json"),
        pad: 17 * 1024 * 1024,
        ..StubConfig::default()
    });
    let report = scan(&fx, &stub);
    assert_eq!(report.status, ExternalStatus::NotUnderstood, "{report:?}");
}

/// Review F4: run folders left by a killed process are swept by a later
/// run; a fresh one (another run in flight) is kept.
#[test]
fn old_run_folders_are_swept() {
    let fx = Fx::new();
    let stub = AgentScanStub::install(&fx.home);
    stub.answer(&output("clean.json"));
    let runs = fx.home.cache_dir().join(external::RUNS_DIR);
    let old = runs.join("run-0123456789abcdef");
    let fresh = runs.join("run-fedcba9876543210");
    for dir in [&old, &fresh] {
        std::fs::create_dir_all(dir.join("skill")).unwrap();
        std::fs::write(dir.join("skill").join("SKILL.md"), SKILL).unwrap();
        std::fs::write(dir.join("started"), "").unwrap();
    }
    let hours_ago = std::time::SystemTime::now() - Duration::from_secs(2 * 3600);
    std::fs::OpenOptions::new()
        .write(true)
        .open(old.join("started"))
        .unwrap()
        .set_modified(hours_ago)
        .unwrap();
    let report = scan(&fx, &stub);
    assert_eq!(report.status, ExternalStatus::Scanned, "{report:?}");
    assert!(!old.exists(), "an old run folder stays");
    assert!(fresh.exists(), "a fresh run folder was removed");
}

/// The heartbeat value, once the file exists (within `wait`).
fn beat(file: &Path, wait: Duration) -> Option<u64> {
    let start = Instant::now();
    loop {
        if let Some(value) = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| text.trim().parse().ok())
        {
            return Some(value);
        }
        if start.elapsed() > wait {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn hang(fx: &Fx, stub: &AgentScanStub) -> PathBuf {
    let heartbeat = fx.th.root().join("heartbeat.txt");
    stub.set(&StubConfig {
        hang: true,
        heartbeat: Some(heartbeat.clone()),
        ..StubConfig::default()
    });
    heartbeat
}

/// After the run, the grandchild must be gone: its heartbeat stops.
fn assert_tree_dead(heartbeat: &Path) {
    let first = beat(heartbeat, Duration::from_secs(5)).expect("the grandchild ran");
    std::thread::sleep(Duration::from_millis(400));
    let later = beat(heartbeat, Duration::from_secs(1)).unwrap();
    assert_eq!(first, later, "the scanner's child outlived the run");
}

#[test]
fn a_timeout_kills_the_whole_process_tree() {
    let fx = Fx::new();
    let stub = AgentScanStub::install(&fx.home);
    let heartbeat = hang(&fx, &stub);
    let start = Instant::now();
    let report = runner(&stub, Duration::from_millis(1500)).scan_files(
        &noisy_env(&fx),
        &fx.home,
        &files(),
        None,
    );
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(report.status, ExternalStatus::NotScanned);
    assert_eq!(report.reason, Some(NotScanned::Timeout));
    assert!(external::gate(&report, false).is_ok());
    assert_tree_dead(&heartbeat);
}

#[test]
fn a_cancel_kills_the_whole_process_tree() {
    let fx = Fx::new();
    let stub = AgentScanStub::install(&fx.home);
    let heartbeat = hang(&fx, &stub);
    let cancel = AtomicBool::new(false);
    let report = std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = beat(&heartbeat, Duration::from_secs(10));
            cancel.store(true, Ordering::SeqCst);
        });
        runner(&stub, Duration::from_secs(60)).scan_files(
            &noisy_env(&fx),
            &fx.home,
            &files(),
            Some(&cancel),
        )
    });
    assert_eq!(report.reason, Some(NotScanned::Canceled));
    assert_tree_dead(&heartbeat);
}

fn folder(fx: &Fx, name: &str) -> PathBuf {
    let dir = fx.th.root().join("src").join(name);
    fx.write(&dir.join("SKILL.md"), SKILL);
    dir
}

fn allow() -> Decisions {
    Decisions {
        allow_dangerous: true,
        ..Decisions::default()
    }
}

#[test]
fn a_dangerous_external_verdict_blocks_the_install_until_allowed() {
    let fx = Fx::new();
    let env = fx.th.env(host_os());
    let sx = sx(&fx, &env);
    let store = fx.store();
    let stub = AgentScanStub::install(&fx.home);
    let request = InstallRequest::new(Source::Local(folder(&fx, "tidy")));
    let mut prepared = install::prepare(&sx, &request).unwrap();
    assert_eq!(prepared.scan().verdict, Verdict::Clean, "built-in: clean");

    stub.answer(&output("dangerous.json"));
    let report =
        runner(&stub, Duration::from_secs(60)).scan_files(&env, &fx.home, prepared.files(), None);
    assert_eq!(report.verdict, Verdict::Dangerous);
    prepared.set_external(Some(report));
    let commits = fx.commit_count();
    let err = install::install_prepared(
        &store,
        &sx,
        &mut prepared,
        &Naming::default(),
        &Decisions::default(),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ScanBlocked);
    assert!(err.message().starts_with("Snyk Agent Scan found"), "{err}");
    assert_eq!(fx.commit_count(), commits, "nothing installed");
    let installed =
        install::install_prepared(&store, &sx, &mut prepared, &Naming::default(), &allow())
            .unwrap();
    assert_eq!(
        installed.external.as_ref().map(|report| report.verdict),
        Some(Verdict::Dangerous)
    );
}

#[test]
fn not_scanned_never_blocks_an_install() {
    let fx = Fx::new();
    let env = fx.th.env(host_os());
    let sx = sx(&fx, &env);
    let stub = AgentScanStub::install(&fx.home);
    stub.answer(&output("limit.json"));
    let request = InstallRequest::new(Source::Local(folder(&fx, "tidy")));
    let mut prepared = install::prepare(&sx, &request).unwrap();
    let report =
        runner(&stub, Duration::from_secs(60)).scan_files(&env, &fx.home, prepared.files(), None);
    assert_eq!(report.reason, Some(NotScanned::LimitReached));
    prepared.set_external(Some(report));
    let installed = install::install_prepared(
        &fx.store(),
        &sx,
        &mut prepared,
        &Naming::default(),
        &Decisions::default(),
    )
    .unwrap();
    assert_eq!(installed.id, "tidy");
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

fn push(fx: &Fx, git: &Path, work: &Path, message: &str) {
    git_run(fx, git, work, &["add", "-A"]);
    git_run(fx, git, work, &["commit", "--quiet", "-m", message]);
    git_run(fx, git, work, &["push", "--quiet", "origin", "HEAD:main"]);
}

#[test]
fn a_dangerous_external_verdict_blocks_the_update_of_that_content() {
    let Some(git) =
        system_git_or_skip("a_dangerous_external_verdict_blocks_the_update_of_that_content")
    else {
        return;
    };
    let fx = Fx::new();
    let env = with_git_vars(&fx.th, &git, fx.th.env_builder(host_os())).build();
    let sx = sx(&fx, &env);
    let store = fx.store();
    let stub = AgentScanStub::install(&fx.home);
    let bare = fx.th.root().join("up.git");
    init_bare(&fx.th, &git, &bare);
    let work = fx.th.root().join("work");
    git_run(
        &fx,
        &git,
        fx.th.root(),
        &["init", "--quiet", "--initial-branch=main", "work"],
    );
    git_run(&fx, &git, &work, &["remote", "add", "origin", "../up.git"]);
    fx.write(&work.join("SKILL.md"), SKILL);
    push(&fx, &git, &work, "initial");
    let url = bare.to_string_lossy().into_owned();
    let source = SourceRef::from_git_url(&url).unwrap();
    sources::trust(&store, &source, SourceScope::Repo, TrustVia::CliCommand).unwrap();
    install::install(
        &store,
        &sx,
        &InstallRequest::new(Source::parse(&url, &fx.fs)),
    )
    .unwrap();

    fx.write(&work.join("SKILL.md"), &SKILL.replace("Sort", "Order"));
    push(&fx, &git, &work, "v2");
    let check = install::check_update(&store, &sx, "tidy").unwrap();
    assert_eq!(check.status, UpdateStatus::Available);
    let latest = check.latest.clone().unwrap();
    let files = install::cached_latest(&store, &sx, "tidy")
        .unwrap()
        .unwrap();
    stub.answer(&output("dangerous.json"));
    let report = runner(&stub, Duration::from_secs(60)).scan_files(&env, &fx.home, &files, None);
    assert_eq!(report.verdict, Verdict::Dangerous);

    let commits = fx.commit_count();
    let blocked = Decisions {
        external: Some(report.clone()),
        ..Decisions::default()
    };
    let err = install::update_reviewed(&store, &sx, "tidy", &latest, &blocked).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ScanBlocked);
    assert_eq!(fx.commit_count(), commits, "nothing written");

    // A report of other content is refused: check again.
    let mut other = report.clone();
    other.content_hash = Some("sha256:0000".to_owned());
    let stale = Decisions {
        external: Some(other),
        allow_dangerous: true,
        ..Decisions::default()
    };
    let err = install::update_reviewed(&store, &sx, "tidy", &latest, &stale).unwrap_err();
    assert_eq!(err.code(), ErrorCode::EntryChanged);

    let allowed = Decisions {
        external: Some(report),
        allow_dangerous: true,
        ..Decisions::default()
    };
    let updated = install::update_reviewed(&store, &sx, "tidy", &latest, &allowed).unwrap();
    assert_eq!(
        updated.external.map(|report| report.verdict),
        Some(Verdict::Dangerous)
    );
    let content = store::read_content(&store, "tidy").unwrap();
    assert!(String::from_utf8_lossy(&content["SKILL.md"]).contains("Order"));
}

#[test]
fn a_run_needs_the_pinned_build_and_a_token() {
    let fx = Fx::new();
    let store = fx.store();
    let not_ready =
        external::prepare_run(&fx.env, &fx.fs, &fx.home, Some(&store), &fx.delivery).unwrap_err();
    let expected = if external::pinned_asset().is_some() {
        NotScanned::NotInstalled
    } else {
        NotScanned::UnsupportedPlatform
    };
    assert_eq!(not_ready.reason, Some(expected));

    // Token references resolve through the providers; values never show.
    let resolve = |reference: &str| {
        external::resolve_token(
            &fx.env,
            &fx.fs,
            &fx.home,
            Some(&store),
            &fx.delivery,
            reference,
        )
    };
    fx.keyring.insert("loadout", "snyk", TOKEN);
    assert_eq!(resolve("keyring://loadout/snyk").unwrap().expose(), TOKEN);
    let (reason, why) = resolve("keyring://loadout/absent").unwrap_err();
    assert_eq!(reason, NotScanned::TokenUnavailable);
    assert!(!why.contains(TOKEN));
    // Review F1: an alias may be a pasted token; its name is never shown.
    let (reason, why) = resolve("UNDEFINED_ALIAS").unwrap_err();
    assert_eq!(reason, NotScanned::TokenUnavailable);
    assert!(!why.contains("UNDEFINED_ALIAS"), "{why}");
}

/// Review F2: the binary is hashed before a command runs it; one changed
/// after install (same size) is not run, and counts as not installed.
#[test]
fn a_binary_changed_after_install_is_not_run() {
    let fx = Fx::new();
    let store = fx.store();
    let stub = AgentScanStub::install(&fx.home);
    fx.keyring.insert("loadout", "snyk", TOKEN);
    external::set_token(&fx.fs, &fx.home, None, Some("keyring://loadout/snyk")).unwrap();
    // The `Err` is a report, as `prepare_run`'s.
    #[allow(clippy::result_large_err)]
    let prepare = || {
        external::prepare_run_with(
            stub.tool().clone(),
            &fx.env,
            &fx.fs,
            &fx.home,
            Some(&store),
            &fx.delivery,
        )
    };
    assert!(prepare().is_ok());
    let binary = stub.tool().binary();
    let mut bytes = std::fs::read(&binary).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    std::fs::write(&binary, &bytes).unwrap();
    let report = prepare().unwrap_err();
    assert_eq!(report.reason, Some(NotScanned::NotInstalled), "{report:?}");
    assert!(
        report.detail.as_deref().unwrap().contains("SHA-256"),
        "{report:?}"
    );
    assert!(!stub.tool().installed(), "the marker stays");
    assert!(stub.calls().is_empty());
}

/// Review F1: an alias is stored only when the secrets catalog defines it.
#[test]
fn a_token_alias_must_be_a_defined_secret() {
    let fx = Fx::new();
    let store = fx.store();
    store
        .transaction(
            agent_loadout_core::store::Operation::new("test.edit", "secrets: catalog"),
            |tx| {
                tx.write(
                    "secrets.toml",
                    "schema = 1\n\n[secrets.SNYK_TOKEN]\nref = \"keyring://loadout/snyk\"\n",
                )
            },
        )
        .unwrap();
    let read: &dyn agent_loadout_core::store::StoreRead = &store;
    let pasted = "deadbeef0123456789abcdef01234567";
    let err = external::set_token(&fx.fs, &fx.home, Some(read), Some(pasted)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidRef);
    assert!(!err.message().contains(pasted), "{}", err.message());
    assert_eq!(external::settings(&fx.fs, &fx.home).unwrap().token, None);
    external::set_token(&fx.fs, &fx.home, Some(read), Some("SNYK_TOKEN")).unwrap();
    assert_eq!(
        external::settings(&fx.fs, &fx.home)
            .unwrap()
            .token
            .as_deref(),
        Some("SNYK_TOKEN")
    );
}

//! The optional external scanner since 0.8.0, NVIDIA SkillSpector (#30),
//! against `skillspector-stub` copied as `skillspector`, `uvx` and `uv`
//! into a folder that is the only `PATH` entry of the core `Env`:
//! detection order, Prepare, exactly the documented arguments (uv always
//! `--offline` for scans), a scrubbed environment without provider keys
//! and with LangSmith tracing off, stdin closed, a staged copy that is
//! removed afterwards, timeout and cancel killing the whole tree, every
//! hand-made report through a real run, and the install/update gates. No
//! network.

mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use agent_loadout_core::adapters::detect::NoRegistry;
use agent_loadout_core::remote::git::SystemRunner;
use agent_loadout_core::scan::external::skillspector::{
    self, Availability, PASSED_VARS, Runner, Runtime, RuntimeKind, SCANNER, SET_VARS, TEMP_VARS,
    VERSION,
};
use agent_loadout_core::scan::external::{
    self, Choice, ExternalReport, ExternalStatus, NotScanned,
};
use agent_loadout_core::scan::{Severity, Verdict};
use agent_loadout_core::skills::install::{
    self, Decisions, InstallRequest, Naming, Source, UpdateStatus,
};
use agent_loadout_core::skills::{LinkKind, SkillFiles, SkillsEnv, content_hash, store};
use agent_loadout_core::sources::{self, SourceRef, SourceScope, TrustVia};
use agent_loadout_core::{Env, ErrorCode};
use agent_loadout_testkit::git_env::{git_vars, init_bare, system_git_or_skip, with_git_vars};
use agent_loadout_testkit::skillspector_stub::{
    SKILL_ESCAPED_PLACEHOLDER, SKILL_PLACEHOLDER, SkillSpectorStub, StubCall, StubConfig, output,
};
use support::{Fx, host_os};

static RUNNER: SystemRunner = SystemRunner {
    idle_timeout: Duration::from_secs(60),
};
static NO_REGISTRY: NoRegistry = NoRegistry;

const SKILL: &str = "---\nname: tidy\ndescription: Tidies.\n---\n\n# Tidy\n\nSort the imports.\n";

/// Never passed to SkillSpector or uv.
const FORBIDDEN: &[&str] = &[
    "--output",
    "-o",
    "--transitive",
    "--transitive-depth",
    "--transitive-allow-prefix",
    "--transitive-deny-prefix",
    "--inspect-dependencies",
    "--use-shipped-baseline",
    "--baseline",
    "-b",
    "--mcp-registry",
    "--mcp-registry-compare",
    "--yara-rules-dir",
    "--recursive",
    "-r",
    "--llm",
    "--model",
    "--provider",
];

/// Set in the parent `Env`; none may reach the child (name, value).
const PROVIDER_VARS: &[(&str, &str)] = &[
    ("OPENAI_API_KEY", "sk-openai-test-1f2e3d"),
    ("OPENAI_BASE_URL", "https://openai.example"),
    ("ANTHROPIC_API_KEY", "sk-ant-test-4c5b6a"),
    ("AWS_ACCESS_KEY_ID", "AKIATESTTESTTEST"),
    ("AWS_SECRET_ACCESS_KEY", "aws-secret-test-778899"),
    ("AWS_SESSION_TOKEN", "aws-session-test-112233"),
    ("AWS_PROFILE", "prod-admin"),
    ("NVIDIA_API_KEY", "nvapi-test-0a0b0c"),
    ("NVIDIA_NIM_API_KEY", "nim-test-0d0e0f"),
    ("GOOGLE_API_KEY", "google-test-445566"),
    ("LANGSMITH_API_KEY", "lsv2-test-998877"),
    ("LANGSMITH_ENDPOINT", "https://langsmith.example"),
    ("LANGCHAIN_API_KEY", "lc-test-665544"),
    ("SKILLSPECTOR_LLM_PROVIDER", "openai"),
    ("SKILLSPECTOR_API_KEY", "skillspector-test-332211"),
    ("GITHUB_TOKEN", "ghp_not_for_skillspector"),
    ("MACHINE_SERIAL", "SERIAL-42"),
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

/// A stub folder `<root>/tools/bin` holding `programs`.
fn stub(fx: &Fx, programs: &[&str]) -> SkillSpectorStub {
    let stub = SkillSpectorStub::new(&fx.th.root().join("tools"));
    for program in programs {
        stub.add(program);
    }
    stub
}

/// The user's temp folder in the parent `Env` (never the child's).
fn user_temp(fx: &Fx) -> PathBuf {
    fx.th.root().join("user-temp")
}

/// `PATH` = the stub folder alone, plus every variable that must never
/// reach the scanner, tracing turned on and a temp folder of the user's.
fn noisy_env(fx: &Fx, stub: &SkillSpectorStub) -> Env {
    let mut builder = fx
        .th
        .env_builder(host_os())
        .var("PATH", stub.bin())
        .var("HOME", fx.th.home())
        .var("LANGSMITH_TRACING", "true")
        .var("LANGCHAIN_TRACING_V2", "true")
        .var("TEMP", user_temp(fx))
        .var("TMP", user_temp(fx))
        .var("TMPDIR", user_temp(fx));
    for (name, value) in PROVIDER_VARS {
        builder = builder.var(name, *value);
    }
    builder.build()
}

/// An env whose `PATH` is only the stub folder.
fn path_env(fx: &Fx, stub: &SkillSpectorStub) -> Env {
    fx.th.env_builder(host_os()).var("PATH", stub.bin()).build()
}

fn runs(fx: &Fx) -> PathBuf {
    fx.home.cache_dir().join(external::RUNS_DIR)
}

/// Run folders left behind.
fn leftover_runs(fx: &Fx) -> Vec<PathBuf> {
    std::fs::read_dir(runs(fx))
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

fn ready(env: &Env, fx: &Fx) -> Runtime {
    match skillspector::detect(env, &fx.home) {
        Availability::Ready(runtime) => runtime,
        other => panic!("not ready: {other:?}"),
    }
}

fn runner(env: &Env, fx: &Fx) -> Runner {
    Runner::new(ready(env, fx), Duration::from_secs(60))
}

fn program(stub: &SkillSpectorStub, name: &str) -> PathBuf {
    stub.bin()
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}

/// What every call must look like: stdin closed, no forbidden flag or
/// URL, no provider key or other variable, tracing off, the run's own
/// temp folder, no secret value anywhere.
fn assert_clean_call(fx: &Fx, call: &StubCall) {
    assert_eq!(call.stdin_bytes, 0, "stdin was not closed: {call:?}");
    for arg in &call.argv {
        assert!(
            !FORBIDDEN.contains(&arg.as_str()),
            "{arg} passed: {:?}",
            call.argv
        );
        assert!(!arg.contains("://"), "a URL passed: {:?}", call.argv);
    }
    let allowed: Vec<String> = PASSED_VARS
        .iter()
        .chain(TEMP_VARS.iter())
        .chain(SET_VARS.iter().map(|(name, _)| name))
        .map(|name| name.to_ascii_uppercase())
        .collect();
    for (key, value) in &call.env {
        // Windows adds `=C:`-style variables to every process it starts;
        // macOS adds __CF_USER_TEXT_ENCODING even after an env_clear.
        let os_added = (cfg!(windows) && key.starts_with('='))
            || (cfg!(target_os = "macos") && key == "__CF_USER_TEXT_ENCODING");
        assert!(
            os_added || allowed.contains(&key.to_ascii_uppercase()),
            "{key} reached {}",
            call.program
        );
        for (name, secret) in PROVIDER_VARS {
            assert!(!value.contains(secret), "{name}'s value in {key}");
        }
    }
    let get = |name: &str| {
        call.env
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(get("LANGSMITH_TRACING"), Some("false"), "{:?}", call.env);
    assert_eq!(get("LANGCHAIN_TRACING_V2"), Some("false"), "{:?}", call.env);
    assert_eq!(get("NO_COLOR"), Some("1"));
    assert_eq!(get("PYTHONUTF8"), Some("1"));
    assert_eq!(get("PYTHONIOENCODING"), Some("utf-8"));
    for name in TEMP_VARS {
        let value = PathBuf::from(get(name).unwrap_or_else(|| panic!("{name} not set")));
        assert!(value.starts_with(runs(fx)), "{name}={value:?}");
        assert!(value.ends_with("tmp"), "{name}={value:?}");
        assert!(!value.starts_with(user_temp(fx)), "{name}={value:?}");
    }
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|arg| (*arg).to_owned()).collect()
}

/// The pinned form's arguments before SkillSpector's own.
fn pinned(uv: bool, offline: bool) -> Vec<String> {
    let mut out = Vec::new();
    if uv {
        out.extend(args(&["tool", "run"]));
    }
    if offline {
        out.push("--offline".to_owned());
    }
    out.extend(args(&[
        "--python",
        "3.13",
        "--from",
        "skillspector==2.13.7",
        "skillspector",
    ]));
    out
}

fn scan_args() -> Vec<String> {
    args(&[
        "scan",
        "--no-llm",
        "--format",
        "json",
        "--fail-on-incomplete",
        "--",
    ])
}

#[test]
fn each_runtime_gets_exactly_the_documented_arguments() {
    for (programs, kind, uv) in [
        (&["uvx", "skillspector"][..], RuntimeKind::Uvx, Some(false)),
        (&["uv"][..], RuntimeKind::Uvx, Some(true)),
        (&["skillspector"][..], RuntimeKind::Path, None),
    ] {
        let fx = Fx::new();
        let stub = stub(&fx, programs);
        stub.set_prepared(true);
        stub.answer(&output("safe.json"), 0);
        let env = noisy_env(&fx, &stub);
        let runtime = ready(&env, &fx);
        assert_eq!(runtime.kind, kind, "{programs:?}");
        let report = Runner::new(runtime, Duration::from_secs(60)).scan_files(
            &env,
            &fx.home,
            &files(),
            None,
        );
        assert_eq!(report.status, ExternalStatus::Scanned, "{report:?}");
        assert_eq!(report.verdict, Verdict::Clean);
        assert_eq!(
            report.content_hash.as_deref(),
            Some(content_hash(&files()).as_str())
        );

        let calls = stub.calls();
        assert_eq!(calls.len(), 2, "one probe, one scan: {calls:?}");
        for call in &calls {
            assert_clean_call(&fx, call);
        }
        let (probe, scan) = (&calls[0], &calls[1]);
        assert!(!probe.is_scan() && scan.is_scan());
        let skill = PathBuf::from(scan.argv.last().unwrap());
        assert!(skill.is_absolute(), "{skill:?}");
        assert!(skill.starts_with(runs(&fx)), "{skill:?}");
        assert!(skill.ends_with("skill"), "{skill:?}");
        let mut expected = match uv {
            Some(uv) => pinned(uv, true),
            None => Vec::new(),
        };
        let mut version = expected.clone();
        version.push("--version".to_owned());
        expected.extend(scan_args());
        expected.push(skill.to_string_lossy().into_owned());
        assert_eq!(scan.argv, expected, "{programs:?}");
        assert_eq!(
            probe.argv, version,
            "{programs:?}: the probe is offline too"
        );
        let name = match uv {
            Some(true) => "uv",
            Some(false) => "uvx",
            None => "skillspector",
        };
        assert_eq!(scan.program, name);
        // Exactly the content, staged, then removed with the run folder.
        assert_eq!(scan.files, ["SKILL.md", "scripts/run.sh"]);
        assert!(!skill.exists(), "the staged copy stays");
        assert!(leftover_runs(&fx).is_empty(), "{:?}", leftover_runs(&fx));
    }
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

/// After the run, the grandchild must be gone: its heartbeat stops.
fn assert_tree_dead(heartbeat: &Path) {
    let first = beat(heartbeat, Duration::from_secs(5)).expect("the grandchild ran");
    std::thread::sleep(Duration::from_millis(400));
    let later = beat(heartbeat, Duration::from_secs(1)).unwrap();
    assert_eq!(first, later, "SkillSpector's child outlived the run");
}

fn heartbeat(fx: &Fx) -> PathBuf {
    fx.th.root().join("heartbeat.txt")
}

fn hanging(fx: &Fx, stub: &SkillSpectorStub) -> PathBuf {
    let file = heartbeat(fx);
    stub.set(&StubConfig {
        hang: true,
        heartbeat: Some(file.clone()),
        ..StubConfig::default()
    });
    file
}

/// A timeout kills the whole tree, also a child the scanner starts first
/// thing (as `uvx` starts `uv`), and the run folder is gone on return.
#[test]
fn a_timeout_kills_the_whole_process_tree() {
    let fx = Fx::new();
    let stub = stub(&fx, &["uvx"]);
    stub.set_prepared(true);
    let env = noisy_env(&fx, &stub);
    let runtime = ready(&env, &fx);
    let heartbeat = heartbeat(&fx);
    let early = fx.th.root().join("early-heartbeat.txt");
    stub.set(&StubConfig {
        hang: true,
        heartbeat: Some(heartbeat.clone()),
        early_heartbeat: Some(early.clone()),
        ..StubConfig::default()
    });
    let start = Instant::now();
    let report =
        Runner::new(runtime, Duration::from_secs(4)).scan_files(&env, &fx.home, &files(), None);
    assert!(
        start.elapsed() < Duration::from_secs(60),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(report.status, ExternalStatus::NotScanned, "{report:?}");
    assert_eq!(report.reason, Some(NotScanned::Timeout));
    assert_eq!(report.verdict, Verdict::Clean);
    assert!(external::gate(&report, false).is_ok());
    assert!(leftover_runs(&fx).is_empty(), "{:?}", leftover_runs(&fx));
    assert_tree_dead(&heartbeat);
    assert_tree_dead(&early);
}

#[test]
fn a_cancel_kills_the_whole_process_tree() {
    let fx = Fx::new();
    let stub = stub(&fx, &["skillspector"]);
    let env = noisy_env(&fx, &stub);
    let runner = Runner::new(ready(&env, &fx), Duration::from_secs(300));
    let heartbeat = hanging(&fx, &stub);
    let cancel = AtomicBool::new(false);
    let report = std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = beat(&heartbeat, Duration::from_secs(120));
            cancel.store(true, Ordering::SeqCst);
        });
        runner.scan_files(&env, &fx.home, &files(), Some(&cancel))
    });
    assert_eq!(report.status, ExternalStatus::NotScanned, "{report:?}");
    assert_eq!(report.reason, Some(NotScanned::Canceled));
    assert!(external::gate(&report, false).is_ok());
    assert!(leftover_runs(&fx).is_empty(), "{:?}", leftover_runs(&fx));
    assert_tree_dead(&heartbeat);
}

fn availability(fx: &Fx, stub: &SkillSpectorStub) -> Availability {
    skillspector::detect(&path_env(fx, stub), &fx.home)
}

#[test]
fn detection_prefers_a_ready_uvx_then_skillspector_then_prepare() {
    // Nothing on PATH, or no PATH at all.
    let fx = Fx::new();
    let stub = stub(&fx, &[]);
    assert_eq!(availability(&fx, &stub), Availability::NotAvailable);
    assert_eq!(
        skillspector::detect(&fx.env, &fx.home),
        Availability::NotAvailable
    );
    // uvx without the download: Prepare.
    let uvx = stub.add("uvx");
    assert_eq!(
        availability(&fx, &stub),
        Availability::NeedsPrepare { uv: uvx.clone() }
    );
    // A skillspector on PATH runs meanwhile.
    let path = stub.add("skillspector");
    let Availability::Ready(runtime) = availability(&fx, &stub) else {
        panic!("skillspector on PATH is ready");
    };
    assert_eq!(runtime.kind, RuntimeKind::Path);
    assert_eq!(runtime.program, path);
    assert_eq!(runtime.version, VERSION);
    assert!(runtime.tested());
    // A prepared uvx wins over it.
    stub.set_prepared(true);
    let Availability::Ready(runtime) = availability(&fx, &stub) else {
        panic!("a prepared uvx is ready");
    };
    assert_eq!(runtime.kind, RuntimeKind::Uvx);
    assert_eq!(runtime.program, uvx);
    assert_eq!(runtime.version, VERSION);
    // uvx is preferred over uv.
    stub.add("uv");
    assert_eq!(ready(&path_env(&fx, &stub), &fx).program, uvx);
    // Probes never download: none ran without --offline.
    assert!(!stub.calls().is_empty());
    for call in stub.calls() {
        assert_clean_call(&fx, &call);
        if call.program != "skillspector" {
            assert!(call.argv.iter().any(|arg| arg == "--offline"), "{call:?}");
        }
    }

    // Only uv: `uv tool run`.
    let fx = Fx::new();
    let stub = self::stub(&fx, &["uv"]);
    let uv = program(&stub, "uv");
    assert_eq!(
        availability(&fx, &stub),
        Availability::NeedsPrepare { uv: uv.clone() }
    );
    stub.set_prepared(true);
    let runtime = ready(&path_env(&fx, &stub), &fx);
    assert_eq!(runtime.kind, RuntimeKind::Uvx);
    assert_eq!(runtime.program, uv);
    assert!(
        runtime
            .command_line()
            .ends_with(" tool run --python 3.13 --from skillspector==2.13.7 skillspector"),
        "{}",
        runtime.command_line()
    );

    // A skillspector whose --version fails is not usable.
    let fx = Fx::new();
    let stub = self::stub(&fx, &["skillspector"]);
    stub.set(&StubConfig {
        version_exit: 1,
        ..StubConfig::default()
    });
    assert_eq!(availability(&fx, &stub), Availability::NotAvailable);
    let uvx = stub.add("uvx");
    assert_eq!(
        availability(&fx, &stub),
        Availability::NeedsPrepare { uv: uvx }
    );
    // Nor one that prints no version.
    let fx = Fx::new();
    let stub = self::stub(&fx, &["skillspector"]);
    stub.set(&StubConfig {
        version: "skillspector".to_owned(),
        ..StubConfig::default()
    });
    assert_eq!(availability(&fx, &stub), Availability::NotAvailable);
}

/// On Windows only `.exe` files are run, never a batch file.
#[cfg(windows)]
#[test]
fn detection_ignores_batch_files_on_windows() {
    let fx = Fx::new();
    let stub = stub(&fx, &[]);
    for name in ["skillspector.cmd", "skillspector.bat", "uvx.cmd", "uv.bat"] {
        std::fs::write(stub.bin().join(name), "@echo skillspector 2.13.7\r\n").unwrap();
    }
    assert_eq!(availability(&fx, &stub), Availability::NotAvailable);
    assert!(stub.calls().is_empty());
}

#[test]
fn another_version_on_path_runs_with_a_note() {
    let fx = Fx::new();
    let stub = stub(&fx, &["skillspector"]);
    stub.set(&StubConfig {
        version: "skillspector, version 2.14.0".to_owned(),
        stdout: output("safe.json"),
        ..StubConfig::default()
    });
    let env = path_env(&fx, &stub);
    let runtime = ready(&env, &fx);
    assert_eq!(runtime.kind, RuntimeKind::Path);
    assert_eq!(runtime.version, "2.14.0");
    assert!(!runtime.tested());
    assert_eq!(
        runtime.note().as_deref(),
        Some("SkillSpector 2.14.0 is not tested with this Loadout (tested: 2.13.7)")
    );
    let status = skillspector::status(&env, &fx.fs, &fx.home).unwrap();
    assert_eq!(status.availability, Availability::Ready(runtime.clone()));
    assert!(!status.enabled);
    assert!(!status.snyk_installed);
    assert_eq!(status.timeout, skillspector::DEFAULT_TIMEOUT);
    // It still scans.
    let report =
        Runner::new(runtime, Duration::from_secs(60)).scan_files(&env, &fx.home, &files(), None);
    assert_eq!(report.status, ExternalStatus::Scanned, "{report:?}");

    // The pinned form is the tested version.
    let fx = Fx::new();
    let stub = self::stub(&fx, &["uvx"]);
    stub.set_prepared(true);
    let runtime = ready(&path_env(&fx, &stub), &fx);
    assert!(runtime.tested());
    assert_eq!(runtime.note(), None);
}

/// Prepare's lines and result.
fn prepare(
    fx: &Fx,
    env: &Env,
    cancel: Option<&AtomicBool>,
) -> (Result<Runtime, agent_loadout_core::CoreError>, Vec<String>) {
    let mut lines = Vec::new();
    let result = skillspector::prepare(
        env,
        &fx.home,
        &mut |line: &str| lines.push(line.to_owned()),
        cancel,
    );
    (result, lines)
}

const PREPARE_LINES: &[&str] = &[
    "Resolved 42 packages in 1.20s",
    "Downloading skillspector (1.1MiB)",
    "\tInstalled 42 packages in 310ms",
];

#[test]
fn prepare_downloads_once_and_reports_progress() {
    for (name, uv) in [("uvx", false), ("uv", true)] {
        let fx = Fx::new();
        let stub = stub(&fx, &[name]);
        stub.set(&StubConfig {
            prepare_lines: PREPARE_LINES
                .iter()
                .map(|line| (*line).to_owned())
                .collect(),
            ..StubConfig::default()
        });
        let env = noisy_env(&fx, &stub);
        assert!(matches!(
            skillspector::detect(&env, &fx.home),
            Availability::NeedsPrepare { .. }
        ));
        let (result, lines) = prepare(&fx, &env, None);
        let runtime = result.unwrap();
        assert_eq!(runtime.kind, RuntimeKind::Uvx);
        assert_eq!(runtime.program, program(&stub, name));
        assert_eq!(runtime.version, VERSION);
        assert_eq!(
            lines,
            [
                "Resolved 42 packages in 1.20s",
                "Downloading skillspector (1.1MiB)",
                "Installed 42 packages in 310ms",
            ],
            "{name}"
        );
        assert!(stub.prepared());
        assert_eq!(ready(&env, &fx), runtime);

        // Prepare is the one call without --offline.
        let calls = stub.calls();
        let downloads: Vec<&StubCall> = calls
            .iter()
            .filter(|call| !call.argv.iter().any(|arg| arg == "--offline"))
            .collect();
        assert_eq!(downloads.len(), 1, "{calls:?}");
        let mut expected = pinned(uv, false);
        expected.push("--version".to_owned());
        assert_eq!(downloads[0].argv, expected);
        for call in &calls {
            assert_clean_call(&fx, call);
        }
        assert!(leftover_runs(&fx).is_empty(), "{:?}", leftover_runs(&fx));
    }
}

#[test]
fn a_failed_prepare_names_uvs_last_line() {
    let fx = Fx::new();
    let stub = stub(&fx, &["uvx"]);
    stub.set(&StubConfig {
        prepare_lines: vec!["Resolved 42 packages in 1.20s".to_owned()],
        prepare_exit: 2,
        ..StubConfig::default()
    });
    let env = noisy_env(&fx, &stub);
    let (result, lines) = prepare(&fx, &env, None);
    let err = result.unwrap_err();
    assert_eq!(err.code(), ErrorCode::ScannerDownloadFailed);
    assert!(
        err.message()
            .ends_with("error: Failed to fetch: `https://pypi.org/simple/skillspector/`"),
        "{}",
        err.message()
    );
    assert!(err.message().contains("exited with code 2"), "{err}");
    assert_eq!(lines[0], "Resolved 42 packages in 1.20s");
    assert!(!stub.prepared());
    assert!(matches!(
        skillspector::detect(&env, &fx.home),
        Availability::NeedsPrepare { .. }
    ));
    assert!(leftover_runs(&fx).is_empty(), "{:?}", leftover_runs(&fx));
}

#[test]
fn a_canceled_prepare_kills_the_whole_process_tree() {
    let fx = Fx::new();
    let stub = stub(&fx, &["uvx"]);
    let file = heartbeat(&fx);
    stub.set(&StubConfig {
        prepare_lines: vec!["Downloading cpython-3.13".to_owned()],
        prepare_hang: true,
        heartbeat: Some(file.clone()),
        ..StubConfig::default()
    });
    let env = noisy_env(&fx, &stub);
    let cancel = AtomicBool::new(false);
    let (result, lines) = std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = beat(&file, Duration::from_secs(120));
            cancel.store(true, Ordering::SeqCst);
        });
        prepare(&fx, &env, Some(&cancel))
    });
    let err = result.unwrap_err();
    assert_eq!(err.code(), ErrorCode::Canceled, "{err}");
    assert_eq!(lines, ["Downloading cpython-3.13"]);
    assert_tree_dead(&file);
    assert!(!stub.prepared());
    assert!(leftover_runs(&fx).is_empty(), "{:?}", leftover_runs(&fx));
}

#[test]
fn prepare_needs_uv() {
    let fx = Fx::new();
    let stub = stub(&fx, &[]);
    for env in [fx.env.clone(), path_env(&fx, &stub)] {
        let (result, lines) = prepare(&fx, &env, None);
        let err = result.unwrap_err();
        assert_eq!(err.code(), ErrorCode::ScannerUnavailable);
        assert!(err.message().contains(skillspector::SETUP), "{err}");
        assert!(lines.is_empty());
    }
    // A skillspector on PATH is not uv.
    stub.add("skillspector");
    let (result, _) = prepare(&fx, &path_env(&fx, &stub), None);
    assert_eq!(result.unwrap_err().code(), ErrorCode::ScannerUnavailable);
    assert!(stub.calls().is_empty(), "nothing ran");
}

/// One run of `stdout` exiting with `exit` through `runtime`'s program.
fn run_answer(fx: &Fx, stub: &SkillSpectorStub, config: StubConfig) -> ExternalReport {
    let env = noisy_env(fx, stub);
    let runner = runner(&env, fx);
    stub.set(&config);
    let report = runner.scan_files(&env, &fx.home, &files(), None);
    for call in stub.calls() {
        assert_clean_call(fx, &call);
    }
    assert!(leftover_runs(fx).is_empty(), "{:?}", leftover_runs(fx));
    report
}

fn answer(stdout: String, exit: i32) -> StubConfig {
    StubConfig {
        stdout,
        exit,
        ..StubConfig::default()
    }
}

fn rules(report: &ExternalReport) -> Vec<(&str, Severity)> {
    report
        .findings
        .iter()
        .map(|f| (f.rule.as_str(), f.severity))
        .collect()
}

#[test]
fn every_report_maps_through_a_real_run() {
    for programs in [&["skillspector"][..], &["uvx"][..]] {
        let fx = Fx::new();
        let stub = stub(&fx, programs);
        stub.set_prepared(true);
        for (name, exit, status, verdict, incomplete) in [
            (
                "safe.json",
                0,
                ExternalStatus::Scanned,
                Verdict::Clean,
                false,
            ),
            (
                "caution.json",
                0,
                ExternalStatus::Scanned,
                Verdict::Warning,
                false,
            ),
            (
                "do-not-install.json",
                1,
                ExternalStatus::Scanned,
                Verdict::Dangerous,
                false,
            ),
            (
                "do-not-install-high.json",
                1,
                ExternalStatus::Scanned,
                Verdict::Dangerous,
                false,
            ),
            (
                "incomplete.json",
                1,
                ExternalStatus::Scanned,
                Verdict::Clean,
                true,
            ),
            (
                "osv-unreachable.json",
                0,
                ExternalStatus::Scanned,
                Verdict::Clean,
                false,
            ),
            (
                "execution-failed.json",
                1,
                ExternalStatus::NotUnderstood,
                Verdict::Clean,
                false,
            ),
            (
                "llm-requested.json",
                0,
                ExternalStatus::NotUnderstood,
                Verdict::Clean,
                false,
            ),
            (
                "malformed.txt",
                0,
                ExternalStatus::NotUnderstood,
                Verdict::Clean,
                false,
            ),
        ] {
            let report = run_answer(&fx, &stub, answer(output(name), exit));
            assert_eq!(report.scanner, SCANNER);
            assert_eq!(report.status, status, "{name}: {report:?}");
            assert_eq!(report.reason, None, "{name}");
            assert_eq!(report.verdict, verdict, "{name}");
            assert_eq!(report.incomplete, incomplete, "{name}");
            assert_eq!(
                report.content_hash.as_deref(),
                Some(content_hash(&files()).as_str()),
                "{name}"
            );
            assert!(
                report.findings.iter().all(|f| f.scanner == "skillspector"),
                "{name}"
            );
            assert_eq!(
                external::gate(&report, false).is_err(),
                verdict == Verdict::Dangerous,
                "{name}"
            );
            assert!(external::gate(&report, true).is_ok(), "{name}");
            match name {
                "do-not-install.json" => {
                    assert!(report.findings.iter().all(|f| f.rule != "DO_NOT_INSTALL"));
                }
                "do-not-install-high.json" => {
                    let forced: Vec<_> = report.dangerous().collect();
                    assert_eq!(forced.len(), 1, "{report:?}");
                    assert_eq!(forced[0].rule, "DO_NOT_INSTALL");
                    assert_eq!(
                        forced[0].message,
                        "SkillSpector recommends not installing this skill (risk score 78 of 100)"
                    );
                }
                "incomplete.json" => {
                    assert_eq!(report.summary(), "not fully scanned (2 findings)");
                    let note = report
                        .findings
                        .iter()
                        .find(|f| f.rule == "incomplete")
                        .unwrap();
                    assert_eq!(note.severity, Severity::Info);
                    assert_eq!(note.message, "not fully scanned (partial, 87.5% coverage)");
                }
                "osv-unreachable.json" => {
                    assert_eq!(rules(&report), [("SC4", Severity::Info)]);
                }
                "caution.json" => {
                    assert_eq!(
                        rules(&report),
                        [("P3", Severity::Warning), ("E2", Severity::Warning)]
                    );
                    assert_eq!(report.findings[1].file.as_deref(), Some("scripts/sync.py"));
                }
                _ => {}
            }
            if status == ExternalStatus::NotUnderstood {
                assert_eq!(
                    report.summary(),
                    "scanner output not understood (version 2.13.7)",
                    "{name}"
                );
            }
        }
    }
}

#[test]
fn a_scanner_error_is_failed_without_the_staged_path() {
    let fx = Fx::new();
    let stub = stub(&fx, &["skillspector"]);
    let report = run_answer(
        &fx,
        &stub,
        StubConfig {
            stdout: output("safe.json"),
            exit: 2,
            stderr: Some(format!(
                "Analyzing...\nError: cannot read {SKILL_PLACEHOLDER}/SKILL.md: unsupported \
                 primary content\n"
            )),
            ..StubConfig::default()
        },
    );
    assert_eq!(report.status, ExternalStatus::NotScanned, "{report:?}");
    assert_eq!(report.reason, Some(NotScanned::Failed));
    assert_eq!(
        report.detail.as_deref(),
        Some(
            "SkillSpector exited with code 2: Error: cannot read <skill>/SKILL.md: unsupported \
             primary content"
        )
    );
    let runs = runs(&fx).to_string_lossy().into_owned();
    assert!(!format!("{report:?}").contains(&runs), "{report:?}");
    assert!(external::gate(&report, false).is_ok());

    // Python's error reprs (and JSON-escaped text) double each backslash
    // of a Windows path; that form goes too.
    let report = run_answer(
        &fx,
        &stub,
        StubConfig {
            exit: 2,
            stderr: Some(format!(
                "FileNotFoundError: [Errno 2] No such file or directory: \
                 '{SKILL_ESCAPED_PLACEHOLDER}/SKILL.md'\n"
            )),
            ..StubConfig::default()
        },
    );
    assert_eq!(report.reason, Some(NotScanned::Failed), "{report:?}");
    assert_eq!(
        report.detail.as_deref(),
        Some(
            "SkillSpector exited with code 2: FileNotFoundError: [Errno 2] No such file or \
             directory: '<skill>/SKILL.md'"
        )
    );
    let doubled = runs.replace('\\', "\\\\");
    assert!(!format!("{report:?}").contains(&doubled), "{report:?}");

    // Any other exit code too, even with a dangerous report on stdout.
    let report = run_answer(&fx, &stub, answer(output("do-not-install.json"), 3));
    assert_eq!(report.reason, Some(NotScanned::Failed), "{report:?}");
    assert_eq!(
        report.detail.as_deref(),
        Some("SkillSpector exited with code 3: skillspector stub: scan finished")
    );
    assert!(external::gate(&report, false).is_ok());
}

/// Review: names that collide on this file system (case, Unicode form)
/// never leave a partial copy that is then reported as scanned.
#[test]
fn colliding_names_are_never_scanned_as_a_partial_copy() {
    let fx = Fx::new();
    let stub = stub(&fx, &["skillspector"]);
    let probe = fx.th.root().join("case-probe");
    std::fs::write(&probe, "").unwrap();
    let case_insensitive = fx.th.root().join("CASE-PROBE").exists();
    let mut files = files();
    files.insert("x.py".to_owned(), b"print('lower')\n".to_vec());
    files.insert("X.py".to_owned(), b"print('upper')\n".to_vec());
    let env = noisy_env(&fx, &stub);
    let runner = runner(&env, &fx);
    stub.answer(&output("safe.json"), 0);
    let report = runner.scan_files(&env, &fx.home, &files, None);
    assert_eq!(
        report.content_hash.as_deref(),
        Some(content_hash(&files).as_str())
    );
    assert!(leftover_runs(&fx).is_empty(), "{:?}", leftover_runs(&fx));
    if case_insensitive {
        assert_eq!(report.status, ExternalStatus::NotScanned, "{report:?}");
        assert_eq!(report.reason, Some(NotScanned::Failed));
        let detail = report.detail.as_deref().unwrap();
        assert!(detail.contains("collides"), "{detail}");
        assert!(stub.scans().is_empty(), "nothing ran");
    } else {
        assert_eq!(report.status, ExternalStatus::Scanned, "{report:?}");
        let scans = stub.scans();
        assert_eq!(
            scans[0].files,
            ["SKILL.md", "X.py", "scripts/run.sh", "x.py"]
        );
    }
}

/// Review: a scan never outlives the stale-run sweep, whatever
/// `timeout_secs` says.
#[test]
fn the_scan_timeout_is_capped() {
    let timeout = |secs: Option<u64>| {
        skillspector::Settings {
            enabled: true,
            timeout_secs: secs,
        }
        .timeout()
    };
    assert_eq!(timeout(None), skillspector::DEFAULT_TIMEOUT);
    assert_eq!(timeout(Some(0)), skillspector::DEFAULT_TIMEOUT);
    assert_eq!(timeout(Some(300)), Duration::from_secs(300));
    assert_eq!(timeout(Some(3540)), skillspector::MAX_TIMEOUT);
    assert_eq!(timeout(Some(3541)), skillspector::MAX_TIMEOUT);
    assert_eq!(timeout(Some(u64::MAX)), skillspector::MAX_TIMEOUT);
    assert_eq!(skillspector::MAX_TIMEOUT, Duration::from_secs(59 * 60));
}

#[test]
fn uv_without_its_cache_is_not_prepared() {
    let fx = Fx::new();
    let stub = stub(&fx, &["uvx"]);
    stub.set_prepared(true);
    let env = noisy_env(&fx, &stub);
    let runner = runner(&env, &fx);
    // uv's cache was cleaned after detection.
    stub.set_prepared(false);
    stub.answer(&output("do-not-install.json"), 1);
    let report = runner.scan_files(&env, &fx.home, &files(), None);
    assert_eq!(report.status, ExternalStatus::NotScanned, "{report:?}");
    assert_eq!(report.reason, Some(NotScanned::NotPrepared));
    assert!(!stub.prepared(), "a scan never downloads");
    assert!(external::gate(&report, false).is_ok());
}

#[test]
fn output_past_the_cap_is_not_understood() {
    let fx = Fx::new();
    let stub = stub(&fx, &["skillspector"]);
    let report = run_answer(
        &fx,
        &stub,
        StubConfig {
            stdout: output("do-not-install.json"),
            exit: 1,
            pad: 17 * 1024 * 1024,
            ..StubConfig::default()
        },
    );
    assert_eq!(report.status, ExternalStatus::NotUnderstood, "{report:?}");
    assert!(external::gate(&report, false).is_ok());
}

#[test]
fn a_run_needs_a_ready_skillspector() {
    let fx = Fx::new();
    let stub = stub(&fx, &[]);
    let report = skillspector::prepare_run(&path_env(&fx, &stub), &fx.fs, &fx.home).unwrap_err();
    assert_eq!(report.status, ExternalStatus::NotScanned);
    assert_eq!(report.reason, Some(NotScanned::NotAvailable));
    assert_eq!(report.scanner, SCANNER);

    stub.add("uvx");
    let report = skillspector::prepare_run(&path_env(&fx, &stub), &fx.fs, &fx.home).unwrap_err();
    assert_eq!(report.reason, Some(NotScanned::NotPrepared));
    assert!(external::gate(&report, false).is_ok());

    stub.set_prepared(true);
    let runner = skillspector::prepare_run(&path_env(&fx, &stub), &fx.fs, &fx.home).unwrap();
    assert_eq!(runner.runtime().kind, RuntimeKind::Uvx);
}

#[test]
fn required_only_when_on_and_ready() {
    let fx = Fx::new();
    let stub = stub(&fx, &[]);
    let required = || skillspector::required(&path_env(&fx, &stub), &fx.fs, &fx.home).unwrap();
    let settings = || skillspector::settings(&fx.fs, &fx.home).unwrap();
    assert!(!required(), "off, nothing on PATH");
    skillspector::set_enabled(&fx.fs, &fx.home, true).unwrap();
    assert!(settings().enabled);
    assert!(!required(), "on, not available");
    stub.add("uvx");
    assert!(!required(), "on, needs prepare");
    stub.set_prepared(true);
    assert!(required(), "on and ready");
    skillspector::set_enabled(&fx.fs, &fx.home, false).unwrap();
    assert!(!required(), "off and ready");

    for (enabled, choice, wanted) in [
        (false, Choice::Default, false),
        (true, Choice::Default, true),
        (false, Choice::On, true),
        (true, Choice::Off, false),
    ] {
        let settings = skillspector::Settings {
            enabled,
            timeout_secs: None,
        };
        assert_eq!(
            skillspector::wanted(&settings, choice),
            wanted,
            "{enabled} {choice:?}"
        );
    }
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
fn a_dangerous_report_blocks_the_install_until_allowed() {
    let fx = Fx::new();
    let env = fx.th.env(host_os());
    let sx = sx(&fx, &env);
    let store = fx.store();
    let stub = stub(&fx, &["skillspector"]);
    let scan_env = noisy_env(&fx, &stub);
    let request = InstallRequest::new(Source::Local(folder(&fx, "tidy")));
    let mut prepared = install::prepare(&sx, &request).unwrap();
    assert_eq!(prepared.scan().verdict, Verdict::Clean, "built-in: clean");

    stub.answer(&output("do-not-install-high.json"), 1);
    let report = runner(&scan_env, &fx).scan_files(&scan_env, &fx.home, prepared.files(), None);
    assert_eq!(report.status, ExternalStatus::Scanned, "{report:?}");
    assert_eq!(report.verdict, Verdict::Dangerous);
    assert_eq!(stub.scans()[0].files, ["SKILL.md"]);
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
    assert!(
        err.message()
            .starts_with("SkillSpector found 1 dangerous finding: DO_NOT_INSTALL"),
        "{err}"
    );
    assert_eq!(fx.commit_count(), commits, "nothing installed");
    let installed =
        install::install_prepared(&store, &sx, &mut prepared, &Naming::default(), &allow())
            .unwrap();
    let external = installed.external.as_ref().unwrap();
    assert_eq!(external.scanner, SCANNER);
    assert_eq!(external.verdict, Verdict::Dangerous);
}

#[test]
fn not_scanned_and_not_understood_never_block_an_install() {
    let fx = Fx::new();
    let env = fx.th.env(host_os());
    let sx = sx(&fx, &env);
    let stub = stub(&fx, &["uvx"]);
    let scan_env = noisy_env(&fx, &stub);
    let not_prepared = skillspector::prepare_run(&scan_env, &fx.fs, &fx.home).unwrap_err();
    stub.set_prepared(true);
    stub.answer(&output("malformed.txt"), 1);
    let not_understood = runner(&scan_env, &fx).scan_files(&scan_env, &fx.home, &files(), None);
    assert_eq!(not_understood.status, ExternalStatus::NotUnderstood);
    for (name, report) in [("alpha", not_prepared), ("beta", not_understood)] {
        let dir = folder(&fx, name);
        fx.write(
            &dir.join("SKILL.md"),
            &SKILL.replace("name: tidy", &format!("name: {name}")),
        );
        let request = InstallRequest::new(Source::Local(dir));
        let mut prepared = install::prepare(&sx, &request).unwrap();
        prepared.set_external(Some(report));
        let installed = install::install_prepared(
            &fx.store(),
            &sx,
            &mut prepared,
            &Naming::default(),
            &Decisions::default(),
        )
        .unwrap();
        assert_eq!(installed.id, name);
    }
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
fn a_dangerous_report_blocks_the_update_of_that_content() {
    let Some(git) = system_git_or_skip("a_dangerous_report_blocks_the_update_of_that_content")
    else {
        return;
    };
    let fx = Fx::new();
    let env = with_git_vars(&fx.th, &git, fx.th.env_builder(host_os())).build();
    let sx = sx(&fx, &env);
    let store = fx.store();
    let stub = stub(&fx, &["uvx"]);
    stub.set_prepared(true);
    let scan_env = noisy_env(&fx, &stub);
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
    stub.answer(&output("do-not-install.json"), 1);
    let report = runner(&scan_env, &fx).scan_files(&scan_env, &fx.home, &files, None);
    assert_eq!(report.verdict, Verdict::Dangerous, "{report:?}");

    let commits = fx.commit_count();
    let blocked = Decisions {
        external: Some(report.clone()),
        ..Decisions::default()
    };
    let err = install::update_reviewed(&store, &sx, "tidy", &latest, &blocked).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ScanBlocked);
    assert!(err.message().starts_with("SkillSpector found"), "{err}");
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

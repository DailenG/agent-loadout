//! Unit tests of SkillSpector: the adapter on the hand-made fixtures
//! (`fixtures/skillspector/output`), version parsing, the argument list,
//! the scrubbed environment and the settings. No process runs here
//! (`tests/skillspector_scan.rs` runs the stub).

use std::path::{Path, PathBuf};

use super::*;
use crate::env::Os;
use crate::error::ErrorCode;
use crate::paths::HomeSource;
use crate::scan::external::{ExternalStatus, gate};
use crate::scan::{Severity, Verdict};

/// Never passed to SkillSpector.
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

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/skillspector/output")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

fn root() -> &'static Path {
    Path::new(if cfg!(windows) {
        r"C:\Users\someone\loadout\cache\scan-external\run-1\skill"
    } else {
        "/home/someone/loadout/cache/scan-external/run-1/skill"
    })
}

fn output(name: &str, exit: i32) -> ExternalReport {
    parse(&fixture(name), b"", Some(exit), root(), VERSION)
}

fn rules(report: &ExternalReport) -> Vec<(&str, Severity)> {
    report
        .findings
        .iter()
        .map(|f| (f.rule.as_str(), f.severity))
        .collect()
}

#[test]
fn a_safe_report_is_scanned_clean() {
    let report = output("safe.json", 0);
    assert_eq!(report.status, ExternalStatus::Scanned, "{report:?}");
    assert_eq!(report.scanner, Scanner::SkillSpector);
    assert_eq!(report.version, "2.13.7");
    assert_eq!(report.verdict, Verdict::Clean);
    assert!(!report.incomplete);
    assert!(report.findings.is_empty());
    assert_eq!(report.summary(), "clean (0 findings)");
    assert!(gate(&report, false).is_ok());
}

#[test]
fn high_and_medium_are_warnings() {
    let report = output("caution.json", 0);
    assert_eq!(report.status, ExternalStatus::Scanned, "{report:?}");
    assert_eq!(report.verdict, Verdict::Warning);
    assert_eq!(
        rules(&report),
        [("P3", Severity::Warning), ("E2", Severity::Warning)]
    );
    let e2 = &report.findings[1];
    assert_eq!(e2.scanner, "skillspector");
    assert_eq!(e2.file.as_deref(), Some("scripts/sync.py"));
    assert_eq!(e2.line, Some(23));
    assert_eq!(
        e2.message,
        "The script reads every environment variable, which can include API keys and tokens."
    );
    assert_eq!(
        e2.excerpt.as_deref(),
        Some("for key, val in os.environ.items():")
    );
    assert!(gate(&report, false).is_ok(), "warnings never block");
}

#[test]
fn critical_is_dangerous_and_gates() {
    let report = output("do-not-install.json", 1);
    assert_eq!(report.status, ExternalStatus::Scanned, "{report:?}");
    assert_eq!(report.verdict, Verdict::Dangerous);
    assert_eq!(
        rules(&report),
        [("E2", Severity::Warning), ("E1", Severity::Dangerous)]
    );
    // A CRITICAL issue already makes it dangerous: no extra finding.
    assert!(report.findings.iter().all(|f| f.rule != "DO_NOT_INSTALL"));
    // Excerpts are masked like the built-in scan's.
    let e1 = &report.findings[1];
    let shown = e1.excerpt.as_deref().unwrap();
    assert!(
        !shown.contains("sk-live-abcdefghijklmnopqrstuvwx"),
        "{shown}"
    );
    assert!(shown.contains("****"), "{shown}");
    let err = gate(&report, false).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ScanBlocked);
    assert!(
        err.message()
            .starts_with("SkillSpector found 1 dangerous finding: E1 (scripts/sync.py:41)"),
        "{}",
        err.message()
    );
    assert!(gate(&report, true).is_ok());
}

#[test]
fn do_not_install_without_a_critical_issue_is_dangerous() {
    let report = output("do-not-install-high.json", 1);
    assert_eq!(report.status, ExternalStatus::Scanned, "{report:?}");
    assert_eq!(report.verdict, Verdict::Dangerous);
    let forced: Vec<_> = report.dangerous().collect();
    assert_eq!(forced.len(), 1, "{report:?}");
    assert_eq!(forced[0].rule, "DO_NOT_INSTALL");
    assert_eq!(
        forced[0].message,
        "SkillSpector recommends not installing this skill (risk score 78 of 100)"
    );
    assert_eq!(forced[0].file, None);
    assert!(gate(&report, false).is_err());
    // The repeated rule keeps one finding per location.
    assert_eq!(report.findings.iter().filter(|f| f.rule == "E2").count(), 2);
}

#[test]
fn incomplete_coverage_is_never_clean() {
    let report = output("incomplete.json", 1);
    assert_eq!(report.status, ExternalStatus::Scanned, "{report:?}");
    assert!(report.incomplete);
    // Only info findings: the verdict reads clean, the summary doesn't.
    assert_eq!(report.verdict, Verdict::Clean);
    assert_eq!(report.summary(), "not fully scanned (2 findings)");
    let note = report
        .findings
        .iter()
        .find(|f| f.rule == "incomplete")
        .unwrap();
    assert_eq!(note.severity, Severity::Info);
    assert_eq!(note.message, "not fully scanned (partial, 87.5% coverage)");
    assert!(gate(&report, false).is_ok());
}

#[test]
fn osv_unreachable_is_shown_as_is() {
    let report = output("osv-unreachable.json", 0);
    assert_eq!(report.status, ExternalStatus::Scanned, "{report:?}");
    assert_eq!(rules(&report), [("SC4", Severity::Info)]);
    let sc4 = &report.findings[0];
    assert!(sc4.message.starts_with("OSV.dev unreachable"), "{sc4:?}");
    assert_eq!(sc4.file.as_deref(), Some("requirements.txt"));
}

#[test]
fn unsuccessful_llm_and_malformed_reports_are_not_understood() {
    for (name, why) in [
        ("execution-failed.json", "execution_successful is not true"),
        ("llm-requested.json", "metadata.llm_requested is not false"),
        ("malformed.txt", "the output is not JSON"),
    ] {
        let report = output(name, 0);
        assert_eq!(report.status, ExternalStatus::NotUnderstood, "{name}");
        assert!(report.detail.as_deref().unwrap().ends_with(why), "{name}");
        assert_eq!(
            report.summary(),
            "scanner output not understood (version 2.13.7)"
        );
        assert!(gate(&report, false).is_ok(), "{name}");
    }
    let safe = String::from_utf8(fixture("safe.json")).unwrap();
    for (from, to) in [
        ("\"SAFE\"", "\"FINE\""),
        ("\"issues\": []", "\"issues\": {}"),
        ("\"is_complete\": true", "\"is_complete\": \"yes\""),
    ] {
        let text = safe.replace(from, to);
        assert_ne!(text, safe, "{from}");
        let report = parse(text.as_bytes(), b"", Some(0), root(), VERSION);
        assert_eq!(report.status, ExternalStatus::NotUnderstood, "{to}");
    }
    let caution = String::from_utf8(fixture("caution.json")).unwrap();
    for (from, to) in [
        ("\"severity\": \"HIGH\"", "\"severity\": \"INFO\""),
        ("\"id\": \"E2\"", "\"id\": \"\""),
    ] {
        let text = caution.replacen(from, to, 1);
        assert_ne!(text, caution, "{from}");
        let report = parse(text.as_bytes(), b"", Some(0), root(), VERSION);
        assert_eq!(report.status, ExternalStatus::NotUnderstood, "{to}");
    }
    for text in ["", "  \n", "[]", "{}", "null"] {
        let report = parse(text.as_bytes(), b"", Some(1), root(), VERSION);
        assert_eq!(report.status, ExternalStatus::NotUnderstood, "{text:?}");
    }
}

#[test]
fn exit_2_and_kills_are_not_scanned() {
    let stderr = format!(
        "Analyzing...\nError: cannot read {}/SKILL.md: unsupported primary content\n",
        root().display()
    );
    let report = parse(
        &fixture("safe.json"),
        stderr.as_bytes(),
        Some(2),
        root(),
        VERSION,
    );
    assert_eq!(report.status, ExternalStatus::NotScanned);
    assert_eq!(report.reason, Some(NotScanned::Failed));
    let detail = report.detail.unwrap();
    assert_eq!(
        detail,
        "SkillSpector exited with code 2: Error: cannot read <skill>/SKILL.md: unsupported primary content"
    );
    let report = parse(b"", b"", None, root(), VERSION);
    assert_eq!(report.reason, Some(NotScanned::Failed));
    assert_eq!(
        report.detail.as_deref(),
        Some("SkillSpector was stopped by the system")
    );
    assert!(gate(&report, false).is_ok());
    // uv without its cache (`--offline`): not prepared.
    let uv = b"  x No solution found when resolving tool dependencies:\n  hint: Packages were \
               unavailable because the network was disabled.\n";
    let report = parse(b"", uv, Some(2), root(), VERSION);
    assert_eq!(report.reason, Some(NotScanned::NotPrepared));
}

#[test]
fn paths_outside_the_skill_are_dropped() {
    let root_text = root().to_string_lossy().replace('\\', "\\\\");
    let caution = String::from_utf8(fixture("caution.json")).unwrap();
    let text = caution
        .replacen(
            "\"file\": \"scripts/sync.py\"",
            &format!("\"file\": \"{root_text}/scripts/sync.py\""),
            1,
        )
        .replacen(
            "\"file\": \"SKILL.md\"",
            "\"file\": \"../../etc/passwd\"",
            1,
        );
    let report = parse(text.as_bytes(), b"", Some(0), root(), VERSION);
    let files: Vec<Option<&str>> = report.findings.iter().map(|f| f.file.as_deref()).collect();
    assert!(files.contains(&Some("scripts/sync.py")), "{files:?}");
    assert!(files.contains(&None), "{files:?}");
}

#[test]
fn versions_are_read_defensively() {
    for (text, version) in [
        ("skillspector 2.13.7\n", Some("2.13.7")),
        ("SkillSpector version 2.14.0rc1", Some("2.14.0rc1")),
        ("v2.13.7", Some("2.13.7")),
        ("2.13.7.", Some("2.13.7")),
        ("skillspector, version 3.0.0 (python 3.13)", Some("3.0.0")),
        ("skillspector", None),
        ("", None),
        ("built 2026", None),
    ] {
        assert_eq!(version_of(text).as_deref(), version, "{text:?}");
    }
    let runtime = |version: &str| Runtime {
        kind: RuntimeKind::Path,
        program: PathBuf::from("/usr/bin/skillspector"),
        version: version.to_owned(),
    };
    assert_eq!(runtime(VERSION).note(), None);
    assert_eq!(
        runtime("2.14.0").note().as_deref(),
        Some("SkillSpector 2.14.0 is not tested with this Loadout (tested: 2.13.7)")
    );
}

#[test]
fn a_scan_passes_exactly_the_documented_arguments() {
    let skill = root();
    let uvx = Runtime {
        kind: RuntimeKind::Uvx,
        program: PathBuf::from("/usr/local/bin/uvx"),
        version: VERSION.to_owned(),
    };
    let argv: Vec<String> = uvx
        .argv(skill)
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let mut expected: Vec<String> = [
        "--offline",
        "--python",
        "3.13",
        "--from",
        "skillspector==2.13.7",
        "skillspector",
        "scan",
        "--no-llm",
        "--format",
        "json",
        "--fail-on-incomplete",
        "--",
    ]
    .iter()
    .map(|arg| (*arg).to_owned())
    .collect();
    expected.push(skill.to_string_lossy().into_owned());
    assert_eq!(argv, expected);
    let uv = Runtime {
        program: PathBuf::from("/usr/local/bin/uv"),
        ..uvx.clone()
    };
    let argv: Vec<String> = uv
        .argv(skill)
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(argv[..3], ["tool", "run", "--offline"]);
    let path = Runtime {
        kind: RuntimeKind::Path,
        program: PathBuf::from("/usr/bin/skillspector"),
        version: VERSION.to_owned(),
    };
    let argv: Vec<String> = path
        .argv(skill)
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(argv, expected[6..]);
    for runtime in [&uvx, &uv, &path] {
        let argv = runtime.argv(skill);
        for flag in FORBIDDEN {
            assert!(
                !argv.iter().any(|arg| arg == *flag),
                "{flag} passed: {argv:?}"
            );
        }
        assert!(!argv.iter().any(|arg| arg.to_string_lossy().contains("://")));
    }
    assert_eq!(
        uvx.command_line(),
        "/usr/local/bin/uvx --python 3.13 --from skillspector==2.13.7 skillspector"
    );
}

#[test]
fn the_child_environment_has_no_provider_keys_and_no_tracing() {
    let env = crate::env::Env::builder(Os::Linux, "/home/someone")
        .var("PATH", "/usr/bin")
        .var("HOME", "/home/someone")
        .var("TMPDIR", "/tmp/x")
        .var("HTTPS_PROXY", "http://proxy:3128")
        .var("UV_CACHE_DIR", "/data/uv")
        .var("OPENAI_API_KEY", "sk-openai")
        .var("ANTHROPIC_API_KEY", "sk-ant")
        .var("AWS_SECRET_ACCESS_KEY", "aws")
        .var("NVIDIA_API_KEY", "nv")
        .var("LANGSMITH_API_KEY", "ls")
        .var("LANGSMITH_TRACING", "true")
        .var("LANGCHAIN_TRACING_V2", "true")
        .var("SKILLSPECTOR_OSV_TIMEOUT", "0")
        .var("GITHUB_TOKEN", "ghp_x")
        .build();
    let tmp = Path::new("/home/someone/loadout/cache/scan-external/run-1/tmp");
    let vars = child_env(&env, tmp);
    let names: Vec<&str> = vars.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "PATH",
            "HOME",
            "HTTPS_PROXY",
            "UV_CACHE_DIR",
            "TEMP",
            "TMP",
            "TMPDIR",
            "PYTHONIOENCODING",
            "PYTHONUTF8",
            "NO_COLOR",
            "LANGSMITH_TRACING",
            "LANGCHAIN_TRACING_V2",
        ]
    );
    let value = |name: &str| {
        vars.iter()
            .find(|(known, _)| known == name)
            .map(|(_, value)| value.to_string_lossy().into_owned())
    };
    assert_eq!(value("LANGSMITH_TRACING").as_deref(), Some("false"));
    assert_eq!(value("LANGCHAIN_TRACING_V2").as_deref(), Some("false"));
    assert_eq!(value("TMPDIR"), Some(tmp.to_string_lossy().into_owned()));
}

/// A temp data directory, removed on drop.
struct TempHome {
    root: PathBuf,
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn temp_home() -> (TempHome, LoadoutHome) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "loadout-skillspector-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let home = LoadoutHome {
        path: root.join("loadout"),
        source: HomeSource::Flag,
        sync_root: None,
    };
    (TempHome { root }, home)
}

#[test]
fn settings_are_machine_local_and_off_by_default() {
    let (_dir, home) = temp_home();
    let fs = crate::fs::RealFs;
    assert_eq!(settings(&fs, &home).unwrap(), Settings::default());
    assert_eq!(settings(&fs, &home).unwrap().timeout(), DEFAULT_TIMEOUT);
    assert_eq!(DEFAULT_TIMEOUT.as_secs(), 120);
    set_enabled(&fs, &home, true).unwrap();
    let saved = settings(&fs, &home).unwrap();
    assert!(saved.enabled);
    let text = std::fs::read_to_string(home.state_dir().join("local-settings.toml")).unwrap();
    assert!(text.contains("[skillspector]"), "{text}");
    assert!(wanted(&saved, Choice::Default));
    assert!(!wanted(&saved, Choice::Off));
    set_enabled(&fs, &home, false).unwrap();
    let saved = settings(&fs, &home).unwrap();
    assert!(!wanted(&saved, Choice::Default));
    assert!(wanted(&saved, Choice::On));
    // Off: nothing is required, and detection is not even started.
    let env = crate::env::Env::builder(Os::Linux, "/home/someone").build();
    assert!(!required(&env, &fs, &home).unwrap());
}

#[test]
fn nothing_on_path_is_not_available() {
    let (_dir, home) = temp_home();
    let fs = crate::fs::RealFs;
    let empty = home.path.join("empty-bin");
    std::fs::create_dir_all(&empty).unwrap();
    let os = if cfg!(windows) {
        Os::Windows
    } else {
        Os::Linux
    };
    let env = crate::env::Env::builder(os, "/home/someone")
        .var("PATH", empty.as_os_str())
        .build();
    assert_eq!(detect(&env, &home), Availability::NotAvailable);
    set_enabled(&fs, &home, true).unwrap();
    assert!(!required(&env, &fs, &home).unwrap());
    let report = prepare_run(&env, &fs, &home).unwrap_err();
    assert_eq!(report.reason, Some(NotScanned::NotAvailable));
    let err = prepare(&env, &home, &mut |_| {}, None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ScannerUnavailable);
    assert!(err.message().contains("pipx install skillspector==2.13.7"));
}

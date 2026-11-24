//! Unit tests of the external scanner: the compiled pins against the
//! signed checksums, the JSON adapter on the recorded outputs, the gate,
//! token references, settings, the download allowlist and verification.
//! No network, no scanner run (`tests/external_scan.rs` runs the stub).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use sha2::{Digest, Sha256};

use super::*;
use crate::paths::HomeSource;
use crate::scan::Severity;

fn fixture(relative: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/agent-scan")
        .join(relative);
    std::fs::read(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

fn output(name: &str) -> ExternalReport {
    parse(
        &fixture(&format!("output/{name}")),
        b"",
        Some(0),
        root(),
        VERSION,
    )
}

fn root() -> &'static Path {
    Path::new(if cfg!(windows) {
        r"C:\Users\someone\loadout\cache\scan-external\run-1\skill"
    } else {
        "/home/someone/loadout/cache/scan-external/run-1/skill"
    })
}

/// The hash lines of the signed body of a cleartext-signed message.
fn signed_hashes(asc: &str) -> Vec<(String, String)> {
    let body = asc
        .split("-----BEGIN PGP SIGNATURE-----")
        .next()
        .expect("signature block");
    let body = body
        .split_once("\n\n")
        .or_else(|| body.split_once("\r\n\r\n"))
        .expect("armor headers end")
        .1;
    body.lines()
        .filter_map(|line| {
            let (hash, name) = line.trim().split_once("  ")?;
            Some((name.trim().to_owned(), hash.to_owned()))
        })
        .collect()
}

#[test]
fn compiled_hashes_are_the_signed_ones() {
    let asc = String::from_utf8(fixture("v0.7.1/sha256sums.txt.asc")).unwrap();
    assert!(asc.starts_with("-----BEGIN PGP SIGNED MESSAGE-----"));
    assert!(asc.contains("Hash: SHA256"));
    let signed = signed_hashes(&asc);
    assert_eq!(signed.len(), 6, "{signed:?}");
    for pinned in PINNED {
        let (_, hash) = signed
            .iter()
            .find(|(name, _)| name == pinned.name)
            .unwrap_or_else(|| panic!("{} is not in the signed checksums", pinned.name));
        assert_eq!(hash, pinned.sha256, "{}", pinned.name);
        assert!(pinned.name.contains(VERSION), "{}", pinned.name);
        assert!(pinned.size > 1_000_000 && pinned.size < MAX_DOWNLOAD);
    }
    // Every platform asset of the release is pinned.
    let assets = signed
        .iter()
        .filter(|(name, _)| name.starts_with("agent-scan-"))
        .count();
    assert_eq!(assets, PINNED.len());
}

#[test]
fn the_signing_key_fixture_is_the_pinned_fingerprint() {
    let key = String::from_utf8(fixture("v0.7.1/snyk-code-signing-public.pgp")).unwrap();
    assert!(key.starts_with("-----BEGIN PGP PUBLIC KEY BLOCK-----"));
    assert_eq!(SIGNING_FINGERPRINT.len(), 40);
    // The issuer fingerprint subpacket of the signature names the key
    // (base64 of 0x04 || fingerprint after "FiEE" in the armored packet).
    let asc = String::from_utf8(fixture("v0.7.1/sha256sums.txt.asc")).unwrap();
    assert!(asc.contains("WIQTvT/nRjYLVYpuCSR0qIPhuTvEx"));
}

#[test]
fn every_platform_has_one_asset() {
    for (os, arch, name) in [
        ("windows", "x86_64", "agent-scan-0.7.1-windows-x86_64.exe"),
        ("windows", "aarch64", "agent-scan-0.7.1-windows-x86_64.exe"),
        ("linux", "x86_64", "agent-scan-0.7.1-linux-x86_64"),
        ("linux", "aarch64", "agent-scan-0.7.1-linux-arm64"),
        ("macos", "aarch64", "agent-scan-0.7.1-macos-arm64"),
        ("macos", "x86_64", "agent-scan-0.7.1-macos-x86_64"),
    ] {
        assert_eq!(asset_for(os, arch).map(|a| a.name), Some(name.to_owned()));
    }
    assert_eq!(asset_for("freebsd", "x86_64"), None);
    assert_eq!(asset_for("linux", "riscv64"), None);
    assert_eq!(
        download_url(VERSION, "agent-scan-0.7.1-linux-arm64"),
        "https://github.com/snyk/agent-scan/releases/download/v0.7.1/agent-scan-0.7.1-linux-arm64"
    );
}

#[test]
fn a_clean_skill_is_scanned_clean() {
    let report = output("clean.json");
    assert_eq!(report.status, ExternalStatus::Scanned);
    assert_eq!(report.reason, None);
    assert_eq!(report.verdict, Verdict::Clean);
    assert!(report.findings.is_empty());
    assert_eq!(report.summary(), "clean (0 findings)");
}

#[test]
fn scores_map_to_severities() {
    let report = output("warning.json");
    assert_eq!(report.status, ExternalStatus::Scanned);
    assert_eq!(report.verdict, Verdict::Warning);
    let finding = |rule: &str| {
        report
            .findings
            .iter()
            .find(|f| f.rule == rule)
            .unwrap_or_else(|| panic!("{rule}"))
    };
    let credential = finding("insecure_credential_handling");
    assert_eq!(credential.severity, Severity::Warning);
    assert_eq!(credential.scanner, SCANNER);
    assert_eq!(credential.file.as_deref(), Some("scripts/deploy.sh"));
    assert_eq!(credential.line, Some(7));
    assert_eq!(
        credential.message,
        "The script prints the deploy token to the terminal."
    );
    let exposure = finding("third_party_content_exposure");
    assert_eq!(exposure.severity, Severity::Info);
    assert_eq!(exposure.file, None);
}

#[test]
fn high_scores_and_malicious_code_are_dangerous() {
    let report = output("dangerous.json");
    assert_eq!(report.status, ExternalStatus::Scanned);
    assert_eq!(report.verdict, Verdict::Dangerous);
    // One finding per location; malicious_code is dangerous at any score.
    let labels: Vec<String> = report.findings.iter().map(Finding::label).collect();
    assert_eq!(
        labels,
        [
            "suspicious_download_url (SKILL.md:3)",
            "suspicious_download_url (scripts/install.sh:12)",
            "malicious_code (scripts/install.sh:20)",
        ]
    );
    assert!(report.dangerous().count() == 3);
    let url = report
        .findings
        .iter()
        .find(|f| f.rule == "suspicious_download_url")
        .unwrap();
    assert_eq!(
        url.excerpt.as_deref(),
        Some("https://downloads.example.invalid/install.sh")
    );
    let err = gate(&report, false).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ScanBlocked);
    assert!(
        err.message()
            .starts_with("Snyk Agent Scan found 3 dangerous findings")
    );
    assert!(gate(&report, true).is_ok());
}

#[test]
fn findings_are_sorted_by_file_line_and_rule() {
    let report = output("dangerous.json");
    let keys: Vec<(Option<String>, Option<u32>)> = report
        .findings
        .iter()
        .map(|f| (f.file.clone(), f.line))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
}

#[test]
fn snyk_errors_are_not_scanned_and_never_block() {
    for (name, reason) in [
        ("limit.json", NotScanned::LimitReached),
        ("too-large.json", NotScanned::TooLarge),
        ("unauthorized.json", NotScanned::TokenRejected),
    ] {
        let report = output(name);
        assert_eq!(report.status, ExternalStatus::NotScanned, "{name}");
        assert_eq!(report.reason, Some(reason), "{name}");
        assert!(gate(&report, false).is_ok(), "{name}");
        let detail = report.detail.clone().unwrap_or_default();
        assert!(!detail.contains("Traceback"), "{name}: {detail}");
        assert!(!detail.contains("/home/"), "{name}: {detail}");
    }
    assert_eq!(
        output("limit.json").summary(),
        "not scanned (Snyk limit reached)"
    );
}

#[test]
fn output_that_is_not_the_schema_is_never_clean() {
    for name in ["malformed.txt", "v05.json"] {
        let report = output(name);
        assert_eq!(report.status, ExternalStatus::NotUnderstood, "{name}");
        assert_eq!(
            report.summary(),
            "scanner output not understood (version 0.7.1)"
        );
    }
    for text in [
        "{}",
        "[]",
        r#"{"scan_path_responses":[]}"#,
        r#"{"scan_path_responses":[{"skill_risks":[{"name":"x","risk_indexes":{"malicious_code":{"evidence":"no score"}}}]}]}"#,
        r#"{"scan_path_responses":[{"skill_risks":"nope"}]}"#,
    ] {
        let report = parse(text.as_bytes(), b"", Some(0), root(), VERSION);
        assert_eq!(report.status, ExternalStatus::NotUnderstood, "{text}");
    }
}

#[test]
fn empty_output_is_failed_unless_the_scan_succeeded() {
    let report = parse(b"", b"agent-scan: crashed\n", Some(1), root(), VERSION);
    assert_eq!(report.reason, Some(NotScanned::Failed));
    assert!(report.detail.unwrap().ends_with("agent-scan: crashed"));
    let report = parse(b"  \n", b"", Some(0), root(), VERSION);
    assert_eq!(report.status, ExternalStatus::NotUnderstood);
}

#[test]
fn declined_servers_would_start_the_server() {
    let text = r#"{"scan_path_responses":[{"path":"x","skill_risks":[{"name":"s","risk_indexes":{}}],
        "server_risks":[{"name":"srv","entities":[],"risk_indexes":{},
        "error":{"message":"user declined","is_failure":true,"category":"user_declined"}}]}]}"#;
    let report = parse(text.as_bytes(), b"", Some(0), root(), VERSION);
    assert_eq!(report.reason, Some(NotScanned::WouldStartServer));
}

#[test]
fn paths_are_made_relative_and_absolute_ones_dropped() {
    let root_text = root().to_string_lossy().replace('\\', "\\\\");
    let text = format!(
        r#"{{"scan_path_responses":[{{"path":"x","skill_risks":[{{"name":"s","risk_indexes":{{
        "a":{{"score":100,"evidence":"in {root_text} somewhere","locations":[
            {{"start":{{"path":"{root_text}/scripts/a.sh","line":1}}}},
            {{"start":{{"path":"/etc/passwd","line":2}}}},
            {{"start":{{"path":"../outside.md","line":3}}}},
            {{"start":{{"path":"C:/Windows/x","line":4}}}}]}}}}}}]}}]}}"#
    );
    let report = parse(text.as_bytes(), b"", Some(0), root(), VERSION);
    assert_eq!(report.status, ExternalStatus::Scanned, "{report:?}");
    let files: Vec<Option<&str>> = report.findings.iter().map(|f| f.file.as_deref()).collect();
    assert!(files.contains(&Some("scripts/a.sh")), "{files:?}");
    assert_eq!(files.iter().filter(|f| f.is_none()).count(), 3, "{files:?}");
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.message.contains("<skill>")),
        "{report:?}"
    );
}

#[test]
fn messages_are_one_line_with_invisible_characters_spelled_out() {
    let text = "{\"scan_path_responses\":[{\"path\":\"x\",\"skill_risks\":[{\"name\":\"s\",\
        \"risk_indexes\":{\"a\":{\"score\":100,\"evidence\":\"line\\none\\u202Eevil\"}}}]}]}";
    let report = parse(text.as_bytes(), b"", Some(0), root(), VERSION);
    assert_eq!(report.findings[0].message, "line one\\u{202E}evil");
}

#[test]
fn token_references_are_aliases_or_provider_references() {
    assert_eq!(
        TokenRef::parse("SNYK_TOKEN").unwrap(),
        TokenRef::Alias("SNYK_TOKEN".to_owned())
    );
    for (text, kind) in [
        ("SNYK_TOKEN", "alias"),
        ("env:SNYK_TOKEN", "env"),
        ("keyring://loadout/snyk", "keyring"),
        ("op://Private/Snyk/token", "one_password"),
    ] {
        assert_eq!(TokenRef::parse(text).unwrap().kind(), kind, "{text}");
    }
    for text in [
        "op-env://abcdefghijklmnopqrstuvwxyz/SNYK",
        "snyk token",
        "",
        "env:",
    ] {
        assert_eq!(
            TokenRef::parse(text).unwrap_err().code(),
            ErrorCode::InvalidRef,
            "{text}"
        );
    }
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
        "loadout-external-{}-{}",
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
    set_enabled(&fs, &home, true).unwrap();
    set_token(&fs, &home, None, Some("keyring://loadout/snyk")).unwrap();
    let saved = settings(&fs, &home).unwrap();
    assert!(saved.enabled);
    assert_eq!(saved.token.as_deref(), Some("keyring://loadout/snyk"));
    let text = std::fs::read_to_string(home.state_dir().join("local-settings.toml")).unwrap();
    assert!(text.contains("[snyk_agent_scan]"), "{text}");
    assert_eq!(
        set_token(&fs, &home, None, Some("not a reference"))
            .unwrap_err()
            .code(),
        ErrorCode::InvalidRef
    );
    set_token(&fs, &home, None, None).unwrap();
    set_enabled(&fs, &home, false).unwrap();
    let saved = settings(&fs, &home).unwrap();
    assert_eq!(saved.token, None);
    assert!(!saved.enabled);
    assert!(!wanted_by(&saved, Choice::Default));
    assert!(wanted_by(&saved, Choice::On));
    assert!(!wanted_by(
        &Settings {
            enabled: true,
            ..Settings::default()
        },
        Choice::Off
    ));
}

/// With the scanner disabled in this build nothing asks for a run, even
/// when a 0.7.0 setting turned it on.
#[test]
fn disabled_build_never_wants_or_requires_a_run() {
    if AVAILABLE {
        return;
    }
    let (_dir, home) = temp_home();
    let fs = crate::fs::RealFs;
    set_enabled(&fs, &home, true).unwrap();
    set_token(&fs, &home, None, Some("keyring://loadout/snyk")).unwrap();
    let saved = settings(&fs, &home).unwrap();
    assert!(saved.enabled);
    for choice in [Choice::Default, Choice::On, Choice::Off] {
        assert!(!wanted(&saved, choice));
    }
    assert!(!required(&fs, &home).unwrap());
    assert_eq!(unavailable().code(), ErrorCode::ScannerUnavailable);
}

#[test]
fn downloads_come_only_from_github_over_https() {
    for ok in [
        "https://github.com/snyk/agent-scan/releases/download/v0.7.1/a",
        "https://objects.githubusercontent.com/x",
        "https://release-assets.githubusercontent.com/github-production-release-asset/1?sig=x",
    ] {
        assert!(check_download_url(ok).is_ok(), "{ok}");
    }
    for bad in [
        "http://github.com/snyk/agent-scan/releases/download/v0.7.1/a",
        "https://evil.example/agent-scan",
        "https://github.com.evil.example/a",
        "https://user:pw@github.com/a",
        "https://github.com:8443/a",
        "file:///etc/passwd",
    ] {
        assert_eq!(
            check_download_url(bad).unwrap_err().code(),
            ErrorCode::ScannerDownloadFailed,
            "{bad}"
        );
    }
}

/// Serves fixed bytes and records the URL asked for.
struct Bytes {
    body: Vec<u8>,
    asked: Mutex<Vec<String>>,
}

impl Download for Bytes {
    fn fetch(
        &self,
        url: &str,
        out: &mut dyn Write,
        max: u64,
        progress: &mut dyn FnMut(u64, u64),
        cancel: Option<&AtomicBool>,
    ) -> Result<u64, CoreError> {
        self.asked.lock().unwrap().push(url.to_owned());
        copy(
            &mut self.body.as_slice(),
            out,
            max,
            self.body.len() as u64,
            progress,
            cancel,
        )
        .map_err(|err| CoreError::new(ErrorCode::ScannerDownloadFailed, err.to_string()))
    }
}

fn asset_of(bytes: &[u8]) -> Asset {
    Asset {
        name: "agent-scan-test".to_owned(),
        sha256: install::hex(&Sha256::digest(bytes)),
        size: bytes.len() as u64,
    }
}

#[test]
fn a_verified_download_is_installed_and_removed() {
    let (_dir, home) = temp_home();
    let body = b"#!/bin/sh\necho stub\n".to_vec();
    let tool = Tool::new(&home, VERSION, asset_of(&body));
    // An older pin left behind is cleaned up by the install.
    std::fs::create_dir_all(tool.dir().parent().unwrap().join("0.6.0")).unwrap();
    let download = Bytes {
        body: body.clone(),
        asked: Mutex::new(Vec::new()),
    };
    let mut seen = Vec::new();
    tool.install(
        &download,
        &mut |received, total| seen.push((received, total)),
        None,
    )
    .unwrap();
    assert!(tool.installed());
    assert_eq!(std::fs::read(tool.binary()).unwrap(), body);
    assert_eq!(
        download.asked.lock().unwrap().as_slice(),
        [download_url(VERSION, "agent-scan-test")]
    );
    assert_eq!(seen.last(), Some(&(body.len() as u64, body.len() as u64)));
    assert!(!tool.dir().parent().unwrap().join("0.6.0").exists());
    assert!(
        tool.binary().starts_with(
            home.path
                .join("tools")
                .join("snyk-agent-scan")
                .join(VERSION)
        )
    );
    assert!(tool.remove().unwrap());
    assert!(!tool.installed());
    assert!(!tool.remove().unwrap());
}

#[test]
fn a_tampered_download_is_rejected_and_deleted() {
    let (_dir, home) = temp_home();
    let body = b"the real build".to_vec();
    let tool = Tool::new(&home, VERSION, asset_of(&body));
    for tampered in [b"the real buile".to_vec(), b"the real build plus".to_vec()] {
        let download = Bytes {
            body: tampered,
            asked: Mutex::new(Vec::new()),
        };
        let err = tool.install(&download, &mut |_, _| {}, None).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ScannerVerificationFailed);
        assert!(!tool.installed());
        assert!(!tool.binary().exists());
        let left: Vec<_> = std::fs::read_dir(tool.dir()).unwrap().flatten().collect();
        assert!(left.is_empty(), "{left:?}");
    }
    // The compiled pin rejects anything that is not Snyk's build.
    let pinned = Tool::new(&home, VERSION, Asset::from(&PINNED[0]));
    let download = Bytes {
        body: b"not snyk".to_vec(),
        asked: Mutex::new(Vec::new()),
    };
    let err = pinned.install(&download, &mut |_, _| {}, None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ScannerVerificationFailed);
    assert!(!pinned.installed());
}

#[test]
fn a_canceled_install_installs_nothing() {
    let (_dir, home) = temp_home();
    let body = vec![7u8; 1024 * 1024];
    let tool = Tool::new(&home, VERSION, asset_of(&body));
    let download = Bytes {
        body,
        asked: Mutex::new(Vec::new()),
    };
    let cancel = AtomicBool::new(true);
    let err = tool
        .install(&download, &mut |_, _| {}, Some(&cancel))
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::Canceled);
    assert!(!tool.installed());
    let left: Vec<_> = std::fs::read_dir(tool.dir()).unwrap().flatten().collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn downloads_past_the_cap_stop() {
    let body = vec![0u8; 1000];
    let mut out = Vec::new();
    let err = copy(&mut body.as_slice(), &mut out, 999, 0, &mut |_, _| {}, None).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::FileTooLarge);
}

#[test]
fn the_child_environment_is_scrubbed() {
    let env = crate::env::Env::builder(crate::env::Os::Linux, "/home/someone")
        .var("PATH", "/usr/bin")
        .var("TMPDIR", "/tmp/x")
        .var("HTTPS_PROXY", "http://proxy:3128")
        .var("MACHINE_SERIAL", "SERIAL-1")
        .var("SNYK_API", "https://evil.example")
        .var("AGENT_SCAN_ENVIRONMENT", "dev")
        .var("GITHUB_TOKEN", "ghp_x")
        .var("SNYK_TOKEN", "from-the-process")
        .build();
    let tmp = Path::new("/home/someone/loadout/cache/scan-external/run-1/tmp");
    let vars = child_env(&env, &SecretValue::new("the-token"), tmp);
    let names: Vec<&str> = vars.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "PATH",
            "HTTPS_PROXY",
            "TEMP",
            "TMP",
            "TMPDIR",
            "PYTHONIOENCODING",
            "PYTHONUTF8",
            "SNYK_TOKEN"
        ]
    );
    // The user's temp folder never reaches the scanner.
    assert!(
        vars.iter()
            .filter(|(name, _)| TEMP_VARS.contains(&name.as_str()))
            .all(|(_, value)| value == tmp.as_os_str())
    );
    let token: Vec<_> = vars
        .iter()
        .filter(|(name, _)| name == "SNYK_TOKEN")
        .collect();
    assert_eq!(token.len(), 1);
    assert_eq!(token[0].1, "the-token");
}

/// Review F1: a pasted token is never echoed, and an alias that is not a
/// defined secret is refused (it is not stored as a "reference").
#[test]
fn a_pasted_token_is_refused_and_never_echoed() {
    for pasted in [
        "3f2a9c1e-7b4d-4e8a-9c0f-1a2b3c4d5e6f",
        "snyk:3f2a9c1e 7b4d",
        "token with spaces 3f2a9c1e",
    ] {
        let err = TokenRef::parse(pasted).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidRef, "{pasted}");
        assert!(!err.message().contains(pasted), "{}", err.message());
        assert!(!err.message().contains("3f2a9c1e"), "{}", err.message());
    }
    let (_dir, home) = temp_home();
    let fs = crate::fs::RealFs;
    let pasted = "deadbeef0123456789abcdef01234567";
    let err = set_token(&fs, &home, None, Some(pasted)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidRef);
    assert!(!err.message().contains(pasted), "{}", err.message());
    assert_eq!(settings(&fs, &home).unwrap().token, None);
    let text =
        std::fs::read_to_string(home.state_dir().join("local-settings.toml")).unwrap_or_default();
    assert!(!text.contains(pasted), "{text}");
}

/// Review F2: a binary rewritten after install (same size) is no longer
/// installed, and `install` repairs it.
#[test]
fn a_binary_changed_after_install_is_not_installed_and_repaired() {
    let (_dir, home) = temp_home();
    let body = b"#!/bin/sh\necho the real build\n".to_vec();
    let tool = Tool::new(&home, VERSION, asset_of(&body));
    let download = Bytes {
        body: body.clone(),
        asked: Mutex::new(Vec::new()),
    };
    tool.install(&download, &mut |_, _| {}, None).unwrap();
    assert!(tool.verify().is_ok());
    let mut changed = body.clone();
    changed[0] ^= 0x20;
    std::fs::write(tool.binary(), &changed).unwrap();
    let why = tool.verify().unwrap_err();
    assert!(why.contains("SHA-256"), "{why}");
    assert!(!tool.installed(), "the marker is deleted");
    tool.install(&download, &mut |_, _| {}, None).unwrap();
    assert_eq!(download.asked.lock().unwrap().len(), 2, "downloaded again");
    assert_eq!(std::fs::read(tool.binary()).unwrap(), body);
    assert!(tool.verify().is_ok());
}

/// Review F4: a partial download a killed install left is removed by the
/// next install.
#[test]
fn a_partial_download_left_behind_is_removed() {
    let (_dir, home) = temp_home();
    let body = b"the build".to_vec();
    let tool = Tool::new(&home, VERSION, asset_of(&body));
    std::fs::create_dir_all(tool.dir()).unwrap();
    let name = tool
        .binary()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let left = tool.dir().join(format!(".{name}.loadout-0badf00d.tmp"));
    std::fs::write(&left, b"partial").unwrap();
    let download = Bytes {
        body,
        asked: Mutex::new(Vec::new()),
    };
    tool.install(&download, &mut |_, _| {}, None).unwrap();
    assert!(!left.exists(), "the partial download stays");
}

/// Review F8: no output and no exit code (killed) is a failure.
#[test]
fn no_output_after_a_kill_is_failed() {
    let report = parse(b"", b"", None, root(), VERSION);
    assert_eq!(report.reason, Some(NotScanned::Failed));
    let report = parse(b"", b"Error: 401 Unauthorized\n", Some(1), root(), VERSION);
    assert_eq!(report.reason, Some(NotScanned::TokenRejected));
}

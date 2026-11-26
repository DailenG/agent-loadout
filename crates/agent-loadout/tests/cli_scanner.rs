//! `loadout scanner ...` and `skills add|update --external/--no-external`
//! (#30) against a temp `LOADOUT_HOME`, with the `skillspector-stub`
//! copied as `uvx`, `uv` or `skillspector` into the temp `bin` folder, the
//! first `PATH` entry of the command (the only one, unless a test needs
//! git): detection, Prepare, the switch, scans of library skills, installs
//! and updates for each kind of report, the gate, and what reaches the
//! scanner (its log). `scanner remove` still cleans up a 0.7.0 Snyk Agent
//! Scan install (`agent-scan-stub` through `LOADOUT_TEST_AGENT_SCAN`).

use std::path::{Path, PathBuf};
use std::process::Output;

use agent_loadout_core::brand;
use agent_loadout_core::paths::{HomeSource, LoadoutHome};
use agent_loadout_core::scan::external::snyk;
use agent_loadout_testkit::TempHome;
use agent_loadout_testkit::agent_scan_stub::{self, AgentScanStub};
use agent_loadout_testkit::git_env::{git_vars, init_bare, system_git_or_skip};
use agent_loadout_testkit::skillspector_stub::{SkillSpectorStub, StubConfig, output};
use serde_json::Value;

const SKILL: &str = "---\nname: tidy\ndescription: Tidies.\n---\n\n# Tidy\n\nSort the imports.\n";

/// Variables a user may have set that must never reach SkillSpector.
const PROVIDER_KEYS: &[(&str, &str)] = &[
    ("OPENAI_API_KEY", "sk-openai-cli-test-1"),
    ("ANTHROPIC_API_KEY", "sk-ant-cli-test-2"),
    ("NVIDIA_API_KEY", "nvapi-cli-test-3"),
    ("AWS_SECRET_ACCESS_KEY", "aws-cli-test-4"),
    ("GOOGLE_API_KEY", "google-cli-test-5"),
    ("LANGSMITH_API_KEY", "ls-cli-test-6"),
    ("LANGSMITH_TRACING", "true"),
    ("LANGCHAIN_API_KEY", "lc-cli-test-7"),
    ("SKILLSPECTOR_LLM_PROVIDER", "openai"),
];

struct Cli {
    th: TempHome,
    home: PathBuf,
    stub: SkillSpectorStub,
    /// The 0.7.0 Snyk Agent Scan install, when a test makes one.
    snyk: Option<AgentScanStub>,
    /// The system git, for the tests that need one (then also on `PATH`,
    /// after the stub's `bin`).
    git: Option<PathBuf>,
}

impl Cli {
    /// Nothing on `PATH` yet.
    fn new() -> Self {
        let th = TempHome::new();
        let home = th.root().join("lh");
        let stub = SkillSpectorStub::new(th.root());
        assert_eq!(stub.bin(), th.bin());
        Self {
            th,
            home,
            stub,
            snyk: None,
            git: None,
        }
    }

    /// uvx on `PATH`, SkillSpector already downloaded: ready (uvx).
    fn ready() -> Self {
        let cli = Self::new();
        cli.stub.add("uvx");
        cli.stub.set_prepared(true);
        cli
    }

    fn loadout_home(&self) -> LoadoutHome {
        LoadoutHome {
            path: self.home.clone(),
            source: HomeSource::Flag,
            sync_root: None,
        }
    }

    fn run_with(&self, args: &[&str], vars: &[(&str, &str)]) -> Output {
        let mut command = self.th.command(env!("CARGO_BIN_EXE_loadout"));
        if let Some(git) = &self.git {
            command.envs(git_vars(&self.th, git));
        }
        if self.snyk.is_some() {
            command.env(brand::TEST_AGENT_SCAN_ENV_VAR, agent_scan_stub::binary());
        }
        command
            .envs(vars.iter().copied())
            .arg("--home")
            .arg(&self.home)
            .args(args)
            .output()
            .expect("run loadout")
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_with(args, &[])
    }

    /// Exit code, stdout and stderr of a human-readable run.
    fn text(&self, args: &[&str]) -> (i32, String, String) {
        let output = self.run(args);
        (
            output.status.code().unwrap(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    fn json_with(&self, args: &[&str], vars: &[(&str, &str)]) -> (i32, Value, String) {
        let output = self.run_with(&[&["--json"], args].concat(), vars);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let doc = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
            panic!(
                "{args:?}: stdout is not JSON ({err}):\n{}\nstderr:\n{stderr}",
                String::from_utf8_lossy(&output.stdout),
            )
        });
        (output.status.code().unwrap(), doc, stderr)
    }

    fn json(&self, args: &[&str]) -> (i32, Value, String) {
        self.json_with(args, &[])
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (code, doc, stderr) = self.json(args);
        assert_eq!(code, 0, "{args:?}: {doc:#}\n{stderr}");
        doc
    }

    fn fails(&self, args: &[&str], code: &str) -> Value {
        let (exit, doc, _) = self.json(args);
        assert_ne!(exit, 0, "{args:?}: {doc:#}");
        assert_eq!(doc["error"]["code"], code, "{args:?}: {doc:#}");
        doc
    }

    /// A skill folder to install.
    fn folder(&self, name: &str) -> String {
        let dir = self.th.root().join("src").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), SKILL.replace("tidy", name)).unwrap();
        dir.to_string_lossy().into_owned()
    }

    /// A library skill, installed without the scanner.
    fn library_skill(&self, name: &str) {
        let doc = self.ok(&["skills", "add", &self.folder(name), "--no-external"]);
        assert!(doc.get("external").is_none(), "{doc:#}");
    }

    fn settings_text(&self) -> String {
        std::fs::read_to_string(self.home.join("state").join("local-settings.toml"))
            .unwrap_or_default()
    }

    /// Installs the 0.7.0 Snyk Agent Scan where 0.7.0 put it, turned on.
    fn with_snyk_0_7_0(mut self) -> Self {
        let home = self.loadout_home();
        self.snyk = Some(AgentScanStub::install(&home));
        snyk::set_enabled(&agent_loadout_core::fs::RealFs, &home, true).unwrap();
        self
    }

    fn git(&self, cwd: &Path, args: &[&str]) {
        let git = self.git.as_ref().expect("a test with git");
        let status = self
            .th
            .command(git)
            .envs(git_vars(&self.th, git))
            .current_dir(cwd)
            .args(args)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed");
    }

    /// Commits `skill_md` as the work tree's `SKILL.md` and pushes it.
    fn push(&self, work: &Path, skill_md: &str, message: &str) {
        std::fs::write(work.join("SKILL.md"), skill_md).unwrap();
        self.git(work, &["add", "-A"]);
        self.git(work, &["commit", "--quiet", "-m", message]);
        self.git(work, &["push", "--quiet", "origin", "HEAD:main"]);
    }
}

fn skill_ids(cli: &Cli) -> Vec<String> {
    cli.ok(&["skills", "ls"])["skills"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|skill| skill["id"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn the_scanner_and_its_flags_are_visible() {
    let cli = Cli::new();
    let (_, help, _) = cli.text(&["--help"]);
    assert!(help.contains("scanner"), "{help}");
    assert!(help.contains("SkillSpector"), "{help}");
    for command in ["add", "update"] {
        let (_, help, _) = cli.text(&["skills", command, "--help"]);
        assert!(help.contains("--external"), "{help}");
        assert!(help.contains("--no-external"), "{help}");
        assert!(help.contains("SkillSpector"), "{help}");
        assert!(!help.contains("Snyk"), "{help}");
    }
    let (_, help, _) = cli.text(&["scanner", "--help"]);
    for command in [
        "ls", "status", "prepare", "enable", "disable", "run", "remove",
    ] {
        assert!(help.contains(command), "{command}: {help}");
    }
    for hidden in ["install", "token"] {
        assert!(
            !help
                .lines()
                .any(|line| line.trim_start().starts_with(hidden)),
            "{help}"
        );
    }
}

#[test]
fn status_when_ready_through_uvx() {
    let cli = Cli::ready();
    let doc = cli.ok(&["scanner", "status"]);
    assert_eq!(doc["schema"], 1, "{doc:#}");
    assert_eq!(doc["name"], "skillspector", "{doc:#}");
    assert_eq!(doc["tested_version"], "2.13.7", "{doc:#}");
    assert_eq!(doc["python"], "3.13", "{doc:#}");
    assert_eq!(doc["availability"], "ready", "{doc:#}");
    assert_eq!(doc["runtime"], "uvx", "{doc:#}");
    let program = doc["program"].as_str().unwrap();
    assert!(
        program.ends_with(&format!("uvx{}", std::env::consts::EXE_SUFFIX)),
        "{doc:#}"
    );
    assert_eq!(doc["version"], "2.13.7", "{doc:#}");
    assert_eq!(doc["version_tested"], true, "{doc:#}");
    assert!(doc["note"].is_null(), "{doc:#}");
    assert_eq!(doc["enabled"], false, "{doc:#}");
    assert_eq!(doc["timeout_secs"], 120, "{doc:#}");
    assert!(
        doc["sends"].as_str().unwrap().contains("OSV.dev"),
        "{doc:#}"
    );
    assert!(doc["setup"].is_null(), "{doc:#}");
    assert_eq!(doc["snyk_installed"], false, "{doc:#}");
    // The probe ran offline.
    let probe = cli.stub.calls().pop().expect("a --version probe");
    assert!(probe.argv.iter().any(|arg| arg == "--offline"), "{probe:?}");
    assert!(probe.argv.iter().any(|arg| arg == "--version"), "{probe:?}");

    let (code, text, _) = cli.text(&["scanner", "status"]);
    assert_eq!(code, 0);
    assert!(text.contains("Runtime:   uvx ("), "{text}");
    assert!(
        text.contains("--from skillspector==2.13.7 skillspector"),
        "{text}"
    );
    assert!(text.contains("Turned on: no"), "{text}");
    assert!(text.contains("never uploads the skill"), "{text}");
}

#[test]
fn status_when_ready_on_path_with_an_untested_version() {
    let cli = Cli::new();
    cli.stub.add("skillspector");
    cli.stub.set(&StubConfig {
        version: "skillspector 2.14.0".to_owned(),
        ..StubConfig::default()
    });
    let doc = cli.ok(&["scanner", "status"]);
    assert_eq!(doc["availability"], "ready", "{doc:#}");
    assert_eq!(doc["runtime"], "path", "{doc:#}");
    assert!(
        doc["program"].as_str().unwrap().contains("skillspector"),
        "{doc:#}"
    );
    assert_eq!(doc["version"], "2.14.0", "{doc:#}");
    assert_eq!(doc["version_tested"], false, "{doc:#}");
    assert_eq!(
        doc["note"], "SkillSpector 2.14.0 is not tested with this Loadout (tested: 2.13.7)",
        "{doc:#}"
    );
    let (_, text, _) = cli.text(&["scanner", "status"]);
    assert!(text.contains("Runtime:   on PATH ("), "{text}");
    assert!(text.contains("is not tested with this Loadout"), "{text}");
}

#[test]
fn status_when_it_needs_prepare_or_is_not_available() {
    let cli = Cli::new();
    let doc = cli.ok(&["scanner", "status"]);
    assert_eq!(doc["availability"], "not_available", "{doc:#}");
    assert!(doc["runtime"].is_null(), "{doc:#}");
    assert!(doc["program"].is_null(), "{doc:#}");
    assert!(doc["version"].is_null(), "{doc:#}");
    assert_eq!(doc["version_tested"], true, "{doc:#}");
    assert!(doc["setup"].as_str().unwrap().contains("pipx"), "{doc:#}");
    let (code, text, _) = cli.text(&["scanner", "status"]);
    assert_eq!(code, 0);
    assert!(
        text.contains("Loadout doesn't install SkillSpector"),
        "{text}"
    );

    cli.stub.add("uvx");
    let doc = cli.ok(&["scanner", "status"]);
    assert_eq!(doc["availability"], "needs_prepare", "{doc:#}");
    assert_eq!(doc["runtime"], "uvx", "{doc:#}");
    assert!(doc["program"].is_string(), "{doc:#}");
    assert!(doc["version"].is_null(), "{doc:#}");
    assert!(doc["setup"].is_null(), "{doc:#}");
    let (_, text, _) = cli.text(&["scanner", "status"]);
    assert!(text.contains("`loadout scanner prepare`"), "{text}");

    let doc = cli.ok(&["scanner", "ls"]);
    let scanners = doc["scanners"].as_array().unwrap();
    assert_eq!(scanners[0]["name"], "builtin", "{doc:#}");
    assert_eq!(scanners[1]["name"], "skillspector", "{doc:#}");
    assert_eq!(scanners[1]["availability"], "needs_prepare", "{doc:#}");
    assert_eq!(scanners[1]["enabled"], false, "{doc:#}");
    let (_, text, _) = cli.text(&["scanner", "ls"]);
    assert!(text.contains("skillspector   off"), "{text}");
}

#[test]
fn prepare_downloads_once_with_progress_on_stderr() {
    let cli = Cli::new();
    cli.stub.add("uvx");
    cli.stub.set(&StubConfig {
        prepare_lines: vec![
            "Downloading skillspector (1.2MiB)".to_owned(),
            "Installed 42 packages in 3s".to_owned(),
        ],
        ..StubConfig::default()
    });
    let (code, text, stderr) = cli.text(&["scanner", "prepare"]);
    assert_eq!(code, 0, "{text}\n{stderr}");
    assert!(stderr.contains("from PyPI"), "{stderr}");
    assert!(
        stderr.contains("  Downloading skillspector (1.2MiB)"),
        "{stderr}"
    );
    assert!(stderr.contains("  Installed 42 packages"), "{stderr}");
    assert!(
        text.contains("SkillSpector 2.13.7 is ready (uvx)"),
        "{text}"
    );
    assert!(text.contains("loadout scanner enable"), "{text}");
    assert!(cli.stub.prepared());
    // Prepare is the one call without --offline.
    let online: Vec<_> = cli
        .stub
        .calls()
        .into_iter()
        .filter(|call| !call.argv.iter().any(|arg| arg == "--offline"))
        .collect();
    assert_eq!(online.len(), 1, "{online:?}");
    assert_eq!(cli.ok(&["scanner", "status"])["availability"], "ready");

    // Again: answers at once, still ready.
    let doc = cli.ok(&["scanner", "prepare"]);
    assert_eq!(doc["prepared"], true, "{doc:#}");
    assert_eq!(doc["availability"], "ready", "{doc:#}");
}

#[test]
fn prepare_failures() {
    let cli = Cli::new();
    // Nothing on PATH: the setup text.
    let doc = cli.fails(&["scanner", "prepare"], "scanner_unavailable");
    assert!(
        doc["error"]["message"].as_str().unwrap().contains("pipx"),
        "{doc:#}"
    );
    // A skillspector on PATH, no uv: nothing to prepare.
    cli.stub.add("skillspector");
    let doc = cli.ok(&["scanner", "prepare"]);
    assert_eq!(doc["prepared"], false, "{doc:#}");
    assert_eq!(doc["runtime"], "path", "{doc:#}");
    let (code, text, _) = cli.text(&["scanner", "prepare"]);
    assert_eq!(code, 0);
    assert!(text.contains("nothing to prepare"), "{text}");

    // uv fails to download.
    let cli = Cli::new();
    cli.stub.add("uvx");
    cli.stub.set(&StubConfig {
        prepare_exit: 2,
        ..StubConfig::default()
    });
    let doc = cli.fails(&["scanner", "prepare"], "scanner_download_failed");
    assert!(
        doc["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Failed to fetch"),
        "{doc:#}"
    );
    assert!(!cli.stub.prepared());
    assert_eq!(
        cli.ok(&["scanner", "status"])["availability"],
        "needs_prepare"
    );
}

#[test]
fn enable_and_disable() {
    let cli = Cli::new();
    let (code, text, _) = cli.text(&["scanner", "enable"]);
    assert_eq!(code, 0);
    assert!(text.contains("SkillSpector is on"), "{text}");
    assert!(text.contains("never uploads the skill"), "{text}");
    assert!(
        text.contains("Loadout doesn't install SkillSpector"),
        "{text}"
    );
    assert!(cli.settings_text().contains("[skillspector]"));

    cli.stub.add("uvx");
    let (_, text, _) = cli.text(&["scanner", "enable"]);
    assert!(text.contains("run `loadout scanner prepare`"), "{text}");

    cli.stub.set_prepared(true);
    let (_, text, _) = cli.text(&["scanner", "enable"]);
    assert!(!text.contains("prepare"), "{text}");
    assert!(!text.contains("Loadout doesn't install"), "{text}");

    let doc = cli.ok(&["scanner", "disable"]);
    assert_eq!(doc["enabled"], false, "{doc:#}");
    let (_, text, _) = cli.text(&["scanner", "disable"]);
    assert!(text.contains("SkillSpector is off"), "{text}");
    let doc = cli.ok(&["scanner", "enable"]);
    assert_eq!(doc["enabled"], true, "{doc:#}");
    assert_eq!(doc["availability"], "ready", "{doc:#}");
}

#[test]
fn run_is_refused_while_off() {
    let cli = Cli::ready();
    cli.library_skill("one");
    cli.fails(&["scanner", "run", "one"], "scanner_unavailable");
    assert!(cli.stub.scans().is_empty());
}

/// `scanner run` on a library skill, for each kind of report.
#[test]
fn run_on_a_library_skill() {
    let cli = Cli::ready();
    cli.library_skill("one");
    cli.ok(&["scanner", "enable"]);

    cli.stub.answer(&output("safe.json"), 0);
    let doc = cli.ok(&["scanner", "run", "one"]);
    assert_eq!(doc["id"], "one", "{doc:#}");
    let external = &doc["external"];
    assert_eq!(external["scanner"], "skillspector", "{doc:#}");
    assert_eq!(external["version"], "2.13.7", "{doc:#}");
    assert_eq!(external["status"], "scanned", "{doc:#}");
    assert!(external["reason"].is_null(), "{doc:#}");
    assert_eq!(external["verdict"], "clean", "{doc:#}");
    assert_eq!(external["incomplete"], false, "{doc:#}");
    let scan = cli.stub.scans().pop().expect("a scan");
    assert_eq!(scan.files, vec!["SKILL.md".to_owned()], "{scan:?}");
    assert_eq!(scan.stdin_bytes, 0, "{scan:?}");
    let (code, text, _) = cli.text(&["scanner", "run", "one"]);
    assert_eq!(code, 0);
    assert!(text.contains("SkillSpector: clean (0 findings)"), "{text}");

    // Dangerous: reported (exit 0), never gated here.
    cli.stub.answer(&output("do-not-install.json"), 1);
    let doc = cli.ok(&["scanner", "run", "one"]);
    assert_eq!(doc["external"]["verdict"], "dangerous", "{doc:#}");
    let (code, text, _) = cli.text(&["scanner", "run", "one"]);
    assert_eq!(code, 0);
    assert!(text.contains("SkillSpector: dangerous ("), "{text}");
    assert!(text.contains("--allow-dangerous"), "{text}");

    // Incomplete: never "clean".
    cli.stub.answer(&output("incomplete.json"), 1);
    let doc = cli.ok(&["scanner", "run", "one"]);
    assert_eq!(doc["external"]["incomplete"], true, "{doc:#}");
    assert!(
        doc["external"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["rule"] == "incomplete"),
        "{doc:#}"
    );
    let (_, text, _) = cli.text(&["scanner", "run", "one"]);
    assert!(text.contains("SkillSpector: not fully scanned ("), "{text}");
    assert!(!text.contains("clean"), "{text}");

    // Not understood: no verdict, exit 1.
    for (fixture, exit) in [
        ("llm-requested.json", 0),
        ("execution-failed.json", 0),
        ("malformed.txt", 0),
    ] {
        cli.stub.answer(&output(fixture), exit);
        let (code, doc, _) = cli.json(&["scanner", "run", "one"]);
        assert_eq!(code, 1, "{fixture}: {doc:#}");
        assert_eq!(
            doc["external"]["status"], "not_understood",
            "{fixture}: {doc:#}"
        );
        assert!(doc["external"]["verdict"].is_null(), "{fixture}: {doc:#}");
    }
    let (code, text, _) = cli.text(&["scanner", "run", "one"]);
    assert_eq!(code, 1);
    assert!(
        text.contains("SkillSpector: scanner output not understood (version 2.13.7)"),
        "{text}"
    );

    // SkillSpector failed (exit 2): not scanned, exit 1.
    cli.stub.set(&StubConfig {
        exit: 2,
        stderr: Some("Error: something broke".to_owned()),
        ..StubConfig::default()
    });
    let (code, doc, _) = cli.json(&["scanner", "run", "one"]);
    assert_eq!(code, 1, "{doc:#}");
    assert_eq!(doc["external"]["status"], "not_scanned", "{doc:#}");
    assert_eq!(doc["external"]["reason"], "failed", "{doc:#}");
    assert!(doc["external"]["verdict"].is_null(), "{doc:#}");
    let (code, text, _) = cli.text(&["scanner", "run", "one"]);
    assert_eq!(code, 1);
    assert!(text.contains("SkillSpector: not scanned ("), "{text}");

    // Not prepared (uv lost its cache): not scanned, exit 1.
    cli.stub.set_prepared(false);
    let (code, doc, _) = cli.json(&["scanner", "run", "one"]);
    assert_eq!(code, 1, "{doc:#}");
    assert_eq!(doc["external"]["reason"], "not_prepared", "{doc:#}");

    let (code, doc, _) = cli.json(&["scanner", "run", "missing"]);
    assert_ne!(code, 0, "{doc:#}");
    assert!(doc["error"]["code"].is_string(), "{doc:#}");
}

/// A dangerous report blocks an install of a local folder unless
/// --allow-dangerous; a not-scanned one never blocks.
#[test]
fn a_dangerous_report_gates_skills_add() {
    let cli = Cli::ready();
    cli.ok(&["scanner", "enable"]);
    cli.stub.answer(&output("do-not-install-high.json"), 1);

    let (code, doc, stderr) = cli.json(&["skills", "add", &cli.folder("bad")]);
    assert_eq!(code, 8, "{doc:#}\n{stderr}");
    assert_eq!(doc["error"]["code"], "scan_blocked", "{doc:#}");
    assert!(
        doc["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("SkillSpector found"),
        "{doc:#}"
    );
    assert_eq!(doc["scan"]["verdict"], "clean", "{doc:#}");
    assert_eq!(doc["external"]["verdict"], "dangerous", "{doc:#}");
    assert!(
        doc["external"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["rule"] == "DO_NOT_INSTALL"),
        "{doc:#}"
    );
    assert!(skill_ids(&cli).is_empty());

    let (code, text, stderr) = cli.text(&["skills", "add", &cli.folder("bad")]);
    assert_eq!(code, 8, "{text}\n{stderr}");
    assert!(text.contains("SkillSpector: dangerous ("), "{text}");
    assert!(stderr.contains("Running SkillSpector on bad"), "{stderr}");

    let doc = cli.ok(&["skills", "add", &cli.folder("bad"), "--allow-dangerous"]);
    assert_eq!(doc["skill"]["id"], "bad", "{doc:#}");
    assert_eq!(doc["external"]["verdict"], "dangerous", "{doc:#}");

    // Not prepared: reported, never blocks.
    cli.stub.set_prepared(false);
    let doc = cli.ok(&["skills", "add", &cli.folder("unprepared")]);
    assert_eq!(doc["external"]["status"], "not_scanned", "{doc:#}");
    assert_eq!(doc["external"]["reason"], "not_prepared", "{doc:#}");
    assert!(doc["external"]["verdict"].is_null(), "{doc:#}");
    let (code, text, _) = cli.text(&["skills", "add", &cli.folder("unprepared-2")]);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains("SkillSpector: not scanned (SkillSpector is not downloaded yet"),
        "{text}"
    );
}

#[test]
fn incomplete_and_clean_installs_report_but_do_not_block() {
    let cli = Cli::ready();
    cli.ok(&["scanner", "enable"]);
    cli.stub.answer(&output("incomplete.json"), 1);
    let (code, text, stderr) = cli.text(&["skills", "add", &cli.folder("partial")]);
    assert_eq!(code, 0, "{text}\n{stderr}");
    assert!(text.contains("Added skill partial."), "{text}");
    assert!(text.contains("SkillSpector: not fully scanned ("), "{text}");

    cli.stub.answer(&output("caution.json"), 0);
    let doc = cli.ok(&["skills", "add", &cli.folder("careful")]);
    assert_eq!(doc["external"]["verdict"], "warning", "{doc:#}");
    assert_eq!(doc["external"]["incomplete"], false, "{doc:#}");
}

/// `--no-external` skips a turned-on scanner; `--external` runs a
/// turned-off one; off by default nothing runs.
#[test]
fn the_flags_override_the_switch() {
    let cli = Cli::ready();
    cli.stub.answer(&output("safe.json"), 0);
    let doc = cli.ok(&["skills", "add", &cli.folder("default-off")]);
    assert!(doc.get("external").is_none(), "{doc:#}");
    assert!(cli.stub.scans().is_empty());

    let doc = cli.ok(&["skills", "add", &cli.folder("asked"), "--external"]);
    assert_eq!(doc["external"]["status"], "scanned", "{doc:#}");
    assert_eq!(cli.stub.scans().len(), 1);

    cli.ok(&["scanner", "enable"]);
    let doc = cli.ok(&["skills", "add", &cli.folder("skipped"), "--no-external"]);
    assert!(doc.get("external").is_none(), "{doc:#}");
    assert_eq!(cli.stub.scans().len(), 1);

    let doc = cli.ok(&["skills", "add", &cli.folder("on")]);
    assert_eq!(doc["external"]["status"], "scanned", "{doc:#}");
    assert_eq!(cli.stub.scans().len(), 2);

    // Nothing on PATH: not available, never blocks.
    let cli = Cli::new();
    let doc = cli.ok(&["skills", "add", &cli.folder("nothing"), "--external"]);
    assert_eq!(doc["external"]["reason"], "not_available", "{doc:#}");
}

/// What the scan gets: the pinned offline argv, closed stdin, and no
/// provider key from Loadout's own environment.
#[test]
fn provider_keys_never_reach_the_scanner() {
    let cli = Cli::ready();
    cli.stub.answer(&output("safe.json"), 0);
    let (code, doc, stderr) = cli.json_with(
        &["skills", "add", &cli.folder("keys"), "--external"],
        PROVIDER_KEYS,
    );
    assert_eq!(code, 0, "{doc:#}\n{stderr}");
    assert_eq!(doc["external"]["status"], "scanned", "{doc:#}");
    let calls = cli.stub.calls();
    assert!(calls.iter().any(|call| call.is_scan()), "{calls:?}");
    for call in &calls {
        for (name, value) in PROVIDER_KEYS {
            let leaked = call.env.get(*name).is_some_and(|seen| seen == value);
            assert!(!leaked, "{name} reached {}: {:?}", call.program, call.argv);
            assert!(
                !call.env.values().any(|seen| seen == value),
                "{name}'s value reached {}",
                call.program
            );
        }
        assert_eq!(
            call.env.get("LANGSMITH_TRACING").map(String::as_str),
            Some("false")
        );
        assert_eq!(call.stdin_bytes, 0, "{call:?}");
    }
    let scan = calls.iter().find(|call| call.is_scan()).unwrap();
    let argv: Vec<&str> = scan.argv.iter().map(String::as_str).collect();
    assert_eq!(
        &argv[..argv.len() - 1],
        &[
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
        ],
        "{argv:?}"
    );
}

#[test]
fn install_and_token_are_refused() {
    let cli = Cli::ready();
    for args in [
        &["scanner", "install"][..],
        &["scanner", "token", "keyring://loadout/snyk"],
        &["scanner", "token", "--clear"],
    ] {
        cli.fails(args, "scanner_unavailable");
    }
    let (_, help, _) = cli.text(&["scanner", "--help"]);
    assert!(!help.contains("Snyk Agent Scan (disabled)"), "{help}");
}

/// `scanner remove` deletes a 0.7.0 install and its settings entry;
/// `status` says one remains until then.
#[test]
fn remove_cleans_up_a_0_7_0_snyk_install() {
    let cli = Cli::new().with_snyk_0_7_0();
    assert_eq!(cli.ok(&["scanner", "status"])["snyk_installed"], true);
    let (_, text, _) = cli.text(&["scanner", "status"]);
    assert!(text.contains("`loadout scanner remove`"), "{text}");
    let doc = cli.ok(&["scanner", "remove"]);
    assert_eq!(doc["removed"], true, "{doc:#}");
    assert_eq!(cli.ok(&["scanner", "status"])["snyk_installed"], false);
    let text = cli.settings_text();
    assert!(!text.contains("snyk_agent_scan"), "{text}");
    assert_eq!(cli.ok(&["scanner", "remove"])["removed"], false);
    // The 0.7.0 scanner never ran.
    assert!(cli.snyk.as_ref().unwrap().calls().is_empty());
}

/// `skills update --external` on a git skill: a dangerous SkillSpector
/// report on the new content blocks the update (exit 8, the skill stays at
/// its commit) unless --allow-dangerous; not-understood and not-prepared
/// reports never block.
#[test]
fn skills_update_external_gates_a_dangerous_update() {
    let Some(git) = system_git_or_skip("skills_update_external_gates_a_dangerous_update") else {
        return;
    };
    let mut cli = Cli::ready();
    cli.git = Some(git.clone());
    let root = cli.th.root().to_path_buf();
    let bare = root.join("up.git");
    init_bare(&cli.th, &git, &bare);
    cli.git(&root, &["init", "--quiet", "--initial-branch=main", "work"]);
    let work = root.join("work");
    cli.git(&work, &["remote", "add", "origin", "../up.git"]);
    let version = |n: u32| format!("---\nname: alpha\ndescription: Alpha.\n---\n\n# Alpha v{n}\n");
    cli.push(&work, &version(1), "v1");
    let url = bare.to_string_lossy().into_owned();
    let doc = cli.ok(&["skills", "add", &url, "--trust-source=repo"]);
    assert!(doc.get("external").is_none(), "off by default: {doc:#}");
    let first = doc["skill"]["source_commit"].as_str().unwrap().to_owned();
    assert!(cli.stub.scans().is_empty());

    // v2: SkillSpector says do not install.
    cli.push(&work, &version(2), "v2");
    cli.stub.answer(&output("do-not-install.json"), 1);
    let (code, doc, stderr) = cli.json(&["skills", "update", "alpha", "--external"]);
    assert_eq!(code, 8, "{doc:#}\n{stderr}");
    let result = &doc["skills"][0];
    assert_eq!(result["status"], "blocked", "{doc:#}");
    assert_eq!(result["error"]["code"], "scan_blocked", "{doc:#}");
    assert_eq!(result["scan"]["verdict"], "clean", "{doc:#}");
    assert_eq!(result["external"]["scanner"], "skillspector", "{doc:#}");
    assert_eq!(result["external"]["verdict"], "dangerous", "{doc:#}");
    let scan = cli.stub.scans().pop().expect("the update was scanned");
    assert_eq!(scan.files, vec!["SKILL.md".to_owned()], "{scan:?}");
    // Not applied: still at the first commit, the update still available.
    let doc = cli.ok(&["skills", "update", "alpha", "--check", "--no-external"]);
    assert_eq!(doc["skills"][0]["status"], "available", "{doc:#}");
    assert_eq!(doc["skills"][0]["current"], first.as_str(), "{doc:#}");
    assert!(doc["skills"][0].get("external").is_none(), "{doc:#}");
    let (code, text, _) = cli.text(&["skills", "update", "alpha", "--external"]);
    assert_eq!(code, 8, "{text}");
    assert!(text.contains("SkillSpector: dangerous ("), "{text}");

    let doc = cli.ok(&[
        "skills",
        "update",
        "alpha",
        "--external",
        "--allow-dangerous",
    ]);
    let result = &doc["skills"][0];
    assert_eq!(result["status"], "updated", "{doc:#}");
    assert_eq!(result["external"]["verdict"], "dangerous", "{doc:#}");
    let second = result["to"].as_str().unwrap().to_owned();
    assert_ne!(second, first);

    // v3: output not understood; applies without the flag.
    cli.push(&work, &version(3), "v3");
    cli.stub.answer(&output("llm-requested.json"), 0);
    let doc = cli.ok(&["skills", "update", "alpha", "--external"]);
    let result = &doc["skills"][0];
    assert_eq!(result["status"], "updated", "{doc:#}");
    assert_eq!(result["external"]["status"], "not_understood", "{doc:#}");
    assert!(result["external"]["verdict"].is_null(), "{doc:#}");
    assert_ne!(result["to"], second.as_str(), "{doc:#}");

    // v4: uv lost the download (not prepared); applies without the flag.
    cli.push(&work, &version(4), "v4");
    cli.stub.answer(&output("do-not-install.json"), 1);
    cli.stub.set_prepared(false);
    let scans = cli.stub.scans().len();
    let doc = cli.ok(&["skills", "update", "alpha", "--external"]);
    let result = &doc["skills"][0];
    assert_eq!(result["status"], "updated", "{doc:#}");
    assert_eq!(result["external"]["status"], "not_scanned", "{doc:#}");
    assert_eq!(result["external"]["reason"], "not_prepared", "{doc:#}");
    assert_eq!(cli.stub.scans().len(), scans, "nothing ran");
    let doc = cli.ok(&["skills", "update", "alpha", "--check", "--no-external"]);
    assert_eq!(doc["skills"][0]["status"], "up_to_date", "{doc:#}");
}

//! `loadout scanner ...` and `skills add|update --external/--no-external`
//! (#27) against a temp `LOADOUT_HOME`, with the scanner disabled in this
//! build (`scan::external::AVAILABLE`, since 0.7.1): it is hidden from
//! help, never runs (not even with `--external` or a 0.7.0 setting that
//! turned it on), every subcommand but `status` and `remove` refuses, and
//! `remove` still cleans up a 0.7.0 install. Debug builds of `loadout`
//! would run `agent-scan-stub` through `LOADOUT_TEST_AGENT_SCAN`; the
//! tests count its calls (none).

use std::path::PathBuf;
use std::process::Output;

use agent_loadout_core::brand;
use agent_loadout_core::paths::{HomeSource, LoadoutHome};
use agent_loadout_core::scan::external;
use agent_loadout_testkit::TempHome;
use agent_loadout_testkit::agent_scan_stub::{self, AgentScanStub, output};
use serde_json::Value;

const TOKEN: &str = "snyk-cli-test-token-9d2e";
const SKILL: &str = "---\nname: tidy\ndescription: Tidies.\n---\n\n# Tidy\n\nSort the imports.\n";

struct Cli {
    th: TempHome,
    home: PathBuf,
    stub: Option<AgentScanStub>,
}

impl Cli {
    fn new() -> Self {
        let th = TempHome::new();
        let home = th.root().join("lh");
        Self {
            th,
            home,
            stub: None,
        }
    }

    fn loadout_home(&self) -> LoadoutHome {
        LoadoutHome {
            path: self.home.clone(),
            source: HomeSource::Flag,
            sync_root: None,
        }
    }

    /// Installs the stub where 0.7.0 installed the scanner and stores the
    /// token in the test keyring, as a 0.7.0 user would have.
    fn with_stub_files(mut self) -> Self {
        let stub = AgentScanStub::install(&self.loadout_home());
        stub.answer(&output("clean.json"));
        std::fs::write(
            self.th.keyring_file(),
            serde_json::json!({ "loadout/snyk": TOKEN }).to_string(),
        )
        .unwrap();
        self.stub = Some(stub);
        self
    }

    /// What a 0.7.0 user who turned the scanner on left behind.
    fn turned_on_in_0_7_0(&self) {
        let fs = agent_loadout_core::fs::RealFs;
        let home = self.loadout_home();
        external::set_enabled(&fs, &home, true).unwrap();
        external::set_token(&fs, &home, None, Some("keyring://loadout/snyk")).unwrap();
    }

    fn settings_text(&self) -> String {
        std::fs::read_to_string(self.home.join("state").join("local-settings.toml"))
            .unwrap_or_default()
    }

    fn calls(&self) -> usize {
        self.stub.as_ref().map_or(0, |stub| stub.calls().len())
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = self.th.command(env!("CARGO_BIN_EXE_loadout"));
        if self.stub.is_some() {
            // The built stub: the same bytes as the installed copy, and
            // the asset name its marker records.
            command.env(brand::TEST_AGENT_SCAN_ENV_VAR, agent_scan_stub::binary());
        }
        command
            .arg("--home")
            .arg(&self.home)
            .args(args)
            .output()
            .expect("run loadout")
    }

    fn json(&self, args: &[&str]) -> (i32, Value, String) {
        let output = self.run(&[&["--json"], args].concat());
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let doc = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
            panic!(
                "{args:?}: stdout is not JSON ({err}):\n{}\nstderr:\n{stderr}",
                String::from_utf8_lossy(&output.stdout),
            )
        });
        (output.status.code().unwrap(), doc, stderr)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (code, doc, stderr) = self.json(args);
        assert_eq!(code, 0, "{args:?}: {doc:#}\n{stderr}");
        doc
    }

    fn fails(&self, args: &[&str], code: &str) {
        let (exit, doc, _) = self.json(args);
        assert_eq!(exit, 1, "{args:?}: {doc:#}");
        assert_eq!(doc["error"]["code"], code, "{args:?}: {doc:#}");
    }

    fn folder(&self, name: &str) -> String {
        let dir = self.th.root().join("src").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), SKILL.replace("tidy", name)).unwrap();
        dir.to_string_lossy().into_owned()
    }
}

#[test]
fn the_scanner_is_hidden_from_help() {
    let cli = Cli::new();
    let help = String::from_utf8_lossy(&cli.run(&["--help"]).stdout).into_owned();
    assert!(!help.contains("scanner"), "{help}");
    for command in ["add", "update"] {
        let output = cli.run(&["skills", command, "--help"]);
        let help = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(!help.contains("--external"), "{help}");
        assert!(!help.contains("Snyk"), "{help}");
    }
}

#[test]
fn scanner_subcommands_refuse_except_status_and_remove() {
    let cli = Cli::new().with_stub_files();
    for args in [
        &["scanner", "ls"][..],
        &["scanner", "install"],
        &["scanner", "enable"],
        &["scanner", "disable"],
        &["scanner", "token", "keyring://loadout/snyk"],
        &["scanner", "run", "one"],
    ] {
        cli.fails(args, "scanner_unavailable");
    }
    let doc = cli.ok(&["scanner", "status"]);
    assert_eq!(doc["available"], false, "{doc:#}");
    assert!(doc.get("sends").is_none(), "{doc:#}");
    let text = String::from_utf8_lossy(&cli.run(&["scanner", "status"]).stdout).into_owned();
    assert!(text.contains("disabled in this build"), "{text}");
    assert_eq!(cli.calls(), 0);
}

/// A 0.7.0 setting that turned it on, an installed scanner and
/// `--external`: nothing runs, `--external` gets a one-line note.
#[test]
fn nothing_runs_even_when_turned_on_or_asked_for() {
    let cli = Cli::new().with_stub_files();
    cli.turned_on_in_0_7_0();
    let doc = cli.ok(&["skills", "add", &cli.folder("one")]);
    assert!(doc.get("external").is_none(), "{doc:#}");
    let (code, doc, stderr) = cli.json(&["skills", "add", &cli.folder("two"), "--external"]);
    assert_eq!(code, 0, "{doc:#}\n{stderr}");
    assert!(doc.get("external").is_none(), "{doc:#}");
    assert!(stderr.contains("--external is ignored"), "{stderr}");
    cli.ok(&["skills", "add", &cli.folder("three"), "--no-external"]);
    assert_eq!(cli.calls(), 0, "the disabled scanner ran");
}

/// `scanner remove` deletes a 0.7.0 install and its settings entry.
#[test]
fn remove_cleans_up_a_0_7_0_install() {
    let cli = Cli::new().with_stub_files();
    cli.turned_on_in_0_7_0();
    assert_eq!(cli.ok(&["scanner", "status"])["installed"], true);
    let doc = cli.ok(&["scanner", "remove"]);
    assert_eq!(doc["removed"], true);
    let doc = cli.ok(&["scanner", "status"]);
    assert_eq!(doc["installed"], false, "{doc:#}");
    assert_eq!(doc["enabled"], false, "{doc:#}");
    let text = cli.settings_text();
    assert!(!text.contains("snyk_agent_scan"), "{text}");
    assert_eq!(cli.ok(&["scanner", "remove"])["removed"], false);
    assert_eq!(cli.calls(), 0);
}

//! `loadout server add --from` (#5) against a temp `LOADOUT_HOME` and local
//! folders from `fixtures/mcp-sources`: the dry run, the trust flag, picks,
//! the scan gate (exit 8), provenance in the server file, `server update
//! --check` and `server rm`. Git sources are covered by the core tests.

use std::path::{Path, PathBuf};
use std::process::Output;

use agent_loadout_testkit::TempHome;
use serde_json::Value;

struct Cli {
    th: TempHome,
    home: PathBuf,
}

impl Cli {
    fn new() -> Self {
        let th = TempHome::new();
        let home = th.root().join("lh");
        Self { th, home }
    }

    fn run(&self, args: &[&str]) -> Output {
        self.th
            .command(env!("CARGO_BIN_EXE_loadout"))
            .arg("--home")
            .arg(&self.home)
            .args(args)
            .output()
            .expect("run loadout")
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let output = self.run(&[&["--json"], args].concat());
        let doc = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
            panic!(
                "{args:?}: stdout is not JSON ({err}):\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.code().unwrap(), doc)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (code, doc) = self.json(args);
        assert_eq!(code, 0, "{args:?}: {doc:#}");
        doc
    }

    /// A copy of `fixtures/mcp-sources/<name>` at `~/code/<name>`.
    fn folder(&self, name: &str) -> String {
        let to = self.th.home().join("code").join(name);
        copy_dir(&fixture(name), &to);
        to.to_string_lossy().into_owned()
    }

    fn server_file(&self, id: &str) -> String {
        std::fs::read_to_string(
            self.home
                .join("store")
                .join("servers")
                .join(format!("{id}.toml")),
        )
        .unwrap()
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("mcp-sources")
        .join(name)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

#[test]
fn a_dry_run_lists_drafts_and_saves_nothing() {
    let cli = Cli::new();
    let folder = cli.folder("registry");
    let doc = cli.ok(&["server", "add", "--from", &folder, "--dry-run"]);
    assert_eq!(doc["saved"], Value::Null);
    assert_eq!(doc["source"]["kind"], "local");
    assert_eq!(doc["source"]["folder"], "${home}/code/registry");
    let drafts = doc["drafts"].as_array().unwrap();
    let keys: Vec<&str> = drafts.iter().map(|d| d["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["server.json#packages[0]", "server.json#remotes[0]"]);
    assert_eq!(drafts[0]["launch"], "npx -y @acme/weather-mcp@1.2.0");
    assert_eq!(drafts[0]["inputs"][0]["secret"], true);
    assert!(!cli.home.join("store").exists());
}

#[test]
fn saving_needs_the_trust_flag_and_a_pick_among_several() {
    let cli = Cli::new();
    let folder = cli.folder("registry");
    let (code, doc) = cli.json(&["server", "add", "--from", &folder]);
    assert_ne!(code, 0);
    assert_eq!(doc["error"]["code"], "source_trust_required", "{doc:#}");

    let (code, doc) = cli.json(&["server", "add", "--from", &folder, "--trust-source"]);
    assert_ne!(code, 0);
    assert_eq!(doc["error"]["code"], "invalid_value", "{doc:#}");

    // Saved; the instant apply then waits for the secret the server needs
    // (exit 5, like `loadout apply`).
    let (code, doc) = cli.json(&[
        "server",
        "add",
        "forecast",
        "--from",
        &folder,
        "--pick",
        "weather",
        "--client",
        "claude-code",
        "--trust-source",
    ]);
    assert_eq!(code, 5, "{doc:#}");
    assert_eq!(doc["saved"], serde_json::json!(["forecast"]));
    let text = cli.server_file("forecast");
    assert!(text.contains("origin = \"source:local\""), "{text}");
    assert!(
        text.contains("folder = \"${home}/code/registry\""),
        "{text}"
    );
    // Aliases come from the draft's id at resolve time.
    assert!(
        text.contains("WEATHER_API_KEY = \"${secret:WEATHER_WEATHER_API_KEY}\""),
        "{text}"
    );

    let doc = cli.ok(&["server", "update", "--check"]);
    assert_eq!(doc["servers"][0]["id"], "forecast");
    assert_eq!(doc["servers"][0]["status"], "up_to_date", "{doc:#}");

    cli.ok(&["server", "rm", "forecast"]);
}

#[test]
fn a_dangerous_command_exits_8_unless_allowed() {
    let cli = Cli::new();
    let folder = cli.folder("dangerous");
    let (code, doc) = cli.json(&["server", "add", "--from", &folder, "--trust-source"]);
    assert_eq!(code, 8, "{doc:#}");
    assert_eq!(doc["error"]["code"], "scan_blocked");
    let rules: Vec<&str> = doc["scans"][0]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["rule"].as_str().unwrap())
        .collect();
    assert!(rules.contains(&"pipe-to-shell"), "{rules:?}");
    let doc = cli.ok(&[
        "server",
        "add",
        "--from",
        &folder,
        "--trust-source",
        "--allow-dangerous",
    ]);
    assert_eq!(doc["saved"], serde_json::json!(["setup"]));
}

#[test]
fn manual_fields_do_not_mix_with_from() {
    let cli = Cli::new();
    let output = cli.run(&["server", "add", "--from", "x", "--url", "https://a.example"]);
    assert!(!output.status.success());
    let output = cli.run(&["server", "add"]);
    assert!(!output.status.success());
}

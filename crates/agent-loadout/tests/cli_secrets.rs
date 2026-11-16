//! `loadout secrets ...` and the leak scanner through the CLI, against a
//! temp `LOADOUT_HOME` and profile (`TempHome::command`: cleared
//! environment, `PATH` = the temp `bin/` holding `op-stub` as `op`, or
//! nothing). No test here reaches the OS keyring: keyring paths are
//! covered in core with the in-memory backend; the CLI tests stop before
//! the keyring is called.

use std::path::PathBuf;
use std::process::{Output, Stdio};

use agent_loadout_testkit::TempHome;
use agent_loadout_testkit::op_stub::OpStub;
use serde_json::{Value, json};

const ITEM: &str = "op://Example Vault/Fake Item/credential";
const ITEM_VALUE: &str = "fake-item-value-0001";

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

    fn op_stub(&self) -> OpStub {
        let mut stub = OpStub::install(&self.th.bin());
        stub.add_item(ITEM, ITEM_VALUE);
        stub
    }

    fn run(&self, args: &[&str]) -> Output {
        self.th
            .command(env!("CARGO_BIN_EXE_loadout"))
            .arg("--home")
            .arg(&self.home)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run loadout")
    }

    /// Runs with `--json`: exit code and document.
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

    fn statuses(&self, args: &[&str]) -> (i32, Vec<(String, String)>) {
        let (code, doc) = self.json(&[&["secrets", "check"], args].concat());
        let statuses = doc["aliases"]
            .as_array()
            .unwrap_or_else(|| panic!("{doc:#}"))
            .iter()
            .map(|alias| {
                (
                    alias["alias"].as_str().unwrap().to_owned(),
                    alias["status"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        (code, statuses)
    }
}

fn pair(alias: &str, status: &str) -> (String, String) {
    (alias.to_owned(), status.to_owned())
}

#[test]
fn providers_without_op_say_1password_cli_not_found() {
    let cli = Cli::new();
    let doc = cli.ok(&["secrets", "providers"]);
    let kinds: Vec<&str> = doc["providers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["env", "keyring", "onepassword"]);
    let op = &doc["providers"][2];
    assert_eq!(op["state"], "unavailable");
    assert_eq!(op["detail"], "1Password CLI not found");
    assert_eq!(
        op["features"],
        json!({"item_read": false, "environments": false, "environment_listing": false})
    );
    let text = cli.run(&["secrets", "providers"]);
    assert!(
        String::from_utf8_lossy(&text.stdout).contains("1Password CLI not found"),
        "{}",
        String::from_utf8_lossy(&text.stdout)
    );
    assert!(!cli.home.exists(), "providers creates nothing");
}

#[test]
fn providers_report_the_op_version_gates() {
    let cli = Cli::new();
    let mut stub = cli.op_stub();
    stub.set_version("2.40.0");
    let op = &cli.ok(&["secrets", "providers"])["providers"][2];
    assert_eq!(op["state"], "available");
    assert_eq!(op["version"], "2.40.0");
    assert_eq!(
        op["features"],
        json!({"item_read": true, "environments": false, "environment_listing": false})
    );
    assert_eq!(stub.calls(), ["--version"], "detection never signs in");
}

#[test]
fn set_ls_and_check_aliases() {
    let cli = Cli::new();
    let stub = cli.op_stub();
    let set = cli.ok(&[
        "secrets",
        "set",
        "GITHUB_PAT",
        "--ref",
        ITEM,
        "--description",
        "Fake PAT",
    ]);
    assert_eq!(set["scope"], "store");
    assert!(set["commit"].is_string());
    cli.ok(&["secrets", "set", "HASS_TOKEN", "--ref", "env:FAKE_UNSET"]);
    cli.ok(&[
        "server",
        "add",
        "github",
        "--env",
        "GITHUB_PERSONAL_ACCESS_TOKEN=${secret:GITHUB_PAT}",
        "--env",
        "EXTRA=${secret:UNDEFINED}",
        "--",
        "npx",
        "-y",
        "@example/github-mcp",
    ]);

    let listed = cli.ok(&["secrets", "ls"]);
    assert_eq!(
        listed["aliases"],
        json!([
            {
                "alias": "GITHUB_PAT",
                "ref": ITEM,
                "source": "store",
                "store_ref": null,
                "description": "Fake PAT",
                "env_name": "GITHUB_PAT",
                "servers": ["github"],
            },
            {
                "alias": "HASS_TOKEN",
                "ref": "env:FAKE_UNSET",
                "source": "store",
                "store_ref": null,
                "description": null,
                "env_name": "HASS_TOKEN",
                "servers": [],
            },
        ])
    );

    let (code, statuses) = cli.statuses(&[]);
    assert_eq!(code, 5, "a missing secret exits 5");
    assert_eq!(
        statuses,
        [
            pair("GITHUB_PAT", "resolved"),
            pair("HASS_TOKEN", "missing"),
            pair("UNDEFINED", "missing"),
        ]
    );
    let (code, statuses) = cli.statuses(&["GITHUB_PAT"]);
    assert_eq!((code, statuses), (0, vec![pair("GITHUB_PAT", "resolved")]));
    let text = cli.run(&["secrets", "check"]);
    for output in [&text.stdout, &text.stderr] {
        assert!(!String::from_utf8_lossy(output).contains(ITEM_VALUE));
    }

    // This machine only: the other machines keep the shared reference.
    let set = cli.ok(&[
        "secrets",
        "set",
        "HASS_TOKEN",
        "--this-machine",
        "--ref",
        "env:HOME",
    ]);
    assert_eq!(set["scope"], "machine");
    let listed = cli.ok(&["secrets", "ls"]);
    assert_eq!(listed["aliases"][1]["source"], "machine");
    assert_eq!(listed["aliases"][1]["ref"], "env:HOME");
    assert_eq!(listed["aliases"][1]["store_ref"], "env:FAKE_UNSET");
    let (_, statuses) = cli.statuses(&["HASS_TOKEN"]);
    assert_eq!(statuses, [pair("HASS_TOKEN", "resolved")]);

    let history = cli.ok(&["history"]);
    let summaries: Vec<&str> = history["commits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["summary"].as_str().unwrap())
        .collect();
    assert_eq!(
        &summaries[..2],
        [
            "secrets: set HASS_TOKEN on this machine",
            "server: add github"
        ]
    );
    assert!(summaries.contains(&"secrets: set GITHUB_PAT"));
    assert_eq!(
        cli.ok(&["history", "--entity", "secrets"])["commits"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(stub.calls().contains(&format!("read --no-newline {ITEM}")));
}

#[test]
fn without_op_only_1password_aliases_are_blocked() {
    let cli = Cli::new();
    cli.ok(&["secrets", "set", "GITHUB_PAT", "--ref", ITEM]);
    cli.ok(&["secrets", "set", "HOME_DIR", "--ref", "env:HOME"]);
    let (code, doc) = cli.json(&["secrets", "check"]);
    assert_eq!(code, 5);
    assert_eq!(doc["aliases"][0]["status"], "provider_unavailable");
    assert_eq!(doc["aliases"][0]["message"], "1Password CLI not found");
    assert_eq!(doc["aliases"][1]["status"], "resolved");
    // "Use environment variable on this machine" fixes it here.
    cli.ok(&[
        "secrets",
        "set",
        "GITHUB_PAT",
        "--this-machine",
        "--ref",
        "env:HOME",
    ]);
    let (code, _) = cli.json(&["secrets", "check"]);
    assert_eq!(code, 0);
}

#[test]
fn bad_input_is_refused_before_anything_is_stored() {
    let cli = Cli::new();
    let (code, doc) = cli.json(&["secrets", "set", "bad-alias", "--ref", "env:X"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("invalid_id"))
    );
    let (code, doc) = cli.json(&["secrets", "set", "X", "--ref", "op://vault/item"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("invalid_ref"))
    );
    // `--keyring` reads the value from stdin; nothing piped, nothing stored.
    let (code, doc) = cli.json(&["secrets", "set", "X", "--keyring"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("invalid_value"))
    );
    assert!(!cli.home.exists(), "nothing was created");
    let usage = cli.run(&["secrets", "set", "X"]);
    assert_eq!(
        usage.status.code(),
        Some(2),
        "--ref or --keyring is required"
    );

    cli.ok(&["secrets", "set", "HOME_DIR", "--ref", "env:HOME"]);
    let (code, doc) = cli.json(&["secrets", "clear", "HOME_DIR"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("invalid_value"))
    );
    let (code, doc) = cli.json(&["secrets", "clear", "NOPE"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("not_found"))
    );
}

#[test]
fn a_token_in_a_server_definition_is_refused() {
    let cli = Cli::new();
    // Assembled at runtime so the repository holds no token-shaped string.
    let token = ["gh", "p_", "FAKEfake0123456789FAKEfake0123456789"].concat();
    let (code, doc) = cli.json(&[
        "server",
        "add",
        "leaky",
        "--env",
        &format!("UPSTREAM={token}"),
        "--",
        "npx",
    ]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "leak_detected");
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(message.contains("servers/leaky.toml"), "{message}");
    assert!(!message.contains(&token), "{message}");
    assert_eq!(cli.ok(&["server", "ls"])["servers"], json!([]));
    // The refused token never reached history.
    let doctor = cli.ok(&["doctor", "--scan-history"]);
    assert_eq!(doctor["findings"], json!([]));
}

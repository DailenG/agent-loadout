//! `loadout status|diff|apply|rollback|import|adopt` and the instant apply
//! of `loadout server add`, against a temp `LOADOUT_HOME` and a temp
//! profile (`TempHome::command`: cleared environment, stub-only `PATH`).

use std::path::PathBuf;
use std::process::Output;

use agent_loadout_testkit::TempHome;
use serde_json::{Value, json};

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

    /// Runs with `--json` and returns the exit code and the document.
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

    fn claude(&self) -> PathBuf {
        self.th.home().join(".claude.json")
    }

    fn claude_entry(&self, name: &str) -> Option<Value> {
        let text = std::fs::read_to_string(self.claude()).ok()?;
        let value: Value = serde_json::from_str(&text).unwrap();
        value["mcpServers"].get(name).cloned()
    }

    fn history(&self) -> Vec<Value> {
        self.ok(&["history"])["commits"].as_array().unwrap().clone()
    }

    fn add_notes(&self, args: &[&str]) -> Value {
        self.ok(&[
            &[
                "server",
                "add",
                "notes",
                "--client",
                "claude-code",
                "--",
                "npx",
            ][..],
            args,
        ]
        .concat())
    }
}

#[test]
fn server_add_applies_in_the_same_commit() {
    let cli = Cli::new();
    let added = cli.add_notes(&["-y", "@example/notes-mcp"]);
    assert_eq!(added["apply"]["outcome"], "ok");
    assert_eq!(added["apply"]["written"], 1);
    assert_eq!(added["apply"]["trigger"], "instant");
    assert_eq!(
        cli.claude_entry("notes"),
        Some(json!({"type": "stdio", "command": "npx", "args": ["-y", "@example/notes-mcp"]}))
    );
    let history = cli.history();
    assert_eq!(
        history.len(),
        2,
        "init and one commit for the edit and its apply"
    );
    assert_eq!(history[0]["summary"], "server: add notes");
    assert_eq!(history[0]["apply"]["id"], added["apply"]["apply_id"]);
    assert_eq!(history[0]["apply"]["outcome"], "ok");

    let removed = cli.ok(&["server", "rm", "notes"]);
    assert_eq!(removed["apply"]["targets"][0]["remove"], 1);
    assert_eq!(cli.claude_entry("notes"), None);
}

#[test]
fn review_mode_status_diff_and_apply() {
    let cli = Cli::new();
    cli.ok(&["server", "add", "seed", "--", "seed-mcp"]);
    std::fs::write(
        cli.home.join("store").join("settings.toml"),
        "schema = 1\napply_mode = \"review\"\n",
    )
    .unwrap();
    let added = cli.add_notes(&["-y", "@example/notes-mcp"]);
    assert_eq!(added["apply"], Value::Null, "review mode does not apply");
    assert!(!cli.claude().exists());

    let (code, status) = cli.json(&["status", "--check"]);
    assert_eq!(code, 3);
    let entry = &status["targets"][0]["entries"][0];
    assert_eq!(
        (entry["name"].as_str(), entry["state"].as_str()),
        (Some("notes"), Some("pending_create"))
    );
    assert_eq!(status["targets"][0]["clients"], json!(["claude-code"]));

    let output = cli.run(&["diff", "--exit-code"]);
    assert_eq!(output.status.code(), Some(3));
    let diff = String::from_utf8_lossy(&output.stdout);
    assert!(diff.contains("+    \"notes\": {"), "{diff}");
    assert!(diff.contains("--- /dev/null"), "{diff}");

    let output = cli.run(&["apply", "--dry-run"]);
    assert!(output.status.success());
    assert!(!cli.claude().exists(), "a dry run changes nothing");

    let commits = cli.history().len();
    let applied = cli.ok(&["apply"]);
    assert_eq!(applied["outcome"], "ok");
    assert_eq!(applied["written"], 1);
    assert!(applied["commit"].is_string());
    assert!(cli.claude_entry("notes").is_some());
    let history = cli.history();
    assert_eq!(history.len(), commits + 1);
    assert_eq!(history[0]["summary"], "apply: 1 file (claude-code)");
    assert_eq!(history[0]["op"], "apply");

    let (code, _) = cli.json(&["status", "--check"]);
    assert_eq!(code, 0);
    let again = cli.ok(&["apply"]);
    assert_eq!(again["written"], 0);
    assert_eq!(again["commit"], Value::Null);
    assert_eq!(cli.history().len(), commits + 1);
}

#[test]
fn collisions_exit_4_until_adopted() {
    let cli = Cli::new();
    std::fs::write(
        cli.claude(),
        "{\n  \"mcpServers\": {\n    \"notes\": {\"command\": \"notes-fork\"}\n  }\n}\n",
    )
    .unwrap();
    // The store edit is committed, and the add exits like `apply` would (4).
    let (code, added) = cli.json(&[
        "server",
        "add",
        "notes",
        "--client",
        "claude-code",
        "--",
        "npx",
        "-y",
        "@example/notes-mcp",
    ]);
    assert_eq!(code, 4, "{added:#}");
    assert!(added["commit"].is_string());
    assert_eq!(added["apply"]["unresolved"], 1);
    assert_eq!(added["apply"]["written"], 0);
    assert_eq!(
        cli.claude_entry("notes"),
        Some(json!({"command": "notes-fork"}))
    );

    let (code, applied) = cli.json(&["apply"]);
    assert_eq!(code, 4, "{applied:#}");
    assert_eq!(applied["targets"][0]["entries"][0]["state"], "collision");
    let (code, _) = cli.json(&["apply", "--on-conflict", "skip"]);
    assert_eq!(code, 0);

    let adopted = cli.ok(&["adopt", "claude-code/notes"]);
    assert_eq!(adopted["not_adopted"], json!([]));
    assert_eq!(adopted["apply"]["written"], 1);
    assert_eq!(
        cli.claude_entry("notes").unwrap()["args"],
        json!(["-y", "@example/notes-mcp"])
    );
    assert_eq!(cli.history()[0]["summary"], "adopt: claude-code/notes");
    let ledger: Value = serde_json::from_str(
        &std::fs::read_to_string(cli.home.join("state").join("ledger.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(ledger["entries"][0]["origin"], "adopted");
    let (code, _) = cli.json(&["apply"]);
    assert_eq!(code, 0);

    let bad = cli.run(&["adopt", "no-slash"]);
    assert_eq!(bad.status.code(), Some(1));
}

#[test]
fn rollback_last_and_older_runs() {
    let cli = Cli::new();
    std::fs::write(cli.claude(), "{\n  \"theme\": \"dark\"\n}\n").unwrap();
    let first = cli.add_notes(&["-y", "@example/notes-mcp"]);
    let first_id = first["apply"]["apply_id"].as_str().unwrap().to_owned();

    let rolled = cli.ok(&["rollback", "last"]);
    assert_eq!(rolled["rolled_back"], first_id.as_str());
    assert_eq!(rolled["files"], json!(["servers/notes.toml"]));
    assert!(
        rolled["tag"]
            .as_str()
            .unwrap()
            .starts_with("auto/pre-rollback/")
    );
    assert_eq!(rolled["apply"]["targets"][0]["remove"], 1);
    assert_eq!(cli.claude_entry("notes"), None);
    assert!(
        std::fs::read_to_string(cli.claude())
            .unwrap()
            .contains("\"theme\": \"dark\"")
    );
    assert_eq!(
        cli.history()[0]["summary"],
        format!("rollback: apply {first_id}")
    );
    assert_eq!(cli.ok(&["server", "ls"])["servers"], json!([]));

    // Rolling back the rollback restores the original.
    cli.ok(&["rollback", "last"]);
    assert!(cli.claude_entry("notes").is_some());

    // An older run over later changes to the same files needs --yes.
    cli.ok(&["server", "rm", "notes"]);
    let (code, doc) = cli.json(&["rollback", &first_id]);
    assert_eq!(code, 4, "{doc:#}");
    assert_eq!(doc["error"]["code"], "confirmation_required");
    let (code, _) = cli.json(&["--yes", "rollback", &first_id]);
    assert_eq!(code, 0);

    let (code, doc) = cli.json(&["rollback", "01JCX4A0M2N8P6Q4R2S0T8U6V4"]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "not_found");
}

#[test]
fn import_scans_and_adopts_identical_entries() {
    let cli = Cli::new();
    let text = "{\n  \"mcpServers\": {\n    \"docs\": {\n      \"type\": \"stdio\",\n      \"command\": \"npx\",\n      \"args\": [\"-y\", \"@example/docs-mcp\"],\n      \"env\": {\"DOCS_TOKEN\": \"${DOCS_TOKEN}\"}\n    },\n    \"chatty\": {\"type\": \"stdio\", \"command\": \"chatty\", \"env\": {\"LOG_LEVEL\": \"info\", \"CHATTY_API_KEY\": \"not-a-reference\"}}\n  }\n}\n";
    std::fs::write(cli.claude(), text).unwrap();

    let scan = cli.ok(&["import", "--scan"]);
    let names: Vec<&str> = scan["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["docs", "chatty"]);
    assert_eq!(scan["candidates"][0]["sources"][0]["importable"], true);
    assert_eq!(
        scan["candidates"][1]["sources"][0]["error"]["code"],
        "needs_secret_extraction"
    );
    assert!(!cli.home.exists(), "a scan creates nothing");

    let imported = cli.ok(&["import"]);
    assert_eq!(imported["imported"], json!(["docs"]));
    assert_eq!(imported["skipped"][0]["name"], "chatty");
    assert_eq!(imported["apply"]["written"], 0);
    assert_eq!(std::fs::read_to_string(cli.claude()).unwrap(), text);
    assert_eq!(cli.history()[0]["summary"], "import: 1 server (docs)");
    let shown = cli.ok(&["server", "show", "docs"]);
    assert_eq!(shown["server"]["env"]["DOCS_TOKEN"], "${env:DOCS_TOKEN}");
    assert_eq!(shown["server"]["enable"]["clients"], json!(["claude-code"]));

    let status = cli.ok(&["status"]);
    let entries = status["targets"][0]["entries"].as_array().unwrap();
    let docs = entries.iter().find(|e| e["name"] == "docs").unwrap();
    assert_eq!(docs["state"], "synced");
    let chatty = entries.iter().find(|e| e["name"] == "chatty").unwrap();
    assert_eq!(chatty["state"], "foreign");
}

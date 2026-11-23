//! `loadout import` (SPEC 8.6, 11.7, 14): the scan of user files and known
//! projects (JSON shape), conflict choices, skips and secret destinations,
//! against a temp `LOADOUT_HOME` and profile (`TempHome::command`: cleared
//! environment, `PATH` = the temp `bin/` with `op-stub` as `op`, the test
//! keyring file instead of the OS keyring).

use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use agent_loadout_testkit::TempHome;
use agent_loadout_testkit::op_stub::OpStub;
use serde_json::{Value, json};

/// A plaintext value planted in a client file.
const PLANTED: &str = "planted-chatty-value-7d1e0b";
const ITEM: &str = "op://Example Vault/Chatty/credential";

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
            .stdin(Stdio::null())
            .output()
            .expect("run loadout")
    }

    /// Runs with `--json`: exit code, the document and stdout as text.
    fn json(&self, args: &[&str]) -> (i32, Value, String) {
        let output = self.run(&[&["--json"], args].concat());
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let doc = serde_json::from_str(&text).unwrap_or_else(|err| {
            panic!(
                "{args:?}: stdout is not JSON ({err}):\n{text}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.code().unwrap(), doc, text)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (code, doc, _) = self.json(args);
        assert_eq!(code, 0, "{args:?}: {doc:#}");
        doc
    }

    fn claude(&self) -> PathBuf {
        self.th.home().join(".claude.json")
    }

    fn omp(&self) -> PathBuf {
        self.th.home().join(".omp").join("agent").join("mcp.json")
    }

    fn store_file(&self, relative: &str) -> String {
        std::fs::read_to_string(self.home.join("store").join(relative)).unwrap_or_default()
    }

    fn import_commits(&self) -> usize {
        self.ok(&["history"])["commits"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["summary"].as_str().unwrap().starts_with("import:"))
            .count()
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn servers(entries: &[(&str, &str)]) -> String {
    let body = entries
        .iter()
        .map(|(name, value)| format!("    \"{name}\": {value}"))
        .collect::<Vec<_>>()
        .join(",\n");
    format!("{{\n  \"mcpServers\": {{\n{body}\n  }}\n}}\n")
}

const DOCS_V1: &str = r#"{"type": "stdio", "command": "npx", "args": ["-y", "@example/docs-mcp"]}"#;
const DOCS_V2: &str =
    r#"{"type": "stdio", "command": "npx", "args": ["-y", "@example/docs-mcp", "--v2"]}"#;
const PG: &str = r#"{"type": "stdio", "command": "uvx", "args": ["postgres-mcp"]}"#;

fn chatty() -> String {
    format!(
        "{{\"type\": \"stdio\", \"command\": \"chatty-mcp\", \"env\": {{\"CHATTY_API_KEY\": \
         \"{PLANTED}\"}}}}"
    )
}

/// The user files of two clients (a conflict and a literal secret) and two
/// known projects: one initialized (trusted), one only added (untrusted).
fn setup(cli: &Cli) -> (PathBuf, String, PathBuf) {
    write(
        &cli.claude(),
        &servers(&[("docs", DOCS_V1), ("chatty", &chatty())]),
    );
    write(&cli.omp(), &servers(&[("docs", DOCS_V2)]));
    let trusted = cli.th.projects().join("alpha");
    std::fs::create_dir_all(&trusted).unwrap();
    let init = cli.ok(&["project", "init", trusted.to_str().unwrap()]);
    let trusted_id = init["project"]["id"].as_str().unwrap().to_owned();
    write(&trusted.join(".mcp.json"), &servers(&[("pg", PG)]));
    let untrusted = cli.th.projects().join("beta");
    write(
        &untrusted.join(".loadout").join("project.toml"),
        "schema = 1\nid = \"beta-c3d4\"\nname = \"beta\"\n",
    );
    cli.ok(&["project", "add", untrusted.to_str().unwrap()]);
    write(&untrusted.join(".mcp.json"), &servers(&[("pg", PG)]));
    (trusted, trusted_id, untrusted)
}

fn candidate<'a>(scan: &'a Value, name: &str) -> &'a Value {
    scan["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no {name}: {scan:#}"))
}

#[test]
fn scan_lists_projects_conflicts_and_secrets() {
    let cli = Cli::new();
    let (_, trusted_id, untrusted) = setup(&cli);
    let mcp_before = std::fs::read(untrusted.join(".mcp.json")).unwrap();

    let (code, scan, text) = cli.json(&["import", "--scan"]);
    assert_eq!(code, 0, "{scan:#}");
    assert!(!text.contains(PLANTED), "{text}");
    assert_eq!(scan["schema"], 1);

    let docs = candidate(&scan, "docs");
    assert_eq!(docs["conflict"], true);
    assert_eq!(docs["identical"], false);
    assert_eq!(docs["in_store"], false);
    assert_eq!(docs["variants"][0]["id"], "docs");
    assert_eq!(docs["variants"][0]["sources"], json!(["claude-code"]));
    assert_eq!(docs["variants"][1]["id"], "docs-omp");
    assert_eq!(docs["variants"][1]["sources"], json!(["omp"]));
    assert_eq!(docs["variants"][1]["enables"], true);
    let source = &docs["sources"][1];
    assert_eq!(source["key"], "omp");
    assert_eq!(source["id"], "omp/docs");
    assert_eq!(source["client"], "omp");
    assert_eq!(source["scope"], "user");
    assert_eq!(source["project"], Value::Null);
    assert_eq!(source["trusted"], Value::Null);
    assert_eq!(source["variant"], 1);
    assert_eq!(source["importable"], true);

    let secret = &candidate(&scan, "chatty")["variants"][0]["secrets"][0];
    assert_eq!(secret["field"], "env.CHATTY_API_KEY");
    assert_eq!(secret["alias"], "CHATTY_CHATTY_API_KEY");
    assert_eq!(secret["reason"], "sensitive_key");
    assert_eq!(
        secret["suggested_ref"],
        "keyring://loadout/CHATTY_CHATTY_API_KEY"
    );

    let pg = candidate(&scan, "pg");
    assert_eq!(pg["conflict"], false);
    assert_eq!(pg["variants"][0]["enables"], false);
    let claude: Vec<(String, Value, Value)> = pg["sources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["client"] == "claude-code")
        .map(|s| {
            (
                s["key"].as_str().unwrap().to_owned(),
                s["scope"].clone(),
                s["trusted"].clone(),
            )
        })
        .collect();
    assert_eq!(
        claude,
        [
            (
                format!("claude-code@{trusted_id}"),
                json!("project"),
                json!(true)
            ),
            (
                "claude-code@beta-c3d4".to_owned(),
                json!("project"),
                json!(false)
            ),
        ]
    );

    let user = cli.ok(&["import", "--scan", "--scope", "system"]);
    assert!(
        user["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["name"] != "pg")
    );
    let beta = cli.ok(&["import", "--scan", "--project", "beta-c3d4"]);
    let names: Vec<&str> = beta["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["pg"]);
    assert_eq!(
        std::fs::read(untrusted.join(".mcp.json")).unwrap(),
        mcp_before
    );
}

/// `--pick` and `--as` import both definitions of a conflict, `--skip`
/// leaves one out, and `--extract` sends a literal secret to 1Password:
/// one commit, no value in the store or the output, and the client entry
/// rewritten to the reference.
#[test]
fn choices_import_both_definitions_and_extract_to_onepassword() {
    let cli = Cli::new();
    let mut stub = OpStub::install(&cli.th.bin());
    stub.add_item(ITEM, PLANTED);
    let (_, _, untrusted) = setup(&cli);
    let omp_before = std::fs::read_to_string(cli.omp()).unwrap();
    let mcp_before = std::fs::read(untrusted.join(".mcp.json")).unwrap();

    // A conflict without a choice is skipped.
    let (code, doc, _) = cli.json(&["import", "--skip", "chatty", "--skip", "pg", "docs"]);
    assert_eq!(code, 0, "{doc:#}");
    assert_eq!(doc["imported"], json!([]));
    assert_eq!(doc["skipped"][0]["name"], "docs");
    assert_eq!(doc["skipped"][0]["error"]["code"], "collision");

    let (code, doc, text) = cli.json(&[
        "import",
        "--pick",
        "docs=claude-code",
        "--as",
        "docs=omp",
        "--skip",
        "pg",
        "--extract",
        &format!("chatty={ITEM}"),
    ]);
    assert!(!text.contains(PLANTED), "{text}");
    assert!(code == 0 || code == 5, "{doc:#}");
    // Defaults first, then the explicit choices.
    assert_eq!(doc["imported"], json!(["chatty", "docs", "docs-omp"]));
    assert_eq!(doc["skipped"], json!([]));
    assert_eq!(
        doc["aliases"],
        json!([{
            "alias": "CHATTY_CHATTY_API_KEY",
            "server": "chatty",
            "field": "env.CHATTY_API_KEY",
            "destination": "onepassword",
        }])
    );
    assert_eq!(cli.import_commits(), 1);
    let renamed = cli.store_file("servers/docs-omp.toml");
    assert!(renamed.contains("native_name = \"docs\""), "{renamed}");
    let secrets = cli.store_file("secrets.toml");
    assert!(secrets.contains(ITEM), "{secrets}");
    for file in ["servers/chatty.toml", "secrets.toml", "servers/docs.toml"] {
        assert!(!cli.store_file(file).contains(PLANTED), "{file}");
    }
    // Identical entries are adopted without a write; the entry with the
    // literal is rewritten to the reference (launch delivery).
    assert_eq!(std::fs::read_to_string(cli.omp()).unwrap(), omp_before);
    let claude = std::fs::read_to_string(cli.claude()).unwrap();
    assert!(!claude.contains(PLANTED), "{claude}");
    assert!(claude.contains(ITEM), "{claude}");
    assert_eq!(
        std::fs::read(untrusted.join(".mcp.json")).unwrap(),
        mcp_before
    );

    // A malformed destination is refused before anything changes.
    let (code, doc, _) = cli.json(&["import", "--extract", "pg=op://two/parts"]);
    assert_eq!(code, 1, "{doc:#}");
    assert_eq!(doc["error"]["code"], "invalid_value");
}

/// Review follow-up 3: `--pick NAME=KEY` selects a client's project-file
/// definition when the client has a user-file one too; `--as` names the
/// other after the client.
#[test]
fn a_source_key_picks_a_project_definition() {
    let cli = Cli::new();
    write(&cli.claude(), &servers(&[("docs", DOCS_V1)]));
    let root = cli.th.projects().join("alpha");
    std::fs::create_dir_all(&root).unwrap();
    let init = cli.ok(&["project", "init", root.to_str().unwrap()]);
    let id = init["project"]["id"].as_str().unwrap().to_owned();
    write(&root.join(".mcp.json"), &servers(&[("docs", DOCS_V2)]));
    let project_before = std::fs::read(root.join(".mcp.json")).unwrap();

    let doc = cli.ok(&[
        "import",
        "--pick",
        &format!("docs=claude-code@{id}"),
        "--as",
        "docs=claude-code",
    ]);
    assert_eq!(
        doc["imported"],
        json!(["docs", "docs-claude-code"]),
        "{doc:#}"
    );
    assert_eq!(doc["definition_only"], json!(["docs"]));
    assert!(cli.store_file("servers/docs.toml").contains("--v2"));
    let renamed = cli.store_file("servers/docs-claude-code.toml");
    assert!(renamed.contains("native_name = \"docs\""), "{renamed}");
    assert_eq!(
        std::fs::read(root.join(".mcp.json")).unwrap(),
        project_before
    );
}

/// `--extract NAME:FIELD=keyring` writes the value from the client file to
/// the keyring; the store holds the reference only. A project-only entry is
/// a definition without enablement.
#[test]
fn keyring_extraction_and_project_definitions() {
    let cli = Cli::new();
    setup(&cli);
    let (code, doc, text) = cli.json(&[
        "import",
        "--skip",
        "docs",
        "--extract",
        "chatty:env.CHATTY_API_KEY=keyring",
    ]);
    assert!(!text.contains(PLANTED), "{text}");
    assert!(code == 0 || code == 5, "{doc:#}");
    assert_eq!(doc["imported"], json!(["chatty", "pg"]));
    assert_eq!(doc["definition_only"], json!(["pg"]));
    assert_eq!(doc["aliases"][0]["destination"], "keyring");
    let keyring = std::fs::read_to_string(cli.th.keyring_file()).unwrap();
    let keyring: Value = serde_json::from_str(&keyring).unwrap();
    assert_eq!(keyring["loadout/CHATTY_CHATTY_API_KEY"], PLANTED);
    let server = cli.store_file("servers/chatty.toml");
    assert!(
        server.contains("CHATTY_API_KEY = \"${secret:CHATTY_CHATTY_API_KEY}\""),
        "{server}"
    );
    assert!(!cli.store_file("servers/pg.toml").contains("[enable]"));
    assert!(
        cli.store_file("secrets.toml")
            .contains("keyring://loadout/CHATTY_CHATTY_API_KEY")
    );
}

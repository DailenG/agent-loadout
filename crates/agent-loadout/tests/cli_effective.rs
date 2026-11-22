//! `loadout effective [PATH] [--client C]` (SPEC 6.4, 14) against a temp
//! `LOADOUT_HOME` and profile: a trusted project override shadows the
//! system entry.

use std::path::PathBuf;
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

    /// Runs with `--json`, expects exit code 0 and returns the document.
    fn ok(&self, args: &[&str]) -> Value {
        let output = self.run(&[&["--json"], args].concat());
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
            panic!(
                "{args:?}: stdout is not JSON ({err}):\n{}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }
}

#[test]
fn a_trusted_project_override_shadows_the_system_entry() {
    let cli = Cli::new();
    cli.ok(&[
        "server",
        "add",
        "github",
        "--client",
        "claude-code",
        "--",
        "github-mcp",
    ]);
    let root = cli.th.projects().join("app");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.to_str().unwrap();
    let init = cli.ok(&["project", "init", path, "--shared"]);
    let id = init["project"]["id"].as_str().unwrap().to_owned();
    // An override of the system server, trusted and applied.
    let manifest = root.join(".loadout").join("project.toml");
    let mut text = std::fs::read_to_string(&manifest).unwrap();
    text.push_str(
        "\n[servers.github]\nfrom = \"github\"\nargs = [\"--project\"]\nclients = [\"claude-code\"]\n",
    );
    std::fs::write(&manifest, text).unwrap();
    cli.ok(&["project", "trust", path]);

    let doc = cli.ok(&["effective", path, "--client", "claude-code"]);
    assert_eq!(doc["schema"], 1);
    assert_eq!(doc["project"]["id"], id.as_str());
    assert_eq!(doc["project"]["root"], path);
    let entries = doc["clients"]["claude-code"].as_array().unwrap();
    let github = entries
        .iter()
        .find(|entry| entry["name"] == "github")
        .unwrap_or_else(|| panic!("{doc:#}"));
    assert_eq!(github["loaded"], true, "{doc:#}");
    assert_eq!(github["winner"]["layer"], "project", "{doc:#}");
    assert_eq!(github["winner"]["file"], "<P>/.mcp.json");
    assert_eq!(github["winner"]["source"], "store:github+project");
    assert_eq!(github["status"], "synced");
    assert_eq!(
        github["shadowed"],
        serde_json::json!([{ "layer": "system", "file": "~/.claude.json" }]),
        "{doc:#}"
    );
    assert_eq!(github["warnings"], serde_json::json!([]));

    // The table names the same winner.
    let output = cli.run(&["effective", path, "--client", "claude-code"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let table = String::from_utf8_lossy(&output.stdout);
    assert!(table.contains(&format!("Project {id}")), "{table}");
    let line = table
        .lines()
        .find(|line| line.trim_start().starts_with("github"))
        .unwrap_or_else(|| panic!("{table}"));
    assert!(line.contains("project <P>/.mcp.json"), "{table}");
    assert!(line.contains("system"), "{table}");

    // An unknown client is an error.
    let output = cli.run(&["--json", "effective", path, "--client", "nope"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
}

//! `loadout migrate skilldock [--apply]` (SPEC 15.1, 14) against a temp
//! `LOADOUT_HOME` and a temp profile holding the synthetic
//! `fixtures/skilldock-home`: the dry run writes nothing, `--apply` needs a
//! confirmation, `--apply --yes` migrates in one commit and leaves
//! `~/.skilldock` and the client files as they were.

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
        copy_dir(&fixture(), &th.home());
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
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("skilldock-home")
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

/// Every file below `dir` with its bytes, sorted.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push((path.clone(), std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

#[test]
fn migrate_dry_run_confirmation_and_apply() {
    let cli = Cli::new();
    let before = snapshot(&cli.th.home());

    // The dry run: a report, nothing written.
    let (code, doc) = cli.json(&["migrate", "skilldock"]);
    assert_eq!(code, 0, "{doc:#}");
    assert_eq!(doc["dry_run"], true);
    let items = doc["report"]["items"].as_array().unwrap();
    assert!(items.iter().any(|item| item["name"] == "context7"
        && item["kind"] == "server"
        && item["status"] == "migrated"));
    assert!(
        items.iter().any(|item| item["name"] == "codex/context7"
            && item["offers"][0]["kind"] == "replace_wrapper")
    );
    assert_eq!(doc["report"]["extractions"].as_array().unwrap().len(), 4);
    assert!(!cli.home.exists(), "the dry run created the data directory");
    let text = cli.run(&["migrate", "skilldock"]);
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(stdout.contains("Dry run: nothing was written."), "{stdout}");
    assert!(!stdout.contains("fixture-env-value"), "{stdout}");

    // `--apply` without `--yes` and without a terminal: no confirmation.
    let (code, doc) = cli.json(&["migrate", "skilldock", "--apply"]);
    assert_eq!(code, 4, "{doc:#}");
    assert_eq!(doc["error"]["code"], "confirmation_required", "{doc:#}");
    assert!(!cli.home.join("store").exists());

    // Confirmed: one commit after the pre-migrate tag.
    let (code, doc) = cli.json(&["--yes", "migrate", "skilldock", "--apply"]);
    assert_eq!(code, 0, "{doc:#}");
    assert_eq!(doc["dry_run"], false);
    assert!(
        doc["tag"]
            .as_str()
            .unwrap()
            .starts_with("auto/pre-migrate/")
    );
    assert!(doc["commit"].is_string());
    assert_eq!(
        doc["report"]["servers"],
        serde_json::json!([
            "context7",
            "fetch",
            "remote-docs",
            "notes",
            "time",
            "docs-search",
            "linear"
        ])
    );
    assert_eq!(
        doc["report"]["skills"],
        serde_json::json!([
            "gh-tree-skill",
            "market-helper",
            "repo-notes",
            "team-review"
        ])
    );
    assert!(
        cli.home
            .join("store")
            .join("servers")
            .join("notes.toml")
            .exists()
    );
    let (code, history) = cli.json(&["history"]);
    assert_eq!(code, 0, "{history:#}");
    let subjects: Vec<&str> = history["commits"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["summary"].as_str())
        .collect();
    assert_eq!(
        subjects[0], "migrate: skilldock (7 servers, 4 skills)",
        "{history:#}"
    );

    // ~/.skilldock and every client file are as they were.
    assert_eq!(snapshot(&cli.th.home()), before);
}

//! `loadout skills ...` (SPEC 12, 14) against a temp `LOADOUT_HOME` and a
//! temp profile: add from a folder, enable (a junction on Windows), list,
//! scan, a foreign folder (exit 4), update --check, disable, rm.

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
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("skills")
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
fn skills_add_enable_scan_disable_rm() {
    let cli = Cli::new();
    let src = cli.th.root().join("src").join("repo-conventions");
    copy_dir(&fixture("repo-conventions"), &src);
    let src = src.to_str().unwrap();

    let doc = cli.ok(&["skills", "add", src]);
    assert_eq!(doc["skill"]["id"], "repo-conventions");
    assert!(doc["commit"].is_string());
    // The same name again needs --yes, then gets a suffix.
    let (code, doc) = cli.json(&["skills", "add", src]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "already_exists");
    let doc = cli.ok(&["--yes", "skills", "add", src]);
    assert_eq!(doc["skill"]["id"], "repo-conventions-2");
    assert_eq!(doc["skill"]["renamed_from"], "repo-conventions");
    cli.ok(&["skills", "rm", "repo-conventions-2"]);

    let doc = cli.ok(&[
        "skills",
        "enable",
        "repo-conventions",
        "--client",
        "claude-code",
    ]);
    let result = &doc["links"]["results"][0];
    assert_eq!(result["status"], "created");
    assert_eq!(result["client"], "claude-code");
    let method = if cfg!(windows) { "junction" } else { "symlink" };
    assert_eq!(result["method"], method);
    let link = cli
        .th
        .home()
        .join(".claude")
        .join("skills")
        .join("repo-conventions");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(link.join("SKILL.md").is_file());

    let doc = cli.ok(&["skills", "ls"]);
    let skill = &doc["skills"][0];
    assert_eq!(skill["id"], "repo-conventions");
    assert_eq!(skill["frontmatter"]["license"], "MIT");
    assert_eq!(
        skill["enable"]["clients"],
        serde_json::json!(["claude-code"])
    );
    assert_eq!(skill["links"][0]["method"], method);
    assert_eq!(skill["source"]["kind"], "local");

    // A real folder for Codex: never overwritten, exit 4.
    let foreign = cli
        .th
        .home()
        .join(".codex")
        .join("skills")
        .join("repo-conventions");
    std::fs::create_dir_all(&foreign).unwrap();
    std::fs::write(foreign.join("SKILL.md"), "mine\n").unwrap();
    let (code, doc) = cli.json(&["skills", "enable", "repo-conventions", "--client", "codex"]);
    assert_eq!(code, 4, "{doc:#}");
    let statuses: Vec<&str> = doc["links"]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| result["status"].as_str().unwrap())
        .collect();
    assert!(statuses.contains(&"foreign"), "{doc:#}");
    assert_eq!(
        std::fs::read_to_string(foreign.join("SKILL.md")).unwrap(),
        "mine\n"
    );

    let doc = cli.ok(&["skills", "scan"]);
    let dirs = doc["dirs"].as_array().unwrap();
    let class = |client: &str| {
        dirs.iter().find(|dir| dir["client"] == client).unwrap()["entries"][0]["class"].clone()
    };
    assert_eq!(class("claude-code"), "managed");
    assert_eq!(class("codex"), "unmanaged");

    // A local skill has no source to update from.
    let doc = cli.ok(&["skills", "update", "--check"]);
    assert_eq!(doc["skills"][0]["status"], "not_tracked");

    // Disable everywhere and remove: the link goes, the foreign folder
    // stays.
    let doc = cli.ok(&["skills", "disable", "repo-conventions"]);
    let results = doc["links"]["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{doc:#}");
    assert_eq!(results[0]["status"], "removed");
    assert_eq!(results[0]["client"], "claude-code");
    assert!(std::fs::symlink_metadata(&link).is_err());
    cli.ok(&["skills", "rm", "repo-conventions"]);
    let doc = cli.ok(&["skills", "ls"]);
    assert_eq!(doc["skills"], serde_json::json!([]));
    assert!(foreign.join("SKILL.md").is_file());

    // The history holds one commit per change.
    let doc = cli.ok(&["history"]);
    let subjects: Vec<&str> = doc["commits"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|commit| commit["summary"].as_str())
        .collect();
    assert!(subjects.contains(&"skill: enable repo-conventions for claude-code"));
    assert!(subjects.contains(&"skill: remove repo-conventions"));
}

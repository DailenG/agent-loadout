//! `loadout show|tag|restore|backup` and `loadout diff --rev` (SPEC 9.3 to
//! 9.5, 14) against a temp `LOADOUT_HOME` and profile.

use std::path::PathBuf;
use std::process::{Output, Stdio};

use agent_loadout_testkit::TempHome;
use serde_json::{Value, json};

struct Cli {
    th: TempHome,
    home: PathBuf,
}

impl Cli {
    fn new() -> Self {
        Self::with(TempHome::new())
    }

    fn with(th: TempHome) -> Self {
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

    fn store_file(&self, relative: &str) -> PathBuf {
        self.home.join("store").join(relative)
    }

    fn head(&self) -> String {
        self.ok(&["history", "--limit", "1"])["commits"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// `server add notes` for Claude Code with `args`; the commit id.
    fn add(&self, id: &str, extra: &[&str]) -> String {
        let doc = self.ok(&[&["server", "add", id, "--client", "claude-code"][..], extra].concat());
        doc["commit"].as_str().unwrap().to_owned()
    }

    fn claude(&self) -> String {
        std::fs::read_to_string(self.th.home().join(".claude.json")).unwrap_or_default()
    }
}

#[test]
fn show_and_diff_rev_report_semantic_changes() {
    let cli = Cli::new();
    let a = cli.add(
        "notes",
        &[
            "--env",
            "LOG_LEVEL=info",
            "--",
            "npx",
            "-y",
            "@example/notes-mcp",
        ],
    );
    let path = cli.store_file("servers/notes.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.replace("LOG_LEVEL = \"info\"", "LOG_LEVEL = \"debug\""),
    )
    .unwrap();
    let backup = cli.ok(&["backup", "-m", "notes: louder logs"]);
    let b = backup["commit"].as_str().unwrap().to_owned();
    assert_eq!(backup["files"], json!(["servers/notes.toml"]));

    let shown = cli.ok(&["show", &b[..10]]);
    assert_eq!(shown["commit"]["summary"], "notes: louder logs");
    assert_eq!(shown["commit"]["op"], "store.backup");
    assert_eq!(
        shown["changes"],
        json!([{
            "entity": "server/notes",
            "file": "servers/notes.toml",
            "change": "changed",
            "fields": [{"field": "env.LOG_LEVEL", "before": "\"info\"", "after": "\"debug\""}],
            "text": null,
        }])
    );
    let text = cli.run(&["show", &b]);
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(
        text.contains("env.LOG_LEVEL: \"info\" -> \"debug\""),
        "{text}"
    );

    let range = format!("{a}..{b}");
    let output = cli.run(&["diff", "--rev", &range, "--exit-code"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stdout).contains("server/notes: changed"));
    let raw = cli.ok(&["diff", "--rev", &format!("{a}.."), "--raw"]);
    assert!(
        raw["diff"]
            .as_str()
            .unwrap()
            .contains("+LOG_LEVEL = \"debug\"")
    );
    let output = cli.run(&["diff", "--rev", &format!("{b}..{b}"), "--exit-code"]);
    assert_eq!(output.status.code(), Some(0));
    let (code, doc) = cli.json(&["diff", "--rev", "nope..", "--exit-code"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("not_found"))
    );
}

#[test]
fn backup_commits_hand_edits_once_and_scans_them() {
    let cli = Cli::new();
    cli.add("notes", &["--", "npx", "-y", "@example/notes-mcp"]);
    let clean = cli.ok(&["backup"]);
    assert_eq!(clean["commit"], Value::Null);
    assert_eq!(clean["files"], json!([]));

    // A token-shaped value in a hand edit is refused like any commit.
    let token = ["gh", "p_", "FAKE0000000000000000000000000000000000"].concat();
    let path = cli.store_file("servers/notes.toml");
    let original = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        format!("{original}\n[env]\nNOTES_TOKEN = \"{token}\"\n"),
    )
    .unwrap();
    let (code, doc) = cli.json(&["backup"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("leak_detected"))
    );
    assert!(!doc.to_string().contains(&token));
    let history = cli.ok(&["history"])["commits"].as_array().unwrap().len();

    // An invalid file is refused too; a valid edit commits, tagged.
    std::fs::write(&path, "schema = 1\nid = \"notes\"\n").unwrap();
    let (code, doc) = cli.json(&["backup"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("missing_field"))
    );
    std::fs::write(&path, original.replace("notes-mcp", "notes-mcp@2")).unwrap();
    let done = cli.ok(&["backup", "--tag", "after-edit"]);
    assert!(done["commit"].is_string());
    assert_eq!(done["tag"]["name"], "after-edit");
    assert_eq!(done["tag"]["commit"], done["commit"]);
    assert_eq!(
        cli.ok(&["history"])["commits"].as_array().unwrap().len(),
        history + 1
    );
}

#[test]
fn tags_list_create_and_delete() {
    let cli = Cli::new();
    let a = cli.add("notes", &["--", "npx", "-y", "@example/notes-mcp"]);
    cli.add("search", &["--", "npx", "-y", "@example/search-mcp"]);
    let tagged = cli.ok(&["tag", "pre-reset", &a, "-m", "before the reset"]);
    assert_eq!(tagged["tag"]["commit"], a);
    assert_eq!(tagged["tag"]["kind"], "user");
    assert_eq!(tagged["tag"]["message"], "before the reset");
    let (code, doc) = cli.json(&["tag", "pre-reset"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("already_exists"))
    );
    let (code, doc) = cli.json(&["tag", "bad name"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("invalid_value"))
    );
    let listed = cli.ok(&["tag", "--list"]);
    assert_eq!(listed["tags"][0]["name"], "pre-reset");
    assert_eq!(cli.ok(&["show", "pre-reset"])["commit"]["id"], a);
    cli.ok(&["tag", "--delete", "pre-reset"]);
    assert_eq!(cli.ok(&["tag", "--list"])["tags"], json!([]));
}

#[test]
fn restore_rev_path_and_apply() {
    let cli = Cli::new();
    let a = cli.add("notes", &["--", "npx", "-y", "@example/notes-mcp"]);
    cli.add("search", &["--", "npx", "-y", "@example/search-mcp"]);
    cli.ok(&["server", "rm", "notes"]);
    assert!(!cli.claude().contains("\"notes\""));

    // Only server/notes, without applying: the store has it back, client
    // files wait for apply.
    let restored = cli.ok(&["restore", &a, "--path", "server/notes"]);
    assert_eq!(restored["files"], json!(["servers/notes.toml"]));
    assert!(
        restored["tag"]
            .as_str()
            .unwrap()
            .starts_with("auto/pre-restore/")
    );
    assert_eq!(restored["apply"], Value::Null);
    assert!(cli.store_file("servers/search.toml").exists());
    assert!(!cli.claude().contains("\"notes\""));
    let output = cli.run(&["restore", &a, "--path", "server/notes"]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Nothing to restore"));

    // The whole store, applied in the same commit.
    let head = cli.head();
    let restored = cli.ok(&["restore", &a, "--apply"]);
    assert_eq!(restored["files"], json!(["servers/search.toml"]));
    assert_eq!(restored["apply"]["trigger"], "restore");
    assert!(!cli.claude().contains("\"search\""));
    let tag = restored["tag"].as_str().unwrap().to_owned();
    assert_eq!(cli.ok(&["show", &tag])["commit"]["id"], head);
    // Undo: restore the tag.
    cli.ok(&["restore", &tag, "--apply"]);
    assert!(cli.claude().contains("\"search\""));

    let (code, doc) = cli.json(&["restore", "nope"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("not_found"))
    );
}

#[test]
fn restore_writes_shared_manifests_back_only_with_yes() {
    let cli = Cli::new();
    cli.add("notes", &["--", "npx", "-y", "@example/notes-mcp"]);
    let machine = cli.ok(&["doctor", "--paths"])["paths"]["state"]
        .as_str()
        .map(|state| PathBuf::from(state).join("machine.toml"))
        .unwrap();
    let machine = std::fs::read_to_string(machine).unwrap();
    let id = machine
        .lines()
        .find_map(|line| line.strip_prefix("machine_id = "))
        .unwrap()
        .trim_matches('"')
        .to_owned();
    let project = cli.th.projects().join("app");
    let shared = project.join(".loadout").join("project.toml");
    std::fs::create_dir_all(shared.parent().unwrap()).unwrap();
    let manifest = |name: &str| format!("schema = 1\nid = \"app-7k2f\"\nname = \"{name}\"\n");
    let os = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    std::fs::create_dir_all(cli.store_file("projects/app-7k2f")).unwrap();
    std::fs::write(
        cli.store_file(&format!("machines/{id}.toml")),
        format!(
            "schema = 1\nmachine_id = \"{id}\"\nlabel = \"desk\"\nos = \"{os}\"\n\n[projects.app-7k2f]\npath = {}\n",
            serde_json::to_string(&project.to_string_lossy()).unwrap()
        ),
    )
    .unwrap();
    std::fs::write(
        cli.store_file("projects/app-7k2f/project.toml"),
        manifest("App"),
    )
    .unwrap();
    let a = cli.ok(&["backup"])["commit"].as_str().unwrap().to_owned();
    std::fs::write(
        cli.store_file("projects/app-7k2f/project.toml"),
        manifest("Renamed"),
    )
    .unwrap();
    cli.ok(&["backup"]);
    std::fs::write(&shared, manifest("Renamed")).unwrap();

    let head = cli.head();
    let (code, doc) = cli.json(&["restore", &a]);
    assert_eq!(code, 4, "{doc:#}");
    assert_eq!(doc["error"]["code"], "confirmation_required");
    assert_eq!(cli.head(), head);
    assert_eq!(
        std::fs::read_to_string(&shared).unwrap(),
        manifest("Renamed")
    );

    let restored = cli.ok(&["--yes", "restore", &a]);
    assert_eq!(restored["manifests"].as_array().unwrap().len(), 1);
    assert_eq!(std::fs::read_to_string(&shared).unwrap(), manifest("App"));
}

#[test]
fn restore_files_brings_back_client_bytes() {
    let cli = Cli::new();
    let before = "{\n  \"theme\": \"dark\"\n}\n";
    std::fs::write(cli.th.home().join(".claude.json"), before).unwrap();
    let added = cli.ok(&[
        "server",
        "add",
        "notes",
        "--client",
        "claude-code",
        "--",
        "npx",
    ]);
    let apply_id = added["apply"]["apply_id"].as_str().unwrap().to_owned();
    assert!(cli.claude().contains("\"notes\""));

    let restored = cli.ok(&["restore", "--files", &apply_id]);
    assert_eq!(restored["files"][0]["outcome"], "restored");
    assert_eq!(cli.claude(), before);
    let again = cli.ok(&["restore", "--files", &apply_id]);
    assert_eq!(again["files"][0]["outcome"], "unchanged");
    let other = cli.th.home().join("nope.json");
    let (code, doc) = cli.json(&[
        "restore",
        "--files",
        &apply_id,
        "--file",
        other.to_str().unwrap(),
    ]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("not_found"))
    );
    // The next apply re-creates the entry.
    cli.ok(&["apply"]);
    assert!(cli.claude().contains("\"notes\""));
}

/// SPEC 16.1: a backup copy is named after the whole client path, so a deep
/// profile pushes it past `MAX_PATH`; apply and restore still work.
#[cfg(windows)]
#[test]
fn restore_files_works_past_max_path() {
    let outer = TempHome::new();
    let deep = outer
        .root()
        .join("a-deep-folder-that-stands-in-for-a-long-profile")
        .join("and-another-one-so-the-backup-name-gets-long");
    std::fs::create_dir_all(&deep).unwrap();
    let cli = Cli::with(TempHome::new_in(&deep));
    let before = "{\n  \"theme\": \"dark\"\n}\n";
    std::fs::write(cli.th.home().join(".claude.json"), before).unwrap();
    let added = cli.ok(&[
        "server",
        "add",
        "notes",
        "--client",
        "claude-code",
        "--",
        "npx",
    ]);
    let apply_id = added["apply"]["apply_id"].as_str().unwrap().to_owned();
    assert!(cli.claude().contains("\"notes\""));
    let runs = cli.home.join("backups").join("files");
    let backups: Vec<PathBuf> = std::fs::read_dir(&runs)
        .unwrap()
        .flat_map(|run| std::fs::read_dir(run.unwrap().path()).unwrap())
        .map(|file| file.unwrap().path())
        .collect();
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert!(
        backups[0].as_os_str().len() > 260,
        "the backup path must pass MAX_PATH: {}",
        backups[0].display()
    );

    let restored = cli.ok(&["restore", "--files", &apply_id]);
    assert_eq!(restored["files"][0]["outcome"], "restored", "{restored:#}");
    assert_eq!(cli.claude(), before);
    // Paths shown to the user never carry the `\\?\` prefix (JSON-escaped).
    for doc in [&added, &restored] {
        assert!(!doc.to_string().contains(r"\\\\?\\"), "{doc:#}");
    }
}

//! `loadout remote ...`, `backup` pushes and `restore --from` (SPEC 9.6 to
//! 9.9, 14) with several `--home` data directories sharing one temp profile
//! and one folder remote.

use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use agent_loadout_testkit::TempHome;
use serde_json::Value;

struct Cli {
    th: TempHome,
    remote: PathBuf,
}

impl Cli {
    fn new() -> Self {
        let th = TempHome::new();
        let remote = th.root().join("remote");
        std::fs::create_dir_all(&remote).unwrap();
        Self { th, remote }
    }

    fn home(&self, name: &str) -> PathBuf {
        self.th.root().join(name)
    }

    fn run(&self, home: &str, args: &[&str]) -> Output {
        self.th
            .command(env!("CARGO_BIN_EXE_loadout"))
            .arg("--home")
            .arg(self.home(home))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run loadout")
    }

    fn json(&self, home: &str, args: &[&str]) -> (i32, Value) {
        let output = self.run(home, &[&["--json"], args].concat());
        let doc = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
            panic!(
                "{args:?}: stdout is not JSON ({err}):\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.code().unwrap(), doc)
    }

    fn ok(&self, home: &str, args: &[&str]) -> Value {
        let (code, doc) = self.json(home, args);
        assert_eq!(code, 0, "{home} {args:?}: {doc:#}");
        doc
    }

    fn add_server(&self, home: &str, id: &str, command: &str) {
        self.ok(
            home,
            &[
                "server",
                "add",
                id,
                "--client",
                "claude-code",
                "--",
                command,
            ],
        );
    }

    fn add_remote(&self, home: &str, name: &str, path: &Path, extra: &[&str]) -> Value {
        let path = path.to_string_lossy();
        self.ok(
            home,
            &[&["remote", "add", "folder", name, &path][..], extra].concat(),
        )
    }

    fn has_server(&self, home: &str, id: &str) -> bool {
        self.home(home)
            .join("store")
            .join("servers")
            .join(format!("{id}.toml"))
            .exists()
    }

    fn head(&self, home: &str) -> String {
        self.ok(home, &["history", "--limit", "1"])["commits"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

#[test]
fn no_remote_until_added_and_unreachable_ones_only_warn() {
    let cli = Cli::new();
    cli.add_server("a", "notes", "notes-mcp");
    assert_eq!(
        cli.ok("a", &["remote", "ls"])["remotes"],
        Value::Array(vec![])
    );
    let text = cli.run("a", &["remote", "ls"]);
    assert!(String::from_utf8_lossy(&text.stdout).contains("No backup remote."));
    let (code, doc) = cli.json("a", &["remote", "push"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("not_found"))
    );
    // A plain backup does not need a remote.
    assert_eq!(cli.ok("a", &["backup"])["pushes"], Value::Array(vec![]));

    let usb = cli.th.root().join("usb");
    let added = cli.add_remote("a", "usb", &usb, &[]);
    assert_eq!(added["reachable"], false);
    assert!(!usb.exists(), "the remote folder is never created");
    let backup = cli.ok("a", &["backup"]);
    assert_eq!(backup["pushes"][0]["ok"], false);
    assert_eq!(backup["pushes"][0]["error"]["code"], "remote_unreachable");
    let output = cli.run("a", &["backup"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stderr).contains("warning[remote_unreachable]: usb"));
    let (code, doc) = cli.json("a", &["remote", "push"]);
    assert_eq!(code, 1, "{doc:#}");
    // Removing it leaves nothing behind.
    cli.ok("a", &["remote", "rm", "usb"]);
    assert_eq!(
        cli.ok("a", &["remote", "ls"])["remotes"],
        Value::Array(vec![])
    );
    // Inside the data directory is refused.
    let inside = cli.home("a").join("remote");
    let (code, doc) = cli.json(
        "a",
        &["remote", "add", "folder", "in", &inside.to_string_lossy()],
    );
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("invalid_path"))
    );
}

#[test]
fn machines_share_a_folder_remote_and_restore_from_it() {
    let cli = Cli::new();
    cli.add_server("a", "notes", "notes-mcp");
    cli.add_remote("a", "share", &cli.remote, &[]);
    let backup = cli.ok("a", &["backup", "--tag", "pre-reset"]);
    assert_eq!(backup["pushes"][0]["ok"], true, "{backup:#}");
    assert_eq!(backup["pushes"][0]["mode"], "repo-per-machine");

    // B starts from the folder: one machine, so no --machine needed.
    let restored = cli.ok("b", &["restore", "--from", &cli.remote.to_string_lossy()]);
    assert_eq!(restored["created"], true);
    assert!(cli.has_server("b", "notes"));
    assert_eq!(cli.head("b"), cli.head("a"));
    assert!(
        cli.ok("b", &["tag", "--list"])["tags"]
            .to_string()
            .contains("pre-reset")
    );

    // B changes something and pushes; A pulls it.
    cli.add_server("b", "search", "search-mcp");
    cli.add_remote("b", "share", &cli.remote, &[]);
    let pushed = cli.ok("b", &["remote", "push"]);
    assert_eq!(pushed["pushes"][0]["ok"], true);
    let pulled = cli.ok("a", &["remote", "pull"]);
    let pull = &pulled["pulls"][0];
    assert_eq!(pull["machines"][0]["outcome"], "fast_forward", "{pulled:#}");
    assert_eq!(pull["changed_servers"], 1);
    assert!(pull["tag"].as_str().unwrap().starts_with("auto/pre-pull/"));
    assert!(cli.has_server("a", "search"));
    let text = cli.run("a", &["remote", "pull"]);
    assert!(String::from_utf8_lossy(&text.stdout).contains("up to date"));

    // Two machines now: restore lists them and wants a pick.
    let listed = cli.ok("a", &["remote", "machines", "share"]);
    let machines = listed["machines"].as_array().unwrap();
    assert_eq!(machines.len(), 2);
    let (code, doc) = cli.json("c", &["restore", "--from", &cli.remote.to_string_lossy()]);
    assert_eq!(code, 1);
    assert_eq!(doc["machines"].as_array().unwrap().len(), 2);
    assert!(!cli.home("c").join("store").exists(), "nothing restored");
    let pick = machines
        .iter()
        .find(|m| m["head"] == cli.head("b"))
        .unwrap()["machine_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let restored = cli.ok(
        "c",
        &[
            "restore",
            "--from",
            &cli.remote.to_string_lossy(),
            "--machine",
            &pick,
        ],
    );
    assert_eq!(restored["created"], true);
    assert!(cli.has_server("c", "search"));
}

#[test]
fn pull_conflicts_exit_4_and_leave_head_alone() {
    let cli = Cli::new();
    cli.add_server("a", "notes", "notes-mcp");
    cli.add_remote("a", "share", &cli.remote, &[]);
    cli.ok("a", &["remote", "push"]);
    cli.ok("b", &["restore", "--from", &cli.remote.to_string_lossy()]);
    cli.add_remote("b", "share", &cli.remote, &[]);
    for (home, command) in [("a", "from-a"), ("b", "from-b")] {
        let path = cli
            .home(home)
            .join("store")
            .join("servers")
            .join("notes.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("notes-mcp", command)).unwrap();
        cli.ok(home, &["backup", "--no-push"]);
    }
    cli.ok("b", &["remote", "push"]);
    let head = cli.head("a");
    let (code, doc) = cli.json("a", &["remote", "pull"]);
    assert_eq!(code, 4, "{doc:#}");
    assert_eq!(doc["pulls"][0]["conflicts"], 1);
    assert_eq!(
        doc["pulls"][0]["machines"][0]["detail"][0]["field"],
        "command"
    );
    assert_eq!(cli.head("a"), head);
    assert!(
        cli.home("a")
            .join("state")
            .join("merge")
            .join("MERGE_STATE.json")
            .exists()
    );
}

#[test]
fn restore_from_mirror_and_zip_exports() {
    let cli = Cli::new();
    cli.add_server("a", "notes", "notes-mcp");
    let mirror = cli.th.root().join("mirror");
    let zips = cli.th.root().join("zips");
    std::fs::create_dir_all(&mirror).unwrap();
    std::fs::create_dir_all(&zips).unwrap();
    cli.add_remote("a", "mirror", &mirror, &["--mode", "mirror"]);
    cli.add_remote("a", "zips", &zips, &["--mode", "archive", "--keep", "3"]);
    let backup = cli.ok("a", &["backup", "--all"]);
    assert_eq!(backup["pushes"].as_array().unwrap().len(), 2);
    let mirror_dir = PathBuf::from(backup["pushes"][0]["dir"].as_str().unwrap());
    let archive = backup["pushes"][1]["archive"].as_str().unwrap().to_owned();
    let zip = PathBuf::from(backup["pushes"][1]["dir"].as_str().unwrap())
        .join("archives")
        .join(&archive);
    assert!(zip.exists(), "{}", zip.display());
    // --remote pushes to one only.
    let one = cli.ok("a", &["backup", "--remote", "mirror"]);
    assert_eq!(one["pushes"].as_array().unwrap().len(), 1);

    // A new machine from the mirror; another from the zip.
    let restored = cli.ok(
        "b",
        &[
            "restore",
            "--from",
            &mirror_dir.join("latest").to_string_lossy(),
        ],
    );
    assert_eq!(restored["created"], true);
    assert!(cli.has_server("b", "notes"));
    let restored = cli.ok("c", &["restore", "--from", &zip.to_string_lossy()]);
    assert_eq!(restored["created"], true);
    assert!(cli.has_server("c", "notes"));

    // On a machine with the store: the export's tree comes back as a new
    // commit.
    cli.ok("a", &["server", "rm", "notes"]);
    assert!(!cli.has_server("a", "notes"));
    cli.ok("a", &["restore", "--from", &zip.to_string_lossy()]);
    assert!(cli.has_server("a", "notes"));

    // Another store's export is refused.
    cli.add_server("d", "other", "other-mcp");
    let (code, doc) = cli.json("d", &["restore", "--from", &zip.to_string_lossy()]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("invalid_value"))
    );
}

//! `loadout server` and `loadout history` against a temp `LOADOUT_HOME`.
//! Every run uses `TempHome::command`: a cleared environment whose `PATH`
//! holds only the empty stub directory, so no git binary is reachable.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};

use agent_loadout_core::store::StoreLock;
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

    /// Runs with `--json` (before the args, so it never lands after `--`)
    /// and expects success.
    fn json(&self, args: &[&str]) -> Value {
        let output = self.run(&[&["--json"], args].concat());
        assert!(output.status.success(), "{args:?}: {output:?}");
        stdout_json(&output)
    }

    fn history(&self) -> Vec<Value> {
        self.json(&["history"])["commits"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn store(&self) -> PathBuf {
        self.home.join("store")
    }
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "stdout is not JSON ({err}):\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn files_below(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_below(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
fn server_add_ls_show_rm_and_history() {
    let cli = Cli::new();
    assert_eq!(
        cli.json(&["server", "ls"]),
        json!({"schema": 1, "servers": []})
    );
    assert_eq!(cli.json(&["history"]), json!({"schema": 1, "commits": []}));
    assert!(
        !cli.home.exists(),
        "read-only commands must not create the store"
    );

    // A bad definition fails before the store is created.
    let output = cli.run(&["server", "add", "Bad_Id", "--json", "--", "npx"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout_json(&output)["error"]["code"], "invalid_id");
    let output = cli.run(&["server", "add", "nocmd", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout_json(&output)["error"]["code"], "missing_field");
    assert!(!cli.home.exists());

    let added = cli.json(&[
        "server",
        "add",
        "github",
        "--env",
        "GITHUB_TOOLSETS=repos,issues",
        "--env",
        "GITHUB_PERSONAL_ACCESS_TOKEN=${secret:GITHUB_PAT}",
        "--client",
        "claude-code",
        "--",
        "npx",
        "-y",
        "@modelcontextprotocol/server-github",
    ]);
    assert_eq!(added["server"], "github");
    let commit = added["commit"].as_str().unwrap().to_owned();

    // Unknown clients are kept, with a warning on stderr.
    let output = cli.run(&[
        "server",
        "add",
        "linear",
        "--url",
        "https://mcp.example.test/mcp",
        "--header",
        "Authorization=Bearer ${secret:LINEAR_TOKEN}",
        "--client",
        "future-client",
    ]);
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("warning[unknown_client]"));

    let output = cli.run(&["server", "add", "github", "--json", "--", "npx"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout_json(&output)["error"]["code"], "already_exists");

    assert_eq!(
        cli.json(&["server", "ls"]),
        json!({"schema": 1, "servers": [
            {
                "id": "github", "valid": true, "display_name": null,
                "transport": "stdio", "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-github"],
                "url": null, "clients": ["claude-code"],
            },
            {
                "id": "linear", "valid": true, "display_name": null,
                "transport": "http", "command": null, "args": null,
                "url": "https://mcp.example.test/mcp", "clients": ["future-client"],
            },
        ]})
    );

    let shown = cli.json(&["server", "show", "github"]);
    assert_eq!(shown["server"]["id"], "github");
    assert_eq!(
        shown["server"]["env"]["GITHUB_PERSONAL_ACCESS_TOKEN"],
        "${secret:GITHUB_PAT}"
    );
    let output = cli.run(&["server", "show", "github"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        std::fs::read_to_string(cli.store().join("servers").join("github.toml")).unwrap()
    );
    let output = cli.run(&["server", "show", "nope", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout_json(&output)["error"]["code"], "not_found");

    // Removing several servers is one commit; a missing id removes nothing.
    let output = cli.run(&["server", "rm", "github", "nope", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout_json(&output)["error"]["code"], "not_found");
    assert_eq!(cli.history().len(), 3);
    let removed = cli.json(&["server", "rm", "github", "linear"]);
    assert_eq!(removed["servers"], json!(["github", "linear"]));
    assert_eq!(cli.json(&["server", "ls"])["servers"], json!([]));

    let history = cli.history();
    let summaries: Vec<&str> = history
        .iter()
        .map(|c| c["summary"].as_str().unwrap())
        .collect();
    assert_eq!(summaries.len(), 4);
    assert_eq!(summaries[0], "server: remove 2 servers (github, linear)");
    assert_eq!(summaries[1], "server: add linear");
    assert_eq!(summaries[2], "server: add github");
    assert!(summaries[3].starts_with("init: store "));
    assert_eq!(history[0]["op"], "server.remove");
    assert_eq!(
        history[0]["entities"],
        json!(["server/github", "server/linear"])
    );
    assert_eq!(history[2]["id"], commit.as_str());
    assert_eq!(history[2]["scope"], "system");
    assert_eq!(
        history[2]["client"],
        format!("loadout-cli/{}", env!("CARGO_PKG_VERSION"))
    );
    let machine_id = history[2]["machine"]["id"].as_str().unwrap();
    assert_eq!(
        history[2]["author"]["email"],
        format!("{machine_id}@loadout.invalid")
    );

    let github = cli.json(&["history", "--entity", "server/github"])["commits"].clone();
    let ids: Vec<&Value> = github
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["id"])
        .collect();
    assert_eq!(ids, [&history[0]["id"], &history[2]["id"]]);
    assert_eq!(
        cli.json(&["history", "--limit", "1"])["commits"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let output = cli.run(&["history"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("server: add github"));
}

#[test]
fn a_bogus_git_config_never_reaches_the_store() {
    let cli = Cli::new();
    // Everything a git config could inject: identity, branch, templates.
    let template = cli.th.root().join("template");
    std::fs::create_dir_all(template.join("hooks")).unwrap();
    std::fs::write(template.join("hooks").join("bogus-hook"), "exit 1\n").unwrap();
    let config = format!(
        "[user]\n\tname = BogusConfigUser\n\temail = bogus@config.invalid\n\
         [init]\n\tdefaultBranch = bogusbranch\n\ttemplateDir = {}\n",
        template.to_string_lossy().replace('\\', "/")
    );
    std::fs::write(cli.th.home().join(".gitconfig"), &config).unwrap();
    let xdg_git = cli.th.xdg().join("config").join("git");
    std::fs::create_dir_all(&xdg_git).unwrap();
    std::fs::write(xdg_git.join("config"), &config).unwrap();

    cli.json(&["server", "add", "a", "--", "npx"]);
    cli.json(&["server", "add", "b", "--", "npx"]);
    cli.json(&["server", "rm", "a", "b"]);

    for commit in cli.history() {
        let machine_id = commit["machine"]["id"].as_str().unwrap();
        let label = commit["machine"]["label"].as_str().unwrap();
        assert_eq!(commit["author"]["name"], format!("loadout {label}"));
        assert_eq!(
            commit["author"]["email"],
            format!("{machine_id}@loadout.invalid")
        );
        assert!(!commit.to_string().contains("Bogus"), "{commit}");
    }

    let git_dir = cli.store().join(".git");
    assert_eq!(
        std::fs::read_to_string(git_dir.join("HEAD")).unwrap(),
        "ref: refs/heads/main\n"
    );
    assert!(!git_dir.join("hooks").join("bogus-hook").exists());
    // Objects are compressed; every other file (refs, reflogs, config,
    // index) is searched as bytes.
    let mut files = Vec::new();
    files_below(&git_dir, &mut files);
    for file in files
        .iter()
        .filter(|file| !file.starts_with(git_dir.join("objects")))
    {
        let bytes = std::fs::read(file).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        for needle in ["BogusConfigUser", "bogus@config.invalid", "bogusbranch"] {
            assert!(!text.contains(needle), "{needle} in {}", file.display());
        }
    }
    assert!(
        std::fs::read_to_string(git_dir.join("logs").join("HEAD"))
            .unwrap()
            .contains("@loadout.invalid"),
        "reflog entries carry the Loadout identity"
    );
}

#[test]
fn lock_contention_exits_6_and_commits_nothing() {
    let cli = Cli::new();
    cli.json(&["server", "add", "a", "--", "npx"]);
    let before = cli.history().len();

    // This test process holds the lock; the CLI is the second process.
    let held = StoreLock::acquire(
        &cli.th.fs(),
        &cli.home.join("state"),
        Duration::from_secs(1),
    )
    .unwrap();
    let started = Instant::now();
    let output = cli.run(&["server", "add", "b", "--json", "--", "npx"]);
    assert_eq!(output.status.code(), Some(6), "{output:?}");
    assert!(started.elapsed() >= Duration::from_secs(9));
    let error = &stdout_json(&output)["error"];
    assert_eq!(error["code"], "locked");
    drop(held);

    assert_eq!(cli.history().len(), before);
    assert!(!cli.store().join("servers").join("b.toml").exists());
    let output = cli.run(&["server", "add", "b", "--", "npx"]);
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn works_with_no_git_binary_on_path() {
    let cli = Cli::new();
    let bin = cli.th.bin();
    assert_eq!(std::fs::read_dir(&bin).unwrap().count(), 0);

    let output = cli
        .th
        .command(env!("CARGO_BIN_EXE_loadout"))
        .env("PATH", &bin)
        .arg("--home")
        .arg(&cli.home)
        .args(["server", "add", "local", "--json", "--", "npx", "-y", "pkg"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(cli.json(&["server", "ls"])["servers"][0]["id"], "local");
    assert_eq!(cli.history().len(), 2);
    cli.json(&["server", "rm", "local"]);
    assert_eq!(cli.history().len(), 3);
}

#[test]
fn mutating_commands_warn_about_a_sync_folder() {
    let th = TempHome::new();
    let sync = th.root().join("Sync");
    std::fs::create_dir_all(sync.join(".sync")).unwrap();
    std::fs::write(sync.join(".sync").join("ID"), b"").unwrap();
    let home = sync.join("loadout");

    let run = |args: &[&str]| {
        th.command(env!("CARGO_BIN_EXE_loadout"))
            .arg("--home")
            .arg(&home)
            .args(args)
            .output()
            .unwrap()
    };
    let output = run(&["server", "add", "a", "--", "npx"]);
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("warning[sync_root_detected]"));
    let output = run(&["server", "ls"]);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}

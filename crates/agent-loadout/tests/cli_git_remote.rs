//! `loadout remote add git`, `remote token`, `remote create-github`,
//! `loadout git-credential` and git pushes and pulls (SPEC 9.6 to 9.8)
//! with temp `--home` folders, a local bare repository, the system git
//! isolated from the user's configuration, and `gh-stub`. Tests that need
//! git skip with a notice when it is absent.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use agent_loadout_core::remote::config::RemoteConfig;
use agent_loadout_core::remote::git::{self, GitAuth};
use agent_loadout_testkit::TempHome;
use agent_loadout_testkit::gh_stub::GhStub;
use agent_loadout_testkit::git_env::{git_vars, init_bare, system_git_or_skip};
use serde_json::Value;

/// A token-shaped value that is no real secret.
fn fake_token() -> String {
    ["stub", "-token-", "fedcba9876543210"].concat()
}

struct Cli {
    th: TempHome,
    git: PathBuf,
}

impl Cli {
    fn new(test: &str) -> Option<Self> {
        let git = system_git_or_skip(test)?;
        Some(Self {
            th: TempHome::new(),
            git,
        })
    }

    fn home(&self, name: &str) -> PathBuf {
        self.th.root().join(name)
    }

    /// `loadout` with the isolated git on `PATH`.
    fn command(&self, home: &str, args: &[&str]) -> Command {
        let mut command = self.th.command(env!("CARGO_BIN_EXE_loadout"));
        command
            .envs(git_vars(&self.th, &self.git))
            .arg("--home")
            .arg(self.home(home))
            .args(args);
        command
    }

    fn run(&self, home: &str, args: &[&str]) -> Output {
        self.command(home, args)
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

    fn with_stdin(&self, home: &str, args: &[&str], input: &str) -> Output {
        let mut child = self
            .command(home, args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn add_server(&self, home: &str, id: &str) {
        self.ok(
            home,
            &[
                "server",
                "add",
                id,
                "--client",
                "claude-code",
                "--",
                &format!("{id}-mcp"),
            ],
        );
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

    /// `git credential fill` with `flags`, as git runs a helper; stdout.
    fn credential_fill(&self, home: &Path, flags: &[std::ffi::OsString], request: &str) -> Output {
        let mut child = self
            .th
            .command(&self.git)
            .envs(git_vars(&self.th, &self.git))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LOADOUT_HOME", home)
            .args(flags)
            .args(["credential", "fill"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
}

#[test]
fn git_remotes_push_pull_and_restore() {
    let Some(cli) = Cli::new("git_remotes_push_pull_and_restore") else {
        return;
    };
    let bare = cli.th.root().join("remote.git");
    init_bare(&cli.th, &cli.git, &bare);
    let url = bare.to_string_lossy().into_owned();
    cli.add_server("a", "notes");
    let added = cli.ok("a", &["remote", "add", "git", "origin", &url]);
    assert_eq!(added["auth"], "system");
    let listed = cli.ok("a", &["remote", "ls"]);
    assert_eq!(listed["remotes"][0]["kind"], "git");
    assert_eq!(listed["remotes"][0]["url"], url.as_str());
    let pushed = cli.ok("a", &["remote", "push"]);
    assert_eq!(pushed["pushes"][0]["ok"], true, "{pushed:#}");
    assert_eq!(pushed["pushes"][0]["kind"], "git");

    // A second machine restores from the bare repository, then both push.
    let restored = cli.ok("b", &["restore", "--from", &url]);
    assert_eq!(restored["created"], true);
    assert!(cli.has_server("b", "notes"));
    cli.ok("b", &["remote", "add", "git", "origin", &url]);
    cli.add_server("b", "search");
    cli.ok("b", &["backup"]);
    cli.add_server("a", "files");
    // A's backup push is rejected, pulls B's change, and retries.
    let backup = cli.ok("a", &["backup"]);
    let push = &backup["pushes"][0];
    assert_eq!(push["ok"], true, "{backup:#}");
    assert_eq!(push["pulled"]["machines"][0]["outcome"], "merged");
    assert!(cli.has_server("a", "search"));
    let pulled = cli.ok("b", &["remote", "pull"]);
    assert_eq!(pulled["pulls"][0]["machines"][0]["outcome"], "fast_forward");
    assert_eq!(cli.head("a"), cli.head("b"));
    let text = cli.run("b", &["remote", "pull"]);
    assert!(String::from_utf8_lossy(&text.stdout).contains("origin: up to date"));
    // Git remotes hold no machine subfolders.
    let (code, doc) = cli.json("a", &["remote", "machines", "origin"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("invalid_value"))
    );
}

#[test]
fn token_auth_answers_git_through_the_keyring() {
    let Some(cli) = Cli::new("token_auth_answers_git_through_the_keyring") else {
        return;
    };
    let url = "https://git.example.com/me/loadout-store.git";
    cli.add_server("a", "notes");
    cli.ok(
        "a",
        &[
            "remote",
            "add",
            "git",
            "origin",
            url,
            "--auth",
            "token",
            "--username",
            "me",
        ],
    );
    // The token comes from stdin, never from arguments.
    let stored = cli.with_stdin(
        "a",
        &["remote", "token", "origin"],
        &format!("{}\n", fake_token()),
    );
    assert!(
        stored.status.success(),
        "{}",
        String::from_utf8_lossy(&stored.stderr)
    );
    let keyring = std::fs::read_to_string(cli.th.keyring_file()).unwrap();
    assert!(keyring.contains("git-remote/origin"), "{keyring}");

    // Git runs `loadout git-credential` with the flags Loadout passes.
    let mut remote = RemoteConfig::git("origin", url, GitAuth::Token);
    remote.username = Some("me".to_owned());
    let helper = Path::new(env!("CARGO_BIN_EXE_loadout"));
    let flags = git::auth_args(
        &remote,
        Some(helper),
        if cfg!(windows) {
            agent_loadout_core::Os::Windows
        } else {
            agent_loadout_core::Os::Linux
        },
    )
    .unwrap();
    for flag in &flags {
        assert!(!flag.to_string_lossy().contains(&fake_token()));
    }
    let home = cli.home("a");
    let output = cli.credential_fill(
        &home,
        &flags,
        "protocol=https\nhost=git.example.com\npath=me/loadout-store.git\n\n",
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("username=me\n"), "{stdout}");
    assert!(
        stdout.contains(&format!("password={}\n", fake_token())),
        "{stdout}"
    );

    // Another host gets nothing from the helper (git then fails without a
    // prompt).
    let output = cli.credential_fill(&home, &flags, "protocol=https\nhost=evil.example.com\n\n");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&fake_token()));
    assert!(!output.status.success());

    // `--clear` and `remote rm` delete the token.
    cli.ok("a", &["remote", "token", "origin", "--clear"]);
    assert!(
        !std::fs::read_to_string(cli.th.keyring_file())
            .unwrap()
            .contains(&fake_token())
    );
    let stored = cli.with_stdin("a", &["remote", "token", "origin"], &fake_token());
    assert!(stored.status.success());
    let removed = cli.ok("a", &["remote", "rm", "origin"]);
    assert_eq!(removed["token_deleted"], true);
    // `token` is for git remotes with auth token only.
    cli.ok("a", &["remote", "add", "git", "plain", url]);
    let output = cli.with_stdin("a", &["--json", "remote", "token", "plain"], &fake_token());
    assert_eq!(output.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&fake_token()));
}

#[test]
fn create_github_makes_a_private_repository_through_gh() {
    let th = TempHome::new();
    let stub = GhStub::install(&th.bin());
    let home = th.root().join("a");
    let run = |args: &[&str]| -> (i32, Value) {
        let output = th
            .command(env!("CARGO_BIN_EXE_loadout"))
            .arg("--home")
            .arg(&home)
            .arg("--json")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let doc = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
            panic!(
                "{args:?}: {err}: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.code().unwrap(), doc)
    };
    let (code, doc) = run(&["remote", "create-github", "loadout-store"]);
    assert_eq!(code, 0, "{doc:#}");
    assert_eq!(
        doc["created"],
        "https://github.com/stub-user/loadout-store.git"
    );
    assert_eq!(doc["auth"], "gh");
    assert!(
        stub.calls()
            .contains(&"repo create loadout-store --private".to_owned())
    );
    let (_, listed) = run(&["remote", "ls"]);
    assert_eq!(listed["remotes"][0]["name"], "github");
    assert_eq!(listed["remotes"][0]["auth"], "gh");
    // The name is taken now; gh is not asked again.
    let calls = stub.calls().len();
    let (code, doc) = run(&["remote", "create-github", "other"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("already_exists"))
    );
    assert_eq!(stub.calls().len(), calls);
    // An invalid remote name is refused before gh creates anything.
    let (code, doc) = run(&["remote", "create-github", "fresh", "--name", "my remote"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("invalid_value"))
    );
    assert_eq!(stub.calls().len(), calls);
    // auth gh is for GitHub URLs only.
    let (code, doc) = run(&[
        "remote",
        "add",
        "git",
        "corp",
        "https://git.example.com/me/s.git",
        "--auth",
        "gh",
    ]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("invalid_value"))
    );
}

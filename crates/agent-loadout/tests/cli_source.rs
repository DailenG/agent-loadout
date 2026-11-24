//! `loadout source ls|trust|block|untrust` and the trust gate of `loadout
//! skills add|update` (#24): without a terminal an untrusted git source
//! needs `--trust-source`, `--yes` never answers it, and a block wins.
//! Git sources are local bare repositories in a temp root.

use std::path::{Path, PathBuf};
use std::process::Output;

use agent_loadout_testkit::TempHome;
use agent_loadout_testkit::git_env::{git_vars, init_bare, system_git_or_skip};
use serde_json::Value;

struct Cli {
    th: TempHome,
    home: PathBuf,
    git: Option<PathBuf>,
}

impl Cli {
    fn new(git: Option<PathBuf>) -> Self {
        let th = TempHome::new();
        let home = th.root().join("lh");
        Self { th, home, git }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = self.th.command(env!("CARGO_BIN_EXE_loadout"));
        if let Some(git) = &self.git {
            command.envs(git_vars(&self.th, git));
        }
        command
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

    fn fails(&self, args: &[&str], code: &str) -> Value {
        let (exit, doc) = self.json(args);
        assert_eq!(exit, 1, "{args:?}: {doc:#}");
        assert_eq!(doc["error"]["code"], code, "{args:?}: {doc:#}");
        doc
    }

    fn git(&self, cwd: &Path, args: &[&str]) {
        let git = self.git.as_ref().unwrap();
        let status = self
            .th
            .command(git)
            .envs(git_vars(&self.th, git))
            .current_dir(cwd)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }
}

/// A bare repository holding one skill at `skills/alpha`.
fn upstream(cli: &Cli) -> PathBuf {
    let git = cli.git.as_ref().unwrap();
    let bare = cli.th.root().join("up.git");
    init_bare(&cli.th, git, &bare);
    let root = cli.th.root();
    cli.git(root, &["init", "--quiet", "--initial-branch=main", "work"]);
    let work = root.join("work");
    cli.git(&work, &["remote", "add", "origin", "../up.git"]);
    let skill = work.join("skills").join("alpha");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: alpha\ndescription: Alpha.\n---\n\n# Alpha\n",
    )
    .unwrap();
    cli.git(&work, &["add", "-A"]);
    cli.git(&work, &["commit", "--quiet", "-m", "initial"]);
    cli.git(&work, &["push", "--quiet", "origin", "HEAD:main"]);
    bare
}

fn strs(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

#[test]
fn source_commands_record_trust_and_blocks() {
    let cli = Cli::new(None);
    let doc = cli.ok(&["source", "ls"]);
    assert_eq!(doc["sources"], serde_json::json!([]));

    let doc = cli.ok(&["source", "trust", "https://github.com/Acme/Skills.git"]);
    assert_eq!(doc["key"], "github.com/acme/skills");
    let doc = cli.ok(&[
        "source",
        "trust",
        "git@github.com:anthropics/skills.git",
        "--scope",
        "owner",
    ]);
    assert_eq!(doc["key"], "github.com/anthropics/*");
    let doc = cli.ok(&["source", "block", "git.example.corp/*"]);
    assert_eq!(doc["key"], "git.example.corp/*");
    cli.fails(&["source", "trust", "github.com/*"], "invalid_value");
    cli.fails(&["source", "trust", "not a key"], "invalid_value");
    cli.fails(
        &["source", "trust", "github.com/acme/*", "--scope", "repo"],
        "invalid_value",
    );

    let doc = cli.ok(&["source", "ls"]);
    let rows: Vec<(&str, &str, &str, &str, bool)> = doc["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["key"].as_str().unwrap(),
                row["decision"].as_str().unwrap(),
                row["scope"].as_str().unwrap(),
                row["via"].as_str().unwrap(),
                row["official"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("git.example.corp/*", "block", "host", "cli-command", false),
            (
                "github.com/acme/skills",
                "trust",
                "repo",
                "cli-command",
                false
            ),
            (
                "github.com/anthropics/*",
                "trust",
                "owner",
                "cli-command",
                true
            ),
        ]
    );
    // The canonical URL git fetches from, never the text as typed.
    assert_eq!(
        doc["sources"][1]["url"],
        "https://github.com/acme/skills.git"
    );

    // Block turns a trust into a block; untrust removes either.
    cli.ok(&["source", "block", "github.com/acme/skills"]);
    let doc = cli.ok(&["source", "ls"]);
    assert_eq!(doc["sources"][1]["decision"], "block");
    cli.ok(&["source", "untrust", "github.com/acme/skills"]);
    cli.fails(
        &["source", "untrust", "github.com/acme/skills"],
        "not_found",
    );
    let doc = cli.ok(&["source", "ls"]);
    assert_eq!(doc["sources"].as_array().unwrap().len(), 2);
}

#[test]
fn skills_add_and_update_need_trust_without_a_terminal() {
    let Some(git) = system_git_or_skip("skills_add_and_update_need_trust_without_a_terminal")
    else {
        return;
    };
    let cli = Cli::new(Some(git));
    let bare = upstream(&cli);
    let url = bare.to_str().unwrap();
    let add = |extra: &[&str]| -> Vec<String> {
        let mut args: Vec<String> = ["skills", "add", url, "--path", "skills/alpha"]
            .iter()
            .map(|arg| (*arg).to_owned())
            .collect();
        args.extend(extra.iter().map(|arg| (*arg).to_owned()));
        args
    };

    // No terminal, no flag: refused, with the way out in the message.
    let doc = cli.fails(&strs(&add(&[])), "source_trust_required");
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(message.contains("--trust-source"), "{message}");
    // --yes is never a trust answer.
    let args = [vec!["--yes".to_owned()], add(&[])].concat();
    cli.fails(&strs(&args), "source_trust_required");
    assert!(!cli.home.join("store").join("skills").join("alpha").exists());

    // --trust-source alone is "this time only".
    cli.ok(&strs(&add(&["--trust-source"])));
    assert_eq!(cli.ok(&["source", "ls"])["sources"], serde_json::json!([]));
    cli.ok(&["skills", "rm", "alpha"]);

    // --trust-source=repo records the decision; later commands need none.
    cli.ok(&strs(&add(&["--trust-source=repo"])));
    let doc = cli.ok(&["source", "ls"]);
    assert_eq!(doc["sources"][0]["via"], "cli-flag");
    assert!(
        doc["sources"][0]["key"]
            .as_str()
            .unwrap()
            .starts_with("file:///"),
        "{doc:#}"
    );
    let doc = cli.ok(&["skills", "update", "--check"]);
    assert_eq!(doc["skills"][0]["trust"]["state"], "trusted");
    cli.ok(&["skills", "update"]);
    // `--trust-source` takes a value only after `=`: here `repo` is the id
    // of a skill to update (there is none), not "always trust the repo".
    cli.fails(&["skills", "update", "--trust-source", "repo"], "not_found");
    assert_eq!(
        cli.ok(&["source", "ls"])["sources"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // A block wins over the trust and over the flag.
    let key = doc["skills"][0]["trust"]["source"]
        .as_str()
        .unwrap()
        .to_owned();
    cli.ok(&["source", "block", &key]);
    cli.ok(&["skills", "rm", "alpha"]);
    cli.fails(&strs(&add(&["--trust-source"])), "source_blocked");
}

/// `source trust|block|untrust` record the key a URL of the same source
/// gets, however the key is spelled (#24 review P2-4, P3-2).
#[test]
fn source_keys_are_canonical() {
    let cli = Cli::new(None);
    for (given, key) in [
        ("github.com/Evil-Org/*", "github.com/evil-org/*"),
        ("github.com/acme/skills.git", "github.com/acme/skills"),
        ("github.com.:443/Acme/Tools", "github.com/acme/tools"),
        ("https://github.com/Acme/*", "github.com/acme/*"),
        ("git.acme.corp:443/*", "git.acme.corp/*"),
        (
            "git.acme.corp:8443/Team/Tools.git",
            "git.acme.corp:8443/Team/Tools",
        ),
        (
            "http://github.com/acme/skills",
            "http://github.com/acme/skills",
        ),
        ("http://github.com/Acme/*", "http://github.com/acme/*"),
    ] {
        let doc = cli.ok(&["source", "block", given]);
        assert_eq!(doc["key"], key, "{given}");
    }
    let doc = cli.ok(&["source", "trust", "https://github.com/acme/skills"]);
    assert_eq!(doc["key"], "github.com/acme/skills");
    // Untrust takes any spelling of the key, too.
    cli.ok(&["source", "untrust", "GitHub.com/Evil-Org/*"]);
    cli.fails(&["source", "untrust", "github.com/evil-org/*"], "not_found");
    cli.fails(
        &[
            "source",
            "block",
            "https://evil.example?@github.com/acme/skills",
        ],
        "invalid_value",
    );
    cli.fails(
        &["source", "trust", "github.com/acme/../evil/repo"],
        "invalid_value",
    );
    let doc = cli.fails(
        &["source", "trust", "https://user:s3cret@github.com/"],
        "invalid_value",
    );
    assert!(!doc.to_string().contains("s3cret"), "{doc:#}");
}

/// `skills add` refuses a blocked source before it clones anything (#24
/// review P2-5): here any clone would fail, as nothing is at the URL.
#[test]
fn skills_add_never_contacts_a_blocked_source() {
    let Some(git) = system_git_or_skip("skills_add_never_contacts_a_blocked_source") else {
        return;
    };
    let cli = Cli::new(Some(git));
    let missing = cli.th.root().join("gone").join("skills.git");
    let url = format!(
        "file:///{}",
        missing
            .to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches('/')
    );
    cli.ok(&["source", "block", &url]);
    cli.fails(&["skills", "add", &url], "source_blocked");
    cli.fails(
        &["skills", "add", &url, "--trust-source=once"],
        "source_blocked",
    );
    assert!(!cli.home.join("cache").join("skill-sources").exists());
}

//! `loadout scanner ...` and `skills add|update --external/--no-external`
//! (#27) against a temp `LOADOUT_HOME`: the token reference is never a
//! value (a pasted token is refused and never echoed), Remove turns the
//! scanner off, the switch decides the default and the flags override it,
//! and an untrusted source is never sent to Snyk. Debug builds of
//! `loadout` run `agent-scan-stub` through `LOADOUT_TEST_AGENT_SCAN`.

use std::path::{Path, PathBuf};
use std::process::Output;

use agent_loadout_core::brand;
use agent_loadout_core::paths::{HomeSource, LoadoutHome};
use agent_loadout_testkit::TempHome;
use agent_loadout_testkit::agent_scan_stub::{self, AgentScanStub, output};
use agent_loadout_testkit::git_env::{git_vars, init_bare, system_git_or_skip};
use serde_json::Value;

const TOKEN: &str = "snyk-cli-test-token-9d2e";
const SKILL: &str = "---\nname: tidy\ndescription: Tidies.\n---\n\n# Tidy\n\nSort the imports.\n";

struct Cli {
    th: TempHome,
    home: PathBuf,
    git: Option<PathBuf>,
    stub: Option<AgentScanStub>,
}

impl Cli {
    fn new(git: Option<PathBuf>) -> Self {
        let th = TempHome::new();
        let home = th.root().join("lh");
        Self {
            th,
            home,
            git,
            stub: None,
        }
    }

    /// Installs the stub as the scanner and stores the token in the test
    /// keyring, referenced as `keyring://loadout/snyk`.
    fn with_stub(mut self) -> Self {
        let home = LoadoutHome {
            path: self.home.clone(),
            source: HomeSource::Flag,
            sync_root: None,
        };
        let stub = AgentScanStub::install(&home);
        stub.answer(&output("clean.json"));
        std::fs::write(
            self.th.keyring_file(),
            serde_json::json!({ "loadout/snyk": TOKEN }).to_string(),
        )
        .unwrap();
        self.stub = Some(stub);
        self.ok(&["scanner", "token", "keyring://loadout/snyk"]);
        self
    }

    fn calls(&self) -> usize {
        self.stub.as_ref().map_or(0, |stub| stub.calls().len())
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = self.th.command(env!("CARGO_BIN_EXE_loadout"));
        if let Some(git) = &self.git {
            command.envs(git_vars(&self.th, git));
        }
        if self.stub.is_some() {
            // The built stub: the same bytes as the installed copy, and
            // the asset name its marker records.
            command.env(brand::TEST_AGENT_SCAN_ENV_VAR, agent_scan_stub::binary());
        }
        command
            .arg("--home")
            .arg(&self.home)
            .args(args)
            .output()
            .expect("run loadout")
    }

    fn json(&self, args: &[&str]) -> (i32, Value, String) {
        let output = self.run(&[&["--json"], args].concat());
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let doc = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
            panic!(
                "{args:?}: stdout is not JSON ({err}):\n{}\nstderr:\n{stderr}",
                String::from_utf8_lossy(&output.stdout),
            )
        });
        (output.status.code().unwrap(), doc, stderr)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (code, doc, stderr) = self.json(args);
        assert_eq!(code, 0, "{args:?}: {doc:#}\n{stderr}");
        doc
    }

    fn fails(&self, args: &[&str], code: &str) -> (Value, String) {
        let (exit, doc, stderr) = self.json(args);
        assert_eq!(exit, 1, "{args:?}: {doc:#}");
        assert_eq!(doc["error"]["code"], code, "{args:?}: {doc:#}");
        (doc, stderr)
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

    fn folder(&self, name: &str) -> String {
        let dir = self.th.root().join("src").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), SKILL.replace("tidy", name)).unwrap();
        dir.to_string_lossy().into_owned()
    }
}

#[test]
fn scanner_commands_keep_references_only() {
    let cli = Cli::new(None);
    let doc = cli.ok(&["scanner", "ls"]);
    assert_eq!(doc["scanners"][1]["name"], "snyk-agent-scan");
    assert_eq!(doc["scanners"][1]["enabled"], false);
    let doc = cli.ok(&["scanner", "status"]);
    assert_eq!(doc["enabled"], false);
    assert_eq!(doc["installed"], false);
    assert!(doc["sends"].as_str().unwrap().contains("api.snyk.io"));

    // Review F1: a pasted token is refused and never echoed or stored.
    for pasted in [
        "3f2a9c1e-7b4d-4e8a-9c0f-1a2b3c4d5e6f",
        "deadbeef0123456789abcdef01234567",
    ] {
        let (doc, stderr) = cli.fails(&["scanner", "token", pasted], "invalid_ref");
        assert!(!doc.to_string().contains(pasted), "{doc:#}");
        assert!(!stderr.contains(pasted), "{stderr}");
        let doc = cli.ok(&["scanner", "token"]);
        assert_eq!(doc["token"], Value::Null, "{doc:#}");
    }
    let settings = cli.home.join("state").join("local-settings.toml");
    let text = std::fs::read_to_string(&settings).unwrap_or_default();
    assert!(!text.contains("deadbeef"), "{text}");

    let doc = cli.ok(&["scanner", "token", "keyring://loadout/snyk"]);
    assert_eq!(doc["token"], "keyring://loadout/snyk");
    assert_eq!(doc["kind"], "keyring");
    assert_eq!(cli.ok(&["scanner", "enable"])["enabled"], true);
    assert_eq!(cli.ok(&["scanner", "disable"])["enabled"], false);
    let doc = cli.ok(&["scanner", "token", "--clear"]);
    assert_eq!(doc["token"], Value::Null);
}

/// Review M49 F8: Remove turns the scanner off with it.
#[test]
fn removing_the_scanner_turns_it_off() {
    let cli = Cli::new(None).with_stub();
    assert_eq!(cli.ok(&["scanner", "enable"])["enabled"], true);
    let doc = cli.ok(&["scanner", "remove"]);
    assert_eq!(doc["removed"], true);
    let doc = cli.ok(&["scanner", "status"]);
    assert_eq!(doc["enabled"], false, "{doc:#}");
    assert_eq!(doc["installed"], false, "{doc:#}");
}

#[test]
fn the_switch_is_the_default_and_the_flags_override_it() {
    let cli = Cli::new(None).with_stub();
    // Off: nothing runs by default.
    let doc = cli.ok(&["skills", "add", &cli.folder("one")]);
    assert!(doc.get("external").is_none(), "{doc:#}");
    assert_eq!(cli.calls(), 0);
    // --external runs it once, while off.
    let doc = cli.ok(&["skills", "add", &cli.folder("two"), "--external"]);
    assert_eq!(doc["external"]["status"], "scanned", "{doc:#}");
    assert_eq!(cli.calls(), 1);
    // On: it runs by default, and --no-external wins.
    cli.ok(&["scanner", "enable"]);
    let doc = cli.ok(&["skills", "add", &cli.folder("three"), "--no-external"]);
    assert!(doc.get("external").is_none(), "{doc:#}");
    assert_eq!(cli.calls(), 1);
    let doc = cli.ok(&["skills", "add", &cli.folder("four")]);
    assert_eq!(doc["external"]["status"], "scanned", "{doc:#}");
    assert_eq!(cli.calls(), 2);
    // A dangerous verdict blocks (exit 8) until --allow-dangerous.
    cli.stub.as_ref().unwrap().answer(&output("dangerous.json"));
    let (code, doc, _) = cli.json(&["skills", "add", &cli.folder("five")]);
    assert_eq!(code, 8, "{doc:#}");
    assert_eq!(doc["external"]["verdict"], "dangerous", "{doc:#}");
    cli.ok(&["skills", "add", &cli.folder("five"), "--allow-dangerous"]);
    // `scanner run` scans a library skill.
    cli.stub.as_ref().unwrap().answer(&output("clean.json"));
    let doc = cli.ok(&["scanner", "run", "one"]);
    assert_eq!(doc["external"]["status"], "scanned", "{doc:#}");
    assert_eq!(doc["external"]["verdict"], "clean", "{doc:#}");
    // Not scanned (Snyk's daily limit): no verdict at all, never "clean".
    cli.stub.as_ref().unwrap().answer(&output("limit.json"));
    let (_, doc, _) = cli.json(&["scanner", "run", "one"]);
    assert_eq!(doc["external"]["status"], "not_scanned", "{doc:#}");
    assert_eq!(doc["external"]["reason"], "limit_reached", "{doc:#}");
    assert!(doc["external"]["verdict"].is_null(), "{doc:#}");
}

/// Review F9: an untrusted source is never sent to Snyk: not by `update
/// --check`, and not before the trust answer of an update.
#[test]
fn an_untrusted_update_is_not_sent_to_snyk() {
    let Some(git) = system_git_or_skip("an_untrusted_update_is_not_sent_to_snyk") else {
        return;
    };
    let cli = Cli::new(Some(git.clone())).with_stub();
    let bare = cli.th.root().join("up.git");
    init_bare(&cli.th, &git, &bare);
    let root = cli.th.root();
    cli.git(root, &["init", "--quiet", "--initial-branch=main", "work"]);
    let work = root.join("work");
    cli.git(&work, &["remote", "add", "origin", "../up.git"]);
    let skill = work.join("skills").join("alpha");
    std::fs::create_dir_all(&skill).unwrap();
    let push = |text: &str, message: &str| {
        std::fs::write(skill.join("SKILL.md"), SKILL.replace("tidy", text)).unwrap();
        cli.git(&work, &["add", "-A"]);
        cli.git(&work, &["commit", "--quiet", "-m", message]);
        cli.git(&work, &["push", "--quiet", "origin", "HEAD:main"]);
    };
    push("alpha", "initial");
    let url = format!(
        "file:///{}",
        bare.to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches('/')
    );
    // Trusted this time only: the source stays untrusted.
    cli.ok(&[
        "skills",
        "add",
        &url,
        "--path",
        "skills/alpha",
        "--trust-source",
        "--no-external",
    ]);
    push("alpha v2", "v2");
    cli.ok(&["scanner", "enable"]);

    let doc = cli.ok(&["skills", "update", "--check"]);
    assert_eq!(doc["skills"][0]["status"], "available", "{doc:#}");
    assert!(doc["skills"][0].get("external").is_none(), "{doc:#}");
    let (_, doc, _) = cli.json(&["skills", "update", "alpha"]);
    assert_eq!(doc["skills"][0]["status"], "failed", "{doc:#}");
    assert_eq!(
        doc["skills"][0]["error"]["code"], "source_trust_required",
        "{doc:#}"
    );
    assert_eq!(cli.calls(), 0, "an untrusted source was sent to Snyk");

    // Trusted: the update is scanned once, then written.
    let doc = cli.ok(&["skills", "update", "alpha", "--trust-source"]);
    assert_eq!(doc["skills"][0]["status"], "updated", "{doc:#}");
    assert_eq!(doc["skills"][0]["external"]["status"], "scanned", "{doc:#}");
    assert_eq!(cli.calls(), 1);
}

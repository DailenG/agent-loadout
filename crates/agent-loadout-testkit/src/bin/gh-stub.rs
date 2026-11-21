//! Fake GitHub CLI for tests. Installed as `gh` by
//! `agent_loadout_testkit::gh_stub::GhStub`, which writes the
//! `gh-stub.json` next to it. See that module for what it answers.

use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;

use agent_loadout_testkit::gh_stub::{CONFIG_FILE, GhStubConfig, LOG_FILE, REPOS_FILE};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(ToOwned::to_owned))
        .expect("gh-stub directory");
    append(&dir.join(LOG_FILE), &args.join(" "));
    let config: GhStubConfig = std::fs::read_to_string(dir.join(CONFIG_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["auth", "status"] => {
            if config.logged_in {
                eprintln!(
                    "github.com\n  Logged in to github.com account {}",
                    config.user
                );
                ExitCode::SUCCESS
            } else {
                eprintln!(
                    "You are not logged into any GitHub hosts. To log in, run: gh auth login"
                );
                ExitCode::from(1)
            }
        }
        ["auth", "git-credential", action] => {
            let mut input = String::new();
            let _ = std::io::stdin().read_to_string(&mut input);
            if *action != "get" || !config.logged_in {
                return ExitCode::SUCCESS;
            }
            let host = input
                .lines()
                .find_map(|line| line.strip_prefix("host="))
                .unwrap_or_default();
            if host != "github.com" {
                return ExitCode::SUCCESS;
            }
            let mut out = std::io::stdout();
            let _ = write!(
                out,
                "protocol=https\nhost=github.com\nusername={}\npassword={}\n",
                config.user, config.token
            );
            ExitCode::SUCCESS
        }
        ["repo", "create", name, "--private"] => {
            if !config.logged_in {
                eprintln!("To get started with GitHub CLI, please run:  gh auth login");
                return ExitCode::from(4);
            }
            let repos_file = dir.join(REPOS_FILE);
            let repos = std::fs::read_to_string(&repos_file).unwrap_or_default();
            if repos.lines().any(|line| line == *name) {
                eprintln!("GraphQL: Name already exists on this account (createRepository)");
                return ExitCode::from(1);
            }
            append(&repos_file, name);
            let full = if name.contains('/') {
                (*name).to_owned()
            } else {
                format!("{}/{name}", config.user)
            };
            println!("https://github.com/{full}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("gh-stub: unsupported call: {}", args.join(" "));
            ExitCode::from(2)
        }
    }
}

fn append(path: &Path, line: &str) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open gh-stub file");
    writeln!(file, "{line}").expect("write gh-stub file");
}

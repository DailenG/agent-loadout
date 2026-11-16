//! Fake 1Password CLI for tests. Installed as `op` by
//! `agent_loadout_testkit::op_stub::OpStub`, which writes the
//! `op-stub.json` next to it. Supports `--version`, `read [--no-newline]
//! <ref>` and `environment secret list <id> --format json`; refuses
//! `environment read` (logged as forbidden, so the test fails).
//!
//! Error messages imitate the real CLI's shape ([verify] wording per
//! version, `scripts/verify-clients.md` SE-2).

use std::io::Write;
use std::process::ExitCode;

use agent_loadout_testkit::op_stub::{CONFIG_FILE, FORBIDDEN, LOG_FILE, OpStubConfig, OpStubMode};

const NOT_SIGNED_IN: &str = "[ERROR] 2026/01/01 00:00:00 You are not currently signed in. \
     Please run `op signin --help` for instructions";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(ToOwned::to_owned))
        .expect("op-stub directory");
    log(&dir.join(LOG_FILE), &args.join(" "));
    let config: OpStubConfig = std::fs::read_to_string(dir.join(CONFIG_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let version = parse_version(&config.version);
    let words: Vec<&str> = args.iter().map(String::as_str).collect();

    match words.as_slice() {
        ["--version"] => {
            println!("{}", config.version);
            ExitCode::SUCCESS
        }
        ["read", rest @ ..] => read(&config, rest),
        ["environment", "read", ..] => {
            log(
                &dir.join(LOG_FILE),
                &format!("{FORBIDDEN} {}", args.join(" ")),
            );
            fail(
                "op-stub: `op environment read` is forbidden for Loadout (SPEC 11.2)",
                99,
            )
        }
        ["environment", ..] if version < (2, 41, 0) => {
            fail("[ERROR] unknown command \"environment\" for \"op\"", 1)
        }
        ["environment", "secret", "list", id, "--format", "json"] => {
            if version < (2, 42, 0) {
                return fail(
                    "[ERROR] unknown command \"secret\" for \"op environment\"",
                    1,
                );
            }
            if config.mode == OpStubMode::Locked {
                return fail(NOT_SIGNED_IN, 1);
            }
            let Some(variables) = config.environments.get(*id) else {
                return fail(
                    &format!("[ERROR] 2026/01/01 00:00:00 could not find environment {id:?}"),
                    1,
                );
            };
            let listed: Vec<serde_json::Value> = variables
                .iter()
                .map(|(name, value)| serde_json::json!({"name": name, "value": value}))
                .collect();
            println!("{}", serde_json::Value::Array(listed));
            ExitCode::SUCCESS
        }
        _ => fail(&format!("op-stub: unsupported command {args:?}"), 64),
    }
}

fn read(config: &OpStubConfig, rest: &[&str]) -> ExitCode {
    let no_newline = rest.contains(&"--no-newline");
    let Some(reference) = rest.iter().rev().find(|word| !word.starts_with("--")) else {
        return fail("[ERROR] expected a secret reference", 1);
    };
    if config.mode == OpStubMode::Locked {
        return fail(NOT_SIGNED_IN, 1);
    }
    match config.items.get(*reference) {
        Some(value) => {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(value.as_bytes());
            if !no_newline {
                let _ = out.write_all(b"\n");
            }
            ExitCode::SUCCESS
        }
        None => fail(
            &format!(
                "[ERROR] 2026/01/01 00:00:00 could not read secret '{reference}': \
                 could not find item in vault"
            ),
            1,
        ),
    }
}

fn fail(message: &str, code: u8) -> ExitCode {
    eprintln!("{message}");
    ExitCode::from(code)
}

fn log(path: &std::path::Path, line: &str) {
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

fn parse_version(text: &str) -> (u32, u32, u32) {
    let mut parts = text
        .trim()
        .split('.')
        .map(|part| part.parse::<u32>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

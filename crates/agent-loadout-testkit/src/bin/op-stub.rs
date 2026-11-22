//! Fake 1Password CLI for tests. Installed as `op` by
//! `agent_loadout_testkit::op_stub::OpStub`, which writes the
//! `op-stub.json` next to it. Supports `--version`, `read [--no-newline]
//! <ref>`, `environment secret list <id> --format json` and `run
//! --environment <id> [--no-masking] -- <cmd> [args]`; refuses
//! `environment read` (logged as forbidden, so the test fails).
//!
//! A configured service account token, passed as
//! `OP_SERVICE_ACCOUNT_TOKEN`, signs every call in even when the desktop
//! app is locked; another token is refused.
//!
//! Error messages imitate the real CLI's shape ([verify] wording per
//! version, `scripts/verify-clients.md` SE-2).

use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use agent_loadout_testkit::op_stub::{
    ACCOUNT_SEEN, CONCEALED, CONFIG_FILE, FORBIDDEN, LOG_FILE, OpStubConfig, OpStubMode,
    SERVICE_ACCOUNT_SEEN,
};

const NOT_SIGNED_IN: &str = "[ERROR] 2026/01/01 00:00:00 You are not currently signed in. \
     Please run `op signin --help` for instructions";
const BAD_TOKEN: &str = "[ERROR] 2026/01/01 00:00:00 invalid service account token";
const MULTIPLE_ACCOUNTS: &str = "[ERROR] 2026/01/01 00:00:00 multiple accounts found. Use the \
     --account flag or set the OP_ACCOUNT environment variable to select an account.";
const UNKNOWN_ACCOUNT: &str = "[ERROR] 2026/01/01 00:00:00 no account found for filter";
const TOKEN_VAR: &str = "OP_SERVICE_ACCOUNT_TOKEN";
const ACCOUNT_VAR: &str = "OP_ACCOUNT";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(ToOwned::to_owned))
        .expect("op-stub directory");
    let log_file = dir.join(LOG_FILE);
    log(&log_file, &args.join(" "));
    let token = std::env::var(TOKEN_VAR)
        .ok()
        .filter(|token| !token.is_empty());
    if token.is_some() {
        log(&log_file, SERVICE_ACCOUNT_SEEN);
    }
    let account = std::env::var(ACCOUNT_VAR)
        .ok()
        .filter(|account| !account.is_empty());
    if let Some(account) = &account {
        log(&log_file, &format!("{ACCOUNT_SEEN} {account}"));
    }
    let config: OpStubConfig = std::fs::read_to_string(dir.join(CONFIG_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let version = parse_version(&config.version);
    let signed_in = || -> Result<(), ExitCode> {
        match (&token, &config.service_account_token) {
            (Some(given), Some(accepted)) if given == accepted => Ok(()),
            (Some(_), _) => Err(fail(BAD_TOKEN, 1)),
            // Desktop app with several accounts: `op` refuses to guess
            // (as 2.42.0 does, gate V1 FB-7).
            (None, _) if config.accounts.len() > 1 && account.is_none() => {
                Err(fail(MULTIPLE_ACCOUNTS, 1))
            }
            (None, _)
                if account.as_ref().is_some_and(|a| {
                    !config.accounts.is_empty() && !config.accounts.contains(a)
                }) =>
            {
                Err(fail(UNKNOWN_ACCOUNT, 1))
            }
            (None, _) if config.mode == OpStubMode::Locked => Err(fail(NOT_SIGNED_IN, 1)),
            (None, _) => Ok(()),
        }
    };
    let words: Vec<&str> = args.iter().map(String::as_str).collect();

    match words.as_slice() {
        ["--version"] => {
            println!("{}", config.version);
            ExitCode::SUCCESS
        }
        ["read", rest @ ..] => match signed_in() {
            Ok(()) => read(&config, rest),
            Err(code) => code,
        },
        ["environment", "read", ..] => {
            log(&log_file, &format!("{FORBIDDEN} {}", args.join(" ")));
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
            if let Err(code) = signed_in() {
                return code;
            }
            let Some(variables) = config.environments.get(*id) else {
                return fail(&not_found(id), 1);
            };
            let listed: Vec<serde_json::Value> = variables
                .iter()
                .map(|(name, value)| serde_json::json!({"name": name, "value": value}))
                .collect();
            println!("{}", serde_json::Value::Array(listed));
            ExitCode::SUCCESS
        }
        ["run", rest @ ..] => {
            if version < (2, 41, 0) && rest.contains(&"--environment") {
                return fail("[ERROR] unknown flag: --environment", 1);
            }
            if let Err(code) = signed_in() {
                return code;
            }
            run(&config, rest)
        }
        _ => fail(&format!("op-stub: unsupported command {args:?}"), 64),
    }
}

fn read(config: &OpStubConfig, rest: &[&str]) -> ExitCode {
    let no_newline = rest.contains(&"--no-newline");
    let Some(reference) = rest.iter().rev().find(|word| !word.starts_with("--")) else {
        return fail("[ERROR] expected a secret reference", 1);
    };
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

/// `op run --environment <id> [--no-masking] -- <cmd> [args]`: the command
/// with the Environment's variables added, stdin and stderr inherited.
fn run(config: &OpStubConfig, rest: &[&str]) -> ExitCode {
    let Some(split) = rest.iter().position(|word| *word == "--") else {
        return fail("[ERROR] expected a command after --", 1);
    };
    let (options, command) = (&rest[..split], &rest[split + 1..]);
    let mut environment = None;
    let mut masking = true;
    let mut words = options.iter();
    while let Some(word) = words.next() {
        match *word {
            "--environment" => environment = words.next().copied(),
            "--no-masking" => masking = false,
            other => {
                return fail(
                    &format!("op-stub: unsupported `op run` option {other:?}"),
                    64,
                );
            }
        }
    }
    let Some(id) = environment else {
        return fail("op-stub: only `op run --environment` is emulated", 64);
    };
    let Some(variables) = config.environments.get(id) else {
        return fail(&not_found(id), 1);
    };
    let Some((program, args)) = command.split_first() else {
        return fail("[ERROR] expected a command after --", 1);
    };
    let mut child = Command::new(program);
    child.args(args).envs(variables);
    if !masking {
        return match child.status() {
            Ok(status) => exit_code(status.code()),
            Err(err) => fail(&format!("[ERROR] cannot run {program}: {err}"), 1),
        };
    }
    let output = match child.stdout(Stdio::piped()).output() {
        Ok(output) => output,
        Err(err) => return fail(&format!("[ERROR] cannot run {program}: {err}"), 1),
    };
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    for value in variables.values() {
        text = text.replace(value.as_str(), CONCEALED);
    }
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes()).and_then(|()| out.flush());
    exit_code(output.status.code())
}

fn exit_code(code: Option<i32>) -> ExitCode {
    ExitCode::from(code.and_then(|code| u8::try_from(code).ok()).unwrap_or(1))
}

fn not_found(id: &str) -> String {
    format!("[ERROR] 2026/01/01 00:00:00 could not find environment {id:?}")
}

fn fail(message: &str, code: u8) -> ExitCode {
    eprintln!("{message}");
    ExitCode::from(code)
}

fn log(path: &Path, line: &str) {
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

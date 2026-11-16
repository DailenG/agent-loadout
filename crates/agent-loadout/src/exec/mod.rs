//! `loadout exec` (SPEC 11.5): launch-time secret injection for stdio MCP
//! servers.
//!
//! ```text
//! loadout exec [--ref NAME=REF]... [--op-env ID --map KEY=NAME...] -- <cmd> [args]
//! ```
//!
//! - Self-contained: the references are in argv (references, never
//!   values); the store is never read.
//! - `env:` references come from the inherited environment, `keyring://`
//!   from the OS keyring, `op://` from `op read --no-newline`. With
//!   `--op-env`, the server starts under `op run --environment <id>
//!   --no-masking -- <this binary> exec --stage2 --map KEY=NAME... -- <cmd>`:
//!   stage 2 copies each Environment variable `NAME` to `KEY` and drops
//!   every variable `op run` added that the server did not ask for.
//! - Values live only in memory and in the child's environment.
//!   `OP_SERVICE_ACCOUNT_TOKEN` (from the environment, else
//!   `keyring://loadout/OP_SERVICE_ACCOUNT_TOKEN`) reaches `op` only, never
//!   the server.
//! - Nothing is written to stdout: it is the server's JSON-RPC channel.
//!   Errors go to stderr as `loadout-exec: ...`; exit 78 for configuration
//!   errors, 77 for secrets that cannot be had; otherwise the server's own
//!   exit code.
//! - Unix replaces this process with the server (`execvp`); Windows starts
//!   it inside a kill-on-close Job Object and forwards its exit code.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};

use agent_loadout_core::RealFs;
use agent_loadout_core::secrets::SecretValue;
use agent_loadout_core::secrets::delivery::{SERVICE_ACCOUNT_VAR, op_env};
use agent_loadout_core::secrets::grammar::{SecretRef, is_op_environment_id};
use agent_loadout_core::secrets::providers::onepassword::{OP_NOT_FOUND, OpCli, OpFeature};
use agent_loadout_core::secrets::providers::{env as env_provider, keyring as keyring_provider};
use agent_loadout_core::secrets::render::parse_binding;
use agent_loadout_core::secrets::status::{Resolution, SecretStatus};

/// Prefix of every message `exec` writes (stderr only).
pub const PREFIX: &str = "loadout-exec:";
/// A secret could not be had: missing, locked, or the provider failed.
pub const EXIT_MISSING: i32 = 77;
/// The invocation or this machine's setup is wrong (bad arguments, a
/// provider not installed or too old, a command that cannot start).
pub const EXIT_CONFIG: i32 = 78;

const STAGE2: &str = "--stage2";
const USAGE: &str = "usage: loadout exec [--ref NAME=REF]... [--op-env ID --map KEY=NAME...] \
     -- <cmd> [args]";
/// Names of the variables stage 1 hands to the server, for stage 2 (names
/// only, separated by line breaks).
const KEEP_VAR: &str = "LOADOUT_EXEC_ENV";

/// Why `exec` stops before the server starts.
#[derive(Debug)]
pub struct Failure {
    pub code: i32,
    pub message: String,
}

impl Failure {
    fn config(message: impl Into<String>) -> Self {
        Self {
            code: EXIT_CONFIG,
            message: message.into(),
        }
    }

    fn missing(message: impl Into<String>) -> Self {
        Self {
            code: EXIT_MISSING,
            message: message.into(),
        }
    }

    /// A failed resolution of `key`. A missing 1Password CLI names the
    /// per-machine fix (SPEC 11.11).
    fn resolution(key: &str, resolution: &Resolution) -> Self {
        let detail = resolution.message.as_deref().unwrap_or("");
        match resolution.status {
            SecretStatus::ProviderUnavailable if detail == OP_NOT_FOUND => Self::config(format!(
                "{OP_NOT_FOUND} (op); install it or map the alias behind {key} to keyring/env \
                 for this machine"
            )),
            SecretStatus::ProviderUnavailable | SecretStatus::ProviderTooOld => {
                Self::config(format!("{key}: {detail}"))
            }
            status => Self::missing(format!("{key}: {status}: {detail}")),
        }
    }
}

/// What to run.
#[derive(Debug, Default)]
struct Invocation {
    stage2: bool,
    refs: Vec<(String, SecretRef)>,
    environment: Option<String>,
    maps: Vec<(String, String)>,
    command: OsString,
    args: Vec<OsString>,
}

/// Runs `loadout exec` with the arguments after `exec`. Returns only on
/// failure or (Windows) when the server exits.
pub fn main(args: &[OsString]) -> ! {
    let code = match run(args) {
        Ok(code) => code,
        Err(failure) => {
            eprintln!("{PREFIX} {}", failure.message);
            failure.code
        }
    };
    std::process::exit(code)
}

fn run(args: &[OsString]) -> Result<i32, Failure> {
    let invocation = parse(args)?;
    if invocation.stage2 {
        stage2(invocation)
    } else {
        stage1(invocation)
    }
}

fn parse(args: &[OsString]) -> Result<Invocation, Failure> {
    let mut invocation = Invocation::default();
    let mut rest = args.iter();
    let text = |value: Option<&OsString>, flag: &str| -> Result<String, Failure> {
        let value = value.ok_or_else(|| Failure::config(format!("{flag} needs a value")))?;
        value
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| Failure::config(format!("{flag} value is not UTF-8")))
    };
    loop {
        let Some(arg) = rest.next() else {
            return Err(Failure::config(
                "expected `-- <command> [args]` after the options",
            ));
        };
        let arg = arg.to_string_lossy();
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => {
                (flag.to_owned(), Some(value.to_owned()))
            }
            _ => (arg.into_owned(), None),
        };
        let value = |rest: &mut std::slice::Iter<'_, OsString>| match &inline {
            Some(value) => Ok(value.clone()),
            None => text(rest.next(), &flag),
        };
        match flag.as_str() {
            "--" => break,
            "-h" | "--help" => {
                return Err(Failure {
                    code: 0,
                    message: USAGE.to_owned(),
                });
            }
            STAGE2 => invocation.stage2 = true,
            "--ref" => {
                let binding = value(&mut rest)?;
                let (key, reference) =
                    parse_binding(&binding).map_err(|err| Failure::config(err.message()))?;
                match reference {
                    SecretRef::OnePasswordEnvironment { environment, name } => {
                        set_environment(&mut invocation, environment)?;
                        invocation.maps.push((key, name));
                    }
                    reference => invocation.refs.push((key, reference)),
                }
            }
            "--op-env" => {
                let id = value(&mut rest)?;
                if !is_op_environment_id(&id) {
                    return Err(Failure::config(format!(
                        "invalid 1Password Environment id {id:?}"
                    )));
                }
                set_environment(&mut invocation, id)?;
            }
            "--map" => {
                let map = value(&mut rest)?;
                let valid = |name: &str| {
                    agent_loadout_core::model::ids::is_valid(
                        agent_loadout_core::model::ids::IdKind::EnvVar,
                        name,
                    )
                };
                match map.split_once('=') {
                    Some((key, name)) if valid(key) && valid(name) => {
                        invocation.maps.push((key.to_owned(), name.to_owned()));
                    }
                    _ => {
                        return Err(Failure::config(format!(
                            "expected --map KEY=NAME (variable names), got {map:?}"
                        )));
                    }
                }
            }
            other => return Err(Failure::config(format!("unknown option {other:?}"))),
        }
    }
    let Some(command) = rest.next() else {
        return Err(Failure::config("expected a command after `--`"));
    };
    invocation.command = command.clone();
    invocation.args = rest.cloned().collect();
    if invocation.stage2 {
        if !invocation.refs.is_empty() || invocation.environment.is_some() {
            return Err(Failure::config("--stage2 takes only --map"));
        }
    } else if !invocation.maps.is_empty() && invocation.environment.is_none() {
        return Err(Failure::config("--map needs --op-env <environment-id>"));
    }
    Ok(invocation)
}

fn set_environment(invocation: &mut Invocation, id: String) -> Result<(), Failure> {
    match &invocation.environment {
        Some(other) if *other != id => Err(Failure::config(
            "one 1Password Environment per server (--op-env and op-env:// references differ)",
        )),
        _ => {
            invocation.environment = Some(id);
            Ok(())
        }
    }
}

/// Resolves the references and starts the server, or `op run` for an
/// Environment.
fn stage1(invocation: Invocation) -> Result<i32, Failure> {
    let env = crate::process_env().map_err(|err| Failure::config(err.message()))?;
    let keyring = crate::keyring::keyring(&env);
    let needs_op = invocation.environment.is_some()
        || invocation
            .refs
            .iter()
            .any(|(_, reference)| matches!(reference, SecretRef::OnePassword { .. }));
    let (op, op_child_env) = if needs_op {
        (OpCli::detect(&env, &RealFs), op_env(&env, keyring.as_ref()))
    } else {
        (OpCli::NotFound, env.clone())
    };

    let mut values: Vec<(String, SecretValue)> = Vec::new();
    for (key, reference) in &invocation.refs {
        let value = match reference {
            SecretRef::Env { name } => env_provider::resolve(&env, name),
            SecretRef::Keyring { service, account } => {
                keyring_provider::resolve(keyring.as_ref(), service, account)
            }
            SecretRef::OnePassword { .. } => op.read(&op_child_env, &reference.to_string()),
            SecretRef::OnePasswordEnvironment { .. } => {
                unreachable!("parse moves Environment references to --map")
            }
        }
        .map_err(|resolution| Failure::resolution(key, &resolution))?;
        values.push((key.clone(), value));
    }

    // The server's environment: inherited, without the service account
    // token, plus the resolved values.
    let mut child: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(name, _)| !same_name(name, SERVICE_ACCOUNT_VAR))
        .filter(|(name, _)| !values.iter().any(|(key, _)| same_name(name, key)))
        .collect();
    child.extend(
        values
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value.expose()))),
    );

    let Some(environment) = &invocation.environment else {
        return spawn(&invocation.command, &invocation.args, &child);
    };
    let op_path = op
        .require(OpFeature::Environments)
        .map_err(|resolution| {
            let key = invocation
                .maps
                .first()
                .map_or("the 1Password Environment", |(key, _)| key.as_str());
            Failure::resolution(key, &resolution)
        })?
        .to_path_buf();
    let names: Vec<String> = child
        .iter()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    let mut op_vars = child.clone();
    op_vars.push((KEEP_VAR.into(), names.join("\n").into()));
    if let Some(token) = op_child_env.var(SERVICE_ACCOUNT_VAR) {
        op_vars.push((SERVICE_ACCOUNT_VAR.into(), token.to_owned()));
    }
    let this = std::env::current_exe()
        .map_err(|err| Failure::config(format!("cannot find this binary: {err}")))?;
    let mut args: Vec<OsString> = [
        "run",
        "--environment",
        environment.as_str(),
        "--no-masking",
        "--",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.push(this.into_os_string());
    args.push("exec".into());
    args.push(STAGE2.into());
    for (key, name) in &invocation.maps {
        args.push("--map".into());
        args.push(format!("{key}={name}").into());
    }
    args.push("--".into());
    args.push(invocation.command.clone());
    args.extend(invocation.args.iter().cloned());
    spawn(op_path.as_os_str(), &args, &op_vars)
}

/// Under `op run`: maps Environment variables to the server's keys, keeps
/// only what stage 1 passed on, and starts the server.
fn stage2(invocation: Invocation) -> Result<i32, Failure> {
    let keep = std::env::var_os(KEEP_VAR).ok_or_else(|| {
        Failure::config("--stage2 runs only under `loadout exec --op-env` (through `op run`)")
    })?;
    let keep: BTreeSet<String> = keep.to_string_lossy().split('\n').map(name_key).collect();
    let current: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    let mut mapped = Vec::new();
    for (key, name) in &invocation.maps {
        let value = current
            .iter()
            .find(|(var, value)| same_name(var, name) && !value.is_empty())
            .map(|(_, value)| value.clone())
            .ok_or_else(|| {
                Failure::missing(format!(
                    "{key}: the 1Password Environment has no variable {name}"
                ))
            })?;
        mapped.push((OsString::from(key), value));
    }
    let mut child: Vec<(OsString, OsString)> = current
        .into_iter()
        .filter(|(name, _)| keep.contains(&name_key(&name.to_string_lossy())))
        .filter(|(name, _)| !same_name(name, SERVICE_ACCOUNT_VAR) && !same_name(name, KEEP_VAR))
        .filter(|(name, _)| {
            !mapped
                .iter()
                .any(|(key, _)| same_name(name, &key.to_string_lossy()))
        })
        .collect();
    child.extend(mapped);
    spawn(&invocation.command, &invocation.args, &child)
}

/// Environment variable names compare case-insensitively on Windows.
fn name_key(name: &str) -> String {
    if cfg!(windows) {
        name.to_ascii_uppercase()
    } else {
        name.to_owned()
    }
}

fn same_name(name: &OsStr, other: &str) -> bool {
    name_key(&name.to_string_lossy()) == name_key(other)
}

#[cfg(unix)]
fn spawn(program: &OsStr, args: &[OsString], env: &[(OsString, OsString)]) -> Result<i32, Failure> {
    unix::exec(program, args, env)
}

#[cfg(windows)]
fn spawn(program: &OsStr, args: &[OsString], env: &[(OsString, OsString)]) -> Result<i32, Failure> {
    windows::run(program, args, env)
}

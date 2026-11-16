//! Unix: the server replaces this process (`execvp`), so the client's
//! signals, process tracking and exit status reach it directly and no extra
//! process stays around.

use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::process::Command;

use super::Failure;

/// Replaces this process with `program`; returns only when that fails.
pub fn exec(
    program: &OsStr,
    args: &[OsString],
    env: &[(OsString, OsString)],
) -> Result<i32, Failure> {
    let err = Command::new(program)
        .args(args)
        .env_clear()
        .envs(env.iter().map(|(name, value)| (name, value)))
        .exec();
    Err(Failure::config(format!(
        "cannot start {}: {err}",
        program.to_string_lossy()
    )))
}

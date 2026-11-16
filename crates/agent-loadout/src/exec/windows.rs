//! Windows: the server runs as a child with inherited handles, inside a
//! kill-on-close Job Object this process joins first. Every process this
//! one starts inherits the job, so when the client kills `loadout exec`
//! (its last handle to the job closes) the whole server tree dies with it.
//! The exit code is forwarded.
//!
//! A command without an extension is looked up with `PATHEXT` on the
//! server's `PATH`, as a shell would, so npm launchers (`npx.cmd`) start.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use win32job::{Job, JobError};

use super::{Failure, PREFIX};

const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// Runs `program` to completion inside the job; returns its exit code.
pub fn run(
    program: &OsStr,
    args: &[OsString],
    env: &[(OsString, OsString)],
) -> Result<i32, Failure> {
    join_kill_on_close_job();
    let resolved = resolve(program, env);
    let status = Command::new(&resolved)
        .args(args)
        .env_clear()
        .envs(env.iter().map(|(name, value)| (name, value)))
        .status()
        .map_err(|err| {
            Failure::config(format!("cannot start {}: {err}", program.to_string_lossy()))
        })?;
    Ok(status.code().unwrap_or(1))
}

/// Puts this process (and so every child) in a new kill-on-close job. The
/// job handle is kept open until the process exits: closing it is what
/// ends the tree.
fn join_kill_on_close_job() {
    let job = || -> Result<Job, JobError> {
        let job = Job::create()?;
        let mut info = job.query_extended_limit_info()?;
        info.limit_kill_on_job_close();
        job.set_extended_limit_info(&info)?;
        job.assign_current_process()?;
        Ok(job)
    };
    match job() {
        Ok(job) => std::mem::forget(job),
        Err(err) => eprintln!(
            "{PREFIX} warning: cannot set up a Job Object ({err}); the server may outlive this \
             process if it is killed"
        ),
    }
}

/// `program` as a file to start: as given when it has a `PATHEXT`
/// extension, else the first `<dir>\<program><ext>` on `PATH` (absolute
/// entries only), else as given.
fn resolve(program: &OsStr, env: &[(OsString, OsString)]) -> OsString {
    let var = |name: &str| {
        env.iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    };
    let extensions: Vec<String> = var("PATHEXT")
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| DEFAULT_PATHEXT.to_owned())
        .split(';')
        .map(str::trim)
        .filter(|ext| ext.starts_with('.'))
        .map(str::to_ascii_lowercase)
        .collect();
    let path = Path::new(program);
    let has_extension = path
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy().to_ascii_lowercase()))
        .is_some_and(|ext| extensions.contains(&ext));
    if has_extension {
        return program.to_owned();
    }
    let with_extensions = |base: &Path| -> Option<PathBuf> {
        extensions
            .iter()
            .map(|ext| {
                let mut name = base.as_os_str().to_owned();
                name.push(ext);
                PathBuf::from(name)
            })
            .find(|candidate| candidate.is_file())
    };
    if path.is_absolute() || path.components().count() > 1 {
        return with_extensions(path).map_or_else(|| program.to_owned(), PathBuf::into_os_string);
    }
    let dirs = var("PATH").unwrap_or_default();
    std::env::split_paths(&dirs)
        .filter(|dir| dir.is_absolute())
        .find_map(|dir| with_extensions(&dir.join(path)))
        .map_or_else(|| program.to_owned(), PathBuf::into_os_string)
}

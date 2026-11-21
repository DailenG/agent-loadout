//! GitHub conveniences (SPEC 9.7, optional, never required): "Create
//! private repository" through `gh repo create <name> --private` when the
//! GitHub CLI is installed and logged in. Any other host works through a
//! plain git remote.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::env::Env;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::remote::git::{Invocation, Runner, locate_program};

/// `gh` on `PATH`.
pub fn locate(env: &Env, fs: &dyn Fs) -> Option<PathBuf> {
    locate_program(env, fs, "gh")
}

fn invocation(env: &Env, gh: &Path, args: &[&str]) -> Invocation {
    Invocation {
        program: gh.to_path_buf(),
        args: args.iter().map(OsString::from).collect(),
        env: env
            .vars()
            .map(|(name, value)| (OsString::from(name), value.to_owned()))
            .collect(),
        cwd: env.home().to_path_buf(),
    }
}

/// Whether `gh auth status` succeeds.
pub fn logged_in(env: &Env, gh: &Path, runner: &dyn Runner) -> bool {
    runner
        .run(&invocation(env, gh, &["auth", "status"]), &mut |_| {})
        .is_ok_and(|output| output.success)
}

/// A repository name: `name` or `owner/name`, each part letters, digits,
/// `.`, `_` or `-`, not starting with `-` or `.`.
pub fn check_repo_name(name: &str) -> Result<(), CoreError> {
    let part_ok = |part: &str| {
        !part.is_empty()
            && part.len() <= 100
            && !part.starts_with(['-', '.'])
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    let valid = match name.split_once('/') {
        Some((owner, repo)) => part_ok(owner) && part_ok(repo),
        None => part_ok(name),
    };
    if valid {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("invalid repository name {name:?}: NAME or OWNER/NAME"),
        ))
    }
}

/// `gh repo create <name> --private`; the new repository's URL.
pub fn create_private_repo(
    env: &Env,
    gh: &Path,
    runner: &dyn Runner,
    name: &str,
) -> Result<String, CoreError> {
    check_repo_name(name)?;
    if !logged_in(env, gh, runner) {
        return Err(CoreError::new(
            ErrorCode::RemoteAuthFailed,
            "the GitHub CLI is not logged in; run `gh auth login` first",
        ));
    }
    let output = runner
        .run(
            &invocation(env, gh, &["repo", "create", name, "--private"]),
            &mut |_| {},
        )
        .map_err(|err| {
            CoreError::new(ErrorCode::ToolNotFound, format!("cannot run gh: {err}")).with_path(gh)
        })?;
    if !output.success {
        let first = output
            .stderr
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("no error message");
        let code = if first.to_lowercase().contains("already exists") {
            ErrorCode::AlreadyExists
        } else {
            ErrorCode::RemoteUnreachable
        };
        return Err(CoreError::new(
            code,
            format!("gh repo create failed: {first}"),
        ));
    }
    output
        .stdout
        .lines()
        .chain(output.stderr.lines())
        .map(str::trim)
        .find(|line| line.starts_with("https://"))
        .map(|url| format!("{}.git", url.trim_end_matches('/').trim_end_matches(".git")))
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::UnexpectedShape,
                "gh repo create did not print the repository URL",
            )
        })
}

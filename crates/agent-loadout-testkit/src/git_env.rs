//! The system `git` for tests of git remotes (SPEC 9.6), isolated from the
//! user's git configuration: `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL`
//! pointing at an empty file in the temp root, `HOME` in the temp root, and
//! a `PATH` of the temp `bin/` plus git's own folder. Nothing is configured
//! with `git config`. Tests skip, with a notice, when git is absent.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use agent_loadout_core::EnvBuilder;

use crate::TempHome;

/// The `git` executable on the real `PATH`, if any.
pub fn system_git() -> Option<PathBuf> {
    let name = format!("git{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&std::env::var_os("PATH")?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(&name))
        .find(|candidate| candidate.is_file())
}

/// [`system_git`], or `None` after printing why the test is skipped.
pub fn system_git_or_skip(test: &str) -> Option<PathBuf> {
    let git = system_git();
    if git.is_none() {
        eprintln!("notice: {test} skipped: git is not installed or not on PATH");
    }
    git
}

/// The variables a git started by a test gets besides `HOME` and the
/// profile folders: `PATH` (`bin/` first, then git's folder), the config
/// isolation, and `SYSTEMROOT` on Windows.
pub fn git_vars(th: &TempHome, git: &Path) -> Vec<(&'static str, OsString)> {
    let global = th.root().join("gitconfig");
    if !global.exists() {
        std::fs::write(&global, "").expect("write the empty git config");
    }
    let git_dir = git.parent().expect("git's folder").to_path_buf();
    let path = std::env::join_paths([th.bin(), git_dir]).expect("join PATH");
    let mut vars = vec![
        ("PATH", path),
        ("GIT_CONFIG_NOSYSTEM", OsString::from("1")),
        ("GIT_CONFIG_GLOBAL", global.into_os_string()),
        ("GIT_AUTHOR_NAME", OsString::from("test")),
        ("GIT_AUTHOR_EMAIL", OsString::from("test@example.invalid")),
        ("GIT_COMMITTER_NAME", OsString::from("test")),
        (
            "GIT_COMMITTER_EMAIL",
            OsString::from("test@example.invalid"),
        ),
    ];
    if cfg!(windows)
        && let Some(root) = std::env::var_os("SYSTEMROOT")
    {
        vars.push(("SYSTEMROOT", root));
    }
    vars
}

/// `builder` with [`git_vars`] and `HOME`/`USERPROFILE` in the temp root.
pub fn with_git_vars(th: &TempHome, git: &Path, mut builder: EnvBuilder) -> EnvBuilder {
    builder = builder.var("HOME", th.home()).var("USERPROFILE", th.home());
    for (name, value) in git_vars(th, git) {
        builder = builder.var(name, value);
    }
    builder
}

/// `git init --bare <path>` with the isolated environment.
pub fn init_bare(th: &TempHome, git: &Path, path: &Path) {
    let status = th
        .command(git)
        .envs(git_vars(th, git))
        .args(["init", "--bare", "--quiet", "--initial-branch=main"])
        .arg(path)
        .status()
        .expect("run git init");
    assert!(status.success(), "git init --bare failed");
}

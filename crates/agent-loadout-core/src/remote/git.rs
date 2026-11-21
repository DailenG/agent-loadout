//! Git remotes (SPEC 9.6, 9.8): any git URL, through the system `git` CLI
//! run in the store folder. The URL is passed to git unchanged; Loadout
//! has no host-specific code paths besides the optional `gh` helper.
//!
//! ```text
//! git [auth flags] push --progress <url> refs/heads/main:refs/heads/main 'refs/tags/*:refs/tags/*'
//! git [auth flags] fetch --progress <url> +refs/heads/main:refs/remotes/<name>/main '+refs/tags/*:refs/tags/*'
//! ```
//!
//! Auth (`auth` in `state/remotes.toml`):
//!
//! - `system` (default): no flags; whatever the user's git does.
//! - `gh`: `-c credential.helper= -c 'credential.helper=!gh auth git-credential'`.
//! - `token`: `-c credential.helper= -c 'credential.helper=!"<loadout>" git-credential --remote <name>'`;
//!   the token comes from the OS keyring through [`super::credential`].
//!
//! No token is ever put in git's environment or arguments. Git's own
//! prompts are off (`GIT_TERMINAL_PROMPT=0`; stdin is closed anyway);
//! credential managers with their own windows still work. A git that
//! prints nothing for [`IDLE_TIMEOUT`] is stopped.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::env::{Env, Os};
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::remote::config::{RemoteConfig, RemoteKind};
use crate::remote::folder::{PullReport, SiblingPull};
use crate::store::merge::{MergeOutcome, PendingMerge, RecordedConflict, write_merge_state};
use crate::store::{CommitId, LOCK_TIMEOUT, Store, StoreLock};

/// A git that prints nothing for this long is stopped (SPEC 9.6).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// How a git remote authenticates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GitAuth {
    /// The user's own git setup (credential manager, SSH agent, ...).
    #[default]
    System,
    /// `gh auth git-credential`.
    Gh,
    /// A token from the OS keyring through `loadout git-credential`.
    Token,
}

impl GitAuth {
    pub const fn as_str(self) -> &'static str {
        match self {
            GitAuth::System => "system",
            GitAuth::Gh => "gh",
            GitAuth::Token => "token",
        }
    }

    /// `auth` as configured; absent means `system`.
    pub fn parse(text: Option<&str>) -> Result<Self, CoreError> {
        match text {
            None | Some("system") => Ok(Self::System),
            Some("gh") => Ok(Self::Gh),
            Some("token") => Ok(Self::Token),
            Some(other) => Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!("unknown auth {other:?}; expected system, gh or token"),
            )),
        }
    }
}

/// Whether `url` points at github.com (where `auth = gh` is offered).
pub fn is_github_url(url: &str) -> bool {
    let host = url_host(url).unwrap_or_default();
    host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("www.github.com")
}

/// The scheme and host (with port) of `url`: `https://host:port/path`,
/// `ssh://user@host/path` or scp-style `user@host:path` (scheme `ssh`).
/// `None` for local paths and `file://`.
pub fn url_scheme_host(url: &str) -> Option<(String, String)> {
    if let Some((scheme, rest)) = url.split_once("://") {
        if scheme.eq_ignore_ascii_case("file") {
            return None;
        }
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        let host = authority.rsplit('@').next().unwrap_or_default();
        return (!host.is_empty())
            .then(|| (scheme.to_ascii_lowercase(), host.to_ascii_lowercase()));
    }
    // scp-style: [user@]host:path, where host has no '/' and is not a
    // drive letter.
    let (before, _) = url.split_once(':')?;
    if before.contains('/') || before.contains('\\') || before.len() < 2 {
        return None;
    }
    let host = before.rsplit('@').next().unwrap_or_default();
    (!host.is_empty()).then(|| ("ssh".to_owned(), host.to_ascii_lowercase()))
}

fn url_host(url: &str) -> Option<String> {
    url_scheme_host(url).map(|(_, host)| {
        host.rsplit_once(':')
            .map_or(host.clone(), |(name, _)| name.to_owned())
    })
}

/// `git` on `PATH`. Relative `PATH` entries are skipped, so a project
/// folder cannot shadow it.
pub fn locate(env: &Env, fs: &dyn Fs) -> Option<PathBuf> {
    locate_program(env, fs, "git")
}

/// An executable on `PATH` (`.exe` added on Windows).
pub(crate) fn locate_program(env: &Env, fs: &dyn Fs, name: &str) -> Option<PathBuf> {
    let (separator, file) = match env.os() {
        Os::Windows => (';', format!("{name}.exe")),
        Os::Linux | Os::MacOs => (':', name.to_owned()),
    };
    let path_var = env.var("PATH")?.to_string_lossy().into_owned();
    path_var
        .split(separator)
        .map(|dir| PathBuf::from(dir.trim().trim_matches('"')))
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(&file))
        .find(|candidate| fs.exists(candidate) && !fs.is_dir(candidate))
}

/// One process to run: program, arguments, the complete environment and
/// the working directory. Kept as data so tests can check what a git
/// receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub cwd: PathBuf,
}

/// What a finished (or stopped) process left.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    /// Stopped after [`IDLE_TIMEOUT`] without output.
    pub timed_out: bool,
}

/// Runs processes; tests record or script it.
pub trait Runner: Send + Sync {
    /// Runs `invocation`, calling `progress` with each line (or `\r`
    /// update) of its stderr.
    fn run(&self, invocation: &Invocation, progress: &mut dyn FnMut(&str))
    -> io::Result<RunOutput>;
}

/// The real thing: stdin closed, stdout and stderr captured, stopped when
/// silent for `idle_timeout`.
#[derive(Debug, Clone, Copy)]
pub struct SystemRunner {
    pub idle_timeout: Duration,
}

impl Default for SystemRunner {
    fn default() -> Self {
        Self {
            idle_timeout: IDLE_TIMEOUT,
        }
    }
}

impl Runner for SystemRunner {
    fn run(
        &self,
        invocation: &Invocation,
        progress: &mut dyn FnMut(&str),
    ) -> io::Result<RunOutput> {
        let mut child = Command::new(&invocation.program)
            .args(&invocation.args)
            .env_clear()
            .envs(invocation.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&invocation.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let (sender, receiver) = mpsc::channel::<String>();
        let out_sender = sender.clone();
        let out_thread = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        bytes.extend_from_slice(&buf[..n]);
                        // Output on stdout counts as activity too.
                        let _ = out_sender.send(String::new());
                    }
                }
            }
            bytes
        });
        let err_thread = std::thread::spawn(move || {
            let mut line = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match stderr.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for &byte in &buf[..n] {
                            if byte == b'\n' || byte == b'\r' {
                                let text = String::from_utf8_lossy(&line).into_owned();
                                line.clear();
                                if sender.send(text).is_err() {
                                    return;
                                }
                            } else {
                                line.push(byte);
                            }
                        }
                    }
                }
            }
            if !line.is_empty() {
                let _ = sender.send(String::from_utf8_lossy(&line).into_owned());
            }
        });
        let mut stderr_text = String::new();
        let mut timed_out = false;
        loop {
            match receiver.recv_timeout(self.idle_timeout) {
                Ok(line) => {
                    if !line.is_empty() {
                        progress(&line);
                        stderr_text.push_str(&line);
                        stderr_text.push('\n');
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    timed_out = true;
                    let _ = child.kill();
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let status = child.wait()?;
        let stdout = out_thread.join().unwrap_or_default();
        let _ = err_thread.join();
        // Lines that arrived after a timeout are not needed.
        while let Ok(line) = receiver.try_recv() {
            if !line.is_empty() {
                stderr_text.push_str(&line);
                stderr_text.push('\n');
            }
        }
        Ok(RunOutput {
            success: status.success() && !timed_out,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: stderr_text,
            timed_out,
        })
    }
}

/// How to reach git and, for `auth = token`, this binary.
pub struct GitCli<'a> {
    /// The git executable.
    pub git: PathBuf,
    pub runner: &'a dyn Runner,
    /// This binary, for `auth = token` (`loadout git-credential`).
    pub helper: Option<PathBuf>,
}

impl<'a> GitCli<'a> {
    /// Git from `PATH`; `tool_not_found` without it.
    pub fn from_env(
        env: &Env,
        fs: &dyn Fs,
        runner: &'a dyn Runner,
        helper: Option<PathBuf>,
    ) -> Result<Self, CoreError> {
        let git = locate(env, fs).ok_or_else(|| {
            CoreError::new(
                ErrorCode::ToolNotFound,
                "git is not installed or not on PATH; git remotes need it",
            )
        })?;
        Ok(Self {
            git,
            runner,
            helper,
        })
    }
}

/// The `-c` flags for the remote's `auth`.
pub fn auth_args(
    remote: &RemoteConfig,
    helper: Option<&Path>,
    os: Os,
) -> Result<Vec<OsString>, CoreError> {
    let helper_flag = |value: String| -> Vec<OsString> {
        vec![
            "-c".into(),
            "credential.helper=".into(),
            "-c".into(),
            format!("credential.helper={value}").into(),
        ]
    };
    Ok(match GitAuth::parse(remote.auth.as_deref())? {
        GitAuth::System => Vec::new(),
        GitAuth::Gh => helper_flag("!gh auth git-credential".to_owned()),
        GitAuth::Token => {
            let exe = helper.ok_or_else(|| {
                CoreError::new(
                    ErrorCode::InvalidValue,
                    "auth = token needs the loadout binary as the credential helper",
                )
            })?;
            // Git runs `!` helpers through a POSIX shell (Git for Windows'
            // sh too): forward slashes, inside double quotes.
            let mut path = exe.to_string_lossy().into_owned();
            if os == Os::Windows {
                path = path.replace('\\', "/");
            }
            if path.contains(['"', '$', '`', '\\']) {
                return Err(CoreError::new(
                    ErrorCode::InvalidPath,
                    "the loadout binary's path cannot be quoted for git",
                )
                .with_path(exe));
            }
            helper_flag(format!(
                "!\"{path}\" git-credential --remote {}",
                remote.name
            ))
        }
    })
}

fn url(remote: &RemoteConfig) -> Result<&str, CoreError> {
    if remote.kind != RemoteKind::Git {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{} is not a git remote", remote.name),
        ));
    }
    remote
        .url
        .as_deref()
        .filter(|u| !u.is_empty())
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::MissingField,
                format!("git remote {} has no url", remote.name),
            )
        })
}

fn invocation(store: &Store<'_>, git: &GitCli<'_>, args: Vec<OsString>) -> Invocation {
    let mut env: Vec<(OsString, OsString)> = store
        .env()
        .vars()
        .map(|(name, value)| (OsString::from(name), value.to_owned()))
        .collect();
    env.retain(|(name, _)| name != OsStr::new("GIT_TERMINAL_PROMPT"));
    env.push(("GIT_TERMINAL_PROMPT".into(), "0".into()));
    Invocation {
        program: git.git.clone(),
        args,
        env,
        cwd: store.root().to_path_buf(),
    }
}

/// Maps a failed git run to an error code by its message
/// [verify: wording per git version and host].
fn failure(remote: &RemoteConfig, action: &str, output: &RunOutput) -> CoreError {
    let lower = output.stderr.to_lowercase();
    let first = output
        .stderr
        .lines()
        .map(str::trim)
        .rfind(|line| {
            line.starts_with("fatal:") || line.starts_with("error:") || line.contains("[rejected]")
        })
        .or_else(|| {
            output
                .stderr
                .lines()
                .map(str::trim)
                .rfind(|l| !l.is_empty())
        })
        .unwrap_or("no error message");
    let first: String = first.chars().take(300).collect();
    let code = if output.timed_out {
        ErrorCode::RemoteUnreachable
    } else if lower.contains("[rejected]")
        || lower.contains("non-fast-forward")
        || lower.contains("fetch first")
        || lower.contains("failed to push some refs")
    {
        ErrorCode::RemoteRejected
    } else if lower.contains("authentication failed")
        || lower.contains("permission denied")
        || lower.contains("could not read username")
        || lower.contains("could not read password")
        || lower.contains("terminal prompts disabled")
        || lower.contains("403")
        || lower.contains("401")
    {
        ErrorCode::RemoteAuthFailed
    } else {
        ErrorCode::RemoteUnreachable
    };
    let message = if output.timed_out {
        format!(
            "git {action} to {} printed nothing for {} s and was stopped",
            remote.name,
            IDLE_TIMEOUT.as_secs()
        )
    } else {
        format!("git {action} with {} failed: {first}", remote.name)
    };
    CoreError::new(code, message)
}

fn run(
    store: &Store<'_>,
    git: &GitCli<'_>,
    remote: &RemoteConfig,
    action: &str,
    args: Vec<OsString>,
    progress: &mut dyn FnMut(&str),
) -> Result<RunOutput, CoreError> {
    let invocation = invocation(store, git, args);
    let output = git.runner.run(&invocation, progress).map_err(|err| {
        CoreError::new(ErrorCode::ToolNotFound, format!("cannot run git: {err}"))
            .with_path(&git.git)
    })?;
    if output.success {
        Ok(output)
    } else {
        Err(failure(remote, action, &output))
    }
}

/// What a push did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitPush {
    pub head: CommitId,
    /// The pull a rejected push ran before its one retry.
    pub pulled: Option<PullReport>,
}

/// `git push` of `main` and every tag. A non-fast-forward rejection pulls
/// (merges) and retries once (SPEC 9.8); a pull with conflicts stops there
/// with `merge_conflict`.
pub fn push(
    store: &Store<'_>,
    git: &GitCli<'_>,
    remote: &RemoteConfig,
    progress: &mut dyn FnMut(&str),
) -> Result<GitPush, CoreError> {
    match push_once(store, git, remote, progress) {
        Ok(head) => Ok(GitPush { head, pulled: None }),
        Err(err) if err.code() == ErrorCode::RemoteRejected => {
            let report = pull(store, git, remote, progress)?;
            if report.conflicts() > 0 {
                return Err(CoreError::new(
                    ErrorCode::MergeConflict,
                    format!(
                        "{} has changes that conflict with this machine's; nothing pushed",
                        remote.name
                    ),
                ));
            }
            let head = push_once(store, git, remote, progress)?;
            Ok(GitPush {
                head,
                pulled: Some(report),
            })
        }
        Err(err) => Err(err),
    }
}

fn push_once(
    store: &Store<'_>,
    git: &GitCli<'_>,
    remote: &RemoteConfig,
    progress: &mut dyn FnMut(&str),
) -> Result<CommitId, CoreError> {
    let url = url(remote)?;
    let head = store.head()?;
    let mut args = auth_args(remote, git.helper.as_deref(), store.env().os())?;
    args.extend(
        [
            "push",
            "--progress",
            url,
            "refs/heads/main:refs/heads/main",
            "refs/tags/*:refs/tags/*",
        ]
        .map(OsString::from),
    );
    run(store, git, remote, "push", args, progress)?;
    Ok(head)
}

/// `git fetch` of `main` into `refs/remotes/<name>/main` and of every tag.
/// `None` when the remote has no `main` yet (a new, empty repository).
pub fn fetch(
    store: &Store<'_>,
    git: &GitCli<'_>,
    remote: &RemoteConfig,
    progress: &mut dyn FnMut(&str),
) -> Result<Option<CommitId>, CoreError> {
    let url = url(remote)?;
    let tracking = format!("refs/remotes/{}/main", remote.name);
    let mut args = auth_args(remote, git.helper.as_deref(), store.env().os())?;
    args.extend(
        [
            "fetch".to_owned(),
            "--progress".to_owned(),
            "--no-write-fetch-head".to_owned(),
            url.to_owned(),
            format!("+refs/heads/main:{tracking}"),
            "+refs/tags/*:refs/tags/*".to_owned(),
        ]
        .map(OsString::from),
    );
    let _lock = StoreLock::acquire(store.fs(), store.state_dir(), LOCK_TIMEOUT)?;
    match run(store, git, remote, "fetch", args, progress) {
        Ok(_) => {}
        Err(err) if is_empty_remote(&err) => return Ok(None),
        Err(err) => return Err(err),
    }
    store.reference_target(&tracking)
}

fn is_empty_remote(err: &CoreError) -> bool {
    let lower = err.message().to_lowercase();
    lower.contains("couldn't find remote ref") || lower.contains("could not find remote ref")
}

/// Fetches and merges the remote's `main` (SPEC 9.9): tagged
/// `auto/pre-pull/<ts>` when it brings anything; conflicts go to
/// `state/merge/MERGE_STATE.json` and leave HEAD as it was.
pub fn pull(
    store: &Store<'_>,
    git: &GitCli<'_>,
    remote: &RemoteConfig,
    progress: &mut dyn FnMut(&str),
) -> Result<PullReport, CoreError> {
    let tag_targets = |store: &Store<'_>| -> Result<BTreeMap<String, CommitId>, CoreError> {
        Ok(store
            .tags()?
            .into_iter()
            .map(|tag| (tag.name, tag.target))
            .collect())
    };
    let before = tag_targets(store)?;
    let incoming = fetch(store, git, remote, progress)?;
    let after = tag_targets(store)?;
    let head_before = store.head()?;
    let mut report = PullReport {
        remote: remote.name.clone(),
        tag: None,
        head_before: head_before.clone(),
        head_after: head_before,
        siblings: Vec::new(),
        artifacts: Vec::new(),
        tags_imported: after
            .keys()
            .filter(|name| !before.contains_key(*name))
            .cloned()
            .collect(),
        // SPEC 9.6 fetches tags with `+`: a same-named tag of the remote
        // replaces ours. Never silently.
        tags_moved: after
            .iter()
            .filter(|(name, target)| before.get(*name).is_some_and(|old| old != *target))
            .map(|(name, _)| name.clone())
            .collect(),
    };
    // Only a fetched head replaces the conflicts recorded for this remote.
    let attempted = if incoming.is_some() {
        vec![remote.name.clone()]
    } else {
        Vec::new()
    };
    let mut pending = Vec::new();
    if let Some(incoming) = incoming {
        if !store.contains(&incoming) {
            report.tag = Some(store.create_auto_tag("pre-pull")?);
        }
        let outcome = store.merge_incoming(&incoming, &remote.name);
        if let Ok(MergeOutcome::Conflicts { base, conflicts }) = &outcome {
            pending.push(PendingMerge {
                source: remote.name.clone(),
                incoming: incoming.to_string(),
                base: base.as_ref().map(ToString::to_string),
                conflicts: conflicts.iter().map(RecordedConflict::from).collect(),
            });
        }
        report.siblings.push(SiblingPull {
            machine_id: None,
            label: remote.name.clone(),
            outcome,
        });
    }
    write_merge_state(store.fs(), store.state_dir(), &attempted, &pending)?;
    report.head_after = store.head()?;
    Ok(report)
}

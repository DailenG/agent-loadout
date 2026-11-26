//! NVIDIA SkillSpector (#30), the optional external scanner since 0.8.0:
//! a local second opinion on a skill before it is installed or updated,
//! or on a library skill on request. Off by default.
//!
//! Loadout never installs it. It runs, in this order of preference
//! ([`detect`]):
//!
//! 1. the pinned uvx form, `uvx --python 3.13 --from skillspector==2.13.7
//!    skillspector ...` (or `uv tool run ...` when only `uv` is on
//!    `PATH`), once [`prepare`] downloaded it into uv's cache; scans pass
//!    uv's `--offline`, so a scan never downloads anything;
//! 2. a `skillspector` the user installed on `PATH` (any version; one
//!    that is not [`VERSION`] gets a visible "not tested" note);
//! 3. uv on `PATH` without the download yet: "needs Prepare".
//!
//! **What leaves the machine** ([`DISCLOSURE`]): only the names and
//! versions of the skill's dependencies, sent to OSV.dev when the skill
//! has a dependency manifest (SkillSpector 2.13.7 has no switch to turn
//! that off), and the one-time download of [`prepare`]. Skill content is
//! never uploaded.
//!
//! One scan ([`Runner::scan_files`]) stages exactly the content that will
//! be written, runs `skillspector scan --no-llm --format json
//! --fail-on-incomplete -- <staged folder>` with stdin closed, a scrubbed
//! environment ([`child_env`]: no provider keys, LangSmith tracing off)
//! and a timeout, kills the whole process tree on timeout or cancel, and
//! maps the JSON report into M40 findings ([`parse`]).

mod adapter;
#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub use adapter::{parse, version_of};

use super::process::{self, Drained, Ended, MAX_STDOUT, RunDir, passed};
use super::{Choice, ExternalReport, NotScanned, Scanner, snyk, text};
use crate::env::{Env, Os};
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::local_settings::{LocalSettings, SkillSpectorSettings};
use crate::paths::LoadoutHome;
use crate::skills::{SkillFiles, content_hash};

/// The `scanner` of SkillSpector's findings and reports.
pub const SCANNER: Scanner = Scanner::SkillSpector;
/// The version this Loadout is tested with; the uvx form pins it.
pub const VERSION: &str = "2.13.7";
/// The uvx form's interpreter (yara-python has no wheels for 3.14+).
pub const PYTHON: &str = "3.13";
/// Seconds per scan unless `[skillspector] timeout_secs` says otherwise.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// The longest a scan may run whatever `timeout_secs` says: a run folder
/// older than an hour is swept as abandoned, so a scan must end before.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(59 * 60);
const _: () = assert!(
    MAX_TIMEOUT.as_secs() < process::STALE_RUN.as_secs()
        && PREPARE_TIMEOUT.as_secs() < process::STALE_RUN.as_secs()
);
/// How long [`prepare`] may take: the first run downloads a few hundred
/// MB (and maybe a Python build).
pub const PREPARE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// How long a `--version` probe may take (a first start compiles
/// Python's bytecode).
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
/// The most a probe's or Prepare's stdout is read.
const PROBE_STDOUT: usize = 64 * 1024;
/// The longest progress line passed on.
const MAX_PROGRESS_LINE: usize = 300;

/// A scan's arguments after the runtime's prefix; the staged folder
/// follows the `--`. Never `--output` (POSIX-only), `--transitive*`,
/// `--inspect-dependencies`, `--use-shipped-baseline`, `--baseline`,
/// `--mcp-registry*`, `--yara-rules-dir`, `--recursive`, a URL or an LLM
/// flag.
pub const SCAN_ARGS: &[&str] = &[
    "scan",
    "--no-llm",
    "--format",
    "json",
    "--fail-on-incomplete",
    "--",
];

/// Variables passed through to SkillSpector (and uv) when set (any case
/// on Windows): what Python and uv need to start, find uv's cache and
/// reach OSV.dev through a proxy. Never a provider key (`OPENAI_*`,
/// `ANTHROPIC_*`, `AWS_*`, `NVIDIA_*`, `GOOGLE_*`, `LANGSMITH_*`,
/// `LANGCHAIN_*`), never `SKILLSPECTOR_*`, nothing else; the temp folder
/// is the run's own ([`TEMP_VARS`]).
pub const PASSED_VARS: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PROGRAMDATA",
    "PROCESSOR_ARCHITECTURE",
    "HOME",
    "USERPROFILE",
    "USERNAME",
    "USER",
    "LOGNAME",
    "APPDATA",
    "LOCALAPPDATA",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "XDG_CONFIG_HOME",
    "LANG",
    "LC_ALL",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
    "UV_CACHE_DIR",
    "UV_PYTHON_INSTALL_DIR",
    "UV_TOOL_DIR",
];

/// Temp-folder variables, always the run's own `tmp` folder (removed with
/// the run).
pub const TEMP_VARS: &[&str] = &["TEMP", "TMP", "TMPDIR"];

/// Always set: UTF-8 output, no colors, and LangSmith tracing off (the
/// scan runs as a LangGraph graph; a user's tracing setting must never
/// send it anywhere).
pub const SET_VARS: &[(&str, &str)] = &[
    ("PYTHONIOENCODING", "utf-8"),
    ("PYTHONUTF8", "1"),
    ("NO_COLOR", "1"),
    ("LANGSMITH_TRACING", "false"),
    ("LANGCHAIN_TRACING_V2", "false"),
];

/// The child's whole environment (after `env_clear`): [`PASSED_VARS`]
/// that are set, [`TEMP_VARS`] set to `tmp`, and [`SET_VARS`].
pub fn child_env(env: &Env, tmp: &Path) -> Vec<(String, OsString)> {
    let mut vars = passed(env, PASSED_VARS);
    for name in TEMP_VARS {
        vars.push(((*name).to_owned(), tmp.as_os_str().to_owned()));
    }
    for (name, value) in SET_VARS {
        vars.push(((*name).to_owned(), (*value).into()));
    }
    vars
}

/// SkillSpector's settings on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    /// Seconds per scan; 0 or unset is [`DEFAULT_TIMEOUT`], more than
    /// [`MAX_TIMEOUT`] is [`MAX_TIMEOUT`].
    pub timeout_secs: Option<u64>,
}

impl Settings {
    /// The effective scan timeout (see [`Settings::timeout_secs`]).
    pub fn timeout(&self) -> Duration {
        self.timeout_secs
            .filter(|secs| *secs > 0)
            .map_or(DEFAULT_TIMEOUT, Duration::from_secs)
            .min(MAX_TIMEOUT)
    }
}

pub fn settings(fs: &dyn Fs, home: &LoadoutHome) -> Result<Settings, CoreError> {
    let saved = LocalSettings::load(fs, &home.state_dir())?
        .skillspector
        .unwrap_or_default();
    Ok(Settings {
        enabled: saved.enabled == Some(true),
        timeout_secs: saved.timeout_secs,
    })
}

/// Turns SkillSpector on or off on this machine.
pub fn set_enabled(fs: &dyn Fs, home: &LoadoutHome, enabled: bool) -> Result<(), CoreError> {
    let state = home.state_dir();
    let mut settings = LocalSettings::load(fs, &state)?;
    let mut scanner = settings.skillspector.take().unwrap_or_default();
    scanner.enabled = Some(enabled);
    settings.skillspector = (scanner != SkillSpectorSettings::default()).then_some(scanner);
    settings.save(fs, &state)
}

/// Whether to run: [`Choice::On`], or [`Choice::Default`] with SkillSpector
/// turned on.
pub fn wanted(settings: &Settings, choice: Choice) -> bool {
    choice.wants(settings.enabled)
}

/// How SkillSpector is run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeKind {
    /// The pinned uvx form (`uvx`, or `uv tool run`).
    Uvx,
    /// A `skillspector` on `PATH`.
    Path,
}

impl RuntimeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            RuntimeKind::Uvx => "uvx",
            RuntimeKind::Path => "path",
        }
    }
}

/// A SkillSpector that answers: ready to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub kind: RuntimeKind,
    /// `uvx`, `uv` or `skillspector`, absolute.
    pub program: PathBuf,
    /// What it reported (the uvx form: [`VERSION`]).
    pub version: String,
}

impl Runtime {
    /// The version is the one this Loadout is tested with.
    pub fn tested(&self) -> bool {
        self.version == VERSION
    }

    /// "SkillSpector X is not tested with this Loadout (tested: 2.13.7)"
    /// for another version.
    pub fn note(&self) -> Option<String> {
        (!self.tested()).then(|| untested_note(&self.version))
    }

    /// Every argument of a scan of `skill` (the staged folder).
    pub fn argv(&self, skill: &Path) -> Vec<OsString> {
        let mut argv: Vec<OsString> = match self.kind {
            RuntimeKind::Uvx => uvx_prefix(&self.program, true)
                .into_iter()
                .map(OsString::from)
                .collect(),
            RuntimeKind::Path => Vec::new(),
        };
        argv.extend(SCAN_ARGS.iter().map(OsString::from));
        argv.push(skill.as_os_str().to_owned());
        argv
    }

    /// How it is started, for `scanner status`: `uvx --python 3.13 --from
    /// skillspector==2.13.7 skillspector` or the program's path.
    pub fn command_line(&self) -> String {
        let program = self.program.display().to_string();
        match self.kind {
            RuntimeKind::Uvx => {
                let prefix = uvx_prefix(&self.program, false).join(" ");
                format!("{program} {prefix}")
            }
            RuntimeKind::Path => program,
        }
    }
}

/// "SkillSpector X is not tested with this Loadout (tested: 2.13.7)".
pub fn untested_note(version: &str) -> String {
    format!("SkillSpector {version} is not tested with this Loadout (tested: {VERSION})")
}

/// The arguments before SkillSpector's own: `[tool run] [--offline]
/// --python 3.13 --from skillspector==2.13.7 skillspector`. `tool run`
/// when `program` is `uv` rather than `uvx`.
fn uvx_prefix(program: &Path, offline: bool) -> Vec<String> {
    let mut prefix = Vec::new();
    if is_uv(program) {
        prefix.extend(["tool".to_owned(), "run".to_owned()]);
    }
    if offline {
        prefix.push("--offline".to_owned());
    }
    prefix.extend([
        "--python".to_owned(),
        PYTHON.to_owned(),
        "--from".to_owned(),
        format!("skillspector=={VERSION}"),
        "skillspector".to_owned(),
    ]);
    prefix
}

/// `program` is `uv` (not `uvx`).
fn is_uv(program: &Path) -> bool {
    program
        .file_stem()
        .is_some_and(|stem| stem.eq_ignore_ascii_case("uv"))
}

/// Whether SkillSpector can run on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Ready(Runtime),
    /// uv is on `PATH` (`uv`: `uvx`, or `uv` itself) but the pinned
    /// SkillSpector is not in its cache: [`prepare`] downloads it.
    NeedsPrepare {
        uv: PathBuf,
    },
    /// Neither uv nor a usable `skillspector` is on `PATH` ([`SETUP`]).
    NotAvailable,
}

impl Availability {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Availability::Ready(_) => "ready",
            Availability::NeedsPrepare { .. } => "needs_prepare",
            Availability::NotAvailable => "not_available",
        }
    }

    pub fn runtime(&self) -> Option<&Runtime> {
        match self {
            Availability::Ready(runtime) => Some(runtime),
            _ => None,
        }
    }
}

/// The first `PATH` folder holding `name` (on Windows `name.exe`; batch
/// files and other `PATHEXT` kinds are not run). Relative entries are
/// skipped.
fn find_program(env: &Env, name: &str) -> Option<PathBuf> {
    let path = env.var("PATH")?.to_string_lossy().into_owned();
    let windows = env.os() == Os::Windows;
    let separator = if windows { ';' } else { ':' };
    let file = if windows {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    path.split(separator)
        .map(|dir| Path::new(dir.trim().trim_matches('"')))
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(&file))
        .find(|candidate| candidate.is_file())
}

/// `uvx`, else `uv`, on `PATH` (what [`prepare`] runs).
pub fn find_uv(env: &Env) -> Option<PathBuf> {
    find_program(env, "uvx").or_else(|| find_program(env, "uv"))
}

/// Runs `program args...` once, bounded, in a run folder of its own with
/// the scrubbed environment; its stdout when it exits 0.
fn probe(
    env: &Env,
    home: &LoadoutHome,
    program: &Path,
    args: &[String],
    timeout: Duration,
) -> Result<String, String> {
    let run = process::stage(home, &SkillFiles::new())?;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(&run.dir)
        .env_clear()
        .envs(child_env(env, &run.tmp));
    let executed = process::execute(command, timeout, None, PROBE_STDOUT, None)?;
    match executed.ended {
        Ended::Exited(Some(0)) => Ok(executed
            .stdout
            .map(|drained| String::from_utf8_lossy(&drained.kept).into_owned())
            .unwrap_or_default()),
        Ended::Exited(code) => Err(format!(
            "exit code {}: {}",
            code.map_or_else(|| "none".to_owned(), |code| code.to_string()),
            executed.last_stderr_line().unwrap_or_default()
        )),
        Ended::TimedOut => Err(format!("no answer in {} seconds", timeout.as_secs())),
        Ended::Canceled => Err("canceled".to_owned()),
        Ended::Failed(why) => Err(why),
    }
}

/// Which SkillSpector runs here ([`Availability`]). No network: the uvx
/// probe runs with `--offline`. Starts up to two `--version` probes.
pub fn detect(env: &Env, home: &LoadoutHome) -> Availability {
    let uv = find_uv(env);
    if let Some(uv) = &uv {
        let mut args = uvx_prefix(uv, true);
        args.push("--version".to_owned());
        if let Ok(stdout) = probe(env, home, uv, &args, PROBE_TIMEOUT) {
            return Availability::Ready(Runtime {
                kind: RuntimeKind::Uvx,
                program: uv.clone(),
                version: version_of(&stdout).unwrap_or_else(|| VERSION.to_owned()),
            });
        }
    }
    if let Some(program) = find_program(env, "skillspector")
        && let Ok(stdout) = probe(
            env,
            home,
            &program,
            &["--version".to_owned()],
            PROBE_TIMEOUT,
        )
        && let Some(version) = version_of(&stdout)
    {
        return Availability::Ready(Runtime {
            kind: RuntimeKind::Path,
            program,
            version,
        });
    }
    match uv {
        Some(uv) => Availability::NeedsPrepare { uv },
        None => Availability::NotAvailable,
    }
}

/// What `scanner status` and Settings show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub availability: Availability,
    pub enabled: bool,
    pub timeout: Duration,
    /// A 0.7.0 Snyk Agent Scan install or its settings remain (`loadout
    /// scanner remove` deletes them).
    pub snyk_installed: bool,
}

pub fn status(env: &Env, fs: &dyn Fs, home: &LoadoutHome) -> Result<Status, CoreError> {
    let settings = settings(fs, home)?;
    let snyk_settings = LocalSettings::load(fs, &home.state_dir())?
        .snyk_agent_scan
        .is_some();
    let snyk_tool = snyk::Tool::pinned(home).is_ok_and(|tool| tool.installed());
    Ok(Status {
        availability: detect(env, home),
        enabled: settings.enabled,
        timeout: settings.timeout(),
        snyk_installed: snyk_settings || snyk_tool,
    })
}

/// Whether an install or update must carry an external report: SkillSpector
/// is on and ready (a not-scanned report still counts as a report).
pub fn required(env: &Env, fs: &dyn Fs, home: &LoadoutHome) -> Result<bool, CoreError> {
    if !settings(fs, home)?.enabled {
        return Ok(false);
    }
    Ok(matches!(detect(env, home), Availability::Ready(_)))
}

/// Downloads the pinned SkillSpector into uv's cache: runs `uvx --python
/// 3.13 --from skillspector==2.13.7 skillspector --version` once (PyPI,
/// and a Python 3.13 build when uv has none), at most [`PREPARE_TIMEOUT`].
/// `progress` gets each line uv prints (one line, control characters
/// removed). `scanner_unavailable` without uv on `PATH`;
/// `scanner_download_failed` when uv fails or takes too long; `canceled`
/// once `cancel` is set (the process tree is killed).
pub fn prepare(
    env: &Env,
    home: &LoadoutHome,
    progress: &mut dyn FnMut(&str),
    cancel: Option<&AtomicBool>,
) -> Result<Runtime, CoreError> {
    let Some(uv) = find_uv(env) else {
        return Err(CoreError::new(
            ErrorCode::ScannerUnavailable,
            format!("uv is not on PATH, so there is nothing to prepare. {SETUP}"),
        ));
    };
    let failed = |why: String| CoreError::new(ErrorCode::ScannerDownloadFailed, why);
    let run = process::stage(home, &SkillFiles::new())
        .map_err(|why| failed(format!("cannot prepare SkillSpector: {why}")))?;
    let mut args = uvx_prefix(&uv, false);
    args.push("--version".to_owned());
    let mut command = Command::new(&uv);
    command
        .args(&args)
        .current_dir(&run.dir)
        .env_clear()
        .envs(child_env(env, &run.tmp));
    let mut on_line = |line: &str| {
        let line = text::clean(line, MAX_PROGRESS_LINE);
        if !line.is_empty() {
            progress(&line);
        }
    };
    let executed = process::execute(
        command,
        PREPARE_TIMEOUT,
        cancel,
        PROBE_STDOUT,
        Some(&mut on_line),
    )
    .map_err(failed)?;
    match executed.ended {
        Ended::Exited(Some(0)) => {
            let stdout = executed
                .stdout
                .map(|drained| String::from_utf8_lossy(&drained.kept).into_owned())
                .unwrap_or_default();
            Ok(Runtime {
                kind: RuntimeKind::Uvx,
                program: uv,
                version: version_of(&stdout).unwrap_or_else(|| VERSION.to_owned()),
            })
        }
        Ended::Exited(code) => {
            let how = code.map_or_else(
                || "uv was stopped by the system".to_owned(),
                |code| format!("uv exited with code {code}"),
            );
            let detail = executed
                .last_stderr_line()
                .map(|line| format!(": {}", text::clean(&line, MAX_PROGRESS_LINE)))
                .unwrap_or_default();
            Err(failed(format!(
                "uv could not download SkillSpector {VERSION} ({how}){detail}"
            )))
        }
        Ended::TimedOut => Err(failed(format!(
            "preparing SkillSpector did not finish in {} minutes",
            PREPARE_TIMEOUT.as_secs() / 60
        ))),
        Ended::Canceled => Err(CoreError::new(
            ErrorCode::Canceled,
            "preparing SkillSpector was canceled",
        )),
        Ended::Failed(why) => Err(failed(why)),
    }
}

/// A ready run, or the `not_scanned` report every skill of the command
/// gets: `not_available` (nothing on `PATH`) or `not_prepared` (uv without
/// the download).
// The `Err` is the report every skill of the command gets, built once per
// command, not an error on a hot path.
#[allow(clippy::result_large_err)]
pub fn prepare_run(env: &Env, fs: &dyn Fs, home: &LoadoutHome) -> Result<Runner, ExternalReport> {
    let timeout = settings(fs, home)
        .map_err(|err| {
            ExternalReport::not_scanned(
                SCANNER,
                VERSION,
                NotScanned::Failed,
                Some(err.message().to_owned()),
            )
        })?
        .timeout();
    match detect(env, home) {
        Availability::Ready(runtime) => Ok(Runner::new(runtime, timeout)),
        Availability::NeedsPrepare { .. } => Err(ExternalReport::not_scanned(
            SCANNER,
            VERSION,
            NotScanned::NotPrepared,
            Some(format!(
                "uv is on PATH but SkillSpector {VERSION} is not downloaded yet; prepare it \
                 first (`loadout scanner prepare`, or Prepare in Settings > Security)"
            )),
        )),
        Availability::NotAvailable => Err(ExternalReport::not_scanned(
            SCANNER,
            VERSION,
            NotScanned::NotAvailable,
            Some("neither uv nor skillspector is on PATH".to_owned()),
        )),
    }
}

/// A SkillSpector that answered: ready to scan.
#[derive(Debug, Clone)]
pub struct Runner {
    runtime: Runtime,
    timeout: Duration,
}

impl Runner {
    pub fn new(runtime: Runtime, timeout: Duration) -> Self {
        Self { runtime, timeout }
    }

    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    /// Scans `files` (a skill, by relative `/`-separated path): one run,
    /// at most the timeout. Never an error: what went wrong is the
    /// report's `not_scanned` reason.
    pub fn scan_files(
        &self,
        env: &Env,
        home: &LoadoutHome,
        files: &SkillFiles,
        cancel: Option<&AtomicBool>,
    ) -> ExternalReport {
        let hash = content_hash(files);
        let mut report = match process::stage(home, files) {
            // The run folder goes when `run` drops.
            Ok(run) => self.run(env, &run, cancel),
            Err(why) => ExternalReport::not_scanned(
                SCANNER,
                &self.runtime.version,
                NotScanned::Failed,
                Some(format!(
                    "the skill could not be prepared for the scanner: {why}"
                )),
            ),
        };
        report.content_hash = Some(hash);
        report
    }

    fn run(&self, env: &Env, run: &RunDir, cancel: Option<&AtomicBool>) -> ExternalReport {
        let version = self.runtime.version.as_str();
        let not_scanned = |reason, detail: Option<String>| {
            ExternalReport::not_scanned(SCANNER, version, reason, detail)
        };
        let mut command = Command::new(&self.runtime.program);
        command
            .args(self.runtime.argv(&run.skill))
            .current_dir(&run.dir)
            .env_clear()
            .envs(child_env(env, &run.tmp));
        let executed = match process::execute(command, self.timeout, cancel, MAX_STDOUT, None) {
            Ok(executed) => executed,
            Err(why) => return not_scanned(NotScanned::Failed, Some(why)),
        };
        match executed.ended {
            Ended::Exited(code) => match executed.stdout {
                Some(Drained { overflow: true, .. }) if matches!(code, Some(0 | 1)) => {
                    ExternalReport::not_understood(
                        SCANNER,
                        version,
                        "the output is larger than 16 MiB",
                    )
                }
                Some(Drained { kept, .. }) => {
                    parse(&kept, &executed.stderr, code, &run.skill, version)
                }
                None => not_scanned(
                    NotScanned::Failed,
                    Some("the scanner's output did not end after it exited".to_owned()),
                ),
            },
            Ended::TimedOut => not_scanned(
                NotScanned::Timeout,
                Some(format!(
                    "SkillSpector did not finish in {} seconds",
                    self.timeout.as_secs()
                )),
            ),
            Ended::Canceled => not_scanned(NotScanned::Canceled, None),
            Ended::Failed(why) => not_scanned(NotScanned::Failed, Some(why)),
        }
    }
}

/// What leaves the machine, verbatim as `docs/security.md` lists it; the
/// CLI prints it when SkillSpector is turned on and in `scanner status`.
pub const DISCLOSURE: &str = "SkillSpector runs on this computer and never uploads the skill. \
Nothing leaves this machine except the names and versions of the skill's dependencies, sent to \
OSV.dev (api.osv.dev) to look up known vulnerabilities, when the skill has a dependency manifest \
(requirements.txt, package.json, pyproject.toml or Pipfile). Preparing the uvx form once \
downloads SkillSpector 2.13.7 and its dependencies (a few hundred MB) from PyPI, and a Python \
3.13 build when uv has none; scans themselves never download anything. No account, no token, no \
upload of skill content. Loadout runs it only on skills, never on MCP servers (SkillSpector does \
not read MCP client configs), and only when you turn it on (or pass --external).";

/// How to get SkillSpector when nothing is found.
pub const SETUP: &str = "Loadout doesn't install SkillSpector. Either install uv \
(https://docs.astral.sh/uv/getting-started/installation/) and run `loadout scanner prepare` (or \
Prepare in Settings > Security), or install SkillSpector yourself with `pipx install \
skillspector==2.13.7 --python 3.13`. It needs Python 3.12 or 3.13: Python 3.14 and later don't \
work yet (yara-python has no wheels for them).";

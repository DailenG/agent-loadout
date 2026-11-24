//! One bounded run of the scanner (#27). The skill's files are written to a
//! fresh folder below `<LOADOUT_HOME>/cache/scan-external/` (the content
//! scanned is exactly the content that will be written), then
//!
//! ```text
//! agent-scan scan --json --storage-file <run>/storage.json <run>/skill
//! ```
//!
//! runs with stdin closed (every consent prompt reads EOF, which declines),
//! a scrubbed environment ([`child_env`]: `SNYK_TOKEN` and the variables a
//! PyInstaller program needs to start and reach the network, nothing else)
//! and a timeout. On timeout or cancel the whole process tree is killed: a
//! kill-on-close Job Object on Windows, the child's own process group on
//! Unix. Never passed: `--ci`, `--verbose`, `--dangerously-run-mcp-servers`,
//! `--scan-all-users`, a prefixed path, or no path. The folder is removed
//! afterwards. The token reaches the child's environment only: never an
//! argument, a file, a log line or the report.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use super::install::{Tool, hex};
use super::{ExternalReport, NotScanned, adapter};
use crate::env::{Env, Os};
use crate::paths::LoadoutHome;
use crate::secrets::SecretValue;
use crate::skills::{SkillFiles, content_hash};

/// `<LOADOUT_HOME>/cache/<RUNS_DIR>/run-<random>/`.
pub const RUNS_DIR: &str = "scan-external";
/// The most stdout kept (the JSON report); more than that is read and
/// dropped, and the run is "not understood".
const MAX_STDOUT: usize = 16 * 1024 * 1024;
/// The end of stderr kept, to tell a token error from another failure.
const STDERR_TAIL: usize = 4 * 1024;
/// How often the run is checked for exit, timeout and cancel.
const POLL: Duration = Duration::from_millis(25);
/// How long the output is waited for once the process tree is killed.
const DRAIN_GRACE: Duration = Duration::from_secs(5);
/// A run folder this old belongs to no live run (the timeout is far
/// shorter): a later run removes it.
const STALE_RUN: Duration = Duration::from_secs(3600);
/// Written when a run folder is created; its age is the run's.
const STARTED: &str = "started";

/// Variables passed through to the scanner when set (any case on Windows).
/// Profile folders (Python finds the user name and home), the system
/// folders Windows programs need, the locale, proxies and CA bundles.
/// Never `MACHINE_SERIAL`, `SNYK_API`, `SNYK_CLI_USE`, `AGENT_SCAN_*`; the
/// temp folder is the run's own ([`TEMP_VARS`]).
pub const PASSED_VARS: &[&str] = &[
    "PATH",
    "SYSTEMROOT",
    "WINDIR",
    "HOME",
    "USERPROFILE",
    "USERNAME",
    "APPDATA",
    "LOCALAPPDATA",
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
];

/// The variable the token is passed in.
pub const TOKEN_VAR: &str = "SNYK_TOKEN";

/// Temp-folder variables, always set to the run's own `tmp` folder: the
/// PyInstaller bootloader unpacks itself there, and a killed run's
/// `_MEI*` folder goes with the run folder instead of piling up in the
/// user's temp folder.
pub const TEMP_VARS: &[&str] = &["TEMP", "TMP", "TMPDIR"];

/// The scanner's environment: [`PASSED_VARS`] that are set, [`TEMP_VARS`]
/// set to `tmp`, the token, and UTF-8 for Python's stdout.
pub fn child_env(env: &Env, token: &SecretValue, tmp: &Path) -> Vec<(String, OsString)> {
    let mut vars: Vec<(String, OsString)> = Vec::new();
    for name in PASSED_VARS {
        let duplicate = env.os() == Os::Windows
            && vars
                .iter()
                .any(|(known, _)| known.eq_ignore_ascii_case(name));
        if duplicate {
            continue;
        }
        if let Some(value) = env.var(name) {
            vars.push(((*name).to_owned(), value.to_owned()));
        }
    }
    for name in TEMP_VARS {
        vars.push(((*name).to_owned(), tmp.as_os_str().to_owned()));
    }
    vars.push(("PYTHONIOENCODING".to_owned(), "utf-8".into()));
    vars.push(("PYTHONUTF8".to_owned(), "1".into()));
    vars.push((TOKEN_VAR.to_owned(), token.expose().into()));
    vars
}

/// A scanner that is installed and has its token: ready to run.
pub struct Runner {
    tool: Tool,
    token: SecretValue,
    timeout: Duration,
}

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner")
            .field("tool", &self.tool)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Runner {
    pub fn new(tool: Tool, token: SecretValue, timeout: Duration) -> Self {
        Self {
            tool,
            token,
            timeout,
        }
    }

    pub fn tool(&self) -> &Tool {
        &self.tool
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
        let mut report = match stage(home, files) {
            Ok(run) => {
                let report = self.run(env, &run, cancel);
                let _ = std::fs::remove_dir_all(&run.dir);
                report
            }
            Err(why) => ExternalReport::not_scanned(
                self.tool.version(),
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
        let version = self.tool.version();
        let not_scanned =
            |reason, detail: Option<String>| ExternalReport::not_scanned(version, reason, detail);
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return not_scanned(NotScanned::Canceled, None);
        }
        let mut command = Command::new(self.tool.binary());
        command
            .arg("scan")
            .arg("--json")
            .arg("--storage-file")
            .arg(&run.storage)
            .arg(&run.skill)
            .current_dir(&run.dir)
            .env_clear()
            .envs(child_env(env, &self.token, &run.tmp))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        tree::prepare(&mut command);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                return not_scanned(
                    NotScanned::Failed,
                    Some(format!("the scanner could not be started: {err}")),
                );
            }
        };
        // Without a way to kill the whole tree, a timeout or cancel could
        // not stop the scanner's own child: it does not run at all.
        let tree = match tree::Tree::of(&child) {
            Ok(tree) => tree,
            Err(why) => {
                let _ = child.kill();
                let _ = child.wait();
                return not_scanned(
                    NotScanned::Failed,
                    Some(format!(
                        "the scanner could not be contained, so it was stopped: {why}"
                    )),
                );
            }
        };
        let stdout = child
            .stdout
            .take()
            .map(|pipe| drain(pipe, Keep::Head(MAX_STDOUT)));
        let stderr = child
            .stderr
            .take()
            .map(|pipe| drain(pipe, Keep::Tail(STDERR_TAIL)));
        let ended = wait(&mut child, self.timeout, cancel);
        // The tree goes in every case: a grandchild left behind would hold
        // the pipes open (and keep running).
        tree.kill(&mut child);
        let _ = child.wait();
        // Bounded: a descendant that escaped the kill and holds a pipe
        // must not hold the command past its timeout.
        let deadline = Instant::now() + DRAIN_GRACE;
        let collect = |reader: Option<Receiver<Drained>>| {
            reader.and_then(|reader| {
                reader
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .ok()
            })
        };
        let stdout = collect(stdout);
        let stderr = collect(stderr).map_or_else(Vec::new, |drained| {
            String::from_utf8_lossy(&drained.kept)
                .replace(self.token.expose(), "<token>")
                .into_bytes()
        });
        match ended {
            Ended::Exited(code) => match stdout {
                Some(Drained { overflow: true, .. }) => {
                    ExternalReport::not_understood(version, "the output is larger than 16 MiB")
                }
                Some(Drained { kept, .. }) => {
                    adapter::parse(&kept, &stderr, code, &run.skill, version)
                }
                None => not_scanned(
                    NotScanned::Failed,
                    Some("the scanner's output did not end after it exited".to_owned()),
                ),
            },
            Ended::TimedOut => not_scanned(
                NotScanned::Timeout,
                Some(format!(
                    "the scanner did not finish in {} seconds",
                    self.timeout.as_secs()
                )),
            ),
            Ended::Canceled => not_scanned(NotScanned::Canceled, None),
            Ended::Failed(why) => not_scanned(NotScanned::Failed, Some(why)),
        }
    }
}

enum Ended {
    Exited(Option<i32>),
    TimedOut,
    Canceled,
    Failed(String),
}

fn wait(child: &mut Child, timeout: Duration, cancel: Option<&AtomicBool>) -> Ended {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ended::Exited(status.code()),
            Ok(None) => {}
            Err(err) => return Ended::Failed(format!("waiting for the scanner failed: {err}")),
        }
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Ended::Canceled;
        }
        if start.elapsed() >= timeout {
            return Ended::TimedOut;
        }
        std::thread::sleep(POLL);
    }
}

/// Which bytes of a pipe are kept.
#[derive(Clone, Copy)]
enum Keep {
    /// The first n bytes.
    Head(usize),
    /// The last n bytes.
    Tail(usize),
}

/// A pipe read to its end.
struct Drained {
    kept: Vec<u8>,
    /// More was written than kept.
    overflow: bool,
}

/// Reads a pipe to its end on a thread; the result arrives on the
/// receiver (never, while a descendant holds the pipe open).
fn drain(mut pipe: impl Read + Send + 'static, keep: Keep) -> Receiver<Drained> {
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut overflow = false;
        let mut buf = [0u8; 16 * 1024];
        while let Ok(n) = pipe.read(&mut buf) {
            if n == 0 {
                break;
            }
            match keep {
                Keep::Head(max) => {
                    let room = max.saturating_sub(kept.len());
                    overflow |= n > room;
                    kept.extend_from_slice(&buf[..n.min(room)]);
                }
                Keep::Tail(max) => {
                    kept.extend_from_slice(&buf[..n]);
                    if kept.len() > max {
                        overflow = true;
                        kept.drain(..kept.len() - max);
                    }
                }
            }
        }
        let _ = send.send(Drained { kept, overflow });
    });
    receive
}

/// The folder of one run.
struct RunDir {
    dir: PathBuf,
    skill: PathBuf,
    storage: PathBuf,
    /// The scanner's temp folder.
    tmp: PathBuf,
}

/// Removes `run-*` folders whose run started more than [`STALE_RUN`] ago
/// (left by a killed process, or a delete Windows refused right after a
/// kill). A folder without a `started` file goes by its own age.
fn sweep_stale_runs(runs: &Path) {
    let Ok(entries) = std::fs::read_dir(runs) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_name().to_string_lossy().starts_with("run-") || !path.is_dir() {
            continue;
        }
        let modified = std::fs::metadata(path.join(STARTED))
            .or_else(|_| entry.metadata())
            .and_then(|meta| meta.modified());
        let stale = modified
            .ok()
            .and_then(|time| time.elapsed().ok())
            .is_some_and(|age| age > STALE_RUN);
        if stale {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Writes `files` below a fresh `cache/scan-external/run-<random>/skill`,
/// with an empty `tmp` beside it, after sweeping stale runs.
fn stage(home: &LoadoutHome, files: &SkillFiles) -> Result<RunDir, String> {
    let runs = crate::paths::plain(&home.cache_dir()).join(RUNS_DIR);
    sweep_stale_runs(&runs);
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).map_err(|err| format!("cannot read random bytes: {err}"))?;
    let dir = runs.join(format!("run-{}", hex(&random)));
    let skill = dir.join("skill");
    let tmp = dir.join("tmp");
    let staged = (|| {
        for folder in [&skill, &tmp] {
            std::fs::create_dir_all(folder)
                .map_err(|err| format!("cannot create a folder: {err}"))?;
        }
        std::fs::write(dir.join(STARTED), b"")
            .map_err(|err| format!("cannot write a file: {err}"))?;
        for (relative, bytes) in files {
            let path = join_relative(&skill, relative)
                .ok_or_else(|| format!("the file name {relative:?} is not a relative path"))?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|err| format!("cannot create a folder: {err}"))?;
            }
            std::fs::write(&path, bytes).map_err(|err| format!("cannot write a file: {err}"))?;
        }
        Ok(())
    })();
    if let Err(why) = staged {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(why);
    }
    Ok(RunDir {
        storage: dir.join("storage.json"),
        skill,
        tmp,
        dir,
    })
}

/// `base` joined with a `/`-separated relative path made of normal
/// components only.
fn join_relative(base: &Path, relative: &str) -> Option<PathBuf> {
    let mut path = base.to_path_buf();
    for part in relative.split('/') {
        let mut components = Path::new(part).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(name)), None) => path.push(name),
            _ => return None,
        }
    }
    Some(path)
}

#[cfg(windows)]
mod tree {
    //! A kill-on-close Job Object holding the child: closing the job's
    //! last handle ends every process in it, grandchildren included. The
    //! child is assigned right after it starts; the PyInstaller
    //! bootloader unpacks itself before it starts its own child, so that
    //! child is in the job.

    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};

    use win32job::Job;

    /// No console window for the child (the app is a GUI process).
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub fn prepare(command: &mut Command) {
        command.creation_flags(CREATE_NO_WINDOW);
    }

    pub struct Tree(Job);

    impl Tree {
        /// The child in a kill-on-close job; the error when the job could
        /// not be created or the child not assigned to it.
        pub fn of(child: &Child) -> Result<Self, String> {
            let job = || -> Result<Job, win32job::JobError> {
                let job = Job::create()?;
                let mut info = job.query_extended_limit_info()?;
                info.limit_kill_on_job_close();
                job.set_extended_limit_info(&info)?;
                job.assign_process(child.as_raw_handle() as isize)?;
                Ok(job)
            };
            job().map(Self).map_err(|err| format!("job object: {err}"))
        }

        pub fn kill(self, child: &mut Child) {
            let _ = child.kill();
            drop(self.0);
        }
    }
}

#[cfg(unix)]
mod tree {
    //! The child leads a process group of its own; the whole group is
    //! killed.

    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    use rustix::process::{Pid, Signal, kill_process_group};

    pub fn prepare(command: &mut Command) {
        command.process_group(0);
    }

    pub struct Tree(Pid);

    impl Tree {
        pub fn of(child: &Child) -> Result<Self, String> {
            Ok(Self(Pid::from_child(child)))
        }

        pub fn kill(self, child: &mut Child) {
            let _ = kill_process_group(self.0, Signal::KILL);
            let _ = child.kill();
        }
    }
}

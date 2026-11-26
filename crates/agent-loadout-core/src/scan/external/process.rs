//! One bounded child process of an external scanner, shared by both
//! scanners: a fresh run folder below `<LOADOUT_HOME>/cache/scan-external/`
//! (the skill's files staged there, exactly the content that will be
//! written, and the child's own temp folder), then the child with stdin
//! closed, its stdout and stderr read on threads (stdout up to a cap, the
//! end of stderr), a timeout and a cancel flag. On timeout or cancel, and
//! in every other case once the child exits, the whole process tree is
//! killed and waited for: a kill-on-close Job Object on Windows, the
//! child's own process group on Unix. The run folder is removed when the
//! [`RunDir`] drops.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use crate::env::{Env, Os};
use crate::paths::LoadoutHome;
use crate::skills::SkillFiles;

/// `<LOADOUT_HOME>/cache/<RUNS_DIR>/run-<random>/`.
pub const RUNS_DIR: &str = "scan-external";
/// The most stdout kept (the JSON report); more than that is read and
/// dropped, and the run is "not understood".
pub(super) const MAX_STDOUT: usize = 16 * 1024 * 1024;
/// The end of stderr kept, to tell one failure from another.
const STDERR_TAIL: usize = 4 * 1024;
/// How often the run is checked for exit, timeout and cancel.
const POLL: Duration = Duration::from_millis(25);
/// How long the output is waited for once the process tree is killed.
const DRAIN_GRACE: Duration = Duration::from_secs(5);
/// A run folder this old belongs to no live run (every timeout is
/// shorter: Prepare's, and a scan's capped below it): a later run removes
/// it.
pub(super) const STALE_RUN: Duration = Duration::from_secs(3600);
/// Written when a run folder is created; its age is the run's.
const STARTED: &str = "started";

/// Lowercase hex of `bytes`.
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The variables of `names` that are set in `env`, once each (Windows
/// names are case-insensitive: the first spelling listed wins).
pub(super) fn passed(env: &Env, names: &[&str]) -> Vec<(String, OsString)> {
    let mut vars: Vec<(String, OsString)> = Vec::new();
    for name in names {
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
    vars
}

/// How a run ended.
pub(super) enum Ended {
    Exited(Option<i32>),
    TimedOut,
    Canceled,
    Failed(String),
}

/// A pipe read to its end.
pub(super) struct Drained {
    pub(super) kept: Vec<u8>,
    /// More was written than kept.
    pub(super) overflow: bool,
}

/// What one run left.
pub(super) struct Executed {
    pub(super) ended: Ended,
    /// `None` when the output did not end in time after the kill.
    pub(super) stdout: Option<Drained>,
    /// The end of stderr ([`STDERR_TAIL`] bytes).
    pub(super) stderr: Vec<u8>,
}

impl Executed {
    /// The last non-empty line of stderr, lossily decoded.
    pub(super) fn last_stderr_line(&self) -> Option<String> {
        String::from_utf8_lossy(&self.stderr)
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned)
    }
}

/// Runs `command` (program, arguments, environment and folder already set)
/// with stdin closed, at most `timeout`. `on_line` gets each stderr line
/// as it arrives (Prepare's progress). `Err` when the child could not be
/// started or contained (a timeout or cancel could not stop its own
/// children). Once this returns, the whole process tree has exited (on
/// Windows within [`DRAIN_GRACE`]), so the run folder can be removed.
pub(super) fn execute(
    mut command: Command,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
    max_stdout: usize,
    mut on_line: Option<&mut dyn FnMut(&str)>,
) -> Result<Executed, String> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
        return Ok(Executed {
            ended: Ended::Canceled,
            stdout: Some(Drained {
                kept: Vec::new(),
                overflow: false,
            }),
            stderr: Vec::new(),
        });
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = tree::Running::spawn(command)?;
    let (line_send, line_receive) = std::sync::mpsc::channel();
    let stdout = child
        .stdout()
        .map(|pipe| drain(pipe, Keep::Head(max_stdout), None));
    let stderr = child.stderr().map(|pipe| {
        let lines = on_line.is_some().then_some(line_send);
        drain(pipe, Keep::Tail(STDERR_TAIL), lines)
    });
    let mut forward = |receive: &Receiver<String>| {
        if let Some(on_line) = on_line.as_mut() {
            while let Ok(line) = receive.try_recv() {
                on_line(&line);
            }
        }
    };
    let start = Instant::now();
    let ended = loop {
        forward(&line_receive);
        match child.try_wait() {
            Ok(Some(status)) => break Ended::Exited(status.code()),
            Ok(None) => {}
            Err(err) => break Ended::Failed(format!("waiting for the scanner failed: {err}")),
        }
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            break Ended::Canceled;
        }
        if start.elapsed() >= timeout {
            break Ended::TimedOut;
        }
        std::thread::sleep(POLL);
    };
    // The tree goes in every case: a grandchild left behind would hold
    // the pipes and the run folder open (and keep running).
    child.kill();
    // Bounded: a descendant that escaped the kill and holds a pipe must
    // not hold the command past its timeout.
    let deadline = Instant::now() + DRAIN_GRACE;
    let collect = |reader: Option<Receiver<Drained>>| {
        reader.and_then(|reader| {
            reader
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .ok()
        })
    };
    let stdout = collect(stdout);
    let stderr = collect(stderr).map_or_else(Vec::new, |drained| drained.kept);
    forward(&line_receive);
    Ok(Executed {
        ended,
        stdout,
        stderr,
    })
}

/// Which bytes of a pipe are kept.
#[derive(Clone, Copy)]
enum Keep {
    /// The first n bytes.
    Head(usize),
    /// The last n bytes.
    Tail(usize),
}

/// Reads a pipe to its end on a thread; the result arrives on the
/// receiver (never, while a descendant holds the pipe open). With `lines`,
/// each complete line (split at `\n` or `\r`) is also sent there.
fn drain(
    mut pipe: impl Read + Send + 'static,
    keep: Keep,
    lines: Option<Sender<String>>,
) -> Receiver<Drained> {
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut overflow = false;
        let mut line = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        while let Ok(n) = pipe.read(&mut buf) {
            if n == 0 {
                break;
            }
            if let Some(lines) = &lines {
                for &byte in &buf[..n] {
                    if byte == b'\n' || byte == b'\r' {
                        if !line.is_empty() {
                            let _ = lines.send(String::from_utf8_lossy(&line).into_owned());
                            line.clear();
                        }
                    } else if line.len() < STDERR_TAIL {
                        line.push(byte);
                    }
                }
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
        if let Some(lines) = &lines
            && !line.is_empty()
        {
            let _ = lines.send(String::from_utf8_lossy(&line).into_owned());
        }
        let _ = send.send(Drained { kept, overflow });
    });
    receive
}

/// The folder of one run, removed when dropped.
pub(super) struct RunDir {
    pub(super) dir: PathBuf,
    /// The staged skill (empty for a run that scans nothing).
    pub(super) skill: PathBuf,
    /// The child's temp folder.
    pub(super) tmp: PathBuf,
}

impl Drop for RunDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Removes `run-*` folders whose run started more than [`STALE_RUN`] ago
/// (left by a Loadout that was itself killed mid-run). A folder without a
/// `started` file goes by its own age.
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

/// A fresh `cache/scan-external/run-<random>/` with `files` written below
/// its `skill` folder and an empty `tmp` beside it, after sweeping stale
/// runs. Each file is created new: names that collide on this file system
/// (`Evil.py` and `evil.py` where case or Unicode normalization is
/// ignored) fail the staging rather than leave a partial copy scanned.
pub(super) fn stage(home: &LoadoutHome, files: &SkillFiles) -> Result<RunDir, String> {
    let runs = crate::paths::plain(&home.cache_dir()).join(RUNS_DIR);
    sweep_stale_runs(&runs);
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).map_err(|err| format!("cannot read random bytes: {err}"))?;
    let dir = runs.join(format!("run-{}", hex(&random)));
    let run = RunDir {
        skill: dir.join("skill"),
        tmp: dir.join("tmp"),
        dir,
    };
    for folder in [&run.skill, &run.tmp] {
        std::fs::create_dir_all(folder).map_err(|err| format!("cannot create a folder: {err}"))?;
    }
    std::fs::write(run.dir.join(STARTED), b"")
        .map_err(|err| format!("cannot write a file: {err}"))?;
    for (relative, bytes) in files {
        let path = join_relative(&run.skill, relative)
            .ok_or_else(|| format!("the file name {relative:?} is not a relative path"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("cannot create a folder: {err}"))?;
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut file| file.write_all(bytes))
            .map_err(|err| match err.kind() {
                std::io::ErrorKind::AlreadyExists => format!(
                    "the file name {relative:?} collides with another file of the skill on this \
                     file system (names that differ only in case or Unicode form)"
                ),
                _ => format!("cannot write a file: {err}"),
            })?;
    }
    Ok(run)
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
    //! A kill-on-close Job Object holding the child and everything it
    //! starts: the child is created suspended, assigned to the job, then
    //! resumed (process-wrap), so not even a process it starts at once
    //! (`uvx` starting `uv`) escapes. A kill terminates the job and waits,
    //! bounded, until no process is left in it (they hold the run folder).

    use std::any::Any;
    use std::io;
    use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus};
    use std::time::Instant;

    use process_wrap::std::{ChildWrapper, CommandWrap, CreationFlags, JobObject, KillOnDrop};

    use super::{DRAIN_GRACE, POLL};

    /// No console window for the child (the app is a GUI process).
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub struct Running(Box<dyn ChildWrapper>);

    impl Running {
        /// Starts `command` inside a new kill-on-close job; the error when
        /// it could not be started or contained (then nothing runs).
        pub fn spawn(command: Command) -> Result<Self, String> {
            let mut wrap = CommandWrap::from(command);
            wrap.wrap(CreationFlags(CREATE_NO_WINDOW))
                .wrap(JobObject)
                .wrap(KillOnDrop);
            wrap.spawn()
                .map(Self)
                .map_err(|err| format!("the scanner could not be started in a job object: {err}"))
        }

        pub fn stdout(&mut self) -> Option<ChildStdout> {
            self.0.stdout().take()
        }

        pub fn stderr(&mut self) -> Option<ChildStderr> {
            self.0.stderr().take()
        }

        /// The child's own exit (not its descendants').
        pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            match native(self.0.as_mut()) {
                Some(child) => child.try_wait(),
                None => self.0.try_wait(),
            }
        }

        /// Terminates the job and waits until it is empty, at most
        /// [`DRAIN_GRACE`]; a process that outlives that is left to the
        /// job's close on another thread, so the caller never hangs.
        pub fn kill(self) {
            let mut child = self.0;
            let _ = child.start_kill();
            let deadline = Instant::now() + DRAIN_GRACE;
            loop {
                // The job's try_wait: the child exited and the job is empty.
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
                    _ => break,
                }
            }
            // KillOnDrop waits for the job without a bound.
            std::thread::spawn(move || drop(child));
        }
    }

    /// The `std` child at the bottom of the wrappers.
    fn native(layer: &mut dyn ChildWrapper) -> Option<&mut Child> {
        if (&*layer as &dyn Any).is::<Child>() {
            return (layer as &mut dyn Any).downcast_mut();
        }
        let next: *const dyn ChildWrapper = layer.inner();
        if std::ptr::addr_eq(next, &raw const *layer) {
            return None;
        }
        native(layer.inner_mut())
    }
}

#[cfg(unix)]
mod tree {
    //! The child leads a process group of its own; the whole group is
    //! killed.

    use std::io;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus};

    use rustix::process::{Pid, Signal, kill_process_group};

    pub struct Running {
        child: Child,
        group: Pid,
    }

    impl Running {
        pub fn spawn(mut command: Command) -> Result<Self, String> {
            command.process_group(0);
            let child = command
                .spawn()
                .map_err(|err| format!("the scanner could not be started: {err}"))?;
            let group = Pid::from_child(&child);
            Ok(Self { child, group })
        }

        pub fn stdout(&mut self) -> Option<ChildStdout> {
            self.child.stdout.take()
        }

        pub fn stderr(&mut self) -> Option<ChildStderr> {
            self.child.stderr.take()
        }

        pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            self.child.try_wait()
        }

        pub fn kill(mut self) {
            let _ = kill_process_group(self.group, Signal::KILL);
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

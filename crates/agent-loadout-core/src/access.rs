//! Write access to client files and folders (SPEC 8.2 `read_only_target`,
//! #4): a folder managed by a separate installer can hold client files
//! this account may read but not write. Loadout shows them, never plans a
//! write into them, and reports a write the OS refuses as
//! `read_only_target` instead of a failure.
//!
//! Two layers:
//! - **The probe** ([`probe`], [`probe_files`]) runs while planning, for the
//!   views only. It never writes: on Windows it opens the file for writing
//!   (`CreateFileW` with `GENERIC_WRITE`, `OPEN_EXISTING`, sharing read,
//!   write and delete, closed at once; never created or truncated) after
//!   checking the read-only attribute, and opens the folder a temp file or
//!   a missing file would be created in with `FILE_ADD_FILE` (or
//!   `FILE_ADD_SUBDIRECTORY` for a missing folder) and backup semantics. On
//!   Unix it asks `access(W_OK)` of the file (links resolved) and
//!   `access(W_OK | X_OK)` of that folder. A sharing violation is not read
//!   only (it stays `file_locked` at write time). Probes run on worker
//!   threads under a time budget ([`PROBE_BUDGET`] per planner run); one
//!   that does not answer in time is [`WriteAccess::Unknown`], which counts
//!   as writable.
//! - **The write-time mapping** ([`write_error`], [`denied`]) is the real
//!   protection, since a probe can race: access denied, a read-only
//!   attribute or a read-only mount at write time becomes
//!   `read_only_target` for that target, and the run goes on.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;

/// Why this account cannot write a file or folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum ReadOnlyKind {
    /// Access denied by the permissions (a Windows ACL, a POSIX ACL).
    Acl,
    /// The read-only attribute (Windows), or no write bit for the owner
    /// (Unix).
    Attribute,
    /// Unix: owned by another user, with no write permission for this one.
    Owner,
    /// A network share that denies writing, a read-only mount or
    /// write-protected media.
    Share,
}

impl ReadOnlyKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            ReadOnlyKind::Acl => "acl",
            ReadOnlyKind::Attribute => "attribute",
            ReadOnlyKind::Owner => "owner",
            ReadOnlyKind::Share => "share",
        }
    }

    /// English detail for messages (the views localize from the kind).
    pub const fn describe(self) -> &'static str {
        match self {
            ReadOnlyKind::Acl => "its permissions deny this account writing",
            ReadOnlyKind::Attribute => "it is marked read only",
            ReadOnlyKind::Owner => "another user owns it and this account may not write it",
            ReadOnlyKind::Share => "it is on a read-only share or mount",
        }
    }
}

impl std::fmt::Display for ReadOnlyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `--json` form of a read-only state: `{"kind": "acl"}`, or `null`.
pub fn read_only_json<S: serde::Serializer>(
    kind: &Option<ReadOnlyKind>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(serde::Serialize)]
    struct ReadOnly {
        kind: ReadOnlyKind,
    }
    serde::Serialize::serialize(&kind.map(|kind| ReadOnly { kind }), serializer)
}

/// What a probe found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WriteAccess {
    Writable,
    ReadOnly(ReadOnlyKind),
    /// The probe could not tell (no answer in time, a sharing violation, a
    /// view that cannot probe). Counts as writable.
    Unknown,
}

impl WriteAccess {
    pub const fn read_only(self) -> Option<ReadOnlyKind> {
        match self {
            WriteAccess::ReadOnly(kind) => Some(kind),
            _ => None,
        }
    }
}

/// A probe a file system view hands out ([`Fs::write_probe`]). It owns what
/// it needs, so it can run on a worker thread that may outlive the
/// planner run waiting for it.
pub type WriteProbe = Arc<dyn Fn(&Path) -> WriteAccess + Send + Sync>;

/// How long one planner run waits for its probes, together.
pub const PROBE_BUDGET: Duration = Duration::from_secs(2);

/// Probes `path` on the real file system: the file when it exists, else
/// the folder it would be created in. Writes nothing.
pub fn probe(path: &Path) -> WriteAccess {
    platform::probe(&crate::fs::long(path))
}

/// The fingerprint of a file's bytes (`None` when it does not exist) that
/// keys cached probe results: the same content hash the plan cache notes.
pub fn fingerprint(bytes: Option<&[u8]>) -> Option<u64> {
    bytes.map(|bytes| {
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        hasher.finish()
    })
}

/// Probe results per file, kept while the file's fingerprint stays the
/// same (the plan cache, M30). One per process in the app; the CLI probes
/// afresh each run. A probe still running when its planner run stopped
/// waiting stores its answer here when it arrives.
#[derive(Debug)]
pub struct ProbeCache {
    budget: Duration,
    entries: Arc<Mutex<HashMap<PathBuf, Cached>>>,
}

#[derive(Debug, Clone, Copy)]
enum Cached {
    Done(Option<u64>, WriteAccess),
    Running(Option<u64>),
}

impl Default for ProbeCache {
    fn default() -> Self {
        Self::with_budget(PROBE_BUDGET)
    }
}

impl ProbeCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// A cache whose planner runs wait `budget` for their probes.
    pub fn with_budget(budget: Duration) -> Self {
        Self {
            budget,
            entries: Arc::default(),
        }
    }

    /// Forgets every result: the next run probes again. Probes still
    /// running stay marked, so a stalled share does not get a new thread
    /// per invalidation, and their late answers are still kept.
    pub fn clear(&self) {
        lock(&self.entries).retain(|_, cached| matches!(cached, Cached::Running(_)));
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Probes each of `files` (path and [`fingerprint`]) through `fs`'s
/// [`Fs::write_probe`], in parallel, waiting at most the cache's budget
/// together; a cached result for the same fingerprint is reused. Every
/// path gets an answer: [`WriteAccess::Unknown`] when the view cannot
/// probe or the probe did not answer in time.
pub fn probe_files(
    fs: &dyn Fs,
    files: &[(PathBuf, Option<u64>)],
    cache: Option<&ProbeCache>,
) -> HashMap<PathBuf, WriteAccess> {
    let mut out: HashMap<PathBuf, WriteAccess> = HashMap::new();
    let Some(probe) = fs.write_probe() else {
        for (path, _) in files {
            out.insert(path.clone(), WriteAccess::Unknown);
        }
        return out;
    };
    let local;
    let cache = match cache {
        Some(cache) => cache,
        None => {
            local = ProbeCache::new();
            &local
        }
    };
    let (tx, rx) = mpsc::channel::<(PathBuf, WriteAccess)>();
    let mut running: Vec<(PathBuf, thread::JoinHandle<()>)> = Vec::new();
    {
        let mut entries = lock(&cache.entries);
        for (path, print) in files {
            if out.contains_key(path) || running.iter().any(|(p, _)| p == path) {
                continue;
            }
            match entries.get(path) {
                Some(Cached::Done(seen, access)) if seen == print => {
                    out.insert(path.clone(), *access);
                    continue;
                }
                Some(Cached::Running(seen)) if seen == print => {
                    out.insert(path.clone(), WriteAccess::Unknown);
                    continue;
                }
                _ => {}
            }
            entries.insert(path.clone(), Cached::Running(*print));
            let (probe, tx, shared) = (probe.clone(), tx.clone(), cache.entries.clone());
            let (owned, print) = (path.clone(), *print);
            let spawned = thread::Builder::new()
                .name("loadout-probe".to_owned())
                .spawn(move || {
                    let access = probe(&owned);
                    {
                        let mut entries = lock(&shared);
                        if matches!(entries.get(&owned), Some(Cached::Running(seen)) if *seen == print)
                        {
                            entries.insert(owned.clone(), Cached::Done(print, access));
                        }
                    }
                    let _ = tx.send((owned, access));
                });
            match spawned {
                Ok(handle) => running.push((path.clone(), handle)),
                Err(_) => {
                    entries.remove(path);
                    out.insert(path.clone(), WriteAccess::Unknown);
                }
            }
        }
    }
    drop(tx);
    let deadline = Instant::now() + cache.budget;
    let mut left = running.len();
    while left > 0 {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        match rx.recv_timeout(deadline - now) {
            Ok((path, access)) => {
                out.insert(path, access);
                left -= 1;
            }
            Err(_) => break,
        }
    }
    for (path, handle) in running {
        if out.contains_key(&path) {
            continue;
        }
        // A probe that panicked (a confined test view refusing a path)
        // fails the run as it would without the thread.
        if handle.is_finished()
            && let Err(panic) = handle.join()
        {
            lock(&cache.entries).remove(&path);
            std::panic::resume_unwind(panic);
        }
        out.insert(path, WriteAccess::Unknown);
    }
    out
}

/// [`probe_files`] for one path, without a cache: the file, or the folder
/// it would be created in.
pub fn probe_path(fs: &dyn Fs, path: &Path) -> WriteAccess {
    probe_files(fs, &[(path.to_path_buf(), None)], None)
        .remove(path)
        .unwrap_or(WriteAccess::Unknown)
}

/// The kind of read-only target `err` reports for a write at `path`, or
/// `None` when the error says something else (a sharing violation stays
/// `file_locked`, other errors stay I/O errors). For access denied, the
/// write probe of `path` through `fs` tells the kind; when it finds `path`
/// writable the denial was about something else (a privilege, a file being
/// deleted) and is not read only. A view that cannot probe, or a probe that
/// cannot tell, gives `acl` (`share` for a UNC path).
pub fn denied(fs: &dyn Fs, err: &io::Error, path: &Path) -> Option<ReadOnlyKind> {
    if cfg!(windows) && matches!(err.raw_os_error(), Some(32 | 33)) {
        return None;
    }
    if err.kind() == io::ErrorKind::ReadOnlyFilesystem
        || (cfg!(windows) && matches!(err.raw_os_error(), Some(19 | 65)))
    {
        return Some(ReadOnlyKind::Share);
    }
    if err.kind() != io::ErrorKind::PermissionDenied {
        return None;
    }
    let fallback = if is_unc(path) {
        ReadOnlyKind::Share
    } else {
        ReadOnlyKind::Acl
    };
    match fs.write_probe().map(|probe| probe(path)) {
        Some(WriteAccess::ReadOnly(kind)) => Some(kind),
        Some(WriteAccess::Writable) => None,
        Some(WriteAccess::Unknown) | None => Some(fallback),
    }
}

/// `read_only_target` for `path`, carrying `kind`.
pub fn read_only_target(kind: ReadOnlyKind, path: &Path) -> CoreError {
    CoreError::new(
        ErrorCode::ReadOnlyTarget,
        format!(
            "this account cannot write here ({}); Loadout leaves it as it is",
            kind.describe()
        ),
    )
    .with_path(path)
    .with_read_only(kind)
}

/// The error of a failed write at `path`: `read_only_target` when the OS
/// refused it as read only ([`denied`]), else `io_error` (`what: err`).
pub fn write_error(fs: &dyn Fs, what: &str, path: &Path, err: &io::Error) -> CoreError {
    match denied(fs, err, path) {
        Some(kind) => read_only_target(kind, path),
        None => CoreError::new(ErrorCode::IoError, format!("{what}: {err}")).with_path(path),
    }
}

/// Whether `path` names a file on a network share by its UNC name.
fn is_unc(path: &Path) -> bool {
    if !cfg!(windows) {
        return false;
    }
    let text = path.as_os_str().to_string_lossy();
    text.starts_with(r"\\?\UNC\")
        || (text.starts_with(r"\\") && !text.starts_with(r"\\?\") && !text.starts_with(r"\\.\"))
}

/// The nearest existing ancestor folder of `path` (a file that does not
/// exist), and whether it is `path`'s own parent.
fn existing_parent(path: &Path) -> Option<(&Path, bool)> {
    let mut direct = true;
    let mut at = path.parent();
    while let Some(dir) = at {
        if dir.is_dir() {
            return Some((dir, direct));
        }
        if dir.exists() {
            return None;
        }
        direct = false;
        at = dir.parent();
    }
    None
}

#[cfg(windows)]
mod platform {
    use std::fs::OpenOptions;
    use std::io;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Path;

    use super::{ReadOnlyKind, WriteAccess, existing_parent, is_unc};

    const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
    const FILE_ADD_FILE: u32 = 0x2;
    const FILE_ADD_SUBDIRECTORY: u32 = 0x4;
    const FILE_DELETE_CHILD: u32 = 0x40;
    const DELETE: u32 = 0x0001_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

    pub fn probe(path: &Path) -> WriteAccess {
        match std::fs::metadata(path) {
            // A folder Loadout makes and removes entries in (skill folders).
            Ok(meta) if meta.is_dir() => open_dir(
                path,
                FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY | FILE_DELETE_CHILD,
            ),
            Ok(meta) => {
                if meta.file_attributes() & FILE_ATTRIBUTE_READONLY != 0 {
                    return WriteAccess::ReadOnly(ReadOnlyKind::Attribute);
                }
                // GENERIC_WRITE and DELETE (the save renames over the file:
                // DELETE on it, or FILE_DELETE_CHILD on its folder, which
                // the open check counts too), OPEN_EXISTING, sharing read,
                // write and delete. Nothing is created, truncated or
                // written; the handle is closed at once. A handle someone
                // holds without delete sharing is a sharing violation: a
                // lock, not read only.
                if let Err(err) = OpenOptions::new()
                    .access_mode(GENERIC_WRITE | DELETE)
                    .open(path)
                {
                    return classify(&err, path);
                }
                // The atomic replace writes a temp file next to it.
                match path.parent() {
                    Some(dir) => open_dir(dir, FILE_ADD_FILE),
                    None => WriteAccess::Writable,
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => match existing_parent(path) {
                Some((dir, true)) => open_dir(dir, FILE_ADD_FILE),
                Some((dir, false)) => open_dir(dir, FILE_ADD_SUBDIRECTORY),
                None => WriteAccess::Unknown,
            },
            Err(_) => WriteAccess::Unknown,
        }
    }

    /// Opens `dir` asking for `access` (backup semantics, needed to open a
    /// folder); nothing is created in it.
    fn open_dir(dir: &Path, access: u32) -> WriteAccess {
        match OpenOptions::new()
            .access_mode(access)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(dir)
        {
            Ok(_) => WriteAccess::Writable,
            Err(err) => classify(&err, dir),
        }
    }

    fn classify(err: &io::Error, path: &Path) -> WriteAccess {
        match err.raw_os_error() {
            // ERROR_ACCESS_DENIED
            Some(5) if is_unc(path) => WriteAccess::ReadOnly(ReadOnlyKind::Share),
            Some(5) => WriteAccess::ReadOnly(ReadOnlyKind::Acl),
            // ERROR_WRITE_PROTECT, ERROR_NETWORK_ACCESS_DENIED
            Some(19 | 65) => WriteAccess::ReadOnly(ReadOnlyKind::Share),
            _ if err.kind() == io::ErrorKind::ReadOnlyFilesystem => {
                WriteAccess::ReadOnly(ReadOnlyKind::Share)
            }
            // A sharing violation (32, 33) is a lock, not read only.
            _ => WriteAccess::Unknown,
        }
    }
}

#[cfg(unix)]
mod platform {
    use std::fs::Metadata;
    use std::io;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    use rustix::fs::{Access, access};
    use rustix::io::Errno;

    use super::{ReadOnlyKind, WriteAccess, existing_parent};

    pub fn probe(path: &Path) -> WriteAccess {
        // `metadata` and `access` follow links: the final target counts.
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => check(path, &meta, Access::WRITE_OK | Access::EXEC_OK),
            Ok(meta) => {
                let file = check(path, &meta, Access::WRITE_OK);
                if file != WriteAccess::Writable {
                    return file;
                }
                // The atomic replace writes a temp file next to it.
                match path.parent() {
                    Some(dir) => folder(dir),
                    None => WriteAccess::Writable,
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => match existing_parent(path) {
                Some((dir, _)) => folder(dir),
                None => WriteAccess::Unknown,
            },
            Err(_) => WriteAccess::Unknown,
        }
    }

    fn folder(dir: &Path) -> WriteAccess {
        match std::fs::metadata(dir) {
            Ok(meta) => check(dir, &meta, Access::WRITE_OK | Access::EXEC_OK),
            Err(_) => WriteAccess::Unknown,
        }
    }

    fn check(path: &Path, meta: &Metadata, mode: Access) -> WriteAccess {
        match access(path, mode) {
            Ok(()) => WriteAccess::Writable,
            Err(Errno::ROFS) => WriteAccess::ReadOnly(ReadOnlyKind::Share),
            Err(Errno::ACCESS | Errno::PERM) => WriteAccess::ReadOnly(kind(meta)),
            Err(_) => WriteAccess::Unknown,
        }
    }

    /// Why the mode bits (or something beyond them) deny writing.
    fn kind(meta: &Metadata) -> ReadOnlyKind {
        let mode = meta.mode();
        if meta.uid() == rustix::process::geteuid().as_raw() {
            if mode & 0o200 == 0 {
                ReadOnlyKind::Attribute
            } else {
                ReadOnlyKind::Acl
            }
        } else if mode & 0o022 == 0 {
            ReadOnlyKind::Owner
        } else {
            ReadOnlyKind::Acl
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    use std::path::Path;

    use super::WriteAccess;

    pub fn probe(_path: &Path) -> WriteAccess {
        WriteAccess::Unknown
    }
}

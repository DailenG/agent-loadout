//! Pre-write copies of client files (SPEC 8.4 step 2, 9.5):
//! `backups/files/<ts>-<apply-id>/<encoded-path>`, readable by the current
//! user only, because client files may hold secrets. Machine-local; never
//! pushed.
//!
//! An import that moves literal secrets to references leaves the old values
//! in the backups its apply made (M35): [`mark_old_values`] records which
//! ones in the run's result, and [`purge_old_values`] removes exactly those.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::apply::{ApplyEnv, journal};
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::paths::LoadoutHome;
use crate::secrets::SecretValue;
use crate::secrets::scanner::MIN_TRACKED_LEN;
use crate::store::{LOCK_TIMEOUT, StoreLock, utc_datetime};

/// Directory under `backups/` holding one directory per apply run.
pub const FILES_DIR: &str = "files";
/// Runs always kept.
const KEEP_RUNS: usize = 50;
/// Older runs beyond [`KEEP_RUNS`] are kept until they are this old.
const KEEP_FOR: Duration = Duration::from_secs(30 * 86_400);

/// `20260101T000000Z`: sorts by time, safe in a file name on every OS.
pub fn timestamp(time: SystemTime) -> String {
    let datetime = utc_datetime(time);
    let (date, clock) = (datetime.date.expect("set"), datetime.time.expect("set"));
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        date.year,
        date.month,
        date.day,
        clock.hour,
        clock.minute,
        clock.second.unwrap_or(0)
    )
}

/// The directory of one apply run's backups.
pub fn run_dir(home: &LoadoutHome, time: SystemTime, apply_id: &str) -> PathBuf {
    home.backups_dir()
        .join(FILES_DIR)
        .join(format!("{}-{apply_id}", timestamp(time)))
}

/// An absolute path as one file name: `%`, the drive colon, both
/// separators and every other character a file name cannot hold on some
/// OS are percent-encoded, so the original path is unambiguous on every OS
/// (`C%3A%5CUsers%5Cme%5C.claude.json`).
pub fn encode_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let reserved = matches!(
            c,
            '%' | ':' | '/' | '\\' | '<' | '>' | '"' | '|' | '?' | '*'
        ) || c.is_control();
        if reserved {
            let mut buf = [0u8; 4];
            for byte in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The path an [`encode_path`] name stands for.
pub fn decode_path(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = name.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Copies `bytes`, the current content of `file`, into the run directory
/// `dir`. The `backups/`, `backups/files/` and run directories and the copy
/// are restricted to the current user.
pub fn save(
    fs: &dyn Fs,
    home: &LoadoutHome,
    dir: &Path,
    file: &Path,
    bytes: &[u8],
) -> Result<PathBuf, CoreError> {
    for level in [
        home.backups_dir(),
        home.backups_dir().join(FILES_DIR),
        dir.to_path_buf(),
    ] {
        fs.create_private_dir_all(&level).map_err(|err| {
            CoreError::new(
                ErrorCode::IoError,
                format!("cannot create backup directory: {err}"),
            )
            .with_path(&level)
        })?;
    }
    let path = dir.join(encode_path(file));
    fs.write_private(&path, bytes).map_err(|err| {
        CoreError::new(ErrorCode::IoError, format!("cannot write backup: {err}")).with_path(&path)
    })?;
    Ok(path)
}

/// Retention (SPEC 9.5): the last 50 runs, or 30 days, whichever keeps
/// more. Directories that do not look like runs are left alone.
pub fn prune(fs: &dyn Fs, home: &LoadoutHome, now: SystemTime) -> Result<(), CoreError> {
    let root = home.backups_dir().join(FILES_DIR);
    if !fs.is_dir(&root) {
        return Ok(());
    }
    let mut runs: Vec<(PathBuf, SystemTime)> = fs
        .read_dir(&root)
        .map_err(|err| {
            CoreError::new(ErrorCode::IoError, format!("cannot list backups: {err}"))
                .with_path(&root)
        })?
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?;
            let time = parse_timestamp(name.get(..16)?)?;
            Some((path, time))
        })
        .collect();
    runs.sort_by_key(|(_, time)| *time);
    let old = runs.len().saturating_sub(KEEP_RUNS);
    for (path, time) in runs.into_iter().take(old) {
        let age = now.duration_since(time).unwrap_or_default();
        if age > KEEP_FOR {
            fs.remove_dir_all(&path).map_err(|err| {
                CoreError::new(ErrorCode::IoError, format!("cannot prune backups: {err}"))
                    .with_path(&path)
            })?;
        }
    }
    Ok(())
}

/// One apply run's client file backups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupRun {
    /// The run's directory name (`<ts>-<apply-id>`), unique per run.
    pub dir: String,
    /// The apply id, or `restore-<run id>-<ulid>` for the bytes a client
    /// file restore replaced.
    pub apply_id: String,
    pub time: SystemTime,
    /// The original paths of the files backed up, sorted.
    pub files: Vec<PathBuf>,
}

/// Every apply run with file backups on this machine, newest first.
/// Directories that do not look like runs are skipped.
pub fn list_runs(fs: &dyn Fs, home: &LoadoutHome) -> Result<Vec<BackupRun>, CoreError> {
    let root = home.backups_dir().join(FILES_DIR);
    if !fs.is_dir(&root) {
        return Ok(Vec::new());
    }
    let list = |dir: &Path| {
        fs.read_dir(dir).map_err(|err| {
            CoreError::new(ErrorCode::IoError, format!("cannot list backups: {err}")).with_path(dir)
        })
    };
    let mut runs = Vec::new();
    for dir in list(&root)? {
        let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let (Some(time), Some(apply_id)) = (
            name.get(..16).and_then(parse_timestamp),
            name.get(17..)
                .filter(|_| name.as_bytes().get(16) == Some(&b'-')),
        ) else {
            continue;
        };
        if !fs.is_dir(&dir) {
            continue;
        }
        let mut files: Vec<PathBuf> = list(&dir)?
            .iter()
            .filter_map(|file| decode_path(file.file_name()?.to_str()?))
            .map(PathBuf::from)
            .collect();
        files.sort();
        runs.push(BackupRun {
            dir: name.to_owned(),
            apply_id: apply_id.to_owned(),
            time,
            files,
        });
    }
    runs.sort_by(|a, b| {
        b.time
            .cmp(&a.time)
            .then_with(|| b.apply_id.cmp(&a.apply_id))
    });
    Ok(runs)
}

/// Reads a [`timestamp`].
pub fn parse_timestamp(text: &str) -> Option<SystemTime> {
    let bytes = text.as_bytes();
    if bytes.len() != 16 || bytes[8] != b'T' || bytes[15] != b'Z' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(4..6)?, number(6..8)?);
    let (hour, minute, second) = (number(9..11)?, number(11..13)?, number(13..15)?);
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second;
    Some(UNIX_EPOCH + Duration::from_secs(u64::try_from(seconds).ok()?))
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The client files whose backup of the finished run `apply_id` holds one
/// of `values` (an import's extracted secrets, which the run replaced with
/// references), each marked `old_values` in the run's result (M35). Values
/// are compared in memory and never kept; values shorter than the leak
/// scanner's minimum are not looked for, and a backup that cannot be read
/// is left unmarked.
pub fn mark_old_values(
    fs: &dyn Fs,
    state_dir: &Path,
    apply_id: &str,
    values: &[SecretValue],
) -> Result<Vec<PathBuf>, CoreError> {
    let needles = value_forms(values);
    if needles.is_empty() {
        return Ok(Vec::new());
    }
    let Some(mut run) = journal::result(fs, state_dir, apply_id)? else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    for record in run.files.iter_mut().filter(|record| !record.backup_removed) {
        let Some(backup) = record.backup.as_deref() else {
            continue;
        };
        let Ok(bytes) = fs.read(Path::new(backup)) else {
            continue;
        };
        if needles.iter().any(|needle| contains(&bytes, needle)) {
            record.old_values = true;
            found.push(PathBuf::from(&record.file));
        }
    }
    if !found.is_empty() {
        run.save_result(fs, state_dir)?;
    }
    Ok(found)
}

/// Each value as a client file may hold it: as is, trimmed, and escaped in
/// a JSON or TOML string.
fn value_forms(values: &[SecretValue]) -> Vec<Vec<u8>> {
    let mut forms: BTreeSet<Vec<u8>> = BTreeSet::new();
    for value in values {
        let value = value.expose();
        let escaped = serde_json::to_string(value).unwrap_or_default();
        let escaped = escaped
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .unwrap_or_default();
        for form in [value, value.trim(), escaped] {
            if form.len() >= MIN_TRACKED_LEN {
                forms.insert(form.as_bytes().to_vec());
            }
        }
    }
    forms.into_iter().collect()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// The files of `files` that hold one of `values` (compared as in
/// [`mark_old_values`]); a file that cannot be read is left out.
pub(crate) fn files_holding(
    fs: &dyn Fs,
    files: &[PathBuf],
    values: &[SecretValue],
) -> Vec<PathBuf> {
    let needles = value_forms(values);
    if needles.is_empty() {
        return Vec::new();
    }
    files
        .iter()
        .filter(|file| {
            fs.read(file)
                .is_ok_and(|bytes| needles.iter().any(|needle| contains(&bytes, needle)))
        })
        .cloned()
        .collect()
}

/// What [`purge_old_values`] did.
#[derive(Debug, Default)]
pub struct Purge {
    /// The client files whose backup was overwritten with zeros (and, but
    /// for those in `not_deleted`, deleted): the old values are gone.
    pub removed: Vec<PathBuf>,
    /// Of `removed`, the client files whose zeroed backup could not be
    /// deleted, with why. It holds no old value any more and is never
    /// restored.
    pub not_deleted: Vec<(PathBuf, CoreError)>,
    /// The client files whose backup was left as it was, with why.
    pub failed: Vec<(PathBuf, CoreError)>,
}

/// Removes the backups of apply run `apply_id` that hold old secret values
/// (those [`mark_old_values`] marked; M35). A recorded backup is touched
/// only when it is this run's backup of its file
/// (`backups/files/<ts>-<apply-id>/<encoded file>`) and a regular file,
/// not a link. It is overwritten with zeros in place, recorded as removed
/// right then (so `restore --files` never writes the zeros back), and
/// deleted. A failure is reported for its file and the others still go. A
/// rollback or `restore --files` of that run then reports the file's
/// original as no longer kept. A run left without backups is removed.
/// Nothing marked is a no-op. Holds the store lock.
pub fn purge_old_values(ax: &ApplyEnv<'_>, apply_id: &str) -> Result<Purge, CoreError> {
    let fs = ax.fs;
    let state_dir = ax.home.state_dir();
    let _lock = StoreLock::acquire(fs, &state_dir, LOCK_TIMEOUT)?;
    let Some(mut run) = journal::result(fs, &state_dir, apply_id)? else {
        return Err(CoreError::new(
            ErrorCode::NotFound,
            format!("no apply run {apply_id:?} is recorded on this machine"),
        ));
    };
    let mut purge = Purge::default();
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
    for record in run
        .files
        .iter_mut()
        .filter(|record| record.old_values && !record.backup_removed)
    {
        let Some(backup) = record.backup.as_deref().map(PathBuf::from) else {
            continue;
        };
        let file = PathBuf::from(&record.file);
        if let Err(err) = check_backup_path(ax.home, apply_id, &backup, &file) {
            purge.failed.push((file, err));
            continue;
        }
        let fail = |what: &str, err: std::io::Error| {
            CoreError::new(ErrorCode::IoError, format!("{what}: {err}")).with_path(&backup)
        };
        if fs.is_regular_file(&backup) {
            if let Err(err) = fs.overwrite_with_zeros(&backup) {
                purge
                    .failed
                    .push((file, fail("cannot overwrite backup", err)));
                continue;
            }
            // The old values are gone: from here on the backup is removed,
            // whether or not the zeroed file can be deleted.
            record.backup_removed = true;
            if let Err(err) = fs.remove_file(&backup) {
                purge
                    .not_deleted
                    .push((file.clone(), fail("cannot delete the zeroed backup", err)));
            }
        } else if fs.exists(&backup) || fs.is_dir(&backup) {
            purge.failed.push((
                file,
                CoreError::new(
                    ErrorCode::InvalidValue,
                    "the recorded backup is not a regular file (a link or a folder); left alone",
                )
                .with_path(&backup),
            ));
            continue;
        } else {
            // Gone already (pruned, deleted by hand), or a dangling link,
            // which holds no value either.
            record.backup_removed = true;
        }
        purge.removed.push(file);
        if let Some(dir) = backup.parent() {
            dirs.insert(dir.to_path_buf());
        }
    }
    if !purge.removed.is_empty() {
        run.save_result(fs, &state_dir)?;
    }
    for dir in dirs {
        if fs.read_dir(&dir).is_ok_and(|entries| entries.is_empty()) {
            // Best effort: an empty run directory lists as a run without files.
            let _ = fs.remove_dir_all(&dir);
        }
    }
    Ok(purge)
}

/// Whether the recorded `backup` is run `apply_id`'s backup of `file`:
/// `backups/files/<ts>-<apply-id>/<encode_path(file)>`, no `..`. The run
/// result is a file on disk, so it is never trusted to name what to
/// overwrite.
fn check_backup_path(
    home: &LoadoutHome,
    apply_id: &str,
    backup: &Path,
    file: &Path,
) -> Result<(), CoreError> {
    let root = home.backups_dir().join(FILES_DIR);
    let suffix = format!("-{apply_id}");
    let in_run = backup.parent().is_some_and(|dir| {
        dir.parent() == Some(root.as_path())
            && dir
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.get(..16).and_then(parse_timestamp).is_some()
                        && name.get(16..) == Some(suffix.as_str())
                })
    });
    let plain = backup
        .components()
        .all(|part| !matches!(part, std::path::Component::ParentDir));
    let named =
        backup.file_name().and_then(|name| name.to_str()) == Some(encode_path(file).as_str());
    if in_run && plain && named {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!(
                "the recorded backup is not apply {apply_id}'s backup of this file; left alone"
            ),
        )
        .with_path(backup))
    }
}

/// The client files whose backup of apply run `apply_id` was removed
/// because it held old secret values ([`purge_old_values`]): their content
/// from before that run is no longer kept.
pub fn removed_backups(
    fs: &dyn Fs,
    state_dir: &Path,
    apply_id: &str,
) -> Result<Vec<PathBuf>, CoreError> {
    Ok(journal::result(fs, state_dir, apply_id)?
        .map(|run| {
            run.files
                .into_iter()
                .filter(|record| record.backup_removed)
                .map(|record| PathBuf::from(record.file))
                .collect()
        })
        .unwrap_or_default())
}

//! Pre-write copies of client files (SPEC 8.4 step 2, 9.5):
//! `backups/files/<ts>-<apply-id>/<encoded-path>`, readable by the current
//! user only, because client files may hold secrets. Machine-local; never
//! pushed.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::paths::LoadoutHome;
use crate::store::utc_datetime;

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

/// Reads a [`timestamp`].
fn parse_timestamp(text: &str) -> Option<SystemTime> {
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

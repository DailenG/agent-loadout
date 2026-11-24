//! The catalog cache (#26): `<LOADOUT_HOME>/cache/catalogs/<id>/<name>.json`,
//! each file `{fetched_at, key, data}`. `key` names what was fetched (the
//! URL and ref of the catalog, plus the query): a file under another key
//! is ignored, so a catalog whose URL changed never serves old results.
//! Machine-local and re-creatable; a file that cannot be read is a miss.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::CoreError;
use crate::skills::{SkillsEnv, io_error};

/// `<LOADOUT_HOME>/cache/catalogs`.
pub(crate) fn root(sx: &SkillsEnv<'_>) -> PathBuf {
    sx.home.cache_dir().join(super::CACHE_DIR)
}

#[derive(Serialize, Deserialize)]
struct File<T> {
    fetched_at: u64,
    key: String,
    data: T,
}

/// A cached value and when it was fetched.
#[derive(Debug, Clone)]
pub(crate) struct Hit<T> {
    pub data: T,
    pub fetched_at: SystemTime,
}

impl<T> Hit<T> {
    pub fn age(&self, now: SystemTime) -> Duration {
        now.duration_since(self.fetched_at).unwrap_or_default()
    }
}

/// 16 hex digits of the SHA-256 of `text`: a file name for a query or an
/// entry name.
pub(crate) fn digest(text: &str) -> String {
    let hash = Sha256::digest(text.as_bytes());
    hash.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn path(sx: &SkillsEnv<'_>, catalog: &str, name: &str) -> PathBuf {
    root(sx).join(catalog).join(format!("{name}.json"))
}

/// The value cached under `name` for `key`, any age.
pub(crate) fn read<T: DeserializeOwned>(
    sx: &SkillsEnv<'_>,
    catalog: &str,
    name: &str,
    key: &str,
) -> Option<Hit<T>> {
    let bytes = sx.fs.read(&path(sx, catalog, name)).ok()?;
    let file: File<T> = serde_json::from_slice(&bytes).ok()?;
    (file.key == key).then(|| Hit {
        data: file.data,
        fetched_at: UNIX_EPOCH + Duration::from_secs(file.fetched_at),
    })
}

/// Caches `data` under `name` for `key`, fetched now.
pub(crate) fn write<T: Serialize>(
    sx: &SkillsEnv<'_>,
    catalog: &str,
    name: &str,
    key: &str,
    data: &T,
) -> Result<SystemTime, CoreError> {
    let now = sx.env.now();
    let fetched_at = now
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let file = File {
        fetched_at,
        key: key.to_owned(),
        data,
    };
    let text = serde_json::to_vec(&file).expect("cache values serialize");
    let target = path(sx, catalog, name);
    let dir = target.parent().expect("a cache file has a folder");
    sx.fs
        .create_dir_all(dir)
        .map_err(|err| io_error("cannot create directory", dir, &err))?;
    crate::apply::write_atomic(sx.fs, &target, &text)?;
    prune(sx, catalog, now);
    Ok(UNIX_EPOCH + Duration::from_secs(fetched_at))
}

/// A cache file unused this long is dropped (a query nobody repeats).
const KEEP: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// A temporary tap clone this old was left by an interrupted index.
const STRAY_CLONE: Duration = Duration::from_secs(60 * 60);

/// The time a ULID (`.tmp-<ulid>`) was made: its first 10 Crockford
/// base32 digits, milliseconds since the epoch.
fn ulid_time(ulid: &str) -> Option<SystemTime> {
    const DIGITS: &str = "0123456789abcdefghjkmnpqrstvwxyz";
    let mut ms: u64 = 0;
    for c in ulid.get(..10)?.chars() {
        let digit = DIGITS.find(c.to_ascii_lowercase())?;
        ms = ms.checked_mul(32)?.checked_add(digit as u64)?;
    }
    Some(UNIX_EPOCH + Duration::from_millis(ms))
}

/// Best effort, after a write: cache files of `catalog` fetched more than
/// [`KEEP`] ago, and temporary clones older than [`STRAY_CLONE`].
fn prune(sx: &SkillsEnv<'_>, catalog: &str, now: SystemTime) {
    let old = |at: SystemTime, keep: Duration| now.duration_since(at).is_ok_and(|age| age > keep);
    for file in sx.fs.read_dir(&root(sx).join(catalog)).unwrap_or_default() {
        let fetched_at = sx
            .fs
            .read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|value| value.get("fetched_at")?.as_u64());
        if fetched_at.is_some_and(|secs| old(UNIX_EPOCH + Duration::from_secs(secs), KEEP)) {
            let _ = sx.fs.remove_file(&file);
        }
    }
    for dir in sx.fs.read_dir(&root(sx)).unwrap_or_default() {
        let stray = dir
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix(".tmp-"))
            .and_then(ulid_time)
            .is_some_and(|made| old(made, STRAY_CLONE));
        if stray && sx.fs.is_dir(&dir) {
            let _ = crate::skills::install::remove_dir(sx.fs, &dir);
        }
    }
}

/// Drops everything cached for `catalog`.
pub(crate) fn clear(sx: &SkillsEnv<'_>, catalog: &str) -> Result<(), CoreError> {
    let dir = root(sx).join(catalog);
    if sx.fs.is_dir(&dir) {
        crate::skills::install::remove_dir(sx.fs, &dir)?;
    }
    Ok(())
}

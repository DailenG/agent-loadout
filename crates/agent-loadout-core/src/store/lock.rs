//! The cross-process lock `<LOADOUT_HOME>/state/loadout.lock` (SPEC 4.1).
//!
//! Any process mutating the store, the ledger or client files holds it. A
//! second process waits up to [`LOCK_TIMEOUT`], then fails with `locked`
//! (CLI exit code 6). The lock is an OS advisory lock on an open handle, so
//! it is released when the holder exits or crashes; the file itself is
//! never read, written or deleted.

use std::fs::{File, TryLockError};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;

/// Lock file name inside `state/`.
pub const LOCK_FILE: &str = "loadout.lock";
/// How long a second process waits for the lock.
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(10);

const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The held lock; released on drop.
#[derive(Debug)]
pub struct StoreLock {
    file: File,
}

impl StoreLock {
    /// Takes the lock in `state_dir`, waiting up to `timeout` for another
    /// holder to release it. Creates `state_dir` if needed.
    pub fn acquire(fs: &dyn Fs, state_dir: &Path, timeout: Duration) -> Result<Self, CoreError> {
        let path = state_dir.join(LOCK_FILE);
        let io_error = |err: std::io::Error| {
            CoreError::new(ErrorCode::IoError, format!("cannot open lock file: {err}"))
                .with_path(&path)
        };
        fs.create_dir_all(state_dir).map_err(io_error)?;
        let file = fs.open_lock_file(&path).map_err(io_error)?;
        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { file }),
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(err)) => return Err(io_error(err)),
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(CoreError::new(
                    ErrorCode::Locked,
                    format!(
                        "another Loadout process is changing the store; gave up after {} ms",
                        timeout.as_millis()
                    ),
                )
                .with_path(&path));
            }
            thread::sleep(POLL_INTERVAL.min(deadline - now));
        }
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        // Closing the handle releases the lock too; unlocking first makes the
        // release immediate on every OS.
        let _ = self.file.unlock();
    }
}

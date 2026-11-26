//! Ctrl+C (SIGINT), SIGTERM and SIGHUP while the external scanner runs or
//! is prepared (SkillSpector scans and `scanner prepare`, #30). The
//! scanner is in its own process group on Unix, so the terminal's Ctrl+C
//! never reaches it, and on Windows the run folder would stay behind.
//! While a [`Busy`] guard is held, the signal sets the cancel flag
//! instead: the run kills the scanner's process tree and removes its run
//! folder, then the guard ends the command with exit code 130. Outside a
//! guard the signal ends the command at once, as without a handler.

use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

/// Exit code of a command ended by Ctrl+C (128 + SIGINT).
pub const EXIT_INTERRUPTED: i32 = 130;

static CANCEL: AtomicBool = AtomicBool::new(false);
static BUSY: AtomicBool = AtomicBool::new(false);
static HANDLER: Once = Once::new();

/// A cancelable stretch of the command (a scanner run or Prepare).
pub struct Busy(());

impl Busy {
    /// Installs the handler (once per process) and marks the command busy.
    pub fn start() -> Self {
        HANDLER.call_once(|| {
            // Without a handler (it could not be installed) the signal
            // simply ends the process, as before.
            let _ = ctrlc::set_handler(|| {
                if BUSY.load(Ordering::SeqCst) {
                    CANCEL.store(true, Ordering::SeqCst);
                } else {
                    std::process::exit(EXIT_INTERRUPTED);
                }
            });
        });
        BUSY.store(true, Ordering::SeqCst);
        Self(())
    }

    /// The flag the run or Prepare polls.
    pub fn flag(&self) -> &'static AtomicBool {
        &CANCEL
    }
}

impl Drop for Busy {
    /// The run has cleaned up: a canceled command ends here.
    fn drop(&mut self) {
        BUSY.store(false, Ordering::SeqCst);
        if CANCEL.load(Ordering::SeqCst) {
            eprintln!("Canceled.");
            std::process::exit(EXIT_INTERRUPTED);
        }
    }
}

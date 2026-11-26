//! One bounded run of Snyk Agent Scan (#27). The skill's files are staged
//! below `<LOADOUT_HOME>/cache/scan-external/` (the content scanned is
//! exactly the content that will be written), then
//!
//! ```text
//! agent-scan scan --json --storage-file <run>/storage.json <run>/skill
//! ```
//!
//! runs with stdin closed (every consent prompt reads EOF, which declines),
//! a scrubbed environment ([`child_env`]: `SNYK_TOKEN` and the variables a
//! PyInstaller program needs to start and reach the network, nothing else)
//! and a timeout; the whole process tree is killed on timeout or cancel
//! (the shared process runner). Never passed: `--ci`, `--verbose`,
//! `--dangerously-run-mcp-servers`, `--scan-all-users`, a prefixed path,
//! or no path. The folder is removed afterwards. The token reaches the
//! child's environment only: never an argument, a file, a log line or the
//! report.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::install::Tool;
use super::{SCANNER, adapter};
use crate::env::Env;
use crate::paths::LoadoutHome;
use crate::scan::external::process::{self, Drained, Ended, MAX_STDOUT, RunDir, passed};
use crate::scan::external::{ExternalReport, NotScanned};
use crate::secrets::SecretValue;
use crate::skills::{SkillFiles, content_hash};

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
    let mut vars = passed(env, PASSED_VARS);
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
        let mut report = match process::stage(home, files) {
            // The run folder goes when `run` drops.
            Ok(run) => self.run(env, &run, cancel),
            Err(why) => ExternalReport::not_scanned(
                SCANNER,
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
        let not_scanned = |reason, detail: Option<String>| {
            ExternalReport::not_scanned(SCANNER, version, reason, detail)
        };
        let mut command = Command::new(self.tool.binary());
        command
            .arg("scan")
            .arg("--json")
            .arg("--storage-file")
            .arg(run.dir.join("storage.json"))
            .arg(&run.skill)
            .current_dir(&run.dir)
            .env_clear()
            .envs(child_env(env, &self.token, &run.tmp));
        let executed = match process::execute(command, self.timeout, cancel, MAX_STDOUT, None) {
            Ok(executed) => executed,
            Err(why) => return not_scanned(NotScanned::Failed, Some(why)),
        };
        let stderr = String::from_utf8_lossy(&executed.stderr)
            .replace(self.token.expose(), "<token>")
            .into_bytes();
        match executed.ended {
            Ended::Exited(code) => match executed.stdout {
                Some(Drained { overflow: true, .. }) => ExternalReport::not_understood(
                    SCANNER,
                    version,
                    "the output is larger than 16 MiB",
                ),
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

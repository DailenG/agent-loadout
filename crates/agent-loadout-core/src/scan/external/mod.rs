//! The optional external scanner: a second opinion beside the built-in
//! scanner, never instead of it. Off by default.
//!
//! Since 0.8.0 (#30) that is NVIDIA SkillSpector ([`skillspector`]), run
//! locally from the user's own `uv` or `skillspector`; Loadout never
//! installs it. Snyk Agent Scan (#27, [`snyk`]) shipped in 0.7.0 and is
//! disabled since 0.7.1 ([`snyk::AVAILABLE`]); its code stays.
//!
//! Results map into the M40 findings model ([`Finding`], `scanner =`
//! [`Scanner::as_str`]) and are shown beside the built-in findings. A
//! `dangerous` external verdict needs the same explicit "install anyway"
//! as a built-in one ([`gate`]); a run that did not happen or whose output
//! was not understood never blocks and is never shown as clean.

mod process;
pub mod skillspector;
pub mod snyk;
mod text;

use serde::{Deserialize, Serialize};

pub use process::RUNS_DIR;

use super::{Finding, Severity, Verdict};
use crate::error::{CoreError, ErrorCode};

/// Which external scanner made a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scanner {
    #[serde(rename = "skillspector")]
    SkillSpector,
    #[serde(rename = "snyk-agent-scan")]
    SnykAgentScan,
}

impl Scanner {
    /// The id: `Finding::scanner`, `--json` and IPC.
    pub const fn as_str(self) -> &'static str {
        match self {
            Scanner::SkillSpector => "skillspector",
            Scanner::SnykAgentScan => "snyk-agent-scan",
        }
    }

    /// The name people read.
    pub const fn name(self) -> &'static str {
        match self {
            Scanner::SkillSpector => "SkillSpector",
            Scanner::SnykAgentScan => "Snyk Agent Scan",
        }
    }
}

/// Whether the run happened and was understood.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalStatus {
    /// The scanner ran and its output was understood: the findings (none
    /// for a clean verdict) are its answer.
    Scanned,
    /// The content was not scanned ([`ExternalReport::reason`]).
    NotScanned,
    /// The scanner ran but its output is not the schema this build reads
    /// ("scanner output not understood (version X)"); never clean.
    NotUnderstood,
}

impl ExternalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            ExternalStatus::Scanned => "scanned",
            ExternalStatus::NotScanned => "not_scanned",
            ExternalStatus::NotUnderstood => "not_understood",
        }
    }
}

/// Why the content was not scanned. None of these blocks an install.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotScanned {
    /// SkillSpector: neither uv nor a usable `skillspector` is on `PATH`.
    NotAvailable,
    /// SkillSpector: uv is on `PATH` but the pinned SkillSpector is not
    /// downloaded yet (or uv's cache lost it): run Prepare.
    NotPrepared,
    /// Snyk: the scanner is not installed on this machine.
    NotInstalled,
    /// Snyk: no build for this OS and architecture.
    UnsupportedPlatform,
    /// Snyk: no token reference is set, or it resolved to an empty value.
    NoToken,
    /// Snyk: the token reference did not resolve (missing, locked,
    /// provider unavailable).
    TokenUnavailable,
    /// Snyk refused the token (401).
    TokenRejected,
    /// Snyk's daily usage limit is reached.
    LimitReached,
    /// The skill is too large for Snyk's analysis (413).
    TooLarge,
    /// Snyk's analysis API could not be reached.
    Unreachable,
    /// The run took longer than the timeout and was stopped.
    Timeout,
    /// The user canceled the run.
    Canceled,
    /// Snyk: scanning would start an MCP server (declined).
    WouldStartServer,
    /// Any other failure (the scanner did not start, exited with an
    /// error): see [`ExternalReport::detail`].
    Failed,
}

impl NotScanned {
    pub const fn as_str(self) -> &'static str {
        match self {
            NotScanned::NotAvailable => "not_available",
            NotScanned::NotPrepared => "not_prepared",
            NotScanned::NotInstalled => "not_installed",
            NotScanned::UnsupportedPlatform => "unsupported_platform",
            NotScanned::NoToken => "no_token",
            NotScanned::TokenUnavailable => "token_unavailable",
            NotScanned::TokenRejected => "token_rejected",
            NotScanned::LimitReached => "limit_reached",
            NotScanned::TooLarge => "too_large",
            NotScanned::Unreachable => "unreachable",
            NotScanned::Timeout => "timeout",
            NotScanned::Canceled => "canceled",
            NotScanned::WouldStartServer => "would_start_server",
            NotScanned::Failed => "failed",
        }
    }

    /// English, for the CLI: "not scanned (<this>)".
    pub const fn text(self) -> &'static str {
        match self {
            NotScanned::NotAvailable => "SkillSpector was not found; see `loadout scanner status`",
            NotScanned::NotPrepared => {
                "SkillSpector is not downloaded yet; run `loadout scanner prepare`"
            }
            NotScanned::NotInstalled => "not installed",
            NotScanned::UnsupportedPlatform => "no build for this platform",
            NotScanned::NoToken => "no token",
            NotScanned::TokenUnavailable => "the token could not be read",
            NotScanned::TokenRejected => "Snyk did not accept the token",
            NotScanned::LimitReached => "Snyk limit reached",
            NotScanned::TooLarge => "too large",
            NotScanned::Unreachable => "Snyk could not be reached",
            NotScanned::Timeout => "timed out",
            NotScanned::Canceled => "canceled",
            NotScanned::WouldStartServer => "would start the server",
            NotScanned::Failed => "the scanner failed",
        }
    }
}

/// One external scan of one skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalReport {
    pub scanner: Scanner,
    /// The scanner's version that ran (or would have run).
    pub version: String,
    pub status: ExternalStatus,
    /// Set when `status` is [`ExternalStatus::NotScanned`].
    pub reason: Option<NotScanned>,
    /// The worst finding. `Clean` also when nothing was scanned: always
    /// read [`ExternalReport::status`] and [`ExternalReport::incomplete`]
    /// first.
    pub verdict: Verdict,
    /// Sorted by file, line and rule.
    pub findings: Vec<Finding>,
    /// The scanner says it did not fully scan the content (an `info`
    /// finding says how much): never shown as clean.
    #[serde(default)]
    pub incomplete: bool,
    /// [`crate::skills::content_hash`] of what was scanned.
    pub content_hash: Option<String>,
    /// English, one line, no paths or values: why it was not scanned,
    /// what was not understood.
    pub detail: Option<String>,
}

impl ExternalReport {
    pub fn not_scanned(
        scanner: Scanner,
        version: &str,
        reason: NotScanned,
        detail: Option<String>,
    ) -> Self {
        Self {
            scanner,
            version: version.to_owned(),
            status: ExternalStatus::NotScanned,
            reason: Some(reason),
            verdict: Verdict::Clean,
            findings: Vec::new(),
            incomplete: false,
            content_hash: None,
            detail,
        }
    }

    pub fn not_understood(scanner: Scanner, version: &str, why: &str) -> Self {
        Self {
            scanner,
            version: version.to_owned(),
            status: ExternalStatus::NotUnderstood,
            reason: None,
            verdict: Verdict::Clean,
            findings: Vec::new(),
            incomplete: false,
            content_hash: None,
            detail: Some(format!(
                "scanner output not understood (version {version}): {why}"
            )),
        }
    }

    /// One line for the CLI: `dangerous (2 findings)`, `not fully scanned
    /// (1 finding)`, `warning (3 findings), not fully scanned`, `not
    /// scanned (timed out)`, `scanner output not understood (version
    /// 2.13.7)`.
    pub fn summary(&self) -> String {
        match (self.status, self.reason) {
            (ExternalStatus::NotScanned, Some(reason)) => {
                format!("not scanned ({})", reason.text())
            }
            (ExternalStatus::NotUnderstood, _) => {
                format!("scanner output not understood (version {})", self.version)
            }
            _ => {
                let n = self.findings.len();
                let plural = if n == 1 { "" } else { "s" };
                match (self.incomplete, self.verdict) {
                    (true, Verdict::Clean) => format!("not fully scanned ({n} finding{plural})"),
                    (true, verdict) => {
                        format!(
                            "{} ({n} finding{plural}), not fully scanned",
                            verdict.as_str()
                        )
                    }
                    (false, verdict) => format!("{} ({n} finding{plural})", verdict.as_str()),
                }
            }
        }
    }

    /// The findings that make the verdict `dangerous`.
    pub fn dangerous(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.severity == Severity::Dangerous)
    }
}

/// Refuses content the external scanner found `dangerous` (`scan_blocked`)
/// unless the user explicitly chose to install it anyway. A report that is
/// not scanned or not understood never blocks (Snyk's partial findings
/// beside a failure still do; SkillSpector's not-scanned reports carry
/// none).
pub fn gate(report: &ExternalReport, allow_dangerous: bool) -> Result<(), CoreError> {
    if allow_dangerous || report.verdict != Verdict::Dangerous {
        return Ok(());
    }
    let labels: Vec<String> = report.dangerous().map(Finding::label).collect();
    let mut shown = labels
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if labels.len() > 3 {
        shown.push_str(&format!(" and {} more", labels.len() - 3));
    }
    let plural = if labels.len() == 1 { "" } else { "s" };
    Err(CoreError::new(
        ErrorCode::ScanBlocked,
        format!(
            "{} found {} dangerous finding{plural}: {shown}; review them, and install anyway only if you trust this content",
            report.scanner.name(),
            labels.len()
        ),
    ))
}

/// A per-run override of the setting (`--external` / `--no-external`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Choice {
    /// The machine's setting.
    #[default]
    Default,
    On,
    Off,
}

impl Choice {
    /// Whether to run: [`Choice::On`], or [`Choice::Default`] with the
    /// scanner turned on (`enabled`).
    pub const fn wants(self, enabled: bool) -> bool {
        match self {
            Choice::On => true,
            Choice::Off => false,
            Choice::Default => enabled,
        }
    }
}

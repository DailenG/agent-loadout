//! Built-in scan results as the frontend sees them (SPEC 17, #25): the
//! verdict and findings of a skill install, update or review, and of MCP
//! drafts. Paths in findings are relative to the scanned root; excerpts
//! have secret-shaped tokens masked by the core.

use agent_loadout_core::scan::{Finding, ScanReport, Severity, Verdict};
use serde::Serialize;
use specta::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ScanSeverity {
    Info,
    Warning,
    Dangerous,
}

impl From<Severity> for ScanSeverity {
    fn from(severity: Severity) -> Self {
        match severity {
            Severity::Info => Self::Info,
            Severity::Warning => Self::Warning,
            Severity::Dangerous => Self::Dangerous,
        }
    }
}

/// The worst finding: `dangerous` blocks an install or update unless the
/// user chooses to install anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ScanVerdict {
    Clean,
    Warning,
    Dangerous,
}

impl From<Verdict> for ScanVerdict {
    fn from(verdict: Verdict) -> Self {
        match verdict {
            Verdict::Clean => Self::Clean,
            Verdict::Warning => Self::Warning,
            Verdict::Dangerous => Self::Dangerous,
        }
    }
}

/// One thing a rule matched.
#[derive(Debug, Clone, Serialize, Type)]
pub struct ScanFinding {
    /// `builtin` for Loadout's own scanner.
    pub scanner: String,
    pub rule: String,
    pub severity: ScanSeverity,
    /// Relative to the skill (or repository) root; `null` for a command
    /// line.
    pub file: Option<String>,
    pub line: Option<u32>,
    /// English; the frontend may localize by `rule`.
    pub message: String,
    /// The matched text, shortened, secret-shaped tokens masked, invisible
    /// characters spelled out.
    pub excerpt: Option<String>,
}

impl From<&Finding> for ScanFinding {
    fn from(finding: &Finding) -> Self {
        Self {
            scanner: finding.scanner.clone(),
            rule: finding.rule.clone(),
            severity: finding.severity.into(),
            file: finding.file.clone(),
            line: finding.line,
            message: finding.message.clone(),
            excerpt: finding.excerpt.clone(),
        }
    }
}

/// A scan's verdict and findings.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ScanResult {
    pub verdict: ScanVerdict,
    pub findings: Vec<ScanFinding>,
    pub rules_version: u32,
    /// From the cache (the same content was scanned before).
    pub cached: bool,
    /// A custom rules file (`state/scan-rules.toml`) was merged into the
    /// built-in rules ("custom rules in use").
    pub custom_rules: bool,
    /// Problems loading the rules (English); the built-in rules ran.
    pub warnings: Vec<String>,
}

impl From<&ScanReport> for ScanResult {
    fn from(report: &ScanReport) -> Self {
        Self {
            verdict: report.verdict.into(),
            findings: report.findings.iter().map(ScanFinding::from).collect(),
            rules_version: report.rules_version,
            cached: report.cached,
            custom_rules: report.custom_rules,
            warnings: report.warnings.clone(),
        }
    }
}

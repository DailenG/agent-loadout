//! The built-in offline scanner (SPEC 17, #25): Loadout's own rules,
//! shipped as data (`rules.toml`), run locally on every skill install and
//! update and on every MCP server drafted from a Git repository or folder.
//! Nothing leaves the machine.
//!
//! A scan gives findings (severity, rule, file and line, message) and a
//! verdict, the worst finding: `clean` (none, or `info` only), `warning`
//! or `dangerous`. [`gate`] refuses `dangerous` content with
//! `scan_blocked` unless the user explicitly chose to install anyway.
//! Trusting a source never skips the scan. Results are cached by content
//! hash ([`Scanner::new`]).
//!
//! The scanner is a second opinion, not a guarantee: a clean verdict means
//! no rule matched, not that the content is safe.

mod cache;
mod engine;
pub mod rules;
#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::paths::LoadoutHome;
use crate::skills::{SkillFiles, content_hash};

pub use cache::CACHE_DIR;
use cache::Cache;
pub use rules::{Rule, Rules, Target};

/// The `scanner` of the built-in scanner's findings.
pub const SCANNER: &str = "builtin";
/// A custom rules file below `<LOADOUT_HOME>/state`, merged into the
/// built-in rules ([`Rules::with_custom`]: add rules or raise severities).
pub const RULES_OVERRIDE_FILE: &str = "scan-rules.toml";

/// How bad one finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Dangerous,
}

impl Severity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Dangerous => "dangerous",
        }
    }
}

/// The worst finding of a scan (`info` findings leave it `clean`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Clean,
    Warning,
    Dangerous,
}

impl Verdict {
    pub const fn as_str(self) -> &'static str {
        match self {
            Verdict::Clean => "clean",
            Verdict::Warning => "warning",
            Verdict::Dangerous => "dangerous",
        }
    }

    pub fn of(findings: &[Finding]) -> Self {
        match findings.iter().map(|finding| finding.severity).max() {
            Some(Severity::Dangerous) => Verdict::Dangerous,
            Some(Severity::Warning) => Verdict::Warning,
            Some(Severity::Info) | None => Verdict::Clean,
        }
    }
}

/// One thing a rule matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// [`SCANNER`] for the built-in scanner.
    pub scanner: String,
    /// The rule's id.
    pub rule: String,
    pub severity: Severity,
    /// The file, relative to the skill (or repository) root,
    /// `/`-separated; `None` for a command line.
    pub file: Option<String>,
    /// 1-based line of the match; `None` for a path rule and a command
    /// line.
    pub line: Option<u32>,
    pub message: String,
    /// The matched text, shortened, with secret-shaped tokens masked and
    /// invisible characters spelled out (`\u{200B}`, `[hidden: ...]`).
    pub excerpt: Option<String>,
}

impl Finding {
    /// `rule (file:line)`, for messages.
    pub fn label(&self) -> String {
        match (&self.file, self.line) {
            (Some(file), Some(line)) => format!("{} ({file}:{line})", self.rule),
            (Some(file), None) => format!("{} ({file})", self.rule),
            _ => self.rule.clone(),
        }
    }
}

/// What a scan found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanReport {
    pub verdict: Verdict,
    /// Sorted by file, line and rule.
    pub findings: Vec<Finding>,
    /// `version` of the rules that ran.
    pub rules_version: u32,
    /// [`content_hash`] of the scanned files; `None` for a command line or
    /// a merged report of different content.
    pub content_hash: Option<String>,
    /// Taken from the cache, not scanned now.
    #[serde(default, skip_serializing)]
    pub cached: bool,
    /// A custom rules file (`state/scan-rules.toml`) was merged into the
    /// built-in rules for this scan.
    #[serde(default, skip_serializing)]
    pub custom_rules: bool,
    /// Problems loading the rules (a custom rules file not used, or an
    /// entry of it skipped); the built-in rules always ran.
    #[serde(default, skip_serializing)]
    pub warnings: Vec<String>,
}

impl ScanReport {
    fn new(rules_version: u32, content_hash: Option<String>, mut findings: Vec<Finding>) -> Self {
        findings.sort_by(|a, b| (&a.file, a.line, &a.rule).cmp(&(&b.file, b.line, &b.rule)));
        Self {
            verdict: Verdict::of(&findings),
            findings,
            rules_version,
            content_hash,
            cached: false,
            custom_rules: false,
            warnings: Vec::new(),
        }
    }

    /// Adds another scan of the same item (an MCP draft's repository
    /// files and its command line): findings together, the worse verdict.
    pub fn merge(&mut self, other: ScanReport) {
        let content_hash = match (self.content_hash.take(), other.content_hash) {
            (Some(a), Some(b)) if a == b => Some(a),
            (Some(hash), None) | (None, Some(hash)) => Some(hash),
            _ => None,
        };
        let mut findings = std::mem::take(&mut self.findings);
        findings.extend(other.findings);
        let cached = self.cached && other.cached;
        let custom_rules = self.custom_rules || other.custom_rules;
        let mut warnings = std::mem::take(&mut self.warnings);
        for warning in other.warnings {
            if !warnings.contains(&warning) {
                warnings.push(warning);
            }
        }
        *self = ScanReport::new(
            self.rules_version.max(other.rules_version),
            content_hash,
            findings,
        );
        self.cached = cached;
        self.custom_rules = custom_rules;
        self.warnings = warnings;
    }

    /// The findings that make the verdict `dangerous`.
    pub fn dangerous(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|finding| finding.severity == Severity::Dangerous)
    }
}

/// Refuses content whose verdict is `dangerous` (`scan_blocked`) unless
/// the user explicitly chose to install it anyway.
pub fn gate(report: &ScanReport, allow_dangerous: bool) -> Result<(), CoreError> {
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
            "the built-in scan found {} dangerous finding{plural}: {shown}; review them, and install anyway only if you trust this content",
            labels.len()
        ),
    ))
}

/// Runs the rules, with the cache when made by [`Scanner::new`].
pub struct Scanner<'a> {
    rules: Cow<'static, Rules>,
    cache: Option<Cache<'a>>,
    warnings: Vec<String>,
}

impl<'a> Scanner<'a> {
    /// The scanner installs and updates use: the built-in rules, with a
    /// custom `state/scan-rules.toml` merged in (it may add rules or
    /// raise severities only; one that does not load is a warning),
    /// results cached in `cache/scan`.
    pub fn new(fs: &'a dyn Fs, home: &LoadoutHome) -> Self {
        let (rules, warnings) = load_rules(fs, &home.state_dir().join(RULES_OVERRIDE_FILE));
        Self {
            rules,
            cache: Some(Cache::new(fs, home.cache_dir().join(CACHE_DIR))),
            warnings,
        }
    }

    pub fn rules(&self) -> &Rules {
        &self.rules
    }

    /// Problems loading the rules; every report carries them too.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    fn finish(&self, mut report: ScanReport) -> ScanReport {
        report.custom_rules = self.rules.is_custom();
        report.warnings.clone_from(&self.warnings);
        report
    }

    /// Scans files (a skill folder, a repository's files) by relative
    /// `/`-separated path.
    pub fn scan_files(&self, files: &SkillFiles) -> ScanReport {
        let hash = content_hash(files);
        let key = self.cache.as_ref().map(|_| Cache::key(&self.rules, &hash));
        if let (Some(cache), Some(key)) = (&self.cache, &key)
            && let Some(report) = cache.get(key)
        {
            return self.finish(report);
        }
        let report = ScanReport::new(
            self.rules.version(),
            Some(hash),
            engine::scan_files(&self.rules, files),
        );
        if let (Some(cache), Some(key)) = (&self.cache, &key) {
            cache.put(key, &report);
        }
        self.finish(report)
    }

    /// Scans an MCP server's command line: `command` and `args` joined by
    /// spaces, as the `command` rules expect.
    pub fn scan_command(&self, command: &str, args: &[String]) -> ScanReport {
        let mut line = command.to_owned();
        for arg in args {
            line.push(' ');
            line.push_str(arg);
        }
        self.finish(ScanReport::new(
            self.rules.version(),
            None,
            engine::scan_command(&self.rules, &line),
        ))
    }
}

impl Scanner<'static> {
    /// The built-in rules, without a cache.
    pub fn builtin() -> Self {
        Self::with_rules(Cow::Borrowed(Rules::builtin()))
    }

    /// Other rules (tests, a rules file being checked), without a cache.
    pub fn with_rules(rules: Cow<'static, Rules>) -> Self {
        Self {
            rules,
            cache: None,
            warnings: Vec::new(),
        }
    }
}

/// The built-in rules, with the custom rules file merged in when there is
/// one that loads. The built-in rules always run.
fn load_rules(fs: &dyn Fs, path: &Path) -> (Cow<'static, Rules>, Vec<String>) {
    let builtin = Rules::builtin();
    if !fs.exists(path) {
        return (Cow::Borrowed(builtin), Vec::new());
    }
    let merged = fs
        .read_to_string(path)
        .map_err(|err| CoreError::new(ErrorCode::IoError, err.to_string()))
        .and_then(|text| builtin.with_custom(&text));
    match merged {
        Ok((rules, warnings)) => (Cow::Owned(rules), warnings),
        Err(err) => (
            Cow::Borrowed(builtin),
            vec![format!(
                "state/{RULES_OVERRIDE_FILE} is not used, only the built-in scan rules ran: {}",
                err.message()
            )],
        ),
    }
}

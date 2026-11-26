//! SkillSpector 2.13.7's `--format json` report as M40 findings.
//!
//! The exit code comes first: 0 or 1 (a finished scan; 1 also when
//! `--fail-on-incomplete` fired) are parsed, anything else is "not
//! scanned (failed)" with the last stderr line. The report must say
//! `execution_successful: true` and `metadata.llm_requested: false`, and
//! carry the fields this build reads; otherwise it is "scanner output not
//! understood", never clean. There is no published JSON Schema: the shape
//! read here is `Finding.to_dict()` and `_format_json` of the tagged
//! source, and anything else in the document is ignored.
//!
//! Severities: `CRITICAL` is `dangerous`, `HIGH` and `MEDIUM` are
//! `warning`, `LOW` is `info`. `DO_NOT_INSTALL` with no critical issue
//! adds a `dangerous` finding (so the verdict is dangerous); an incomplete
//! analysis adds an `info` finding and marks the report incomplete.

use std::path::Path;

use serde_json::{Map, Value};

use super::{SCANNER, VERSION};
use crate::scan::engine::excerpt;
use crate::scan::external::text::{clean, json_of, relative, strip_root};
use crate::scan::external::{ExternalReport, ExternalStatus, NotScanned};
use crate::scan::{Finding, Severity, Verdict};

/// Longest message (the explanation) and detail kept.
const MAX_MESSAGE: usize = 400;
const MAX_DETAIL: usize = 240;
/// Longest rule id and completeness status kept.
const MAX_ID: usize = 64;

/// The rule of the finding a `DO_NOT_INSTALL` recommendation adds.
pub const RULE_DO_NOT_INSTALL: &str = "DO_NOT_INSTALL";
/// The rule of the finding an incomplete analysis adds.
pub const RULE_INCOMPLETE: &str = "incomplete";

/// The first version-looking word of `text` (`skillspector 2.13.7`,
/// `SkillSpector version 2.13.7`, `2.14.0rc1`): digits and dots, then
/// letters, digits, `.`, `+` or `-`; at least one dot.
pub fn version_of(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')')
        .map(|word| word.trim_start_matches(['v', 'V']))
        .find(|word| {
            let mut chars = word.chars();
            chars.next().is_some_and(|c| c.is_ascii_digit())
                && word.contains('.')
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
        })
        .map(|word| word.trim_end_matches('.').to_owned())
}

/// Parses one run: its stdout, the end of its stderr, its exit code.
/// `root` is the staged folder (its absolute form is removed from paths
/// and messages); `version` is the runtime's (the report's own
/// `metadata.skillspector_version` wins when it has one).
pub fn parse(
    stdout: &[u8],
    stderr: &[u8],
    exit: Option<i32>,
    root: &Path,
    version: &str,
) -> ExternalReport {
    if !matches!(exit, Some(0 | 1)) {
        return failed(stderr, exit, root, version);
    }
    let text = String::from_utf8_lossy(stdout);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return ExternalReport::not_understood(SCANNER, version, "the scanner printed nothing");
    }
    let Some(value) = json_of(trimmed) else {
        return ExternalReport::not_understood(SCANNER, version, "the output is not JSON");
    };
    let version = value
        .get("metadata")
        .and_then(|metadata| metadata.get("skillspector_version"))
        .and_then(Value::as_str)
        .map(|reported| clean(reported, MAX_ID))
        .filter(|reported| !reported.is_empty())
        .unwrap_or_else(|| version.to_owned());
    match report(&value, root) {
        Ok((findings, incomplete)) => {
            let verdict = Verdict::of(&findings);
            ExternalReport {
                scanner: SCANNER,
                version,
                status: ExternalStatus::Scanned,
                reason: None,
                verdict,
                findings,
                incomplete,
                content_hash: None,
                detail: None,
            }
        }
        Err(why) => ExternalReport::not_understood(SCANNER, &version, why),
    }
}

/// An exit other than 0 or 1: SkillSpector's error (2), a crash, a kill.
/// uv refusing for want of its cache (`--offline`) is "not prepared".
fn failed(stderr: &[u8], exit: Option<i32>, root: &Path, version: &str) -> ExternalReport {
    let text = String::from_utf8_lossy(stderr);
    let lower = text.to_ascii_lowercase();
    if lower.contains("network was disabled")
        || lower.contains("network is disabled")
        || lower.contains("no interpreter found")
    {
        return ExternalReport::not_scanned(
            SCANNER,
            version,
            NotScanned::NotPrepared,
            Some(format!(
                "SkillSpector {VERSION} (or Python {}) is not in uv's cache; prepare it first \
                 (`loadout scanner prepare`, or Prepare in Settings > Security)",
                super::PYTHON
            )),
        );
    }
    let last = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| clean(&strip_root(line, root), MAX_DETAIL));
    let how = match exit {
        Some(code) => format!("SkillSpector exited with code {code}"),
        None => "SkillSpector was stopped by the system".to_owned(),
    };
    let detail = match last {
        Some(line) => format!("{how}: {line}"),
        None => how,
    };
    ExternalReport::not_scanned(SCANNER, version, NotScanned::Failed, Some(detail))
}

/// The findings (sorted) and whether the analysis was incomplete.
fn report(value: &Value, root: &Path) -> Result<(Vec<Finding>, bool), &'static str> {
    let object = value.as_object().ok_or("the output is not a JSON object")?;
    if object.get("execution_successful").and_then(Value::as_bool) != Some(true) {
        return Err("execution_successful is not true");
    }
    let metadata = object
        .get("metadata")
        .and_then(Value::as_object)
        .ok_or("there is no metadata")?;
    if metadata.get("llm_requested").and_then(Value::as_bool) != Some(false) {
        return Err("metadata.llm_requested is not false");
    }
    let risk = object
        .get("risk_assessment")
        .and_then(Value::as_object)
        .ok_or("there is no risk_assessment")?;
    let do_not_install = match risk.get("recommendation").and_then(Value::as_str) {
        Some("SAFE" | "CAUTION") => false,
        Some("DO_NOT_INSTALL") => true,
        _ => return Err("the recommendation is not SAFE, CAUTION or DO_NOT_INSTALL"),
    };
    let issues = object
        .get("issues")
        .and_then(Value::as_array)
        .ok_or("issues is not a list")?;
    let completeness = object
        .get("analysis_completeness")
        .and_then(Value::as_object)
        .ok_or("there is no analysis_completeness")?;
    let complete = completeness
        .get("is_complete")
        .and_then(Value::as_bool)
        .ok_or("analysis_completeness.is_complete is not true or false")?;

    let mut findings = Vec::with_capacity(issues.len() + 2);
    for issue in issues {
        findings.push(finding(issue, root)?);
    }
    if do_not_install && !findings.iter().any(|f| f.severity == Severity::Dangerous) {
        let score = risk
            .get("score")
            .and_then(Value::as_f64)
            .map(|score| format!(" (risk score {} of 100)", number(score)))
            .unwrap_or_default();
        findings.push(synthetic(
            RULE_DO_NOT_INSTALL,
            Severity::Dangerous,
            format!("SkillSpector recommends not installing this skill{score}"),
        ));
    }
    if !complete {
        findings.push(synthetic(
            RULE_INCOMPLETE,
            Severity::Info,
            incomplete_message(completeness),
        ));
    }
    findings.sort_by(|a, b| (&a.file, a.line, &a.rule).cmp(&(&b.file, b.line, &b.rule)));
    findings.dedup();
    Ok((findings, !complete))
}

/// One `issues[]` entry.
fn finding(issue: &Value, root: &Path) -> Result<Finding, &'static str> {
    let issue = issue.as_object().ok_or("an issue is not an object")?;
    let rule = issue
        .get("id")
        .and_then(Value::as_str)
        .map(|id| clean(id, MAX_ID))
        .filter(|id| !id.is_empty())
        .ok_or("an issue has no id")?;
    let severity = match issue.get("severity").and_then(Value::as_str) {
        Some("CRITICAL") => Severity::Dangerous,
        Some("HIGH" | "MEDIUM") => Severity::Warning,
        Some("LOW") => Severity::Info,
        _ => return Err("an issue's severity is not LOW, MEDIUM, HIGH or CRITICAL"),
    };
    let location = issue.get("location").and_then(Value::as_object);
    let file = location
        .and_then(|location| location.get("file"))
        .and_then(Value::as_str)
        .and_then(|path| relative(path, root));
    let line = location
        .and_then(|location| location.get("start_line"))
        .and_then(Value::as_u64)
        .and_then(|line| u32::try_from(line).ok())
        .filter(|line| *line > 0);
    let message = text_of(issue, &["explanation", "pattern"])
        .map(|text| clean(&strip_root(text, root), MAX_MESSAGE))
        .unwrap_or_else(|| format!("SkillSpector rule {rule}"));
    let shown = issue
        .get("finding")
        .and_then(Value::as_str)
        .map(|text| excerpt(&strip_root(text, root)))
        .filter(|text| !text.is_empty());
    Ok(Finding {
        scanner: SCANNER.as_str().to_owned(),
        rule,
        severity,
        file,
        line,
        message,
        excerpt: shown,
    })
}

/// The first of `keys` that holds non-blank text.
fn text_of<'a>(object: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .filter_map(|key| object.get(*key).and_then(Value::as_str))
        .find(|text| !text.trim().is_empty())
}

fn synthetic(rule: &str, severity: Severity, message: String) -> Finding {
    Finding {
        scanner: SCANNER.as_str().to_owned(),
        rule: rule.to_owned(),
        severity,
        file: None,
        line: None,
        message,
        excerpt: None,
    }
}

/// "not fully scanned (partial, 87.5% coverage)".
fn incomplete_message(completeness: &Map<String, Value>) -> String {
    let status = completeness
        .get("status")
        .and_then(Value::as_str)
        .map(|status| clean(status, MAX_ID))
        .filter(|status| !status.is_empty())
        .unwrap_or_else(|| "partial".to_owned());
    match completeness.get("coverage_percent").and_then(Value::as_f64) {
        Some(coverage) => format!(
            "not fully scanned ({status}, {}% coverage)",
            number(coverage)
        ),
        None => format!("not fully scanned ({status})"),
    }
}

/// `78`, `87.5`: whole numbers without a fraction, else one decimal.
fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

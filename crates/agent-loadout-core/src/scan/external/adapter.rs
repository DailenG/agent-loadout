//! Snyk Agent Scan's `--json` output (the v0.6+ `ScanResponse` schema) as
//! M40 findings. Defensive: anything that is not that schema, or a risk
//! entry that does not parse, is "scanner output not understood", never
//! clean; an `is_failure` error is "not scanned (<reason>)". The exit code
//! is not the verdict (without `--ci` a finished scan exits 0 whatever it
//! found). Empty output after a failure is "token rejected" when the end
//! of stderr names the token, else "failed" with that line as the detail.

use std::path::Path;

use serde_json::{Map, Value};

use super::{ExternalReport, ExternalStatus, NotScanned, SCANNER};
use crate::scan::{Finding, Severity, Verdict};

/// Longest message (Snyk's evidence) and excerpt (URLs) kept.
const MAX_MESSAGE: usize = 400;
const MAX_EXCERPT: usize = 120;
const MAX_DETAIL: usize = 240;

/// The risk whose any score is `dangerous`.
const MALICIOUS_CODE: &str = "malicious_code";

/// Parses one run's stdout. `stderr` is the end of its stderr (token
/// value already masked), used only when stdout is empty. `root` is the
/// folder that was scanned (its absolute form is removed from paths);
/// `version` is the scanner's.
pub fn parse(
    stdout: &[u8],
    stderr: &[u8],
    exit: Option<i32>,
    root: &Path,
    version: &str,
) -> ExternalReport {
    let text = String::from_utf8_lossy(stdout);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return if exit == Some(0) {
            ExternalReport::not_understood(version, "the scanner printed nothing")
        } else {
            failed_without_output(stderr, exit, root, version)
        };
    }
    let Some(value) = json_of(trimmed) else {
        return ExternalReport::not_understood(version, "the output is not JSON");
    };
    match Parsed::from(&value, root) {
        Ok(parsed) => parsed.into_report(version),
        Err(why) => ExternalReport::not_understood(version, why),
    }
}

/// No output and a failure: `prepare_run` resolved a token, so this is
/// Snyk refusing it (`SnykTokenError`, 401) or anything else (a crash, a
/// kill by a signal, the OS refusing the binary).
fn failed_without_output(
    stderr: &[u8],
    exit: Option<i32>,
    root: &Path,
    version: &str,
) -> ExternalReport {
    let text = String::from_utf8_lossy(stderr);
    let lower = text.to_ascii_lowercase();
    if lower.contains("token") || lower.contains("unauthorized") || lower.contains("401") {
        return ExternalReport::not_scanned(
            version,
            NotScanned::TokenRejected,
            Some("Snyk did not accept the token".to_owned()),
        );
    }
    let last = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| clean(&strip_root(line, root), MAX_DETAIL));
    let how = match exit {
        Some(code) => format!("the scanner exited with code {code} and no output"),
        None => "the scanner was stopped by the system and printed nothing".to_owned(),
    };
    let detail = match last {
        Some(line) => format!("{how}: {line}"),
        None => how,
    };
    ExternalReport::not_scanned(version, NotScanned::Failed, Some(detail))
}

/// The JSON document, also when something was printed around it.
fn json_of(text: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str(text) {
        return Some(value);
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (start < end)
        .then(|| serde_json::from_str(&text[start..=end]).ok())
        .flatten()
}

#[derive(Default)]
struct Parsed {
    findings: Vec<Finding>,
    failure: Option<(NotScanned, Option<String>)>,
    skills: usize,
}

impl Parsed {
    fn from(value: &Value, root: &Path) -> Result<Self, &'static str> {
        let responses = value
            .get("scan_path_responses")
            .and_then(Value::as_array)
            .ok_or("no scan_path_responses")?;
        let mut parsed = Parsed::default();
        for response in responses {
            let response = response.as_object().ok_or("a response is not an object")?;
            parsed.error(response.get("error"), root)?;
            for skill in list(response, "skill_risks")? {
                let skill = skill.as_object().ok_or("a skill entry is not an object")?;
                parsed.skills += 1;
                parsed.error(skill.get("error"), root)?;
                parsed.risks(skill.get("risk_indexes"), root)?;
            }
            for server in list(response, "server_risks")? {
                let server = server
                    .as_object()
                    .ok_or("a server entry is not an object")?;
                parsed.error(server.get("error"), root)?;
                parsed.risks(server.get("risk_indexes"), root)?;
            }
        }
        Ok(parsed)
    }

    /// An `is_failure` error: the first one decides the reason.
    fn error(&mut self, error: Option<&Value>, root: &Path) -> Result<(), &'static str> {
        let Some(error) = error.filter(|error| !error.is_null()) else {
            return Ok(());
        };
        let error = error.as_object().ok_or("an error is not an object")?;
        if error.get("is_failure").and_then(Value::as_bool) != Some(true) {
            return Ok(());
        }
        if self.failure.is_none() {
            let message = error.get("message").and_then(Value::as_str).unwrap_or("");
            let category = error.get("category").and_then(Value::as_str).unwrap_or("");
            let reason = classify(message, category);
            let detail = if message.trim().is_empty() {
                (!category.is_empty()).then(|| format!("Snyk Agent Scan error: {category}"))
            } else {
                Some(clean(&strip_root(message, root), MAX_DETAIL))
            };
            self.failure = Some((reason, detail));
        }
        Ok(())
    }

    fn risks(&mut self, risks: Option<&Value>, root: &Path) -> Result<(), &'static str> {
        let Some(risks) = risks.filter(|risks| !risks.is_null()) else {
            return Ok(());
        };
        let risks = risks.as_object().ok_or("risk_indexes is not an object")?;
        for (name, risk) in risks {
            let risk = risk.as_object().ok_or("a risk is not an object")?;
            let score = risk
                .get("score")
                .and_then(Value::as_f64)
                .ok_or("a risk has no score")?;
            let severity = if name == MALICIOUS_CODE || score >= 600.0 {
                Severity::Dangerous
            } else if score >= 300.0 {
                Severity::Warning
            } else {
                Severity::Info
            };
            let evidence = risk.get("evidence").and_then(Value::as_str).unwrap_or("");
            let message = if evidence.trim().is_empty() {
                format!("Snyk Agent Scan reported {name} (score {score})")
            } else {
                clean(&strip_root(evidence, root), MAX_MESSAGE)
            };
            let urls: Vec<&str> = ["malicious_urls", "unverifiable_urls"]
                .iter()
                .filter_map(|key| risk.get(*key).and_then(Value::as_array))
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let excerpt = (!urls.is_empty()).then(|| clean(&urls.join(", "), MAX_EXCERPT));
            let finding = |file: Option<String>, line: Option<u32>| Finding {
                scanner: SCANNER.to_owned(),
                rule: name.clone(),
                severity,
                file,
                line,
                message: message.clone(),
                excerpt: excerpt.clone(),
            };
            let locations = risk
                .get("locations")
                .and_then(Value::as_array)
                .filter(|locations| !locations.is_empty());
            match locations {
                Some(locations) => {
                    for location in locations {
                        let start = location.get("start");
                        let file = start
                            .and_then(|start| start.get("path"))
                            .and_then(Value::as_str)
                            .and_then(|path| relative(path, root));
                        let line = start
                            .and_then(|start| start.get("line"))
                            .and_then(Value::as_u64)
                            .and_then(|line| u32::try_from(line).ok());
                        self.findings.push(finding(file, line));
                    }
                }
                None => self.findings.push(finding(None, None)),
            }
        }
        Ok(())
    }

    fn into_report(mut self, version: &str) -> ExternalReport {
        self.findings
            .sort_by(|a, b| (&a.file, a.line, &a.rule).cmp(&(&b.file, b.line, &b.rule)));
        self.findings.dedup();
        let verdict = Verdict::of(&self.findings);
        let (status, reason, detail) = match self.failure {
            Some((reason, detail)) => (ExternalStatus::NotScanned, Some(reason), detail),
            None if self.skills == 0 => {
                return ExternalReport::not_understood(version, "no skill in the output");
            }
            None => (ExternalStatus::Scanned, None, None),
        };
        ExternalReport {
            scanner: SCANNER.to_owned(),
            version: version.to_owned(),
            status,
            reason,
            verdict,
            findings: self.findings,
            content_hash: None,
            detail,
        }
    }
}

/// `key` of `object` as a list: absent or null is empty; anything else
/// that is not an array does not parse.
fn list<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a [Value], &'static str> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(items)) => Ok(items),
        Some(_) => Err("a risk list is not an array"),
    }
}

/// The reason of an `is_failure` error, from its message and category
/// (Snyk's wording at v0.7.1, `verify_api.py`).
fn classify(message: &str, category: &str) -> NotScanned {
    let lower = message.to_ascii_lowercase();
    if lower.contains("usage limit") {
        NotScanned::LimitReached
    } else if lower.contains("413") || lower.contains("too large") {
        NotScanned::TooLarge
    } else if lower.contains("unauthorized") || lower.contains("401") {
        NotScanned::TokenRejected
    } else if category == "user_declined" {
        NotScanned::WouldStartServer
    } else if lower.contains("could not reach") || lower.contains("tried calling") {
        NotScanned::Unreachable
    } else {
        NotScanned::Failed
    }
}

/// `path` relative to `root`, `/`-separated; `None` for a path outside it
/// (never an absolute path in a finding).
fn relative(path: &str, root: &Path) -> Option<String> {
    let slashed = path.replace('\\', "/");
    let root_text = root.to_string_lossy().replace('\\', "/");
    let root_text = root_text.trim_end_matches('/');
    let rest = if let Some(rest) = strip_prefix_ci(&slashed, root_text) {
        rest.trim_start_matches('/').to_owned()
    } else {
        slashed
    };
    let absolute =
        rest.starts_with('/') || rest.starts_with('~') || rest.as_bytes().get(1) == Some(&b':');
    let escapes = rest.split('/').any(|part| part == "..");
    (!rest.is_empty() && !absolute && !escapes).then_some(rest)
}

/// `text` without `prefix`, ASCII case-insensitively (Windows paths).
fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix.is_empty() || text.len() < prefix.len() || !text.is_char_boundary(prefix.len()) {
        return None;
    }
    let (head, rest) = text.split_at(prefix.len());
    (head.eq_ignore_ascii_case(prefix) && (rest.is_empty() || rest.starts_with('/')))
        .then_some(rest)
}

/// `text` with the scanned folder's path replaced by `<skill>`.
fn strip_root(text: &str, root: &Path) -> String {
    let native = root.to_string_lossy().into_owned();
    if native.is_empty() {
        return text.to_owned();
    }
    let forward = native.replace('\\', "/");
    text.replace(&native, "<skill>")
        .replace(&forward, "<skill>")
}

/// One line: control characters as spaces, invisible characters (bidi
/// controls, zero-width, tags) spelled out as `\u{..}`, runs of spaces
/// collapsed, at most `max` characters (with an ellipsis).
fn clean(text: &str, max: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max + 3));
    let mut space = false;
    for ch in text.chars() {
        if matches!(
            ch,
            '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{2069}'
                | '\u{FEFF}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{E0000}'..='\u{E01EF}'
        ) {
            out.push_str(&format!("\\u{{{:X}}}", u32::from(ch)));
            space = false;
            continue;
        }
        let ch = if ch.is_control() { ' ' } else { ch };
        if ch == ' ' {
            if space || out.is_empty() {
                continue;
            }
            space = true;
        } else {
            space = false;
        }
        out.push(ch);
    }
    let out = out.trim_end().to_owned();
    if out.chars().count() <= max {
        return out;
    }
    let mut cut: String = out.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

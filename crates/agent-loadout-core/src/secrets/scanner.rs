//! Leak scanner (SPEC 11.8). `Store::transaction` runs it over every staged
//! file and the commit message before anything reaches the git object
//! database or the working tree; a hit fails the transaction with
//! `leak_detected`.
//!
//! Two checks:
//!
//! - the classifier ([`crate::secrets::classify`]) at the stored strength:
//!   token formats, URL credentials and private keys anywhere in the text,
//!   and high-entropy values under credential names in TOML files;
//! - exact matches against every value resolved during this process,
//!   kept only as keyed hashes in a [`SecretMemory`].
//!
//! Classifier hits on text a person marked "not secret" pass: the
//! allowlist `store/.loadout-allow` holds one `sha256:<hex>` per line
//! ([`allow_hash`] of the flagged text). Exact matches of resolved values
//! are never allowlisted.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::ops::Range;
use std::sync::{Mutex, PoisonError};

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use toml_edit::{Item, TableLike, Value};

use crate::error::{CoreError, ErrorCode};
use crate::secrets::classify::{self, Reason};

/// The allowlist file at the store root.
pub const ALLOWLIST_FILE: &str = ".loadout-allow";
/// Prefix of allowlist hashes.
const ALLOW_PREFIX: &str = "sha256:";
/// Domain separation for allowlist hashes.
const ALLOW_DOMAIN: &[u8] = b"loadout-allow\0";
/// Resolved values shorter than this are not tracked: a short value would
/// match ordinary text and block unrelated commits.
pub const MIN_TRACKED_LEN: usize = 8;

/// Keyed hashes of the secret values this process has resolved (SPEC
/// 11.8). The key is random per instance and never leaves memory; values
/// themselves are never kept.
pub struct SecretMemory {
    key: [u8; 32],
    by_len: Mutex<BTreeMap<usize, HashSet<[u8; 32]>>>,
}

impl fmt::Debug for SecretMemory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretMemory")
            .field("tracked", &self.len())
            .finish_non_exhaustive()
    }
}

impl SecretMemory {
    pub fn new() -> Result<Self, CoreError> {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|err| {
            CoreError::new(
                ErrorCode::IoError,
                format!("cannot read random bytes for the secret memory key: {err}"),
            )
        })?;
        Ok(Self {
            key,
            by_len: Mutex::new(BTreeMap::new()),
        })
    }

    /// Remembers `value` (and its trimmed form) for exact-match scanning.
    pub fn record(&self, value: &str) {
        let mut by_len = self.by_len.lock().unwrap_or_else(PoisonError::into_inner);
        for form in [value, value.trim()] {
            if form.len() >= MIN_TRACKED_LEN {
                by_len
                    .entry(form.len())
                    .or_default()
                    .insert(self.digest(form.as_bytes()));
            }
        }
    }

    /// How many distinct values are tracked.
    pub fn len(&self) -> usize {
        self.by_len
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .map(HashSet::len)
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The first byte range of `haystack` equal to a tracked value.
    pub fn find(&self, haystack: &[u8]) -> Option<Range<usize>> {
        let by_len = self.by_len.lock().unwrap_or_else(PoisonError::into_inner);
        let mut first: Option<Range<usize>> = None;
        for (&len, digests) in by_len.iter() {
            if len > haystack.len() {
                break;
            }
            let limit = first.as_ref().map_or(haystack.len(), |found| found.start);
            let hit = (0..=haystack.len() - len)
                .take_while(|start| *start < limit)
                .find(|&start| digests.contains(&self.digest(&haystack[start..start + len])));
            if let Some(start) = hit {
                first = Some(start..start + len);
            }
        }
        first
    }

    fn digest(&self, bytes: &[u8]) -> [u8; 32] {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.key)
            .expect("HMAC accepts any key length");
        mac.update(bytes);
        mac.finalize().into_bytes().into()
    }
}

/// `sha256:<hex>` of text a person marked as not secret.
pub fn allow_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(ALLOW_DOMAIN);
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(ALLOW_PREFIX.len() + 64);
    out.push_str(ALLOW_PREFIX);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// `store/.loadout-allow`: one hash per line; `#` starts a comment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Allowlist(BTreeSet<String>);

impl Allowlist {
    pub fn parse(text: &str) -> Self {
        Self(
            text.lines()
                .map(|line| line.split('#').next().unwrap_or("").trim())
                .filter(|line| line.starts_with(ALLOW_PREFIX))
                .map(str::to_owned)
                .collect(),
        )
    }

    pub fn contains(&self, hash: &str) -> bool {
        self.0.contains(hash)
    }

    /// Appends `hash` to the file text `existing` (if not listed yet).
    pub fn append(existing: Option<&str>, hash: &str) -> String {
        let mut text = existing.unwrap_or_default().to_owned();
        if Self::parse(&text).contains(hash) {
            return text;
        }
        if text.is_empty() {
            text.push_str("# Values marked as not secret (SPEC 11.7); hashes only.\n");
        } else if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(hash);
        text.push('\n');
        text
    }
}

/// What a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeakKind {
    /// The classifier flagged the text.
    Classified(Reason),
    /// The text holds a value resolved during this process.
    ResolvedValue,
}

impl LeakKind {
    /// Stable `snake_case` name: the classifier reason, or
    /// `resolved_value`.
    pub const fn as_str(self) -> &'static str {
        match self {
            LeakKind::Classified(reason) => reason.as_str(),
            LeakKind::ResolvedValue => "resolved_value",
        }
    }
}

/// One possible leak. Never holds the flagged text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeakFinding {
    /// Store-relative path, or `commit message`.
    pub file: String,
    /// 1-based line.
    pub line: Option<usize>,
    /// Dotted key path in a TOML file (`env.API_KEY`).
    pub key: Option<String>,
    pub kind: LeakKind,
}

impl fmt::Display for LeakFinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.file)?;
        if let Some(line) = self.line {
            write!(f, " line {line}")?;
        }
        if let Some(key) = &self.key {
            write!(f, " ({key})")?;
        }
        match self.kind {
            LeakKind::Classified(reason) => write!(f, ": {}", reason.describe()),
            LeakKind::ResolvedValue => f.write_str(": holds a secret value resolved by Loadout"),
        }
    }
}

/// Scans one file (or the commit message). `toml` enables the key-aware
/// checks for store TOML files.
pub fn scan_text(
    file: &str,
    bytes: &[u8],
    toml: bool,
    memory: &SecretMemory,
    allow: &Allowlist,
) -> Vec<LeakFinding> {
    let mut findings = Vec::new();
    if let Some(range) = memory.find(bytes) {
        findings.push(LeakFinding {
            file: file.to_owned(),
            line: Some(line_of(bytes, range.start)),
            key: None,
            kind: LeakKind::ResolvedValue,
        });
    }
    let text = String::from_utf8_lossy(bytes);
    for hit in classify::find_in_text(&text) {
        if allow.contains(&allow_hash(&text[hit.range.clone()])) {
            continue;
        }
        findings.push(LeakFinding {
            file: file.to_owned(),
            line: Some(line_of(text.as_bytes(), hit.range.start)),
            key: None,
            kind: LeakKind::Classified(hit.reason),
        });
    }
    if toml && let Ok(doc) = toml_edit::Document::parse(text.as_ref()) {
        let mut keyed = Vec::new();
        walk_table(doc.as_table(), "", &mut keyed);
        for (key, value, span) in keyed {
            if allow.contains(&allow_hash(&value)) {
                continue;
            }
            findings.push(LeakFinding {
                file: file.to_owned(),
                line: span.map(|span| line_of(text.as_bytes(), span.start)),
                key: Some(key),
                kind: LeakKind::Classified(Reason::HighEntropy),
            });
        }
    }
    findings
}

/// The `leak_detected` error for `findings`, or `None` when empty.
pub fn leak_error(findings: &[LeakFinding]) -> Option<CoreError> {
    if findings.is_empty() {
        return None;
    }
    let listed: Vec<String> = findings.iter().map(ToString::to_string).collect();
    Some(CoreError::new(
        ErrorCode::LeakDetected,
        format!(
            "refusing to commit a possible secret: {}. Store files hold references \
             (`${{secret:ALIAS}}`), never values; mark text that is not secret in {ALLOWLIST_FILE}",
            listed.join("; ")
        ),
    ))
}

fn line_of(bytes: &[u8], offset: usize) -> usize {
    bytes[..offset.min(bytes.len())]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}

type Keyed = Vec<(String, String, Option<Range<usize>>)>;

/// High-entropy values under credential names: string values by their
/// key, and argument lists (`--token VALUE`). Token formats and URLs were
/// already found in the raw text.
fn walk_table(table: &dyn TableLike, prefix: &str, out: &mut Keyed) {
    for (key, item) in table.iter() {
        let path = if prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{prefix}.{key}")
        };
        match item {
            Item::Value(value) => walk_value(key, value, &path, out),
            Item::Table(table) => walk_table(table, &path, out),
            Item::ArrayOfTables(array) => {
                for (index, table) in array.iter().enumerate() {
                    walk_table(table, &format!("{path}[{index}]"), out);
                }
            }
            Item::None => {}
        }
    }
}

fn walk_value(key: &str, value: &Value, path: &str, out: &mut Keyed) {
    match value {
        Value::String(text) => {
            if classify::classify_stored(Some(key), text.value()) == Some(Reason::HighEntropy) {
                out.push((path.to_owned(), text.value().clone(), value.span()));
            }
        }
        Value::Array(array) => {
            let strings: Vec<(usize, &Value)> = array
                .iter()
                .enumerate()
                .filter(|(_, item)| item.is_str())
                .collect();
            let args: Vec<String> = strings
                .iter()
                .filter_map(|(_, item)| item.as_str().map(str::to_owned))
                .collect();
            for ((index, item), reason) in strings.iter().zip(classify::classify_args_stored(&args))
            {
                if reason == Some(Reason::HighEntropy) {
                    let text = item.as_str().unwrap_or_default().to_owned();
                    out.push((format!("{path}[{index}]"), text, item.span()));
                }
            }
            for (index, item) in array.iter().enumerate() {
                if !item.is_str() {
                    walk_value(key, item, &format!("{path}[{index}]"), out);
                }
            }
        }
        Value::InlineTable(table) => walk_table(table, path, out),
        Value::Integer(_) | Value::Float(_) | Value::Boolean(_) | Value::Datetime(_) => {}
    }
}

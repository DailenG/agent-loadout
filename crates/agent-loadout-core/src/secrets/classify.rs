// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan

//! Secret classifier (SPEC 11.7), shared by import and the leak scanner.
//!
//! A literal value is sensitive when
//!
//! - its key (env var, header, `--flag` name) matches
//!   `(?i)(token|secret|password|passwd|api[_-]?key|auth|credential|private[_-]?key|bearer)`,
//!   or is one of SkillDock's `SENSITIVE_ARGUMENT_NAMES`;
//! - it holds a known token format: `ghp_`, `github_pat_`, `gho_`, `sk-`,
//!   `sk-ant-`, `xox[abposr]-`, `AKIA` plus 16, `AIza`, `glpat-`, a PEM
//!   private key block or a JWT;
//! - it holds a URL with userinfo or a token-like query parameter;
//! - or it is longer than 20 characters with more than 3.5 bits of entropy
//!   per character under a sensitive key.
//!
//! References (`${env:..}`, `${secret:..}`) are never sensitive; in a value
//! that mixes them with text only the text is classified.
//!
//! Two strengths: [`classify_import`] flags any literal under a sensitive
//! key, because a person confirms every hit in the import wizard. The leak
//! scanner uses [`classify_stored`], which needs evidence in the value
//! itself (a token format, URL credentials, or high entropy under a
//! sensitive key), so `AUTH_MODE = "oauth"` can still be committed.
//!
//! No regular expression engine: the patterns are matched by hand.

use std::ops::Range;

use crate::secrets::grammar::{holds_reference, literal_parts};

/// Why a value counts as sensitive. Never carries the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reason {
    /// The key or `--flag` name marks a credential.
    SensitiveKey,
    /// The value holds a known token format.
    TokenFormat(TokenKind),
    /// A URL holds userinfo (`https://user:pass@host`).
    UrlUserinfo,
    /// A URL query parameter is named like a credential (`?access_token=`).
    UrlTokenParameter,
    /// High entropy (more than 3.5 bits per character over 20 or more
    /// characters) under a sensitive key.
    HighEntropy,
}

impl Reason {
    /// Stable `snake_case` name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Reason::SensitiveKey => "sensitive_key",
            Reason::TokenFormat(_) => "token_format",
            Reason::UrlUserinfo => "url_userinfo",
            Reason::UrlTokenParameter => "url_token_parameter",
            Reason::HighEntropy => "high_entropy",
        }
    }

    /// English description for messages.
    pub fn describe(self) -> String {
        match self {
            Reason::SensitiveKey => "named like a credential".to_owned(),
            Reason::TokenFormat(kind) => format!("looks like {}", kind.describe()),
            Reason::UrlUserinfo => "a URL with a user name or password".to_owned(),
            Reason::UrlTokenParameter => "a URL with a credential query parameter".to_owned(),
            Reason::HighEntropy => "a high-entropy value under a credential name".to_owned(),
        }
    }
}

/// Known token formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// `ghp_...`
    GithubClassic,
    /// `github_pat_...`
    GithubFineGrained,
    /// `gho_...`
    GithubOauth,
    /// `sk-...`
    OpenAi,
    /// `sk-ant-...`
    Anthropic,
    /// `xoxa-`, `xoxb-`, `xoxp-`, `xoxo-`, `xoxs-`, `xoxr-`
    Slack,
    /// `AKIA` followed by 16 upper-case letters or digits.
    AwsAccessKey,
    /// `AIza...`
    GoogleApiKey,
    /// `glpat-...`
    GitlabToken,
    /// `-----BEGIN ... PRIVATE KEY-----`
    PrivateKey,
    /// Three base64url segments, the first two JSON objects (`eyJ`).
    Jwt,
}

impl TokenKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            TokenKind::GithubClassic => "github_classic",
            TokenKind::GithubFineGrained => "github_fine_grained",
            TokenKind::GithubOauth => "github_oauth",
            TokenKind::OpenAi => "openai",
            TokenKind::Anthropic => "anthropic",
            TokenKind::Slack => "slack",
            TokenKind::AwsAccessKey => "aws_access_key",
            TokenKind::GoogleApiKey => "google_api_key",
            TokenKind::GitlabToken => "gitlab_token",
            TokenKind::PrivateKey => "private_key",
            TokenKind::Jwt => "jwt",
        }
    }

    const fn describe(self) -> &'static str {
        match self {
            TokenKind::GithubClassic => "a GitHub personal access token",
            TokenKind::GithubFineGrained => "a GitHub fine-grained token",
            TokenKind::GithubOauth => "a GitHub OAuth token",
            TokenKind::OpenAi => "an OpenAI-style API key",
            TokenKind::Anthropic => "an Anthropic API key",
            TokenKind::Slack => "a Slack token",
            TokenKind::AwsAccessKey => "an AWS access key id",
            TokenKind::GoogleApiKey => "a Google API key",
            TokenKind::GitlabToken => "a GitLab token",
            TokenKind::PrivateKey => "a private key",
            TokenKind::Jwt => "a JSON Web Token",
        }
    }
}

/// One classifier hit inside a text: what and where (byte range of the
/// sensitive part).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub reason: Reason,
    pub range: Range<usize>,
}

/// Words that mark a credential in a key, matched case-insensitively
/// anywhere in the key (`api[_-]?key` and `private[_-]?key` spelled out).
const SENSITIVE_WORDS: [&str; 13] = [
    "token",
    "secret",
    "password",
    "passwd",
    "apikey",
    "api_key",
    "api-key",
    "auth",
    "credential",
    "privatekey",
    "private_key",
    "private-key",
    "bearer",
];

/// SkillDock's `SENSITIVE_ARGUMENT_NAMES` (`mcp_manager.rs`): an argument
/// whose name, lower-cased with `_` as `-`, equals one of these or ends in
/// `-<name>`.
pub const SENSITIVE_ARGUMENT_NAMES: [&str; 8] = [
    "api-key",
    "apikey",
    "authorization",
    "password",
    "secret",
    "token",
    "access-token",
    "auth-token",
];

/// Query parameter names that carry credentials besides the sensitive
/// words.
const TOKEN_PARAMETERS: [&str; 3] = ["key", "sig", "signature"];

/// Shortest value [`Reason::HighEntropy`] applies to.
pub const ENTROPY_MIN_LEN: usize = 20;
/// Bits per character above which a value counts as random.
pub const ENTROPY_THRESHOLD: f64 = 3.5;

/// Whether a key (env var, header, argument or parameter name) marks a
/// credential.
pub fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SENSITIVE_WORDS.iter().any(|word| lower.contains(word)) || is_sensitive_argument_name(key)
}

/// SkillDock's argument rule: `--api-key`, `--github-token=x`, `AUTH_TOKEN`.
pub fn is_sensitive_argument_name(argument: &str) -> bool {
    let name = argument
        .trim()
        .trim_start_matches('-')
        .split('=')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace('_', "-");
    SENSITIVE_ARGUMENT_NAMES
        .iter()
        .any(|known| name == *known || name.ends_with(&format!("-{known}")))
}

/// Shannon entropy of `value` in bits per character.
pub fn entropy(value: &str) -> f64 {
    let chars: Vec<char> = value.chars().collect();
    if chars.is_empty() {
        return 0.0;
    }
    let mut counts = std::collections::BTreeMap::<char, usize>::new();
    for c in &chars {
        *counts.entry(*c).or_default() += 1;
    }
    let total = chars.len() as f64;
    counts
        .values()
        .map(|&count| {
            let p = count as f64 / total;
            -p * p.log2()
        })
        .sum()
}

/// More than [`ENTROPY_THRESHOLD`] bits per character over at least
/// [`ENTROPY_MIN_LEN`] characters.
pub fn is_high_entropy(value: &str) -> bool {
    value.chars().count() >= ENTROPY_MIN_LEN && entropy(value) > ENTROPY_THRESHOLD
}

/// A high-entropy single word: no whitespace, and not a URL or a path
/// (those look random too, and URLs have their own rules).
fn looks_random(value: &str) -> bool {
    let path_like = value.contains("://")
        || value.contains('\\')
        || value.starts_with(['/', '~', '.'])
        || value.as_bytes().get(1) == Some(&b':');
    !path_like && !value.chars().any(char::is_whitespace) && is_high_entropy(value)
}

/// Classifies one imported literal value (SPEC 11.7): `key` is the env var
/// or header name, or the `--flag` an argument belongs to. Any literal
/// under a sensitive key counts.
pub fn classify_import(key: Option<&str>, value: &str) -> Option<Reason> {
    classify(key, value, Strength::Import)
}

/// Classifies a value already in a store file (SPEC 11.8): a sensitive key
/// alone is not enough; the value must hold a token format or URL
/// credentials, or be high-entropy under a sensitive key.
pub fn classify_stored(key: Option<&str>, value: &str) -> Option<Reason> {
    classify(key, value, Strength::Stored)
}

/// Classifies an argument list, one result per argument: each argument on
/// its own, `--token=VALUE` as `VALUE` under `token`, and `--token VALUE`
/// as the next argument under `token`.
pub fn classify_args_import(args: &[String]) -> Vec<Option<Reason>> {
    classify_args(args, Strength::Import)
}

/// [`classify_args_import`] at the leak scanner's strength.
pub fn classify_args_stored(args: &[String]) -> Vec<Option<Reason>> {
    classify_args(args, Strength::Stored)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strength {
    Import,
    Stored,
}

fn classify(key: Option<&str>, value: &str, strength: Strength) -> Option<Reason> {
    if value.trim().is_empty() {
        return None;
    }
    for part in literal_parts(value) {
        if let Some(hit) = find_tokens(&part).into_iter().next() {
            return Some(hit.reason);
        }
    }
    if let Some(hit) = find_url_credentials(value).into_iter().next() {
        return Some(hit.reason);
    }
    if holds_reference(value) {
        return None;
    }
    let sensitive = key.is_some_and(is_sensitive_key);
    match strength {
        Strength::Import if sensitive => Some(Reason::SensitiveKey),
        _ if sensitive && looks_random(value) => Some(Reason::HighEntropy),
        _ => None,
    }
}

fn classify_args(args: &[String], strength: Strength) -> Vec<Option<Reason>> {
    let mut results = Vec::with_capacity(args.len());
    let mut pending_key: Option<&str> = None;
    for arg in args {
        let key = pending_key.take();
        let flag = arg.starts_with('-');
        let result = match (flag, arg.split_once('=')) {
            (true, Some((name, value))) => classify(Some(name), value, strength),
            (true, None) => {
                if is_sensitive_key(arg) {
                    pending_key = Some(arg);
                }
                classify(None, arg, strength)
            }
            (false, _) => classify(key, arg, strength),
        };
        results.push(result);
    }
    results
}

// Token formats.

/// `(prefix, kind, shortest tail)`, longer prefixes first.
const PREFIXED: [(&str, TokenKind, usize); 14] = [
    ("github_pat_", TokenKind::GithubFineGrained, 20),
    ("ghp_", TokenKind::GithubClassic, 20),
    ("gho_", TokenKind::GithubOauth, 20),
    ("sk-ant-", TokenKind::Anthropic, 20),
    ("sk-", TokenKind::OpenAi, 20),
    ("glpat-", TokenKind::GitlabToken, 20),
    ("AIza", TokenKind::GoogleApiKey, 30),
    ("xoxa-", TokenKind::Slack, 10),
    ("xoxb-", TokenKind::Slack, 10),
    ("xoxp-", TokenKind::Slack, 10),
    ("xoxo-", TokenKind::Slack, 10),
    ("xoxs-", TokenKind::Slack, 10),
    ("xoxr-", TokenKind::Slack, 10),
    ("AKIA", TokenKind::AwsAccessKey, 16),
];

fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn is_base64url(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Every token-format hit in `text`.
pub fn find_tokens(text: &str) -> Vec<Hit> {
    let bytes = text.as_bytes();
    let mut hits = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let boundary = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let found = if boundary {
            prefixed_at(bytes, i).or_else(|| jwt_at(bytes, i))
        } else {
            None
        };
        match found {
            Some((kind, end)) => {
                hits.push(Hit {
                    reason: Reason::TokenFormat(kind),
                    range: i..end,
                });
                i = end;
            }
            None => i += 1,
        }
    }
    hits.extend(private_keys(text));
    hits
}

fn prefixed_at(bytes: &[u8], start: usize) -> Option<(TokenKind, usize)> {
    let rest = &bytes[start..];
    for (prefix, kind, min_tail) in PREFIXED {
        if !rest.starts_with(prefix.as_bytes()) {
            continue;
        }
        let tail_start = start + prefix.len();
        if kind == TokenKind::AwsAccessKey {
            let tail = bytes.get(tail_start..tail_start + 16)?;
            let after = bytes.get(tail_start + 16).copied();
            let valid = tail
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
                && !after.is_some_and(|b| b.is_ascii_alphanumeric());
            return valid.then_some((kind, tail_start + 16));
        }
        let tail = bytes[tail_start..]
            .iter()
            .take_while(|b| is_token_char(**b))
            .count();
        // `sk-ant-` shorter than its minimum must not fall through to `sk-`.
        return (tail >= min_tail).then_some((kind, tail_start + tail));
    }
    None
}

/// `eyJ...` `.` `eyJ...` `.` signature, each segment at least 10
/// base64url characters.
fn jwt_at(bytes: &[u8], start: usize) -> Option<(TokenKind, usize)> {
    let segment = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| is_base64url(**b))
            .count()
    };
    let header = segment(start);
    if !bytes[start..].starts_with(b"eyJ") || header < 10 {
        return None;
    }
    let second = start + header + 1;
    if bytes.get(start + header) != Some(&b'.') || !bytes[second..].starts_with(b"eyJ") {
        return None;
    }
    let payload = segment(second);
    let third = second + payload + 1;
    if payload < 10 || bytes.get(second + payload) != Some(&b'.') {
        return None;
    }
    let signature = segment(third);
    (signature >= 10).then_some((TokenKind::Jwt, third + signature))
}

/// `-----BEGIN <...>PRIVATE KEY-----` through its `-----END` line (or the
/// end of the text).
fn private_keys(text: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    let mut from = 0;
    while let Some(offset) = text[from..].find("-----BEGIN ") {
        let start = from + offset;
        let header_end = text[start..]
            .find('\n')
            .map_or(text.len(), |end| start + end);
        let header = &text[start..header_end];
        if !(header.contains("PRIVATE KEY") && header[11..].contains("-----")) {
            from = start + 11;
            continue;
        }
        let end = text[header_end..]
            .find("-----END ")
            .map(|offset| header_end + offset)
            .map_or(text.len(), |end_start| {
                text[end_start..]
                    .find('\n')
                    .map_or(text.len(), |nl| end_start + nl)
            });
        hits.push(Hit {
            reason: Reason::TokenFormat(TokenKind::PrivateKey),
            range: start..end,
        });
        from = end.max(start + 11);
    }
    hits
}

// URLs.

/// Userinfo and credential query parameters of every `scheme://` URL in
/// `text`. A component that is only references (`${secret:X}`) is fine.
pub fn find_url_credentials(text: &str) -> Vec<Hit> {
    let bytes = text.as_bytes();
    let mut hits = Vec::new();
    let mut from = 0;
    while let Some(offset) = text[from..].find("://") {
        let sep = from + offset;
        let scheme_start = bytes[..sep]
            .iter()
            .rposition(|b| !(b.is_ascii_alphanumeric() || b"+.-".contains(b)))
            .map_or(0, |i| i + 1);
        from = sep + 3;
        if scheme_start == sep || !bytes[scheme_start].is_ascii_alphabetic() {
            continue;
        }
        let end = bytes[from..]
            .iter()
            .position(|b| b.is_ascii_whitespace() || b"\"'<>`".contains(b))
            .map_or(text.len(), |i| from + i);
        url_hits(text, from, end, &mut hits);
        from = end.max(from);
    }
    hits
}

fn url_hits(text: &str, start: usize, end: usize, hits: &mut Vec<Hit>) {
    let url = &text[start..end];
    let authority_len = url.find(['/', '?', '#']).unwrap_or(url.len());
    let authority = &url[..authority_len];
    if let Some(at) = authority.rfind('@') {
        let userinfo = &authority[..at];
        let (user, password) = match colon_outside_references(userinfo) {
            Some(colon) => (&userinfo[..colon], Some(&userinfo[colon + 1..])),
            None => (userinfo, None),
        };
        let flagged = match password {
            Some(password) => is_literal(password) || (password.is_empty() && is_literal(user)),
            None => is_literal(user),
        };
        if flagged {
            hits.push(Hit {
                reason: Reason::UrlUserinfo,
                range: start..start + at,
            });
        }
    }
    let Some(query_start) = url.find('?') else {
        return;
    };
    let query_end = url[query_start..]
        .find('#')
        .map_or(url.len(), |i| query_start + i);
    let mut offset = start + query_start + 1;
    for pair in url[query_start + 1..query_end].split('&') {
        if let Some((name, value)) = pair.split_once('=') {
            let named = is_sensitive_key(name)
                || TOKEN_PARAMETERS
                    .iter()
                    .any(|known| name.eq_ignore_ascii_case(known));
            if named && is_literal(value) {
                let value_start = offset + name.len() + 1;
                hits.push(Hit {
                    reason: Reason::UrlTokenParameter,
                    range: value_start..value_start + value.len(),
                });
            }
        }
        offset += pair.len() + 1;
    }
}

/// The first `:` not inside a `${...}` reference.
fn colon_outside_references(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate() {
        match b {
            b'{' if i > 0 && bytes[i - 1] == b'$' => depth += 1,
            b'}' if depth > 0 => depth -= 1,
            b':' if depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// Non-empty and not made of references only.
fn is_literal(component: &str) -> bool {
    !component.is_empty() && literal_parts(component).iter().any(|part| !part.is_empty())
}

/// Every token-format and URL-credential hit in arbitrary text, sorted by
/// position: what the leak scanner looks for in any staged file.
pub fn find_in_text(text: &str) -> Vec<Hit> {
    let mut hits = find_tokens(text);
    hits.extend(find_url_credentials(text));
    hits.sort_by_key(|hit| (hit.range.start, hit.range.end));
    hits.dedup_by(|later, earlier| later.range.start < earlier.range.end);
    hits
}

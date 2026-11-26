//! Runs rules over files and command lines.

use std::fmt::Write as _;
use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;

use super::rules::{Rule, Rules, Target};
use super::{Finding, SCANNER, Severity};
use crate::secrets::classify::{
    find_in_text, is_high_entropy, is_sensitive_argument_name, is_sensitive_key,
};
use crate::skills::SkillFiles;

/// Findings of one rule in one file (or command line), at most: a pattern
/// that matches on every line says so a few times, not thousands.
const MAX_PER_RULE: usize = 5;
/// Characters of matched text an excerpt shows.
const EXCERPT_CHARS: usize = 120;
/// Bytes of matched text an excerpt is made from.
const EXCERPT_SOURCE_BYTES: usize = 600;
/// Text scanned per file; the rest of a larger file is reported as not
/// scanned (`not-fully-scanned`), never silently passed.
pub(super) const MAX_SCAN_BYTES: usize = 4 * 1024 * 1024;

/// Engine findings (not in the rules file).
pub(super) const NOT_FULLY_SCANNED: &str = "not-fully-scanned";
pub(super) const NOT_CONTENT_SCANNED: &str = "not-content-scanned";
const NATIVE_BINARY: &str = "native-binary";

const MARKDOWN_EXTENSIONS: [&str; 6] = ["md", "markdown", "mdx", "txt", "rst", "adoc"];

/// What a file is, by its first bytes (then its name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text(Target),
    /// Images, fonts, archives, media: path rules only.
    Media,
    /// ELF, PE, Mach-O, WebAssembly: path rules and `native-binary`.
    Native,
}

fn extension(path: &str) -> Option<String> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty()).then(|| ext.to_ascii_lowercase())
}

fn is_native(bytes: &[u8]) -> bool {
    const MACH_O: [[u8; 4]; 5] = [
        [0xFE, 0xED, 0xFA, 0xCE],
        [0xFE, 0xED, 0xFA, 0xCF],
        [0xCE, 0xFA, 0xED, 0xFE],
        [0xCF, 0xFA, 0xED, 0xFE],
        [0xCA, 0xFE, 0xBA, 0xBE],
    ];
    let pe = || {
        let Some(offset) = bytes.get(0x3C..0x40) else {
            return false;
        };
        let offset = u32::from_le_bytes([offset[0], offset[1], offset[2], offset[3]]) as usize;
        bytes.get(offset..offset + 4) == Some(b"PE\0\0")
    };
    bytes.starts_with(b"\x7fELF")
        || bytes.starts_with(b"\0asm")
        || MACH_O.iter().any(|magic| bytes.starts_with(magic))
        || (bytes.starts_with(b"MZ") && pe())
}

fn is_media(bytes: &[u8]) -> bool {
    const MAGIC: [&[u8]; 18] = [
        b"\x89PNG\r\n\x1a\n",
        b"\xFF\xD8\xFF",
        b"GIF87a",
        b"GIF89a",
        b"%PDF-",
        b"PK\x03\x04",
        b"\x1F\x8B",
        b"7z\xBC\xAF\x27\x1C",
        b"Rar!\x1A\x07",
        b"\xFD7zXZ\0",
        b"wOFF",
        b"wOF2",
        b"OTTO",
        b"\0\x01\0\0\0",
        b"\0\0\x01\0",
        b"OggS",
        b"\x1A\x45\xDF\xA3",
        b"fLaC",
    ];
    MAGIC.iter().any(|magic| bytes.starts_with(magic))
        || (bytes.starts_with(b"RIFF") && bytes.len() >= 12)
        || bytes.get(4..8) == Some(b"ftyp")
}

/// What a file is scanned as. A file named like an image that is not one
/// is scanned as code; a native executable is one whatever its name.
fn kind_of(path: &str, bytes: &[u8]) -> Kind {
    if is_native(bytes) {
        return Kind::Native;
    }
    if is_media(bytes) {
        return Kind::Media;
    }
    match extension(path) {
        Some(ext) if MARKDOWN_EXTENSIONS.contains(&ext.as_str()) => Kind::Text(Target::Markdown),
        Some(_) => Kind::Text(Target::Code),
        None if bytes.starts_with(b"#!") => Kind::Text(Target::Code),
        None => Kind::Text(Target::Markdown),
    }
}

fn utf16(bytes: &[u8], little: bool) -> String {
    let (pairs, _) = bytes.as_chunks::<2>();
    let units: Vec<u16> = pairs
        .iter()
        .map(|&pair| {
            if little {
                u16::from_le_bytes(pair)
            } else {
                u16::from_be_bytes(pair)
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

fn utf32(bytes: &[u8], little: bool) -> String {
    let (quads, _) = bytes.as_chunks::<4>();
    quads
        .iter()
        .map(|&quad| {
            let word = if little {
                u32::from_le_bytes(quad)
            } else {
                u32::from_be_bytes(quad)
            };
            char::from_u32(word).unwrap_or(char::REPLACEMENT_CHARACTER)
        })
        .collect()
}

/// The text of a file: UTF-32 or UTF-16 by BOM, UTF-16 without a BOM by
/// its NUL pattern (ASCII text in UTF-16 has a NUL in every other byte),
/// else UTF-8 with the BOM removed and invalid bytes replaced. PowerShell
/// runs UTF-16 scripts, so they must not read as NUL-interleaved noise.
fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE\0\0") {
        return utf32(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(b"\0\0\xFE\xFF") {
        return utf32(rest, false);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        return utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        return utf16(rest, false);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        return String::from_utf8_lossy(rest).into_owned();
    }
    let sample = &bytes[..bytes.len().min(4096)];
    if sample.len() >= 4 {
        let half = (sample.len() / 2) as f64;
        let nul_at = |parity: usize| {
            sample
                .iter()
                .skip(parity)
                .step_by(2)
                .filter(|b| **b == 0)
                .count() as f64
                / half
        };
        let (even, odd) = (nul_at(0), nul_at(1));
        if odd > 0.3 && even < 0.05 {
            return utf16(bytes, true);
        }
        if even > 0.3 && odd < 0.05 {
            return utf16(bytes, false);
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// The longest prefix of `text` of at most `max` bytes, on a character
/// boundary.
fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Byte offsets of line starts, for line numbers by binary search.
struct Lines(Vec<usize>);

impl Lines {
    fn of(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
        Self(starts)
    }

    /// 1-based line of byte `offset`.
    fn at(&self, offset: usize) -> u32 {
        let line = self.0.partition_point(|start| *start <= offset);
        u32::try_from(line).unwrap_or(u32::MAX)
    }
}

/// Byte ranges of fenced code blocks (``` or ~~~) in Markdown; an
/// unclosed fence runs to the end.
fn fenced_blocks(text: &str) -> Vec<Range<usize>> {
    let mut blocks = Vec::new();
    let mut open: Option<(usize, char)> = None;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let fence = ['`', '~']
            .into_iter()
            .find(|mark| trimmed.starts_with(&mark.to_string().repeat(3)));
        match (open, fence) {
            (None, Some(mark)) => open = Some((at, mark)),
            (Some((start, mark)), Some(close)) if close == mark => {
                blocks.push(start..at + line.len());
                open = None;
            }
            _ => {}
        }
        at += line.len();
    }
    if let Some((start, _)) = open {
        blocks.push(start..text.len());
    }
    blocks
}

pub(super) fn scan_files(rules: &Rules, files: &SkillFiles) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut not_read = Vec::new();
    for (path, bytes) in files {
        let kind = kind_of(path, bytes);
        let decoded = matches!(kind, Kind::Text(_)).then(|| decode(bytes));
        let text = decoded.as_deref().map(|text| {
            if text.len() > MAX_SCAN_BYTES {
                findings.push(engine_finding(
                    NOT_FULLY_SCANNED,
                    Severity::Warning,
                    Some(path),
                    format!(
                        "Only the first {} MiB of this file were scanned; review the rest yourself.",
                        MAX_SCAN_BYTES / (1024 * 1024)
                    ),
                ));
            }
            cut(text, MAX_SCAN_BYTES)
        });
        let lines = text.map(Lines::of);
        let fences = match (kind, text) {
            (Kind::Text(Target::Markdown), Some(text)) => fenced_blocks(text),
            _ => Vec::new(),
        };
        let before = findings.len();
        for rule in rules.rules() {
            if rule.path.as_ref().is_some_and(|re| !re.is_match(path)) {
                continue;
            }
            if rule.pattern.is_none() {
                findings.push(finding(rule, rule.severity, Some(path), None, None));
                continue;
            }
            if let (Kind::Text(target), Some(text), Some(lines)) = (kind, text, &lines)
                && rule.applies_to(target)
            {
                let file = File {
                    path,
                    lines,
                    prose: target == Target::Markdown,
                    fences: &fences,
                };
                matches(rule, text, Some(&file), &mut findings);
            }
        }
        match kind {
            Kind::Text(_) => {}
            Kind::Media => not_read.push(path.as_str()),
            Kind::Native => {
                not_read.push(path.as_str());
                let flagged = findings[before..]
                    .iter()
                    .any(|finding| finding.rule == NATIVE_BINARY);
                if !flagged {
                    findings.push(match rules.rules().iter().find(|rule| rule.id == NATIVE_BINARY) {
                        Some(rule) => finding(rule, rule.severity, Some(path), None, None),
                        None => engine_finding(
                            NATIVE_BINARY,
                            Severity::Warning,
                            Some(path),
                            "Ships a native executable, which cannot be reviewed by reading it.".to_owned(),
                        ),
                    });
                }
            }
        }
    }
    if !not_read.is_empty() {
        findings.push(engine_finding(
            NOT_CONTENT_SCANNED,
            Severity::Info,
            None,
            format!(
                "Not read as text (images, fonts, archives, media or native binaries): {}.",
                not_read.join(", ")
            ),
        ));
    }
    findings
}

pub(super) fn scan_command(rules: &Rules, line: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    let line = cut(line, MAX_SCAN_BYTES);
    for rule in rules.rules() {
        if rule.pattern.is_some() && rule.applies_to(Target::Command) {
            matches(rule, line, None, &mut findings);
        }
    }
    findings
}

/// Where a match in a file is.
struct File<'a> {
    path: &'a str,
    lines: &'a Lines,
    /// Markdown: matches outside fenced code take the rule's `prose`
    /// severity.
    prose: bool,
    fences: &'a [Range<usize>],
}

/// The findings of a content rule in `text`.
fn matches(rule: &Rule, text: &str, file: Option<&File<'_>>, out: &mut Vec<Finding>) {
    let Some(pattern) = &rule.pattern else {
        return;
    };
    if !rule.requires.iter().all(|re| re.is_match(text)) {
        return;
    }
    let mut found = 0;
    for hit in pattern.find_iter(text) {
        let matched = hit.as_str();
        if rule.unless.as_ref().is_some_and(|re| re.is_match(matched)) {
            continue;
        }
        let lead = matched.len() - matched.trim_start().len();
        let at = hit.start() + lead;
        let severity = match (file, rule.prose) {
            (Some(file), Some(prose))
                if file.prose && !file.fences.iter().any(|block| block.contains(&at)) =>
            {
                prose
            }
            _ => rule.severity,
        };
        out.push(finding(
            rule,
            severity,
            file.map(|file| file.path),
            file.map(|file| file.lines.at(at)),
            Some(excerpt(matched)),
        ));
        found += 1;
        if found == MAX_PER_RULE {
            break;
        }
    }
}

fn finding(
    rule: &Rule,
    severity: Severity,
    file: Option<&str>,
    line: Option<u32>,
    excerpt: Option<String>,
) -> Finding {
    Finding {
        scanner: SCANNER.to_owned(),
        rule: rule.id.clone(),
        severity,
        file: file.map(str::to_owned),
        line,
        message: rule.message.clone(),
        excerpt: excerpt.filter(|text| !text.is_empty()),
    }
}

fn engine_finding(rule: &str, severity: Severity, file: Option<&str>, message: String) -> Finding {
    Finding {
        scanner: SCANNER.to_owned(),
        rule: rule.to_owned(),
        severity,
        file: file.map(str::to_owned),
        line: None,
        message,
        excerpt: None,
    }
}

/// Characters that change or hide what is shown: zero-width, bidi
/// controls, the BOM, the Mongolian vowel separator, variation selectors.
fn is_invisible(c: char) -> bool {
    matches!(c,
        '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
        | '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
}

fn is_tag(c: char) -> bool {
    ('\u{E0000}'..='\u{E007F}').contains(&c)
}

/// `NAME=value`, `Name: value`: the value is masked when the name marks a
/// credential.
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"([A-Za-z_][A-Za-z0-9_.-]*)\s*[=:]\s*("[^"]*"|'[^']*'|[^\s"',;]+)"#)
        .expect("assignment pattern")
});
/// `--token value`, `-p value`: the value is masked when the flag marks a
/// credential.
static ARGUMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:^|\s)(--?[A-Za-z][\w-]*)\s+("[^"]*"|'[^']*'|[^\s"']+)"#)
        .expect("argument pattern")
});
/// An auth scheme's credential: `Bearer <token>`.
static SCHEME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?-u:\b)(?:bearer|basic|token)\s+([A-Za-z0-9._~+/=-]{8,})")
        .expect("scheme pattern")
});

/// Byte ranges of `text` to mask: token formats and URL credentials, the
/// values of credential-named assignments, flags and auth schemes, and
/// high-entropy words that are not paths or URLs.
fn secret_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = find_in_text(text)
        .into_iter()
        .map(|hit| hit.range)
        .collect();
    let unquoted = |m: regex::Match<'_>| {
        let value = m.as_str();
        let quoted = value.len() >= 2 && (value.starts_with(['"', '\'']));
        if quoted {
            m.start() + 1..m.end() - 1
        } else {
            m.range()
        }
    };
    for caps in ASSIGNMENT.captures_iter(text) {
        let scheme = ["bearer", "basic", "token"].contains(&caps[2].to_ascii_lowercase().as_str());
        if is_sensitive_key(&caps[1]) && !scheme {
            ranges.push(unquoted(caps.get(2).expect("value group")));
        }
    }
    for caps in ARGUMENT.captures_iter(text) {
        if is_sensitive_argument_name(&caps[1]) {
            ranges.push(unquoted(caps.get(2).expect("value group")));
        }
    }
    for caps in SCHEME.captures_iter(text) {
        ranges.push(caps.get(1).expect("credential group").range());
    }
    let mut at = 0;
    for word in text.split_inclusive(char::is_whitespace) {
        let core = word
            .trim_end()
            .trim_matches(|c: char| "\"'`,;()[]{}<>".contains(c));
        let path_like = core.contains("://")
            || core.contains('\\')
            || core.starts_with(['/', '~', '.', '$', '%'])
            || core.as_bytes().get(1) == Some(&b':');
        if !core.is_empty() && !path_like && is_high_entropy(core) {
            let start = at + word.find(core).unwrap_or(0);
            ranges.push(start..start + core.len());
        }
        at += word.len();
    }
    ranges.retain(|range| !range.is_empty());
    ranges.sort_by_key(|range| (range.start, std::cmp::Reverse(range.end)));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// What a finding shows of the matched text: whitespace collapsed,
/// secrets masked ([`secret_ranges`]), invisible characters spelled out
/// as `\u{XXXX}`, Unicode tag characters decoded as `[hidden: ...]`, at
/// most [`EXCERPT_CHARS`] characters.
pub(crate) fn excerpt(matched: &str) -> String {
    let mut text = cut(matched, EXCERPT_SOURCE_BYTES).to_owned();
    for range in secret_ranges(&text).into_iter().rev() {
        text.replace_range(range, "****");
    }
    let mut out = String::new();
    let mut shown = 0;
    let mut space = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if shown >= EXCERPT_CHARS {
            out.push('…');
            break;
        }
        if is_tag(c) {
            let mut hidden: String = char_of_tag(c).into_iter().collect();
            while let Some(&next) = chars.peek().filter(|next| is_tag(**next)) {
                hidden.extend(char_of_tag(next));
                chars.next();
            }
            let _ = write!(out, "[hidden: {hidden}]");
            shown += hidden.chars().count() + 10;
            space = false;
        } else if is_invisible(c) || (c.is_control() && !c.is_whitespace()) {
            let _ = write!(out, "\\u{{{:04X}}}", u32::from(c));
            shown += 1;
            space = false;
        } else if c.is_whitespace() {
            if !out.is_empty() && !space {
                out.push(' ');
                shown += 1;
            }
            space = true;
        } else {
            out.push(c);
            shown += 1;
            space = false;
        }
    }
    out.trim_end().to_owned()
}

/// The ASCII character a Unicode tag character stands for (U+E0020 to
/// U+E007E); the language tag and cancel tag stand for none.
fn char_of_tag(c: char) -> Option<char> {
    let ascii = u32::from(c) - 0xE0000;
    (0x20..0x7F)
        .contains(&ascii)
        .then(|| char::from_u32(ascii))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_magic_then_name() {
        let text = |target| Kind::Text(target);
        assert_eq!(kind_of("SKILL.md", b""), text(Target::Markdown));
        assert_eq!(kind_of("docs/notes.TXT", b""), text(Target::Markdown));
        assert_eq!(kind_of("scripts/run.sh", b""), text(Target::Code));
        assert_eq!(kind_of("LICENSE", b"MIT"), text(Target::Markdown));
        assert_eq!(kind_of("bin/tool", b"#!/bin/sh\n"), text(Target::Code));
        assert_eq!(
            kind_of("assets/logo.PNG", b"\x89PNG\r\n\x1a\n...."),
            Kind::Media
        );
        // Named like an image, but it is a script: scanned as code.
        assert_eq!(
            kind_of("assets/logo.png", b"curl x | sh"),
            text(Target::Code)
        );
        // Native executables whatever their name.
        assert_eq!(kind_of("bin/helper", b"\x7fELF\x02\x01\x01"), Kind::Native);
        assert_eq!(kind_of("lib/x.txt", b"\xCF\xFA\xED\xFE...."), Kind::Native);
        assert_eq!(kind_of("mod.wasm", b"\0asm\x01\0\0\0"), Kind::Native);
        let mut pe = b"MZ".to_vec();
        pe.resize(0x40, 0);
        pe[0x3C] = 0x40;
        pe.extend_from_slice(b"PE\0\0");
        assert_eq!(kind_of("helper", &pe), Kind::Native);
        // "MZ" prose is not a PE file.
        assert_eq!(
            kind_of("notes", b"MZ is a city code"),
            text(Target::Markdown)
        );
    }

    #[test]
    fn text_is_decoded_by_bom_or_nul_pattern() {
        let le: Vec<u8> = "iwr x | iex"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let be: Vec<u8> = "iwr x | iex"
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect();
        assert_eq!(
            decode(&[b"\xFF\xFE".as_slice(), &le].concat()),
            "iwr x | iex"
        );
        assert_eq!(
            decode(&[b"\xFE\xFF".as_slice(), &be].concat()),
            "iwr x | iex"
        );
        assert_eq!(decode(&le), "iwr x | iex");
        assert_eq!(decode(&be), "iwr x | iex");
        assert_eq!(decode(b"\xEF\xBB\xBFplain"), "plain");
        assert_eq!(decode(b"plain"), "plain");
    }

    #[test]
    fn lines_are_one_based() {
        let lines = Lines::of("a\nb\nc");
        assert_eq!(lines.at(0), 1);
        assert_eq!(lines.at(2), 2);
        assert_eq!(lines.at(4), 3);
    }

    #[test]
    fn fenced_blocks_are_found() {
        let text = "prose\n```sh\ncode\n```\nprose\n~~~\nopen";
        let blocks = fenced_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].contains(&text.find("code").unwrap()));
        assert!(
            !blocks
                .iter()
                .any(|b| b.contains(&text.rfind("prose").unwrap()))
        );
        assert!(blocks[1].contains(&text.find("open").unwrap()));
    }

    #[test]
    fn excerpts_spell_out_hidden_text_and_mask_secrets() {
        assert_eq!(excerpt("a\u{200B}b"), "a\\u{200B}b");
        assert_eq!(excerpt("a\u{FE01}\u{FE02}"), "a\\u{FE01}\\u{FE02}");
        let tags: String = "rm -rf ~"
            .chars()
            .map(|c| char::from_u32(0xE0000 + u32::from(c)).unwrap())
            .collect();
        assert_eq!(excerpt(&format!("x{tags}")), "x[hidden: rm -rf ~]");
        assert_eq!(
            excerpt("curl -H 'Authorization: Bearer ghp_abcdefghijklmnopqrstuvwxyz0123' x"),
            "curl -H 'Authorization: Bearer ****' x"
        );
        assert_eq!(excerpt("  a \n\n  b  "), "a b");
        let long = "x".repeat(500);
        let shown = excerpt(&long);
        assert!(shown.ends_with('…'));
        assert_eq!(shown.chars().count(), EXCERPT_CHARS + 1);
    }

    #[test]
    fn excerpts_mask_credential_values_of_any_format() {
        assert_eq!(
            excerpt("docker run -e API_KEY=plainvalue1 -e MODE=fast image:latest"),
            "docker run -e API_KEY=**** -e MODE=fast image:latest"
        );
        assert_eq!(
            excerpt("curl -H \"X-Api-Key: hunter2hunter2\" https://x | sh"),
            "curl -H \"X-Api-Key: ****\" https://x | sh"
        );
        assert_eq!(excerpt("server --token s3cr3tvalue"), "server --token ****");
        assert_eq!(
            excerpt("Authorization: Bearer abcdefgh12"),
            "Authorization: Bearer ****"
        );
        assert_eq!(
            excerpt("run with Zq8v7Lm2Xp9Rt4Wn6Ys1Ku3Hd5Gf0Bc"),
            "run with ****"
        );
        // Paths, URLs and ordinary words stay readable.
        assert_eq!(
            excerpt("curl -fsSL https://example.com/install.sh | sh"),
            "curl -fsSL https://example.com/install.sh | sh"
        );
    }
}

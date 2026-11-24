//! The scanner's rules: data (`rules.toml`, built in, or a newer copy in
//! `<LOADOUT_HOME>/state/scan-rules.toml`), checked and compiled once.
//! The file format is described at the top of `rules.toml`.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::Severity;
use crate::error::{CoreError, ErrorCode};

/// The rules file format this build reads.
pub const SCHEMA: u32 = 1;

const BUILTIN_TEXT: &str = include_str!("rules.toml");

/// A unit test loads the built-in file; a build that ships broken rules
/// must not scan with none.
static BUILTIN: LazyLock<Rules> = LazyLock::new(|| match Rules::parse(BUILTIN_TEXT) {
    Ok(rules) => rules,
    Err(err) => panic!("the built-in scan rules do not load: {err:?}"),
});

/// What a content pattern runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// Prose: `.md`, `.markdown`, `.mdx`, `.txt`, `.rst`, `.adoc`, and
    /// files without an extension that do not start with `#!`.
    Markdown,
    /// Every other text file.
    Code,
    /// An MCP server's command line.
    Command,
}

/// One compiled rule.
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: String,
    pub severity: Severity,
    pub message: String,
    /// Severity of a match in Markdown prose (outside fenced code
    /// blocks): talking about a command is not running it.
    pub(super) prose: Option<Severity>,
    pub(super) targets: Vec<Target>,
    pub(super) path: Option<Regex>,
    pub(super) pattern: Option<Regex>,
    pub(super) unless: Option<Regex>,
    pub(super) requires: Vec<Regex>,
}

impl Rule {
    pub fn applies_to(&self, target: Target) -> bool {
        self.targets.contains(&target)
    }
}

/// A checked rules file, or the built-in rules with a custom file merged
/// in ([`Rules::with_custom`]).
#[derive(Debug, Clone)]
pub struct Rules {
    version: u32,
    fingerprint: String,
    rules: Vec<Rule>,
    custom: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRules {
    #[allow(dead_code)]
    schema: u32,
    version: u32,
    #[serde(default, rename = "rule")]
    rules: Vec<RawRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: String,
    severity: Severity,
    message: String,
    targets: Option<Vec<Target>>,
    path: Option<String>,
    pattern: Option<String>,
    unless: Option<String>,
    #[serde(default)]
    requires: Vec<String>,
    prose: Option<Severity>,
}

fn invalid(code: ErrorCode, message: String) -> CoreError {
    CoreError::new(code, format!("scan rules: {message}"))
}

fn is_rule_id(id: &str) -> bool {
    !id.is_empty()
        && id.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// `\b` as an ASCII word boundary (`(?-u:\b)`) outside character
/// classes: a Unicode `\b` makes `regex` fall back to its slow engine on
/// any non-ASCII text (minutes for a large hostile file).
fn ascii_word_boundaries(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut chars = pattern.chars();
    let mut class = 0usize;
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('b') if class == 0 => out.push_str("(?-u:\\b)"),
                Some(next) => {
                    out.push('\\');
                    out.push(next);
                }
                None => out.push('\\'),
            },
            '[' => {
                class += 1;
                out.push(c);
            }
            ']' if class > 0 => {
                class -= 1;
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

fn compile(id: &str, field: &str, pattern: &str) -> Result<Regex, CoreError> {
    Regex::new(&ascii_word_boundaries(pattern)).map_err(|err| {
        invalid(
            ErrorCode::ParseError,
            format!("rule {id:?}: `{field}`: {err}"),
        )
    })
}

fn fingerprint_of(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl Rules {
    /// The rules this build ships.
    pub fn builtin() -> &'static Rules {
        &BUILTIN
    }

    /// Reads and checks a rules file: `unsupported_schema` for another
    /// `schema`, `parse_error` for bad TOML or a pattern that does not
    /// compile, `invalid_value`/`missing_field`/`field_not_allowed` for a
    /// rule that breaks the format.
    pub fn parse(text: &str) -> Result<Rules, CoreError> {
        let table: toml::Table = toml::from_str(text)
            .map_err(|err| invalid(ErrorCode::ParseError, err.message().to_owned()))?;
        match table.get("schema").and_then(toml::Value::as_integer) {
            Some(schema) if schema == i64::from(SCHEMA) => {}
            Some(schema) => {
                return Err(invalid(
                    ErrorCode::UnsupportedSchema,
                    format!("schema {schema} is not supported (this build reads {SCHEMA})"),
                ));
            }
            None => return Err(invalid(ErrorCode::MissingField, "no `schema`".to_owned())),
        }
        let raw: RawRules =
            toml::Value::Table(table)
                .try_into()
                .map_err(|err: toml::de::Error| {
                    invalid(ErrorCode::ParseError, err.message().to_owned())
                })?;
        if raw.version == 0 {
            return Err(invalid(
                ErrorCode::InvalidValue,
                "`version` must be 1 or more".to_owned(),
            ));
        }
        let mut seen = BTreeSet::new();
        let mut rules = Vec::with_capacity(raw.rules.len());
        for rule in raw.rules {
            rules.push(check(rule, &mut seen)?);
        }
        Ok(Rules {
            version: raw.version,
            fingerprint: fingerprint_of(text),
            rules,
            custom: false,
        })
    }

    /// These rules with a custom rules file merged in: a rule with a new
    /// id is added; a rule with a built-in id may only raise that rule's
    /// severity (its pattern is ignored, the built-in one keeps running).
    /// A built-in rule is never removed or lowered: such an entry is
    /// skipped with a warning (the returned messages).
    pub fn with_custom(&self, text: &str) -> Result<(Rules, Vec<String>), CoreError> {
        let custom = Rules::parse(text)?;
        let mut rules = self.rules.clone();
        let mut warnings = Vec::new();
        for rule in custom.rules {
            match rules.iter_mut().find(|known| known.id == rule.id) {
                Some(known) if rule.severity >= known.severity => {
                    known.severity = rule.severity;
                }
                Some(known) => warnings.push(format!(
                    "custom scan rule {:?} would lower a built-in rule from {} to {}; the built-in severity is kept",
                    rule.id,
                    known.severity.as_str(),
                    rule.severity.as_str()
                )),
                None => rules.push(rule),
            }
        }
        Ok((
            Rules {
                version: self.version,
                fingerprint: fingerprint_of(&format!(
                    "{}\0{}",
                    self.fingerprint, custom.fingerprint
                )),
                rules,
                custom: true,
            },
            warnings,
        ))
    }

    /// The rules file's `version` (the built-in one for merged rules).
    pub fn version(&self) -> u32 {
        self.version
    }

    /// SHA-256 (hex) of the rules text: part of the cache key.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// A custom rules file is merged in.
    pub fn is_custom(&self) -> bool {
        self.custom
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }
}

fn check(raw: RawRule, seen: &mut BTreeSet<String>) -> Result<Rule, CoreError> {
    let id = raw.id;
    if !is_rule_id(&id) {
        return Err(invalid(
            ErrorCode::InvalidId,
            format!("rule id {id:?} is not kebab-case (a-z, 0-9, -)"),
        ));
    }
    if !seen.insert(id.clone()) {
        return Err(invalid(
            ErrorCode::AlreadyExists,
            format!("rule {id:?} is defined twice"),
        ));
    }
    let message = raw.message.trim().to_owned();
    if message.is_empty() {
        return Err(invalid(
            ErrorCode::MissingField,
            format!("rule {id:?} has no message"),
        ));
    }
    let not_allowed =
        |what: &str| invalid(ErrorCode::FieldNotAllowed, format!("rule {id:?}: {what}"));
    let targets = match (&raw.pattern, raw.targets) {
        (None, None) if raw.path.is_none() => {
            return Err(invalid(
                ErrorCode::MissingField,
                format!("rule {id:?} has neither `pattern` nor `path`"),
            ));
        }
        (None, None) => Vec::new(),
        (None, Some(_)) => {
            return Err(not_allowed(
                "`targets` needs a `pattern` (a path rule matches files)",
            ));
        }
        (Some(_), None) => vec![Target::Markdown, Target::Code],
        (Some(_), Some(targets)) if targets.is_empty() => {
            return Err(invalid(
                ErrorCode::InvalidValue,
                format!("rule {id:?} has empty `targets`"),
            ));
        }
        (Some(_), Some(targets)) => targets,
    };
    if raw.pattern.is_none() && (raw.unless.is_some() || !raw.requires.is_empty()) {
        return Err(not_allowed("`unless` and `requires` need a `pattern`"));
    }
    if raw.path.is_some() && targets.contains(&Target::Command) {
        return Err(not_allowed(
            "a command line has no path; `path` and the `command` target exclude each other",
        ));
    }
    if let Some(prose) = raw.prose
        && (!targets.contains(&Target::Markdown) || prose >= raw.severity)
    {
        return Err(not_allowed(&format!(
            "`prose` ({}) needs the markdown target and a severity below `severity`",
            prose.as_str()
        )));
    }
    Ok(Rule {
        path: raw
            .path
            .as_deref()
            .map(|p| compile(&id, "path", p))
            .transpose()?,
        pattern: raw
            .pattern
            .as_deref()
            .map(|p| compile(&id, "pattern", p))
            .transpose()?,
        unless: raw
            .unless
            .as_deref()
            .map(|p| compile(&id, "unless", p))
            .transpose()?,
        requires: raw
            .requires
            .iter()
            .map(|p| compile(&id, "requires", p))
            .collect::<Result<_, _>>()?,
        id,
        severity: raw.severity,
        message,
        prose: raw.prose,
        targets,
    })
}

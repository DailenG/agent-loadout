//! Format-preserving editors for native client files (SPEC 7.2).
//!
//! A [`Document`] edits one entry at a time inside a container (for example
//! `mcpServers` in JSON or `mcp_servers` in TOML) and leaves every other byte
//! alone: comments, key order, indentation, trailing-comma style, a UTF-8 BOM,
//! CRLF line endings and the final newline survive, and an operation with no
//! logical change returns the input bytes unchanged.
//!
//! Paths are slices of keys from the document root, so keys holding `/` or
//! `.` (such as project paths in `~/.claude.json`) need no escaping.
//!
//! - JSON and JSONC: `jsonc-parser`'s CST. Strict JSON parses without
//!   comments or trailing commas and never gains them; JSONC accepts both
//!   (and nothing from JSON5).
//! - TOML: `toml_edit`. An existing entry is updated key by key inside its
//!   table, so comments in it survive; tables outside the entry are never
//!   touched.
//! - JSON5: a file that also parses as JSONC is edited as JSONC; any other
//!   JSON5 file is read with the `json5` crate and never written (every
//!   edit that would change it fails with `read_only_format`).
//! - YAML: span splice over saphyr's parser (see `yaml.rs`). Entries that
//!   cannot be located unambiguously (anchors, aliases, flow-style
//!   containers, several documents) make edits fail with
//!   `read_only_format`. A last container segment `[field]` addresses a
//!   sequence of mappings keyed by their `field` member (YAML only; in the
//!   other formats it is an ordinary key).

mod json5;
mod jsonc;
mod text;
mod toml;
mod yaml;

use crate::error::{CoreError, ErrorCode};

pub use text::{LineEnding, TextStyle};

/// The syntax of a native client file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Format {
    /// Strict JSON (RFC 8259): no comments, no trailing commas.
    Json,
    /// JSON with comments and trailing commas.
    Jsonc,
    Toml,
    /// JSON5: read always, written only when the file is also JSONC.
    Json5,
    Yaml,
}

impl Format {
    pub const fn as_str(self) -> &'static str {
        match self {
            Format::Json => "json",
            Format::Jsonc => "jsonc",
            Format::Toml => "toml",
            Format::Json5 => "json5",
            Format::Yaml => "yaml",
        }
    }
}

/// A format-neutral value read from or written to a native file.
///
/// Objects keep their key order (see [`NativeMap`]); equality ignores it,
/// because neither JSON nor TOML gives key order a meaning.
#[derive(Debug, Clone, PartialEq)]
pub enum NativeValue {
    /// JSON `null`. TOML has none, so writing it to TOML is `invalid_value`.
    Null,
    Bool(bool),
    Integer(i64),
    /// A finite float. JSON numbers with a fraction or exponent, or outside
    /// the `i64` range, read as floats.
    Float(f64),
    String(String),
    /// A TOML date-time, in its TOML text form. JSON writes it as a string.
    Datetime(String),
    Array(Vec<NativeValue>),
    Object(NativeMap),
}

impl NativeValue {
    /// Parses JSON or JSONC text (one value). Duplicate keys: the first
    /// wins, as in every [`Document`] operation.
    pub fn parse_json(text: &str) -> Result<Self, CoreError> {
        jsonc::parse_value(text)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            NativeValue::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&NativeMap> {
        match self {
            NativeValue::Object(map) => Some(map),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[NativeValue]> {
        match self {
            NativeValue::Array(items) => Some(items),
            _ => None,
        }
    }
}

impl From<&str> for NativeValue {
    fn from(value: &str) -> Self {
        NativeValue::String(value.to_owned())
    }
}

impl From<String> for NativeValue {
    fn from(value: String) -> Self {
        NativeValue::String(value)
    }
}

impl From<bool> for NativeValue {
    fn from(value: bool) -> Self {
        NativeValue::Bool(value)
    }
}

impl From<i64> for NativeValue {
    fn from(value: i64) -> Self {
        NativeValue::Integer(value)
    }
}

impl From<NativeMap> for NativeValue {
    fn from(value: NativeMap) -> Self {
        NativeValue::Object(value)
    }
}

/// Object members in document (or insertion) order, keys unique.
#[derive(Debug, Clone, Default)]
pub struct NativeMap {
    entries: Vec<(String, NativeValue)>,
}

impl NativeMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&NativeValue> {
        self.entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut NativeValue> {
        self.entries
            .iter_mut()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Replaces the value of `key` in place, or appends it.
    pub fn insert(&mut self, key: impl Into<String>, value: NativeValue) -> Option<NativeValue> {
        let key = key.into();
        match self.entries.iter_mut().find(|(name, _)| *name == key) {
            Some((_, slot)) => Some(std::mem::replace(slot, value)),
            None => {
                self.entries.push((key, value));
                None
            }
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<NativeValue> {
        let index = self.entries.iter().position(|(name, _)| name == key)?;
        Some(self.entries.remove(index).1)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(name, _)| name.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &NativeValue)> {
        self.entries
            .iter()
            .map(|(name, value)| (name.as_str(), value))
    }
}

impl PartialEq for NativeMap {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .all(|(key, value)| other.get(key) == Some(value))
    }
}

impl<K: Into<String>> FromIterator<(K, NativeValue)> for NativeMap {
    fn from_iter<I: IntoIterator<Item = (K, NativeValue)>>(iter: I) -> Self {
        let mut map = NativeMap::new();
        for (key, value) in iter {
            map.insert(key, value);
        }
        map
    }
}

impl IntoIterator for NativeMap {
    type Item = (String, NativeValue);
    type IntoIter = std::vec::IntoIter<(String, NativeValue)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

/// A parsed native file that can be edited without disturbing anything
/// outside the edited entry.
///
/// Containers created by [`Document::upsert`] or [`Document::set_list_member`]
/// are removed again when a later [`Document::remove`] or
/// `set_list_member(.., false)` leaves them empty, so an edit that is undone
/// within one document returns the original bytes. Containers that were in
/// the file are never removed.
#[derive(Debug, Clone)]
pub struct Document {
    format: Format,
    style: TextStyle,
    body: Body,
    /// Paths of containers this document created, in creation order.
    created: Vec<Vec<String>>,
}

#[derive(Debug, Clone)]
enum Body {
    Json(jsonc::JsonBody),
    Toml(toml::TomlBody),
    Json5(json5::Json5Body),
    Yaml(yaml::YamlBody),
}

impl Document {
    /// Parses a native file. Empty (or whitespace-only) input is an empty
    /// document, which is how a missing file is edited. Fails with
    /// `parse_error` when the bytes are not UTF-8 or not valid `format`.
    pub fn parse(bytes: &[u8], format: Format) -> Result<Self, CoreError> {
        let (text, style) = text::decode(bytes)?;
        let body = match format {
            Format::Json => Body::Json(jsonc::JsonBody::parse(text, true)?),
            Format::Jsonc => Body::Json(jsonc::JsonBody::parse(text, false)?),
            Format::Toml => Body::Toml(toml::TomlBody::parse(text)?),
            Format::Json5 => match jsonc::JsonBody::parse(text.clone(), false) {
                Ok(body) => Body::Json(body),
                Err(_) => Body::Json5(json5::Json5Body::parse(text)?),
            },
            Format::Yaml => Body::Yaml(yaml::YamlBody::parse(text)?),
        };
        Ok(Self {
            format,
            style,
            body,
            created: Vec::new(),
        })
    }

    /// An empty document: the skeleton for a file that does not exist yet.
    pub fn empty(format: Format) -> Self {
        Self::parse(b"", format).expect("an empty document parses")
    }

    pub fn format(&self) -> Format {
        self.format
    }

    /// BOM, line endings and final newline detected at parse time and
    /// restored by [`Document::to_bytes`].
    pub fn style(&self) -> TextStyle {
        self.style
    }

    /// Entry keys of the container at `container`, in document order. A
    /// missing container has no entries.
    pub fn list(&self, container: &[&str]) -> Result<Vec<String>, CoreError> {
        match &self.body {
            Body::Json(body) => body.list(container),
            Body::Toml(body) => body.list(container),
            Body::Json5(body) => body.list(container),
            Body::Yaml(body) => body.list(container),
        }
    }

    /// The value of `key` in the container at `container`.
    pub fn get(&self, container: &[&str], key: &str) -> Result<Option<NativeValue>, CoreError> {
        match &self.body {
            Body::Json(body) => body.get(container, key),
            Body::Toml(body) => body.get(container, key),
            Body::Json5(body) => body.get(container, key),
            Body::Yaml(body) => body.get(container, key),
        }
    }

    /// Makes `key` in the container at `container` equal to `value`,
    /// creating missing containers. A new key is appended at the end of the
    /// container. An existing key is updated in place: object members that
    /// keep their value keep their text and comments, members missing from
    /// `value` are removed and new members are appended. Members the caller
    /// does not model survive only if `value` carries them (read the entry
    /// with [`Document::get`] first). Returns whether the document changed;
    /// on error it is unchanged (`unexpected_shape` for a container that is
    /// not an object or table, `invalid_value` for a value the format cannot
    /// hold).
    pub fn upsert(
        &mut self,
        container: &[&str],
        key: &str,
        value: &NativeValue,
    ) -> Result<bool, CoreError> {
        let mut created = Vec::new();
        let changed = match &mut self.body {
            Body::Json(body) => body.upsert(container, key, value, &mut created)?,
            Body::Toml(body) => body.upsert(container, key, value, &mut created)?,
            Body::Json5(body) => {
                if body.get(container, key)?.as_ref() == Some(value) {
                    false
                } else {
                    return Err(body.read_only(container));
                }
            }
            Body::Yaml(body) => body.upsert(container, key, value, &mut created)?,
        };
        self.created.extend(created);
        Ok(changed)
    }

    /// Removes `key` from the container at `container`, with the comment
    /// lines directly above it (no blank line between; in JSONC only when
    /// other members remain); comments above a blank line stay. Returns
    /// whether it was there.
    pub fn remove(&mut self, container: &[&str], key: &str) -> Result<bool, CoreError> {
        let removed = match &mut self.body {
            Body::Json(body) => body.remove(container, key)?,
            Body::Toml(body) => body.remove(container, key)?,
            Body::Json5(body) => {
                if body.get(container, key)?.is_some() {
                    return Err(body.read_only(container));
                }
                false
            }
            Body::Yaml(body) => body.remove(container, key)?,
        };
        if removed {
            self.prune(container)?;
        }
        Ok(removed)
    }

    /// Adds the string `member` to the array at `list` (creating it) when
    /// `present`, or removes every occurrence of it otherwise. A new member
    /// is appended. Returns whether the document changed.
    pub fn set_list_member(
        &mut self,
        list: &[&str],
        member: &str,
        present: bool,
    ) -> Result<bool, CoreError> {
        if list.is_empty() {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                "a list path needs at least one key",
            ));
        }
        let mut created = Vec::new();
        let changed = match &mut self.body {
            Body::Json(body) => body.set_list_member(list, member, present, &mut created)?,
            Body::Toml(body) => body.set_list_member(list, member, present, &mut created)?,
            Body::Json5(body) => {
                let (name, parent) = list.split_last().expect("checked above");
                let listed = body.get(parent, name)?.is_some_and(|value| match value {
                    NativeValue::Array(items) => {
                        items.iter().any(|item| item.as_str() == Some(member))
                    }
                    _ => false,
                });
                if listed != present {
                    return Err(body.read_only(list));
                }
                false
            }
            Body::Yaml(body) => body.set_list_member(list, member, present, &mut created)?,
        };
        self.created.extend(created);
        if changed && !present {
            self.prune(list)?;
        }
        Ok(changed)
    }

    /// The document text with the original BOM, line endings and final
    /// newline.
    pub fn to_bytes(&self) -> Vec<u8> {
        let body = match &self.body {
            Body::Json(body) => body.text(),
            Body::Toml(body) => body.text(),
            Body::Json5(body) => body.text(),
            Body::Yaml(body) => body.text(),
        };
        text::encode(&body, self.style)
    }

    /// Removes containers this document created that `path` and its
    /// ancestors left empty, innermost first.
    fn prune(&mut self, path: &[&str]) -> Result<(), CoreError> {
        for len in (0..=path.len()).rev() {
            let prefix = &path[..len];
            let Some(index) = self.created.iter().position(|created| *created == prefix) else {
                return Ok(());
            };
            let removed = match &mut self.body {
                Body::Json(body) => body.remove_if_empty(prefix)?,
                Body::Toml(body) => body.remove_if_empty(prefix)?,
                Body::Json5(_) => false,
                Body::Yaml(body) => body.remove_if_empty(prefix)?,
            };
            if !removed {
                return Ok(());
            }
            self.created.remove(index);
        }
        Ok(())
    }
}

/// The mapping keys of a container path: `path` without a last `[field]`
/// segment (a YAML list keyed by `field`, see the module docs), so the
/// container is the value at the returned path.
pub fn mapping_path<'p, 'a>(path: &'p [&'a str]) -> &'p [&'a str] {
    match path.split_last() {
        Some((last, rest)) if last.len() > 2 && last.starts_with('[') && last.ends_with(']') => {
            rest
        }
        _ => path,
    }
}

/// `unexpected_shape` for the value at `path`.
fn unexpected_shape(format: Format, path: &[&str], expected: &str) -> CoreError {
    CoreError::new(
        ErrorCode::UnexpectedShape,
        format!(
            "{} is not {expected}",
            display_path(format, path.iter().copied())
        ),
    )
}

/// `path` as a JSON pointer (`/a/b`) or a TOML dotted key (`a.b`), for
/// messages.
fn display_path<'a>(format: Format, path: impl Iterator<Item = &'a str>) -> String {
    let path: Vec<&str> = path.collect();
    match format {
        Format::Toml if path.is_empty() => "the document root".to_owned(),
        Format::Toml => path
            .iter()
            .map(|key| ::toml_edit::Key::new(*key).display_repr().into_owned())
            .collect::<Vec<_>>()
            .join("."),
        _ if path.is_empty() => "the document root".to_owned(),
        _ => path
            .iter()
            .map(|key| format!("/{}", key.replace('~', "~0").replace('/', "~1")))
            .collect(),
    }
}

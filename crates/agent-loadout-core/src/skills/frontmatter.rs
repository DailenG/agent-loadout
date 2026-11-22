//! `SKILL.md` frontmatter (SPEC 12.1): the YAML block between a leading
//! `---` line and the next `---` (or `...`) line, read with the same YAML
//! reader as client files. Every field is kept, in file order, with its
//! value (SkillDock read only single-line `name` and `description`).

use serde_json::Value;

use crate::error::{CoreError, ErrorCode};
use crate::formats::{Document, Format, NativeValue};

/// The parsed frontmatter of a `SKILL.md`.
#[derive(Debug, Clone, PartialEq)]
pub struct Frontmatter {
    /// Top-level fields in file order.
    pub fields: Vec<(String, NativeValue)>,
}

impl Frontmatter {
    pub fn get(&self, key: &str) -> Option<&NativeValue> {
        self.fields
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// `name`, when it is a non-empty string.
    pub fn name(&self) -> Option<&str> {
        self.text("name")
    }

    /// `description`, when it is a non-empty string.
    pub fn description(&self) -> Option<&str> {
        self.text("description")
    }

    fn text(&self, key: &str) -> Option<&str> {
        self.get(key)
            .and_then(NativeValue::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
    }

    /// Every field as a JSON object, in file order.
    pub fn to_json(&self) -> Value {
        Value::Object(
            self.fields
                .iter()
                .map(|(name, value)| (name.clone(), to_json(value)))
                .collect(),
        )
    }
}

/// The YAML text of the frontmatter block and the Markdown body after it;
/// `None` when the text does not start with a `---` line (after an
/// optional BOM) or the block is not closed.
pub fn split(text: &str) -> Option<(&str, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let first_end = text.find('\n')?;
    if text[..first_end].trim_end() != "---" {
        return None;
    }
    let yaml_start = first_end + 1;
    let mut offset = yaml_start;
    for line in text[yaml_start..].split_inclusive('\n') {
        let trimmed = line.trim_end();
        if trimmed == "---" || trimmed == "..." {
            return Some((&text[yaml_start..offset], &text[offset + line.len()..]));
        }
        offset += line.len();
    }
    None
}

/// Parses the frontmatter of a `SKILL.md`. `Ok(None)`: the file has none.
/// YAML that does not parse, or a block that is not a mapping, is
/// `parse_error`.
pub fn parse(text: &str) -> Result<Option<Frontmatter>, CoreError> {
    let Some((yaml, _)) = split(text) else {
        return Ok(None);
    };
    let document = Document::parse(yaml.as_bytes(), Format::Yaml).map_err(|err| {
        CoreError::new(
            ErrorCode::ParseError,
            format!(
                "the SKILL.md frontmatter is not valid YAML: {}",
                err.message()
            ),
        )
    })?;
    let keys = document.list(&[]).map_err(|_| {
        CoreError::new(
            ErrorCode::ParseError,
            "the SKILL.md frontmatter is not a YAML mapping",
        )
    })?;
    let mut fields = Vec::with_capacity(keys.len());
    for key in keys {
        let value = document.get(&[], &key)?.unwrap_or(NativeValue::Null);
        fields.push((key, value));
    }
    Ok(Some(Frontmatter { fields }))
}

/// A native value as JSON (datetimes as their text, non-finite floats as
/// `null`).
pub fn to_json(value: &NativeValue) -> Value {
    match value {
        NativeValue::Null => Value::Null,
        NativeValue::Bool(value) => Value::Bool(*value),
        NativeValue::Integer(value) => Value::from(*value),
        NativeValue::Float(value) => {
            serde_json::Number::from_f64(*value).map_or(Value::Null, Value::Number)
        }
        NativeValue::String(value) | NativeValue::Datetime(value) => Value::String(value.clone()),
        NativeValue::Array(items) => Value::Array(items.iter().map(to_json).collect()),
        NativeValue::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.to_owned(), to_json(value)))
                .collect(),
        ),
    }
}

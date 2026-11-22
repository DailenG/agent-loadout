//! JSON5 reading (SPEC 7.2).
//!
//! A JSON5 file that also parses as JSONC is edited through the JSONC CST
//! (see [`super::Document::parse`]); only the rest lands here. Such a body
//! is read with the `json5` crate and never written: every edit fails with
//! `read_only_format`, because rewriting it would drop comments, unquoted
//! keys, single quotes and the rest of what JSON5 adds (SkillDock wrote
//! strict JSON over these files).

use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

use super::{Format, NativeMap, NativeValue, display_path, unexpected_shape};
use crate::error::{CoreError, ErrorCode};

#[derive(Debug, Clone)]
pub(super) struct Json5Body {
    text: String,
    root: NativeValue,
}

impl Json5Body {
    pub(super) fn parse(text: String) -> Result<Self, CoreError> {
        let root = json5::from_str::<Json5Value>(&text)
            .map_err(|err| CoreError::new(ErrorCode::ParseError, format!("invalid JSON5: {err}")))?
            .0;
        Ok(Self { text, root })
    }

    pub(super) fn text(&self) -> String {
        self.text.clone()
    }

    pub(super) fn list(&self, container: &[&str]) -> Result<Vec<String>, CoreError> {
        Ok(self
            .object_at(container)?
            .map(|object| object.keys().map(str::to_owned).collect())
            .unwrap_or_default())
    }

    pub(super) fn get(
        &self,
        container: &[&str],
        key: &str,
    ) -> Result<Option<NativeValue>, CoreError> {
        Ok(self
            .object_at(container)?
            .and_then(|object| object.get(key))
            .cloned())
    }

    /// The `read_only_format` error of an edit at `path`.
    pub(super) fn read_only(&self, path: &[&str]) -> CoreError {
        CoreError::new(
            ErrorCode::ReadOnlyFormat,
            format!(
                "this file is JSON5 but not JSONC, so Loadout only reads it; {} is left as it is",
                display_path(Format::Json5, path.iter().copied())
            ),
        )
    }

    fn object_at(&self, path: &[&str]) -> Result<Option<&NativeMap>, CoreError> {
        let mut object = self
            .root
            .as_object()
            .ok_or_else(|| unexpected_shape(Format::Json5, &[], "an object"))?;
        for (depth, key) in path.iter().enumerate() {
            let Some(value) = object.get(key) else {
                return Ok(None);
            };
            object = value
                .as_object()
                .ok_or_else(|| unexpected_shape(Format::Json5, &path[..=depth], "an object"))?;
        }
        Ok(Some(object))
    }
}

/// A [`NativeValue`] deserialized in document order. Duplicate keys: the
/// first one counts, as in the JSON editor. JSON5's `Infinity` and `NaN`
/// have no [`NativeValue`] form and read as the strings `inf`, `-inf` and
/// `NaN`.
struct Json5Value(NativeValue);

impl<'de> Deserialize<'de> for Json5Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueVisitor).map(Json5Value)
    }
}

struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = NativeValue;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON5 value")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<NativeValue, E> {
        Ok(NativeValue::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<NativeValue, E> {
        Ok(NativeValue::Integer(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<NativeValue, E> {
        Ok(i64::try_from(value)
            .map(NativeValue::Integer)
            .unwrap_or(NativeValue::Float(value as f64)))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<NativeValue, E> {
        Ok(if value.is_finite() {
            NativeValue::Float(value)
        } else {
            NativeValue::String(value.to_string())
        })
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<NativeValue, E> {
        Ok(NativeValue::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<NativeValue, E> {
        Ok(NativeValue::String(value))
    }

    fn visit_unit<E: de::Error>(self) -> Result<NativeValue, E> {
        Ok(NativeValue::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<NativeValue, E> {
        Ok(NativeValue::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<NativeValue, D::Error> {
        deserializer.deserialize_any(self)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<NativeValue, A::Error> {
        let mut items = Vec::new();
        while let Some(Json5Value(item)) = seq.next_element()? {
            items.push(item);
        }
        Ok(NativeValue::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<NativeValue, A::Error> {
        let mut map = NativeMap::new();
        while let Some((key, Json5Value(value))) = access.next_entry::<String, Json5Value>()? {
            if !map.contains_key(&key) {
                map.insert(key, value);
            }
        }
        Ok(NativeValue::Object(map))
    }
}

//! Store file schemas (SPEC 5).
//!
//! Every store file is TOML with `schema = 1` at the top. A
//! [`TomlDocument`] pairs the typed value with the `toml_edit` document it
//! was read from. Saving merges the typed value back into that document, so
//! comments, key order, formatting and keys this build does not know survive;
//! loading and saving an unchanged file returns the same bytes (including a
//! UTF-8 BOM and CRLF line endings).
//!
//! Every table type keeps keys it does not model in a flattened `extra` map,
//! which is how unknown keys survive even a rewrite from the typed value.

pub mod applied;
pub mod catalogs;
pub mod clients;
pub mod ids;
pub mod machine;
pub mod project;
pub mod secrets;
pub mod server;
pub mod settings;
pub mod skill_meta;
pub mod sources;
pub mod validate;

use std::ops::Range;
use std::path::Path;

use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use toml_edit::{ArrayOfTables, DocumentMut, Item, TableLike, Value};

use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
pub use validate::{
    Diagnostic, Diagnostics, Severity, StoreDocument, StoreFile, validate_store_file,
};

/// The only store schema version this build reads and writes.
pub const SCHEMA_VERSION: i64 = 1;

const BOM: char = '\u{feff}';

/// A store file type.
pub trait StoreModel: Serialize + DeserializeOwned + JsonSchema {
    /// File stem of the generated JSON Schema: `docs/schema/<NAME>.schema.json`.
    const SCHEMA_NAME: &'static str;
}

/// JSON Schema of `T`, pretty-printed with a final newline. Optional fields
/// are simply not required: TOML has no `null`.
pub fn json_schema<T: StoreModel>() -> String {
    let generator = schemars::generate::SchemaSettings::draft2020_12()
        .with_transform(drop_null)
        .into_generator();
    let schema = generator.into_root_schema_for::<T>();
    let mut text = serde_json::to_string_pretty(&schema).expect("JSON Schema serializes");
    text.push('\n');
    text
}

/// Removes the `null` alternative schemars adds for `Option<T>`.
fn drop_null(schema: &mut schemars::Schema) {
    use serde_json::Value as Json;
    if let Some(object) = schema.as_object_mut() {
        if let Some(Json::Array(types)) = object.get_mut("type") {
            types.retain(|ty| ty != "null");
            if types.len() == 1 {
                let only = types.remove(0);
                object.insert("type".to_owned(), only);
            }
        }
        if let Some(Json::Array(options)) = object.get_mut("anyOf") {
            options.retain(|option| option.get("type").is_none_or(|ty| ty != "null"));
            if let [Json::Object(_)] = options.as_slice() {
                let Some(Json::Object(only)) = options.pop() else {
                    unreachable!("checked above");
                };
                object.remove("anyOf");
                for (key, value) in only {
                    object.entry(key).or_insert(value);
                }
            }
        }
    }
    schemars::transform::transform_subschemas(&mut drop_null, schema);
}

/// A typed store file plus the document it came from.
#[derive(Debug, Clone)]
pub struct TomlDocument<T> {
    doc: DocumentMut,
    value: T,
    bom: bool,
    crlf: bool,
}

impl<T: StoreModel> TomlDocument<T> {
    /// A new file holding `value`, with default formatting.
    pub fn new(value: T) -> Self {
        Self {
            doc: DocumentMut::new(),
            value,
            bom: false,
            crlf: false,
        }
    }

    /// Parses `text`. Fails with one diagnostic: `parse_error` (not TOML),
    /// `missing_field` or `unsupported_schema` (the `schema` key), or
    /// `missing_field`/`invalid_value` (the text does not fit `T`).
    pub fn parse(text: &str) -> Result<Self, Diagnostic> {
        let bom = text.starts_with(BOM);
        let text = text.strip_prefix(BOM).unwrap_or(text);
        let crlf = text.find('\n').is_some_and(|i| text[..i].ends_with('\r'));

        let spanned = toml_edit::Document::parse(text).map_err(|err| {
            Diagnostic::error(
                ErrorCode::ParseError,
                "",
                located(text, err.span(), err.message()),
            )
        })?;
        check_schema_version(spanned.as_table())?;
        let value =
            toml::from_str::<T>(text).map_err(|err| serde_diagnostic(text, &spanned, &err))?;
        Ok(Self {
            doc: spanned.into_mut(),
            value,
            bom,
            crlf,
        })
    }

    /// Reads and parses `path`.
    pub fn load(fs: &dyn Fs, path: &Path) -> Result<Self, CoreError> {
        let text = fs.read_to_string(path).map_err(|err| {
            CoreError::new(ErrorCode::IoError, format!("cannot read file: {err}")).with_path(path)
        })?;
        Self::parse(&text).map_err(|diagnostic| diagnostic.to_core_error(Some(path)))
    }

    /// Writes [`Self::to_text`] to `path`.
    pub fn save(&self, fs: &dyn Fs, path: &Path) -> Result<(), CoreError> {
        let text = self.to_text().map_err(|err| err.with_path(path))?;
        fs.write(path, text.as_bytes()).map_err(|err| {
            CoreError::new(ErrorCode::IoError, format!("cannot write file: {err}")).with_path(path)
        })
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn value_mut(&mut self) -> &mut T {
        &mut self.value
    }

    pub fn into_value(self) -> T {
        self.value
    }

    /// The typed value merged into the original document: unchanged keys keep
    /// their text, changed values keep their surrounding comments, keys the
    /// value no longer holds are removed and new keys are appended.
    pub fn to_text(&self) -> Result<String, CoreError> {
        let rendered = toml::to_string(&self.value).map_err(|err| {
            CoreError::new(ErrorCode::InvalidValue, format!("cannot serialize: {err}"))
        })?;
        let fresh: DocumentMut = rendered.parse().map_err(|err: toml_edit::TomlError| {
            CoreError::new(
                ErrorCode::ParseError,
                format!("serialized TOML does not parse: {}", err.message()),
            )
        })?;
        let mut doc = self.doc.clone();
        merge_table(doc.as_table_mut(), fresh.as_table());
        let mut text = doc.to_string();
        if self.crlf {
            text = text.replace('\n', "\r\n");
        }
        if self.bom {
            text.insert(0, BOM);
        }
        Ok(text)
    }
}

/// `schema` is checked before the typed parse, so a file from a newer
/// schema reports `unsupported_schema` rather than a confusing type error.
fn check_schema_version(root: &toml_edit::Table) -> Result<(), Diagnostic> {
    match root.get("schema") {
        None => Err(Diagnostic::error(
            ErrorCode::MissingField,
            "schema",
            format!("missing `schema = {SCHEMA_VERSION}`"),
        )),
        Some(item) => match item.as_integer() {
            Some(SCHEMA_VERSION) => Ok(()),
            Some(other) => Err(Diagnostic::error(
                ErrorCode::UnsupportedSchema,
                "schema",
                format!(
                    "schema {other} is not supported; this build reads schema {SCHEMA_VERSION}"
                ),
            )),
            None => Err(Diagnostic::error(
                ErrorCode::InvalidValue,
                "schema",
                "`schema` must be an integer",
            )),
        },
    }
}

fn serde_diagnostic(
    text: &str,
    spanned: &toml_edit::Document<&str>,
    err: &toml::de::Error,
) -> Diagnostic {
    let root = spanned.as_table();
    let message = err.message();
    let mut span = err.span().filter(|span| !span.is_empty());
    let mut path = span
        .as_ref()
        .and_then(|span| path_at(root, span.start, ""))
        .unwrap_or_default();
    // serde's stable wording for a required field: "missing field `name`".
    if let Some(field) = message
        .strip_prefix("missing field `")
        .and_then(|rest| rest.strip_suffix('`'))
    {
        path = validate::join(&path, field);
        return Diagnostic::error(ErrorCode::MissingField, path, message);
    }
    // Fields read through `#[serde(flatten)]` lose their span; the offending
    // value can still be found when it occurs once in the file.
    if path.is_empty()
        && let Some(needle) = offending_value(message)
    {
        let mut hits = Vec::new();
        find_scalars(root, &needle, "", &mut hits);
        if let [(found, found_span)] = hits.as_slice() {
            path = found.clone();
            span = found_span.clone();
        }
    }
    let span = span.filter(|_| !path.is_empty());
    Diagnostic::error(ErrorCode::InvalidValue, path, located(text, span, message))
}

/// The value quoted in serde's "unknown variant `x`" and
/// "invalid type: integer `8080`" / "invalid type: string \"x\"" messages.
fn offending_value(message: &str) -> Option<String> {
    let quoted = |rest: &str, open: char, close: char| {
        let rest = &rest[rest.find(open)? + open.len_utf8()..];
        rest.find(close).map(|end| rest[..end].to_owned())
    };
    if let Some(rest) = message.strip_prefix("unknown variant ") {
        return quoted(rest, '`', '`');
    }
    let rest = message.strip_prefix("invalid type: ")?;
    let rest = &rest[..rest.find(", expected").unwrap_or(rest.len())];
    match rest.strip_prefix("string ") {
        Some(string) => quoted(string, '"', '"'),
        None => quoted(rest, '`', '`'),
    }
}

/// Paths and spans of every scalar whose text is `needle`.
fn find_scalars(
    table: &dyn TableLike,
    needle: &str,
    prefix: &str,
    hits: &mut Vec<(String, Option<Range<usize>>)>,
) {
    fn visit_value(
        value: &Value,
        needle: &str,
        path: String,
        hits: &mut Vec<(String, Option<Range<usize>>)>,
    ) {
        let text = match value {
            Value::String(v) => v.value().clone(),
            Value::Integer(v) => v.value().to_string(),
            Value::Float(v) => v.value().to_string(),
            Value::Boolean(v) => v.value().to_string(),
            Value::Datetime(v) => v.value().to_string(),
            Value::Array(array) => {
                for (index, value) in array.iter().enumerate() {
                    visit_value(value, needle, validate::at(&path, index), hits);
                }
                return;
            }
            Value::InlineTable(table) => return find_scalars(table, needle, &path, hits),
        };
        if text == needle {
            hits.push((path, value.span()));
        }
    }
    for (key, item) in table.iter() {
        let path = validate::join(prefix, key);
        match item {
            Item::Value(value) => visit_value(value, needle, path, hits),
            Item::Table(table) => find_scalars(table, needle, &path, hits),
            Item::ArrayOfTables(array) => {
                for (index, table) in array.iter().enumerate() {
                    find_scalars(table, needle, &validate::at(&path, index), hits);
                }
            }
            Item::None => {}
        }
    }
}

/// The dotted key path of the innermost item whose span holds `offset`.
fn path_at(table: &dyn TableLike, offset: usize, prefix: &str) -> Option<String> {
    for (key, item) in table.iter() {
        let path = validate::join(prefix, key);
        if let Some(inner) = item_path_at(item, offset, &path) {
            return Some(inner);
        }
        let key_span = table.key(key).and_then(|key| key.span());
        if contains(item.span(), offset) || contains(key_span, offset) {
            return Some(path);
        }
    }
    None
}

fn item_path_at(item: &Item, offset: usize, path: &str) -> Option<String> {
    match item {
        Item::Table(table) => path_at(table, offset, path),
        Item::ArrayOfTables(array) => array.iter().enumerate().find_map(|(index, table)| {
            path_at(table, offset, &validate::at(path, index))
                .or_else(|| contains(table.span(), offset).then(|| validate::at(path, index)))
        }),
        Item::Value(Value::InlineTable(table)) => path_at(table, offset, path),
        Item::Value(Value::Array(array)) => array.iter().enumerate().find_map(|(index, value)| {
            contains(value.span(), offset).then(|| validate::at(path, index))
        }),
        Item::Value(_) | Item::None => None,
    }
}

fn contains(span: Option<Range<usize>>, offset: usize) -> bool {
    span.is_some_and(|span| span.contains(&offset))
}

/// `message`, prefixed with the 1-based line and column of `span`.
fn located(text: &str, span: Option<Range<usize>>, message: &str) -> String {
    let Some(span) = span else {
        return message.to_owned();
    };
    let before = &text[..span.start.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
    format!("line {line}, column {column}: {message}")
}

// Merging a freshly serialized document into the original one.

fn merge_table(old: &mut dyn TableLike, new: &dyn TableLike) {
    let stale: Vec<String> = old
        .iter()
        .filter(|(key, _)| !new.contains_key(key))
        .map(|(key, _)| key.to_owned())
        .collect();
    for key in stale {
        old.remove(&key);
    }
    for (key, new_item) in new.iter() {
        match old.get_mut(key) {
            Some(old_item) => merge_item(old_item, new_item),
            None => {
                old.insert(key, detached(new_item));
            }
        }
    }
}

fn merge_item(old: &mut Item, new: &Item) {
    if plain(old) == plain(new) {
        return;
    }
    if let (Some(old), Some(new)) = (old.as_table_like_mut(), new.as_table_like()) {
        return merge_table(old, new);
    }
    if let (Item::ArrayOfTables(old), Item::ArrayOfTables(new)) = (&mut *old, new) {
        return merge_array_of_tables(old, new);
    }
    let replacement = detached(new);
    if let Item::Value(old_value) = old {
        // Keep the value's inline style and its surrounding whitespace and
        // trailing comment.
        if let Ok(mut value) = replacement.clone().into_value() {
            *value.decor_mut() = old_value.decor().clone();
            *old_value = value;
            return;
        }
    }
    *old = replacement;
}

fn merge_array_of_tables(old: &mut ArrayOfTables, new: &ArrayOfTables) {
    while old.len() > new.len() {
        old.remove(old.len() - 1);
    }
    for (index, new_table) in new.iter().enumerate() {
        match old.get_mut(index) {
            Some(old_table) => merge_table(old_table, new_table),
            None => {
                let mut table = new_table.clone();
                clear_positions(&mut table);
                old.push(table);
            }
        }
    }
}

/// A copy of `item` without document positions from the serialized
/// document, so new tables sort after their siblings in the original.
fn detached(item: &Item) -> Item {
    let mut item = item.clone();
    match &mut item {
        Item::Table(table) => clear_positions(table),
        Item::ArrayOfTables(array) => array.iter_mut().for_each(clear_positions),
        Item::Value(_) | Item::None => {}
    }
    item
}

fn clear_positions(table: &mut toml_edit::Table) {
    table.set_position(None);
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(table) => clear_positions(table),
            Item::ArrayOfTables(array) => array.iter_mut().for_each(clear_positions),
            Item::Value(_) | Item::None => {}
        }
    }
}

/// Formatting-free form of an item, for "did this value change" checks.
fn plain(item: &Item) -> Option<toml::Value> {
    match item {
        Item::None => None,
        Item::Value(value) => Some(plain_value(value)),
        Item::Table(table) => Some(plain_table(table)),
        Item::ArrayOfTables(array) => Some(toml::Value::Array(
            array.iter().map(|table| plain_table(table)).collect(),
        )),
    }
}

fn plain_table(table: &dyn TableLike) -> toml::Value {
    toml::Value::Table(
        table
            .iter()
            .filter_map(|(key, item)| plain(item).map(|value| (key.to_owned(), value)))
            .collect(),
    )
}

fn plain_value(value: &Value) -> toml::Value {
    match value {
        Value::String(v) => toml::Value::String(v.value().clone()),
        Value::Integer(v) => toml::Value::Integer(*v.value()),
        Value::Float(v) => toml::Value::Float(*v.value()),
        Value::Boolean(v) => toml::Value::Boolean(*v.value()),
        Value::Datetime(v) => toml::Value::Datetime(*v.value()),
        Value::Array(array) => toml::Value::Array(array.iter().map(plain_value).collect()),
        Value::InlineTable(table) => plain_table(table),
    }
}

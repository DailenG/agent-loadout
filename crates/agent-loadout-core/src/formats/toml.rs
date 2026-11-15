//! TOML editing through `toml_edit`.
//!
//! An existing entry is merged key by key, so comments and unchanged keys in
//! its table survive. A new entry in a table container is a new `[a.b.name]`
//! table (nested objects in it are inline tables), placed after the last
//! table of the container; in an inline container (or one whose entries are
//! all `name = { ... }`) it is an inline table. Tables outside the entry are
//! never touched.
//!
//! Comment lines directly above a removed entry go with it (`toml_edit`
//! keeps them in the entry's decor). Lines above the last blank line before
//! it, such as a file header or a section comment, stay in place.

use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, TableLike, Value};

use super::text::line_col;
use super::{Format, NativeMap, NativeValue, unexpected_shape};
use crate::error::{CoreError, ErrorCode};

#[derive(Debug, Clone)]
pub(super) struct TomlBody {
    doc: DocumentMut,
    /// The parsed text, returned as is until the first change.
    original: String,
    dirty: bool,
}

impl TomlBody {
    pub(super) fn parse(text: String) -> Result<Self, CoreError> {
        let doc = text.parse::<DocumentMut>().map_err(|err| {
            let at = err
                .span()
                .map(|span| {
                    let (line, column) = line_col(&text, span.start);
                    format!(" on line {line} column {column}")
                })
                .unwrap_or_default();
            CoreError::new(
                ErrorCode::ParseError,
                format!("invalid TOML{at}: {}", err.message().trim_end()),
            )
        })?;
        Ok(Self {
            doc,
            original: text,
            dirty: false,
        })
    }

    pub(super) fn text(&self) -> String {
        if self.dirty {
            self.doc.to_string()
        } else {
            self.original.clone()
        }
    }

    pub(super) fn list(&self, container: &[&str]) -> Result<Vec<String>, CoreError> {
        let Some(table) = table_at(self.doc.as_item(), container)? else {
            return Ok(Vec::new());
        };
        Ok(table
            .iter()
            .filter(|(_, item)| !item.is_none())
            .map(|(key, _)| key.to_owned())
            .collect())
    }

    pub(super) fn get(
        &self,
        container: &[&str],
        key: &str,
    ) -> Result<Option<NativeValue>, CoreError> {
        let Some(table) = table_at(self.doc.as_item(), container)? else {
            return Ok(None);
        };
        Ok(table.get(key).and_then(to_native))
    }

    pub(super) fn upsert(
        &mut self,
        container: &[&str],
        key: &str,
        value: &NativeValue,
        created: &mut Vec<Vec<String>>,
    ) -> Result<bool, CoreError> {
        // Converting first validates the whole value (TOML has no null)
        // before anything changes.
        let fresh = to_value(value)?;
        if let Some(table) = table_at(self.doc.as_item(), container)?
            && table.get(key).and_then(to_native).as_ref() == Some(value)
        {
            return Ok(false);
        }
        let item = item_at_or_create(self.doc.as_item_mut(), container, created)?;
        let std_container = item.is_table();
        let table = item
            .as_table_like_mut()
            .expect("checked by item_at_or_create");
        match table.get_mut(key).filter(|item| !item.is_none()) {
            Some(existing) => merge_item(existing, value)?,
            None => {
                // A table container gets a new `[container.key]` table,
                // unless its entries are all inline (`key = { ... }`).
                let inline_siblings = table.iter().any(|(_, item)| item.is_value())
                    && !table
                        .iter()
                        .any(|(_, item)| item.is_table() || item.is_array_of_tables());
                let entry = match (std_container && !inline_siblings, fresh) {
                    (true, Value::InlineTable(inline)) => Item::Table(inline.into_table()),
                    (_, fresh) => Item::Value(fresh),
                };
                insert(table, key, entry);
            }
        }
        self.dirty = true;
        Ok(true)
    }

    pub(super) fn remove(&mut self, container: &[&str], key: &str) -> Result<bool, CoreError> {
        let Some(item) = item_at_mut(self.doc.as_item_mut(), container)? else {
            return Ok(false);
        };
        let container_position = item.as_table().and_then(Table::position);
        let table = item.as_table_like_mut().expect("checked by item_at_mut");
        let key_prefix = table
            .key(key)
            .and_then(|key| key.leaf_decor().prefix())
            .and_then(|prefix| prefix.as_str())
            .map(str::to_owned);
        let next_value_key = table
            .iter()
            .skip_while(|(name, _)| *name != key)
            .skip(1)
            .find(|(_, item)| item.is_value())
            .map(|(name, _)| name.to_owned());
        let Some(removed) = table.remove(key).filter(|item| !item.is_none()) else {
            return Ok(false);
        };
        self.dirty = true;
        match removed {
            Item::Table(removed) => {
                let kept = detached(removed.decor().prefix().and_then(|prefix| prefix.as_str()));
                if let (Some(kept), Some(position)) = (kept, removed.position()) {
                    self.prepend_after(position, &kept);
                }
            }
            Item::Value(_) => {
                let Some(kept) = detached(key_prefix.as_deref()) else {
                    return Ok(true);
                };
                match next_value_key.and_then(|next| table.key_mut(&next)) {
                    Some(mut next) => {
                        let prefix = next
                            .leaf_decor()
                            .prefix()
                            .and_then(|prefix| prefix.as_str());
                        let joined = join_prefix(&kept, prefix.unwrap_or(""));
                        next.leaf_decor_mut().set_prefix(joined);
                    }
                    None => {
                        if let Some(position) = container_position {
                            self.prepend_after(position, &kept);
                        }
                    }
                }
            }
            Item::None | Item::ArrayOfTables(_) => {}
        }
        Ok(true)
    }

    /// Puts `kept` in front of whatever is rendered after the table at
    /// `position`: the next table header, or the end of the document.
    fn prepend_after(&mut self, position: isize, kept: &str) {
        let mut next = None;
        next_table_position(self.doc.as_table(), position, &mut next);
        let target = next.and_then(|next| table_at_position(self.doc.as_table_mut(), next));
        match target {
            Some(table) => {
                let prefix = table.decor().prefix().and_then(|prefix| prefix.as_str());
                let joined = join_prefix(kept, prefix.unwrap_or("\n"));
                table.decor_mut().set_prefix(joined);
            }
            None => {
                // Nothing follows, so the blank lines that separated the
                // comment from the removed item are not needed.
                let kept = kept.trim_end_matches(['\n', ' ', '\t']);
                let trailing = self.doc.trailing().as_str().unwrap_or("").to_owned();
                self.doc.set_trailing(format!("{kept}\n{trailing}"));
            }
        }
    }

    pub(super) fn set_list_member(
        &mut self,
        list: &[&str],
        member: &str,
        present: bool,
        created: &mut Vec<Vec<String>>,
    ) -> Result<bool, CoreError> {
        let (name, parent) = list.split_last().expect("checked by Document");
        let current = match table_at(self.doc.as_item(), parent)? {
            Some(table) => table.get(name).filter(|item| !item.is_none()),
            None => None,
        };
        let Some(current) = current else {
            if !present {
                return Ok(false);
            }
            let item = item_at_or_create(self.doc.as_item_mut(), parent, created)?;
            let table = item
                .as_table_like_mut()
                .expect("checked by item_at_or_create");
            let array = Array::from_iter([member]);
            insert(table, name, Item::Value(Value::Array(array)));
            created.push(owned(list));
            self.dirty = true;
            return Ok(true);
        };
        let Some(array) = current.as_array() else {
            return Err(unexpected_shape(Format::Toml, list, "an array"));
        };
        let hits: Vec<usize> = array
            .iter()
            .enumerate()
            .filter(|(_, value)| value.as_str() == Some(member))
            .map(|(index, _)| index)
            .collect();
        if present != hits.is_empty() {
            return Ok(false);
        }
        let array = item_at_mut(self.doc.as_item_mut(), list)?
            .and_then(Item::as_array_mut)
            .expect("checked above");
        if present {
            push_like_siblings(array, Value::from(member));
        } else {
            for index in hits.into_iter().rev() {
                remove_like_siblings(array, index);
            }
        }
        self.dirty = true;
        Ok(true)
    }

    /// Removes the table or array at `path` when it is empty. The root is
    /// never removed.
    pub(super) fn remove_if_empty(&mut self, path: &[&str]) -> Result<bool, CoreError> {
        let Some((name, parent)) = path.split_last() else {
            return Ok(false);
        };
        let Some(item) = item_at_mut(self.doc.as_item_mut(), parent)? else {
            return Ok(false);
        };
        let table = item.as_table_like_mut().expect("checked by item_at_mut");
        let empty = table.get(name).is_some_and(|item| match item {
            Item::Table(table) => table.is_empty(),
            Item::Value(Value::InlineTable(table)) => table.is_empty(),
            Item::Value(Value::Array(array)) => array.is_empty(),
            _ => false,
        });
        if empty {
            table.remove(name);
            self.dirty = true;
        }
        Ok(empty)
    }
}

/// The lines of a removed item's prefix up to and including the last blank
/// line, when they hold a comment: what was written above the item but not
/// attached to it.
fn detached(prefix: Option<&str>) -> Option<String> {
    let lines: Vec<&str> = prefix?.split_inclusive('\n').collect();
    let last_blank = lines
        .iter()
        .rposition(|line| line.ends_with('\n') && line.trim().is_empty())?;
    let kept = &lines[..=last_blank];
    kept.iter()
        .any(|line| !line.trim().is_empty())
        .then(|| kept.concat())
}

/// `kept` (ending with a blank line) in front of `prefix`, without doubling
/// the blank line `prefix` may start with.
fn join_prefix(kept: &str, prefix: &str) -> String {
    let rest = match prefix.split_inclusive('\n').next() {
        Some(first) if first.ends_with('\n') && first.trim().is_empty() => &prefix[first.len()..],
        _ => prefix,
    };
    format!("{kept}{rest}")
}

/// Tables `toml_edit` writes with a `[header]` (and so with their decor):
/// not dotted, and not implicit without keys of their own.
fn has_header(table: &Table) -> bool {
    !table.is_dotted() && !(table.is_implicit() && table.get_values().is_empty())
}

/// The smallest position after `after` of a table rendered with a header.
fn next_table_position(table: &Table, after: isize, next: &mut Option<isize>) {
    if has_header(table)
        && let Some(position) = table.position()
        && position > after
        && next.is_none_or(|next| position < next)
    {
        *next = Some(position);
    }
    for (_, item) in table.iter() {
        match item {
            Item::Table(child) => next_table_position(child, after, next),
            Item::ArrayOfTables(array) => {
                for child in array.iter() {
                    next_table_position(child, after, next);
                }
            }
            Item::None | Item::Value(_) => {}
        }
    }
}

fn table_at_position(table: &mut Table, position: isize) -> Option<&mut Table> {
    if table.position() == Some(position) && has_header(table) {
        return Some(table);
    }
    for (_, item) in table.iter_mut() {
        let found = match item {
            Item::Table(child) => table_at_position(child, position),
            Item::ArrayOfTables(array) => array
                .iter_mut()
                .find_map(|child| table_at_position(child, position)),
            Item::None | Item::Value(_) => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// The table-like item at `path`; `None` when a key on the way is missing.
fn table_at<'a>(mut item: &'a Item, path: &[&str]) -> Result<Option<&'a dyn TableLike>, CoreError> {
    for (depth, key) in path.iter().enumerate() {
        let table = item
            .as_table_like()
            .ok_or_else(|| unexpected_shape(Format::Toml, &path[..depth], "a table"))?;
        match table.get(key).filter(|item| !item.is_none()) {
            Some(child) => item = child,
            None => return Ok(None),
        }
    }
    item.as_table_like()
        .map(Some)
        .ok_or_else(|| unexpected_shape(Format::Toml, path, "a table"))
}

/// The item at `path`, whose parent items are tables, checked to be table-like
/// unless it is the last one of a list path.
fn item_at_mut<'a>(
    mut item: &'a mut Item,
    path: &[&str],
) -> Result<Option<&'a mut Item>, CoreError> {
    for (depth, key) in path.iter().enumerate() {
        let table = item
            .as_table_like_mut()
            .ok_or_else(|| unexpected_shape(Format::Toml, &path[..depth], "a table"))?;
        match table.get_mut(key).filter(|item| !item.is_none()) {
            Some(child) => item = child,
            None => return Ok(None),
        }
    }
    if item.as_table_like().is_none() && item.as_array().is_none() {
        return Err(unexpected_shape(Format::Toml, path, "a table"));
    }
    Ok(Some(item))
}

/// The table-like item at `path`, creating missing tables: implicit tables
/// (no header of their own) under tables, inline tables under inline tables.
fn item_at_or_create<'a>(
    mut item: &'a mut Item,
    path: &[&str],
    created: &mut Vec<Vec<String>>,
) -> Result<&'a mut Item, CoreError> {
    for (depth, key) in path.iter().enumerate() {
        let std_parent = item.is_table();
        let table = item
            .as_table_like_mut()
            .ok_or_else(|| unexpected_shape(Format::Toml, &path[..depth], "a table"))?;
        if table.get(key).is_none_or(Item::is_none) {
            let child = if std_parent {
                let mut child = Table::new();
                child.set_implicit(true);
                Item::Table(child)
            } else {
                Item::Value(Value::InlineTable(InlineTable::new()))
            };
            table.insert(key, child);
            created.push(owned(&path[..=depth]));
        }
        item = table.get_mut(key).expect("present or just inserted");
    }
    if item.as_table_like().is_none() {
        return Err(unexpected_shape(Format::Toml, path, "a table"));
    }
    Ok(item)
}

/// Inserts a new key at the end of `table`, indented like the keys already
/// there.
fn insert(table: &mut dyn TableLike, key: &str, item: Item) {
    let indent = table
        .iter()
        .filter(|(_, item)| item.is_value())
        .last()
        .and_then(|(last, _)| table.key(last))
        .and_then(|last| last.leaf_decor().prefix())
        .and_then(|prefix| prefix.as_str())
        .map(|prefix| prefix.rsplit('\n').next().unwrap_or(prefix))
        .filter(|indent| !indent.is_empty() && indent.trim().is_empty())
        .map(str::to_owned);
    let is_value = item.is_value();
    table.insert(key, item);
    if let (true, Some(indent)) = (is_value, indent)
        && let Some(mut key) = table.key_mut(key)
    {
        key.leaf_decor_mut().set_prefix(indent);
    }
}

/// Makes `item` equal to `new`, keeping the text of every key whose value
/// already is and the decor (spacing, trailing comment) of a replaced value.
fn merge_item(item: &mut Item, new: &NativeValue) -> Result<(), CoreError> {
    if to_native(item).as_ref() == Some(new) {
        return Ok(());
    }
    if let (Some(table), NativeValue::Object(map)) = (item.as_table_like_mut(), new) {
        return merge_table(table, map);
    }
    let mut value = to_value(new)?;
    match item {
        Item::Value(old) => {
            *value.decor_mut() = old.decor().clone();
            *old = value;
        }
        _ => *item = Item::Value(value),
    }
    Ok(())
}

fn merge_table(table: &mut dyn TableLike, new: &NativeMap) -> Result<(), CoreError> {
    let stale: Vec<String> = table
        .iter()
        .filter(|(key, item)| !item.is_none() && !new.contains_key(key))
        .map(|(key, _)| key.to_owned())
        .collect();
    for key in stale {
        table.remove(&key);
    }
    for (key, value) in new.iter() {
        match table.get_mut(key).filter(|item| !item.is_none()) {
            Some(item) => merge_item(item, value)?,
            None => insert(table, key, Item::Value(to_value(value)?)),
        }
    }
    Ok(())
}

/// Appends to an array, formatted like its last element: on its own line in
/// a multi-line array, after `, ` in a one-line array.
fn push_like_siblings(array: &mut Array, value: Value) {
    let Some(last) = array.iter_mut().last() else {
        array.push(value);
        return;
    };
    let mut decor = last.decor().clone();
    // A first element has no space before it; the next one does.
    if decor.prefix().and_then(|prefix| prefix.as_str()) == Some("") {
        decor.set_prefix(" ");
    }
    // What follows the last element (such as the line break before `]`)
    // moves to the new last element.
    last.decor_mut().set_suffix("");
    let mut value = value;
    std::mem::swap(value.decor_mut(), &mut decor);
    array.push_formatted(value);
}

/// Removes element `index`, handing its leading (first) or trailing (last)
/// decor to the neighbor that takes its place, the inverse of
/// [`push_like_siblings`].
fn remove_like_siblings(array: &mut Array, index: usize) {
    let removed = array.remove(index);
    let len = array.len();
    if len == 0 {
        return;
    }
    if index == 0 {
        if let (Some(prefix), Some(first)) = (removed.decor().prefix().cloned(), array.get_mut(0)) {
            first.decor_mut().set_prefix(prefix);
        }
    } else if index == len
        && let (Some(suffix), Some(last)) =
            (removed.decor().suffix().cloned(), array.get_mut(len - 1))
    {
        last.decor_mut().set_suffix(suffix);
    }
}

fn to_native(item: &Item) -> Option<NativeValue> {
    match item {
        Item::None => None,
        Item::Value(value) => Some(value_to_native(value)),
        Item::Table(table) => Some(table_to_native(table)),
        Item::ArrayOfTables(array) => Some(NativeValue::Array(
            array.iter().map(|table| table_to_native(table)).collect(),
        )),
    }
}

fn table_to_native(table: &dyn TableLike) -> NativeValue {
    NativeValue::Object(
        table
            .iter()
            .filter_map(|(key, item)| Some((key, to_native(item)?)))
            .collect(),
    )
}

fn value_to_native(value: &Value) -> NativeValue {
    match value {
        Value::String(value) => NativeValue::String(value.value().clone()),
        Value::Integer(value) => NativeValue::Integer(*value.value()),
        Value::Float(value) => NativeValue::Float(*value.value()),
        Value::Boolean(value) => NativeValue::Bool(*value.value()),
        Value::Datetime(value) => NativeValue::Datetime(value.value().to_string()),
        Value::Array(array) => NativeValue::Array(array.iter().map(value_to_native).collect()),
        Value::InlineTable(table) => table_to_native(table),
    }
}

fn to_value(value: &NativeValue) -> Result<Value, CoreError> {
    Ok(match value {
        NativeValue::Null => {
            return Err(CoreError::new(ErrorCode::InvalidValue, "TOML has no null"));
        }
        NativeValue::Bool(value) => Value::from(*value),
        NativeValue::Integer(value) => Value::from(*value),
        NativeValue::Float(value) => Value::from(*value),
        NativeValue::String(value) => Value::from(value.as_str()),
        NativeValue::Datetime(text) => {
            let datetime = text.parse::<toml_edit::Datetime>().map_err(|err| {
                CoreError::new(
                    ErrorCode::InvalidValue,
                    format!("invalid TOML date-time {text:?}: {err}"),
                )
            })?;
            Value::from(datetime)
        }
        NativeValue::Array(items) => {
            Value::Array(items.iter().map(to_value).collect::<Result<Array, _>>()?)
        }
        NativeValue::Object(map) => {
            let mut table = InlineTable::new();
            for (key, value) in map.iter() {
                table.insert(key, to_value(value)?);
            }
            Value::InlineTable(table)
        }
    })
}

fn owned(path: &[&str]) -> Vec<String> {
    path.iter().map(|key| (*key).to_owned()).collect()
}

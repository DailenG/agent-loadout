//! JSON and JSONC editing through `jsonc-parser`'s CST.
//!
//! The body is kept as text and parsed again for each step of an operation
//! (the CST is `Rc`-based, so it cannot live in a `Send` document). An
//! operation that changes nothing never re-serializes, so its output is the
//! input, and a failed operation restores the text it started from.
//!
//! The CST lays out new values on several lines, indented like the file.
//! Inside a one-line object (`{"a": 1}`, or anything in a one-line file) it
//! would also break the object over several lines, so members are added and
//! values replaced there by splicing one-line text that copies the object's
//! own `:` and `,` spacing.

use jsonc_parser::cst::{
    CstContainerNode, CstInputValue, CstLeafNode, CstNode, CstObject, CstObjectProp, CstRootNode,
};
use jsonc_parser::{CollectOptions, ParseOptions};

use super::{Format, NativeMap, NativeValue, unexpected_shape};
use crate::error::{CoreError, ErrorCode};

#[derive(Debug, Clone)]
pub(super) struct JsonBody {
    text: String,
    strict: bool,
    /// The original text when it held no value (an empty or new file). Once
    /// the root object this document created is empty again, the text goes
    /// back to it.
    blank: Option<String>,
}

impl JsonBody {
    pub(super) fn parse(text: String, strict: bool) -> Result<Self, CoreError> {
        if strict {
            // The CST parser keeps comments as tokens whatever the options
            // say; the AST parser enforces `allow_comments`.
            jsonc_parser::parse_to_ast(&text, &CollectOptions::default(), &options(true))
                .map_err(|err| parse_error(strict, &err))?;
        }
        let root = parse(&text, strict)?;
        let blank = root.value().is_none().then(|| text.clone());
        Ok(Self {
            text,
            strict,
            blank,
        })
    }

    pub(super) fn text(&self) -> String {
        self.text.clone()
    }

    fn format(&self) -> Format {
        if self.strict {
            Format::Json
        } else {
            Format::Jsonc
        }
    }

    /// The CST of the current text, which this module only ever sets to
    /// valid JSON(C).
    fn root(&self) -> Result<CstRootNode, CoreError> {
        parse(&self.text, self.strict)
    }

    pub(super) fn list(&self, container: &[&str]) -> Result<Vec<String>, CoreError> {
        let root = self.root()?;
        let Some(object) = self.object_at(&root, container)? else {
            return Ok(Vec::new());
        };
        Ok(object
            .properties()
            .iter()
            .filter_map(|prop| prop.decoded_name())
            .collect())
    }

    pub(super) fn get(
        &self,
        container: &[&str],
        key: &str,
    ) -> Result<Option<NativeValue>, CoreError> {
        let root = self.root()?;
        let Some(object) = self.object_at(&root, container)? else {
            return Ok(None);
        };
        Ok(object.get(key).map(|prop| {
            prop.value()
                .map_or(NativeValue::Null, |node| to_native(&node))
        }))
    }

    pub(super) fn upsert(
        &mut self,
        container: &[&str],
        key: &str,
        value: &NativeValue,
        created: &mut Vec<Vec<String>>,
    ) -> Result<bool, CoreError> {
        check_finite(value)?;
        let current = self.get(container, key)?;
        if current.as_ref() == Some(value) {
            return Ok(false);
        }
        self.atomic(|body| {
            body.ensure_object(container, created)?;
            match current {
                Some(_) => body.merge(container, key, value),
                None => body.append(container, key, value),
            }
        })?;
        Ok(true)
    }

    pub(super) fn remove(&mut self, container: &[&str], key: &str) -> Result<bool, CoreError> {
        let root = self.root()?;
        let Some(prop) = self
            .object_at(&root, container)?
            .and_then(|object| object.get(key))
        else {
            return Ok(false);
        };
        remove_member(prop.into());
        self.text = root.to_string();
        Ok(true)
    }

    pub(super) fn set_list_member(
        &mut self,
        list: &[&str],
        member: &str,
        present: bool,
        created: &mut Vec<Vec<String>>,
    ) -> Result<bool, CoreError> {
        let (name, parent) = list.split_last().expect("checked by Document");
        let root = self.root()?;
        let current = match self.object_at(&root, parent)? {
            Some(object) => object.get(name),
            None => None,
        };
        let Some(prop) = current else {
            if !present {
                return Ok(false);
            }
            let array = NativeValue::Array(vec![NativeValue::from(member)]);
            self.atomic(|body| {
                body.ensure_object(parent, created)?;
                body.append(parent, name, &array)
            })?;
            created.push(owned(list));
            return Ok(true);
        };
        let array = prop
            .value()
            .and_then(|node| node.as_array())
            .ok_or_else(|| unexpected_shape(self.format(), list, "an array"))?;
        let hits: Vec<CstNode> = array
            .elements()
            .into_iter()
            .filter(|node| to_native(node).as_str() == Some(member))
            .collect();
        if present == !hits.is_empty() {
            return Ok(false);
        }
        if present {
            array.append(CstInputValue::String(member.to_owned()));
        } else {
            for node in hits {
                remove_member(node);
            }
        }
        self.text = root.to_string();
        Ok(true)
    }

    /// Removes the object or array at `path` when it is empty. The root
    /// object is "removed" by restoring the blank original text.
    pub(super) fn remove_if_empty(&mut self, path: &[&str]) -> Result<bool, CoreError> {
        let root = self.root()?;
        let Some((name, parent)) = path.split_last() else {
            let empty = root
                .object_value()
                .is_some_and(|object| object.properties().is_empty());
            return Ok(match (&self.blank, empty) {
                (Some(blank), true) => {
                    self.text = blank.clone();
                    true
                }
                _ => false,
            });
        };
        let Some(prop) = self
            .object_at(&root, parent)?
            .and_then(|object| object.get(name))
        else {
            return Ok(false);
        };
        let empty = match prop.value() {
            Some(CstNode::Container(CstContainerNode::Object(object))) => {
                object.properties().is_empty()
            }
            Some(CstNode::Container(CstContainerNode::Array(array))) => array.elements().is_empty(),
            _ => false,
        };
        if empty {
            remove_member(prop.into());
            self.text = root.to_string();
        }
        Ok(empty)
    }

    /// Runs `steps`; on failure the text goes back to what it was.
    fn atomic(
        &mut self,
        steps: impl FnOnce(&mut Self) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let saved = self.text.clone();
        let result = steps(self);
        if result.is_err() {
            self.text = saved;
        }
        result
    }

    /// Creates the root object and the objects on `path` that are missing.
    fn ensure_object(
        &mut self,
        path: &[&str],
        created: &mut Vec<Vec<String>>,
    ) -> Result<(), CoreError> {
        let root = self.root()?;
        if root.value().is_none() {
            root.object_value_or_set();
            self.text = root.to_string();
            created.push(Vec::new());
        }
        for depth in 0..path.len() {
            let root = self.root()?;
            if self.object_at(&root, &path[..=depth])?.is_none() {
                self.append(
                    &path[..depth],
                    path[depth],
                    &NativeValue::Object(NativeMap::new()),
                )?;
                created.push(owned(&path[..=depth]));
            }
        }
        Ok(())
    }

    /// Makes `key` in the object at `parent` equal to `new`, keeping the text
    /// of every member whose value already is.
    fn merge(&mut self, parent: &[&str], key: &str, new: &NativeValue) -> Result<(), CoreError> {
        let root = self.root()?;
        let prop = self.prop_at(&root, parent, key)?;
        let old = prop.value();
        if old.as_ref().map(to_native).as_ref() == Some(new) {
            return Ok(());
        }
        let (Some(object), NativeValue::Object(map)) = (old.and_then(|node| node.as_object()), new)
        else {
            return self.replace(parent, key, new);
        };
        let stale: Vec<CstObjectProp> = object
            .properties()
            .into_iter()
            .filter(|prop| {
                prop.decoded_name()
                    .is_some_and(|name| !map.contains_key(&name))
            })
            .collect();
        if !stale.is_empty() {
            for prop in stale {
                remove_member(prop.into());
            }
            self.text = root.to_string();
        }
        let mut path = parent.to_vec();
        path.push(key);
        for (member, value) in map.iter() {
            let root = self.root()?;
            let exists = self
                .object_at(&root, &path)?
                .is_some_and(|object| object.get(member).is_some());
            if exists {
                self.merge(&path, member, value)?;
            } else {
                self.append(&path, member, value)?;
            }
        }
        Ok(())
    }

    /// Appends `key: value` to the object at `path`.
    fn append(&mut self, path: &[&str], key: &str, value: &NativeValue) -> Result<(), CoreError> {
        let root = self.root()?;
        let object = self
            .object_at(&root, path)?
            .expect("the caller checked the object exists");
        if !one_line(&root, &object) {
            object.append(key, to_input(value));
            self.text = root.to_string();
            return Ok(());
        }
        let spacing = Spacing::of(&root, &object);
        let member = format!(
            "{}{}{}",
            json_string(key),
            spacing.colon,
            spacing.render(value)
        );
        let (at, insert) = match object.properties().last() {
            Some(last) => (
                end_offset(&last.clone().into()),
                format!("{}{member}", spacing.comma),
            ),
            // Just after `{`.
            None => (start_offset(&object.clone().into()) + 1, member),
        };
        let mut text = root.to_string();
        text.insert_str(at, &insert);
        self.text = text;
        Ok(())
    }

    /// Replaces the value of `key` in the object at `parent`.
    fn replace(
        &mut self,
        parent: &[&str],
        key: &str,
        value: &NativeValue,
    ) -> Result<(), CoreError> {
        let root = self.root()?;
        let object = self
            .object_at(&root, parent)?
            .expect("the caller checked the object exists");
        let prop = object
            .get(key)
            .expect("the caller checked the member exists");
        match prop.value() {
            Some(old) if one_line(&root, &object) => {
                let start = start_offset(&old);
                let end = start + old.to_string().len();
                let mut text = root.to_string();
                text.replace_range(start..end, &Spacing::of(&root, &object).render(value));
                self.text = text;
            }
            _ => {
                prop.set_value(to_input(value));
                self.text = root.to_string();
            }
        }
        Ok(())
    }

    fn prop_at(
        &self,
        root: &CstRootNode,
        parent: &[&str],
        key: &str,
    ) -> Result<CstObjectProp, CoreError> {
        Ok(self
            .object_at(root, parent)?
            .and_then(|object| object.get(key))
            .expect("the caller checked the member exists"))
    }

    fn object_at(&self, root: &CstRootNode, path: &[&str]) -> Result<Option<CstObject>, CoreError> {
        let Some(value) = root.value() else {
            return Ok(None);
        };
        let mut object = value
            .as_object()
            .ok_or_else(|| unexpected_shape(self.format(), &[], "an object"))?;
        for (depth, key) in path.iter().enumerate() {
            let Some(prop) = object.get(key) else {
                return Ok(None);
            };
            object = prop
                .value()
                .and_then(|node| node.as_object())
                .ok_or_else(|| unexpected_shape(self.format(), &path[..=depth], "an object"))?;
        }
        Ok(Some(object))
    }
}

fn parse(text: &str, strict: bool) -> Result<CstRootNode, CoreError> {
    CstRootNode::parse(text, &options(strict)).map_err(|err| parse_error(strict, &err))
}

fn parse_error(strict: bool, err: &jsonc_parser::errors::ParseError) -> CoreError {
    let kind = if strict { "JSON" } else { "JSONC" };
    CoreError::new(ErrorCode::ParseError, format!("invalid {kind}: {err}"))
}

/// Strict JSON, or JSON plus comments and trailing commas. The JSON5
/// extensions `jsonc-parser` also knows stay off, so a JSON5 file never
/// passes as JSONC.
fn options(strict: bool) -> ParseOptions {
    ParseOptions {
        allow_comments: !strict,
        allow_trailing_commas: !strict,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    }
}

pub(super) fn parse_value(text: &str) -> Result<NativeValue, CoreError> {
    let root = parse(text, false)?;
    root.value()
        .map(|node| to_native(&node))
        .ok_or_else(|| CoreError::new(ErrorCode::ParseError, "invalid JSON: no value"))
}

/// Whether new text in `object` belongs on one line: the object is on one
/// line and has members, or it is empty and the whole file is one line.
fn one_line(root: &CstRootNode, object: &CstObject) -> bool {
    if object.to_string().contains('\n') {
        return false;
    }
    if !object.properties().is_empty() {
        return true;
    }
    root.object_value()
        .is_some_and(|root| !root.properties().is_empty() && !root.to_string().contains('\n'))
}

/// The `:` and `,` spacing of a one-line object.
struct Spacing {
    colon: String,
    comma: String,
}

impl Spacing {
    /// Copied from `object`, or from the root object when that is on one
    /// line too; otherwise `": "` and `", "`.
    fn of(root: &CstRootNode, object: &CstObject) -> Self {
        let samples: Vec<CstObject> = std::iter::once(object.clone())
            .chain(
                root.object_value()
                    .filter(|root| !root.to_string().contains('\n')),
            )
            .collect();
        let colon = samples
            .iter()
            .flat_map(CstObject::properties)
            .find_map(|prop| colon_of(&prop))
            .unwrap_or_else(|| ": ".to_owned());
        let comma = samples
            .iter()
            .find_map(comma_of)
            .unwrap_or_else(|| if colon.ends_with(' ') { ", " } else { "," }.to_owned());
        Self { colon, comma }
    }

    /// `value` as one-line JSON with this spacing.
    fn render(&self, value: &NativeValue) -> String {
        match value {
            NativeValue::Array(items) => {
                let items: Vec<String> = items.iter().map(|item| self.render(item)).collect();
                format!("[{}]", items.join(&self.comma))
            }
            NativeValue::Object(map) => {
                let members: Vec<String> = map
                    .iter()
                    .map(|(key, value)| {
                        format!("{}{}{}", json_string(key), self.colon, self.render(value))
                    })
                    .collect();
                format!("{{{}}}", members.join(&self.comma))
            }
            scalar => match to_input(scalar) {
                CstInputValue::Null => "null".to_owned(),
                CstInputValue::Bool(value) => value.to_string(),
                CstInputValue::Number(text) => text,
                CstInputValue::String(text) => json_string(&text),
                CstInputValue::Array(_) | CstInputValue::Object(_) => unreachable!("scalar"),
            },
        }
    }
}

/// The text between a member's name and its value (`:`, `: `).
fn colon_of(prop: &CstObjectProp) -> Option<String> {
    let children = prop.children();
    let name = children
        .iter()
        .position(|node| node.as_string_lit().is_some())?;
    let value = prop.value()?.child_index();
    Some(
        children[name + 1..value]
            .iter()
            .map(ToString::to_string)
            .collect(),
    )
}

/// The text between the first two members of `object` (`,`, `, `).
fn comma_of(object: &CstObject) -> Option<String> {
    let props = object.properties();
    let (first, second) = (props.first()?, props.get(1)?);
    let children = object.children();
    Some(
        children[first.child_index() + 1..second.child_index()]
            .iter()
            .map(ToString::to_string)
            .collect(),
    )
}

/// Removes an object member or array element. A member takes with it the
/// own-line comments directly above it (no blank line between), as a TOML
/// table does, unless it is the only member: a member added to an object
/// that holds only comments goes below them, and removing it again must
/// leave them. Removing the first item of a one-line `[a, b]` or `{...}`
/// leaves no space after the bracket.
fn remove_member(node: CstNode) {
    let parent = node.parent();
    let gap = |parent: &CstContainerNode| parent.child_at_index(1).filter(CstNode::is_whitespace);
    let had_gap = parent.as_ref().map(|parent| gap(parent).is_some());
    if let Some(object) = parent.as_ref().and_then(CstContainerNode::as_object)
        && object.properties().len() > 1
    {
        for comment in attached_comments(&node) {
            if !comment.is_line_comment() {
                // Do what removing a line comment does: its indentation and
                // the line break before it go too.
                for previous in comment.previous_siblings() {
                    let newline = previous.is_newline();
                    if !newline && !previous.is_whitespace() {
                        break;
                    }
                    previous.remove();
                    if newline {
                        break;
                    }
                }
            }
            comment.remove();
        }
    }
    node.remove();
    if let (Some(parent), Some(false)) = (parent, had_gap)
        && !parent.to_string().contains('\n')
        && let Some(space) = gap(&parent)
    {
        space.remove();
    }
}

/// Own-line comments directly above `node`, nearest first.
fn attached_comments(node: &CstNode) -> Vec<jsonc_parser::cst::CstComment> {
    let previous: Vec<CstNode> = node.previous_siblings().collect();
    let skip_space = |mut index: usize| {
        while previous.get(index).is_some_and(CstNode::is_whitespace) {
            index += 1;
        }
        index
    };
    let mut comments = Vec::new();
    let mut index = skip_space(0);
    while previous.get(index).is_some_and(CstNode::is_newline) {
        let at = skip_space(index + 1);
        let Some(comment) = previous.get(at).and_then(CstNode::as_comment) else {
            break;
        };
        let before = skip_space(at + 1);
        // A comment after `{`, `,` or a value on its line is not own-line.
        if !previous.get(before).is_some_and(CstNode::is_newline) {
            break;
        }
        comments.push(comment);
        index = before;
    }
    comments
}

/// Byte offset of `node` in the text of its tree.
fn start_offset(node: &CstNode) -> usize {
    let mut offset = 0;
    let mut current = node.clone();
    loop {
        offset += current
            .previous_siblings()
            .map(|sibling| sibling.to_string().len())
            .sum::<usize>();
        match current.parent() {
            Some(parent) => current = CstNode::Container(parent),
            None => return offset,
        }
    }
}

fn end_offset(node: &CstNode) -> usize {
    start_offset(node) + node.to_string().len()
}

fn to_native(node: &CstNode) -> NativeValue {
    match node {
        CstNode::Container(CstContainerNode::Object(object)) => {
            // Duplicate keys: the first one counts, as in `CstObject::get`,
            // which every edit goes through.
            let mut map = NativeMap::new();
            for prop in object.properties() {
                if let Some(name) = prop.decoded_name()
                    && !map.contains_key(&name)
                {
                    let value = prop
                        .value()
                        .map_or(NativeValue::Null, |node| to_native(&node));
                    map.insert(name, value);
                }
            }
            NativeValue::Object(map)
        }
        CstNode::Container(CstContainerNode::Array(array)) => {
            NativeValue::Array(array.elements().iter().map(to_native).collect())
        }
        CstNode::Leaf(CstLeafNode::StringLit(string)) => NativeValue::String(
            string
                .decoded_value()
                .unwrap_or_else(|_| string.raw_value()),
        ),
        CstNode::Leaf(CstLeafNode::NumberLit(number)) => number_value(&number.to_string()),
        CstNode::Leaf(CstLeafNode::BooleanLit(boolean)) => NativeValue::Bool(boolean.value()),
        CstNode::Leaf(CstLeafNode::WordLit(word)) => NativeValue::String(word.to_string()),
        _ => NativeValue::Null,
    }
}

fn number_value(raw: &str) -> NativeValue {
    let integral = !raw.contains(['.', 'e', 'E']);
    if integral && let Ok(value) = raw.parse::<i64>() {
        return NativeValue::Integer(value);
    }
    match raw.parse::<f64>() {
        Ok(value) => NativeValue::Float(value),
        Err(_) => NativeValue::String(raw.to_owned()),
    }
}

/// JSON has no NaN or infinity.
fn check_finite(value: &NativeValue) -> Result<(), CoreError> {
    match value {
        NativeValue::Float(value) if !value.is_finite() => Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("JSON has no {value}"),
        )),
        NativeValue::Array(items) => items.iter().try_for_each(check_finite),
        NativeValue::Object(map) => map.iter().try_for_each(|(_, value)| check_finite(value)),
        _ => Ok(()),
    }
}

/// `value` for the CST; floats are finite (see [`check_finite`]).
fn to_input(value: &NativeValue) -> CstInputValue {
    match value {
        NativeValue::Null => CstInputValue::Null,
        NativeValue::Bool(value) => CstInputValue::Bool(*value),
        NativeValue::Integer(value) => CstInputValue::Number(value.to_string()),
        // Debug keeps a fraction (`1.0`), so the number reads back as a float.
        NativeValue::Float(value) => CstInputValue::Number(format!("{value:?}")),
        NativeValue::String(value) | NativeValue::Datetime(value) => {
            CstInputValue::String(value.clone())
        }
        NativeValue::Array(items) => CstInputValue::Array(items.iter().map(to_input).collect()),
        NativeValue::Object(map) => CstInputValue::Object(
            map.iter()
                .map(|(key, value)| (key.to_owned(), to_input(value)))
                .collect(),
        ),
    }
}

/// A JSON string literal, escaped like the CST escapes the strings it writes.
fn json_string(value: &str) -> String {
    let mut text = String::with_capacity(value.len() + 2);
    text.push('"');
    for ch in value.chars() {
        match ch {
            '"' => text.push_str("\\\""),
            '\\' => text.push_str("\\\\"),
            '\u{08}' => text.push_str("\\b"),
            '\u{0c}' => text.push_str("\\f"),
            '\n' => text.push_str("\\n"),
            '\r' => text.push_str("\\r"),
            '\t' => text.push_str("\\t"),
            ch if ch.is_control() => text.push_str(&format!("\\u{:04x}", u32::from(ch))),
            ch => text.push(ch),
        }
    }
    text.push('"');
    text
}

fn owned(path: &[&str]) -> Vec<String> {
    path.iter().map(|key| (*key).to_owned()).collect()
}

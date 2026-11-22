//! YAML editing by span splice (SPEC 7.2).
//!
//! The text is parsed with saphyr's event parser, whose events carry source
//! spans, into a small tree of nodes with byte ranges. An edit replaces,
//! deletes or inserts whole lines of one entry and leaves every other byte
//! alone: comments, quoting, key order and blank lines elsewhere survive.
//! New text is emitted in block style at the entry's column, with the
//! file's indentation width and sequence style (`key:` followed by `- x` at
//! the same column or indented). An existing mapping entry is updated
//! member by member, so members that keep their value keep their text.
//!
//! Only entries Loadout can locate unambiguously are edited. An edit fails
//! with `read_only_format`, leaving the text as it was, when the file holds
//! more than one document, when a mapping on the way to the container (the
//! root included) is in flow style (`{...}`) or carries an anchor, or when
//! the container holds an anchor or an alias anywhere: changing an anchored
//! node would change every alias of it too. Flow style inside an entry
//! (`args: [a, b]`) is fine; a replaced member is written in block style.
//!
//! A container path may end in a `[field]` segment (`["mcpServers",
//! "[name]"]`): the container is then a sequence of mappings, each entry
//! keyed by its `field` member (Continue's `mcpServers` list). An entry
//! read from it holds every member but `field`; a written entry gets
//! `field` first.
//!
//! Plain scalars resolve with the YAML 1.2 core schema (`null`, `~`, an
//! empty value, `true`/`false`, integers, floats); `.inf` and `.nan` read
//! as strings. Emitted strings are plain only when no YAML 1.1 or 1.2
//! reader could take them for anything else, and double-quoted otherwise.
//! Every edit is checked by reading the entry back; one that would not
//! read back as written is refused (`invalid_value`).

use std::collections::HashMap;

use saphyr_parser::{Event, Parser, ScalarStyle};

use super::{Format, NativeMap, NativeValue, display_path, unexpected_shape};
use crate::error::{CoreError, ErrorCode};

#[derive(Debug, Clone)]
pub(super) struct YamlBody {
    text: String,
}

impl YamlBody {
    pub(super) fn parse(text: String) -> Result<Self, CoreError> {
        build(&text)?;
        Ok(Self { text })
    }

    pub(super) fn text(&self) -> String {
        self.text.clone()
    }

    fn tree(&self) -> Result<Tree, CoreError> {
        build(&self.text)
    }

    pub(super) fn list(&self, container: &[&str]) -> Result<Vec<String>, CoreError> {
        let tree = self.tree()?;
        let path = PathSpec::of(container);
        let Some(node) = found_node(&tree, &path)? else {
            return Ok(Vec::new());
        };
        let mut names: Vec<String> = Vec::new();
        for name in path.entries(node)?.into_iter().map(|(name, _)| name) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        Ok(names)
    }

    pub(super) fn get(
        &self,
        container: &[&str],
        key: &str,
    ) -> Result<Option<NativeValue>, CoreError> {
        let tree = self.tree()?;
        let path = PathSpec::of(container);
        let Some(node) = found_node(&tree, &path)? else {
            return Ok(None);
        };
        let Some((_, entry)) = path
            .entries(node)?
            .into_iter()
            .find(|(name, _)| name == key)
        else {
            return Ok(None);
        };
        Ok(Some(path.entry_value(entry, &tree.anchors)))
    }

    pub(super) fn upsert(
        &mut self,
        container: &[&str],
        key: &str,
        value: &NativeValue,
        created: &mut Vec<Vec<String>>,
    ) -> Result<bool, CoreError> {
        check_finite(value)?;
        let path = PathSpec::of(container);
        if path.field.is_some() && value.as_object().is_none() {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!(
                    "an entry of the list at {} must be a mapping",
                    display_path(Format::Yaml, container.iter().copied())
                ),
            ));
        }
        if self.get(container, key)?.as_ref() == Some(value) {
            return Ok(false);
        }
        let saved = self.text.clone();
        let mut made = Vec::new();
        let result = self
            .upsert_steps(&path, key, value, &mut made)
            .and_then(|()| {
                if self.get(container, key)?.as_ref() == Some(value) {
                    Ok(())
                } else {
                    Err(CoreError::new(
                        ErrorCode::InvalidValue,
                        format!(
                            "{key:?} cannot be written to this YAML file so that it reads back as written"
                        ),
                    ))
                }
            });
        match result {
            Ok(()) => {
                created.extend(made);
                Ok(true)
            }
            Err(err) => {
                self.text = saved;
                Err(err)
            }
        }
    }

    pub(super) fn remove(&mut self, container: &[&str], key: &str) -> Result<bool, CoreError> {
        let tree = self.tree()?;
        let path = PathSpec::of(container);
        let found = find(&tree, &path.keys)?;
        let Found::Node { node, chain } = found else {
            return Ok(false);
        };
        let entries = path.entries(node)?;
        let Some(entry) = entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, entry)| *entry)
        else {
            return Ok(false);
        };
        check_editable(&tree, &chain, Some(node), container)?;
        let children = match &node.kind {
            Kind::Map(members) => members.len(),
            Kind::Seq(items) => items.len(),
            _ => unreachable!("entries come from a mapping or a sequence"),
        };
        if children == 1 && !path.keys.is_empty() {
            // The container stays, empty: `key: {}` or `key: []`.
            let empty = if path.field.is_some() {
                NativeValue::Array(Vec::new())
            } else {
                NativeValue::Object(NativeMap::new())
            };
            let (last, parent_keys) = path.keys.split_last().expect("checked above");
            return self
                .replace_member(&tree, parent_keys, last, &empty)
                .map(|()| true);
        }
        let (start, end) = match &node.kind {
            Kind::Map(members) => {
                let member = members
                    .iter()
                    .find(|(name, _)| key_of(name).as_deref() == Some(key))
                    .expect("listed above");
                let span = member_span(&self.text, member);
                with_attached_comments(&self.text, span.start, span.end, span.inline)
            }
            Kind::Seq(_) => {
                let span = item_span(&self.text, entry, container)?;
                with_attached_comments(&self.text, span.start, span.end, span.inline)
            }
            _ => unreachable!("entries come from a mapping or a sequence"),
        };
        self.text.replace_range(start..end, "");
        Ok(true)
    }

    pub(super) fn set_list_member(
        &mut self,
        list: &[&str],
        member: &str,
        present: bool,
        created: &mut Vec<Vec<String>>,
    ) -> Result<bool, CoreError> {
        let tree = self.tree()?;
        let keys: Vec<&str> = list.to_vec();
        match find(&tree, &keys)? {
            Found::Node { node, chain } => {
                let Kind::Seq(items) = &node.kind else {
                    return Err(unexpected_shape(Format::Yaml, list, "a sequence"));
                };
                let hits: Vec<&Node> = items
                    .iter()
                    .filter(|item| to_native(item, &tree.anchors).as_str() == Some(member))
                    .collect();
                if present == !hits.is_empty() {
                    return Ok(false);
                }
                check_editable(&tree, &chain, Some(node), list)?;
                let style = detect_style(&tree, &self.text);
                if present {
                    let span = item_span(&self.text, items.last().expect("not empty"), list)?;
                    let dash = item_span(&self.text, &items[0], list)?.dash_column;
                    let mut text = String::new();
                    emit_item(&mut text, dash, &NativeValue::from(member), style);
                    let at = past_comments(&self.text, span.end, dash);
                    self.insert_at(at, &text);
                } else if hits.len() == items.len() {
                    let (last, parent_keys) = keys.split_last().expect("a list path has keys");
                    self.replace_member(&tree, parent_keys, last, &NativeValue::Array(Vec::new()))?;
                } else {
                    for item in hits.into_iter().rev() {
                        let span = item_span(&self.text, item, list)?;
                        self.text.replace_range(span.start..span.end, "");
                    }
                }
                Ok(true)
            }
            Found::Missing { .. } if !present => Ok(false),
            Found::Missing {
                depth,
                parent,
                empty,
                chain,
            } => {
                check_editable(&tree, &chain, None, list)?;
                let mut nested = NativeValue::Array(vec![NativeValue::from(member)]);
                for key in keys[depth + 1..].iter().rev() {
                    nested = NativeValue::Object(NativeMap::from_iter([(*key, nested)]));
                }
                self.create(&tree, &keys, depth, parent, empty, &nested, created)?;
                Ok(true)
            }
        }
    }

    /// Removes the container at `path` when it is empty (or null). A
    /// `[field]` level is virtual: it counts as removed once its sequence
    /// is empty or gone, and the sequence itself goes with the level above.
    pub(super) fn remove_if_empty(&mut self, path: &[&str]) -> Result<bool, CoreError> {
        let tree = self.tree()?;
        let spec = PathSpec::of(path);
        if spec.field.is_some() {
            return Ok(match find(&tree, &spec.keys)? {
                Found::Node { node, .. } => {
                    matches!(&node.kind, Kind::Seq(items) if items.is_empty())
                }
                Found::Missing { .. } => true,
            });
        }
        let Some((last, parent_keys)) = spec.keys.split_last() else {
            return Ok(tree.root.as_ref().is_none_or(is_null));
        };
        let Found::Node { node: parent, .. } = find(&tree, parent_keys)? else {
            return Ok(false);
        };
        let Kind::Map(members) = &parent.kind else {
            return Ok(false);
        };
        let Some(member) = members
            .iter()
            .find(|(name, _)| key_of(name).as_deref() == Some(*last))
        else {
            return Ok(false);
        };
        if !is_empty_container(&member.1) {
            return Ok(false);
        }
        let span = member_span(&self.text, member);
        self.text.replace_range(span.start..span.end, "");
        Ok(true)
    }

    // -----------------------------------------------------------------------
    // Edit steps

    fn upsert_steps(
        &mut self,
        path: &PathSpec<'_>,
        key: &str,
        value: &NativeValue,
        created: &mut Vec<Vec<String>>,
    ) -> Result<(), CoreError> {
        let container = path.segments();
        let tree = self.tree()?;
        let style = detect_style(&tree, &self.text);
        match find(&tree, &path.keys)? {
            Found::Node { node, chain } => {
                check_editable(&tree, &chain, Some(node), &container)?;
                let entries = path.entries(node)?;
                let existing = entries
                    .iter()
                    .find(|(name, _)| name == key)
                    .map(|(_, e)| *e);
                match (&node.kind, path.field) {
                    (Kind::Map(members), None) => match existing {
                        Some(_) => {
                            let member = members
                                .iter()
                                .find(|(name, _)| key_of(name).as_deref() == Some(key))
                                .expect("listed above");
                            self.update_member(path, member, key, value, style)
                        }
                        None => {
                            let (column, at) = match members.first() {
                                Some((first, _)) => {
                                    let column = column(&self.text, first.start);
                                    let end = line_end(&self.text, node.end);
                                    (column, past_comments(&self.text, end, column))
                                }
                                None => (0, self.text.len()),
                            };
                            let mut text = String::new();
                            emit_member(&mut text, column, key, value, style);
                            self.insert_at(at, &text);
                            Ok(())
                        }
                    },
                    (Kind::Seq(items), Some(field)) => {
                        let named = with_field(field, key, value);
                        match existing {
                            Some(item) => self.update_item(path, item, &named, style),
                            None => {
                                let first = item_span(&self.text, &items[0], &container)?;
                                let last = item_span(
                                    &self.text,
                                    items.last().expect("not empty"),
                                    &container,
                                )?;
                                let mut text = String::new();
                                emit_item(&mut text, first.dash_column, &named, style);
                                let at = past_comments(&self.text, last.end, first.dash_column);
                                self.insert_at(at, &text);
                                Ok(())
                            }
                        }
                    }
                    _ => unreachable!("entries checked the container's kind"),
                }
            }
            Found::Missing {
                depth,
                parent,
                empty,
                chain,
            } => {
                check_editable(&tree, &chain, None, &container)?;
                if depth == path.keys.len() {
                    // The root is the container and holds nothing yet.
                    if path.field.is_some() {
                        return Err(unexpected_shape(Format::Yaml, &[], "a mapping"));
                    }
                    let mut text = String::new();
                    emit_member(&mut text, 0, key, value, style);
                    let end = self.text.len();
                    self.insert_at(end, &text);
                    return Ok(());
                }
                let mut nested = match path.field {
                    None => NativeValue::Object(NativeMap::from_iter([(key, value.clone())])),
                    Some(field) => NativeValue::Array(vec![with_field(field, key, value)]),
                };
                for segment in path.keys[depth + 1..].iter().rev() {
                    nested = NativeValue::Object(NativeMap::from_iter([(*segment, nested)]));
                }
                self.create(&tree, &path.keys, depth, parent, empty, &nested, created)?;
                if path.field.is_some() {
                    created.push(owned(&container));
                }
                Ok(())
            }
        }
    }

    /// Writes `nested` as the value of `keys[depth]`, the first missing (or
    /// empty) level of `keys`, and records the levels it creates.
    #[allow(clippy::too_many_arguments)]
    fn create(
        &mut self,
        tree: &Tree,
        keys: &[&str],
        depth: usize,
        parent: Option<&Node>,
        empty: Option<&(Node, Node)>,
        nested: &NativeValue,
        created: &mut Vec<Vec<String>>,
    ) -> Result<(), CoreError> {
        let style = detect_style(tree, &self.text);
        let segment = keys[depth];
        match (parent, empty) {
            (_, Some(member)) => {
                let span = member_span(&self.text, member);
                let column = column(&self.text, member.0.start);
                let mut text = String::new();
                emit_member(&mut text, column, segment, nested, style);
                if span.inline {
                    text.replace_range(..column, "");
                }
                self.text.replace_range(span.start..span.end, &text);
            }
            (Some(map), None) => {
                let Kind::Map(members) = &map.kind else {
                    unreachable!("find returns mappings as parents")
                };
                let column = members
                    .first()
                    .map_or(0, |(first, _)| column(&self.text, first.start));
                let mut text = String::new();
                emit_member(&mut text, column, segment, nested, style);
                let end = line_end(&self.text, map.end);
                let at = past_comments(&self.text, end, column);
                self.insert_at(at, &text);
            }
            (None, None) => {
                let mut text = String::new();
                emit_member(&mut text, 0, segment, nested, style);
                let end = self.text.len();
                self.insert_at(end, &text);
            }
        }
        let first_created = if empty.is_some() { depth + 1 } else { depth };
        for len in first_created..keys.len() {
            created.push(owned(&keys[..=len]));
        }
        Ok(())
    }

    /// Makes the existing member `key` of a mapping container equal to
    /// `value`: member by member when both are block mappings, else as a
    /// whole.
    fn update_member(
        &mut self,
        path: &PathSpec<'_>,
        member: &(Node, Node),
        key: &str,
        value: &NativeValue,
        style: Style,
    ) -> Result<(), CoreError> {
        if let (Kind::Map(old), NativeValue::Object(new)) = (&member.1.kind, value)
            && !member.1.flow
            && !old.is_empty()
            && !new.is_empty()
        {
            let locator = Locator {
                keys: owned(&path.keys),
                field: path.field.map(str::to_owned),
                key: key.to_owned(),
            };
            if self.merge_map(&locator, new, None, style)? {
                return Ok(());
            }
        }
        let span = member_span(&self.text, member);
        let column = column(&self.text, member.0.start);
        let mut text = String::new();
        emit_member(&mut text, column, key, value, style);
        if span.inline {
            text.replace_range(..column, "");
        }
        self.text.replace_range(span.start..span.end, &text);
        Ok(())
    }

    /// Makes an existing item of a `[field]` container equal to `named`
    /// (which holds `field`).
    fn update_item(
        &mut self,
        path: &PathSpec<'_>,
        item: &Node,
        named: &NativeValue,
        style: Style,
    ) -> Result<(), CoreError> {
        let field = path.field.expect("an item container");
        let key = named
            .as_object()
            .and_then(|map| map.get(field))
            .and_then(NativeValue::as_str)
            .expect("with_field sets the field");
        if let (Kind::Map(old), NativeValue::Object(new)) = (&item.kind, named)
            && !item.flow
            && !old.is_empty()
        {
            let locator = Locator {
                keys: owned(&path.keys),
                field: Some(field.to_owned()),
                key: key.to_owned(),
            };
            if self.merge_map(&locator, new, Some(field), style)? {
                return Ok(());
            }
        }
        let container = path.segments();
        let span = item_span(&self.text, item, &container)?;
        let mut text = String::new();
        emit_item(&mut text, span.dash_column, named, style);
        if span.inline {
            text.replace_range(..span.dash_column, "");
        }
        self.text.replace_range(span.start..span.end, &text);
        Ok(())
    }

    /// Updates the block mapping `locator` names to hold exactly `new`,
    /// keeping the text of members whose value stays; `keep` is never
    /// touched. Returns `false`, changing nothing, when this cannot be done
    /// member by member (no member would survive, the first member sits on
    /// a `- ` line and would go, or a key is duplicated or not a scalar).
    fn merge_map(
        &mut self,
        locator: &Locator,
        new: &NativeMap,
        keep: Option<&str>,
        style: Style,
    ) -> Result<bool, CoreError> {
        let tree = self.tree()?;
        let map = locator.locate(&tree)?;
        let Kind::Map(members) = &map.kind else {
            return Ok(false);
        };
        let mut old_keys: Vec<String> = Vec::with_capacity(members.len());
        for (name, _) in members {
            match key_of(name) {
                Some(name) if !old_keys.contains(&name) => old_keys.push(name),
                _ => return Ok(false),
            }
        }
        let survives = |name: &str| Some(name) == keep || new.contains_key(name);
        if !old_keys.iter().any(|name| survives(name)) {
            return Ok(false);
        }
        let first = member_span(&self.text, &members[0]);
        if first.inline && !survives(&old_keys[0]) {
            return Ok(false);
        }
        let column = column(&self.text, members[0].0.start);
        for (index, member) in members.iter().enumerate().rev() {
            let name = &old_keys[index];
            if Some(name.as_str()) == keep {
                continue;
            }
            let span = member_span(&self.text, member);
            match new.get(name) {
                None => self.text.replace_range(span.start..span.end, ""),
                Some(value) if to_native(&member.1, &tree.anchors) != *value => {
                    let mut text = String::new();
                    emit_member(&mut text, column, name, value, style);
                    if span.inline {
                        text.replace_range(..column, "");
                    }
                    self.text.replace_range(span.start..span.end, &text);
                }
                Some(_) => {}
            }
        }
        let mut added = String::new();
        for (name, value) in new.iter() {
            if !old_keys.iter().any(|old| old == name) && Some(name) != keep {
                emit_member(&mut added, column, name, value, style);
            }
        }
        if !added.is_empty() {
            let tree = self.tree()?;
            let map = locator.locate(&tree)?;
            let end = line_end(&self.text, map.end);
            let at = past_comments(&self.text, end, column);
            self.insert_at(at, &added);
        }
        Ok(true)
    }

    /// Replaces the member `last` of the mapping at `parent_keys` with
    /// `last: value`.
    fn replace_member(
        &mut self,
        tree: &Tree,
        parent_keys: &[&str],
        last: &str,
        value: &NativeValue,
    ) -> Result<(), CoreError> {
        let style = detect_style(tree, &self.text);
        let Found::Node { node: parent, .. } = find(tree, parent_keys)? else {
            unreachable!("the parent of an existing container exists")
        };
        let Kind::Map(members) = &parent.kind else {
            unreachable!("find walks through mappings only")
        };
        let member = members
            .iter()
            .find(|(name, _)| key_of(name).as_deref() == Some(last))
            .expect("the container exists");
        let span = member_span(&self.text, member);
        let column = column(&self.text, member.0.start);
        let mut text = String::new();
        emit_member(&mut text, column, last, value, style);
        if span.inline {
            text.replace_range(..column, "");
        }
        self.text.replace_range(span.start..span.end, &text);
        Ok(())
    }

    /// Inserts whole lines at `at`, the start of a line or the end of the
    /// text (which then first gets a line break when it has none).
    fn insert_at(&mut self, at: usize, lines: &str) {
        if at == self.text.len() && !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
            self.text.push_str(lines);
        } else {
            self.text.insert_str(at, lines);
        }
    }
}

// ---------------------------------------------------------------------------
// Paths

/// A container path split into its mapping keys and an optional `[field]`
/// last segment.
struct PathSpec<'a> {
    keys: Vec<&'a str>,
    field: Option<&'a str>,
    /// The `[field]` segment as given.
    segment: Option<&'a str>,
}

impl<'a> PathSpec<'a> {
    fn of(path: &[&'a str]) -> Self {
        let keys = super::mapping_path(path);
        let segment = path.get(keys.len()).copied();
        Self {
            keys: keys.to_vec(),
            field: segment.map(|segment| &segment[1..segment.len() - 1]),
            segment,
        }
    }

    /// The path as given.
    fn segments(&self) -> Vec<&'a str> {
        let mut segments = self.keys.clone();
        segments.extend(self.segment);
        segments
    }

    /// The entries of container `node`, by key, in document order.
    fn entries<'n>(&self, node: &'n Node) -> Result<Vec<(String, &'n Node)>, CoreError> {
        match (self.field, &node.kind) {
            (None, Kind::Map(members)) => Ok(members
                .iter()
                .filter_map(|(name, value)| key_of(name).map(|name| (name, value)))
                .collect()),
            (Some(field), Kind::Seq(items)) => Ok(items
                .iter()
                .filter_map(|item| {
                    let Kind::Map(members) = &item.kind else {
                        return None;
                    };
                    members
                        .iter()
                        .find(|(name, _)| key_of(name).as_deref() == Some(field))
                        .and_then(|(_, value)| match &value.kind {
                            Kind::Scalar { value, .. } => Some((value.clone(), item)),
                            _ => None,
                        })
                })
                .collect()),
            (None, _) => Err(unexpected_shape(Format::Yaml, &self.keys, "a mapping")),
            (Some(_), _) => Err(unexpected_shape(Format::Yaml, &self.keys, "a sequence")),
        }
    }

    /// An entry's value: an item without its `field` member.
    fn entry_value(&self, entry: &Node, anchors: &HashMap<usize, NativeValue>) -> NativeValue {
        let value = to_native(entry, anchors);
        match (self.field, value) {
            (Some(field), NativeValue::Object(mut map)) => {
                map.remove(field);
                NativeValue::Object(map)
            }
            (_, value) => value,
        }
    }
}

/// Finds the mapping an update edits again after each step: a member value
/// of a mapping container, or an item of a `[field]` container.
struct Locator {
    keys: Vec<String>,
    field: Option<String>,
    key: String,
}

impl Locator {
    fn locate<'t>(&self, tree: &'t Tree) -> Result<&'t Node, CoreError> {
        let keys: Vec<&str> = self.keys.iter().map(String::as_str).collect();
        let lost = || {
            CoreError::new(
                ErrorCode::ReadOnlyFormat,
                "the YAML entry moved during an edit",
            )
        };
        let Found::Node { node, .. } = find(tree, &keys)? else {
            return Err(lost());
        };
        let path = PathSpec {
            keys: keys.clone(),
            field: self.field.as_deref(),
            segment: None,
        };
        path.entries(node)?
            .into_iter()
            .find(|(name, _)| *name == self.key)
            .map(|(_, entry)| entry)
            .ok_or_else(lost)
    }
}

enum Found<'t> {
    /// The node at the full path, and the mappings walked through to it
    /// (the root first).
    Node {
        node: &'t Node,
        chain: Vec<&'t Node>,
    },
    /// `keys[depth]` is missing from `parent` (`None`: the root, which is
    /// absent or null), or `empty` is its member holding null or an empty
    /// collection. `chain` holds the mappings walked through.
    Missing {
        depth: usize,
        parent: Option<&'t Node>,
        empty: Option<&'t (Node, Node)>,
        chain: Vec<&'t Node>,
    },
}

fn find<'t>(tree: &'t Tree, keys: &[&str]) -> Result<Found<'t>, CoreError> {
    let Some(root) = tree.root.as_ref().filter(|root| !is_null(root)) else {
        return Ok(Found::Missing {
            depth: 0,
            parent: None,
            empty: None,
            chain: Vec::new(),
        });
    };
    let mut node = root;
    let mut chain = Vec::with_capacity(keys.len() + 1);
    for (depth, key) in keys.iter().enumerate() {
        let Kind::Map(members) = &node.kind else {
            return Err(unexpected_shape(Format::Yaml, &keys[..depth], "a mapping"));
        };
        chain.push(node);
        let member = members
            .iter()
            .find(|(name, _)| key_of(name).as_deref() == Some(*key));
        match member {
            None => {
                return Ok(Found::Missing {
                    depth,
                    parent: Some(node),
                    empty: None,
                    chain,
                });
            }
            Some(member) if is_empty_container(&member.1) => {
                return Ok(Found::Missing {
                    depth,
                    parent: Some(node),
                    empty: Some(member),
                    chain,
                });
            }
            Some((_, value)) => node = value,
        }
    }
    Ok(Found::Node { node, chain })
}

/// The container node at `path`, or `None` when it is missing or empty.
fn found_node<'t>(tree: &'t Tree, path: &PathSpec<'_>) -> Result<Option<&'t Node>, CoreError> {
    Ok(match find(tree, &path.keys)? {
        Found::Node { node, .. } => Some(node),
        Found::Missing { .. } => None,
    })
}

/// Fails with `read_only_format` when an edit at `path` could change more
/// than the entry it means to (see the module docs).
fn check_editable(
    tree: &Tree,
    chain: &[&Node],
    container: Option<&Node>,
    path: &[&str],
) -> Result<(), CoreError> {
    let read_only = |why: &str| {
        CoreError::new(
            ErrorCode::ReadOnlyFormat,
            format!(
                "{} cannot be edited safely: {why}",
                display_path(Format::Yaml, path.iter().copied())
            ),
        )
    };
    if tree.documents > 1 {
        return Err(read_only("the file holds more than one YAML document"));
    }
    for node in chain {
        if node.flow {
            return Err(read_only("a mapping on the way to it is in flow style"));
        }
        if node.anchor != 0 {
            return Err(read_only("a mapping on the way to it carries an anchor"));
        }
    }
    if let Some(container) = container {
        if container.flow {
            return Err(read_only("it is in flow style"));
        }
        if has_anchor_or_alias(container) {
            return Err(read_only("it holds anchors or aliases"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tree

#[derive(Debug)]
struct Node {
    kind: Kind,
    /// Byte offset of the node's first character (a block collection's
    /// first key or item).
    start: usize,
    /// Byte offset just past the node's last content character.
    end: usize,
    /// A flow collection (`[...]`, `{...}`).
    flow: bool,
    /// The anchor id, or 0.
    anchor: usize,
}

#[derive(Debug)]
enum Kind {
    Scalar {
        value: String,
        plain: bool,
        /// Tagged `!!str`.
        string_tag: bool,
    },
    Alias(usize),
    Seq(Vec<Node>),
    Map(Vec<(Node, Node)>),
}

struct Tree {
    /// The first document's root.
    root: Option<Node>,
    documents: usize,
    /// Anchored values by anchor id, for aliases.
    anchors: HashMap<usize, NativeValue>,
}

enum Frame {
    Seq {
        items: Vec<Node>,
        start: usize,
        flow: bool,
        anchor: usize,
    },
    Map {
        members: Vec<(Node, Node)>,
        key: Option<Node>,
        start: usize,
        flow: bool,
        anchor: usize,
    },
}

fn build(text: &str) -> Result<Tree, CoreError> {
    // Event markers count characters; the tree uses byte offsets.
    let mut offsets: Vec<usize> = text.char_indices().map(|(offset, _)| offset).collect();
    offsets.push(text.len());
    let byte = |index: usize| offsets[index.min(offsets.len() - 1)];

    let mut stack: Vec<Frame> = Vec::new();
    let mut root: Option<Node> = None;
    let mut documents = 0;
    for event in Parser::new_from_str(text) {
        let (event, span) = event
            .map_err(|err| CoreError::new(ErrorCode::ParseError, format!("invalid YAML: {err}")))?;
        let (start, end) = (byte(span.start.index()), byte(span.end.index()));
        let node = match event {
            Event::DocumentStart(_) => {
                documents += 1;
                continue;
            }
            Event::Scalar(value, style, anchor, tag) => Node {
                end: start + text[start..end].trim_end().len(),
                start,
                kind: Kind::Scalar {
                    value: value.into_owned(),
                    plain: style == ScalarStyle::Plain,
                    string_tag: tag.is_some_and(|tag| tag.suffix == "str"),
                },
                flow: false,
                anchor,
            },
            Event::Alias(id) => Node {
                kind: Kind::Alias(id),
                start,
                end,
                flow: false,
                anchor: 0,
            },
            Event::SequenceStart(anchor, _) => {
                let flow = text[start..end].ends_with('[');
                stack.push(Frame::Seq {
                    items: Vec::new(),
                    start: if flow { end - 1 } else { end },
                    flow,
                    anchor,
                });
                continue;
            }
            Event::MappingStart(anchor, _) => {
                let flow = text[start..end].ends_with('{');
                stack.push(Frame::Map {
                    members: Vec::new(),
                    key: None,
                    start: if flow { end - 1 } else { end },
                    flow,
                    anchor,
                });
                continue;
            }
            Event::SequenceEnd => {
                let Some(Frame::Seq {
                    items,
                    start: open,
                    flow,
                    anchor,
                }) = stack.pop()
                else {
                    return Err(CoreError::new(
                        ErrorCode::ParseError,
                        "invalid YAML: unbalanced sequence",
                    ));
                };
                let close = if flow {
                    end
                } else {
                    items.iter().map(|item| item.end).max().unwrap_or(open)
                };
                Node {
                    start: if flow {
                        open
                    } else {
                        items.first().map_or(open, |item| item.start)
                    },
                    end: close,
                    kind: Kind::Seq(items),
                    flow,
                    anchor,
                }
            }
            Event::MappingEnd => {
                let Some(Frame::Map {
                    members,
                    start: open,
                    flow,
                    anchor,
                    ..
                }) = stack.pop()
                else {
                    return Err(CoreError::new(
                        ErrorCode::ParseError,
                        "invalid YAML: unbalanced mapping",
                    ));
                };
                let close = if flow {
                    end
                } else {
                    members
                        .iter()
                        .map(|(key, value)| key.end.max(value.end))
                        .max()
                        .unwrap_or(open)
                };
                Node {
                    start: if flow {
                        open
                    } else {
                        members.first().map_or(open, |(key, _)| key.start)
                    },
                    end: close,
                    kind: Kind::Map(members),
                    flow,
                    anchor,
                }
            }
            Event::DocumentEnd | Event::StreamStart | Event::StreamEnd | Event::Nothing => continue,
        };
        match stack.last_mut() {
            None => {
                if documents == 1 && root.is_none() {
                    root = Some(node);
                }
            }
            Some(Frame::Seq { items, .. }) => items.push(node),
            Some(Frame::Map { members, key, .. }) => match key.take() {
                None => *key = Some(node),
                Some(name) => {
                    let mut node = node;
                    // An empty value sits right after its key.
                    if matches!(&node.kind, Kind::Scalar { value, plain: true, .. } if value.is_empty())
                    {
                        node.start = name.end;
                        node.end = name.end;
                    }
                    members.push((name, node));
                }
            },
        }
    }
    let mut anchors = HashMap::new();
    if let Some(root) = &root {
        collect_anchors(root, &mut anchors);
    }
    Ok(Tree {
        root,
        documents,
        anchors,
    })
}

fn collect_anchors(node: &Node, anchors: &mut HashMap<usize, NativeValue>) {
    match &node.kind {
        Kind::Seq(items) => items.iter().for_each(|item| collect_anchors(item, anchors)),
        Kind::Map(members) => members.iter().for_each(|(key, value)| {
            collect_anchors(key, anchors);
            collect_anchors(value, anchors);
        }),
        Kind::Scalar { .. } | Kind::Alias(_) => {}
    }
    if node.anchor != 0 {
        let value = to_native(node, anchors);
        anchors.insert(node.anchor, value);
    }
}

fn has_anchor_or_alias(node: &Node) -> bool {
    node.anchor != 0
        || match &node.kind {
            Kind::Alias(_) => true,
            Kind::Scalar { .. } => false,
            Kind::Seq(items) => items.iter().any(has_anchor_or_alias),
            Kind::Map(members) => members
                .iter()
                .any(|(key, value)| has_anchor_or_alias(key) || has_anchor_or_alias(value)),
        }
}

/// The text of a scalar key; `None` for any other key.
fn key_of(node: &Node) -> Option<String> {
    match &node.kind {
        Kind::Scalar { value, .. } => Some(value.clone()),
        _ => None,
    }
}

fn is_null(node: &Node) -> bool {
    matches!(
        &node.kind,
        Kind::Scalar { value, plain: true, string_tag: false } if resolve_plain(value) == NativeValue::Null
    )
}

/// Null, `{}` or `[]`: a container with nothing in it yet.
fn is_empty_container(node: &Node) -> bool {
    node.anchor == 0
        && match &node.kind {
            Kind::Map(members) => members.is_empty(),
            Kind::Seq(items) => items.is_empty(),
            _ => is_null(node),
        }
}

fn to_native(node: &Node, anchors: &HashMap<usize, NativeValue>) -> NativeValue {
    match &node.kind {
        Kind::Scalar {
            value,
            plain,
            string_tag,
        } => {
            if *plain && !*string_tag {
                resolve_plain(value)
            } else {
                NativeValue::String(value.clone())
            }
        }
        Kind::Alias(id) => anchors.get(id).cloned().unwrap_or(NativeValue::Null),
        Kind::Seq(items) => {
            NativeValue::Array(items.iter().map(|item| to_native(item, anchors)).collect())
        }
        Kind::Map(members) => {
            let mut map = NativeMap::new();
            for (key, value) in members {
                let name = match &key.kind {
                    Kind::Alias(id) => anchors
                        .get(id)
                        .and_then(NativeValue::as_str)
                        .map(str::to_owned),
                    _ => key_of(key),
                };
                if let Some(name) = name
                    && !map.contains_key(&name)
                {
                    map.insert(name, to_native(value, anchors));
                }
            }
            NativeValue::Object(map)
        }
    }
}

/// A plain scalar under the YAML 1.2 core schema.
fn resolve_plain(text: &str) -> NativeValue {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => return NativeValue::Null,
        "true" | "True" | "TRUE" => return NativeValue::Bool(true),
        "false" | "False" | "FALSE" => return NativeValue::Bool(false),
        _ => {}
    }
    let radix = |digits: &str, radix: u32| {
        (!digits.is_empty() && digits.chars().all(|ch| ch.is_digit(radix)))
            .then(|| i64::from_str_radix(digits, radix).ok())
            .flatten()
    };
    if let Some(value) = text
        .strip_prefix("0x")
        .and_then(|digits| radix(digits, 16))
        .or_else(|| text.strip_prefix("0o").and_then(|digits| radix(digits, 8)))
    {
        return NativeValue::Integer(value);
    }
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
    if !unsigned.is_empty()
        && unsigned.bytes().all(|byte| byte.is_ascii_digit())
        && let Ok(value) = text.parse::<i64>()
    {
        return NativeValue::Integer(value);
    }
    if is_float(unsigned)
        && let Ok(value) = text.parse::<f64>()
        && value.is_finite()
    {
        return NativeValue::Float(value);
    }
    NativeValue::String(text.to_owned())
}

/// `(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?`
fn is_float(text: &str) -> bool {
    let digits = |text: &str| text.bytes().take_while(u8::is_ascii_digit).count();
    let (mantissa, exponent) = match text.find(['e', 'E']) {
        Some(index) => (&text[..index], Some(&text[index + 1..])),
        None => (text, None),
    };
    let mantissa_ok = match mantissa.split_once('.') {
        Some((whole, fraction)) => {
            digits(whole) == whole.len()
                && digits(fraction) == fraction.len()
                && (!whole.is_empty() || !fraction.is_empty())
        }
        None => !mantissa.is_empty() && digits(mantissa) == mantissa.len(),
    };
    let exponent_ok = exponent.is_none_or(|exponent| {
        let exponent = exponent.strip_prefix(['-', '+']).unwrap_or(exponent);
        !exponent.is_empty() && digits(exponent) == exponent.len()
    });
    mantissa_ok && exponent_ok
}

// ---------------------------------------------------------------------------
// Spans

struct Span {
    start: usize,
    end: usize,
    /// The span starts after `- ` on its line rather than at the line start.
    inline: bool,
}

/// The lines of a mapping member, from its key to the end of the line its
/// value ends on.
fn member_span(text: &str, member: &(Node, Node)) -> Span {
    let key = member.0.start;
    let inline = !only_spaces_before(text, key);
    let start = if inline { key } else { line_start(text, key) };
    Span {
        start,
        end: line_end(text, member.0.end.max(member.1.end)),
        inline,
    }
}

struct ItemSpan {
    start: usize,
    end: usize,
    inline: bool,
    dash_column: usize,
}

/// The lines of a sequence item, from its `-` to the end of the line its
/// value ends on.
fn item_span(text: &str, item: &Node, path: &[&str]) -> Result<ItemSpan, CoreError> {
    let before = text[..item.start].trim_end_matches([' ', '\n']);
    if !before.ends_with('-') {
        return Err(CoreError::new(
            ErrorCode::ReadOnlyFormat,
            format!(
                "{} cannot be edited safely: an item has no `-` in front of it",
                display_path(Format::Yaml, path.iter().copied())
            ),
        ));
    }
    let dash = before.len() - 1;
    let inline = !only_spaces_before(text, dash);
    Ok(ItemSpan {
        start: if inline { dash } else { line_start(text, dash) },
        end: line_end(text, item.end.max(dash + 1)),
        inline,
        dash_column: column(text, dash),
    })
}

/// The lines `start..end` of an entry (`start` a line start, `end` past a
/// line break) widened over the comments that belong to it: comment lines
/// directly above (no blank line between) indented exactly like its first
/// line, and comment lines directly below indented deeper (commented-out
/// members). Comments indented otherwise belong to a neighbor or to the
/// file, and stay. An entry that starts after `- ` takes none.
fn with_attached_comments(text: &str, start: usize, end: usize, inline: bool) -> (usize, usize) {
    if inline {
        return (start, end);
    }
    let own = indent(&text[start..]);
    let mut start = start;
    while start > 0 {
        let above = line_start(text, start - 1);
        let line = &text[above..start];
        if is_comment(line) && indent(line) == own {
            start = above;
        } else {
            break;
        }
    }
    (start, past_comments(text, end, own))
}

/// `end` (past a line break) moved down over the comment lines directly
/// below it indented deeper than `column`: commented-out members of the
/// entry that ends there. New text goes after them, and a removed entry
/// takes them along.
fn past_comments(text: &str, end: usize, column: usize) -> usize {
    let mut end = end;
    while end < text.len() {
        let below = line_end(text, end + 1);
        let line = &text[end..below];
        if is_comment(line) && indent(line) > column {
            end = below;
        } else {
            break;
        }
    }
    end
}

fn indent(line: &str) -> usize {
    line.bytes().take_while(|byte| *byte == b' ').count()
}

fn is_comment(line: &str) -> bool {
    line.trim_start_matches(' ').starts_with('#')
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |newline| newline + 1)
}

/// The end of the line holding the character before `end`, past its line
/// break.
fn line_end(text: &str, end: usize) -> usize {
    if end > 0 && text.as_bytes()[end - 1] == b'\n' {
        return end;
    }
    text[end..]
        .find('\n')
        .map_or(text.len(), |newline| end + newline + 1)
}

fn column(text: &str, at: usize) -> usize {
    text[line_start(text, at)..at].chars().count()
}

fn only_spaces_before(text: &str, at: usize) -> bool {
    text[line_start(text, at)..at]
        .bytes()
        .all(|byte| byte == b' ')
}

// ---------------------------------------------------------------------------
// Emitting

/// The file's block layout: indentation width, and whether a sequence under
/// a key starts at the key's column (`key:` then `- x`) or deeper.
#[derive(Debug, Clone, Copy)]
struct Style {
    unit: usize,
    indentless: bool,
}

fn detect_style(tree: &Tree, text: &str) -> Style {
    fn walk(node: &Node, text: &str, unit: &mut Option<usize>, indentless: &mut Option<bool>) {
        match &node.kind {
            Kind::Map(members) => {
                for (key, value) in members {
                    let key_column = column(text, key.start);
                    match &value.kind {
                        Kind::Map(inner) if !value.flow && !inner.is_empty() => {
                            let inner_column = column(text, inner[0].0.start);
                            if inner_column > key_column {
                                let step = inner_column - key_column;
                                *unit = Some(unit.map_or(step, |unit| unit.min(step)));
                            }
                        }
                        Kind::Seq(items)
                            if !value.flow && !items.is_empty() && indentless.is_none() =>
                        {
                            let before = text[..items[0].start].trim_end_matches([' ', '\n']);
                            if before.ends_with('-') {
                                *indentless = Some(column(text, before.len() - 1) == key_column);
                            }
                        }
                        _ => {}
                    }
                    walk(value, text, unit, indentless);
                }
            }
            Kind::Seq(items) => items
                .iter()
                .for_each(|item| walk(item, text, unit, indentless)),
            Kind::Scalar { .. } | Kind::Alias(_) => {}
        }
    }
    let (mut unit, mut indentless) = (None, None);
    if let Some(root) = &tree.root {
        walk(root, text, &mut unit, &mut indentless);
    }
    Style {
        unit: unit.unwrap_or(2),
        indentless: indentless.unwrap_or(false),
    }
}

/// `key: value` at `indent`, in block style, ending in a line break.
fn emit_member(out: &mut String, indent: usize, key: &str, value: &NativeValue, style: Style) {
    out.push_str(&" ".repeat(indent));
    out.push_str(&string_text(key));
    match scalar_text(value) {
        Some(text) => {
            out.push_str(": ");
            out.push_str(&text);
            out.push('\n');
        }
        None => {
            out.push_str(":\n");
            match value {
                NativeValue::Object(map) => {
                    for (name, member) in map.iter() {
                        emit_member(out, indent + style.unit, name, member, style);
                    }
                }
                NativeValue::Array(items) => {
                    let indent = if style.indentless {
                        indent
                    } else {
                        indent + style.unit
                    };
                    for item in items {
                        emit_item(out, indent, item, style);
                    }
                }
                _ => unreachable!("scalars have a scalar text"),
            }
        }
    }
}

/// `- value` at `indent`; a mapping or sequence starts on the dash line.
fn emit_item(out: &mut String, indent: usize, value: &NativeValue, style: Style) {
    let pad = " ".repeat(indent);
    if let Some(text) = scalar_text(value) {
        out.push_str(&pad);
        out.push_str("- ");
        out.push_str(&text);
        out.push('\n');
        return;
    }
    let mut inner = String::new();
    match value {
        NativeValue::Object(map) => {
            for (name, member) in map.iter() {
                emit_member(&mut inner, indent + 2, name, member, style);
            }
        }
        NativeValue::Array(items) => {
            for item in items {
                emit_item(&mut inner, indent + 2, item, style);
            }
        }
        _ => unreachable!("scalars have a scalar text"),
    }
    out.push_str(&pad);
    out.push_str("- ");
    out.push_str(&inner[indent + 2..]);
}

/// The text of a scalar or an empty collection; `None` for a collection
/// with members.
fn scalar_text(value: &NativeValue) -> Option<String> {
    Some(match value {
        NativeValue::Null => "null".to_owned(),
        NativeValue::Bool(value) => value.to_string(),
        NativeValue::Integer(value) => value.to_string(),
        // Debug keeps a fraction (`1.0`), so the number reads back as a float.
        NativeValue::Float(value) => format!("{value:?}"),
        NativeValue::String(value) | NativeValue::Datetime(value) => string_text(value),
        NativeValue::Array(items) if items.is_empty() => "[]".to_owned(),
        NativeValue::Object(map) if map.is_empty() => "{}".to_owned(),
        NativeValue::Array(_) | NativeValue::Object(_) => return None,
    })
}

/// A string plain when that is safe, else double-quoted.
fn string_text(value: &str) -> String {
    if plain_safe(value) {
        return value.to_owned();
    }
    let mut text = String::with_capacity(value.len() + 2);
    text.push('"');
    for ch in value.chars() {
        match ch {
            '"' => text.push_str("\\\""),
            '\\' => text.push_str("\\\\"),
            '\n' => text.push_str("\\n"),
            '\r' => text.push_str("\\r"),
            '\t' => text.push_str("\\t"),
            ch if ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}' | '\u{feff}') => {
                text.push_str(&format!("\\u{:04x}", u32::from(ch)));
            }
            ch => text.push(ch),
        }
    }
    text.push('"');
    text
}

/// Whether `value` reads back as this same string when written plain, by a
/// YAML 1.2 reader and by a YAML 1.1 one (PyYAML: `yes`, `on`, `012`, ...).
fn plain_safe(value: &str) -> bool {
    let Some(first) = value.chars().next() else {
        return false;
    };
    let second = value.chars().nth(1);
    let indicator = "-?:,[]{}#&*!|>'\"%@`".contains(first);
    let dash_like = matches!(first, '-' | '?' | ':') && second.is_some_and(|ch| ch != ' ');
    let numeric_start = first.is_ascii_digit()
        || (matches!(first, '-' | '+' | '.') && second.is_some_and(|ch| ch.is_ascii_digit()));
    let yaml11_word = matches!(
        value.to_ascii_lowercase().as_str(),
        "y" | "n" | "yes" | "no" | "on" | "off" | ".inf" | "-.inf" | "+.inf" | ".nan"
    );
    !(value.starts_with([' ', '\t'])
        || value.ends_with([' ', '\t', ':'])
        || (indicator && !dash_like)
        || numeric_start
        || yaml11_word
        || value.starts_with("---")
        || value.starts_with("...")
        || value.contains(": ")
        || value.contains(" #")
        || value.chars().any(|ch| {
            ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}' | '\u{feff}' | '\u{85}')
        })
        || resolve_plain(value) != NativeValue::String(value.to_owned()))
}

/// `value` with `field: key` as its first member.
fn with_field(field: &str, key: &str, value: &NativeValue) -> NativeValue {
    let mut map = NativeMap::new();
    map.insert(field, NativeValue::from(key));
    if let Some(members) = value.as_object() {
        for (name, member) in members.iter() {
            if name != field {
                map.insert(name, member.clone());
            }
        }
    }
    NativeValue::Object(map)
}

/// YAML could hold `.inf` and `.nan`, but they would not read back as
/// floats here.
fn check_finite(value: &NativeValue) -> Result<(), CoreError> {
    match value {
        NativeValue::Float(value) if !value.is_finite() => Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("Loadout does not write {value} to YAML"),
        )),
        NativeValue::Array(items) => items.iter().try_for_each(check_finite),
        NativeValue::Object(map) => map.iter().try_for_each(|(_, value)| check_finite(value)),
        _ => Ok(()),
    }
}

fn owned(path: &[&str]) -> Vec<String> {
    path.iter().map(|key| (*key).to_owned()).collect()
}

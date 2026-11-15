//! Property tests for the format-preserving editors (SPEC 7.2, 19 item 4).
//!
//! Documents are generated from a small model in many styles (indentation,
//! comments, trailing commas, BOM, CRLF, final newline, container shapes),
//! rendered to text while remembering the exact text of every foreign entry,
//! then edited. Everything runs in memory.

use agent_loadout_core::formats::{Document, Format, NativeMap, NativeValue};
use proptest::prelude::*;
use proptest::test_runner::Config;

const BOM: &str = "\u{feff}";

/// 256 cases per property (proptest's default); `PROPTEST_CASES` overrides
/// it for longer runs.
fn config() -> Config {
    Config {
        // No regression files written into the source tree.
        failure_persistence: None,
        ..Config::default()
    }
}

// ---------------------------------------------------------------------------
// Values

/// Text with characters that need escaping or quoting in JSON and TOML.
fn text() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 _./:$@{}\\[\\],#=\\-\"'\\\\\u{e9}\u{20ac}\t]{0,12}"
}

/// Object keys, some of which need quoting in TOML.
fn key() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "[a-z][a-zA-Z0-9_]{0,7}",
        1 => Just("a.b".to_owned()),
        1 => Just("with space".to_owned()),
        1 => Just("\u{fc}ber".to_owned()),
    ]
}

/// Entry names; never `new-*`, which the tests use for added entries.
fn entry_name() -> impl Strategy<Value = String> {
    prop_oneof![
        6 => "[a-m][a-z0-9-]{0,7}",
        1 => Just("my server".to_owned()),
        1 => Just("dotted.name".to_owned()),
    ]
}

fn scalar(json: bool) -> BoxedStrategy<NativeValue> {
    let mut options = vec![
        text().prop_map(NativeValue::String).boxed(),
        any::<bool>().prop_map(NativeValue::Bool).boxed(),
        any::<i64>().prop_map(NativeValue::Integer).boxed(),
        (-1.0e9..1.0e9f64).prop_map(NativeValue::Float).boxed(),
    ];
    if json {
        options.push(Just(NativeValue::Null).boxed());
    }
    proptest::strategy::Union::new(options).boxed()
}

fn value(json: bool) -> impl Strategy<Value = NativeValue> {
    scalar(json).prop_recursive(2, 12, 4, move |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(NativeValue::Array),
            prop::collection::vec((key(), inner), 0..4)
                .prop_map(|members| NativeValue::Object(members.into_iter().collect())),
        ]
    })
}

/// A server-like entry: an object with one to five members.
fn entry(json: bool) -> impl Strategy<Value = NativeMap> {
    prop::collection::vec((key(), value(json)), 1..6)
        .prop_map(|members| members.into_iter().collect())
}

fn unique_entries(json: bool, min: usize) -> impl Strategy<Value = Vec<(String, NativeMap)>> {
    prop::collection::vec((entry_name(), entry(json)), min..=4).prop_map(|entries| {
        let mut seen = Vec::<String>::new();
        entries
            .into_iter()
            .filter(|(name, _)| {
                let fresh = !seen.contains(name);
                seen.push(name.clone());
                fresh
            })
            .collect()
    })
}

// ---------------------------------------------------------------------------
// Byte style

#[derive(Debug, Clone, Copy)]
struct Style {
    bom: bool,
    crlf: bool,
    final_newline: bool,
}

fn style() -> impl Strategy<Value = Style> {
    (any::<bool>(), any::<bool>(), prop::bool::weighted(0.8)).prop_map(
        |(bom, crlf, final_newline)| Style {
            bom,
            crlf,
            final_newline,
        },
    )
}

/// LF text as the bytes of a file in `style`, and the style an editor can
/// detect in them: a file without any line break has no line ending to keep,
/// so new lines get `\n`, and an empty file gets a final newline like any
/// new file.
fn apply_style(lf: &str, style: Style) -> (Vec<u8>, Style) {
    let mut text = lf.trim_end_matches('\n').to_owned();
    if style.final_newline {
        text.push('\n');
    }
    let detectable = Style {
        crlf: style.crlf && text.contains('\n'),
        final_newline: style.final_newline || text.is_empty(),
        ..style
    };
    if detectable.crlf {
        text = text.replace('\n', "\r\n");
    }
    if style.bom {
        text.insert_str(0, BOM);
    }
    (text.into_bytes(), detectable)
}

/// Checks that `bytes` has `style` and returns it as LF text.
fn check_style(bytes: &[u8], style: Style) -> Result<String, TestCaseError> {
    let text = std::str::from_utf8(bytes).map_err(|err| TestCaseError::fail(err.to_string()))?;
    prop_assert_eq!(text.starts_with(BOM), style.bom, "BOM kept");
    let text = text.strip_prefix(BOM).unwrap_or(text);
    if !text.is_empty() {
        prop_assert_eq!(
            text.ends_with('\n'),
            style.final_newline,
            "final newline kept"
        );
    }
    let breaks = text.matches('\n').count();
    let crlfs = text.matches("\r\n").count();
    if style.crlf {
        prop_assert_eq!(crlfs, breaks, "every line break is CRLF");
    } else {
        prop_assert_eq!(crlfs, 0, "no CRLF in an LF file");
    }
    Ok(text.replace("\r\n", "\n"))
}

// ---------------------------------------------------------------------------
// JSON and JSONC documents

#[derive(Debug, Clone)]
struct JsonDoc {
    format: Format,
    indent: &'static str,
    /// The whole file on one line (`{"a":1,"b":{}}`), as `JSON.stringify`
    /// writes it.
    one_line: bool,
    /// Spacing inside one-line objects and arrays.
    colon: &'static str,
    comma: &'static str,
    trailing_commas: bool,
    comments: bool,
    /// `None`: no container; `Some(true)`: `"mcpServers": {}`.
    container: Option<bool>,
    entries: Vec<(String, NativeMap, bool)>,
    before: Vec<(String, NativeValue)>,
    after: Vec<(String, NativeValue)>,
    list: Option<(Vec<String>, bool)>,
    style: Style,
}

const JSON_CONTAINER: &[&str] = &["mcpServers"];
const JSON_LIST: &[&str] = &["disabledServers"];

/// Documents with at least `min_entries` entries in `mcpServers`.
fn json_doc(min_entries: usize) -> impl Strategy<Value = JsonDoc> {
    let format = prop_oneof![Just(Format::Json), Just(Format::Jsonc)];
    let indent = prop_oneof![Just("  "), Just("    "), Just("\t")];
    let others = || {
        prop::collection::vec(("[n-z][a-z]{0,6}", value(true)), 0..3).prop_map(|members| {
            let map: NativeMap = members.into_iter().collect();
            map.into_iter()
                .filter(|(name, _)| name != "mcpServers" && name != "disabledServers")
                .collect::<Vec<_>>()
        })
    };
    let entries = unique_entries(true, min_entries).prop_flat_map(|entries| {
        let n = entries.len();
        (
            Just(entries),
            prop::collection::vec(prop::bool::weighted(0.3), n),
        )
            .prop_map(|(entries, compact)| {
                entries
                    .into_iter()
                    .zip(compact)
                    .map(|((name, value), compact)| (name, value, compact))
                    .collect::<Vec<_>>()
            })
    });
    let list = proptest::option::of((
        prop::collection::vec("[a-m][a-z]{0,5}", 0..4),
        any::<bool>(),
    ));
    let layout = (
        prop::bool::weighted(0.25),
        prop_oneof![Just(": "), Just(":")],
        prop_oneof![Just(", "), Just(",")],
    );
    (
        (format, indent, layout),
        any::<bool>(),
        any::<bool>(),
        proptest::option::of(any::<bool>()).prop_map(move |container| {
            // At least one entry: the container is there and not `{}`.
            if min_entries > 0 {
                Some(false)
            } else {
                container
            }
        }),
        entries,
        (others(), others()),
        list,
        style(),
    )
        .prop_map(
            |(
                (format, indent, (one_line, colon, comma)),
                trailing,
                comments,
                container,
                entries,
                (before, after),
                list,
                style,
            )| {
                let jsonc = format == Format::Jsonc;
                JsonDoc {
                    format,
                    indent,
                    one_line,
                    colon,
                    comma,
                    trailing_commas: jsonc && trailing && !one_line,
                    comments: jsonc && comments && !one_line,
                    container: container.map(|empty| empty || entries.is_empty()),
                    entries,
                    before,
                    after,
                    list,
                    style,
                }
            },
        )
}

struct Rendered {
    lf: String,
    /// Exact text of every foreign entry or top-level key, by name.
    segments: Vec<(String, String)>,
}

fn json_scalar(value: &NativeValue) -> String {
    match value {
        NativeValue::Null => "null".to_owned(),
        NativeValue::Bool(value) => value.to_string(),
        NativeValue::Integer(value) => value.to_string(),
        NativeValue::Float(value) => format!("{value:?}"),
        NativeValue::String(value) | NativeValue::Datetime(value) => {
            serde_json::to_string(value).unwrap()
        }
        NativeValue::Array(_) | NativeValue::Object(_) => unreachable!("not a scalar"),
    }
}

fn json_key(key: &str) -> String {
    serde_json::to_string(key).unwrap()
}

/// `value` as JSON: objects with members on their own lines when
/// `multiline` (never in a one-line file), arrays always on one line.
fn json_value(value: &NativeValue, doc: &JsonDoc, depth: usize, multiline: bool) -> String {
    let pad = |depth: usize| doc.indent.repeat(depth);
    match value {
        NativeValue::Array(items) => {
            let items: Vec<String> = items
                .iter()
                .map(|item| json_value(item, doc, depth, false))
                .collect();
            format!("[{}]", items.join(doc.comma))
        }
        NativeValue::Object(map) if map.is_empty() => "{}".to_owned(),
        NativeValue::Object(map) if multiline && !doc.one_line => {
            let members: Vec<String> = map
                .iter()
                .map(|(key, value)| {
                    format!(
                        "{}{}: {}",
                        pad(depth + 1),
                        json_key(key),
                        json_value(value, doc, depth + 1, true)
                    )
                })
                .collect();
            let trailing = if doc.trailing_commas { "," } else { "" };
            format!("{{\n{}{trailing}\n{}}}", members.join(",\n"), pad(depth))
        }
        NativeValue::Object(map) => {
            let members: Vec<String> = map
                .iter()
                .map(|(key, value)| {
                    format!(
                        "{}{}{}",
                        json_key(key),
                        doc.colon,
                        json_value(value, doc, depth, false)
                    )
                })
                .collect();
            format!("{{{}}}", members.join(doc.comma))
        }
        scalar => json_scalar(scalar),
    }
}

fn render_json(doc: &JsonDoc) -> Rendered {
    let pad = |depth: usize| {
        if doc.one_line {
            String::new()
        } else {
            doc.indent.repeat(depth)
        }
    };
    let colon = if doc.one_line { doc.colon } else { ": " };
    let member = |name: &str, value: String| format!("{}{colon}{value}", json_key(name));
    let mut segments = Vec::new();
    let mut props: Vec<String> = Vec::new();
    let top = |name: &str,
               text: String,
               segments: &mut Vec<(String, String)>,
               props: &mut Vec<String>| {
        segments.push((name.to_owned(), text.clone()));
        props.push(format!("{}{text}", pad(1)));
    };
    for (name, value) in &doc.before {
        let text = member(name, json_value(value, doc, 1, true));
        top(name, text, &mut segments, &mut props);
    }
    if let Some((members, multiline)) = &doc.list {
        let list = NativeValue::Array(
            members
                .iter()
                .map(|m| NativeValue::from(m.as_str()))
                .collect(),
        );
        let text = if *multiline && !members.is_empty() && !doc.one_line {
            let items: Vec<String> = members
                .iter()
                .map(|m| format!("{}{}", pad(2), json_key(m)))
                .collect();
            let trailing = if doc.trailing_commas { "," } else { "" };
            member(
                JSON_LIST[0],
                format!("[\n{}{trailing}\n{}]", items.join(",\n"), pad(1)),
            )
        } else {
            member(JSON_LIST[0], json_value(&list, doc, 1, false))
        };
        top(JSON_LIST[0], text, &mut segments, &mut props);
    }
    match doc.container {
        None => {}
        Some(true) => top(
            "mcpServers",
            member("mcpServers", "{}".to_owned()),
            &mut segments,
            &mut props,
        ),
        Some(false) => {
            let mut entries = Vec::new();
            for (name, value, compact) in &doc.entries {
                let text = member(
                    name,
                    json_value(&NativeValue::Object(value.clone()), doc, 2, !compact),
                );
                segments.push((name.clone(), text.clone()));
                let comment = if doc.comments {
                    format!("{}// about {name}\n", pad(2))
                } else {
                    String::new()
                };
                entries.push(format!("{comment}{}{text}", pad(2)));
            }
            let container = if doc.one_line {
                format!("{{{}}}", entries.join(doc.comma))
            } else {
                let trailing = if doc.trailing_commas { "," } else { "" };
                format!("{{\n{}{trailing}\n{}}}", entries.join(",\n"), pad(1))
            };
            props.push(format!("{}{}", pad(1), member("mcpServers", container)));
        }
    }
    for (name, value) in &doc.after {
        let text = member(name, json_value(value, doc, 1, true));
        top(name, text, &mut segments, &mut props);
    }
    let mut lf = String::new();
    if doc.comments {
        lf.push_str("// generated test config\n");
    }
    if props.is_empty() {
        lf.push_str("{}\n");
    } else if doc.one_line {
        lf.push_str(&format!("{{{}}}\n", props.join(doc.comma)));
    } else {
        let trailing = if doc.trailing_commas { "," } else { "" };
        lf.push_str(&format!("{{\n{}{trailing}\n}}\n", props.join(",\n")));
    }
    Rendered { lf, segments }
}

/// The output must still be valid in the document's format; strict JSON
/// never gains comments or trailing commas, and JSONC gains neither unless
/// it already used them. A one-line file stays on one line (a bare `{}`
/// is laid out like a new file).
fn check_json_syntax(doc: &JsonDoc, out: &[u8], lf: &str) -> Result<(), TestCaseError> {
    let reparsed = Document::parse(out, doc.format);
    let bare = doc.before.is_empty()
        && doc.after.is_empty()
        && doc.list.is_none()
        && doc.container.is_none();
    if doc.one_line && !bare {
        prop_assert!(
            !lf.trim_end_matches('\n').contains('\n'),
            "one line:\n{}",
            lf
        );
    }
    prop_assert!(
        reparsed.is_ok(),
        "output parses: {:?}\n{lf}",
        reparsed.err()
    );
    if !doc.trailing_commas {
        // The generator writes comments only as whole lines.
        let uncommented: String = lf
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .map(|line| format!("{line}\n"))
            .collect();
        let strict = Document::parse(uncommented.as_bytes(), Format::Json);
        prop_assert!(
            strict.is_ok(),
            "no trailing commas or inline comments: {:?}\n{lf}",
            strict.err()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// TOML documents

#[derive(Debug, Clone, Copy, PartialEq)]
enum TomlContainer {
    Missing,
    /// `[mcp_servers.<name>]` tables, with or without a `[mcp_servers]` header.
    Tables {
        header: bool,
    },
    /// `[mcp_servers]` holding `name = { ... }` lines.
    Inline,
}

#[derive(Debug, Clone)]
struct TomlDoc {
    indent: &'static str,
    comments: bool,
    root: Vec<(String, NativeValue)>,
    container: TomlContainer,
    entries: Vec<(String, NativeMap)>,
    legacy: Option<NativeMap>,
    others: Vec<(String, NativeMap)>,
    /// Order in which the table blocks are written.
    shuffle: Vec<usize>,
    style: Style,
}

const TOML_CONTAINER: &[&str] = &["mcp_servers"];

/// Documents with at least `min_entries` entries in `mcp_servers`.
fn toml_doc(min_entries: usize) -> impl Strategy<Value = TomlDoc> {
    let mut containers = vec![
        Just(TomlContainer::Tables { header: false }),
        Just(TomlContainer::Tables { header: true }),
        Just(TomlContainer::Inline),
    ];
    if min_entries == 0 {
        containers.push(Just(TomlContainer::Missing));
    }
    let container = proptest::strategy::Union::new(containers);
    let root =
        prop::collection::vec(("[n-z][a-z_]{0,6}", value(false)), 0..3).prop_map(|members| {
            let map: NativeMap = members.into_iter().collect();
            map.into_iter()
                .filter(|(name, value)| {
                    // Inline tables are covered by entry values.
                    name != "mcp_servers"
                        && name != "mcp"
                        && !matches!(value, NativeValue::Object(_))
                })
                .collect::<Vec<_>>()
        });
    (
        prop_oneof![Just(""), Just("  ")],
        any::<bool>(),
        root,
        container,
        unique_entries(false, min_entries),
        proptest::option::of(entry(false)),
        prop::collection::vec(("[n-z][a-z]{0,5}", entry(false)), 0..3),
        style(),
    )
        .prop_flat_map(
            |(indent, comments, root, container, entries, legacy, others, style)| {
                let entries = if container == TomlContainer::Missing {
                    Vec::new()
                } else {
                    entries
                };
                let mut seen = Vec::new();
                let others: Vec<_> = others
                    .into_iter()
                    .filter(|(name, _)| {
                        let fresh = !seen.contains(name);
                        seen.push(name.clone());
                        fresh
                    })
                    .collect();
                let blocks = entries.len() + others.len() + 2;
                let shuffle = Just((0..blocks).collect::<Vec<_>>()).prop_shuffle();
                (
                    Just(indent),
                    Just(comments),
                    Just(root),
                    Just(container),
                    Just(entries),
                    Just(legacy),
                    Just(others),
                    shuffle,
                    Just(style),
                )
            },
        )
        .prop_map(
            |(indent, comments, root, container, entries, legacy, others, shuffle, style)| {
                TomlDoc {
                    indent,
                    comments,
                    root,
                    container,
                    entries,
                    legacy,
                    others,
                    shuffle,
                    style,
                }
            },
        )
}

fn toml_key(key: &str) -> String {
    toml_edit::Key::new(key).display_repr().into_owned()
}

fn to_toml(value: &NativeValue) -> toml::Value {
    match value {
        NativeValue::Bool(value) => toml::Value::Boolean(*value),
        NativeValue::Integer(value) => toml::Value::Integer(*value),
        NativeValue::Float(value) => toml::Value::Float(*value),
        NativeValue::String(value) => toml::Value::String(value.clone()),
        NativeValue::Array(items) => toml::Value::Array(items.iter().map(to_toml).collect()),
        NativeValue::Object(map) => toml::Value::Table(
            map.iter()
                .map(|(key, value)| (key.to_owned(), to_toml(value)))
                .collect(),
        ),
        NativeValue::Null | NativeValue::Datetime(_) => unreachable!("not generated for TOML"),
    }
}

fn toml_value(value: &NativeValue) -> String {
    to_toml(value).to_string()
}

/// A table block: header and `key = value` lines, each key with a trailing
/// comment and a comment line above it when `comments`.
fn toml_table(doc: &TomlDoc, header: &str, members: &NativeMap) -> String {
    let mut text = format!("[{header}]\n");
    for (key, value) in members.iter() {
        if doc.comments {
            text.push_str(&format!("{}# above {key}\n", doc.indent));
        }
        let comment = if doc.comments {
            format!(" # on {key}")
        } else {
            String::new()
        };
        text.push_str(&format!(
            "{}{} = {}{comment}\n",
            doc.indent,
            toml_key(key),
            toml_value(value)
        ));
    }
    text
}

fn render_toml(doc: &TomlDoc) -> Rendered {
    let mut segments = Vec::new();
    let mut head = String::new();
    if doc.comments {
        head.push_str("# generated test config\n");
    }
    for (key, value) in &doc.root {
        let text = format!("{} = {}", toml_key(key), toml_value(value));
        head.push_str(&format!("{text}\n"));
        segments.push((key.clone(), text));
    }

    let mut blocks: Vec<Option<String>> = Vec::new();
    // Block 0: the `[mcp_servers]` header (with inline entries).
    blocks.push(match doc.container {
        TomlContainer::Tables { header: true } => Some("[mcp_servers]\n".to_owned()),
        TomlContainer::Inline => {
            let mut text = "[mcp_servers]\n".to_owned();
            for (name, value) in &doc.entries {
                let line = format!(
                    "{}{} = {}",
                    doc.indent,
                    toml_key(name),
                    toml_value(&NativeValue::Object(value.clone()))
                );
                segments.push((name.clone(), line.clone()));
                text.push_str(&format!("{line}\n"));
            }
            Some(text)
        }
        _ => None,
    });
    // Block 1: the legacy `[mcp.servers]` table.
    blocks.push(doc.legacy.as_ref().map(|legacy| {
        let text = toml_table(doc, "mcp.servers.legacy", legacy);
        segments.push(("legacy".to_owned(), text.clone()));
        text
    }));
    for (name, value) in &doc.entries {
        blocks.push(match doc.container {
            TomlContainer::Tables { .. } => {
                let text = toml_table(doc, &format!("mcp_servers.{}", toml_key(name)), value);
                segments.push((name.clone(), text.clone()));
                Some(text)
            }
            _ => None,
        });
    }
    for (name, value) in &doc.others {
        let text = toml_table(doc, &format!("profiles.{}", toml_key(name)), value);
        segments.push((format!("profiles.{name}"), text.clone()));
        blocks.push(Some(text));
    }

    let mut parts: Vec<String> = Vec::new();
    if !head.is_empty() {
        parts.push(head);
    }
    for index in &doc.shuffle {
        if let Some(block) = &blocks[*index] {
            parts.push(block.clone());
        }
    }
    Rendered {
        lf: parts.join("\n"),
        segments,
    }
}

// ---------------------------------------------------------------------------
// Shared checks

fn others_intact(rendered: &Rendered, except: &str, lf: &str) -> Result<(), TestCaseError> {
    for (name, segment) in &rendered.segments {
        // The last line of a file without a final newline has no `\n`.
        let segment = segment.trim_end_matches('\n');
        if name != except {
            prop_assert!(
                lf.contains(segment),
                "entry {name:?} is byte-identical\n--- expected\n{segment}\n--- in\n{lf}"
            );
        }
    }
    Ok(())
}

fn object(map: &NativeMap) -> NativeValue {
    NativeValue::Object(map.clone())
}

/// A different value for an existing entry: one member changed, one added,
/// the rest kept.
fn modified(map: &NativeMap, replacement: NativeValue) -> NativeMap {
    let mut map = map.clone();
    let first = map.keys().next().map(str::to_owned);
    if let Some(first) = first {
        map.insert(first, replacement.clone());
    }
    map.insert("added_by_test", replacement);
    map
}

/// Entry names in the order the shuffled blocks put them in the document.
fn toml_order(doc: &TomlDoc, rendered: &Rendered) -> Vec<String> {
    let mut names: Vec<(usize, String)> = doc
        .entries
        .iter()
        .map(|(name, _)| {
            let segment = &rendered
                .segments
                .iter()
                .find(|(other, _)| other == name)
                .unwrap()
                .1;
            (rendered.lf.find(segment.as_str()).unwrap(), name.clone())
        })
        .collect();
    names.sort();
    names.into_iter().map(|(_, name)| name).collect()
}

// ---------------------------------------------------------------------------
// JSON and JSONC properties

proptest! {
    #![proptest_config(config())]

    #[test]
    fn json_no_op_edits_are_byte_identical(doc in json_doc(0)) {
        let rendered = render_json(&doc);
        let (input, _) = apply_style(&rendered.lf, doc.style);
        let mut parsed = Document::parse(&input, doc.format).unwrap();
        prop_assert_eq!(&parsed.to_bytes(), &input);

        let names = parsed.list(JSON_CONTAINER).unwrap();
        let expected: Vec<String> = if doc.container == Some(false) {
            doc.entries.iter().map(|(name, _, _)| name.clone()).collect()
        } else {
            Vec::new()
        };
        prop_assert_eq!(&names, &expected);
        for (name, value, _) in &doc.entries {
            let current = parsed.get(JSON_CONTAINER, name).unwrap();
            if doc.container == Some(false) {
                prop_assert_eq!(current.as_ref(), Some(&object(value)));
                prop_assert!(!parsed.upsert(JSON_CONTAINER, name, &object(value)).unwrap());
            }
        }
        prop_assert!(!parsed.remove(JSON_CONTAINER, "new-absent").unwrap());
        prop_assert!(!parsed.set_list_member(JSON_LIST, "new-absent", false).unwrap());
        if let Some((members, _)) = &doc.list {
            for member in members {
                prop_assert!(!parsed.set_list_member(JSON_LIST, member, true).unwrap());
            }
        }
        prop_assert_eq!(&parsed.to_bytes(), &input);
    }

    #[test]
    fn json_upsert_then_remove_restores_the_original(
        doc in json_doc(0),
        value in entry(true),
        member in "new-[a-z]{1,5}",
    ) {
        let rendered = render_json(&doc);
        let (input, style) = apply_style(&rendered.lf, doc.style);
        let mut parsed = Document::parse(&input, doc.format).unwrap();

        prop_assert!(parsed.upsert(JSON_CONTAINER, "new-entry", &object(&value)).unwrap());
        let added = parsed.to_bytes();
        let lf = check_style(&added, style)?;
        check_json_syntax(&doc, &added, &lf)?;
        // The empty `"mcpServers": {}` is the one container that changes.
        others_intact(&rendered, "mcpServers", &lf)?;
        prop_assert_eq!(parsed.get(JSON_CONTAINER, "new-entry").unwrap(), Some(object(&value)));
        prop_assert_eq!(parsed.list(JSON_CONTAINER).unwrap().last().cloned(), Some("new-entry".to_owned()));

        prop_assert!(parsed.remove(JSON_CONTAINER, "new-entry").unwrap());
        prop_assert_eq!(&parsed.to_bytes(), &input, "\n{}", lf);

        prop_assert!(parsed.set_list_member(JSON_LIST, &member, true).unwrap());
        let listed = parsed.to_bytes();
        let lf = check_style(&listed, style)?;
        check_json_syntax(&doc, &listed, &lf)?;
        prop_assert!(parsed.set_list_member(JSON_LIST, &member, false).unwrap());
        prop_assert_eq!(&parsed.to_bytes(), &input, "\n{}", lf);
    }

    #[test]
    fn json_foreign_entries_survive_any_operation(
        doc in json_doc(1),
        target in any::<prop::sample::Index>(),
        replacement in value(true),
        op in 0..4u8,
    ) {
        let rendered = render_json(&doc);
        let (input, style) = apply_style(&rendered.lf, doc.style);
        let mut parsed = Document::parse(&input, doc.format).unwrap();
        let (name, current, _) = &doc.entries[target.index(doc.entries.len())];

        let expected = match op {
            0 => Some(modified(current, replacement.clone())),
            1 => Some([("only", replacement.clone())].into_iter().collect()),
            2 => None,
            _ => {
                // A whole entry replaced by a non-object value.
                prop_assert!(parsed.upsert(JSON_CONTAINER, name, &replacement).unwrap());
                let lf = check_style(&parsed.to_bytes(), style)?;
                others_intact(&rendered, name, &lf)?;
                prop_assert_eq!(parsed.get(JSON_CONTAINER, name).unwrap(), Some(replacement));
                return Ok(());
            }
        };
        match &expected {
            Some(map) => prop_assert!(parsed.upsert(JSON_CONTAINER, name, &object(map)).unwrap()),
            None => prop_assert!(parsed.remove(JSON_CONTAINER, name).unwrap()),
        }
        let out = parsed.to_bytes();
        let lf = check_style(&out, style)?;
        if doc.comments {
            prop_assert!(lf.starts_with("// generated test config"), "file comment kept:\n{}", lf);
        }
        check_json_syntax(&doc, &out, &lf)?;
        others_intact(&rendered, name, &lf)?;
        prop_assert_eq!(parsed.get(JSON_CONTAINER, name).unwrap(), expected.as_ref().map(object));

        // Comments above other entries stay; the one above a removed entry
        // goes with it unless it was the only entry.
        if doc.comments {
            for (other, _, _) in &doc.entries {
                let comment = format!("// about {other}\n");
                let gone = other == name && expected.is_none() && doc.entries.len() > 1;
                prop_assert_eq!(lf.contains(&comment), !gone, "{:?}:\n{}", comment, lf);
            }
        }

        // Order: an updated entry keeps its place, a removed one is gone.
        let mut names: Vec<String> = doc.entries.iter().map(|(name, _, _)| name.clone()).collect();
        if expected.is_none() {
            names.retain(|other| other != name);
        }
        prop_assert_eq!(parsed.list(JSON_CONTAINER).unwrap(), names);

        // New lines use the document's indentation.
        if let Some(map) = &expected {
            let compact = doc.entries[target.index(doc.entries.len())].2;
            if !compact && !doc.one_line {
                let added = format!("\n{}{}", doc.indent.repeat(3), "\"added_by_test\"");
                if map.contains_key("added_by_test") {
                    prop_assert!(lf.contains(&added), "indented with {:?}:\n{}", doc.indent, lf);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TOML properties

proptest! {
    #![proptest_config(config())]

    #[test]
    fn toml_no_op_edits_are_byte_identical(doc in toml_doc(0)) {
        let rendered = render_toml(&doc);
        let (input, _) = apply_style(&rendered.lf, doc.style);
        let mut parsed = Document::parse(&input, Format::Toml).unwrap();
        prop_assert_eq!(&parsed.to_bytes(), &input);

        let expected = toml_order(&doc, &rendered);
        prop_assert_eq!(parsed.list(TOML_CONTAINER).unwrap(), expected);
        for (name, value) in &doc.entries {
            prop_assert_eq!(parsed.get(TOML_CONTAINER, name).unwrap(), Some(object(value)));
            prop_assert!(!parsed.upsert(TOML_CONTAINER, name, &object(value)).unwrap());
        }
        prop_assert!(!parsed.remove(TOML_CONTAINER, "new-absent").unwrap());
        prop_assert_eq!(&parsed.to_bytes(), &input);
    }

    #[test]
    fn toml_upsert_then_remove_restores_the_original(
        doc in toml_doc(0),
        value in entry(false),
        member in "new-[a-z]{1,5}",
    ) {
        let rendered = render_toml(&doc);
        let (input, style) = apply_style(&rendered.lf, doc.style);
        let mut parsed = Document::parse(&input, Format::Toml).unwrap();

        prop_assert!(parsed.upsert(TOML_CONTAINER, "new-entry", &object(&value)).unwrap());
        let added = parsed.to_bytes();
        let lf = check_style(&added, style)?;
        prop_assert!(Document::parse(&added, Format::Toml).is_ok(), "output parses:\n{}", lf);
        others_intact(&rendered, "", &lf)?;
        prop_assert_eq!(parsed.get(TOML_CONTAINER, "new-entry").unwrap(), Some(object(&value)));
        prop_assert_eq!(parsed.list(TOML_CONTAINER).unwrap().last().cloned(), Some("new-entry".to_owned()));

        prop_assert!(parsed.remove(TOML_CONTAINER, "new-entry").unwrap());
        prop_assert_eq!(&parsed.to_bytes(), &input, "\n{}", lf);

        let list = ["profiles", "new-list", "disabled"];
        prop_assert!(parsed.set_list_member(&list, &member, true).unwrap());
        prop_assert!(parsed.set_list_member(&list, &member, false).unwrap());
        prop_assert_eq!(&parsed.to_bytes(), &input);
    }

    #[test]
    fn toml_foreign_entries_and_comments_survive_any_operation(
        doc in toml_doc(1),
        target in any::<prop::sample::Index>(),
        replacement in value(false),
        op in 0..3u8,
    ) {
        let rendered = render_toml(&doc);
        let (input, style) = apply_style(&rendered.lf, doc.style);
        let mut parsed = Document::parse(&input, Format::Toml).unwrap();
        let (name, current) = &doc.entries[target.index(doc.entries.len())];

        let expected = match op {
            0 => Some(modified(current, replacement.clone())),
            1 => Some([("only", replacement.clone())].into_iter().collect()),
            _ => None,
        };
        match &expected {
            Some(map) => prop_assert!(parsed.upsert(TOML_CONTAINER, name, &object(map)).unwrap()),
            None => prop_assert!(parsed.remove(TOML_CONTAINER, name).unwrap()),
        }
        let out = parsed.to_bytes();
        let lf = check_style(&out, style)?;
        // A file comment separated from the first table by a blank line
        // stays, even when that table is the one removed.
        if doc.comments {
            prop_assert!(lf.starts_with("# generated test config"), "file comment kept:\n{}", lf);
        }
        prop_assert!(Document::parse(&out, Format::Toml).is_ok(), "output parses:\n{}", lf);
        // The legacy `[mcp.servers]` table is among the foreign segments.
        others_intact(&rendered, name, &lf)?;
        prop_assert_eq!(parsed.get(TOML_CONTAINER, name).unwrap(), expected.as_ref().map(object));

        let mut names = toml_order(&doc, &rendered);
        if expected.is_none() {
            names.retain(|other| other != name);
        }
        prop_assert_eq!(parsed.list(TOML_CONTAINER).unwrap(), names);

        // An update inside an existing table keeps the comments of every
        // member it keeps, and new keys get the table's indentation.
        if op == 0 && doc.comments && matches!(doc.container, TomlContainer::Tables { .. }) {
            for key in current.keys() {
                let above = format!("{}# above {key}\n", doc.indent);
                let on = format!(" # on {key}\n");
                prop_assert!(lf.contains(&above) && lf.contains(&on), "comments of {key:?} kept:\n{lf}");
            }
            prop_assert!(lf.contains(&format!("\n{}added_by_test = ", doc.indent)), "indented:\n{}", lf);
        }
    }
}

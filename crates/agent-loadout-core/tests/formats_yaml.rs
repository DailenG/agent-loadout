//! The YAML and JSON5 editors (SPEC 7.2, M22): YAML span splice keeps
//! every byte outside the edited entry, anchors, aliases, flow-style
//! containers and multi-document files make edits `read_only_format`, a
//! `[field]` container edits a list matched by name (Continue), and JSON5
//! files are written only when they also parse as JSONC. The common rules
//! (BOM, line endings, final newline, byte-identical no-ops) are checked
//! here and property-tested at the end.

use agent_loadout_core::ErrorCode;
use agent_loadout_core::formats::{Document, Format, NativeMap, NativeValue};
use proptest::prelude::*;
use proptest::test_runner::Config;

fn fixture(relative: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("formats")
        .join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
        .replace("\r\n", "\n")
}

fn yaml(text: &str) -> Document {
    Document::parse(text.as_bytes(), Format::Yaml).unwrap()
}

fn text(doc: &Document) -> String {
    String::from_utf8(doc.to_bytes()).unwrap()
}

fn json(text: &str) -> NativeValue {
    NativeValue::parse_json(text).unwrap()
}

/// The changed lines only: `-` removed, `+` added, with the line number in
/// the old text (`@N`) at the start of each run.
fn line_diff(before: &str, after: &str) -> String {
    let diff = similar::TextDiff::from_lines(before, after);
    let mut out = String::new();
    let mut in_run = false;
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            similar::ChangeTag::Equal => {
                in_run = false;
                continue;
            }
            similar::ChangeTag::Delete => '-',
            similar::ChangeTag::Insert => '+',
        };
        if !in_run {
            let line = change.old_index().or(change.new_index()).unwrap_or(0) + 1;
            out.push_str(&format!("@{line}\n"));
            in_run = true;
        }
        out.push(sign);
        out.push_str(&change.to_string_lossy());
    }
    out
}

const EXTENSIONS: &[&str] = &["extensions"];

// ---------------------------------------------------------------------------
// Reading

#[test]
fn yaml_reads_core_schema_values_and_resolves_aliases() {
    let doc = yaml(&fixture("yaml/goose-config.yaml"));
    assert_eq!(
        doc.list(EXTENSIONS).unwrap(),
        ["developer", "files", "search"]
    );
    assert_eq!(
        doc.get(EXTENSIONS, "files").unwrap().unwrap(),
        json(
            r#"{"args": ["-y", "files-mcp"], "bundled": null, "cmd": "npx",
                "description": "Local files", "enabled": true, "env_keys": [],
                "envs": {"ROOT": "~/work"}, "name": "files", "timeout": 300, "type": "stdio"}"#
        )
    );
    assert_eq!(
        doc.get(EXTENSIONS, "search").unwrap().unwrap(),
        json(
            r#"{"enabled": false, "name": "search", "type": "streamable_http",
                "uri": "https://search.example.com/mcp",
                "headers": {"Authorization": "Bearer ${SEARCH_TOKEN}"}, "timeout": 300}"#
        )
    );
    assert_eq!(doc.get(EXTENSIONS, "missing").unwrap(), None);
    assert_eq!(
        doc.list(&["nothing", "here"]).unwrap(),
        Vec::<String>::new()
    );

    let doc = yaml(&fixture("yaml/anchors.yaml"));
    let files = doc.get(&["mcp_servers"], "files").unwrap().unwrap();
    assert_eq!(
        files.as_object().unwrap().get("<<"),
        Some(&json(r#"{"timeout": 300, "enabled": true}"#))
    );

    let doc = yaml(
        "a: ~\nb: 0x1F\nc: 0o17\nd: -12\ne: 1.5e3\nf: .inf\ng: 'yes'\nh: yes\ni: !!str 12\nj: \"x\\ty\"\nk: |\n  one\n  two\n",
    );
    let value = |key: &str| doc.get(&[], key).unwrap().unwrap();
    assert_eq!(value("a"), NativeValue::Null);
    assert_eq!(value("b"), NativeValue::Integer(31));
    assert_eq!(value("c"), NativeValue::Integer(15));
    assert_eq!(value("d"), NativeValue::Integer(-12));
    assert_eq!(value("e"), NativeValue::Float(1500.0));
    assert_eq!(value("f"), NativeValue::from(".inf"));
    assert_eq!(value("g"), NativeValue::from("yes"));
    // YAML 1.2: `yes` is a string.
    assert_eq!(value("h"), NativeValue::from("yes"));
    assert_eq!(value("i"), NativeValue::from("12"));
    assert_eq!(value("j"), NativeValue::from("x\ty"));
    assert_eq!(value("k"), NativeValue::from("one\ntwo\n"));
}

#[test]
fn yaml_container_shapes() {
    // A null or empty container has no entries; a scalar is the wrong shape.
    for text in ["extensions:\n", "extensions: ~\n", "extensions: {}\n", ""] {
        assert!(yaml(text).list(EXTENSIONS).unwrap().is_empty(), "{text:?}");
    }
    let err = yaml("extensions: 3\n").list(EXTENSIONS).unwrap_err();
    assert_eq!(err.code(), ErrorCode::UnexpectedShape);
    let err = yaml("- a\n").list(EXTENSIONS).unwrap_err();
    assert_eq!(err.code(), ErrorCode::UnexpectedShape);
    let err = Document::parse(b"a: [1, 2\n", Format::Yaml).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ParseError);
}

// ---------------------------------------------------------------------------
// Span splice

#[test]
fn yaml_span_splice_keeps_unrelated_bytes() {
    let original = fixture("yaml/goose-config.yaml");

    // A new entry: lines appended after the last entry, nothing else moves.
    let mut doc = yaml(&original);
    let entry = json(
        r#"{"enabled": true, "name": "github", "type": "stdio", "cmd": "npx",
            "args": ["-y", "@modelcontextprotocol/server-github"],
            "envs": {"GITHUB_TOKEN": "${GITHUB_TOKEN}"}}"#,
    );
    assert!(doc.upsert(EXTENSIONS, "github", &entry).unwrap());
    let added = text(&doc);
    insta::assert_snapshot!("yaml_goose_upsert_new", line_diff(&original, &added));
    assert_eq!(doc.get(EXTENSIONS, "github").unwrap(), Some(entry.clone()));
    // Undone in the same document: the original bytes.
    assert!(doc.remove(EXTENSIONS, "github").unwrap());
    assert_eq!(text(&doc), original);

    // An update: only the changed members' lines differ; the comment on
    // `- files-mcp` survives because `args` keeps its value.
    let mut doc = yaml(&original);
    let mut files = doc.get(EXTENSIONS, "files").unwrap().unwrap();
    let NativeValue::Object(map) = &mut files else {
        unreachable!()
    };
    map.insert("cmd", "bunx".into());
    map.remove("description");
    map.insert("envs", json(r#"{"ROOT": "~/work", "MODE": "ro"}"#));
    map.insert("display_name", "Files".into());
    assert!(doc.upsert(EXTENSIONS, "files", &files).unwrap());
    let updated = text(&doc);
    insta::assert_snapshot!("yaml_goose_update_member", line_diff(&original, &updated));
    assert!(updated.contains("    - files-mcp # pinned by the lockfile\n"));
    assert_eq!(doc.get(EXTENSIONS, "files").unwrap(), Some(files));

    // A removal takes the comment lines directly above the entry with it.
    let mut doc = yaml(&original);
    assert!(doc.remove(EXTENSIONS, "search").unwrap());
    insta::assert_snapshot!("yaml_goose_remove", line_diff(&original, &text(&doc)));
    assert!(!doc.remove(EXTENSIONS, "search").unwrap());
    assert_eq!(doc.list(EXTENSIONS).unwrap(), ["developer", "files"]);
}

#[test]
fn yaml_removal_takes_only_comments_indented_like_the_entry() {
    // Deeper comments are commented-out members of the entry above, and a
    // column-0 block belongs to the file: both stay.
    let original = "extensions:\n  files:\n    cmd: npx\n    # envs:\n    #   ROOT: x\n# old:\n#   cmd: gone\n  # Remote search\n  search:\n    uri: https://x.example.com\n  last:\n    cmd: y\n";
    let mut doc = yaml(original);
    assert!(doc.remove(EXTENSIONS, "search").unwrap());
    assert_eq!(
        text(&doc),
        "extensions:\n  files:\n    cmd: npx\n    # envs:\n    #   ROOT: x\n# old:\n#   cmd: gone\n  last:\n    cmd: y\n"
    );
    // A deeper comment right above the entry is not taken either.
    let original = "extensions:\n  files:\n    cmd: npx\n    # envs: {}\n  search:\n    uri: https://x.example.com\n";
    let mut doc = yaml(original);
    assert!(doc.remove(EXTENSIONS, "files").unwrap());
    assert_eq!(
        text(&doc),
        "extensions:\n  search:\n    uri: https://x.example.com\n"
    );
    let mut doc = yaml(original);
    assert!(doc.remove(EXTENSIONS, "search").unwrap());
    assert_eq!(
        text(&doc),
        "extensions:\n  files:\n    cmd: npx\n    # envs: {}\n"
    );

    // The same for items of a named list, by the dash line's indentation.
    let servers: &[&str] = &["mcpServers", "[name]"];
    let original = "mcpServers:\n  - name: files\n    command: npx\n    # args: [-y]\n# kept\n  # search, remote\n  - name: search\n    url: https://x.example.com\n";
    let mut doc = yaml(original);
    assert!(doc.remove(servers, "search").unwrap());
    assert_eq!(
        text(&doc),
        "mcpServers:\n  - name: files\n    command: npx\n    # args: [-y]\n# kept\n"
    );
}

#[test]
fn yaml_no_op_edits_are_byte_identical() {
    let original = fixture("yaml/goose-config.yaml");
    let mut doc = yaml(&original);
    for name in doc.list(EXTENSIONS).unwrap() {
        let value = doc.get(EXTENSIONS, &name).unwrap().unwrap();
        assert!(!doc.upsert(EXTENSIONS, &name, &value).unwrap(), "{name}");
    }
    assert!(!doc.remove(EXTENSIONS, "missing").unwrap());
    assert_eq!(text(&doc), original);

    // BOM, CRLF and a missing final newline come back as they were.
    let crlf = format!("\u{feff}{}", original.trim_end().replace('\n', "\r\n"));
    let mut doc = Document::parse(crlf.as_bytes(), Format::Yaml).unwrap();
    assert_eq!(doc.to_bytes(), crlf.as_bytes());
    let entry = json(r#"{"enabled": true, "name": "new", "type": "stdio", "cmd": "new-mcp"}"#);
    assert!(doc.upsert(EXTENSIONS, "new", &entry).unwrap());
    let edited = String::from_utf8(doc.to_bytes()).unwrap();
    assert!(edited.starts_with('\u{feff}'));
    assert!(!edited.ends_with('\n'));
    assert_eq!(edited.matches('\n').count(), edited.matches("\r\n").count());
    assert!(
        edited.contains("\r\n  new:\r\n    enabled: true\r\n"),
        "{edited}"
    );
    assert!(doc.remove(EXTENSIONS, "new").unwrap());
    assert_eq!(doc.to_bytes(), crlf.as_bytes());
}

#[test]
fn yaml_new_files_and_created_containers_undo_cleanly() {
    let entry =
        json(r#"{"command": "npx", "args": ["-y", "files-mcp"], "env": {"ROOT": "~/work"}}"#);
    for original in [
        "",
        "# only a comment",
        "# only a comment\n",
        "theme: dark\n",
    ] {
        let mut doc = yaml(original);
        assert!(doc.upsert(&["mcp_servers"], "files", &entry).unwrap());
        assert_eq!(
            doc.get(&["mcp_servers"], "files").unwrap(),
            Some(entry.clone())
        );
        if original.is_empty() {
            insta::assert_snapshot!("yaml_new_file", text(&doc));
        }
        assert!(doc.remove(&["mcp_servers"], "files").unwrap());
        assert_eq!(text(&doc), original, "{original:?}");
    }
    // Nested containers, created and pruned in one document.
    let mut doc = yaml("theme: dark\n");
    assert!(doc.upsert(&["mcp", "servers"], "files", &entry).unwrap());
    assert!(text(&doc).starts_with("theme: dark\nmcp:\n  servers:\n    files:\n"));
    assert!(doc.remove(&["mcp", "servers"], "files").unwrap());
    assert_eq!(text(&doc), "theme: dark\n");

    // A container that was in the file stays, empty, when its last entry
    // goes; a null one takes entries.
    let mut doc = yaml("mcp_servers:\n  files:\n    command: npx\nother: 1\n");
    assert!(doc.remove(&["mcp_servers"], "files").unwrap());
    assert_eq!(text(&doc), "mcp_servers: {}\nother: 1\n");
    assert!(doc.upsert(&["mcp_servers"], "files", &entry).unwrap());
    assert_eq!(
        doc.get(&["mcp_servers"], "files").unwrap(),
        Some(entry.clone())
    );
    let mut doc = yaml("mcp_servers:\nother: 1\n");
    assert!(doc.upsert(&["mcp_servers"], "files", &entry).unwrap());
    assert!(text(&doc).starts_with("mcp_servers:\n  files:\n    command: npx\n"));
    assert!(text(&doc).ends_with("other: 1\n"));
}

#[test]
fn yaml_new_text_follows_the_files_layout() {
    // Four-space indentation and sequences indented under their key.
    let four = "mcp_servers:\n    files:\n        command: npx\n        args:\n            - -y\n";
    let mut doc = yaml(four);
    let entry = json(r#"{"command": "uvx", "args": ["search-mcp"], "env": {"A": "1"}}"#);
    assert!(doc.upsert(&["mcp_servers"], "search", &entry).unwrap());
    assert_eq!(
        text(&doc),
        format!(
            "{four}    search:\n        command: uvx\n        args:\n            - search-mcp\n        env:\n            A: \"1\"\n"
        )
    );

    // Two spaces, sequences at the key's column (as serde_yaml writes).
    let two = "mcp_servers:\n  files:\n    args:\n    - -y\n";
    let mut doc = yaml(two);
    assert!(doc.upsert(&["mcp_servers"], "search", &entry).unwrap());
    assert_eq!(
        text(&doc),
        format!(
            "{two}  search:\n    command: uvx\n    args:\n    - search-mcp\n    env:\n      A: \"1\"\n"
        )
    );
}

#[test]
fn yaml_strings_are_quoted_when_plain_would_change_them() {
    let tricky = [
        "",
        "yes",
        "No",
        "on",
        "null",
        "~",
        "true",
        "12",
        "1.5",
        "0x10",
        "-1",
        ".5",
        "- item",
        "key: value",
        "a #comment",
        "trailing:",
        " padded",
        "tab\there",
        "line\nbreak",
        "#hash",
        "*alias",
        "&anchor",
        "!tag",
        "@at",
        "`tick`",
        "'single'",
        "\"double\"",
        "{brace}",
        "[bracket]",
        "%percent",
        "|pipe",
        ">fold",
        "---",
        "...",
        "\u{e9}t\u{e9}",
        "${env:TOKEN}",
        "https://example.com/mcp?x=1",
        "-y",
        "--flag=value",
        "C:\\tools\\mcp.exe",
        "back\\slash",
    ];
    let mut entry = NativeMap::new();
    for (index, value) in tricky.iter().enumerate() {
        entry.insert(format!("k{index}"), NativeValue::from(*value));
    }
    entry.insert("quoted key: yes", NativeValue::from("v"));
    let entry = NativeValue::Object(entry);
    let mut doc = yaml("");
    assert!(doc.upsert(&["servers"], "tricky", &entry).unwrap());
    let written = text(&doc);
    assert_eq!(
        yaml(&written).get(&["servers"], "tricky").unwrap(),
        Some(entry)
    );
    insta::assert_snapshot!("yaml_quoting", written);

    // Numbers, booleans, null and empty collections keep their kind.
    let kinds =
        json(r#"{"i": -7, "f": 2.0, "b": false, "n": null, "a": [], "o": {}, "big": 1e20}"#);
    let mut doc = yaml("");
    assert!(doc.upsert(&["servers"], "kinds", &kinds).unwrap());
    assert_eq!(
        yaml(&text(&doc)).get(&["servers"], "kinds").unwrap(),
        Some(kinds)
    );
    let err = doc
        .upsert(&["servers"], "nan", &NativeValue::Float(f64::NAN))
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);
}

// ---------------------------------------------------------------------------
// read_only_format

#[test]
fn yaml_anchors_aliases_and_flow_make_the_target_read_only() {
    let entry = json(r#"{"command": "new-mcp"}"#);
    let servers: &[&str] = &["mcp_servers"];
    for (fixture_name, original) in [
        ("anchors", fixture("yaml/anchors.yaml")),
        ("flow", fixture("yaml/flow.yaml")),
        (
            "multi-document",
            "mcp_servers:\n  a:\n    command: a\n---\nother: 1\n".to_owned(),
        ),
        (
            "anchored container",
            "mcp_servers: &servers\n  a:\n    command: a\ncopy: *servers\n".to_owned(),
        ),
        ("flow root", "{mcp_servers: {a: {command: a}}}\n".to_owned()),
    ] {
        let mut doc = yaml(&original);
        // Reading works.
        assert!(!doc.list(servers).unwrap().is_empty(), "{fixture_name}");
        let first = doc.list(servers).unwrap().remove(0);
        let value = doc.get(servers, &first).unwrap().unwrap();
        // A no-op is fine; any change is refused and changes nothing.
        assert!(
            !doc.upsert(servers, &first, &value).unwrap(),
            "{fixture_name}"
        );
        let err = doc.upsert(servers, "new", &entry).unwrap_err();
        assert_eq!(
            err.code(),
            ErrorCode::ReadOnlyFormat,
            "{fixture_name}: {err}"
        );
        let err = doc.remove(servers, &first).unwrap_err();
        assert_eq!(
            err.code(),
            ErrorCode::ReadOnlyFormat,
            "{fixture_name}: {err}"
        );
        assert_eq!(text(&doc), original, "{fixture_name}");
    }
    assert_eq!(ErrorCode::ReadOnlyFormat.as_str(), "read_only_format");

    // An anchor outside the container does not matter, and flow style
    // inside an entry is fine (the replaced member is written in block
    // style).
    let original = "defaults: &d\n  timeout: 300\nmcp_servers:\n  files:\n    command: npx\n    args: [-y, files-mcp]\n";
    let mut doc = yaml(original);
    let files = json(r#"{"command": "npx", "args": ["-y", "files-mcp", "--ro"]}"#);
    assert!(doc.upsert(servers, "files", &files).unwrap());
    assert_eq!(
        text(&doc),
        "defaults: &d\n  timeout: 300\nmcp_servers:\n  files:\n    command: npx\n    args:\n      - -y\n      - files-mcp\n      - --ro\n"
    );
    assert!(doc.upsert(servers, "other", &entry).unwrap());
}

// ---------------------------------------------------------------------------
// Lists matched by name

#[test]
fn yaml_named_lists_edit_items_by_their_name() {
    let original = fixture("yaml/continue-config.yaml");
    let servers: &[&str] = &["mcpServers", "[name]"];
    let mut doc = yaml(&original);
    assert_eq!(doc.list(servers).unwrap(), ["files", "search"]);
    // Entries come without their `name`.
    assert_eq!(
        doc.get(servers, "search").unwrap(),
        Some(json(
            r#"{"type": "streamable-http", "url": "https://search.example.com/mcp"}"#
        ))
    );

    let github = json(r#"{"command": "npx", "args": ["-y", "server-github"]}"#);
    assert!(doc.upsert(servers, "github", &github).unwrap());
    let mut files = doc.get(servers, "files").unwrap().unwrap();
    let NativeValue::Object(map) = &mut files else {
        unreachable!()
    };
    map.insert("command", "bunx".into());
    map.remove("env");
    assert!(doc.upsert(servers, "files", &files).unwrap());
    let edited = text(&doc);
    insta::assert_snapshot!("yaml_continue_named_items", line_diff(&original, &edited));
    assert_eq!(doc.get(servers, "github").unwrap(), Some(github));
    assert_eq!(doc.get(servers, "files").unwrap(), Some(files));
    // `models` and `rules` are lists too, but nothing touched them.
    assert!(edited.contains("models:\n    - name: Claude\n"));
    assert!(edited.ends_with("rules:\n    - Keep answers short\n"));

    assert!(doc.remove(servers, "github").unwrap());
    assert!(doc.remove(servers, "search").unwrap());
    assert_eq!(doc.list(servers).unwrap(), ["files"]);

    // A new file gets the list; undoing removes it again.
    let mut doc = yaml("name: Local Assistant\n");
    assert!(
        doc.upsert(servers, "files", &json(r#"{"command": "npx"}"#))
            .unwrap()
    );
    assert_eq!(
        text(&doc),
        "name: Local Assistant\nmcpServers:\n  - name: files\n    command: npx\n"
    );
    assert!(doc.remove(servers, "files").unwrap());
    assert_eq!(text(&doc), "name: Local Assistant\n");

    // Only a mapping can be a list entry.
    let err = doc
        .upsert(servers, "bad", &NativeValue::from("text"))
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);
}

#[test]
fn yaml_string_lists() {
    let mut doc = yaml("project:\n  disabled:\n  - a\n  - b\n");
    assert!(
        doc.set_list_member(&["project", "disabled"], "c", true)
            .unwrap()
    );
    assert!(
        !doc.set_list_member(&["project", "disabled"], "c", true)
            .unwrap()
    );
    assert_eq!(text(&doc), "project:\n  disabled:\n  - a\n  - b\n  - c\n");
    assert!(
        doc.set_list_member(&["project", "disabled"], "a", false)
            .unwrap()
    );
    assert_eq!(text(&doc), "project:\n  disabled:\n  - b\n  - c\n");

    let mut doc = yaml("other: 1\n");
    assert!(
        doc.set_list_member(&["project", "disabled"], "x", true)
            .unwrap()
    );
    // Without a block sequence to copy, sequences are indented.
    assert_eq!(text(&doc), "other: 1\nproject:\n  disabled:\n    - x\n");
    assert!(
        doc.set_list_member(&["project", "disabled"], "x", false)
            .unwrap()
    );
    assert_eq!(text(&doc), "other: 1\n");
}

// ---------------------------------------------------------------------------
// JSON5

#[test]
fn json5_files_that_are_not_jsonc_are_read_only() {
    let original = fixture("json5/openclaw.json5");
    let servers: &[&str] = &["mcp", "servers"];
    let mut doc = Document::parse(original.as_bytes(), Format::Json5).unwrap();
    assert_eq!(doc.format(), Format::Json5);
    assert_eq!(doc.list(servers).unwrap(), ["files", "search"]);
    let files = doc.get(servers, "files").unwrap().unwrap();
    assert_eq!(
        files,
        json(
            r#"{"type": "local", "command": ["npx", "-y", "files-mcp"], "environment": {"ROOT": "~/work"}}"#
        )
    );
    // No change is fine; every change is refused and leaves the bytes.
    assert!(!doc.upsert(servers, "files", &files).unwrap());
    assert!(!doc.remove(servers, "missing").unwrap());
    for err in [
        doc.upsert(servers, "new", &json(r#"{"type": "local"}"#))
            .unwrap_err(),
        doc.remove(servers, "files").unwrap_err(),
        doc.set_list_member(&["mcp", "disabled"], "files", true)
            .unwrap_err(),
    ] {
        assert_eq!(err.code(), ErrorCode::ReadOnlyFormat, "{err}");
    }
    assert_eq!(text(&doc), original);
    // Strict JSON readers reject it, so this is the only way to read it.
    assert!(Document::parse(original.as_bytes(), Format::Jsonc).is_err());
    let err = Document::parse(b"{ unclosed: ", Format::Json5).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ParseError);
}

#[test]
fn json5_files_that_are_jsonc_are_edited_as_jsonc() {
    let original = fixture("json5/codebuddy-jsonc.json5");
    let servers: &[&str] = &["mcpServers"];
    let mut doc = Document::parse(original.as_bytes(), Format::Json5).unwrap();
    let entry = json(r#"{"type": "http", "url": "https://search.example.com/mcp"}"#);
    assert!(doc.upsert(servers, "search", &entry).unwrap());
    let edited = text(&doc);
    assert!(edited.contains("// CodeBuddy MCP servers"));
    assert!(edited.contains("\"args\": [\"-y\", \"files-mcp\"], // pinned"));
    assert_eq!(
        Document::parse(edited.as_bytes(), Format::Json5)
            .unwrap()
            .get(servers, "search")
            .unwrap(),
        Some(entry)
    );
    assert!(doc.remove(servers, "search").unwrap());
    assert_eq!(text(&doc), original);
    // A new file is JSON(C).
    let mut doc = Document::empty(Format::Json5);
    assert!(
        doc.upsert(servers, "files", &json(r#"{"command": "npx"}"#))
            .unwrap()
    );
    assert!(NativeValue::parse_json(&text(&doc)).is_ok());
}

// ---------------------------------------------------------------------------
// Properties

fn config() -> Config {
    Config {
        failure_persistence: None,
        ..Config::default()
    }
}

fn scalar() -> impl Strategy<Value = NativeValue> {
    prop_oneof![
        "[a-zA-Z0-9 _./:$@{}#,'\"\\-\u{e9}]{0,10}".prop_map(NativeValue::String),
        any::<bool>().prop_map(NativeValue::Bool),
        any::<i64>().prop_map(NativeValue::Integer),
        Just(NativeValue::Null),
    ]
}

fn value() -> impl Strategy<Value = NativeValue> {
    scalar().prop_recursive(2, 10, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..3).prop_map(NativeValue::Array),
            prop::collection::vec(("[a-z][a-z0-9_]{0,5}", inner), 0..3)
                .prop_map(|members| NativeValue::Object(members.into_iter().collect())),
        ]
    })
}

fn entry() -> impl Strategy<Value = NativeValue> {
    prop::collection::vec(("[a-z][a-z0-9_]{0,5}", value()), 1..4)
        .prop_map(|members| NativeValue::Object(members.into_iter().collect()))
}

/// A YAML file with a comment, `entries` under `servers` written by the
/// editor itself into `indent`-wide text, and a key after the container.
fn rendered(entries: &[(String, NativeValue)], indent: usize) -> String {
    // A seed entry sets the indentation width the editor copies.
    let pad = " ".repeat(indent);
    let mut doc = yaml(&format!(
        "# servers\nservers:\n{pad}seed:\n{pad}{pad}k: v\n"
    ));
    for (name, entry) in entries {
        doc.upsert(&["servers"], name, entry).unwrap();
    }
    doc.remove(&["servers"], "seed").unwrap();
    format!("{}tail: true # end\n", text(&doc))
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn yaml_edits_keep_other_entries_and_undo_cleanly(
        entries in prop::collection::btree_map("[a-m][a-z0-9-]{0,6}", entry(), 1..4),
        new in entry(),
        indent in prop_oneof![Just(2usize), Just(4usize)],
    ) {
        let entries: Vec<(String, NativeValue)> = entries.into_iter().collect();
        let original = rendered(&entries, indent);
        let servers: &[&str] = &["servers"];

        // No-op edits are byte-identical.
        let mut doc = yaml(&original);
        for (name, entry) in &entries {
            prop_assert_eq!(doc.get(servers, name).unwrap(), Some(entry.clone()));
            prop_assert!(!doc.upsert(servers, name, entry).unwrap());
        }
        prop_assert_eq!(text(&doc), original.clone());

        // Upsert then remove of a new entry restores the original.
        prop_assert!(doc.upsert(servers, "new-entry", &new).unwrap());
        prop_assert_eq!(doc.get(servers, "new-entry").unwrap(), Some(new.clone()));
        prop_assert!(doc.remove(servers, "new-entry").unwrap());
        prop_assert_eq!(text(&doc), original.clone());

        // Replacing one entry leaves the others' values and the tail alone.
        let (target, _) = &entries[0];
        let mut doc = yaml(&original);
        prop_assert!(doc.upsert(servers, target, &new).unwrap() || entries[0].1 == new);
        let reread = yaml(&text(&doc));
        prop_assert_eq!(reread.get(servers, target).unwrap(), Some(new.clone()));
        for (name, entry) in &entries[1..] {
            prop_assert_eq!(reread.get(servers, name).unwrap(), Some(entry.clone()));
        }
        prop_assert!(text(&doc).starts_with("# servers\nservers:\n"));
        prop_assert!(text(&doc).ends_with("tail: true # end\n"));
    }
}

//! Golden tests for the format-preserving editors (SPEC 7.2, 19 item 3):
//! the resulting file text of upsert, remove and list edits on synthetic
//! client files in `fixtures/formats`.
//!
//! Every edit also runs on a BOM + CRLF copy of the fixture, which must give
//! the same text with a BOM and CRLF line endings.

use std::path::{Path, PathBuf};

use agent_loadout_core::ErrorCode;
use agent_loadout_core::formats::{Document, Format, NativeValue};

const BOM: &str = "\u{feff}";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("formats")
}

/// A fixture with LF line endings, whatever the checkout did to it.
fn fixture(relative: &str) -> String {
    let path = fixtures().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
        .replace("\r\n", "\n")
}

fn json(text: &str) -> NativeValue {
    NativeValue::parse_json(text).unwrap()
}

/// Runs `ops` on `lf` and on its BOM + CRLF copy, checks that both give the
/// same text in their own style, and returns the LF result.
fn edit(format: Format, lf: &str, ops: impl Fn(&mut Document)) -> String {
    let mut doc = Document::parse(lf.as_bytes(), format).unwrap();
    ops(&mut doc);
    let out = String::from_utf8(doc.to_bytes()).unwrap();

    // A file without line breaks has no line ending to keep.
    let crlf = lf.contains('\n');
    let styled = |text: &str| {
        let text = if crlf {
            text.replace('\n', "\r\n")
        } else {
            text.to_owned()
        };
        format!("{BOM}{text}")
    };
    let mut doc = Document::parse(styled(lf).as_bytes(), format).unwrap();
    ops(&mut doc);
    assert_eq!(
        String::from_utf8(doc.to_bytes()).unwrap(),
        styled(&out),
        "BOM + CRLF copy"
    );
    out
}

/// Parsing and writing back changes nothing, and adding then removing an
/// entry (and a list member) restores the file.
fn round_trips(format: Format, lf: &str, container: &[&str], list: &[&str]) {
    let doc = Document::parse(lf.as_bytes(), format).unwrap();
    assert_eq!(doc.to_bytes(), lf.as_bytes());
    let out = edit(format, lf, |doc| {
        let value = json(r#"{"command": "probe", "args": ["--stdio"], "env": {"A": "1"}}"#);
        assert!(doc.upsert(container, "probe", &value).unwrap());
        assert!(doc.remove(container, "probe").unwrap());
        assert!(doc.set_list_member(list, "probe", true).unwrap());
        assert!(doc.set_list_member(list, "probe", false).unwrap());
    });
    assert_eq!(out, lf);
}

// ---------------------------------------------------------------------------
// JSON

const CLAUDE: &str = "json/claude-user.json";
const SAMPLE_APP: &str = "C:/work/sample-app";

#[test]
fn json_claude_user_file() {
    let lf = fixture(CLAUDE);
    round_trips(
        Format::Json,
        &lf,
        &["mcpServers"],
        &["projects", SAMPLE_APP, "disabledMcpServers"],
    );

    let doc = Document::parse(lf.as_bytes(), Format::Json).unwrap();
    assert_eq!(
        doc.list(&["mcpServers"]).unwrap(),
        ["filesystem", "remote-docs"]
    );
    assert_eq!(
        doc.get(&["mcpServers"], "remote-docs").unwrap(),
        Some(json(
            r#"{"type": "http", "url": "https://docs.example.com/mcp",
                "headers": {"Authorization": "Bearer ${DOCS_TOKEN}"}}"#
        ))
    );
    assert_eq!(
        doc.list(&["projects", "C:/work/other-app", "mcpServers"])
            .unwrap(),
        ["local-db"]
    );
    assert_eq!(
        doc.list(&["projects", "C:/work/missing", "mcpServers"])
            .unwrap(),
        Vec::<String>::new()
    );

    insta::assert_snapshot!(
        "json_claude_upsert_new",
        edit(Format::Json, &lf, |doc| {
            let value = json(
                r#"{"type": "stdio", "command": "new-mcp", "args": ["--port", "0"], "env": {"LEVEL": "debug"}}"#,
            );
            assert!(doc.upsert(&["mcpServers"], "new-server", &value).unwrap());
        })
    );
    insta::assert_snapshot!(
        "json_claude_update_existing",
        edit(Format::Json, &lf, |doc| {
            // `args` changes, `env` goes, `cwd` is new; `type` and `command`
            // keep their text.
            let value = json(
                r#"{"type": "stdio", "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "D:\\data"], "cwd": "C:\\work"}"#,
            );
            assert!(doc.upsert(&["mcpServers"], "filesystem", &value).unwrap());
        })
    );
    insta::assert_snapshot!(
        "json_claude_remove",
        edit(Format::Json, &lf, |doc| {
            assert!(doc.remove(&["mcpServers"], "remote-docs").unwrap());
        })
    );
    insta::assert_snapshot!(
        "json_claude_project_scope",
        edit(Format::Json, &lf, |doc| {
            let local = ["projects", SAMPLE_APP, "mcpServers"];
            let disabled = ["projects", SAMPLE_APP, "disabledMcpServers"];
            let value = json(r#"{"type": "http", "url": "http://localhost:8080/mcp"}"#);
            assert!(doc.upsert(&local, "local-api", &value).unwrap());
            assert!(doc.set_list_member(&disabled, "filesystem", true).unwrap());
            assert!(
                doc.set_list_member(&disabled, "remote-docs", false)
                    .unwrap()
            );
            assert!(
                !doc.set_list_member(&disabled, "remote-docs", false)
                    .unwrap()
            );
        })
    );
    insta::assert_snapshot!(
        "json_claude_new_project",
        edit(Format::Json, &lf, |doc| {
            let local = ["projects", "C:/work/new-app", "mcpServers"];
            let value = json(r#"{"type": "stdio", "command": "new-mcp"}"#);
            assert!(doc.upsert(&local, "new-server", &value).unwrap());
        })
    );
}

#[test]
fn json_containers_created_by_an_edit_are_removed_with_it() {
    let lf = fixture(CLAUDE);
    let out = edit(Format::Json, &lf, |doc| {
        let local = ["projects", "C:/work/new-app", "mcpServers"];
        let value = json(r#"{"command": "new-mcp"}"#);
        doc.upsert(&local, "a", &value).unwrap();
        doc.upsert(&local, "b", &value).unwrap();
        doc.remove(&local, "a").unwrap();
        doc.remove(&local, "b").unwrap();
    });
    assert_eq!(out, lf);

    // An empty container that was already there stays.
    let out = edit(Format::Json, &lf, |doc| {
        let local = ["projects", SAMPLE_APP, "mcpServers"];
        doc.upsert(&local, "a", &json(r#"{"command": "x"}"#))
            .unwrap();
        doc.remove(&local, "a").unwrap();
    });
    assert_eq!(out, lf);
    assert!(out.contains(r#""mcpServers": {},"#));
}

#[test]
fn json_omp_file_with_four_space_indent() {
    let lf = fixture("json/omp-mcp.json");
    round_trips(Format::Json, &lf, &["mcpServers"], &["disabledServers"]);
    insta::assert_snapshot!(
        "json_omp_upsert_and_enable",
        edit(Format::Json, &lf, |doc| {
            let value =
                json(r#"{"type": "stdio", "command": "notes-mcp", "args": [], "timeout": 5000}"#);
            assert!(doc.upsert(&["mcpServers"], "notes", &value).unwrap());
            assert!(
                doc.set_list_member(&["disabledServers"], "notes", true)
                    .unwrap()
            );
            assert!(
                doc.set_list_member(&["disabledServers"], "issues", false)
                    .unwrap()
            );
        })
    );
}

#[test]
fn json_compact_file() {
    let lf = fixture("json/compact.json");
    round_trips(Format::Json, &lf, &["mcpServers"], &["disabledServers"]);
    // A one-line file stays on one line and keeps its `:` and `,` spacing.
    insta::assert_snapshot!(
        "json_compact_edits",
        edit(Format::Json, &lf, |doc| {
            let value = json(r#"{"command": "second-mcp", "env": {"A": "1", "B": "2"}}"#);
            assert!(doc.upsert(&["mcpServers"], "second", &value).unwrap());
            let value = json(r#"{"command": "tool-mcp", "args": ["--stdio"]}"#);
            assert!(doc.upsert(&["mcpServers"], "tool", &value).unwrap());
            assert!(
                doc.set_list_member(&["disabledServers"], "tool", true)
                    .unwrap()
            );
        })
    );
}

#[test]
fn json_new_file_skeleton() {
    let value = json(r#"{"command": "new-mcp", "args": ["--stdio"]}"#);
    for format in [Format::Json, Format::Jsonc] {
        let mut doc = Document::empty(format);
        assert_eq!(doc.to_bytes(), b"");
        assert!(doc.upsert(&["mcpServers"], "new-server", &value).unwrap());
        insta::assert_snapshot!(
            format!("{}_new_file", format.as_str()),
            String::from_utf8(doc.to_bytes()).unwrap()
        );
        assert!(doc.remove(&["mcpServers"], "new-server").unwrap());
        assert_eq!(doc.to_bytes(), b"");
    }
}

// ---------------------------------------------------------------------------
// JSONC

#[test]
fn jsonc_opencode_file() {
    let lf = fixture("jsonc/opencode.jsonc");
    round_trips(Format::Jsonc, &lf, &["mcp"], &["disabled"]);
    insta::assert_snapshot!(
        "jsonc_opencode_edits",
        edit(Format::Jsonc, &lf, |doc| {
            // Disable `files` in place: its comments stay.
            let mut files = doc.get(&["mcp"], "files").unwrap().unwrap();
            let NativeValue::Object(members) = &mut files else {
                panic!("files is an object");
            };
            members.insert("enabled", false.into());
            assert!(doc.upsert(&["mcp"], "files", &files).unwrap());
            let value = json(
                r#"{"type": "local", "command": ["new-mcp", "--stdio"], "environment": {"LEVEL": "debug"}, "enabled": true}"#,
            );
            assert!(doc.upsert(&["mcp"], "new-server", &value).unwrap());
            assert!(doc.remove(&["mcp"], "search").unwrap());
        })
    );
}

#[test]
fn jsonc_tab_indented_file_without_trailing_commas() {
    let lf = fixture("jsonc/tabs-no-trailing-commas.jsonc");
    round_trips(Format::Jsonc, &lf, &["servers"], &["disabled"]);
    let out = edit(Format::Jsonc, &lf, |doc| {
        let value = json(r#"{"command": "fmt-mcp", "args": ["--check"]}"#);
        assert!(doc.upsert(&["servers"], "fmt", &value).unwrap());
        let value = json(r#"{"command": "lint-mcp", "args": ["--fix"]}"#);
        assert!(doc.upsert(&["servers"], "lint", &value).unwrap());
    });
    insta::assert_snapshot!("jsonc_tabs_edits", out);
    // No trailing commas appear: without its comments the file is strict JSON.
    let stripped = out
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .map(|line| line.replace(" /* inline note */", "") + "\n")
        .collect::<String>();
    Document::parse(stripped.as_bytes(), Format::Json).unwrap();
}

// ---------------------------------------------------------------------------
// TOML

const CODEX: &str = "toml/codex-config.toml";
const LEGACY: &str = "[mcp.servers.legacy-tool]\ncommand = \"legacy-tool\"\nargs = [\"--stdio\"]\n";

#[test]
fn toml_codex_file() {
    let lf = fixture(CODEX);
    round_trips(
        Format::Toml,
        &lf,
        &["mcp_servers"],
        &["profiles", "fast", "disabled_tools"],
    );

    let doc = Document::parse(lf.as_bytes(), Format::Toml).unwrap();
    assert_eq!(doc.list(&["mcp_servers"]).unwrap(), ["docs", "tracker"]);
    assert_eq!(doc.list(&["mcp", "servers"]).unwrap(), ["legacy-tool"]);
    assert_eq!(
        doc.get(&["mcp_servers"], "docs").unwrap(),
        Some(json(
            r#"{"command": "npx", "args": ["-y", "docs-mcp"], "startup_timeout_sec": 20,
                "env": {"DOCS_ROOT": "~/docs"}}"#
        ))
    );

    let upsert_new = edit(Format::Toml, &lf, |doc| {
        let value = json(
            r#"{"command": "new-mcp", "args": ["--stdio"], "env": {"LEVEL": "debug"}, "enabled": true}"#,
        );
        assert!(doc.upsert(&["mcp_servers"], "new-server", &value).unwrap());
    });
    let update = edit(Format::Toml, &lf, |doc| {
        // `args` and `env` change, `startup_timeout_sec` goes, `enabled` is
        // new: the comments on `command` stay.
        let value = json(
            r#"{"command": "npx", "args": ["-y", "docs-mcp@2"], "env": {"DOCS_ROOT": "~/docs", "LEVEL": "info"}, "enabled": false}"#,
        );
        assert!(doc.upsert(&["mcp_servers"], "docs", &value).unwrap());
    });
    let remove = edit(Format::Toml, &lf, |doc| {
        assert!(doc.remove(&["mcp_servers"], "docs").unwrap());
        assert!(doc.remove(&["mcp_servers"], "tracker").unwrap());
    });
    for out in [&upsert_new, &update, &remove] {
        assert!(out.contains(LEGACY), "legacy table untouched:\n{out}");
        Document::parse(out.as_bytes(), Format::Toml).unwrap();
    }
    assert!(update.contains("command = \"npx\"   # launcher\n"));
    insta::assert_snapshot!("toml_codex_upsert_new", upsert_new);
    insta::assert_snapshot!("toml_codex_update_existing", update);
    insta::assert_snapshot!("toml_codex_remove", remove);
}

#[test]
fn toml_inline_entries() {
    let lf = fixture("toml/inline-entries.toml");
    round_trips(Format::Toml, &lf, &["mcp_servers"], &["disabled"]);
    insta::assert_snapshot!(
        "toml_inline_edits",
        edit(Format::Toml, &lf, |doc| {
            let value = json(r#"{"command": "new-mcp", "env": {"LEVEL": "debug"}}"#);
            assert!(doc.upsert(&["mcp_servers"], "new-server", &value).unwrap());
            let value = json(r#"{"url": "https://search.example.com/v2/mcp"}"#);
            assert!(doc.upsert(&["mcp_servers"], "search", &value).unwrap());
            assert!(doc.remove(&["mcp_servers"], "files").unwrap());
        })
    );
}

#[test]
fn toml_new_file_skeleton() {
    let mut doc = Document::empty(Format::Toml);
    let value = json(r#"{"command": "new-mcp", "args": ["--stdio"], "env": {"LEVEL": "debug"}}"#);
    assert!(doc.upsert(&["mcp_servers"], "new-server", &value).unwrap());
    insta::assert_snapshot!("toml_new_file", String::from_utf8(doc.to_bytes()).unwrap());
    assert!(doc.remove(&["mcp_servers"], "new-server").unwrap());
    assert_eq!(doc.to_bytes(), b"");
}

// ---------------------------------------------------------------------------
// Errors

#[test]
fn errors_are_typed_and_leave_the_document_unchanged() {
    let code = |result: Result<Document, agent_loadout_core::CoreError>| result.unwrap_err().code();

    // Strict JSON rejects comments and trailing commas; JSONC rejects JSON5.
    assert_eq!(
        code(Document::parse(b"{\n  // c\n}\n", Format::Json)),
        ErrorCode::ParseError
    );
    assert_eq!(
        code(Document::parse(b"{\"a\": 1,}\n", Format::Json)),
        ErrorCode::ParseError
    );
    assert_eq!(
        code(Document::parse(b"{'a': 1}\n", Format::Jsonc)),
        ErrorCode::ParseError
    );
    assert_eq!(
        code(Document::parse(b"{a: 1}\n", Format::Jsonc)),
        ErrorCode::ParseError
    );
    assert_eq!(
        code(Document::parse(b"{\"a\": 0x1}\n", Format::Jsonc)),
        ErrorCode::ParseError
    );
    assert_eq!(
        code(Document::parse(b"[a\nb = 1\n", Format::Toml)),
        ErrorCode::ParseError
    );
    assert_eq!(
        code(Document::parse(b"\xff\xfe", Format::Toml)),
        ErrorCode::ParseError
    );
    let err = Document::parse(b"a = 1\nb = = 2\n", Format::Toml).unwrap_err();
    assert!(err.message().contains("line 2"), "{err}");

    let value = json(r#"{"command": "x"}"#);
    for (format, text, container) in [
        (Format::Json, "{\"mcpServers\": []}\n", "mcpServers"),
        (Format::Jsonc, "[1, 2]\n", "mcpServers"),
        (Format::Toml, "mcp_servers = [1]\n", "mcp_servers"),
    ] {
        let mut doc = Document::parse(text.as_bytes(), format).unwrap();
        let err = doc.upsert(&[container], "a", &value).unwrap_err();
        assert_eq!(err.code(), ErrorCode::UnexpectedShape, "{format:?}: {err}");
        assert_eq!(
            doc.list(&[container]).unwrap_err().code(),
            ErrorCode::UnexpectedShape
        );
        assert_eq!(doc.to_bytes(), text.as_bytes());
    }

    // TOML has no null; nothing is written.
    let lf = fixture(CODEX);
    let mut doc = Document::parse(lf.as_bytes(), Format::Toml).unwrap();
    let err = doc
        .upsert(&["mcp_servers"], "docs", &json(r#"{"command": null}"#))
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);
    assert_eq!(doc.to_bytes(), lf.as_bytes());

    // A list that is not an array.
    let mut doc = Document::parse(b"{\"disabled\": \"x\"}", Format::Json).unwrap();
    let err = doc.set_list_member(&["disabled"], "x", true).unwrap_err();
    assert_eq!(err.code(), ErrorCode::UnexpectedShape);
}

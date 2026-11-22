//! Gemini CLI, Antigravity, OpenCode and Grok Build (SPEC 7.3, 7.4 and 7.5
//! rows `gemini`, `antigravity`, `opencode`, `grok`; M21): mapper goldens
//! per transport with their round trip, transport inference (Gemini `url`
//! is SSE and `httpUrl` http, Antigravity `serverUrl`, OpenCode `remote`
//! by URL path), OpenCode command arrays, reading each client's user and
//! project files, targets and detection through a fake `Env`, and through
//! the apply engine: user and project files, per-client reference syntax,
//! project suppression (Gemini `mcp.excluded`, OpenCode `enabled: false`,
//! none for Grok and Antigravity) and Grok's compatibility sources in the
//! effective view (SPEC 6.4).

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use agent_loadout_core::adapters::detect::{self, Evidence, NoRegistry};
use agent_loadout_core::adapters::mapper::{
    EntryMapper, Launch, MapError, RemoteLaunch, RenderedServer, ServerDef, StdioLaunch, WriteCx,
};
use agent_loadout_core::adapters::{
    ADAPTERS, AdapterSpec, McpSpec, Target, Tier, antigravity, claude_code, cursor, gemini, grok,
    opencode,
};
use agent_loadout_core::apply::ledger::EntryKind;
use agent_loadout_core::apply::{EntryState, Plan};
use agent_loadout_core::formats::{Document, Format, NativeValue};
use agent_loadout_core::layering::{EffectiveView, LOCAL_FILE, SHARED_FILE, effective};
use agent_loadout_core::model::applied::Scope;
use agent_loadout_core::model::clients::ClientPrefs;
use agent_loadout_core::model::server::{ServerFields, Transport};
use agent_loadout_core::{Env, ErrorCode, Os};
use agent_loadout_testkit::TempHome;
use serde_json::Value;
use support::{Fx, git_commit, git_init, stdio as store_stdio};

const GEMINI: &AdapterSpec = &gemini::SPEC;
const ANTIGRAVITY: &AdapterSpec = &antigravity::SPEC;
const OPENCODE: &AdapterSpec = &opencode::SPEC;
const GROK: &AdapterSpec = &grok::SPEC;
const TIER_C: [&AdapterSpec; 4] = [GEMINI, ANTIGRAVITY, OPENCODE, GROK];

static NO_PREFS: LazyLock<ClientPrefs> = LazyLock::new(ClientPrefs::default);

fn fixture_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("clients")
        .join(relative)
}

/// A fixture with LF line endings, whatever the checkout did to it.
fn fixture(relative: &str) -> String {
    let path = fixture_path(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
        .replace("\r\n", "\n")
}

fn mcp(spec: &AdapterSpec) -> &McpSpec {
    spec.mcp.as_ref().unwrap()
}

fn mapper(spec: &AdapterSpec) -> &'static dyn EntryMapper {
    mcp(spec).mapper
}

fn servers(spec: &AdapterSpec) -> &'static [&'static str] {
    mcp(spec).container.segments()
}

fn parse(spec: &AdapterSpec, text: &str) -> Document {
    Document::parse(text.as_bytes(), mcp(spec).format).unwrap()
}

fn text(doc: &Document) -> String {
    String::from_utf8(doc.to_bytes()).unwrap()
}

fn json(text: &str) -> NativeValue {
    NativeValue::parse_json(text).unwrap()
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn owned(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

fn stdio(name: &str, command: &str, args: &[&str], env: &[(&str, &str)]) -> RenderedServer {
    RenderedServer::new(
        name,
        Launch::Stdio(StdioLaunch {
            command: command.to_owned(),
            args: owned(args),
            env: map(env),
            ..StdioLaunch::default()
        }),
    )
}

fn remote(name: &str, transport: Transport, url: &str, headers: &[(&str, &str)]) -> RenderedServer {
    let launch = RemoteLaunch {
        url: url.to_owned(),
        headers: map(headers),
        ..RemoteLaunch::default()
    };
    RenderedServer::new(
        name,
        match transport {
            Transport::Http => Launch::Http(launch),
            Transport::Sse => Launch::Sse(launch),
            Transport::Stdio => unreachable!("remote transports only"),
        },
    )
}

fn stdio_mut(server: &mut RenderedServer) -> &mut StdioLaunch {
    match &mut server.launch {
        Launch::Stdio(stdio) => stdio,
        _ => unreachable!("a stdio server"),
    }
}

fn write(spec: &AdapterSpec, os: Os, server: &RenderedServer) -> Result<NativeValue, MapError> {
    mapper(spec).to_native(server, &WriteCx { os, existing: None })
}

fn fields_of(server: &RenderedServer) -> ServerFields {
    let non_empty = |map: BTreeMap<String, String>| (!map.is_empty()).then_some(map);
    match &server.launch {
        Launch::Stdio(stdio) => ServerFields {
            transport: Some(Transport::Stdio),
            command: Some(stdio.command.clone()),
            args: Some(stdio.args.clone()),
            cwd: stdio.cwd.clone(),
            env: non_empty(stdio.env.clone()),
            ..ServerFields::default()
        },
        Launch::Http(remote) | Launch::Sse(remote) => ServerFields {
            transport: Some(server.launch.transport()),
            url: Some(remote.url.clone()),
            headers: non_empty(remote.headers.clone()),
            ..ServerFields::default()
        },
    }
}

/// A read-back definition as store TOML plus what has no store field.
fn describe(def: &ServerDef) -> String {
    let mut out = toml::to_string(&def.fields).unwrap();
    if def.disabled {
        out.push_str("# disabled\n");
    }
    if let Some(allow) = &def.tools.allow {
        out.push_str(&format!("# tools.allow: {allow:?}\n"));
    }
    if let Some(deny) = &def.tools.deny {
        out.push_str(&format!("# tools.deny: {deny:?}\n"));
    }
    for (key, value) in def.passthrough.iter() {
        out.push_str(&format!("# passthrough {key}: {value:?}\n"));
    }
    for (key, value) in def.unmapped.iter() {
        out.push_str(&format!("# unmapped {key}: {value:?}\n"));
    }
    out
}

/// The file a new entry makes (from the client's skeleton) and what it
/// reads back as; checks the round trip.
fn golden(spec: &AdapterSpec, os: Os, server: &RenderedServer) -> String {
    let mapper = mapper(spec);
    let native = write(spec, os, server).unwrap();
    let mut doc = Document::parse(mcp(spec).skeleton.as_bytes(), mcp(spec).format).unwrap();
    assert!(doc.upsert(servers(spec), &server.name, &native).unwrap());
    let reread = parse(spec, &text(&doc))
        .get(servers(spec), &server.name)
        .unwrap()
        .unwrap();
    assert_eq!(mapper.canonical(&reread), mapper.canonical(&native));
    let def = mapper.from_native(&server.name, &reread).unwrap();
    assert_eq!(
        def.fields,
        fields_of(server),
        "round trip of {}",
        server.name
    );
    assert_eq!(def.tools, server.tools);
    assert_eq!(def.disabled, !server.enabled);
    assert_eq!(def.passthrough, server.passthrough);
    assert!(def.unmapped.is_empty(), "{:?}", def.unmapped);
    assert!(mapper.skipped_by_client(&reread).is_none());
    format!("{}\n# read back\n{}", text(&doc), describe(&def))
}

/// Every entry of a container read back, or the error it gives.
fn read_all(spec: &AdapterSpec, doc: &Document) -> String {
    let container = servers(spec);
    let mut out = String::new();
    for key in doc.list(container).unwrap() {
        let value = doc.get(container, &key).unwrap().unwrap();
        out.push_str(&format!("## {key}\n"));
        match mapper(spec).from_native(&key, &value) {
            Ok(def) => out.push_str(&describe(&def)),
            Err(err) => out.push_str(&format!("error[{}]: {err}\n", err.code())),
        }
        if let Some(reason) = mapper(spec).skipped_by_client(&value) {
            out.push_str(&format!("# skipped by the client: {reason}\n"));
        }
    }
    out
}

/// A file's read-back text through every entry, plus round trips: writing
/// each readable entry back over itself gives the same canonical form.
fn read_and_rewrite(spec: &AdapterSpec, relative: &str) -> String {
    let doc = parse(spec, &fixture(relative));
    let container = servers(spec);
    for key in doc.list(container).unwrap() {
        let value = doc.get(container, &key).unwrap().unwrap();
        let Ok(def) = mapper(spec).from_native(&key, &value) else {
            continue;
        };
        if def.unmapped.iter().count() > 0 || mapper(spec).skipped_by_client(&value).is_some() {
            continue;
        }
        let Some(server) = rendered_of(&key, &def) else {
            continue;
        };
        let Ok(again) = mapper(spec).to_native(
            &server,
            &WriteCx {
                os: Os::Linux,
                existing: Some(&value),
            },
        ) else {
            continue;
        };
        assert_eq!(
            mapper(spec).canonical(&again),
            mapper(spec).canonical(&value),
            "{relative}: {key} does not write back as it reads"
        );
    }
    read_all(spec, &doc)
}

/// The rendered server a read-back definition stands for.
fn rendered_of(name: &str, def: &ServerDef) -> Option<RenderedServer> {
    let fields = &def.fields;
    let launch = match fields.transport? {
        Transport::Stdio => Launch::Stdio(StdioLaunch {
            command: fields.command.clone()?,
            args: fields.args.clone().unwrap_or_default(),
            env: fields.env.clone().unwrap_or_default(),
            cwd: fields.cwd.clone(),
            ..StdioLaunch::default()
        }),
        transport => {
            let remote = RemoteLaunch {
                url: fields.url.clone()?,
                headers: fields.headers.clone().unwrap_or_default(),
                ..RemoteLaunch::default()
            };
            match transport {
                Transport::Http => Launch::Http(remote),
                _ => Launch::Sse(remote),
            }
        }
    };
    Some(RenderedServer {
        name: name.to_owned(),
        launch,
        enabled: !def.disabled,
        tools: def.tools.clone(),
        passthrough: def.passthrough.clone(),
    })
}

// ---------------------------------------------------------------------------
// Table

#[test]
fn tier_c_adapters_are_registered() {
    let ids: Vec<&str> = ADAPTERS.iter().map(|spec| spec.id).collect();
    // SPEC 7.3 table order: after Cursor, before the Tier D rows.
    let position = |id: &str| ids.iter().position(|other| *other == id).unwrap();
    assert!(position("cursor") < position("gemini"));
    assert!(position("gemini") < position("antigravity"));
    assert!(position("antigravity") < position("opencode"));
    assert!(position("opencode") < position("grok"));
    assert!(position("grok") < position("vscode-legacy"));
    for spec in TIER_C {
        assert_eq!(spec.tier, Tier::C, "{}", spec.id);
        assert!(!mcp(spec).read_only, "{}", spec.id);
        assert!(mcp(spec).project_local.is_empty(), "{}", spec.id);
        assert_eq!(
            mcp(spec).oses,
            [Os::Windows, Os::Linux, Os::MacOs],
            "{}",
            spec.id
        );
    }
    assert_eq!(mcp(GEMINI).format, Format::Json);
    assert_eq!(mcp(ANTIGRAVITY).format, Format::Json);
    assert_eq!(mcp(OPENCODE).format, Format::Jsonc);
    assert_eq!(mcp(GROK).format, Format::Toml);
    assert_eq!(servers(GEMINI), ["mcpServers"]);
    assert_eq!(servers(ANTIGRAVITY), ["mcpServers"]);
    assert_eq!(servers(OPENCODE), ["mcp"]);
    assert_eq!(servers(GROK), ["mcp_servers"]);
    // Grok reads Claude Code's and Cursor's files as compatibility sources.
    let compat: Vec<&str> = mcp(GROK)
        .caps
        .compat_reads
        .iter()
        .map(|source| source.client)
        .collect();
    assert_eq!(compat, [claude_code::ID, cursor::ID, claude_code::ID]);
}

// ---------------------------------------------------------------------------
// Mappers

#[test]
fn gemini_goldens_per_transport() {
    let mut github = stdio(
        "github",
        "npx",
        &["-y", "@modelcontextprotocol/server-github"],
        &[("GITHUB_PERSONAL_ACCESS_TOKEN", "${GITHUB_PAT}")],
    );
    stdio_mut(&mut github).cwd = Some("${HOME}/src".to_owned());
    github.tools.allow = Some(owned(&["search_code", "get_issue"]));
    github.tools.deny = Some(owned(&["delete_repository"]));
    github
        .passthrough
        .insert("timeout", NativeValue::Integer(30000));
    github.passthrough.insert("trust", NativeValue::Bool(false));
    insta::assert_snapshot!("gemini_stdio", golden(GEMINI, Os::Windows, &github));
    // No `cmd /c` shim (`[verify]` GEM-9): every OS writes the same.
    assert_eq!(
        golden(GEMINI, Os::Windows, &github),
        golden(GEMINI, Os::Linux, &github)
    );

    let tracker = remote(
        "tracker",
        Transport::Http,
        "https://tracker.example.com/mcp",
        &[("Authorization", "Bearer ${TRACKER_TOKEN}")],
    );
    insta::assert_snapshot!("gemini_http", golden(GEMINI, Os::Linux, &tracker));

    let events = remote(
        "events",
        Transport::Sse,
        "https://events.example.com/sse",
        &[],
    );
    insta::assert_snapshot!("gemini_sse", golden(GEMINI, Os::Linux, &events));

    // No native disable; a timeout must be milliseconds as a number.
    let mut disabled = stdio("disabled", "example-mcp", &[], &[]);
    disabled.enabled = false;
    assert_eq!(
        write(GEMINI, Os::Linux, &disabled).unwrap_err(),
        MapError::UnsupportedField {
            field: "enabled".to_owned()
        }
    );
    let mut slow = stdio("slow", "example-mcp", &[], &[]);
    slow.passthrough.insert("timeout", NativeValue::from("30s"));
    assert_eq!(
        write(GEMINI, Os::Linux, &slow).unwrap_err().code(),
        ErrorCode::UnexpectedShape
    );
}

#[test]
fn gemini_infers_sse_from_url_and_http_from_http_url() {
    let mapper = mapper(GEMINI);
    let read = |text: &str| mapper.from_native("x", &json(text)).unwrap().fields;

    // A bare `url` is SSE, `httpUrl` is Streamable HTTP; no `type` written.
    let sse = read(r#"{"url": "https://example.com/sse"}"#);
    assert_eq!(sse.transport, Some(Transport::Sse));
    assert_eq!(sse.url.as_deref(), Some("https://example.com/sse"));
    let http = read(r#"{"httpUrl": "https://example.com/mcp"}"#);
    assert_eq!(http.transport, Some(Transport::Http));
    assert_eq!(http.url.as_deref(), Some("https://example.com/mcp"));
    for transport in [Transport::Http, Transport::Sse] {
        let native = write(
            GEMINI,
            Os::Linux,
            &remote("x", transport, "https://example.com/mcp", &[]),
        )
        .unwrap();
        let entry = native.as_object().unwrap();
        assert!(!entry.contains_key("type"), "{native:?}");
        assert_eq!(
            entry.contains_key("httpUrl"),
            transport == Transport::Http,
            "{native:?}"
        );
        assert_eq!(
            entry.contains_key("url"),
            transport == Transport::Sse,
            "{native:?}"
        );
    }

    // An explicit `type` wins (SPEC 7.5), and Gemini's typed spellings
    // compare equal to the untyped entries Loadout writes.
    let typed_http = json(r#"{"type": "http", "url": "https://example.com/mcp"}"#);
    assert_eq!(
        mapper.from_native("x", &typed_http).unwrap().fields,
        read(r#"{"httpUrl": "https://example.com/mcp"}"#)
    );
    let ours_http = write(
        GEMINI,
        Os::Linux,
        &remote("x", Transport::Http, "https://example.com/mcp", &[]),
    )
    .unwrap();
    assert_eq!(mapper.canonical(&typed_http), mapper.canonical(&ours_http));
    let typed_sse = json(r#"{"type": "sse", "url": "https://example.com/sse"}"#);
    let ours_sse = write(
        GEMINI,
        Os::Linux,
        &remote("x", Transport::Sse, "https://example.com/sse", &[]),
    )
    .unwrap();
    assert_eq!(mapper.canonical(&typed_sse), mapper.canonical(&ours_sse));
    // ...but SSE and HTTP at the same URL differ.
    assert_ne!(
        mapper.canonical(&json(r#"{"url": "https://example.com/mcp"}"#)),
        mapper.canonical(&ours_http)
    );

    // Gemini checks `httpUrl`, then `url`, then `command` (`[verify]` GEM-3).
    let both = read(r#"{"httpUrl": "https://example.com/mcp", "url": "https://example.com/sse"}"#);
    assert_eq!(both.transport, Some(Transport::Http));
    assert_eq!(both.url.as_deref(), Some("https://example.com/mcp"));
    let with_command = read(r#"{"url": "https://example.com/sse", "command": "x"}"#);
    assert_eq!(with_command.transport, Some(Transport::Sse));

    // Unknown types and empty entries.
    assert_eq!(
        mapper
            .from_native("x", &json(r#"{"type": "ws", "url": "wss://example.com"}"#))
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedTransport
    );
    assert_eq!(
        mapper.from_native("x", &json("{}")).unwrap_err().code(),
        ErrorCode::UnexpectedShape
    );

    // Updating an entry Gemini wrote with `type` drops the now redundant key
    // and carries unknown keys over.
    let existing =
        json(r#"{"type": "http", "url": "https://old.example.com/mcp", "description": "kept"}"#);
    let updated = mapper
        .to_native(
            &remote("x", Transport::Http, "https://example.com/mcp", &[]),
            &WriteCx {
                os: Os::Linux,
                existing: Some(&existing),
            },
        )
        .unwrap();
    let entry = updated.as_object().unwrap();
    assert!(!entry.contains_key("type") && !entry.contains_key("url"));
    assert_eq!(entry.get("description"), Some(&NativeValue::from("kept")));
}

#[test]
fn antigravity_goldens_per_transport() {
    let github = stdio(
        "github",
        "npx",
        &["-y", "@modelcontextprotocol/server-github"],
        &[("GITHUB_TOOLSETS", "repos,issues")],
    );
    insta::assert_snapshot!(
        "antigravity_stdio",
        golden(ANTIGRAVITY, Os::Windows, &github)
    );
    assert_eq!(
        golden(ANTIGRAVITY, Os::Windows, &github),
        golden(ANTIGRAVITY, Os::Linux, &github)
    );

    let mut tracker = remote(
        "tracker",
        Transport::Http,
        "https://tracker.example.com/mcp",
        &[("X-Client", "example")],
    );
    tracker
        .passthrough
        .insert("timeoutSeconds", NativeValue::Integer(45));
    insta::assert_snapshot!("antigravity_http", golden(ANTIGRAVITY, Os::Linux, &tracker));
    // `serverUrl` means http (SPEC 7.5).
    let read = mapper(ANTIGRAVITY)
        .from_native("x", &json(r#"{"serverUrl": "https://example.com/mcp"}"#))
        .unwrap();
    assert_eq!(read.fields.transport, Some(Transport::Http));

    // No SSE, no `cwd`, no tool filter, no native disable.
    let blocked = |server: &RenderedServer| write(ANTIGRAVITY, Os::Linux, server).unwrap_err();
    let events = remote(
        "events",
        Transport::Sse,
        "https://events.example.com/sse",
        &[],
    );
    assert_eq!(blocked(&events).code(), ErrorCode::UnsupportedTransport);
    let mut cwd = stdio("cwd", "example-mcp", &[], &[]);
    stdio_mut(&mut cwd).cwd = Some("/srv".to_owned());
    assert_eq!(
        blocked(&cwd),
        MapError::UnsupportedField {
            field: "cwd".to_owned()
        }
    );
    let mut tools = stdio("tools", "example-mcp", &[], &[]);
    tools.tools.allow = Some(owned(&["read"]));
    assert_eq!(blocked(&tools).code(), ErrorCode::UnsupportedField);
    let mut disabled = stdio("disabled", "example-mcp", &[], &[]);
    disabled.enabled = false;
    assert_eq!(blocked(&disabled).code(), ErrorCode::UnsupportedField);
}

#[test]
fn opencode_goldens_per_transport() {
    let github = stdio(
        "github",
        "npx",
        &["-y", "@modelcontextprotocol/server-github"],
        &[("GITHUB_PERSONAL_ACCESS_TOKEN", "{env:GITHUB_PAT}")],
    );
    // A new file starts from OpenCode's `$schema` skeleton.
    insta::assert_snapshot!("opencode_stdio", golden(OPENCODE, Os::Windows, &github));
    assert_eq!(
        golden(OPENCODE, Os::Windows, &github),
        golden(OPENCODE, Os::Linux, &github)
    );

    let tracker = remote(
        "tracker",
        Transport::Http,
        "https://tracker.example.com/mcp",
        &[("Authorization", "Bearer {env:TRACKER_TOKEN}")],
    );
    insta::assert_snapshot!("opencode_http", golden(OPENCODE, Os::Linux, &tracker));

    let events = remote(
        "events",
        Transport::Sse,
        "https://events.example.com/sse",
        &[],
    );
    insta::assert_snapshot!("opencode_sse", golden(OPENCODE, Os::Linux, &events));

    // Native disable: `enabled: false`.
    let mut paused = stdio("paused", "example-mcp", &["--stdio"], &[]);
    paused.enabled = false;
    insta::assert_snapshot!("opencode_disabled", golden(OPENCODE, Os::Linux, &paused));

    // No `cwd`, no per-server tool filter.
    let mut cwd = stdio("cwd", "example-mcp", &[], &[]);
    stdio_mut(&mut cwd).cwd = Some("/srv".to_owned());
    assert_eq!(
        write(OPENCODE, Os::Linux, &cwd).unwrap_err(),
        MapError::UnsupportedField {
            field: "cwd".to_owned()
        }
    );
    let mut tools = stdio("tools", "example-mcp", &[], &[]);
    tools.tools.allow = Some(owned(&["read"]));
    assert_eq!(
        write(OPENCODE, Os::Linux, &tools).unwrap_err().code(),
        ErrorCode::UnsupportedField
    );
}

#[test]
fn opencode_command_arrays() {
    let mapper = mapper(OPENCODE);
    // `command` holds the command and its arguments in one array.
    let native = write(
        OPENCODE,
        Os::Linux,
        &stdio("x", "npx", &["-y", "example-mcp", "--flag=a b"], &[]),
    )
    .unwrap();
    let entry = native.as_object().unwrap();
    assert_eq!(entry.get("type"), Some(&NativeValue::from("local")));
    assert_eq!(
        entry.get("command"),
        Some(&NativeValue::Array(vec![
            NativeValue::from("npx"),
            NativeValue::from("-y"),
            NativeValue::from("example-mcp"),
            NativeValue::from("--flag=a b"),
        ]))
    );
    assert!(!entry.contains_key("args"));
    let def = mapper.from_native("x", &native).unwrap();
    assert_eq!(def.fields.command.as_deref(), Some("npx"));
    assert_eq!(
        def.fields.args,
        Some(owned(&["-y", "example-mcp", "--flag=a b"]))
    );
    // A command alone is a one-element array, read back with no arguments.
    let bare = write(OPENCODE, Os::Linux, &stdio("x", "example-mcp", &[], &[])).unwrap();
    assert_eq!(
        bare.as_object().unwrap().get("command"),
        Some(&NativeValue::Array(vec![NativeValue::from("example-mcp")]))
    );
    assert_eq!(
        mapper.from_native("x", &bare).unwrap().fields.args,
        Some(Vec::new())
    );
    // Anything else is not a command array.
    for text in [
        r#"{"type": "local", "command": "npx -y example-mcp"}"#,
        r#"{"type": "local", "command": []}"#,
        r#"{"type": "local", "command": ["npx", 1]}"#,
        r#"{"type": "local"}"#,
    ] {
        assert_eq!(
            mapper.from_native("x", &json(text)).unwrap_err(),
            MapError::UnexpectedShape {
                field: "command".to_owned(),
                expected: "a non-empty array of strings (the command, then its arguments)",
            },
            "{text}"
        );
    }
    // Updating keeps `environment` when the entry had one, and unknown keys.
    let existing =
        json(r#"{"type": "local", "command": ["old"], "environment": {}, "timeout": 5000}"#);
    let updated = mapper
        .to_native(
            &stdio("x", "new", &["--v2"], &[]),
            &WriteCx {
                os: Os::Linux,
                existing: Some(&existing),
            },
        )
        .unwrap();
    let entry = updated.as_object().unwrap();
    assert_eq!(
        entry.get("environment"),
        Some(&NativeValue::Object(Default::default()))
    );
    assert_eq!(entry.get("timeout"), Some(&NativeValue::Integer(5000)));
    // `enabled: true` is the default.
    let enabled = json(r#"{"type": "local", "command": ["new", "--v2"], "enabled": true}"#);
    assert_eq!(
        mapper.canonical(&enabled),
        mapper.canonical(&write(OPENCODE, Os::Linux, &stdio("x", "new", &["--v2"], &[])).unwrap())
    );
}

#[test]
fn opencode_remote_entries_are_sse_only_by_url_path() {
    let mapper = mapper(OPENCODE);
    let transport = |url: &str| {
        let text = format!(r#"{{"type": "remote", "url": "{url}"}}"#);
        mapper
            .from_native("x", &json(&text))
            .unwrap()
            .fields
            .transport
    };
    assert_eq!(transport("https://example.com/mcp"), Some(Transport::Http));
    assert_eq!(transport("https://example.com/sse"), Some(Transport::Sse));
    assert_eq!(
        transport("https://example.com/v1/sse/"),
        Some(Transport::Sse)
    );
    assert_eq!(
        transport("https://example.com/sse?token=x#top"),
        Some(Transport::Sse)
    );
    assert_eq!(
        transport("https://example.com/sse-proxy"),
        Some(Transport::Http)
    );
    assert_eq!(transport("https://sse"), Some(Transport::Http));
    assert_eq!(
        transport("https://example.com/?next=/sse"),
        Some(Transport::Http)
    );
    // Both transports are written as `remote`.
    for kind in [Transport::Http, Transport::Sse] {
        let native = write(
            OPENCODE,
            Os::Linux,
            &remote("x", kind, "https://example.com/sse", &[]),
        )
        .unwrap();
        assert_eq!(
            native.as_object().unwrap().get("type"),
            Some(&NativeValue::from("remote"))
        );
    }
    // Other types are unsupported; an untyped entry reads by its keys but
    // OpenCode rejects it.
    assert_eq!(
        mapper
            .from_native("x", &json(r#"{"type": "stdio", "command": ["x"]}"#))
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedTransport
    );
    let untyped = json(r#"{"url": "https://example.com/mcp"}"#);
    assert_eq!(
        mapper.from_native("x", &untyped).unwrap().fields.transport,
        Some(Transport::Http)
    );
    assert!(mapper.skipped_by_client(&untyped).is_some());
}

#[test]
fn grok_goldens_per_transport() {
    let mut github = stdio(
        "github",
        "npx",
        &["-y", "@modelcontextprotocol/server-github"],
        &[("GITHUB_PERSONAL_ACCESS_TOKEN", "${GITHUB_PAT}")],
    );
    github
        .passthrough
        .insert("startup_timeout_sec", NativeValue::Integer(20));
    insta::assert_snapshot!("grok_stdio", golden(GROK, Os::Windows, &github));
    assert_eq!(
        golden(GROK, Os::Windows, &github),
        golden(GROK, Os::Linux, &github)
    );

    let mut tracker = remote(
        "tracker",
        Transport::Http,
        "https://tracker.example.com/mcp",
        &[("Authorization", "Bearer ${TRACKER_TOKEN:-anonymous}")],
    );
    tracker
        .passthrough
        .insert("tool_timeout_sec", NativeValue::Float(2.5));
    insta::assert_snapshot!("grok_http", golden(GROK, Os::Linux, &tracker));

    // No SSE, no `cwd`, no tool filter, no native disable (`[verify]`
    // GRK-5).
    let blocked = |server: &RenderedServer| write(GROK, Os::Linux, server).unwrap_err();
    let events = remote(
        "events",
        Transport::Sse,
        "https://events.example.com/sse",
        &[],
    );
    assert_eq!(blocked(&events).code(), ErrorCode::UnsupportedTransport);
    let mut cwd = stdio("cwd", "example-mcp", &[], &[]);
    stdio_mut(&mut cwd).cwd = Some("/srv".to_owned());
    assert_eq!(blocked(&cwd).code(), ErrorCode::UnsupportedField);
    let mut disabled = stdio("disabled", "example-mcp", &[], &[]);
    disabled.enabled = false;
    assert_eq!(blocked(&disabled).code(), ErrorCode::UnsupportedField);
    let mut slow = stdio("slow", "example-mcp", &[], &[]);
    slow.passthrough
        .insert("startup_timeout_sec", NativeValue::Integer(-1));
    assert_eq!(blocked(&slow).code(), ErrorCode::UnexpectedShape);
}

#[test]
fn tier_c_clients_read_their_files() {
    for (spec, name, relative) in [
        (GEMINI, "gemini_read_user", "gemini/user/settings.json"),
        (
            GEMINI,
            "gemini_read_project",
            "gemini/project/.gemini/settings.json",
        ),
        (
            ANTIGRAVITY,
            "antigravity_read_user",
            "antigravity/user/mcp_config.json",
        ),
        (
            ANTIGRAVITY,
            "antigravity_read_project",
            "antigravity/project/.agents/mcp_config.json",
        ),
        (
            OPENCODE,
            "opencode_read_user",
            "opencode/user/opencode.jsonc",
        ),
        (
            OPENCODE,
            "opencode_read_project",
            "opencode/project/opencode.json",
        ),
        (GROK, "grok_read_user", "grok/user/config.toml"),
        (GROK, "grok_read_project", "grok/project/.grok/config.toml"),
    ] {
        insta::assert_snapshot!(name, read_and_rewrite(spec, relative));
    }
}

/// Writing into a client's own file keeps everything else in it: an
/// update and a new entry in each user fixture.
#[test]
fn tier_c_edits_keep_the_rest_of_the_file() {
    for (spec, relative, existing) in [
        (GEMINI, "gemini/user/settings.json", "github"),
        (ANTIGRAVITY, "antigravity/user/mcp_config.json", "github"),
        (OPENCODE, "opencode/user/opencode.jsonc", "github"),
        (GROK, "grok/user/config.toml", "github"),
    ] {
        let original = fixture(relative);
        let mut doc = parse(spec, &original);
        let container = servers(spec);
        let value = doc.get(container, existing).unwrap().unwrap();
        let mut server = stdio(
            existing,
            "npx",
            &["-y", "@modelcontextprotocol/server-github", "--read-only"],
            &[],
        );
        server.passthrough = mapper(spec)
            .from_native(existing, &value)
            .unwrap()
            .passthrough;
        let updated = mapper(spec)
            .to_native(
                &server,
                &WriteCx {
                    os: Os::Linux,
                    existing: Some(&value),
                },
            )
            .unwrap();
        assert!(doc.upsert(container, existing, &updated).unwrap());
        let added = write(spec, Os::Linux, &stdio("added", "example-mcp", &[], &[])).unwrap();
        assert!(doc.upsert(container, "added", &added).unwrap());
        let edited = text(&doc);
        let reread = parse(spec, &edited);
        let mut names = doc.list(container).unwrap();
        names.sort_unstable();
        let mut before = parse(spec, &original).list(container).unwrap();
        before.push("added".to_owned());
        before.sort_unstable();
        assert_eq!(names, before, "{relative}");
        // Entries not written are byte-for-byte the same values.
        for name in parse(spec, &original).list(container).unwrap() {
            if name != existing {
                assert_eq!(
                    reread.get(container, &name).unwrap(),
                    parse(spec, &original).get(container, &name).unwrap(),
                    "{relative}: {name}"
                );
            }
        }
        insta::assert_snapshot!(format!("{}_edit_user", spec.id.replace('-', "_")), edited);
    }
}

// ---------------------------------------------------------------------------
// Targets and detection

fn put(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn files(targets: &[Target]) -> Vec<PathBuf> {
    targets.iter().map(|target| target.file.clone()).collect()
}

fn user_files(spec: &AdapterSpec, env: &Env, fs: &dyn agent_loadout_core::Fs) -> Vec<PathBuf> {
    files(
        &spec
            .mcp_targets(env, fs, Scope::User, None, &NO_PREFS)
            .unwrap(),
    )
}

fn project_files(
    spec: &AdapterSpec,
    env: &Env,
    fs: &dyn agent_loadout_core::Fs,
    project: &Path,
) -> Vec<PathBuf> {
    files(
        &spec
            .mcp_targets(env, fs, Scope::Project, Some(project), &NO_PREFS)
            .unwrap(),
    )
}

#[test]
fn tier_c_targets() {
    let home = TempHome::new();
    let fs = home.fs();
    let project = home.projects().join("app");
    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env(os);
        let h = home.home();
        assert_eq!(
            user_files(GEMINI, &env, &fs),
            [h.join(".gemini").join("settings.json")]
        );
        assert_eq!(
            project_files(GEMINI, &env, &fs, &project),
            [project.join(".gemini").join("settings.json")]
        );
        assert_eq!(
            user_files(ANTIGRAVITY, &env, &fs),
            [h.join(".gemini").join("config").join("mcp_config.json")]
        );
        assert_eq!(
            project_files(ANTIGRAVITY, &env, &fs, &project),
            [project.join(".agents").join("mcp_config.json")]
        );
        // `~/.config` on every OS, Windows and macOS included.
        assert_eq!(
            user_files(OPENCODE, &env, &fs),
            [h.join(".config").join("opencode").join("opencode.json")]
        );
        assert_eq!(
            project_files(OPENCODE, &env, &fs, &project),
            [project.join("opencode.json")]
        );
        assert_eq!(
            user_files(GROK, &env, &fs),
            [h.join(".grok").join("config.toml")]
        );
        assert_eq!(
            project_files(GROK, &env, &fs, &project),
            [project.join(".grok").join("config.toml")]
        );
        for spec in TIER_C {
            let err = spec
                .mcp_targets(&env, &fs, Scope::ProjectLocal, Some(&project), &NO_PREFS)
                .unwrap_err();
            assert_eq!(err.code(), ErrorCode::ScopeNotSupported, "{}", spec.id);
        }
    }

    // OpenCode: $XDG_CONFIG_HOME when absolute, on Windows too.
    let xdg = home.xdg();
    let env = home
        .env_builder(Os::Windows)
        .var("XDG_CONFIG_HOME", &xdg)
        .build();
    assert_eq!(
        user_files(OPENCODE, &env, &fs),
        [xdg.join("opencode").join("opencode.json")]
    );
    // An existing `opencode.jsonc` is used when there is no `opencode.json`.
    let jsonc = xdg.join("opencode").join("opencode.jsonc");
    put(&jsonc, "{}\n");
    assert_eq!(
        user_files(OPENCODE, &env, &fs),
        std::slice::from_ref(&jsonc)
    );
    put(&xdg.join("opencode").join("opencode.json"), "{}\n");
    assert_eq!(
        user_files(OPENCODE, &env, &fs),
        [xdg.join("opencode").join("opencode.json")]
    );
    // Project: `opencode.jsonc`, then `.opencode/`, when they exist.
    let nested = project.join(".opencode").join("opencode.jsonc");
    put(&nested, "{}\n");
    assert_eq!(
        project_files(OPENCODE, &env, &fs, &project),
        std::slice::from_ref(&nested)
    );
    put(&project.join("opencode.jsonc"), "{}\n");
    assert_eq!(
        project_files(OPENCODE, &env, &fs, &project),
        [project.join("opencode.jsonc")]
    );
    // OpenCode's skeleton is the text of a new file.
    let target = OPENCODE
        .mcp_targets(&env, &fs, Scope::User, None, &NO_PREFS)
        .unwrap()
        .swap_remove(0);
    assert_eq!(text(&target.new_document()), opencode::SKELETON);

    // Gemini hides user servers in the project file's `mcp.excluded`.
    let suppression = GEMINI
        .suppression_target(&env, &fs, &project, &NO_PREFS)
        .unwrap()
        .unwrap();
    assert_eq!(suppression.scope, Scope::Project);
    assert_eq!(
        suppression.file,
        project.join(".gemini").join("settings.json")
    );
    assert_eq!(suppression.container.segments(), ["mcp", "excluded"]);
    // OpenCode upserts the disabled entry in the project file.
    let suppression = OPENCODE
        .suppression_target(&env, &fs, &project, &NO_PREFS)
        .unwrap()
        .unwrap();
    assert_eq!(suppression.file, project.join("opencode.jsonc"));
    assert_eq!(suppression.container.segments(), ["mcp"]);
    for spec in [ANTIGRAVITY, GROK] {
        assert!(
            spec.suppression_target(&env, &fs, &project, &NO_PREFS)
                .unwrap()
                .is_none(),
            "{}",
            spec.id
        );
    }
}

#[test]
fn tier_c_detection_per_os() {
    let home = TempHome::new();
    let fs = home.fs();
    let registry = NoRegistry;
    let evidence =
        |spec: &AdapterSpec, env: &Env| detect::detect(spec, env, &fs, &registry).evidence;

    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env_builder(os).var("PATH", home.bin()).build();
        for spec in TIER_C {
            let found = detect::detect(spec, &env, &fs, &registry);
            assert!(found.supported, "{} on {os:?}", spec.id);
            assert!(!found.detected(), "{} on {os:?}", spec.id);
        }
    }

    // Antigravity's dir under `~/.gemini` does not detect Gemini.
    let antigravity_dir = home.home().join(".gemini").join("config");
    std::fs::create_dir_all(&antigravity_dir).unwrap();
    let grok_dir = home.home().join(".grok");
    std::fs::create_dir_all(&grok_dir).unwrap();
    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env(os);
        assert!(evidence(GEMINI, &env).is_empty(), "{os:?}");
        assert_eq!(
            evidence(ANTIGRAVITY, &env),
            [Evidence::Path(antigravity_dir.clone())]
        );
        assert_eq!(evidence(GROK, &env), [Evidence::Path(grok_dir.clone())]);
        assert!(evidence(OPENCODE, &env).is_empty(), "{os:?}");
    }
    let settings = home.home().join(".gemini").join("settings.json");
    put(&settings, "{}\n");
    let opencode_dir = home.home().join(".config").join("opencode");
    std::fs::create_dir_all(&opencode_dir).unwrap();
    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env(os);
        assert_eq!(evidence(GEMINI, &env), [Evidence::Path(settings.clone())]);
        assert_eq!(
            evidence(OPENCODE, &env),
            [Evidence::Path(opencode_dir.clone())]
        );
    }

    // Executables on a Windows PATH, through PATHEXT.
    for name in ["gemini.cmd", "antigravity.exe", "opencode.exe", "grok.cmd"] {
        put(&home.bin().join(name), "");
    }
    let env = home
        .env_builder(Os::Windows)
        .var("PATH", home.bin())
        .var("PATHEXT", ".EXE;.CMD")
        .build();
    for (spec, file) in [
        (GEMINI, "gemini.cmd"),
        (ANTIGRAVITY, "antigravity.exe"),
        (OPENCODE, "opencode.exe"),
        (GROK, "grok.cmd"),
    ] {
        assert!(
            evidence(spec, &env).contains(&Evidence::Executable(home.bin().join(file))),
            "{}",
            spec.id
        );
    }
}

// ---------------------------------------------------------------------------
// Through the apply engine

const CLIENTS: &str = "[\"gemini\", \"antigravity\", \"opencode\", \"grok\"]";

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap();
    native_json(&NativeValue::parse_json(&text).unwrap())
}

/// A native value as `serde_json` (JSONC files have comments).
fn native_json(value: &NativeValue) -> Value {
    match value {
        NativeValue::Null => Value::Null,
        NativeValue::Bool(value) => Value::Bool(*value),
        NativeValue::Integer(value) => Value::from(*value),
        NativeValue::Float(value) => Value::from(*value),
        NativeValue::String(value) | NativeValue::Datetime(value) => Value::from(value.as_str()),
        NativeValue::Array(items) => Value::Array(items.iter().map(native_json).collect()),
        NativeValue::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.to_owned(), native_json(value)))
                .collect(),
        ),
    }
}

fn read_toml(path: &Path) -> toml::Value {
    toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A project `<root>/projects/<name>` with `project.toml` holding `body`
/// (and `project.local.toml` holding `local`, when set), known on this
/// machine and trusted.
fn project(fx: &Fx, name: &str, id: &str, body: &str, local: Option<&str>) -> PathBuf {
    let root = fx.th.projects().join(name);
    std::fs::create_dir_all(&root).unwrap();
    fx.manifest(
        &root,
        SHARED_FILE,
        &format!("schema = 1\nid = \"{id}\"\nname = \"{name}\"\n\n{body}"),
    );
    if let Some(local) = local {
        fx.manifest(&root, LOCAL_FILE, &format!("schema = 1\n\n{local}"));
    }
    fx.know_projects(&[(id, &root)]);
    fx.trust(&root);
    root
}

/// The user and project file of each Tier C client.
fn tier_c_files(fx: &Fx, root: &Path) -> Vec<(&'static str, PathBuf)> {
    let mut out = Vec::new();
    for spec in TIER_C {
        let user = spec
            .mcp_targets(&fx.env, &fx.fs, Scope::User, None, &NO_PREFS)
            .unwrap()
            .swap_remove(0)
            .file;
        let shared = spec
            .mcp_targets(&fx.env, &fx.fs, Scope::Project, Some(root), &NO_PREFS)
            .unwrap()
            .swap_remove(0)
            .file;
        out.push((spec.id, user));
        out.push((spec.id, shared));
    }
    out
}

#[test]
fn tier_c_user_and_project_files_through_the_apply_engine() {
    let fx = Fx::new();
    fx.edit("servers", |tx| {
        tx.write(
            "servers/github.toml",
            format!(
                "schema = 1\nid = \"github\"\ntransport = \"stdio\"\ncommand = \"npx\"\n\
                 args = [\"-y\", \"@modelcontextprotocol/server-github\"]\n\
                 [env]\nGITHUB_TOOLSETS = \"repos,issues\"\n[enable]\nclients = {CLIENTS}\n"
            ),
        )?;
        tx.write(
            "servers/tracker.toml",
            format!(
                "schema = 1\nid = \"tracker\"\ntransport = \"http\"\n\
                 url = \"https://tracker.example.test/mcp\"\n\
                 [headers]\nX-Client = \"example\"\n[enable]\nclients = {CLIENTS}\n"
            ),
        )
    });
    let root = project(
        &fx,
        "tier-c",
        "tier-c-a1b2",
        &format!(
            "[defaults]\nclients = {CLIENTS}\n\n\
             [servers.repo-docs]\ntransport = \"http\"\nurl = \"https://docs.example.test/mcp\"\n\n\
             [servers.repo-tools]\ntransport = \"stdio\"\ncommand = \"node\"\nargs = [\"tools/mcp.js\"]\n"
        ),
        None,
    );
    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");

    let mut out = String::new();
    for (client, file) in tier_c_files(&fx, &root) {
        let shown = file
            .strip_prefix(&root)
            .map(|rel| format!("<P>/{}", rel.display()))
            .or_else(|_| {
                file.strip_prefix(fx.th.home())
                    .map(|rel| format!("~/{}", rel.display()))
            })
            .unwrap()
            .replace('\\', "/");
        out.push_str(&format!(
            "===== {client}: {shown}\n{}\n",
            fx.read(&file).replace("\r\n", "\n")
        ));
    }
    insta::assert_snapshot!("engine_user_and_project_files", out);

    // Each file reads back as what was written: a second run has no work.
    assert!(!fx.plan().has_work(), "{:#?}", fx.plan());
    // The ledger owns every entry, in its scope.
    let ledger = fx.ledger();
    for spec in TIER_C {
        let mut keys: Vec<(&str, Scope)> = ledger
            .entries
            .iter()
            .filter(|row| row.client == spec.id && row.kind == EntryKind::McpEntry)
            .map(|row| (row.key.as_str(), row.scope))
            .collect();
        keys.sort_unstable_by(|a, b| a.0.cmp(b.0));
        assert_eq!(
            keys,
            [
                ("github", Scope::User),
                ("repo-docs", Scope::Project),
                ("repo-tools", Scope::Project),
                ("tracker", Scope::User),
            ],
            "{}",
            spec.id
        );
    }

    // Removing a server removes it from both user files it went to.
    fx.remove_server("tracker");
    let (report, _) = fx.apply();
    assert!(report.writes() > 0, "{report:#?}");
    let gemini_user = fx.th.home().join(".gemini").join("settings.json");
    assert!(
        read_json(&gemini_user)["mcpServers"]
            .get("tracker")
            .is_none()
    );
    let grok_user = fx.th.home().join(".grok").join("config.toml");
    assert!(
        read_toml(&grok_user)["mcp_servers"]
            .get("tracker")
            .is_none()
    );
}

#[test]
fn env_references_use_each_clients_syntax() {
    let fx = Fx::new();
    fx.edit("servers", |tx| {
        tx.write(
            "servers/tools.toml",
            "schema = 1\nid = \"tools\"\ntransport = \"stdio\"\ncommand = \"${env:TOOLS_BIN}/tools-mcp\"\n\
             args = [\"--root=${env:WORK_ROOT}\"]\n[env]\nEXAMPLE_TOKEN = \"${env:EXAMPLE_TOKEN}\"\n\
             [enable]\nclients = [\"gemini\", \"opencode\"]\n",
        )?;
        tx.write(
            "servers/envonly.toml",
            "schema = 1\nid = \"envonly\"\ntransport = \"stdio\"\ncommand = \"envonly-mcp\"\n\
             [env]\nEXAMPLE_TOKEN = \"${env:EXAMPLE_TOKEN}\"\n[enable]\nclients = [\"grok\"]\n",
        )?;
        tx.write(
            "servers/tracker.toml",
            "schema = 1\nid = \"tracker\"\ntransport = \"http\"\nurl = \"https://tracker.example.test/mcp\"\n\
             [headers]\nAuthorization = \"Bearer ${env:TRACKER_TOKEN}\"\n\
             [enable]\nclients = [\"gemini\", \"opencode\", \"grok\"]\n",
        )
    });
    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");

    let gemini_file = read_json(&fx.th.home().join(".gemini").join("settings.json"));
    let tools = &gemini_file["mcpServers"]["tools"];
    assert_eq!(tools["command"], "${TOOLS_BIN}/tools-mcp");
    assert_eq!(tools["args"][0], "--root=${WORK_ROOT}");
    assert_eq!(tools["env"]["EXAMPLE_TOKEN"], "${EXAMPLE_TOKEN}");
    let tracker = &gemini_file["mcpServers"]["tracker"];
    assert_eq!(tracker["httpUrl"], "https://tracker.example.test/mcp");
    assert_eq!(
        tracker["headers"]["Authorization"],
        "Bearer ${TRACKER_TOKEN}"
    );

    let opencode_file = read_json(
        &fx.th
            .home()
            .join(".config")
            .join("opencode")
            .join("opencode.json"),
    );
    assert_eq!(opencode_file["$schema"], "https://opencode.ai/config.json");
    let tools = &opencode_file["mcp"]["tools"];
    assert_eq!(tools["type"], "local");
    assert_eq!(tools["command"][0], "{env:TOOLS_BIN}/tools-mcp");
    assert_eq!(tools["command"][1], "--root={env:WORK_ROOT}");
    assert_eq!(tools["environment"]["EXAMPLE_TOKEN"], "{env:EXAMPLE_TOKEN}");
    assert_eq!(
        opencode_file["mcp"]["tracker"]["headers"]["Authorization"],
        "Bearer {env:TRACKER_TOKEN}"
    );

    let grok_file = read_toml(&fx.th.home().join(".grok").join("config.toml"));
    assert_eq!(
        grok_file["mcp_servers"]["envonly"]["env"]["EXAMPLE_TOKEN"].as_str(),
        Some("${EXAMPLE_TOKEN}")
    );
    assert_eq!(
        grok_file["mcp_servers"]["tracker"]["headers"]["Authorization"].as_str(),
        Some("Bearer ${TRACKER_TOKEN}")
    );
    // A second run reads every reference back as written.
    assert!(!fx.plan().has_work(), "{:#?}", fx.plan());
}

fn row<'a>(doc: &'a Value, client: &str, name: &str) -> &'a Value {
    doc["clients"][client]
        .as_array()
        .unwrap_or_else(|| panic!("no {client} in {doc:#}"))
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("no {client}/{name} in {doc:#}"))
}

fn view(fx: &Fx, folder: &Path, clients: Option<&[&str]>) -> Value {
    let store = fx.store();
    let ids: Option<Vec<String>> =
        clients.map(|ids| ids.iter().map(|id| (*id).to_owned()).collect());
    let view: EffectiveView = effective(Some(&store), &fx.ax(), folder, ids.as_deref()).unwrap();
    serde_json::to_value(&view).unwrap()
}

fn warning_codes(row: &Value) -> Vec<&str> {
    row["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|warning| warning["code"].as_str().unwrap())
        .collect()
}

#[test]
fn gemini_hides_a_user_server_through_mcp_excluded() {
    let fx = Fx::new();
    fx.put_server(store_stdio("slack", &["gemini"], &["-y", "slack-mcp"]));
    let manifest = "[servers.slack]\nenabled = false\nclients = [\"gemini\"]\n";
    let root = project(&fx, "quiet", "quiet-c3d4", manifest, None);
    let settings = root.join(".gemini").join("settings.json");
    // Gemini's own settings in the project file stay.
    fx.write(
        &settings,
        "{\n  \"ui\": {\n    \"theme\": \"GitHub\"\n  }\n}\n",
    );

    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");
    let user = read_json(&fx.th.home().join(".gemini").join("settings.json"));
    assert_eq!(user["mcpServers"]["slack"]["command"], "npx");
    let project_file = read_json(&settings);
    assert_eq!(
        project_file["mcp"]["excluded"],
        serde_json::json!(["slack"])
    );
    assert_eq!(project_file["ui"]["theme"], "GitHub");
    assert!(project_file.get("mcpServers").is_none(), "{project_file:#}");
    insta::assert_snapshot!(
        "gemini_project_excluded",
        fx.read(&settings).replace("\r\n", "\n")
    );
    let row_of = fx
        .ledger()
        .entries
        .into_iter()
        .find(|row| row.kind == EntryKind::ListMember)
        .unwrap();
    assert_eq!(row_of.client, "gemini");
    assert_eq!(row_of.key, "slack");
    assert_eq!(row_of.container, "/mcp/excluded");
    assert!(!fx.plan().has_work(), "{:#?}", fx.plan());

    // The effective view: loaded outside the project, hidden inside.
    let doc = view(&fx, &root, Some(&["gemini"]));
    let slack = row(&doc, "gemini", "slack");
    assert_eq!(slack["loaded"], false, "{slack:#}");
    assert_eq!(slack["winner"]["layer"], "system");
    // A folder in the home directory: discovery never searches above it.
    let outside = fx.th.home().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    let doc = view(&fx, &outside, Some(&["gemini"]));
    assert_eq!(row(&doc, "gemini", "slack")["loaded"], true);

    // Dropping the suppression takes the name off the list.
    fx.manifest(
        &root,
        SHARED_FILE,
        "schema = 1\nid = \"quiet-c3d4\"\nname = \"quiet\"\n",
    );
    fx.trust(&root);
    let (report, _) = fx.apply();
    assert!(report.writes() > 0, "{report:#?}");
    let project_file = read_json(&settings);
    assert_eq!(project_file["mcp"]["excluded"], serde_json::json!([]));
    assert_eq!(project_file["ui"]["theme"], "GitHub");
    let doc = view(&fx, &root, Some(&["gemini"]));
    assert_eq!(row(&doc, "gemini", "slack")["loaded"], true);
}

#[test]
fn a_private_gemini_suppression_stays_out_of_git() {
    let fx = Fx::new();
    fx.put_server(store_stdio("slack", &["gemini"], &["-y", "slack-mcp"]));
    let local = "[servers.slack]\nenabled = false\nclients = [\"gemini\"]\n";
    let root = fx.th.projects().join("private");
    std::fs::create_dir_all(&root).unwrap();
    let repo = git_init(&root);
    let root = project(&fx, "private", "private-e5f6", "", Some(local));
    let settings = root.join(".gemini").join("settings.json");

    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");
    assert_eq!(
        read_json(&settings)["mcp"]["excluded"],
        serde_json::json!(["slack"])
    );
    let exclude = std::fs::read_to_string(repo.path().join("info").join("exclude")).unwrap();
    assert!(
        exclude.lines().any(|line| line == "/.gemini/settings.json"),
        "{exclude}"
    );

    // A tracked project file never gets a private member.
    let fx = Fx::new();
    fx.put_server(store_stdio("slack", &["gemini"], &["-y", "slack-mcp"]));
    let root = fx.th.projects().join("tracked");
    let settings = root.join(".gemini").join("settings.json");
    fx.write(&settings, "{}\n");
    let repo = git_init(&root);
    git_commit(&repo, ".gemini/settings.json");
    let root = project(&fx, "tracked", "tracked-a7b8", "", Some(local));
    let plan: Plan = fx.plan();
    assert!(
        plan.warnings
            .iter()
            .any(|warning| warning.code() == ErrorCode::PrivateInTrackedFile),
        "{plan:#?}"
    );
    assert!(plan.targets.iter().all(|target| target.file != settings));
    fx.apply();
    assert_eq!(fx.read(&root.join(".gemini").join("settings.json")), "{}\n");
}

#[test]
fn opencode_hides_a_user_server_with_enabled_false() {
    let fx = Fx::new();
    fx.put_server(store_stdio("slack", &["opencode"], &["-y", "slack-mcp"]));
    let root = project(
        &fx,
        "quiet",
        "quiet-c9d0",
        "[servers.slack]\nenabled = false\nclients = [\"opencode\"]\n",
        None,
    );
    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");
    // The whole entry, disabled: OpenCode validates each file alone.
    let slack = &read_json(&root.join("opencode.json"))["mcp"]["slack"];
    assert_eq!(slack["type"], "local");
    assert_eq!(
        slack["command"],
        serde_json::json!(["npx", "-y", "slack-mcp"])
    );
    assert_eq!(slack["enabled"], false);
    let user = read_json(
        &fx.th
            .home()
            .join(".config")
            .join("opencode")
            .join("opencode.json"),
    );
    assert!(user["mcp"]["slack"].get("enabled").is_none());
    assert!(!fx.plan().has_work(), "{:#?}", fx.plan());

    let doc = view(&fx, &root, Some(&["opencode"]));
    let slack = row(&doc, "opencode", "slack");
    assert_eq!(slack["loaded"], false, "{slack:#}");
    assert_eq!(slack["winner"]["layer"], "project");
    assert_eq!(slack["winner"]["source"], "store:slack+project");
}

#[test]
fn grok_and_antigravity_cannot_suppress_in_project() {
    let fx = Fx::new();
    fx.put_server(store_stdio(
        "slack",
        &["grok", "antigravity"],
        &["-y", "slack-mcp"],
    ));
    let root = project(
        &fx,
        "loud",
        "loud-e1f2",
        "[servers.slack]\nenabled = false\nclients = [\"grok\", \"antigravity\"]\n",
        None,
    );
    let plan = fx.plan();
    let refusals: Vec<&str> = plan
        .warnings
        .iter()
        .filter(|warning| warning.code() == ErrorCode::CannotSuppressInProject)
        .map(|warning| warning.message())
        .collect();
    assert_eq!(refusals.len(), 2, "{plan:#?}");
    assert!(
        refusals
            .iter()
            .any(|message| message.contains("Grok Build"))
    );
    assert!(
        refusals
            .iter()
            .any(|message| message.contains("Antigravity"))
    );
    assert!(
        plan.targets
            .iter()
            .all(|target| target.project_id.is_none())
    );

    // The effective view says why the server still loads.
    fx.apply();
    for client in ["grok", "antigravity"] {
        let doc = view(&fx, &root, Some(&[client]));
        let slack = row(&doc, client, "slack");
        assert_eq!(slack["loaded"], true, "{slack:#}");
        assert!(
            warning_codes(slack).contains(&"cannot_suppress_in_project"),
            "{slack:#}"
        );
    }
}

#[test]
fn grok_compat_duplicates_are_flagged_in_the_effective_view() {
    let fx = Fx::new();
    fx.put_server(store_stdio("github", &["grok"], &["-y", "github-mcp"]));
    let root = project(
        &fx,
        "grok-compat",
        "grok-compat-a3b4",
        "[servers.repo-docs]\ntransport = \"http\"\nurl = \"https://docs.example.test/mcp\"\n\
         clients = [\"grok\"]\n",
        None,
    );
    // Claude Code's user file, its project file, and Cursor's project file.
    fx.write(
        &fx.claude(),
        r#"{ "mcpServers": { "claude-user": { "type": "stdio", "command": "claude-user-mcp" },
            "github": { "type": "stdio", "command": "npx", "args": ["-y", "github-mcp"] } } }"#,
    );
    fx.write(
        &root.join(".mcp.json"),
        r#"{ "mcpServers": { "repo-docs": { "type": "http", "url": "https://docs.example.test/mcp" },
            "mcp-only": { "type": "stdio", "command": "mcp-only" } } }"#,
    );
    fx.write(
        &root.join(".cursor").join("mcp.json"),
        r#"{ "mcpServers": { "cursor-only": { "command": "cursor-only-mcp" },
            "mcp-only": { "command": "cursor-copy" } } }"#,
    );
    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");
    // Grok's own files never get the compat entries.
    let grok_project = read_toml(&root.join(".grok").join("config.toml"));
    let names: Vec<&String> = grok_project["mcp_servers"]
        .as_table()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(names, ["repo-docs"]);

    let doc = view(&fx, &root, Some(&["grok"]));
    // A native project entry also in `.mcp.json`.
    let docs = row(&doc, "grok", "repo-docs");
    assert_eq!(docs["winner"]["layer"], "project");
    assert_eq!(docs["winner"]["file"], "<P>/.grok/config.toml");
    assert_eq!(docs["shadowed"][0]["layer"], "client-compat:<P>/.mcp.json");
    assert_eq!(warning_codes(docs), ["duplicate_via_compat"]);
    // A native user entry also in `~/.claude.json`.
    let github = row(&doc, "grok", "github");
    assert_eq!(github["winner"]["layer"], "system");
    assert_eq!(github["winner"]["file"], "~/.grok/config.toml");
    assert_eq!(
        github["shadowed"][0]["layer"],
        "client-compat:~/.claude.json"
    );
    assert_eq!(warning_codes(github), ["duplicate_via_compat"]);
    // Compat-only names win from their file, without a warning; between
    // compat files the first source wins (`.cursor/mcp.json` before
    // `.mcp.json`, `[verify]` GRK-4).
    let claude_user = row(&doc, "grok", "claude-user");
    assert_eq!(
        claude_user["winner"]["layer"],
        "client-compat:~/.claude.json"
    );
    assert_eq!(claude_user["loaded"], true);
    assert!(warning_codes(claude_user).is_empty(), "{claude_user:#}");
    assert_eq!(
        row(&doc, "grok", "cursor-only")["winner"]["layer"],
        "client-compat:<P>/.cursor/mcp.json"
    );
    let mcp_only = row(&doc, "grok", "mcp-only");
    assert_eq!(
        mcp_only["winner"]["layer"],
        "client-compat:<P>/.cursor/mcp.json"
    );
    assert_eq!(
        mcp_only["shadowed"][0]["layer"],
        "client-compat:<P>/.mcp.json"
    );
    assert!(warning_codes(mcp_only).is_empty(), "{mcp_only:#}");

    // The other clients see Grok as a reader of their files.
    let doc = view(&fx, &root, Some(&["claude-code", "cursor"]));
    for (client, name) in [
        ("claude-code", "repo-docs"),
        ("claude-code", "claude-user"),
        ("cursor", "cursor-only"),
    ] {
        let also = &row(&doc, client, name)["also_visible_to"];
        assert!(
            also.as_array().unwrap().iter().any(|v| v == "grok"),
            "{client}/{name}: {also}"
        );
    }
    // Every Tier C client has a row in the full view.
    let doc = view(&fx, &root, None);
    let clients = doc["clients"].as_object().unwrap();
    for spec in TIER_C {
        assert!(clients.contains_key(spec.id), "{}", spec.id);
    }
}

#[test]
fn opencode_project_entries_land_in_an_existing_jsonc_file() {
    let fx = Fx::new();
    let root = fx.th.projects().join("jsonc");
    let file = root.join("opencode.jsonc");
    fx.write(
        &file,
        "{\n  // project settings\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"model\": \"example/model\",\n}\n",
    );
    let root = project(
        &fx,
        "jsonc",
        "jsonc-c5d6",
        "[servers.repo-tools]\ntransport = \"stdio\"\ncommand = \"node\"\nargs = [\"tools/mcp.js\"]\n\
         clients = [\"opencode\"]\n",
        None,
    );
    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");
    assert!(!root.join("opencode.json").exists());
    let text = fx.read(&file).replace("\r\n", "\n");
    assert!(text.contains("// project settings"), "{text}");
    insta::assert_snapshot!("opencode_project_jsonc", text);
    let entry = support::entry(&fx.plan(), &file, "repo-tools").state;
    assert_eq!(entry, EntryState::Synced);
}

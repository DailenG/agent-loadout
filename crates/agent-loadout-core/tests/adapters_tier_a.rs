//! Codex, omp and Pi adapters (SPEC 7.3, 7.4 rows `codex`, `omp`, `pi`):
//! `to_native`/`from_native` goldens per transport, the Codex legacy
//! `[mcp.servers]` table (SkillDock deleted it), omp profiles from
//! `clients.toml` and its `$schema` skeleton, timeout units, project
//! suppression, Pi name collisions and `pi-mcp-adapter`, targets and
//! detection through a fake `Env`.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use agent_loadout_core::adapters::detect::{self, Evidence, RegistryProbe};
use agent_loadout_core::adapters::mapper::{
    EntryMapper, Launch, MapError, RemoteLaunch, RenderedServer, ServerDef, StdioLaunch, ToolLists,
    WriteCx,
};
use agent_loadout_core::adapters::{
    AdapterSpec, McpSpec, claude_code, claude_desktop, codex, omp, pi,
};
use agent_loadout_core::formats::{Document, Format, NativeValue};
use agent_loadout_core::model::applied::Scope;
use agent_loadout_core::model::clients::{ClientPrefs, ClientPrefsFile};
use agent_loadout_core::model::server::{ServerFields, Transport};
use agent_loadout_core::{Env, ErrorCode, Fs, Os};
use agent_loadout_testkit::{RootedFs, TempHome};

const CODEX: &AdapterSpec = &codex::SPEC;
const OMP: &AdapterSpec = &omp::SPEC;
const PI: &AdapterSpec = &pi::SPEC;
const CODE: &AdapterSpec = &claude_code::SPEC;
const DESKTOP: &AdapterSpec = &claude_desktop::SPEC;
const TIER_A: [&AdapterSpec; 3] = [CODEX, OMP, PI];

/// No `clients.toml` preferences.
static NO_PREFS: LazyLock<ClientPrefs> = LazyLock::new(ClientPrefs::default);

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
        .join("clients")
}

/// A fixture with LF line endings, whatever the checkout did to it.
fn fixture(relative: &str) -> String {
    let path = fixtures().join(relative);
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

fn remote_mut(server: &mut RenderedServer) -> &mut RemoteLaunch {
    match &mut server.launch {
        Launch::Http(remote) | Launch::Sse(remote) => remote,
        Launch::Stdio(_) => unreachable!("a remote server"),
    }
}

fn write(spec: &AdapterSpec, os: Os, server: &RenderedServer) -> Result<NativeValue, MapError> {
    mapper(spec).to_native(server, &WriteCx { os, existing: None })
}

fn update(
    spec: &AdapterSpec,
    existing: &NativeValue,
    server: &RenderedServer,
) -> Result<NativeValue, MapError> {
    mapper(spec).to_native(
        server,
        &WriteCx {
            os: Os::Linux,
            existing: Some(existing),
        },
    )
}

/// The store fields a rendered server reads back as. Variables a client
/// forwards by name read back as `${env:NAME}` references.
fn fields_of(server: &RenderedServer) -> ServerFields {
    let env_ref = |name: &str| format!("${{env:{name}}}");
    let non_empty = |map: BTreeMap<String, String>| (!map.is_empty()).then_some(map);
    match &server.launch {
        Launch::Stdio(stdio) => {
            let mut env: BTreeMap<String, String> = stdio
                .forward_env
                .iter()
                .map(|name| (name.clone(), env_ref(name)))
                .collect();
            env.extend(stdio.env.clone());
            ServerFields {
                transport: Some(Transport::Stdio),
                command: Some(stdio.command.clone()),
                args: Some(stdio.args.clone()),
                cwd: stdio.cwd.clone(),
                env: non_empty(env),
                ..ServerFields::default()
            }
        }
        Launch::Http(remote) | Launch::Sse(remote) => {
            let mut headers = BTreeMap::new();
            if let Some(name) = &remote.bearer_env {
                headers.insert(
                    "Authorization".to_owned(),
                    format!("Bearer {}", env_ref(name)),
                );
            }
            for (header, name) in &remote.headers_env {
                headers.insert(header.clone(), env_ref(name));
            }
            headers.extend(remote.headers.clone());
            ServerFields {
                transport: Some(server.launch.transport()),
                url: Some(remote.url.clone()),
                headers: non_empty(headers),
                ..ServerFields::default()
            }
        }
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

/// The file a new entry makes (starting from the client's skeleton), and
/// what it reads back as. Checks the round trip.
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
    assert_eq!(def.disabled, !server.enabled);
    assert_eq!(def.tools, server.tools);
    assert_eq!(def.passthrough, server.passthrough);
    assert!(def.unmapped.is_empty(), "{:?}", def.unmapped);
    format!("{}\n# read back\n{}", text(&doc), describe(&def))
}

/// Every entry of a container read back, or the error it gives.
fn read_all(spec: &AdapterSpec, doc: &Document, container: &[&str]) -> String {
    let mut out = String::new();
    for key in doc.list(container).unwrap() {
        let value = doc.get(container, &key).unwrap().unwrap();
        out.push_str(&format!("## {key}\n"));
        match mapper(spec).from_native(&key, &value) {
            Ok(def) => out.push_str(&describe(&def)),
            Err(err) => out.push_str(&format!("error[{}]: {err}\n", err.code())),
        }
    }
    out
}

/// Lines that differ between two texts of the same length.
fn changed_lines<'a>(before: &'a str, after: &'a str) -> Vec<(&'a str, &'a str)> {
    assert_eq!(before.lines().count(), after.lines().count());
    before
        .lines()
        .zip(after.lines())
        .filter(|(a, b)| a != b)
        .collect()
}

// ---------------------------------------------------------------------------
// Mappers

#[test]
fn codex_goldens_per_transport() {
    let mut docs = stdio(
        "docs",
        "npx",
        &["-y", "example-docs-mcp"],
        &[("DOCS_ROOT", "/srv/example/docs")],
    );
    stdio_mut(&mut docs).forward_env = owned(&["DOCS_TOKEN"]);
    stdio_mut(&mut docs).cwd = Some("/srv/example".to_owned());
    docs.tools.deny = Some(owned(&["delete_page"]));
    docs.passthrough
        .insert("startup_timeout_sec", NativeValue::Integer(20));
    insta::assert_snapshot!("codex_stdio", golden(CODEX, Os::Windows, &docs));
    // Codex gets no `cmd /c` shim (`[verify]`), so every OS writes the same.
    assert_eq!(
        golden(CODEX, Os::Windows, &docs),
        golden(CODEX, Os::Linux, &docs)
    );
    let bare = stdio("bare", "example-local-mcp", &[], &[]);
    insta::assert_snapshot!("codex_stdio_bare", golden(CODEX, Os::Linux, &bare));

    let mut tracker = remote(
        "tracker",
        Transport::Http,
        "https://tracker.example.com/mcp",
        &[("X-Client", "example")],
    );
    remote_mut(&mut tracker).bearer_env = Some("TRACKER_TOKEN".to_owned());
    remote_mut(&mut tracker).headers_env = map(&[("X-Workspace", "TRACKER_WORKSPACE")]);
    tracker.tools.allow = Some(owned(&["search", "read"]));
    tracker.enabled = false;
    insta::assert_snapshot!("codex_http", golden(CODEX, Os::Linux, &tracker));

    // No SSE in Codex.
    let events = remote(
        "events",
        Transport::Sse,
        "https://events.example.com/sse",
        &[],
    );
    let err = write(CODEX, Os::Linux, &events).unwrap_err();
    assert_eq!(
        err,
        MapError::UnsupportedTransport {
            transport: "sse".to_owned()
        }
    );
    assert_eq!(err.code().as_str(), "unsupported_transport");
}

#[test]
fn omp_goldens_per_transport() {
    let mut notes = stdio(
        "notes",
        "npx",
        &["-y", "example-notes-mcp"],
        &[(
            "NOTES_TOKEN",
            r#"!"/opt/example/bin/loadout" secret get --ref example"#,
        )],
    );
    stdio_mut(&mut notes).cwd = Some("${HOME}/notes".to_owned());
    notes
        .passthrough
        .insert("timeout", NativeValue::Integer(30000));
    notes
        .passthrough
        .insert("instructions", NativeValue::Bool(false));
    // A new file starts with omp's `$schema` line.
    insta::assert_snapshot!("omp_stdio", golden(OMP, Os::Windows, &notes));
    assert_eq!(
        golden(OMP, Os::Windows, &notes),
        golden(OMP, Os::Linux, &notes)
    );

    let search = remote(
        "search",
        Transport::Http,
        "https://search.example.com/mcp",
        &[("Authorization", "Bearer ${SEARCH_TOKEN}")],
    );
    insta::assert_snapshot!("omp_http", golden(OMP, Os::Linux, &search));

    let mut events = remote(
        "events",
        Transport::Sse,
        "https://events.example.com/sse",
        &[],
    );
    events.enabled = false;
    insta::assert_snapshot!("omp_sse", golden(OMP, Os::Linux, &events));
}

#[test]
fn pi_goldens_per_transport() {
    let mut files = stdio(
        "filesystem",
        "npx",
        &["-y", "example-fs-mcp", "."],
        &[("FS_TOKEN", "!example-secret-cli read fs")],
    );
    stdio_mut(&mut files).cwd = Some("~/work".to_owned());
    files
        .passthrough
        .insert("timeout", NativeValue::Integer(30));
    files
        .passthrough
        .insert("exposure", NativeValue::from("direct"));
    insta::assert_snapshot!("pi_stdio", golden(PI, Os::Windows, &files));
    assert_eq!(
        golden(PI, Os::Windows, &files),
        golden(PI, Os::Linux, &files)
    );

    // Streamable HTTP: `type: "http"`, as omp writes it (gate V2 OMP-10); a
    // rendered `!command` header value is written as it is.
    let mut docs = remote(
        "docs",
        Transport::Http,
        "https://docs.example.com/mcp",
        &[("Authorization", "!example-secret-cli read docs")],
    );
    docs.enabled = false;
    insta::assert_snapshot!("pi_http", golden(PI, Os::Linux, &docs));

    let events = remote(
        "events",
        Transport::Sse,
        "https://events.example.com/sse",
        &[],
    );
    assert_eq!(
        write(PI, Os::Linux, &events).unwrap_err().code(),
        ErrorCode::UnsupportedTransport
    );
}

#[test]
fn tier_a_clients_read_their_files() {
    // Codex: forwarded variables read as `${env:NAME}`; a remote-sourced
    // `env_vars` entry and `command` plus `url` are `unexpected_shape`.
    let codex = parse(CODEX, &fixture("codex/user/config.toml"));
    insta::assert_snapshot!("codex_read_user", read_all(CODEX, &codex, servers(CODEX)));
    // The legacy table reads through the same mapper (import only).
    let legacy = mcp(CODEX).legacy_containers[0].segments();
    assert_eq!(legacy, ["mcp", "servers"]);
    insta::assert_snapshot!("codex_read_legacy", read_all(CODEX, &codex, legacy));

    // omp: a bare `url` is http, `ws` is unsupported, `requestIdFormat` is
    // unmapped, a bare variable name in `env` stays as written.
    let omp = parse(OMP, &fixture("omp/user/mcp.json"));
    insta::assert_snapshot!("omp_read_user", read_all(OMP, &omp, servers(OMP)));

    // Pi: `streamable-http` is http; `sse` reads as sse (Pi rejects it);
    // `description` and `toolExposure` are unmapped.
    let pi = parse(PI, &fixture("pi/user/mcp.json"));
    insta::assert_snapshot!("pi_read_user", read_all(PI, &pi, servers(PI)));
}

#[test]
fn fields_a_client_cannot_express_block_the_target() {
    let blocked =
        |spec: &AdapterSpec, server: &RenderedServer| write(spec, Os::Linux, server).unwrap_err();
    let field = |name: &str| MapError::UnsupportedField {
        field: name.to_owned(),
    };

    // Forwarding by name is Codex only.
    let mut forward = stdio("s", "x", &[], &[]);
    stdio_mut(&mut forward).forward_env = owned(&["TOKEN"]);
    for spec in [OMP, PI, CODE, DESKTOP] {
        assert_eq!(blocked(spec, &forward), field("env_vars"), "{}", spec.id);
    }
    write(CODEX, Os::Linux, &forward).unwrap();
    let mut bearer = remote("s", Transport::Http, "https://s.example.com/mcp", &[]);
    remote_mut(&mut bearer).bearer_env = Some("TOKEN".to_owned());
    for spec in [OMP, PI, CODE] {
        assert_eq!(blocked(spec, &bearer), field("bearer_token_env_var"));
    }
    let mut header_env = remote("s", Transport::Http, "https://s.example.com/mcp", &[]);
    remote_mut(&mut header_env).headers_env = map(&[("X-Key", "KEY")]);
    for spec in [OMP, PI, CODE] {
        assert_eq!(blocked(spec, &header_env), field("env_http_headers"));
    }

    // `headersHelper` is Claude Code only.
    let mut helper = remote("s", Transport::Http, "https://s.example.com/mcp", &[]);
    remote_mut(&mut helper).headers_helper = Some("/opt/example/bin/headers".to_owned());
    for spec in TIER_A {
        assert_eq!(
            blocked(spec, &helper),
            field("headersHelper"),
            "{}",
            spec.id
        );
    }

    // Tool lists are Codex only among these.
    let mut filtered = stdio("s", "x", &[], &[]);
    filtered.tools = ToolLists {
        allow: None,
        deny: Some(owned(&["delete"])),
    };
    for spec in [OMP, PI, CODE] {
        assert_eq!(blocked(spec, &filtered), field("tools"), "{}", spec.id);
    }
    write(CODEX, Os::Linux, &filtered).unwrap();

    // Native disable exists on all three, not on the Claude clients.
    let mut disabled = stdio("s", "x", &[], &[]);
    disabled.enabled = false;
    for spec in TIER_A {
        write(spec, Os::Linux, &disabled).unwrap();
    }
    for spec in [CODE, DESKTOP] {
        assert_eq!(blocked(spec, &disabled), field("enabled"));
    }

    // `cwd` exists on all three.
    let mut cwd = stdio("s", "x", &[], &[]);
    stdio_mut(&mut cwd).cwd = Some("/srv/example".to_owned());
    for spec in TIER_A {
        write(spec, Os::Linux, &cwd).unwrap();
    }

    // Passthrough keys outside each allowlist.
    let mut foreign = stdio("s", "x", &[], &[]);
    foreign.passthrough.insert("trust", NativeValue::Bool(true));
    for spec in TIER_A {
        assert_eq!(blocked(spec, &foreign), field("trust"));
    }

    // Name rules: Codex `[A-Za-z0-9_.-]` (gate V1), Pi `[A-Za-z0-9_-]`; omp
    // also `:` and single inner spaces, up to 100 characters.
    let named = |name: &str| stdio(name, "x", &[], &[]);
    let invalid = |name: &str| MapError::InvalidName {
        name: name.to_owned(),
    };
    assert_eq!(blocked(PI, &named("notes.v2")), invalid("notes.v2"));
    write(CODEX, Os::Linux, &named("notes.v2")).unwrap();
    assert_eq!(blocked(CODEX, &named("team:notes")), invalid("team:notes"));
    for spec in [CODEX, PI] {
        write(spec, Os::Linux, &named("Notes_v2-x")).unwrap();
    }
    for name in ["notes.v2", "team:notes", "Team Notes"] {
        write(OMP, Os::Linux, &named(name)).unwrap();
    }
    for name in ["two  spaces", " lead", "trail ", "semi;colon", ""] {
        assert_eq!(blocked(OMP, &named(name)), invalid(name), "{name:?}");
    }
    write(OMP, Os::Linux, &named(&"n".repeat(100))).unwrap();
    assert_eq!(
        blocked(OMP, &named(&"n".repeat(101))).code(),
        ErrorCode::InvalidId
    );
}

#[test]
fn timeouts_keep_the_client_unit() {
    let field = |spec: &AdapterSpec, key: &str| {
        *mcp(spec)
            .caps
            .timeouts
            .iter()
            .find(|timeout| timeout.key == key)
            .unwrap()
    };
    let secs = Duration::from_secs;
    // The same 30 seconds is 30000 in omp and 30 in Pi.
    assert_eq!(
        field(OMP, "timeout").duration(&NativeValue::Integer(30000)),
        Some(secs(30))
    );
    assert_eq!(
        field(PI, "timeout").duration(&NativeValue::Integer(30)),
        Some(secs(30))
    );
    assert_eq!(
        field(CODEX, "startup_timeout_sec").duration(&NativeValue::Integer(20)),
        Some(secs(20))
    );
    assert_eq!(
        field(CODEX, "tool_timeout_sec").duration(&NativeValue::Float(2.5)),
        Some(Duration::from_millis(2500))
    );
    for bad in [
        NativeValue::Integer(-1),
        NativeValue::Float(-0.5),
        NativeValue::from("30s"),
        NativeValue::Bool(true),
    ] {
        assert_eq!(field(OMP, "timeout").duration(&bad), None, "{bad:?}");
    }

    // A value that is not a timeout blocks the write, naming the unit.
    let mut server = stdio("s", "x", &[], &[]);
    server
        .passthrough
        .insert("timeout", NativeValue::from("30s"));
    assert_eq!(
        write(OMP, Os::Linux, &server).unwrap_err(),
        MapError::UnexpectedShape {
            field: "timeout".to_owned(),
            expected: "a non-negative number of milliseconds",
        }
    );
    server
        .passthrough
        .insert("timeout", NativeValue::Integer(-5));
    assert_eq!(
        write(PI, Os::Linux, &server).unwrap_err(),
        MapError::UnexpectedShape {
            field: "timeout".to_owned(),
            expected: "a non-negative number of seconds",
        }
    );
    // `0` disables omp's timeout and is valid.
    server
        .passthrough
        .insert("timeout", NativeValue::Integer(0));
    write(OMP, Os::Linux, &server).unwrap();

    // Fixture values read back as passthrough in the client's unit.
    let read = |spec: &AdapterSpec, file: &str, name: &str| {
        let doc = parse(spec, &fixture(file));
        let value = doc.get(servers(spec), name).unwrap().unwrap();
        let def = mapper(spec).from_native(name, &value).unwrap();
        field(spec, "timeout").duration(def.passthrough.get("timeout").unwrap())
    };
    assert_eq!(read(OMP, "omp/user/mcp.json", "notes"), Some(secs(30)));
    assert_eq!(read(PI, "pi/user/mcp.json", "filesystem"), Some(secs(30)));
}

#[test]
fn unmodeled_keys_survive_an_update() {
    // omp: `requestIdFormat` stays; changing `args` touches one line.
    let original = fixture("omp/user/mcp.json");
    let mut doc = parse(OMP, &original);
    let existing = doc.get(servers(OMP), "notes").unwrap().unwrap();
    let mut notes = stdio(
        "notes",
        "npx",
        &["-y", "example-notes-mcp", "--fast"],
        &[("NOTES_TOKEN", "!example-secret-cli read notes")],
    );
    stdio_mut(&mut notes).cwd = Some("/srv/example/notes".to_owned());
    notes
        .passthrough
        .insert("timeout", NativeValue::Integer(30000));
    let native = update(OMP, &existing, &notes).unwrap();
    assert!(doc.upsert(servers(OMP), "notes", &native).unwrap());
    assert_eq!(
        changed_lines(&original, &text(&doc)),
        [(
            r#"      "args": ["-y", "example-notes-mcp"],"#,
            r#"      "args": ["-y", "example-notes-mcp", "--fast"],"#
        )]
    );

    // omp: enabling a disabled entry flips `enabled` in place.
    let mut doc = parse(OMP, &original);
    let existing = doc.get(servers(OMP), "search").unwrap().unwrap();
    let mut search = remote(
        "search",
        Transport::Http,
        "https://search.example.com/mcp",
        &[("Authorization", "Bearer ${SEARCH_TOKEN}")],
    );
    search
        .passthrough
        .insert("instructions", NativeValue::Bool(false));
    let mut native = update(OMP, &existing, &search).unwrap();
    // The entry had no `type`; Loadout writes it, as omp recommends.
    native.as_object().unwrap().get("type").unwrap();
    if let NativeValue::Object(entry) = &mut native {
        entry.remove("type");
    }
    assert!(doc.upsert(servers(OMP), "search", &native).unwrap());
    assert_eq!(
        changed_lines(&original, &text(&doc)),
        [(r#"      "enabled": false"#, r#"      "enabled": true"#)]
    );
    // Both are the same server to omp: the typed one only spells out http.
    assert_eq!(
        mapper(OMP).canonical(&update(OMP, &existing, &search).unwrap()),
        mapper(OMP).canonical(&native)
    );

    // Codex: `required` and the comments in the table stay.
    let original = fixture("codex/user/config.toml");
    let mut doc = parse(CODEX, &original);
    let existing = doc.get(servers(CODEX), "docs").unwrap().unwrap();
    let mut docs = stdio(
        "docs",
        "npx",
        &["-y", "example-docs-mcp", "--fast"],
        &[("DOCS_ROOT", "/srv/example/docs")],
    );
    stdio_mut(&mut docs).forward_env = owned(&["DOCS_TOKEN"]);
    docs.passthrough
        .insert("startup_timeout_sec", NativeValue::Integer(20));
    let native = update(CODEX, &existing, &docs).unwrap();
    assert!(doc.upsert(servers(CODEX), "docs", &native).unwrap());
    assert_eq!(
        changed_lines(&original, &text(&doc)),
        [(
            r#"args = ["-y", "example-docs-mcp"]"#,
            r#"args = ["-y", "example-docs-mcp", "--fast"]"#
        )]
    );

    // Without an existing entry nothing extra is written.
    let fresh = write(CODEX, Os::Linux, &docs).unwrap();
    assert!(!fresh.as_object().unwrap().contains_key("required"));
}

#[test]
fn canonical_ignores_client_defaults() {
    let canon = |spec: &AdapterSpec, text: &str| mapper(spec).canonical(&json(text));

    // Pi: `type` is optional and `streamable-http` is http.
    let bare = canon(PI, r#"{"url": "u"}"#);
    assert_eq!(canon(PI, r#"{"type": "http", "url": "u"}"#), bare);
    assert_eq!(
        canon(PI, r#"{"type": "streamable-http", "url": "u"}"#),
        bare
    );
    assert_ne!(canon(PI, r#"{"type": "sse", "url": "u"}"#), bare);
    assert_eq!(
        canon(
            PI,
            r#"{"type": "stdio", "command": "x", "enabled": true, "env": {}}"#
        ),
        canon(PI, r#"{"command": "x"}"#)
    );
    assert_ne!(
        canon(PI, r#"{"command": "x", "enabled": false}"#),
        canon(PI, r#"{"command": "x"}"#)
    );

    // omp: an untyped `url` is http; the timeout is not a default.
    assert_eq!(
        canon(OMP, r#"{"type": "http", "url": "u"}"#),
        canon(OMP, r#"{"url": "u"}"#)
    );
    assert_ne!(
        canon(OMP, r#"{"type": "sse", "url": "u"}"#),
        canon(OMP, r#"{"url": "u"}"#)
    );
    assert_ne!(
        canon(OMP, r#"{"command": "x", "timeout": 5000}"#),
        canon(OMP, r#"{"command": "x"}"#)
    );

    // Codex: empty lists and maps and `enabled = true` are defaults; an
    // empty `enabled_tools` allows nothing, so it counts.
    let plain = canon(CODEX, r#"{"command": "x"}"#);
    assert_eq!(
        canon(
            CODEX,
            r#"{"command": "x", "args": [], "env": {}, "env_vars": [], "enabled": true, "disabled_tools": []}"#
        ),
        plain
    );
    assert_ne!(
        canon(CODEX, r#"{"command": "x", "enabled_tools": []}"#),
        plain
    );
    assert_ne!(canon(CODEX, r#"{"command": "x", "enabled": false}"#), plain);
    let toml_doc = parse(
        CODEX,
        "[mcp_servers.x]\ncommand = \"x\"\nenv = { B = \"2\", A = \"1\" }\n",
    );
    let toml_value = toml_doc.get(&["mcp_servers"], "x").unwrap().unwrap();
    assert_eq!(
        mapper(CODEX).canonical(&toml_value),
        canon(CODEX, r#"{"env": {"A": "1", "B": "2"}, "command": "x"}"#)
    );

    // Idempotent.
    for spec in TIER_A {
        let once = canon(
            spec,
            r#"{"type": "stdio", "command": "x", "args": [], "z": [1]}"#,
        );
        assert_eq!(canon(spec, once.as_str()), once, "{}", spec.id);
    }
}

// ---------------------------------------------------------------------------
// Codex legacy table (SkillDock deleted `[mcp.servers]` on save)

#[test]
fn codex_legacy_mcp_servers_survive_every_op() {
    let original = fixture("codex/user/config.toml");
    let legacy_text = |text: &str| -> String {
        let start = text.find("# Written by an older version").unwrap();
        let end = text.find("[sandbox_workspace_write]").unwrap();
        text[start..end].to_owned()
    };
    let legacy = ["mcp", "servers"];
    let legacy_entries = |doc: &Document| -> Vec<(String, NativeValue)> {
        doc.list(&legacy)
            .unwrap()
            .into_iter()
            .map(|key| {
                let value = doc.get(&legacy, &key).unwrap().unwrap();
                (key, value)
            })
            .collect()
    };
    let before = parse(CODEX, &original);
    let entries = legacy_entries(&before);
    assert_eq!(entries.len(), 2);

    let docs = |args: &[&str]| {
        let mut server = stdio("docs", "npx", args, &[("DOCS_ROOT", "/srv/example/docs")]);
        stdio_mut(&mut server).forward_env = owned(&["DOCS_TOKEN"]);
        server
    };
    let upsert = |doc: &mut Document, server: &RenderedServer| {
        let existing = doc.get(servers(CODEX), &server.name).unwrap();
        let native = mapper(CODEX)
            .to_native(
                server,
                &WriteCx {
                    os: Os::Linux,
                    existing: existing.as_ref(),
                },
            )
            .unwrap();
        doc.upsert(servers(CODEX), &server.name, &native).unwrap();
    };
    type Op = Box<dyn Fn(&mut Document)>;
    let ops: Vec<(&str, Op)> = vec![
        (
            "create",
            Box::new(move |doc| upsert(doc, &stdio("fresh", "example-fresh-mcp", &[], &[]))),
        ),
        (
            // `docs` is in both tables; only `[mcp_servers.docs]` changes.
            "update the entry with a legacy twin",
            Box::new(move |doc| upsert(doc, &docs(&["-y", "example-docs-mcp", "--v2"]))),
        ),
        (
            "disable",
            Box::new(move |doc| {
                let mut server = stdio("scoped", "example-scoped-mcp", &[], &[]);
                server.enabled = false;
                upsert(doc, &server);
            }),
        ),
        (
            "suppress the legacy name in a project entry",
            Box::new(|doc| {
                let entry = mcp(CODEX)
                    .suppression_entry(&stdio("legacy-tool", "legacy-tool", &[], &[]), Os::Linux)
                    .unwrap()
                    .unwrap();
                doc.upsert(servers(CODEX), "legacy-tool", &entry).unwrap();
            }),
        ),
        (
            "remove the entry with a legacy twin",
            Box::new(|doc| {
                assert!(doc.remove(servers(CODEX), "docs").unwrap());
            }),
        ),
        (
            "remove every entry",
            Box::new(|doc| {
                for key in doc.list(servers(CODEX)).unwrap() {
                    assert!(doc.remove(servers(CODEX), &key).unwrap());
                }
            }),
        ),
        (
            "a removal Loadout never asked for",
            Box::new(|doc| {
                assert!(!doc.remove(servers(CODEX), "legacy-tool").unwrap());
            }),
        ),
    ];

    let mut chained = parse(CODEX, &original);
    for (name, op) in &ops {
        let mut doc = parse(CODEX, &original);
        op(&mut doc);
        op(&mut chained);
        for doc in [&doc, &chained] {
            let after = text(doc);
            assert_eq!(legacy_text(&after), legacy_text(&original), "{name}");
            assert_eq!(legacy_entries(doc), entries, "{name}");
        }
    }
    assert!(chained.list(servers(CODEX)).unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Targets

fn put(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn files(targets: &[agent_loadout_core::adapters::Target]) -> Vec<PathBuf> {
    targets.iter().map(|target| target.file.clone()).collect()
}

#[test]
fn codex_and_pi_targets_follow_their_home_variables() {
    let home = TempHome::new();
    let fs = home.fs();
    let project = home.projects().join("app");
    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env(os);
        let user = CODEX
            .mcp_targets(&env, &fs, Scope::User, None, &NO_PREFS)
            .unwrap();
        assert_eq!(
            files(&user),
            [home.home().join(".codex").join("config.toml")]
        );
        assert_eq!(user[0].format, Format::Toml);
        assert_eq!(user[0].container.segments(), ["mcp_servers"]);
        let shared = CODEX
            .mcp_targets(&env, &fs, Scope::Project, Some(&project), &NO_PREFS)
            .unwrap();
        assert_eq!(files(&shared), [project.join(".codex").join("config.toml")]);

        let user = PI
            .mcp_targets(&env, &fs, Scope::User, None, &NO_PREFS)
            .unwrap();
        assert_eq!(
            files(&user),
            [home.home().join(".pi").join("agent").join("mcp.json")]
        );
        let shared = PI
            .mcp_targets(&env, &fs, Scope::Project, Some(&project), &NO_PREFS)
            .unwrap();
        assert_eq!(files(&shared), [project.join(".pi").join("mcp.json")]);

        // No private project file for any of the three.
        for spec in TIER_A {
            let err = spec
                .mcp_targets(&env, &fs, Scope::ProjectLocal, Some(&project), &NO_PREFS)
                .unwrap_err();
            assert_eq!(err.code(), ErrorCode::ScopeNotSupported, "{}", spec.id);
        }
    }

    // CODEX_HOME and PI_CODING_AGENT_DIR move the user files; relative
    // values are ignored.
    let codex_home = home.root().join("codex-home");
    let agent_dir = home.root().join("pi-agent");
    let env = home
        .env_builder(Os::Windows)
        .var("CODEX_HOME", &codex_home)
        .var("PI_CODING_AGENT_DIR", &agent_dir)
        .build();
    let user = |spec: &AdapterSpec, env: &Env| {
        files(
            &spec
                .mcp_targets(env, &fs, Scope::User, None, &NO_PREFS)
                .unwrap(),
        )
    };
    assert_eq!(user(CODEX, &env), [codex_home.join("config.toml")]);
    assert_eq!(user(PI, &env), [agent_dir.join("mcp.json")]);
    let env = home
        .env_builder(Os::Linux)
        .var("CODEX_HOME", "relative/codex")
        .var("PI_CODING_AGENT_DIR", "relative/pi")
        .build();
    assert_eq!(
        user(CODEX, &env),
        [home.home().join(".codex").join("config.toml")]
    );
    assert_eq!(
        user(PI, &env),
        [home.home().join(".pi").join("agent").join("mcp.json")]
    );
}

/// Gate V2 FB-8 (PI-1): Pi expands a leading `~` in `PI_CODING_AGENT_DIR`;
/// omp does not (it resolves the value against its working directory), so
/// omp keeps its own file.
#[test]
fn pi_agent_dir_expands_a_leading_tilde() {
    let home = TempHome::new();
    let fs = home.fs();
    let user = |spec: &AdapterSpec, env: &Env| {
        files(
            &spec
                .mcp_targets(env, &fs, Scope::User, None, &NO_PREFS)
                .unwrap(),
        )
    };
    let moved = home.home().join("piagent");
    let omp_default = home.home().join(".omp").join("agent").join("mcp.json");
    for (os, value) in [
        (Os::Windows, "~/piagent"),
        (Os::Windows, r"~\piagent"),
        (Os::Linux, "~/piagent"),
    ] {
        let env = home
            .env_builder(os)
            .var("PI_CODING_AGENT_DIR", value)
            .build();
        assert_eq!(user(PI, &env), [moved.join("mcp.json")], "{value}");
        assert_eq!(
            user(OMP, &env),
            std::slice::from_ref(&omp_default),
            "{value}"
        );
    }
    // `~` alone is home; `~\` is no home prefix off Windows.
    let env = home
        .env_builder(Os::Linux)
        .var("PI_CODING_AGENT_DIR", "~")
        .build();
    assert_eq!(user(PI, &env), [home.home().join("mcp.json")]);
    let env = home
        .env_builder(Os::Linux)
        .var("PI_CODING_AGENT_DIR", r"~\piagent")
        .build();
    assert_eq!(
        user(PI, &env),
        [home.home().join(".pi").join("agent").join("mcp.json")]
    );

    // Everything else in the agent dir follows: detection and the
    // `pi-mcp-adapter` settings.
    put(
        &moved.join("settings.json"),
        r#"{"packages": ["npm:pi-mcp-adapter"]}"#,
    );
    let env = home
        .env_builder(Os::Windows)
        .var("PI_CODING_AGENT_DIR", "~/piagent")
        .build();
    let detection = detect::detect(PI, &env, &fs, &detect::NoRegistry);
    assert!(detection.evidence.contains(&Evidence::Path(moved.clone())));
    assert_eq!(
        detection.warnings[0].path(),
        Some(moved.join("settings.json").as_path())
    );
}

#[test]
fn omp_targets_follow_its_config_dirs() {
    let home = TempHome::new();
    let fs = home.fs();
    let project = home.projects().join("app");
    let user = |env: &Env| {
        files(
            &OMP.mcp_targets(env, &fs, Scope::User, None, &NO_PREFS)
                .unwrap(),
        )
    };
    let omp_root = home.home().join(".omp");
    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env(os);
        assert_eq!(user(&env), [omp_root.join("agent").join("mcp.json")]);
        let shared = OMP
            .mcp_targets(&env, &fs, Scope::Project, Some(&project), &NO_PREFS)
            .unwrap();
        assert_eq!(files(&shared), [project.join(".omp").join("mcp.json")]);
        assert_eq!(shared[0].profile, None);
    }

    // PI_CONFIG_DIR renames the root under home; an absolute value or one
    // with `..` is ignored.
    let env = home
        .env_builder(Os::Linux)
        .var("PI_CONFIG_DIR", ".omp-alt")
        .build();
    assert_eq!(
        user(&env),
        [home.home().join(".omp-alt").join("agent").join("mcp.json")]
    );
    for ignored in [home.root().join("elsewhere"), PathBuf::from("../escape")] {
        let env = home
            .env_builder(Os::Windows)
            .var("PI_CONFIG_DIR", &ignored)
            .build();
        assert_eq!(user(&env), [omp_root.join("agent").join("mcp.json")]);
    }

    // PI_CODING_AGENT_DIR replaces the default profile's agent dir.
    let agent_dir = home.root().join("omp-agent");
    let env = home
        .env_builder(Os::Windows)
        .var("PI_CODING_AGENT_DIR", &agent_dir)
        .build();
    assert_eq!(user(&env), [agent_dir.join("mcp.json")]);
}

#[test]
fn omp_profile_targeting_from_clients_toml() {
    let home = TempHome::new();
    let fs = home.fs();
    let prefs_of = |text: &str| -> ClientPrefs {
        let file: ClientPrefsFile = toml::from_str(text).unwrap();
        assert!(file.validate().is_empty());
        file.clients.get("omp").cloned().unwrap_or_default()
    };
    let user = |env: &Env, prefs: &ClientPrefs| OMP.mcp_targets(env, &fs, Scope::User, None, prefs);
    let root = home.home().join(".omp");
    let default_file = root.join("agent").join("mcp.json");
    let work_file = root
        .join("profiles")
        .join("work")
        .join("agent")
        .join("mcp.json");

    // The spec's example, and no `[omp]` table at all: the default profile.
    let env = home.env(Os::Windows);
    for text in [
        "schema = 1\n[omp]\nprofiles = [\"default\"]\n",
        "schema = 1\n",
    ] {
        let targets = user(&env, &prefs_of(text)).unwrap();
        assert_eq!(files(&targets), std::slice::from_ref(&default_file));
        assert_eq!(targets[0].profile, None);
    }

    // Default plus a named profile, in order, duplicates dropped.
    let prefs = prefs_of("schema = 1\n[omp]\nprofiles = [\"default\", \"work\", \"default\"]\n");
    let targets = user(&env, &prefs).unwrap();
    assert_eq!(files(&targets), [default_file.clone(), work_file.clone()]);
    let profiles: Vec<Option<&str>> = targets.iter().map(|t| t.profile.as_deref()).collect();
    assert_eq!(profiles, [None, Some("work")]);
    for target in &targets {
        assert_eq!(target.client, "omp");
        assert_eq!(target.scope, Scope::User);
        assert!(target.warnings.is_empty());
    }

    // A named profile ignores PI_CODING_AGENT_DIR; PI_CONFIG_DIR moves both.
    let agent_dir = home.root().join("omp-agent");
    let env = home
        .env_builder(Os::Linux)
        .var("PI_CODING_AGENT_DIR", &agent_dir)
        .build();
    assert_eq!(
        files(&user(&env, &prefs).unwrap()),
        [agent_dir.join("mcp.json"), work_file.clone()]
    );
    let env = home
        .env_builder(Os::Linux)
        .var("PI_CONFIG_DIR", ".omp-alt")
        .build();
    let alt = home.home().join(".omp-alt");
    assert_eq!(
        files(&user(&env, &prefs).unwrap()),
        [
            alt.join("agent").join("mcp.json"),
            alt.join("profiles")
                .join("work")
                .join("agent")
                .join("mcp.json")
        ]
    );

    // Only named profiles; an empty list selects nothing.
    let env = home.env(Os::Windows);
    let only_work = prefs_of("schema = 1\n[omp]\nprofiles = [\"work\"]\n");
    assert_eq!(files(&user(&env, &only_work).unwrap()), [work_file]);
    let none = prefs_of("schema = 1\n[omp]\nprofiles = []\n");
    assert!(user(&env, &none).unwrap().is_empty());

    // Names omp rejects.
    for bad in [
        "Work",
        "con",
        "nul.txt",
        "COM1",
        "trailing.",
        "-dash",
        "a/b",
        "..",
    ] {
        let prefs = ClientPrefs {
            profiles: Some(vec![bad.to_owned()]),
            ..ClientPrefs::default()
        };
        let err = user(&env, &prefs).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidValue, "{bad:?}");
    }
    for good in ["work.2", "a_b-c", "com10", "console"] {
        let prefs = ClientPrefs {
            profiles: Some(vec![good.to_owned()]),
            ..ClientPrefs::default()
        };
        user(&env, &prefs).unwrap();
    }

    // Profiles are a user-scope notion, and only omp's.
    let project = home.projects().join("app");
    let shared = OMP
        .mcp_targets(&env, &fs, Scope::Project, Some(&project), &prefs)
        .unwrap();
    assert_eq!(files(&shared), [project.join(".omp").join("mcp.json")]);
    for spec in [CODEX, PI, CODE] {
        let targets = spec
            .mcp_targets(&env, &fs, Scope::User, None, &prefs)
            .unwrap();
        assert_eq!(targets.len(), 1, "{}", spec.id);
        assert_eq!(targets[0].profile, None);
    }
}

#[test]
fn omp_files_start_with_schema_and_keep_their_lists() {
    let home = TempHome::new();
    let fs = home.fs();
    let env = home.env(Os::Linux);
    let target = OMP
        .mcp_targets(&env, &fs, Scope::User, None, &NO_PREFS)
        .unwrap()
        .remove(0);
    let mut doc = target.new_document();
    let skeleton = text(&doc);
    assert!(
        skeleton.contains(r#""$schema": "https://raw.githubusercontent.com/can1357/oh-my-pi/"#)
    );
    let server = stdio("notes", "example-notes-mcp", &[], &[]);
    let native = write(OMP, Os::Linux, &server).unwrap();
    assert!(doc.upsert(servers(OMP), "notes", &native).unwrap());
    insta::assert_snapshot!("omp_new_file", text(&doc));
    assert!(doc.remove(servers(OMP), "notes").unwrap());
    assert_eq!(text(&doc), skeleton);
    // Other clients start from an empty file.
    for spec in [CODEX, PI, CODE] {
        let target = spec
            .mcp_targets(&env, &fs, Scope::User, None, &NO_PREFS)
            .unwrap()
            .remove(0);
        assert_eq!(target.new_document().to_bytes(), b"", "{}", spec.id);
    }

    // `disabledServers` and `enabledServers` are never touched, even when
    // the entry they name is removed.
    let original = fixture("omp/user/mcp.json");
    let lists = |text: &str| -> Vec<String> {
        text.lines()
            .filter(|line| line.contains("disabledServers") || line.contains("enabledServers"))
            .map(str::to_owned)
            .collect()
    };
    let mut doc = parse(OMP, &original);
    let fresh = write(OMP, Os::Linux, &stdio("fresh", "x", &[], &[])).unwrap();
    assert!(doc.upsert(servers(OMP), "fresh", &fresh).unwrap());
    assert!(doc.remove(servers(OMP), "events").unwrap());
    assert!(doc.remove(servers(OMP), "search").unwrap());
    let edited = text(&doc);
    assert_eq!(lists(&edited), lists(&original));
    assert_eq!(lists(&original).len(), 2);
    assert!(edited.contains(r#""disabledServers": ["events"]"#));
    assert!(edited.contains(r#""enabledServers": ["search"]"#));
}

// ---------------------------------------------------------------------------
// Project suppression and names

#[test]
fn project_suppression_per_client() {
    let home = TempHome::new();
    let fs = home.fs();
    let env = home.env(Os::Windows);
    let project = home.projects().join("app");
    let server = stdio(
        "docs",
        "npx",
        &["-y", "example-docs-mcp"],
        &[("DOCS_ROOT", "/srv/example/docs")],
    );

    let mut out = String::new();
    for (spec, file) in [
        (CODEX, project.join(".codex").join("config.toml")),
        (OMP, project.join(".omp").join("mcp.json")),
        (PI, project.join(".pi").join("mcp.json")),
    ] {
        let target = spec
            .suppression_target(&env, &fs, &project, &NO_PREFS)
            .unwrap()
            .unwrap();
        assert_eq!(target.file, file);
        assert_eq!(target.scope, Scope::Project);
        assert_eq!(target.container.segments(), servers(spec));
        let entry = mcp(spec)
            .suppression_entry(&server, env.os())
            .unwrap()
            .unwrap();
        let mut doc = target.new_document();
        let keys = target.container_keys().unwrap();
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        assert!(doc.upsert(&keys, &server.name, &entry).unwrap());
        out.push_str(&format!("## {}\n{}\n", spec.id, text(&doc)));
    }
    // Codex and Pi merge an `enabled`-only entry over the user one; omp
    // replaces the user entry, so it gets the whole entry, disabled.
    insta::assert_snapshot!("project_suppression", out);

    // Claude Code suppresses through a list; Claude Desktop cannot.
    assert_eq!(
        mcp(CODE).suppression_entry(&server, env.os()).unwrap(),
        None
    );
    assert_eq!(
        mcp(DESKTOP).suppression_entry(&server, env.os()).unwrap(),
        None
    );
    assert!(
        DESKTOP
            .suppression_target(&env, &fs, &project, &NO_PREFS)
            .unwrap()
            .is_none()
    );
    // omp's full entry follows the client's rules like any write.
    assert_eq!(
        mcp(OMP)
            .suppression_entry(&stdio("bad  name", "x", &[], &[]), env.os())
            .unwrap_err()
            .code(),
        ErrorCode::InvalidId
    );
}

#[test]
fn pi_names_differing_in_dash_and_underscore_collide() {
    let keys = owned(&["my_tool", "other", "my-tool", "a_b-c"]);
    let collisions = PI.name_collisions("my-tool", &keys);
    assert_eq!(collisions.len(), 1);
    assert_eq!(collisions[0].code(), ErrorCode::Collision);
    assert_eq!(collisions[0].code().as_str(), "collision");
    assert!(collisions[0].message().contains("\"my_tool\""));
    assert_eq!(PI.name_collisions("a-b_c", &keys).len(), 1);
    assert!(PI.name_collisions("other", &keys).is_empty());
    assert!(PI.name_collisions("my-tool-2", &keys).is_empty());
    // Case still matters, and other clients compare names exactly.
    assert!(PI.name_collisions("My-Tool", &keys).is_empty());
    for spec in [CODEX, OMP, CODE] {
        assert!(
            spec.name_collisions("my-tool", &keys).is_empty(),
            "{}",
            spec.id
        );
    }

    // Against a real-shaped file: the foreign `my_tool` clashes with a
    // desired `my-tool`.
    let doc = parse(PI, &fixture("pi/user/mcp.json"));
    let existing = doc.list(servers(PI)).unwrap();
    assert_eq!(PI.name_collisions("my-tool", &existing).len(), 1);
}

// ---------------------------------------------------------------------------
// Detection

#[derive(Default)]
struct FakeRegistry {
    lookups: Cell<usize>,
}

impl RegistryProbe for FakeRegistry {
    fn has_uninstall_entry(&self, _display_name: &str) -> bool {
        self.lookups.set(self.lookups.get() + 1);
        false
    }
}

/// A rooted file system that fails the test on any write.
struct ReadOnlyFs(RootedFs);

impl Fs for ReadOnlyFs {
    fn root(&self) -> Option<&Path> {
        self.0.root()
    }
    fn exists(&self, path: &Path) -> bool {
        self.0.exists(path)
    }
    fn is_dir(&self, path: &Path) -> bool {
        self.0.is_dir(path)
    }
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.0.read_to_string(path)
    }
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.0.read(path)
    }
    fn write_new(&self, path: &Path, _: &[u8], _: Option<&Path>) -> io::Result<()> {
        panic!("detection wrote {}", path.display())
    }
    fn replace(&self, _: &Path, to: &Path) -> io::Result<()> {
        panic!("detection replaced {}", to.display())
    }
    fn write_private(&self, path: &Path, _: &[u8]) -> io::Result<()> {
        panic!("detection wrote {}", path.display())
    }
    fn create_private_dir_all(&self, path: &Path) -> io::Result<()> {
        panic!("detection created {}", path.display())
    }
    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        panic!("detection removed {}", path.display())
    }
    fn write(&self, path: &Path, _: &[u8]) -> io::Result<()> {
        panic!("detection wrote {}", path.display())
    }
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        panic!("detection created {}", path.display())
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        panic!("detection removed {}", path.display())
    }
    fn is_regular_file(&self, path: &Path) -> bool {
        self.0.is_regular_file(path)
    }
    fn overwrite_with_zeros(&self, path: &Path) -> io::Result<()> {
        panic!("detection overwrote {}", path.display())
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        self.0.read_dir(path)
    }
    fn open_lock_file(&self, path: &Path) -> io::Result<std::fs::File> {
        panic!("detection locked {}", path.display())
    }
}

#[test]
fn tier_a_detection_per_os() {
    let home = TempHome::new();
    let fs = ReadOnlyFs(home.fs());
    let registry = FakeRegistry::default();
    let evidence =
        |spec: &AdapterSpec, env: &Env| detect::detect(spec, env, &fs, &registry).evidence;

    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env_builder(os).var("PATH", home.bin()).build();
        for spec in TIER_A {
            let found = detect::detect(spec, &env, &fs, &registry);
            assert!(found.supported, "{} on {os:?}", spec.id);
            assert!(!found.detected(), "{} on {os:?}", spec.id);
            assert!(found.warnings.is_empty());
        }
    }

    // Config dirs, on every OS.
    let codex_dir = home.home().join(".codex");
    let omp_dir = home.home().join(".omp");
    let pi_dir = home.home().join(".pi").join("agent");
    for dir in [&codex_dir, &omp_dir, &pi_dir] {
        std::fs::create_dir_all(dir).unwrap();
    }
    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env(os);
        assert_eq!(evidence(CODEX, &env), [Evidence::Path(codex_dir.clone())]);
        assert_eq!(evidence(OMP, &env), [Evidence::Path(omp_dir.clone())]);
        assert_eq!(evidence(PI, &env), [Evidence::Path(pi_dir.clone())]);
    }

    // The variables that move the dirs replace them.
    let codex_home = home.root().join("codex-home");
    let agent_dir = home.root().join("pi-agent");
    let env = home
        .env_builder(Os::Linux)
        .var("CODEX_HOME", &codex_home)
        .var("PI_CODING_AGENT_DIR", &agent_dir)
        .var("PI_CONFIG_DIR", ".omp-alt")
        .build();
    for spec in TIER_A {
        assert!(evidence(spec, &env).is_empty(), "{}", spec.id);
    }
    let omp_alt = home.home().join(".omp-alt");
    for dir in [&codex_home, &agent_dir, &omp_alt] {
        std::fs::create_dir_all(dir).unwrap();
    }
    assert_eq!(evidence(CODEX, &env), [Evidence::Path(codex_home)]);
    assert_eq!(evidence(OMP, &env), [Evidence::Path(omp_alt)]);
    assert_eq!(evidence(PI, &env), [Evidence::Path(agent_dir)]);
    for dir in [&codex_dir, &omp_dir, &pi_dir] {
        std::fs::remove_dir_all(dir).unwrap();
    }

    // Executables on a Windows PATH, through PATHEXT.
    for name in ["codex.exe", "omp.cmd", "pi.exe"] {
        put(&home.bin().join(name), "");
    }
    let env = home
        .env_builder(Os::Windows)
        .var("PATH", home.bin())
        .var("PATHEXT", ".EXE;.CMD")
        .build();
    assert_eq!(
        evidence(CODEX, &env),
        [Evidence::Executable(home.bin().join("codex.exe"))]
    );
    assert_eq!(
        evidence(OMP, &env),
        [Evidence::Executable(home.bin().join("omp.cmd"))]
    );
    assert_eq!(
        evidence(PI, &env),
        [Evidence::Executable(home.bin().join("pi.exe"))]
    );

    // None of them has a registry probe.
    assert_eq!(registry.lookups.get(), 0);
}

#[test]
fn pi_mcp_adapter_is_reported() {
    let home = TempHome::new();
    let fs = ReadOnlyFs(home.fs());
    let env = home.env(Os::Linux);
    let agent = home.home().join(".pi").join("agent");
    std::fs::create_dir_all(&agent).unwrap();
    let warnings = |env: &Env| detect::detect(PI, env, &fs, &detect::NoRegistry).warnings;
    assert!(detect::detect(PI, &env, &fs, &detect::NoRegistry).detected());
    assert!(warnings(&env).is_empty());

    // Installed as a package in the user settings.
    let settings = agent.join("settings.json");
    put(&settings, &fixture("pi/settings/settings.json"));
    let found = warnings(&env);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].code(), ErrorCode::ClientMcpReplacedByExtension);
    assert_eq!(found[0].code().as_str(), "client_mcp_replaced_by_extension");
    assert_eq!(found[0].path(), Some(settings.as_path()));

    // Every source spelling names the package; look-alikes do not.
    for source in [
        "npm:pi-mcp-adapter",
        "npm:@example/pi-mcp-adapter@2.0.0",
        "git:github.com/example/pi-mcp-adapter@v1",
        "https://github.com/example/pi-mcp-adapter.git",
        "../local/pi-mcp-adapter",
    ] {
        put(&settings, &format!(r#"{{"packages": ["{source}"]}}"#));
        assert_eq!(warnings(&env).len(), 1, "{source}");
    }
    for contents in [
        r#"{"packages": ["npm:pi-mcp-adapter-extra", "npm:other@pi-mcp-adapter"]}"#,
        r#"{"packages": "npm:pi-mcp-adapter"}"#,
        r#"{"extensions": ["pi-mcp-adapter"]}"#,
        "{ not json",
    ] {
        put(&settings, contents);
        assert!(warnings(&env).is_empty(), "{contents}");
    }

    // Installed as an auto-discovered extension.
    let extension = agent.join("extensions").join("pi-mcp-adapter");
    std::fs::create_dir_all(&extension).unwrap();
    assert_eq!(warnings(&env)[0].path(), Some(extension.as_path()));
    std::fs::remove_dir_all(&extension).unwrap();

    // Installed by `pi install npm:pi-mcp-adapter` (gate V2 PI-8): the
    // package under `npm/node_modules`.
    let package = agent
        .join("npm")
        .join("node_modules")
        .join("pi-mcp-adapter");
    std::fs::create_dir_all(&package).unwrap();
    assert_eq!(warnings(&env)[0].path(), Some(package.as_path()));
    std::fs::remove_dir_all(agent.join("npm")).unwrap();

    // PI_CODING_AGENT_DIR moves where Pi looks.
    let moved = home.root().join("pi-agent");
    put(
        &moved.join("extensions").join("pi-mcp-adapter.ts"),
        "export default {};\n",
    );
    let moved_env = home
        .env_builder(Os::Windows)
        .var("PI_CODING_AGENT_DIR", &moved)
        .build();
    assert_eq!(warnings(&moved_env).len(), 1);
    assert!(warnings(&home.env(Os::Windows)).is_empty());

    // A project install (`pi install -l`: the `packages` entry in
    // `<P>/.pi/settings.json`, the package under `<P>/.pi/npm`) shows only
    // when the project is given.
    let project = home.projects().join("app");
    let project_settings = project.join(".pi").join("settings.json");
    for contents in [
        r#"{"packages": ["npm:pi-mcp-adapter"]}"#,
        r#"{"packages": [{"source": "npm:pi-mcp-adapter"}]}"#,
    ] {
        put(&project_settings, contents);
        assert!(warnings(&env).is_empty());
        let found = detect::replacing_extension(PI, &env, &fs, Some(&project)).unwrap();
        assert_eq!(found.code(), ErrorCode::ClientMcpReplacedByExtension);
        assert_eq!(found.path(), Some(project_settings.as_path()));
        assert!(detect::replacing_extension(PI, &env, &fs, None).is_none());
    }
    let project_package = project
        .join(".pi")
        .join("npm")
        .join("node_modules")
        .join("pi-mcp-adapter");
    std::fs::create_dir_all(&project_package).unwrap();
    let found = detect::replacing_extension(PI, &env, &fs, Some(&project)).unwrap();
    assert_eq!(found.path(), Some(project_package.as_path()));

    // Other clients have no such extension, whatever is on disk.
    for spec in [CODEX, OMP, CODE] {
        assert!(detect::replacing_extension(spec, &env, &fs, Some(&project)).is_none());
    }
}

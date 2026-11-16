//! Claude Code and Claude Desktop adapters (SPEC 7.1, 7.3, 7.4, 7.7):
//! `to_native`/`from_native` goldens per transport, blocked targets,
//! target files per scope and OS (including the MSIX dual write), the
//! Windows `projects` key, `disabledMcpServers`, and detection through a
//! fake `Env` and a fake registry probe.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use agent_loadout_core::adapters::detect::{self, Evidence, RegistryProbe};
use agent_loadout_core::adapters::mapper::{
    EntryMapper, Launch, MapError, RemoteLaunch, RenderedServer, ServerDef, StdioLaunch, WriteCx,
};
use agent_loadout_core::adapters::{AdapterSpec, claude_code, claude_desktop};
use agent_loadout_core::formats::{Document, Format, NativeValue};
use agent_loadout_core::model::applied::Scope;
use agent_loadout_core::model::clients::ClientPrefs;
use agent_loadout_core::model::server::{ServerFields, Transport};
use agent_loadout_core::{CoreError, ErrorCode, Fs, Os};
use agent_loadout_testkit::{RootedFs, TempHome};

const CODE: &AdapterSpec = &claude_code::SPEC;
const DESKTOP: &AdapterSpec = &claude_desktop::SPEC;
const SERVERS: &[&str] = &["mcpServers"];
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

fn parse(text: &str) -> Document {
    Document::parse(text.as_bytes(), Format::Json).unwrap()
}

fn text(doc: &Document) -> String {
    String::from_utf8(doc.to_bytes()).unwrap()
}

fn mapper(spec: &AdapterSpec) -> &'static dyn EntryMapper {
    spec.mcp.as_ref().unwrap().mapper
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

fn stdio(name: &str, command: &str, args: &[&str], env: &[(&str, &str)]) -> RenderedServer {
    RenderedServer::new(
        name,
        Launch::Stdio(StdioLaunch {
            command: command.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: map(env),
            ..StdioLaunch::default()
        }),
    )
}

fn remote(
    name: &str,
    transport: Transport,
    url: &str,
    headers: &[(&str, &str)],
    helper: Option<&str>,
) -> RenderedServer {
    let launch = RemoteLaunch {
        url: url.to_owned(),
        headers: map(headers),
        headers_helper: helper.map(str::to_owned),
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

fn write(spec: &AdapterSpec, os: Os, server: &RenderedServer) -> Result<NativeValue, MapError> {
    mapper(spec).to_native(server, &WriteCx { os, existing: None })
}

/// The fields a rendered server reads back as.
fn fields_of(server: &RenderedServer) -> ServerFields {
    let non_empty = |map: &BTreeMap<String, String>| (!map.is_empty()).then(|| map.clone());
    match &server.launch {
        Launch::Stdio(stdio) => ServerFields {
            transport: Some(Transport::Stdio),
            command: Some(stdio.command.clone()),
            args: Some(stdio.args.clone()),
            env: non_empty(&stdio.env),
            ..ServerFields::default()
        },
        Launch::Http(remote) | Launch::Sse(remote) => ServerFields {
            transport: Some(server.launch.transport()),
            url: Some(remote.url.clone()),
            headers: non_empty(&remote.headers),
            ..ServerFields::default()
        },
    }
}

/// A read-back definition as store TOML plus what has no store field.
fn describe(def: &ServerDef) -> String {
    let mut out = toml::to_string(&def.fields).unwrap();
    if let Some(helper) = &def.headers_helper {
        out.push_str(&format!("# headers_helper: {helper}\n"));
    }
    for (key, value) in def.passthrough.iter() {
        out.push_str(&format!("# passthrough {key}: {value:?}\n"));
    }
    for (key, value) in def.unmapped.iter() {
        out.push_str(&format!("# unmapped {key}: {value:?}\n"));
    }
    out
}

/// The file a new entry makes, and what it reads back as. Checks the round
/// trip: the entry reads back as the rendered server.
fn golden(spec: &AdapterSpec, os: Os, server: &RenderedServer) -> String {
    let mapper = mapper(spec);
    let native = write(spec, os, server).unwrap();
    let mut doc = Document::empty(Format::Json);
    assert!(doc.upsert(SERVERS, &server.name, &native).unwrap());
    let reread = parse(&text(&doc))
        .get(SERVERS, &server.name)
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
    let helper = match &server.launch {
        Launch::Http(remote) | Launch::Sse(remote) => remote.headers_helper.clone(),
        Launch::Stdio(_) => None,
    };
    assert_eq!(def.headers_helper, helper);
    assert!(def.unmapped.is_empty());
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

// ---------------------------------------------------------------------------
// Mappers

#[test]
fn claude_code_goldens_per_transport() {
    let server = stdio(
        "notes",
        "npx",
        &["-y", "example-notes-mcp", "${NOTES_DIR:-.}"],
        &[("NOTES_TOKEN", "${NOTES_TOKEN}")],
    );
    insta::assert_snapshot!("claude_code_stdio", golden(CODE, Os::Windows, &server));
    // Claude Code spawns stdio servers itself on every OS: no `cmd /c`.
    assert_eq!(
        golden(CODE, Os::Windows, &server),
        golden(CODE, Os::Linux, &server)
    );
    let bare = stdio("bare", "example-local-mcp", &[], &[]);
    insta::assert_snapshot!("claude_code_stdio_bare", golden(CODE, Os::Linux, &bare));

    let http = remote(
        "search",
        Transport::Http,
        "https://search.example.com/mcp",
        &[("X-Workspace", "demo")],
        Some("/opt/example/bin/loadout secret-header Authorization=example"),
    );
    insta::assert_snapshot!("claude_code_http", golden(CODE, Os::Linux, &http));

    let sse = remote(
        "events",
        Transport::Sse,
        "https://events.example.com/sse",
        &[],
        None,
    );
    insta::assert_snapshot!("claude_code_sse", golden(CODE, Os::Linux, &sse));
}

#[test]
fn claude_desktop_goldens_per_os() {
    let server = stdio(
        "notes",
        "npx",
        &["-y", "example-notes-mcp"],
        &[("EXAMPLE_MODE", "readonly")],
    );
    // Windows: the app spawns without a shell, so the `npx` shim runs
    // through `cmd /c` (SPEC 5.2) and reads back as plain `npx`.
    insta::assert_snapshot!(
        "claude_desktop_stdio_windows",
        golden(DESKTOP, Os::Windows, &server)
    );
    insta::assert_snapshot!(
        "claude_desktop_stdio_macos",
        golden(DESKTOP, Os::MacOs, &server)
    );

    let shimmed = |command: &str| {
        let native = write(DESKTOP, Os::Windows, &stdio("s", command, &["x"], &[])).unwrap();
        let entry = native.as_object().unwrap();
        let args: Vec<&str> = entry
            .get("args")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        (
            entry.get("command").unwrap().as_str().unwrap().to_owned(),
            args.join(" "),
        )
    };
    assert_eq!(shimmed("npx"), ("cmd".to_owned(), "/c npx x".to_owned()));
    assert_eq!(shimmed("pnpm"), ("cmd".to_owned(), "/c pnpm x".to_owned()));
    assert_eq!(
        shimmed(r"C:\tools\run-mcp.CMD"),
        ("cmd".to_owned(), r"/c C:\tools\run-mcp.CMD x".to_owned())
    );
    assert_eq!(
        shimmed(r"C:\tools\example-mcp.exe"),
        (r"C:\tools\example-mcp.exe".to_owned(), "x".to_owned())
    );
    assert_eq!(shimmed("uvx"), ("uvx".to_owned(), "x".to_owned()));
    for command in [r"C:\tools\run-mcp.CMD", "pnpm"] {
        let server = stdio("s", command, &["x"], &[]);
        golden(DESKTOP, Os::Windows, &server);
    }
}

#[test]
fn claude_code_reads_a_user_file() {
    let doc = parse(&fixture("claude-code/user/.claude.json"));
    // `ws` has no store transport; a bare `url` is http (SPEC 7.5); `cwd`
    // and `x-note` have no Claude Code field and are kept as unmapped.
    insta::assert_snapshot!("claude_code_read_user", read_all(CODE, &doc, SERVERS));
    insta::assert_snapshot!(
        "claude_code_read_project_local",
        read_all(CODE, &doc, &["projects", "C:/work/other-app", "mcpServers"])
    );
    let project = parse(&fixture("claude-code/project/.mcp.json"));
    insta::assert_snapshot!(
        "claude_code_read_project",
        read_all(CODE, &project, SERVERS)
    );
}

#[test]
fn claude_desktop_reads_a_windows_file() {
    let doc = parse(&fixture(
        "claude-desktop/windows/claude_desktop_config.json",
    ));
    // `cmd /c npx` reads back as `npx`; `cmd /c start` is not a shim and
    // stays as written, so it writes back unchanged.
    insta::assert_snapshot!("claude_desktop_read", read_all(DESKTOP, &doc, SERVERS));
    let launcher = doc.get(SERVERS, "launcher").unwrap().unwrap();
    let def = mapper(DESKTOP).from_native("launcher", &launcher).unwrap();
    let server = stdio(
        "launcher",
        def.fields.command.as_deref().unwrap(),
        &def.fields
            .args
            .iter()
            .flatten()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        &[],
    );
    let written = write(DESKTOP, Os::Windows, &server).unwrap();
    assert_eq!(
        mapper(DESKTOP).canonical(&written),
        mapper(DESKTOP).canonical(&launcher)
    );
}

#[test]
fn remote_servers_are_blocked_on_claude_desktop() {
    for transport in [Transport::Http, Transport::Sse] {
        let server = remote(
            "search",
            transport,
            "https://search.example.com/mcp",
            &[],
            None,
        );
        let err = write(DESKTOP, Os::Windows, &server).unwrap_err();
        assert_eq!(
            err,
            MapError::UnsupportedTransport {
                transport: transport.as_str().to_owned()
            }
        );
        let err = CoreError::from(err);
        assert_eq!(err.code(), ErrorCode::UnsupportedTransport);
        assert_eq!(err.code().as_str(), "unsupported_transport");
        // Claude Code takes the same server.
        write(CODE, Os::Windows, &server).unwrap();
    }
}

#[test]
fn fields_a_client_cannot_express_block_the_target() {
    let mut server = stdio("notes", "npx", &["example-notes-mcp"], &[]);
    let Launch::Stdio(launch) = &mut server.launch else {
        unreachable!()
    };
    launch.cwd = Some("/srv/example".to_owned());
    for spec in [CODE, DESKTOP] {
        let err = write(spec, Os::Windows, &server).unwrap_err();
        assert_eq!(err.code(), ErrorCode::UnsupportedField, "{}", spec.id);
        assert_eq!(
            err,
            MapError::UnsupportedField {
                field: "cwd".to_owned()
            }
        );
    }

    // Passthrough keys outside the (empty) allowlist never reach the file.
    let mut server = stdio("notes", "npx", &[], &[]);
    server
        .passthrough
        .insert("timeout", NativeValue::Integer(30));
    for spec in [CODE, DESKTOP] {
        assert_eq!(
            write(spec, Os::Linux, &server).unwrap_err().code(),
            ErrorCode::UnsupportedField
        );
    }

    // Claude Code names are `[A-Za-z0-9_-]`; `native_name` fixes the rest.
    let dotted = stdio("notes.v2", "npx", &[], &[]);
    let err = write(CODE, Os::Linux, &dotted).unwrap_err();
    assert_eq!(
        err,
        MapError::InvalidName {
            name: "notes.v2".to_owned()
        }
    );
    assert_eq!(err.code(), ErrorCode::InvalidId);
    write(CODE, Os::Linux, &stdio("Notes_v2-x", "npx", &[], &[])).unwrap();
    write(DESKTOP, Os::Windows, &dotted).unwrap();
}

#[test]
fn unmodeled_native_keys_survive_an_update() {
    let original = fixture("claude-code/user/.claude.json");
    let mut doc = parse(&original);
    let existing = doc.get(SERVERS, "tuned").unwrap().unwrap();
    let server = stdio("tuned", "uvx", &["example-tuned-mcp", "--fast"], &[]);
    let native = mapper(CODE)
        .to_native(
            &server,
            &WriteCx {
                os: Os::Linux,
                existing: Some(&existing),
            },
        )
        .unwrap();
    assert!(doc.upsert(SERVERS, "tuned", &native).unwrap());
    let updated = text(&doc);
    // Only the changed `args` line moved; `cwd` and `x-note` are kept, and
    // so is the empty `env` that was there.
    let diff: Vec<(&str, &str)> = original
        .lines()
        .zip(updated.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(
        diff,
        [(
            r#"      "args": ["example-tuned-mcp"],"#,
            r#"      "args": ["example-tuned-mcp", "--fast"],"#
        )]
    );
    assert_eq!(original.lines().count(), updated.lines().count());

    // Without an existing entry nothing extra is written.
    let fresh = write(CODE, Os::Linux, &server).unwrap();
    assert!(!fresh.as_object().unwrap().contains_key("x-note"));
}

#[test]
fn canonical_ignores_key_order_and_client_defaults() {
    let canon = |spec: &AdapterSpec, text: &str| mapper(spec).canonical(&json(text));
    let plain = canon(CODE, r#"{"command": "x"}"#);
    assert_eq!(
        canon(
            CODE,
            r#"{"env": {}, "args": [], "command": "x", "type": "stdio"}"#
        ),
        plain
    );
    assert_ne!(canon(CODE, r#"{"command": "x", "args": ["a"]}"#), plain);
    assert_eq!(
        canon(
            CODE,
            r#"{"url": "u", "type": "http", "headers": {"B": "2", "A": "1"}}"#
        ),
        canon(
            CODE,
            r#"{"headers": {"A": "1", "B": "2"}, "type": "http", "url": "u"}"#
        )
    );
    // A missing `type` on a remote entry is not filled in: Claude Code
    // requires it, so the entry is not the one Loadout writes.
    assert_ne!(
        canon(CODE, r#"{"url": "u"}"#),
        canon(CODE, r#"{"type": "http", "url": "u"}"#)
    );
    assert_eq!(
        canon(DESKTOP, r#"{"command": "x", "args": [], "env": {}}"#),
        canon(DESKTOP, r#"{"command": "x"}"#)
    );
    // Canonicalizing the canonical form changes nothing.
    let once = canon(
        CODE,
        r#"{"type": "stdio", "command": "x", "args": [], "z": [1, 2.5, null]}"#,
    );
    let again = canon(CODE, once.as_str());
    assert_eq!(once, again);
}

// ---------------------------------------------------------------------------
// Targets

fn put(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn codes(errors: &[CoreError]) -> Vec<ErrorCode> {
    errors.iter().map(CoreError::code).collect()
}

#[test]
fn claude_code_targets_per_scope() {
    let home = TempHome::new();
    let fs = home.fs();
    let project = home.projects().join("app");
    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env(os);
        let user = CODE
            .mcp_targets(&env, &fs, Scope::User, None, &NO_PREFS)
            .unwrap();
        assert_eq!(user.len(), 1);
        assert_eq!(user[0].file, home.home().join(".claude.json"));
        assert_eq!(user[0].container.segments(), ["mcpServers"]);
        assert!(user[0].warnings.is_empty());

        let shared = CODE
            .mcp_targets(&env, &fs, Scope::Project, Some(&project), &NO_PREFS)
            .unwrap();
        assert_eq!(shared[0].file, project.join(".mcp.json"));
        assert_eq!(shared[0].container.segments(), ["mcpServers"]);

        let local = CODE
            .mcp_targets(&env, &fs, Scope::ProjectLocal, Some(&project), &NO_PREFS)
            .unwrap();
        assert_eq!(local[0].file, home.home().join(".claude.json"));
        let keys = local[0]
            .container_keys(&Document::empty(Format::Json))
            .unwrap();
        assert_eq!(keys.len(), 3);
        assert_eq!(
            (keys[0].as_str(), keys[2].as_str()),
            ("projects", "mcpServers")
        );

        let err = CODE
            .mcp_targets(&env, &fs, Scope::Project, None, &NO_PREFS)
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidPath);
    }

    // CLAUDE_CONFIG_DIR moves the user file (and the project-local
    // entries in it); a relative value is ignored.
    let config = home.root().join("claude-config");
    let env = home
        .env_builder(Os::Windows)
        .var("CLAUDE_CONFIG_DIR", &config)
        .build();
    for scope in [Scope::User, Scope::ProjectLocal] {
        let targets = CODE
            .mcp_targets(&env, &fs, scope, Some(&project), &NO_PREFS)
            .unwrap();
        assert_eq!(targets[0].file, config.join(".claude.json"));
    }
    let env = home
        .env_builder(Os::Linux)
        .var("CLAUDE_CONFIG_DIR", "relative/dir")
        .build();
    let targets = CODE
        .mcp_targets(&env, &fs, Scope::User, None, &NO_PREFS)
        .unwrap();
    assert_eq!(targets[0].file, home.home().join(".claude.json"));
}

#[test]
fn claude_desktop_targets_per_os() {
    let home = TempHome::new();
    let fs = home.fs();
    let roaming = home
        .appdata()
        .join("Claude")
        .join("claude_desktop_config.json");
    let env = home.env(Os::Windows);

    // No MSIX install: one target, created if missing.
    let targets = DESKTOP
        .mcp_targets(&env, &fs, Scope::User, None, &NO_PREFS)
        .unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].file, roaming);
    assert!(targets[0].warnings.is_empty());

    // An MSIX package without its own copy of the file reads the real one.
    let package = home
        .local_appdata()
        .join("Packages")
        .join("Claude_0000fake0000");
    std::fs::create_dir_all(&package).unwrap();
    let targets = DESKTOP
        .mcp_targets(&env, &fs, Scope::User, None, &NO_PREFS)
        .unwrap();
    assert_eq!(targets.len(), 1);

    // With the copy present, both files are planned, with a warning.
    let copy = package
        .join("LocalCache")
        .join("Roaming")
        .join("Claude")
        .join("claude_desktop_config.json");
    put(&copy, "{}\n");
    let targets = DESKTOP
        .mcp_targets(&env, &fs, Scope::User, None, &NO_PREFS)
        .unwrap();
    let files: Vec<&Path> = targets.iter().map(|t| t.file.as_path()).collect();
    assert_eq!(files, [roaming.as_path(), copy.as_path()]);
    assert!(targets[0].warnings.is_empty());
    assert_eq!(codes(&targets[1].warnings), [ErrorCode::MsixDualWrite]);
    assert_eq!(targets[1].warnings[0].path(), Some(copy.as_path()));
    for target in &targets {
        assert_eq!(target.client, "claude-desktop");
        assert_eq!(target.scope, Scope::User);
        assert_eq!(target.container.segments(), ["mcpServers"]);
    }

    // No project config anywhere.
    let project = home.projects().join("app");
    for scope in [Scope::Project, Scope::ProjectLocal] {
        let err = DESKTOP
            .mcp_targets(&env, &fs, scope, Some(&project), &NO_PREFS)
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ScopeNotSupported);
    }

    // macOS: best effort, no MSIX.
    let env = home.env(Os::MacOs);
    let targets = DESKTOP
        .mcp_targets(&env, &fs, Scope::User, None, &NO_PREFS)
        .unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(
        targets[0].file,
        home.home()
            .join("Library")
            .join("Application Support")
            .join("Claude")
            .join("claude_desktop_config.json")
    );

    // Linux: no target at all, even with a config dir present.
    std::fs::create_dir_all(home.home().join(".config").join("Claude")).unwrap();
    let env = home.env(Os::Linux);
    for scope in [Scope::User, Scope::Project] {
        let err = DESKTOP
            .mcp_targets(&env, &fs, scope, Some(&project), &NO_PREFS)
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::NotSupportedOnOs);
        assert_eq!(err.code().as_str(), "not_supported_on_os");
    }
    let err = DESKTOP
        .suppression_target(&env, &fs, &project, &NO_PREFS)
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotSupportedOnOs);
}

#[test]
fn windows_project_keys_follow_the_file() {
    let local = CODE.mcp.as_ref().unwrap().project_local_container;
    let keys = |doc: &Document, project: &str| {
        local
            .keys(Os::Windows, doc, Some(Path::new(project)))
            .unwrap()[1]
            .clone()
    };

    // Forward-slash file: an existing key is found with either slash style,
    // any case and a trailing separator, and reused as written.
    let slash = parse(&fixture("claude-code/user/.claude.json"));
    for project in [
        r"C:\work\other-app",
        "C:/work/other-app",
        r"c:\WORK\Other-App\",
    ] {
        assert_eq!(keys(&slash, project), "C:/work/other-app");
    }
    // A new key copies the file's style.
    assert_eq!(keys(&slash, r"E:\new\proj"), "E:/new/proj");

    let backslash = parse(&fixture("claude-code/windows-backslash-keys/.claude.json"));
    assert_eq!(
        keys(&backslash, "C:/work/sample-app"),
        r"C:\Work\Sample-App"
    );
    assert_eq!(keys(&backslash, "E:/new/proj"), r"E:\new\proj");

    // No drive-letter keys yet: `/` (`[verify]` against Claude Code).
    assert_eq!(
        keys(&Document::empty(Format::Json), r"E:\new\proj"),
        "E:/new/proj"
    );

    // Linux keys compare exactly.
    assert_eq!(
        local
            .keys(
                Os::Linux,
                &slash,
                Some(Path::new("/home/demo/work/linux-app/"))
            )
            .unwrap()[1],
        "/home/demo/work/linux-app"
    );

    // An edit through the reused key lands in the existing project entry.
    let mut doc = backslash.clone();
    let container = local
        .keys(Os::Windows, &doc, Some(Path::new("c:/work/sample-app")))
        .unwrap();
    let container: Vec<&str> = container.iter().map(String::as_str).collect();
    let native = write(
        CODE,
        Os::Windows,
        &stdio("lint", "npx", &["example-lint-mcp"], &[]),
    )
    .unwrap();
    assert!(doc.upsert(&container, "lint", &native).unwrap());
    assert_eq!(doc.list(&["projects"]).unwrap().len(), 2);
    insta::assert_snapshot!("claude_code_backslash_key_upsert", text(&doc));
}

#[test]
fn claude_code_suppresses_user_servers_through_disabled_mcp_servers() {
    let home = TempHome::new();
    let fs = home.fs();
    let env = home.env(Os::Windows);
    let project = home.projects().join("app");
    let target = CODE
        .suppression_target(&env, &fs, &project, &NO_PREFS)
        .unwrap()
        .unwrap();
    assert_eq!(target.file, home.home().join(".claude.json"));
    assert_eq!(target.scope, Scope::ProjectLocal);
    assert_eq!(
        target.container.segments(),
        ["projects", "{project_key}", "disabledMcpServers"]
    );
    assert!(
        DESKTOP
            .suppression_target(&env, &fs, &project, &NO_PREFS)
            .unwrap()
            .is_none()
    );

    // In a fixture file, for a project keyed with the other slash style.
    let original = fixture("claude-code/user/.claude.json");
    let mut doc = parse(&original);
    let list = target
        .container
        .keys(Os::Windows, &doc, Some(Path::new(r"C:\work\sample-app")))
        .unwrap();
    let list: Vec<&str> = list.iter().map(String::as_str).collect();
    assert_eq!(
        list,
        ["projects", "C:/work/sample-app", "disabledMcpServers"]
    );
    assert!(doc.set_list_member(&list, "notes", true).unwrap());
    assert!(!doc.set_list_member(&list, "notes", true).unwrap());
    insta::assert_snapshot!("claude_code_disable_in_project", text(&doc));
    assert!(doc.set_list_member(&list, "notes", false).unwrap());
    assert_eq!(text(&doc), original);

    // A project without an entry gets one, removed again with its member.
    let list = target
        .container
        .keys(Os::Windows, &doc, Some(Path::new(r"E:\fresh")))
        .unwrap();
    let list: Vec<&str> = list.iter().map(String::as_str).collect();
    assert!(doc.set_list_member(&list, "notes", true).unwrap());
    assert!(text(&doc).contains(r#""E:/fresh": {"#));
    assert!(doc.set_list_member(&list, "notes", false).unwrap());
    assert_eq!(text(&doc), original);
}

#[test]
fn a_target_round_trips_through_the_file_it_names() {
    let home = TempHome::new();
    let fs = home.fs();
    let env = home.env(Os::Windows);
    let original = fixture("claude-desktop/windows/claude_desktop_config.json");
    let file = home
        .appdata()
        .join("Claude")
        .join("claude_desktop_config.json");
    put(&file, &original);

    let target = DESKTOP
        .mcp_targets(&env, &fs, Scope::User, None, &NO_PREFS)
        .unwrap()
        .remove(0);
    let mut doc = Document::parse(
        fs.read_to_string(&target.file).unwrap().as_bytes(),
        target.format,
    )
    .unwrap();
    let keys = target.container_keys(&doc).unwrap();
    let container: Vec<&str> = keys.iter().map(String::as_str).collect();
    let server = stdio("docs", "npx", &["-y", "example-docs-mcp"], &[]);
    let native = write(DESKTOP, env.os(), &server).unwrap();
    assert!(doc.upsert(&container, &server.name, &native).unwrap());
    fs.write(&target.file, &doc.to_bytes()).unwrap();

    let doc = Document::parse(fs.read_to_string(&file).unwrap().as_bytes(), Format::Json).unwrap();
    let stored = doc.get(&container, "docs").unwrap().unwrap();
    let def = mapper(DESKTOP).from_native("docs", &stored).unwrap();
    assert_eq!(def.fields, fields_of(&server));
    for foreign in ["notes", "launcher", "native-tool"] {
        assert!(doc.get(&container, foreign).unwrap().is_some());
    }
    let mut doc = doc;
    assert!(doc.remove(&container, "docs").unwrap());
    assert_eq!(String::from_utf8(doc.to_bytes()).unwrap(), original);
}

// ---------------------------------------------------------------------------
// Detection

/// A registry holding uninstall entries with these display names; counts
/// lookups.
#[derive(Default)]
struct FakeRegistry {
    display_names: Vec<&'static str>,
    lookups: Cell<usize>,
}

impl RegistryProbe for FakeRegistry {
    fn has_uninstall_entry(&self, display_name: &str) -> bool {
        self.lookups.set(self.lookups.get() + 1);
        self.display_names.contains(&display_name)
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
    fn write(&self, path: &Path, _: &[u8]) -> io::Result<()> {
        panic!("detection wrote {}", path.display())
    }
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        panic!("detection created {}", path.display())
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        panic!("detection removed {}", path.display())
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        self.0.read_dir(path)
    }
    fn open_lock_file(&self, path: &Path) -> io::Result<std::fs::File> {
        panic!("detection locked {}", path.display())
    }
}

fn touch(path: &Path) {
    put(path, "");
}

#[test]
fn nothing_is_detected_in_an_empty_profile() {
    let home = TempHome::new();
    let fs = ReadOnlyFs(home.fs());
    let registry = FakeRegistry::default();
    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env_builder(os).var("PATH", home.bin()).build();
        for spec in [CODE, DESKTOP] {
            let found = detect::detect(spec, &env, &fs, &registry);
            assert!(!found.detected(), "{} on {os:?}", spec.id);
            assert!(found.evidence.is_empty());
            assert_eq!(found.client, spec.id);
        }
    }
}

#[test]
fn claude_code_detection_per_os() {
    let home = TempHome::new();
    let fs = ReadOnlyFs(home.fs());
    let registry = FakeRegistry::default();

    // Windows: `claude.exe` (native installer) or `claude.cmd` (npm) on
    // PATH, found through PATHEXT.
    let exe = home.bin().join("claude.exe");
    touch(&exe);
    let env = home
        .env_builder(Os::Windows)
        .var("PATH", home.bin())
        .build();
    let found = detect::detect(CODE, &env, &fs, &registry);
    assert!(found.detected());
    assert_eq!(found.evidence, [Evidence::Executable(exe.clone())]);
    std::fs::remove_file(&exe).unwrap();
    let cmd = home.bin().join("claude.cmd");
    touch(&cmd);
    let env = home
        .env_builder(Os::Windows)
        .var("PATH", home.bin())
        .var("PATHEXT", ".EXE;.CMD")
        .build();
    assert_eq!(
        detect::detect(CODE, &env, &fs, &registry).evidence,
        [Evidence::Executable(cmd.clone())]
    );
    let env = home
        .env_builder(Os::Windows)
        .var("PATH", home.bin())
        .var("PATHEXT", ".EXE")
        .build();
    assert!(!detect::detect(CODE, &env, &fs, &registry).detected());
    std::fs::remove_file(&cmd).unwrap();

    // Every OS: the user file or the config dir.
    let user_file = home.home().join(".claude.json");
    let config_dir = home.home().join(".claude");
    touch(&user_file);
    std::fs::create_dir_all(&config_dir).unwrap();
    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let found = detect::detect(CODE, &home.env(os), &fs, &registry);
        assert!(found.detected(), "{os:?}");
        assert_eq!(
            found.evidence,
            [
                Evidence::Path(user_file.clone()),
                Evidence::Path(config_dir.clone())
            ]
        );
    }

    // CLAUDE_CONFIG_DIR replaces both.
    let config = home.root().join("claude-config");
    let env = home
        .env_builder(Os::Linux)
        .var("CLAUDE_CONFIG_DIR", &config)
        .build();
    assert!(!detect::detect(CODE, &env, &fs, &registry).detected());
    std::fs::create_dir_all(&config).unwrap();
    assert_eq!(
        detect::detect(CODE, &env, &fs, &registry).evidence,
        [Evidence::Path(config)]
    );

    // Claude Code has no registry probe.
    assert_eq!(registry.lookups.get(), 0);
}

/// `:`-separated PATH lookup; Windows temp paths hold a drive colon, so this
/// runs on Unix hosts only (CI covers it).
#[cfg(unix)]
#[test]
fn claude_code_detection_on_a_unix_path() {
    let home = TempHome::new();
    let fs = ReadOnlyFs(home.fs());
    let claude = home.bin().join("claude");
    touch(&claude);
    let other = home.root().join("other-bin");
    std::fs::create_dir_all(&other).unwrap();
    let path = format!("relative/bin:{}:{}", other.display(), home.bin().display());
    for os in [Os::Linux, Os::MacOs] {
        let env = home.env_builder(os).var("PATH", &path).build();
        assert_eq!(
            detect::detect(CODE, &env, &fs, &detect::NoRegistry).evidence,
            [Evidence::Executable(claude.clone())]
        );
    }
}

#[test]
fn claude_desktop_detection_per_os() {
    let home = TempHome::new();
    let fs = ReadOnlyFs(home.fs());
    let installed = FakeRegistry {
        display_names: vec!["Claude"],
        ..FakeRegistry::default()
    };
    let other = FakeRegistry {
        display_names: vec!["Some Other App"],
        ..FakeRegistry::default()
    };

    // Windows: the uninstall entry alone is enough.
    let env = home.env(Os::Windows);
    let found = detect::detect(DESKTOP, &env, &fs, &installed);
    assert!(found.detected());
    assert_eq!(found.evidence, [Evidence::UninstallEntry("Claude")]);
    assert!(!detect::detect(DESKTOP, &env, &fs, &other).detected());

    // Windows: the MSIX package dir alone, or the Roaming config dir.
    let package = home
        .local_appdata()
        .join("Packages")
        .join("Claude_0000fake0000");
    std::fs::create_dir_all(&package).unwrap();
    assert_eq!(
        detect::detect(DESKTOP, &env, &fs, &other).evidence,
        [Evidence::MsixPackage(package.clone())]
    );
    let roaming = home.appdata().join("Claude");
    std::fs::create_dir_all(&roaming).unwrap();
    assert_eq!(
        detect::detect(DESKTOP, &env, &fs, &installed).evidence,
        [
            Evidence::Path(roaming),
            Evidence::UninstallEntry("Claude"),
            Evidence::MsixPackage(package)
        ]
    );

    // macOS: the Application Support dir; no registry, no MSIX.
    let lookups = installed.lookups.get();
    let env = home.env(Os::MacOs);
    assert!(!detect::detect(DESKTOP, &env, &fs, &installed).detected());
    let support = home
        .home()
        .join("Library")
        .join("Application Support")
        .join("Claude");
    std::fs::create_dir_all(&support).unwrap();
    assert_eq!(
        detect::detect(DESKTOP, &env, &fs, &installed).evidence,
        [Evidence::Path(support)]
    );

    // Linux: unsupported, so not detected whatever is on disk.
    std::fs::create_dir_all(home.home().join(".config").join("Claude")).unwrap();
    let found = detect::detect(DESKTOP, &home.env(Os::Linux), &fs, &installed);
    assert!(!found.supported);
    assert!(!found.detected());
    assert!(found.evidence.is_empty());
    assert_eq!(
        installed.lookups.get(),
        lookups,
        "registry read off Windows"
    );
}

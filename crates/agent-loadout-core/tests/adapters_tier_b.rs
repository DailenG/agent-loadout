//! Copilot, Cursor and the legacy VS Code format (SPEC 7.3, 7.4 rows
//! `copilot`, `cursor`, `vscode-legacy`; M10): mapper goldens per
//! transport, reading each client's files, targets and detection through a
//! fake `Env`, and through the apply engine: the `<P>/.mcp.json` file
//! Copilot shares with Claude Code (SPEC 6.5), per-client reference
//! syntax (SPEC 11.4), legacy VS Code files that stay byte-identical
//! through every operation while import reads them, and the effective view
//! of compatibility sources (SPEC 6.4).

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use agent_loadout_core::adapters::detect::{self, Evidence, NoRegistry};
use agent_loadout_core::adapters::mapper::{
    EntryMapper, Launch, MapError, RemoteLaunch, RenderedServer, ServerDef, StdioLaunch, WriteCx,
};
use agent_loadout_core::adapters::{
    ADAPTERS, AdapterSpec, McpSpec, Target, Tier, claude_code, copilot, cursor, vscode_legacy,
};
use agent_loadout_core::apply::import::{scan, stage_import};
use agent_loadout_core::apply::render::DesiredState;
use agent_loadout_core::apply::{EntryState, Plan, PlanOptions, TargetPlan};
use agent_loadout_core::formats::{Document, Format, NativeValue};
use agent_loadout_core::layering::{EffectiveView, SHARED_FILE, effective};
use agent_loadout_core::model::applied::Scope;
use agent_loadout_core::model::clients::{ClientPrefs, ClientPrefsFile};
use agent_loadout_core::model::server::{ServerFields, Transport};
use agent_loadout_core::{Env, ErrorCode, Os};
use agent_loadout_testkit::TempHome;
use serde_json::Value;
use support::{Fx, host_os, json_entry, stdio as store_stdio};

const COPILOT: &AdapterSpec = &copilot::SPEC;
const CURSOR: &AdapterSpec = &cursor::SPEC;
const LEGACY: &AdapterSpec = &vscode_legacy::SPEC;
const CODE: &AdapterSpec = &claude_code::SPEC;

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

/// A fixture's bytes as checked out.
fn fixture_bytes(relative: &str) -> Vec<u8> {
    std::fs::read(fixture_path(relative)).unwrap()
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
    for (key, value) in def.passthrough.iter() {
        out.push_str(&format!("# passthrough {key}: {value:?}\n"));
    }
    for (key, value) in def.unmapped.iter() {
        out.push_str(&format!("# unmapped {key}: {value:?}\n"));
    }
    out
}

/// The file a new entry makes and what it reads back as; checks the round
/// trip.
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
    assert_eq!(def.passthrough, server.passthrough);
    assert!(def.unmapped.is_empty(), "{:?}", def.unmapped);
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
    }
    out
}

// ---------------------------------------------------------------------------
// Table

#[test]
fn tier_b_adapters_are_registered() {
    // Tier A and B in table order; later tiers add their own rows.
    let ids: Vec<&str> = ADAPTERS
        .iter()
        .map(|spec| spec.id)
        .filter(|id| {
            [
                "claude-code",
                "omp",
                "pi",
                "codex",
                "claude-desktop",
                "copilot",
                "cursor",
                "vscode-legacy",
            ]
            .contains(id)
        })
        .collect();
    assert_eq!(
        ids,
        [
            "claude-code",
            "omp",
            "pi",
            "codex",
            "claude-desktop",
            "copilot",
            "cursor",
            "vscode-legacy"
        ]
    );
    for spec in [COPILOT, CURSOR] {
        assert_eq!(spec.tier, Tier::B, "{}", spec.id);
        assert!(!mcp(spec).read_only, "{}", spec.id);
    }
    assert_eq!(LEGACY.tier, Tier::D);
    assert!(mcp(LEGACY).read_only);
    assert_eq!(mcp(LEGACY).format, Format::Jsonc);
    assert_eq!(servers(LEGACY), ["servers"]);
    // Copilot's workspace file is Claude Code's project file and container.
    assert_eq!(mcp(COPILOT).project, mcp(CODE).project);
    assert_eq!(mcp(COPILOT).container, mcp(CODE).container);
}

// ---------------------------------------------------------------------------
// Mappers

#[test]
fn copilot_goldens_per_transport() {
    let mut github = stdio(
        "github",
        "${TOOLS_BIN}/github-mcp",
        &["--root=${WORK_ROOT}"],
        &[("GITHUB_PERSONAL_ACCESS_TOKEN", "${GITHUB_PAT}")],
    );
    stdio_mut(&mut github).cwd = Some("${HOME}/src".to_owned());
    github.tools.allow = Some(owned(&["search_code", "get_issue"]));
    insta::assert_snapshot!("copilot_stdio", golden(COPILOT, Os::Windows, &github));
    // No `cmd /c` shim (`[verify]` COP-9): every OS writes the same.
    assert_eq!(
        golden(COPILOT, Os::Windows, &github),
        golden(COPILOT, Os::Linux, &github)
    );

    let mut tracker = remote(
        "tracker",
        Transport::Http,
        "https://tracker.example.com/mcp",
        &[("Authorization", "Bearer ${TRACKER_TOKEN:-anonymous}")],
    );
    tracker
        .passthrough
        .insert("oauthClientId", NativeValue::from("example-client"));
    insta::assert_snapshot!("copilot_http", golden(COPILOT, Os::Linux, &tracker));

    let events = remote(
        "events",
        Transport::Sse,
        "https://events.example.com/sse",
        &[],
    );
    insta::assert_snapshot!("copilot_sse", golden(COPILOT, Os::Linux, &events));

    // An allow list only; no native disable.
    let mut denied = stdio("denied", "example-mcp", &[], &[]);
    denied.tools.deny = Some(owned(&["delete"]));
    assert_eq!(
        write(COPILOT, Os::Linux, &denied).unwrap_err(),
        MapError::UnsupportedField {
            field: "tools.deny".to_owned()
        }
    );
    let mut disabled = stdio("disabled", "example-mcp", &[], &[]);
    disabled.enabled = false;
    assert_eq!(
        write(COPILOT, Os::Linux, &disabled).unwrap_err().code(),
        ErrorCode::UnsupportedField
    );
}

#[test]
fn copilot_never_emits_editor_only_fields() {
    let existing = json(
        r#"{
            "type": "stdio",
            "command": "example-mcp",
            "inputs": [{"id": "token", "type": "promptString"}],
            "envFile": "${workspaceFolder}/.env",
            "dev": {"watch": "src/**"},
            "sandbox": {"network": false},
            "description": "kept"
        }"#,
    );
    let written = mapper(COPILOT)
        .to_native(
            &stdio("example", "example-mcp", &["--v2"], &[]),
            &WriteCx {
                os: Os::Linux,
                existing: Some(&existing),
            },
        )
        .unwrap();
    let entry = written.as_object().unwrap();
    for key in copilot::EDITOR_ONLY {
        assert!(!entry.contains_key(key), "{key}");
    }
    // Other unmodeled keys survive an update.
    assert_eq!(entry.get("description"), Some(&NativeValue::from("kept")));
    // Editor-only keys are not passthrough fields either.
    let mut server = stdio("example", "example-mcp", &[], &[]);
    server
        .passthrough
        .insert("envFile", NativeValue::from(".env"));
    assert_eq!(
        write(COPILOT, Os::Linux, &server).unwrap_err(),
        MapError::UnsupportedField {
            field: "envFile".to_owned()
        }
    );
}

#[test]
fn copilot_reads_the_cli_spellings() {
    let mapper = mapper(COPILOT);
    // `"type": "local"` is stdio and `"tools": ["*"]` is no filter.
    let cli = json(r#"{"type": "local", "command": "npx", "args": ["-y", "x"], "tools": ["*"]}"#);
    let ours = write(COPILOT, Os::Linux, &stdio("x", "npx", &["-y", "x"], &[])).unwrap();
    assert_eq!(mapper.canonical(&cli), mapper.canonical(&ours));
    let def = mapper.from_native("x", &cli).unwrap();
    assert_eq!(def.fields.transport, Some(Transport::Stdio));
    assert!(def.tools.is_empty());
    // A real filter is kept.
    let filtered = json(r#"{"type": "local", "command": "npx", "tools": ["read"]}"#);
    assert_ne!(mapper.canonical(&filtered), mapper.canonical(&ours));
    assert_eq!(
        mapper.from_native("x", &filtered).unwrap().tools.allow,
        Some(owned(&["read"]))
    );
}

#[test]
fn cursor_goldens_per_transport() {
    let mut github = stdio(
        "github",
        "npx",
        &["-y", "@modelcontextprotocol/server-github"],
        &[("GITHUB_PERSONAL_ACCESS_TOKEN", "${env:GITHUB_PAT}")],
    );
    github
        .passthrough
        .insert("envFile", NativeValue::from("${userHome}/.example.env"));
    insta::assert_snapshot!("cursor_stdio", golden(CURSOR, Os::Windows, &github));
    assert_eq!(
        golden(CURSOR, Os::Windows, &github),
        golden(CURSOR, Os::Linux, &github)
    );

    let tracker = remote(
        "tracker",
        Transport::Http,
        "https://tracker.example.com/mcp",
        &[("Authorization", "Bearer ${env:TRACKER_TOKEN}")],
    );
    insta::assert_snapshot!("cursor_http", golden(CURSOR, Os::Linux, &tracker));

    let events = remote(
        "events",
        Transport::Sse,
        "https://events.example.com/sse",
        &[],
    );
    insta::assert_snapshot!("cursor_sse", golden(CURSOR, Os::Linux, &events));

    // No `cwd`, tool filter or native disable.
    let blocked = |server: &RenderedServer| write(CURSOR, Os::Linux, server).unwrap_err();
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
    assert_eq!(
        blocked(&tools),
        MapError::UnsupportedField {
            field: "tools".to_owned()
        }
    );
    let mut disabled = stdio("disabled", "example-mcp", &[], &[]);
    disabled.enabled = false;
    assert_eq!(blocked(&disabled).code(), ErrorCode::UnsupportedField);
}

#[test]
fn tier_b_clients_read_their_files() {
    // Copilot: `local` is stdio, `tools: ["*"]` no filter, `oauthClientId`
    // a passthrough field, editor-only keys unmapped.
    for (name, relative) in [
        ("copilot_read_user", "copilot/user/mcp-config.json"),
        ("copilot_read_project", "copilot/project/.mcp.json"),
    ] {
        let doc = parse(COPILOT, &fixture(relative));
        insta::assert_snapshot!(name, read_all(COPILOT, &doc));
    }
    // Cursor: `envFile` passthrough; `${env:NAME}` read as written.
    for (name, relative) in [
        ("cursor_read_user", "cursor/user/mcp.json"),
        ("cursor_read_project", "cursor/project/.cursor/mcp.json"),
    ] {
        let doc = parse(CURSOR, &fixture(relative));
        insta::assert_snapshot!(name, read_all(CURSOR, &doc));
    }
    // Legacy VS Code: JSONC with comments and trailing commas; `envFile`,
    // `dev` and `sandbox` unmapped; `inputs[]` is outside the container.
    for (name, relative) in [
        ("vscode_legacy_read_user", "vscode-legacy/user/mcp.json"),
        (
            "vscode_legacy_read_profile",
            "vscode-legacy/profile/mcp.json",
        ),
        (
            "vscode_legacy_read_project",
            "vscode-legacy/project/.vscode/mcp.json",
        ),
    ] {
        let doc = parse(LEGACY, &fixture(relative));
        insta::assert_snapshot!(name, read_all(LEGACY, &doc));
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

fn user_files(
    spec: &AdapterSpec,
    env: &Env,
    fs: &dyn agent_loadout_core::Fs,
    prefs: &ClientPrefs,
) -> Vec<PathBuf> {
    files(&spec.mcp_targets(env, fs, Scope::User, None, prefs).unwrap())
}

/// `<AppData>` of `os` in a temp home, as VS Code uses it.
fn app_data(home: &TempHome, os: Os) -> PathBuf {
    match os {
        Os::Windows => home.appdata(),
        Os::MacOs => home.home().join("Library").join("Application Support"),
        Os::Linux => home.home().join(".config"),
    }
}

fn prefs_of(text: &str, client: &str) -> ClientPrefs {
    let file: ClientPrefsFile = toml::from_str(text).unwrap();
    assert!(file.validate().is_empty(), "{text}");
    file.clients.get(client).cloned().unwrap_or_default()
}

#[test]
fn copilot_and_cursor_targets() {
    let home = TempHome::new();
    let fs = home.fs();
    let project = home.projects().join("app");
    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env(os);
        assert_eq!(
            user_files(COPILOT, &env, &fs, &NO_PREFS),
            [home.home().join(".copilot").join("mcp-config.json")]
        );
        let shared = COPILOT
            .mcp_targets(&env, &fs, Scope::Project, Some(&project), &NO_PREFS)
            .unwrap();
        assert_eq!(files(&shared), [project.join(".mcp.json")]);
        assert_eq!(shared[0].container.segments(), ["mcpServers"]);

        assert_eq!(
            user_files(CURSOR, &env, &fs, &NO_PREFS),
            [home.home().join(".cursor").join("mcp.json")]
        );
        let shared = CURSOR
            .mcp_targets(&env, &fs, Scope::Project, Some(&project), &NO_PREFS)
            .unwrap();
        assert_eq!(files(&shared), [project.join(".cursor").join("mcp.json")]);

        for spec in [COPILOT, CURSOR, LEGACY] {
            let err = spec
                .mcp_targets(&env, &fs, Scope::ProjectLocal, Some(&project), &NO_PREFS)
                .unwrap_err();
            assert_eq!(err.code(), ErrorCode::ScopeNotSupported, "{}", spec.id);
        }
    }

    // COPILOT_HOME moves the user file; a relative value is ignored.
    let copilot_home = home.root().join("copilot-home");
    let env = home
        .env_builder(Os::Windows)
        .var("COPILOT_HOME", &copilot_home)
        .build();
    assert_eq!(
        user_files(COPILOT, &env, &fs, &NO_PREFS),
        [copilot_home.join("mcp-config.json")]
    );
    let env = home
        .env_builder(Os::Linux)
        .var("COPILOT_HOME", "relative/copilot")
        .build();
    assert_eq!(
        user_files(COPILOT, &env, &fs, &NO_PREFS),
        [home.home().join(".copilot").join("mcp-config.json")]
    );
}

#[test]
fn legacy_vscode_targets_follow_variants_and_profiles() {
    let home = TempHome::new();
    let fs = home.fs();
    let project = home.projects().join("app");
    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env(os);
        let user = app_data(&home, os).join("Code").join("User");
        assert_eq!(
            user_files(LEGACY, &env, &fs, &NO_PREFS),
            [user.join("mcp.json")]
        );
        let shared = LEGACY
            .mcp_targets(&env, &fs, Scope::Project, Some(&project), &NO_PREFS)
            .unwrap();
        assert_eq!(files(&shared), [project.join(".vscode").join("mcp.json")]);
        assert_eq!(shared[0].format, Format::Jsonc);
        assert_eq!(shared[0].container.segments(), ["servers"]);
    }

    // Linux: $XDG_CONFIG_HOME when absolute.
    let xdg = home.xdg();
    let env = home
        .env_builder(Os::Linux)
        .var("XDG_CONFIG_HOME", &xdg)
        .build();
    assert_eq!(
        user_files(LEGACY, &env, &fs, &NO_PREFS),
        [xdg.join("Code").join("User").join("mcp.json")]
    );

    // Variants and profiles from clients.toml.
    let os = Os::Windows;
    let env = home.env(os);
    let base = app_data(&home, os);
    let code = base.join("Code").join("User");
    let insiders = base.join("Code - Insiders").join("User");
    let codium = base.join("VSCodium").join("User");
    put(&code.join("profiles").join("b2").join("mcp.json"), "{}");
    put(&code.join("profiles").join("a1").join("mcp.json"), "{}");
    std::fs::create_dir_all(code.join("profiles").join("empty")).unwrap();
    put(&insiders.join("profiles").join("i9").join("mcp.json"), "{}");

    let all = prefs_of(
        "schema = 1\n[vscode-legacy]\nimport_variants = [\"Code\", \"Code - Insiders\", \"VSCodium\"]\nimport_profiles = \"all\"\n",
        "vscode-legacy",
    );
    let targets = LEGACY
        .mcp_targets(&env, &fs, Scope::User, None, &all)
        .unwrap();
    assert_eq!(
        files(&targets),
        [
            code.join("mcp.json"),
            code.join("profiles").join("a1").join("mcp.json"),
            code.join("profiles").join("b2").join("mcp.json"),
            insiders.join("mcp.json"),
            insiders.join("profiles").join("i9").join("mcp.json"),
            codium.join("mcp.json"),
        ]
    );
    let labels: Vec<Option<&str>> = targets.iter().map(|t| t.profile.as_deref()).collect();
    assert_eq!(
        labels,
        [
            None,
            Some("Code/profiles/a1"),
            Some("Code/profiles/b2"),
            Some("Code - Insiders"),
            Some("Code - Insiders/profiles/i9"),
            Some("VSCodium"),
        ]
    );

    // A list names profiles (`default` is `User/mcp.json`), existing or not.
    let listed = prefs_of(
        "schema = 1\n[vscode-legacy]\nimport_profiles = [\"b2\", \"default\", \"missing\"]\n",
        "vscode-legacy",
    );
    assert_eq!(
        user_files(LEGACY, &env, &fs, &listed),
        [
            code.join("profiles").join("b2").join("mcp.json"),
            code.join("mcp.json"),
            code.join("profiles").join("missing").join("mcp.json"),
        ]
    );
    let escape = prefs_of(
        "schema = 1\n[vscode-legacy]\nimport_profiles = [\"../escape\"]\n",
        "vscode-legacy",
    );
    let err = LEGACY
        .mcp_targets(&env, &fs, Scope::User, None, &escape)
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);
}

#[test]
fn tier_b_detection_per_os() {
    let home = TempHome::new();
    let fs = home.fs();
    let registry = NoRegistry;
    let evidence =
        |spec: &AdapterSpec, env: &Env| detect::detect(spec, env, &fs, &registry).evidence;

    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env_builder(os).var("PATH", home.bin()).build();
        for spec in [COPILOT, CURSOR, LEGACY] {
            let found = detect::detect(spec, &env, &fs, &registry);
            assert!(found.supported, "{} on {os:?}", spec.id);
            assert!(!found.detected(), "{} on {os:?}", spec.id);
        }
    }

    let copilot_dir = home.home().join(".copilot");
    let chat = home
        .home()
        .join(".vscode")
        .join("extensions")
        .join("github.copilot-chat-0.30.0");
    let cursor_dir = home.home().join(".cursor");
    for dir in [&copilot_dir, &chat, &cursor_dir] {
        std::fs::create_dir_all(dir).unwrap();
    }
    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env(os);
        assert_eq!(
            evidence(COPILOT, &env),
            [
                Evidence::Path(copilot_dir.clone()),
                Evidence::Path(chat.clone())
            ]
        );
        assert_eq!(evidence(CURSOR, &env), [Evidence::Path(cursor_dir.clone())]);
        let code_user = app_data(&home, os).join("Code").join("User");
        std::fs::create_dir_all(&code_user).unwrap();
        assert_eq!(evidence(LEGACY, &env), [Evidence::Path(code_user)]);
    }

    // COPILOT_HOME replaces `~/.copilot`.
    let copilot_home = home.root().join("copilot-home");
    std::fs::create_dir_all(&copilot_home).unwrap();
    let env = home
        .env_builder(Os::Linux)
        .var("COPILOT_HOME", &copilot_home)
        .build();
    assert_eq!(evidence(COPILOT, &env)[0], Evidence::Path(copilot_home));

    // Executables on a Windows PATH, through PATHEXT.
    for name in ["copilot.cmd", "cursor.exe", "code.cmd"] {
        put(&home.bin().join(name), "");
    }
    let env = home
        .env_builder(Os::Windows)
        .var("PATH", home.bin())
        .var("PATHEXT", ".EXE;.CMD")
        .build();
    assert!(
        evidence(COPILOT, &env).contains(&Evidence::Executable(home.bin().join("copilot.cmd")))
    );
    assert!(evidence(CURSOR, &env).contains(&Evidence::Executable(home.bin().join("cursor.exe"))));
    assert!(evidence(LEGACY, &env).contains(&Evidence::Executable(home.bin().join("code.cmd"))));
}

/// Gate V2 CUR-9: the per-user installer registers "Cursor (User)"; an
/// exact `DisplayName` match finds it as well as a machine-wide "Cursor".
#[test]
fn cursor_uninstall_entries_per_installer() {
    struct Names(Vec<&'static str>);
    impl detect::RegistryProbe for Names {
        fn has_uninstall_entry(&self, display_name: &str) -> bool {
            self.0.contains(&display_name)
        }
    }
    let home = TempHome::new();
    let fs = home.fs();
    let env = home.env(Os::Windows);
    for name in ["Cursor", "Cursor (User)"] {
        let found = detect::detect(CURSOR, &env, &fs, &Names(vec![name]));
        assert_eq!(found.evidence, [Evidence::UninstallEntry(name)]);
    }
    let found = detect::detect(CURSOR, &env, &fs, &Names(vec!["Cursor Helper"]));
    assert!(!found.detected());
}

// ---------------------------------------------------------------------------
// Through the apply engine

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn target<'a>(plan: &'a Plan, file: &Path) -> &'a TargetPlan {
    plan.targets
        .iter()
        .find(|target| target.file == file)
        .unwrap_or_else(|| panic!("no target {} in {plan:#?}", file.display()))
}

/// A project `<root>/projects/<name>` with `project.toml` holding `body`,
/// known on this machine and trusted.
fn project(fx: &Fx, name: &str, id: &str, body: &str) -> PathBuf {
    let root = fx.th.projects().join(name);
    std::fs::create_dir_all(&root).unwrap();
    fx.manifest(
        &root,
        SHARED_FILE,
        &format!("schema = 1\nid = \"{id}\"\nname = \"{name}\"\n\n{body}"),
    );
    fx.know_projects(&[(id, &root)]);
    fx.trust(&root);
    root
}

#[test]
fn copilot_shares_the_project_mcp_json_with_claude_code() {
    let mut fx = Fx::new();
    let copilot_home = fx.th.root().join("copilot-home");
    fx.env = fx
        .th
        .env_builder(host_os())
        .hostname("test-desk")
        .var("COPILOT_HOME", &copilot_home)
        .build();
    fx.put_server(store_stdio(
        "github",
        &["claude-code", "copilot", "cursor"],
        &["-y", "@modelcontextprotocol/server-github"],
    ));
    let root = project(
        &fx,
        "shared",
        "shared-a1b2",
        "[defaults]\nclients = [\"claude-code\", \"copilot\"]\n\n\
         [servers.docs]\ntransport = \"stdio\"\ncommand = \"docs-mcp\"\n\n\
         [servers.claude-only]\ntransport = \"stdio\"\ncommand = \"claude-only-mcp\"\nclients = [\"claude-code\"]\n\n\
         [servers.copilot-only]\ntransport = \"stdio\"\ncommand = \"copilot-only-mcp\"\ncwd = \"src\"\nclients = [\"copilot\"]\n\n\
         [servers.cursor-only]\ntransport = \"stdio\"\ncommand = \"cursor-only-mcp\"\nclients = [\"cursor\"]\n",
    );

    let plan = fx.plan();
    let shared = root.join(".mcp.json");
    let targets: Vec<&TargetPlan> = plan
        .targets
        .iter()
        .filter(|target| target.file == shared)
        .collect();
    assert_eq!(targets.len(), 1, "{plan:#?}");
    let mut clients = targets[0].clients.clone();
    clients.sort_unstable();
    assert_eq!(clients, ["claude-code", "copilot"]);

    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");
    let text = fx.read(&shared);
    // The union of both clients' entries, in one file.
    let names: Vec<String> = read_json(&shared)["mcpServers"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    // (`serde_json` sorts object keys; the file order is in the snapshot.)
    assert_eq!(names, ["claude-only", "copilot-only", "docs"]);
    assert_eq!(json_entry(&text, "copilot-only").unwrap()["cwd"], "src");
    insta::assert_snapshot!("shared_project_mcp_json", text.replace("\r\n", "\n"));

    // User files: $COPILOT_HOME and ~/.cursor; Cursor's project file.
    let copilot_user = copilot_home.join("mcp-config.json");
    assert_eq!(
        json_entry(&fx.read(&copilot_user), "github").unwrap()["type"],
        "stdio"
    );
    assert!(!fx.th.home().join(".copilot").exists());
    let cursor_user = fx.th.home().join(".cursor").join("mcp.json");
    let github = json_entry(&fx.read(&cursor_user), "github").unwrap();
    assert!(github.get("type").is_none(), "{github}");
    let cursor_project = root.join(".cursor").join("mcp.json");
    assert!(json_entry(&fx.read(&cursor_project), "cursor-only").is_some());

    // A second run writes nothing; removing Claude Code from the defaults
    // keeps the entries Copilot still wants.
    assert!(!fx.plan().has_work(), "{:#?}", fx.plan());
    fx.manifest(
        &root,
        SHARED_FILE,
        "schema = 1\nid = \"shared-a1b2\"\nname = \"shared\"\n\n[defaults]\nclients = [\"copilot\"]\n\n\
         [servers.docs]\ntransport = \"stdio\"\ncommand = \"docs-mcp\"\n",
    );
    fx.trust(&root);
    let (report, _) = fx.apply();
    let names: Vec<String> = read_json(&shared)["mcpServers"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(names, ["docs"], "{report:#?}");
}

#[test]
fn clients_sharing_a_file_must_agree_on_an_entry() {
    let fx = Fx::new();
    // Claude Code has no `cwd` and no tool filter: the shared name blocks
    // instead of one client's entry overwriting the other's.
    let root = project(
        &fx,
        "disagree",
        "disagree-c3d4",
        "[servers.filtered]\ntransport = \"stdio\"\ncommand = \"filtered-mcp\"\n\
         clients = [\"claude-code\", \"copilot\"]\n[servers.filtered.tools]\nallow = [\"read\"]\n",
    );
    let plan = fx.plan();
    let entry = support::entry(&plan, &root.join(".mcp.json"), "filtered");
    assert_eq!(entry.state, EntryState::Blocked, "{entry:#?}");
    fx.apply();
    assert!(
        !root.join(".mcp.json").exists()
            || json_entry(&fx.read(&root.join(".mcp.json")), "filtered").is_none()
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
             [enable]\nclients = [\"copilot\", \"cursor\"]\n",
        )?;
        tx.write(
            "servers/tracker.toml",
            "schema = 1\nid = \"tracker\"\ntransport = \"http\"\nurl = \"https://tracker.example.test/mcp\"\n\
             [headers]\nAuthorization = \"Bearer ${env:TRACKER_TOKEN}\"\n\
             [tools]\nallow = [\"search\"]\n[enable]\nclients = [\"copilot\"]\n",
        )
    });
    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");

    let copilot_file = fx.th.home().join(".copilot").join("mcp-config.json");
    let tools = json_entry(&fx.read(&copilot_file), "tools").unwrap();
    assert_eq!(tools["command"], "${TOOLS_BIN}/tools-mcp");
    assert_eq!(tools["args"][0], "--root=${WORK_ROOT}");
    assert_eq!(tools["env"]["EXAMPLE_TOKEN"], "${EXAMPLE_TOKEN}");
    let tracker = json_entry(&fx.read(&copilot_file), "tracker").unwrap();
    assert_eq!(
        tracker["headers"]["Authorization"],
        "Bearer ${TRACKER_TOKEN}"
    );
    assert_eq!(tracker["tools"], serde_json::json!(["search"]));
    for key in copilot::EDITOR_ONLY {
        assert!(
            tools.get(key).is_none() && tracker.get(key).is_none(),
            "{key}"
        );
    }

    let cursor_file = fx.th.home().join(".cursor").join("mcp.json");
    let tools = json_entry(&fx.read(&cursor_file), "tools").unwrap();
    assert_eq!(tools["command"], "${env:TOOLS_BIN}/tools-mcp");
    assert_eq!(tools["args"][0], "--root=${env:WORK_ROOT}");
    assert_eq!(tools["env"]["EXAMPLE_TOKEN"], "${env:EXAMPLE_TOKEN}");
}

/// The legacy VS Code files of a fixture home: Code's user file and a
/// profile, Insiders' and VSCodium's user files, and `<P>/.vscode/mcp.json`.
struct Legacy {
    files: Vec<(PathBuf, Vec<u8>)>,
}

impl Legacy {
    fn plant(fx: &Fx, root: &Path) -> Self {
        // `<AppData>/Code/User/mcp.json` is the default file: three levels
        // below `<AppData>`.
        let default = LEGACY
            .mcp_targets(&fx.env, &fx.fs, Scope::User, None, &NO_PREFS)
            .unwrap()
            .swap_remove(0)
            .file;
        let base = default.ancestors().nth(3).unwrap().to_path_buf();
        let files = vec![
            (
                base.join("Code").join("User").join("mcp.json"),
                fixture_bytes("vscode-legacy/user/mcp.json"),
            ),
            (
                base.join("Code")
                    .join("User")
                    .join("profiles")
                    .join("5f2e1a")
                    .join("mcp.json"),
                fixture_bytes("vscode-legacy/profile/mcp.json"),
            ),
            (
                base.join("Code - Insiders").join("User").join("mcp.json"),
                fixture_bytes("vscode-legacy/profile/mcp.json"),
            ),
            (
                base.join("VSCodium").join("User").join("mcp.json"),
                fixture_bytes("vscode-legacy/project/.vscode/mcp.json"),
            ),
            (
                root.join(".vscode").join("mcp.json"),
                fixture_bytes("vscode-legacy/project/.vscode/mcp.json"),
            ),
        ];
        for (path, bytes) in &files {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        Self { files }
    }

    fn assert_untouched(&self, op: &str) {
        for (path, bytes) in &self.files {
            assert_eq!(
                &std::fs::read(path).unwrap(),
                bytes,
                "{op} changed {}",
                path.display()
            );
        }
    }

    fn in_plan(&self, plan: &Plan) -> bool {
        plan.targets
            .iter()
            .any(|target| self.files.iter().any(|(path, _)| *path == target.file))
    }
}

const ALL_VSCODE: &str = "schema = 1\n[vscode-legacy]\nimport_variants = [\"Code\", \"Code - Insiders\", \"VSCodium\"]\nimport_profiles = \"all\"\n";

#[test]
fn legacy_vscode_files_are_byte_identical_after_every_op() {
    let fx = Fx::new();
    let root = fx.th.projects().join("legacy");
    std::fs::create_dir_all(&root).unwrap();
    let legacy = Legacy::plant(&fx, &root);
    fx.edit("prefs", |tx| tx.write("clients.toml", ALL_VSCODE));
    // Named for vscode-legacy at system and project scope, and selected
    // through `all-detected` (the VS Code user dir exists).
    fx.put_server(store_stdio(
        "github",
        &["copilot", "vscode-legacy"],
        &["-y", "github-mcp"],
    ));
    let mut detected = store_stdio("detected", &[], &["-y", "detected-mcp"]);
    detected.enable = Some(agent_loadout_core::model::server::Enable {
        clients: Some(agent_loadout_core::model::clients::ClientSelection::AllDetected),
        ..Default::default()
    });
    fx.put_server(detected);
    let root = project(
        &fx,
        "legacy",
        "legacy-e5f6",
        "[servers.repo-docs]\ntransport = \"http\"\nurl = \"https://docs.example.com/mcp\"\n\
         clients = [\"copilot\", \"vscode-legacy\"]\n",
    );
    legacy.assert_untouched("setup");

    let plan = fx.plan();
    assert!(!legacy.in_plan(&plan), "{plan:#?}");
    assert!(
        plan.warnings
            .iter()
            .any(|warning| warning.code() == ErrorCode::ReadOnlyClient),
        "{:?}",
        plan.warnings
    );
    assert_eq!(ErrorCode::ReadOnlyClient.as_str(), "read_only_client");
    legacy.assert_untouched("plan");

    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");
    assert!(json_entry(&fx.read(&root.join(".mcp.json")), "repo-docs").is_some());
    legacy.assert_untouched("apply");

    // Import: scan, stage, adopt identical entries, apply.
    let desired = DesiredState::load(&fx.store(), None).unwrap();
    let (candidates, _) = scan(&fx.ax(), &desired, &fx.ledger(), None).unwrap();
    legacy.assert_untouched("import --scan");
    fx.edit("import", |tx| {
        stage_import(tx, &candidates, None, false)?;
        Ok(())
    });
    let desired = DesiredState::load(&fx.store(), None).unwrap();
    let (candidates, _) = scan(&fx.ax(), &desired, &fx.ledger(), None).unwrap();
    let ax = fx.ax();
    fx.edit("adopt", |tx| {
        agent_loadout_core::apply::import::stage_adopt_identical(tx, &ax, &desired, &candidates)?;
        Ok(())
    });
    let (report, _) = fx.apply();
    assert!(
        !report
            .targets
            .iter()
            .any(|target| legacy.files.iter().any(|(p, _)| *p == target.file))
    );
    legacy.assert_untouched("import and apply");

    // The effective view reads them.
    let store = fx.store();
    effective(Some(&store), &fx.ax(), &root, None).unwrap();
    effective(Some(&store), &fx.ax(), &root, Some(&["copilot".to_owned()])).unwrap();
    let err = effective(
        Some(&store),
        &fx.ax(),
        &root,
        Some(&["vscode-legacy".to_owned()]),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ReadOnlyClient);
    legacy.assert_untouched("effective");

    // Update and remove every server, then apply.
    fx.put_server(store_stdio(
        "github",
        &["copilot", "vscode-legacy"],
        &["-y", "github-mcp", "--v2"],
    ));
    fx.apply();
    legacy.assert_untouched("update");
    let ids: Vec<String> = DesiredState::load(&fx.store(), None)
        .unwrap()
        .servers
        .keys()
        .cloned()
        .collect();
    for id in ids {
        fx.remove_server(&id);
    }
    fx.manifest(
        &root,
        SHARED_FILE,
        "schema = 1\nid = \"legacy-e5f6\"\nname = \"legacy\"\n",
    );
    fx.trust(&root);
    fx.apply();
    legacy.assert_untouched("remove");
}

#[test]
fn import_reads_legacy_vscode_files() {
    let fx = Fx::new();
    let root = fx.th.projects().join("legacy");
    std::fs::create_dir_all(&root).unwrap();
    let legacy = Legacy::plant(&fx, &root);
    fx.edit("prefs", |tx| tx.write("clients.toml", ALL_VSCODE));
    // Copilot holds an identical `github` (Copilot's own reference syntax).
    fx.write(
        &fx.th.home().join(".copilot").join("mcp-config.json"),
        r#"{ "mcpServers": { "github": { "type": "stdio", "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-github"],
            "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_PAT}" } } } }"#,
    );

    let desired = DesiredState::load(&fx.store(), None).unwrap();
    let (candidates, _) = scan(&fx.ax(), &desired, &fx.ledger(), None).unwrap();
    let candidate = |name: &str| {
        candidates
            .iter()
            .find(|candidate| candidate.name == name)
            .unwrap_or_else(|| panic!("no candidate {name}: {candidates:#?}"))
    };
    // `${env:NAME}` reads as a store reference, identical to Copilot's
    // `${NAME}`.
    let github = candidate("github");
    assert!(github.identical, "{github:#?}");
    let clients: Vec<&str> = github.sources.iter().map(|s| s.client).collect();
    assert_eq!(clients, ["copilot", "vscode-legacy"]);
    let def = github.sources[1].def.as_ref().unwrap();
    assert_eq!(
        def.fields.env.as_ref().unwrap()["GITHUB_PERSONAL_ACCESS_TOKEN"],
        "${env:GITHUB_PAT}"
    );
    // `${input:...}` has no store form.
    let err = candidate("tracker").sources[0].def.as_ref().unwrap_err();
    assert_eq!(err.code(), ErrorCode::NeedsManualSecret);
    assert_eq!(err.code().as_str(), "needs_manual_secret");
    // A profile file is read too (`import_profiles = "all"`).
    assert_eq!(
        candidate("profile-notes").sources[0].client,
        "vscode-legacy"
    );
    candidate("editor-only");

    fx.edit("import", |tx| {
        let result = stage_import(tx, &candidates, None, false)?;
        assert!(
            result.imported.contains(&"github".to_owned()),
            "{result:#?}"
        );
        assert!(
            result.imported.contains(&"profile-notes".to_owned()),
            "{result:#?}"
        );
        let skipped: Vec<(&str, ErrorCode)> = result
            .skipped
            .iter()
            .map(|(name, err)| (name.as_str(), err.code()))
            .collect();
        assert!(
            skipped.contains(&("tracker", ErrorCode::NeedsManualSecret)),
            "{skipped:?}"
        );
        Ok(())
    });
    let desired = DesiredState::load(&fx.store(), None).unwrap();
    let enabled = |id: &str| -> Option<Vec<String>> {
        match desired.servers[id].enable.as_ref()?.clients.as_ref()? {
            agent_loadout_core::model::clients::ClientSelection::List(ids) => Some(ids.clone()),
            agent_loadout_core::model::clients::ClientSelection::AllDetected => None,
        }
    };
    // Enabled for Copilot only; found only in VS Code: enabled for nobody.
    assert_eq!(enabled("github"), Some(owned(&["copilot"])));
    assert_eq!(enabled("profile-notes"), None);
    let meta = desired.servers["profile-notes"].body.meta.as_ref().unwrap();
    assert_eq!(meta.origin.as_deref(), Some("imported:vscode-legacy"));
    assert_eq!(
        desired.servers["profile-notes"].body.env.as_ref().unwrap()["LOG_LEVEL"],
        "info"
    );

    // The apply adopts Copilot's entry without a write and never touches
    // VS Code's files.
    let plan = fx.plan_with(&PlanOptions::default());
    let copilot_file = fx.th.home().join(".copilot").join("mcp-config.json");
    assert_eq!(target(&plan, &copilot_file).after, None, "{plan:#?}");
    assert!(!legacy.in_plan(&plan));
    fx.apply();
    legacy.assert_untouched("import");
}

// ---------------------------------------------------------------------------
// Effective view

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

#[test]
fn effective_view_lists_legacy_vscode_as_copilot_compat() {
    let fx = Fx::new();
    let root = fx.th.projects().join("compat");
    std::fs::create_dir_all(&root).unwrap();
    // VS Code's user file holds `github` and others; its workspace file
    // `repo-docs` and `workspace-only`.
    Legacy::plant(&fx, &root);
    let root = project(
        &fx,
        "compat",
        "compat-a7b8",
        "[servers.repo-docs]\ntransport = \"http\"\nurl = \"https://docs.example.com/mcp\"\n\
         clients = [\"claude-code\", \"copilot\"]\n",
    );
    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");

    // VS Code runs the `.vscode/mcp.json` entry over the same-named
    // `<P>/.mcp.json` one (gate V2 VSL-3, FB-15); the Copilot CLI reads
    // only `.mcp.json`, which the warning says.
    let doc = view(&fx, &root, Some(&["copilot"]));
    let docs = row(&doc, "copilot", "repo-docs");
    assert_eq!(docs["loaded"], true);
    assert_eq!(
        docs["winner"]["layer"],
        "client-compat:<P>/.vscode/mcp.json"
    );
    assert_eq!(docs["winner"]["file"], "<P>/.vscode/mcp.json");
    assert_eq!(docs["winner"]["source"], "foreign");
    assert_eq!(
        docs["shadowed"],
        serde_json::json!([{ "layer": "project", "file": "<P>/.mcp.json" }])
    );
    assert_eq!(docs["warnings"][0]["code"], "duplicate_via_compat");
    let message = docs["warnings"][0]["message"].as_str().unwrap();
    assert!(
        message.starts_with("VS Code runs \"repo-docs\" from <P>/.vscode/mcp.json over")
            && message.contains("the Copilot CLI reads only <P>/.mcp.json"),
        "{message}"
    );
    let also: Vec<&str> = docs["also_visible_to"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(also.contains(&"omp"), "{also:?}");
    assert_eq!(
        row(&doc, "copilot", "workspace-only")["winner"]["layer"],
        "client-compat:<P>/.vscode/mcp.json"
    );
    let github = row(&doc, "copilot", "github");
    let layer = github["winner"]["layer"].as_str().unwrap();
    assert!(
        layer.starts_with("client-compat:") && layer.ends_with("Code/User/mcp.json"),
        "{layer}"
    );
    // Copilot's own user file still ranks above VS Code's user file.
    fx.write(
        &fx.th.home().join(".copilot").join("mcp-config.json"),
        r#"{ "mcpServers": { "github": { "type": "stdio", "command": "github-mcp" } } }"#,
    );
    let doc = view(&fx, &root, Some(&["copilot"]));
    let github = row(&doc, "copilot", "github");
    assert_eq!(
        github["winner"]["layer"],
        "foreign:~/.copilot/mcp-config.json"
    );
    let shadowed = github["shadowed"][0]["layer"].as_str().unwrap();
    assert!(
        shadowed.starts_with("client-compat:") && shadowed.ends_with("Code/User/mcp.json"),
        "{shadowed}"
    );
    let message = github["warnings"][0]["message"].as_str().unwrap();
    assert!(
        message.starts_with("GitHub Copilot loads \"github\" from ~/.copilot/mcp-config.json"),
        "{message}"
    );

    // Claude Code's project entry is visible to Copilot too.
    let doc = view(&fx, &root, Some(&["claude-code"]));
    let also = &row(&doc, "claude-code", "repo-docs")["also_visible_to"];
    assert!(
        also.as_array().unwrap().iter().any(|v| v == "copilot"),
        "{also}"
    );

    // Every client Loadout writes has a row; the read-only one does not.
    let doc = view(&fx, &root, None);
    let clients = doc["clients"].as_object().unwrap();
    assert!(clients.contains_key("copilot") && clients.contains_key("cursor"));
    assert!(!clients.contains_key("vscode-legacy"));
}

#[test]
fn omp_compat_reads_resolve_to_cursor_and_vscode() {
    let fx = Fx::new();
    let root = project(
        &fx,
        "omp-compat",
        "omp-compat-c9d0",
        "[servers.local]\ntransport = \"stdio\"\ncommand = \"local-mcp\"\nclients = [\"omp\"]\n",
    );
    fx.write(
        &fx.th.home().join(".cursor").join("mcp.json"),
        r#"{ "mcpServers": { "cursor-user": { "command": "cursor-user-mcp" } } }"#,
    );
    fx.write(
        &root.join(".cursor").join("mcp.json"),
        r#"{ "mcpServers": { "cursor-project": { "url": "https://cursor.example.test/mcp" }, "local": { "command": "local-mcp" } } }"#,
    );
    fx.write(
        &root.join(".vscode").join("mcp.json"),
        "{\n  // workspace\n  \"servers\": { \"vscode-project\": { \"type\": \"stdio\", \"command\": \"vscode-mcp\" } },\n}\n",
    );
    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");

    let doc = view(&fx, &root, Some(&["omp"]));
    assert_eq!(
        row(&doc, "omp", "cursor-user")["winner"]["layer"],
        "client-compat:~/.cursor/mcp.json"
    );
    assert_eq!(
        row(&doc, "omp", "cursor-project")["winner"]["layer"],
        "client-compat:<P>/.cursor/mcp.json"
    );
    assert_eq!(
        row(&doc, "omp", "vscode-project")["winner"]["layer"],
        "client-compat:<P>/.vscode/mcp.json"
    );
    let local = row(&doc, "omp", "local");
    assert_eq!(local["winner"]["layer"], "project");
    assert_eq!(local["warnings"][0]["code"], "duplicate_via_compat");

    // Cursor's own view of the same files.
    let doc = view(&fx, &root, Some(&["cursor"]));
    assert_eq!(
        row(&doc, "cursor", "cursor-project")["winner"]["layer"],
        "foreign:<P>/.cursor/mcp.json"
    );
    let also = &row(&doc, "cursor", "cursor-project")["also_visible_to"];
    assert!(
        also.as_array().unwrap().iter().any(|v| v == "omp"),
        "{also}"
    );
}

#[test]
fn cursor_runs_same_named_user_and_project_entries_side_by_side() {
    let fx = Fx::new();
    let mut github = store_stdio("github", &["cursor"], &["-y", "github-mcp"]);
    github.body.env = Some(
        [("DEBUG", "1"), ("KEEP", "yes")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect(),
    );
    fx.put_server(github);
    fx.put_server(store_stdio("noisy", &["cursor"], &["-y", "noisy-mcp"]));
    // An override with `unset` and a suppression, both for Cursor.
    let root = project(
        &fx,
        "side-by-side",
        "side-by-side-e5f6",
        "[servers.github]\nfrom = \"github\"\nunset = [\"env.DEBUG\"]\nclients = [\"cursor\"]\n\n\
         [servers.noisy]\nenabled = false\nclients = [\"cursor\"]\n",
    );
    let plan = fx.plan();
    let codes: Vec<ErrorCode> = plan
        .warnings
        .iter()
        .chain(plan.targets.iter().flat_map(|target| &target.warnings))
        .map(|warning| warning.code())
        .collect();
    // Cursor runs both entries, so `unset` is not merged away from anything.
    assert!(
        !format!("{plan:?}").contains("UnsetNotExpressible"),
        "{plan:#?}"
    );
    assert_eq!(
        codes
            .iter()
            .filter(|code| **code == ErrorCode::CannotSuppressInProject)
            .count(),
        1,
        "{plan:#?}"
    );
    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");

    // Both `github` entries load, one row per file, neither shadowing the
    // other (gate V2 CUR-5, FB-17).
    let doc = view(&fx, &root, Some(&["cursor"]));
    let rows: Vec<&Value> = doc["clients"]["cursor"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["name"] == "github")
        .collect();
    let layers: Vec<(&str, &str)> = rows
        .iter()
        .map(|entry| {
            (
                entry["winner"]["layer"].as_str().unwrap(),
                entry["winner"]["file"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        layers,
        [
            ("project", "<P>/.cursor/mcp.json"),
            ("system", "~/.cursor/mcp.json")
        ],
        "{doc:#}"
    );
    for entry in &rows {
        assert_eq!(entry["loaded"], true, "{entry:#}");
        assert_eq!(entry["shadowed"], serde_json::json!([]), "{entry:#}");
        assert_eq!(entry["status"], "synced", "{entry:#}");
        let codes = warning_codes(entry);
        assert!(codes.contains(&"duplicate_across_layers"), "{entry:#}");
        assert!(!codes.contains(&"unset_not_expressible"), "{entry:#}");
        let message = entry["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["code"] == "duplicate_across_layers")
            .unwrap()["message"]
            .as_str()
            .unwrap();
        assert_eq!(
            message,
            "Cursor runs a separate server named \"github\" from each of \
             <P>/.cursor/mcp.json and ~/.cursor/mcp.json: same-named entries of its own \
             files do not replace each other"
        );
    }
    // Only the project row needs the one-time enable in Cursor.
    assert!(warning_codes(rows[0]).contains(&"client_enable_required"));
    assert!(!warning_codes(rows[1]).contains(&"client_enable_required"));

    // The suppression cannot be expressed: `noisy` keeps loading.
    let noisy = row(&doc, "cursor", "noisy");
    assert_eq!(noisy["loaded"], true, "{noisy:#}");
    assert_eq!(
        warning_codes(noisy),
        ["cannot_suppress_in_project"],
        "{noisy:#}"
    );
}

fn warning_codes(entry: &Value) -> Vec<&str> {
    entry["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|warning| warning["code"].as_str().unwrap())
        .collect()
}

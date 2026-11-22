//! Windsurf, Devin Desktop and the ported clients (SPEC 7.3 rows
//! `windsurf`, `devin`, SPEC 7.5; M22): mapper goldens per transport, the
//! ported table golden per client (user scope only), reading each
//! client's files, native disable flags, targets and detection through a
//! fake `Env`, and through the apply engine: Devin's project and
//! project-local files, YAML clients edited by span splice, and JSON5
//! files that are written only when they are also JSONC.

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use agent_loadout_core::adapters::detect::{self, Evidence, NoRegistry};
use agent_loadout_core::adapters::mapper::{
    EntryMapper, Launch, MapError, RemoteLaunch, RenderedServer, ServerDef, StdioLaunch, WriteCx,
};
use agent_loadout_core::adapters::{
    ADAPTERS, AdapterSpec, McpSpec, Target, Tier, devin, ported, windsurf,
};
use agent_loadout_core::apply::EntryState;
use agent_loadout_core::formats::{Document, Format, NativeValue};
use agent_loadout_core::layering::{LOCAL_FILE, SHARED_FILE};
use agent_loadout_core::model::applied::Scope;
use agent_loadout_core::model::clients::ClientPrefs;
use agent_loadout_core::model::server::{ServerFields, Transport};
use agent_loadout_core::{Env, ErrorCode, Os};
use agent_loadout_testkit::TempHome;
use support::{Fx, json_entry, stdio as store_stdio};

const WINDSURF: &AdapterSpec = &windsurf::SPEC;
const DEVIN: &AdapterSpec = &devin::SPEC;

static NO_PREFS: LazyLock<ClientPrefs> = LazyLock::new(ClientPrefs::default);

/// The ported clients of SPEC 7.5, in table order.
fn ported_clients() -> [&'static AdapterSpec; 24] {
    [
        &ported::AUGMENT,
        &ported::CLINE,
        &ported::ROO_CODE,
        &ported::KILO_CODE,
        &ported::KIRO,
        &ported::JUNIE,
        &ported::DROID,
        &ported::COMMANDCODE,
        &ported::CRUSH,
        &ported::QODER,
        &ported::TRAE,
        &ported::TRAE_CN,
        &ported::CODEBUDDY,
        &ported::IFLOW,
        &ported::QWEN_CODE,
        &ported::ZENCODER,
        &ported::OPENCLAW,
        &ported::MIMO_CODE,
        &ported::WORKBUDDY,
        &ported::ZCODE,
        &ported::CONTINUE,
        &ported::GOOSE,
        &ported::HERMES,
        &ported::INTELLIJ,
    ]
}

/// The ported clients with an MCP target.
fn ported_mcp() -> Vec<&'static AdapterSpec> {
    ported_clients()
        .into_iter()
        .filter(|spec| spec.mcp.is_some())
        .collect()
}

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

/// The file the servers make in a new file, and what each reads back as;
/// checks every round trip.
fn golden(spec: &AdapterSpec, os: Os, list: &[&RenderedServer]) -> String {
    let mapper = mapper(spec);
    let mut doc = Document::parse(mcp(spec).skeleton.as_bytes(), mcp(spec).format).unwrap();
    for server in list {
        let native = write(spec, os, server).unwrap();
        assert!(doc.upsert(servers(spec), &server.name, &native).unwrap());
    }
    let reread = parse(spec, &text(&doc));
    let mut out = text(&doc);
    out.push_str("\n# read back\n");
    for server in list {
        let native = write(spec, os, server).unwrap();
        let value = reread.get(servers(spec), &server.name).unwrap().unwrap();
        assert_eq!(
            mapper.canonical(&value),
            mapper.canonical(&native),
            "{}",
            spec.id
        );
        let def = mapper.from_native(&server.name, &value).unwrap();
        assert_eq!(def.fields, fields_of(server), "{} {}", spec.id, server.name);
        assert!(def.unmapped.is_empty(), "{} {:?}", spec.id, def.unmapped);
        assert!(!def.disabled, "{}", spec.id);
        out.push_str(&format!("## {}\n{}", server.name, describe(&def)));
    }
    out
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
fn tier_d_adapters_are_registered() {
    let ids: Vec<&str> = ADAPTERS.iter().map(|spec| spec.id).collect();
    let legacy = ids.iter().position(|id| *id == "vscode-legacy").unwrap();
    let mut expected = vec!["vscode-legacy", "windsurf", "devin"];
    expected.extend(ported_clients().iter().map(|spec| spec.id));
    assert_eq!(ids[legacy..], expected[..]);
    for spec in [WINDSURF, DEVIN] {
        assert_eq!(spec.tier, Tier::D);
        assert!(!mcp(spec).read_only);
        assert_eq!(mcp(spec).format, Format::Json);
        assert_eq!(servers(spec), ["mcpServers"]);
    }
    // Ported clients: user scope only, never read-only.
    for spec in ported_mcp() {
        assert_eq!(spec.tier, Tier::D, "{}", spec.id);
        let mcp = mcp(spec);
        assert!(!mcp.read_only, "{}", spec.id);
        assert!(
            mcp.project.is_empty() && mcp.project_local.is_empty(),
            "{}",
            spec.id
        );
    }
    // IntelliJ has skills only.
    assert!(ported::INTELLIJ.mcp.is_none());
    let formats: Vec<(&str, Format)> = ported_mcp()
        .iter()
        .filter(|spec| mcp(spec).format != Format::Json)
        .map(|spec| (spec.id, mcp(spec).format))
        .collect();
    assert_eq!(
        formats,
        [
            ("codebuddy", Format::Json5),
            ("openclaw", Format::Json5),
            ("continue", Format::Yaml),
            ("goose", Format::Yaml),
            ("hermes", Format::Yaml),
        ]
    );
    assert_eq!(servers(&ported::CONTINUE), ["mcpServers", "[name]"]);
    assert_eq!(servers(&ported::ZENCODER), ["zencoder", "mcpServers"]);
    assert_eq!(servers(&ported::ZCODE), ["mcp", "servers"]);
    assert_eq!(servers(&ported::OPENCLAW), ["mcp", "servers"]);
    assert_eq!(servers(&ported::CRUSH), ["mcp"]);
    assert_eq!(servers(&ported::MIMO_CODE), ["mcp"]);
    assert_eq!(servers(&ported::GOOSE), ["extensions"]);
    assert_eq!(servers(&ported::HERMES), ["mcp_servers"]);
}

// ---------------------------------------------------------------------------
// Windsurf and Devin

fn github() -> RenderedServer {
    stdio(
        "github",
        "npx",
        &["-y", "@modelcontextprotocol/server-github"],
        &[("GITHUB_PERSONAL_ACCESS_TOKEN", "${env:GITHUB_PAT}")],
    )
}

fn tracker() -> RenderedServer {
    remote(
        "tracker",
        Transport::Http,
        "https://tracker.example.com/mcp",
        &[("Authorization", "Bearer ${env:TRACKER_TOKEN}")],
    )
}

fn events() -> RenderedServer {
    remote(
        "events",
        Transport::Sse,
        "https://events.example.com/sse",
        &[],
    )
}

#[test]
fn windsurf_goldens_and_reads() {
    let list = [&github(), &tracker(), &events()];
    insta::assert_snapshot!("windsurf_goldens", golden(WINDSURF, Os::Windows, &list));
    assert_eq!(
        golden(WINDSURF, Os::Windows, &list),
        golden(WINDSURF, Os::Linux, &list)
    );
    let tracker_entry = write(WINDSURF, Os::Linux, &tracker()).unwrap();
    assert!(tracker_entry.as_object().unwrap().contains_key("serverUrl"));
    // An entry spelled with `url` is the same server.
    let with_url = json(
        r#"{"url": "https://tracker.example.com/mcp",
            "headers": {"Authorization": "Bearer ${env:TRACKER_TOKEN}"}}"#,
    );
    assert_eq!(
        mapper(WINDSURF).canonical(&with_url),
        mapper(WINDSURF).canonical(&tracker_entry)
    );
    // `disabled: true` disables; an update keeps `disabled: false`.
    let mut off = github();
    off.enabled = false;
    let written = write(WINDSURF, Os::Linux, &off).unwrap();
    assert_eq!(
        written.as_object().unwrap().get("disabled"),
        Some(&NativeValue::Bool(true))
    );
    let existing = json(r#"{"command": "npx", "disabled": false, "disabledTools": ["x"]}"#);
    let updated = mapper(WINDSURF)
        .to_native(
            &github(),
            &WriteCx {
                os: Os::Linux,
                existing: Some(&existing),
            },
        )
        .unwrap();
    let updated = updated.as_object().unwrap();
    assert_eq!(updated.get("disabled"), Some(&NativeValue::Bool(false)));
    assert_eq!(updated.get("disabledTools"), Some(&json(r#"["x"]"#)));

    for (name, relative) in [
        ("windsurf_read_user", "windsurf/user/mcp_config.json"),
        (
            "windsurf_read_project",
            "windsurf/project/.windsurf/mcp_config.json",
        ),
    ] {
        let doc = parse(WINDSURF, &fixture(relative));
        insta::assert_snapshot!(name, read_all(WINDSURF, &doc));
    }
}

#[test]
fn devin_goldens_and_reads() {
    let list = [&github(), &tracker(), &events()];
    insta::assert_snapshot!("devin_goldens", golden(DEVIN, Os::Linux, &list));
    let tracker_entry = write(DEVIN, Os::Linux, &tracker()).unwrap();
    assert_eq!(
        tracker_entry.as_object().unwrap().get("transport"),
        Some(&NativeValue::from("http"))
    );
    // No native disable.
    let mut off = github();
    off.enabled = false;
    assert_eq!(
        write(DEVIN, Os::Linux, &off).unwrap_err().code(),
        ErrorCode::UnsupportedField
    );
    for (name, relative) in [
        ("devin_read_user", "devin/user/mcp_config.json"),
        ("devin_read_project", "devin/project/.devin/mcp_config.json"),
        (
            "devin_read_project_local",
            "devin/project/.devin/mcp_config.local.json",
        ),
    ] {
        let doc = parse(DEVIN, &fixture(relative));
        insta::assert_snapshot!(name, read_all(DEVIN, &doc));
    }
}

// ---------------------------------------------------------------------------
// Ported clients

#[test]
fn ported_table_golden_per_client() {
    let files = stdio(
        "files",
        "npx",
        &["-y", "files-mcp"],
        &[("ROOT", "/srv/work")],
    );
    let search = remote(
        "search",
        Transport::Http,
        "https://search.example.com/mcp",
        &[("Authorization", "Bearer example-token")],
    );
    for spec in ported_mcp() {
        let golden = golden(spec, Os::Linux, &[&files, &search]);
        assert_eq!(
            golden,
            self::golden(spec, Os::Windows, &[&files, &search]),
            "{}",
            spec.id
        );
        insta::assert_snapshot!(format!("ported_{}", spec.id.replace('-', "_")), golden);
    }
}

#[test]
fn ported_transports_and_disable_flags() {
    // SSE: written by every ported client but Goose.
    let sse = events();
    for spec in ported_mcp() {
        let result = write(spec, Os::Linux, &sse);
        if spec.id == "goose" {
            assert_eq!(
                result.unwrap_err(),
                MapError::UnsupportedTransport {
                    transport: "sse".to_owned()
                }
            );
        } else {
            let native = result.unwrap_or_else(|err| panic!("{}: {err}", spec.id));
            let def = mapper(spec).from_native("events", &native).unwrap();
            assert_eq!(def.fields.transport, Some(Transport::Sse), "{}", spec.id);
        }
    }

    // Native disable flags: `disabled: true` for the Cline family and
    // Kiro, `enabled: false` for Goose, OpenClaw and MiMo, ZCode's
    // `enable: false`; the rest block a disabled server.
    let mut off = stdio("off", "off-mcp", &[], &[]);
    off.enabled = false;
    let mut flags = Vec::new();
    for spec in ported_mcp() {
        let flag = match write(spec, Os::Linux, &off) {
            Ok(native) => {
                let entry = native.as_object().unwrap();
                let flag = ["disabled", "enabled", "enable"]
                    .into_iter()
                    .find(|key| entry.contains_key(key))
                    .unwrap_or_else(|| panic!("{}: no flag in {native:?}", spec.id));
                assert!(mapper(spec).from_native("off", &native).unwrap().disabled);
                format!("{}={:?}", flag, entry.get(flag).unwrap())
            }
            Err(err) => {
                assert_eq!(err.code(), ErrorCode::UnsupportedField, "{}", spec.id);
                "-".to_owned()
            }
        };
        flags.push(format!("{}: {flag}", spec.id));
    }
    insta::assert_snapshot!("ported_disable_flags", flags.join("\n"));

    // Spellings each client's own entries use read as Loadout's.
    let mapper_of = |spec: &AdapterSpec| mapper(spec);
    let cline = json(r#"{"type": "streamableHttp", "url": "https://x.example.com/mcp"}"#);
    let def = mapper_of(&ported::CLINE).from_native("x", &cline).unwrap();
    assert_eq!(def.fields.transport, Some(Transport::Http));
    let roo = json(r#"{"type": "streamable-http", "url": "https://x.example.com/mcp"}"#);
    let def = mapper_of(&ported::ROO_CODE).from_native("x", &roo).unwrap();
    assert_eq!(def.fields.transport, Some(Transport::Http));
    let goose_url = json(
        r#"{"enabled": true, "name": "x", "type": "streamable_http", "url": "https://x.example.com/mcp"}"#,
    );
    let def = mapper_of(&ported::GOOSE)
        .from_native("x", &goose_url)
        .unwrap();
    assert_eq!(def.fields.url.as_deref(), Some("https://x.example.com/mcp"));
    let builtin = json(r#"{"enabled": true, "name": "developer", "type": "builtin"}"#);
    assert_eq!(
        mapper_of(&ported::GOOSE)
            .from_native("developer", &builtin)
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedTransport
    );
}

#[test]
fn ported_clients_read_their_files() {
    for (spec, name, relative) in [
        (
            &ported::CLINE,
            "cline_read_user",
            "cline/user/cline_mcp_settings.json",
        ),
        (&ported::KIRO, "kiro_read_user", "kiro/user/mcp.json"),
        (&ported::ZCODE, "zcode_read_user", "zcode/user/config.json"),
        (&ported::CRUSH, "crush_read_user", "crush/user/crush.json"),
        (&ported::TRAE, "trae_read_user", "trae/user/mcp.json"),
        (
            &ported::TRAE,
            "trae_read_profile",
            "trae/user/profiles/a1b2/mcp.json",
        ),
        (
            &ported::ZENCODER,
            "zencoder_read_user",
            "zencoder/user/settings.json",
        ),
        (
            &ported::QWEN_CODE,
            "qwen_code_read_user",
            "qwen-code/user/settings.json",
        ),
        (
            &ported::CODEBUDDY,
            "codebuddy_read_user",
            "codebuddy/user/.mcp.json",
        ),
        (
            &ported::OPENCLAW,
            "openclaw_read_user",
            "openclaw/user/openclaw.json",
        ),
        (
            &ported::CONTINUE,
            "continue_read_user",
            "continue/user/config.yaml",
        ),
        (&ported::GOOSE, "goose_read_user", "goose/user/config.yaml"),
        (
            &ported::HERMES,
            "hermes_read_user",
            "hermes/user/config.yaml",
        ),
    ] {
        let doc = parse(spec, &fixture(relative));
        insta::assert_snapshot!(name, read_all(spec, &doc));
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

fn app_data(home: &TempHome, os: Os) -> PathBuf {
    match os {
        Os::Windows => home.appdata(),
        Os::MacOs => home.home().join("Library").join("Application Support"),
        Os::Linux => home.home().join(".config"),
    }
}

fn join(base: PathBuf, rel: &str) -> PathBuf {
    rel.split('/').fold(base, |path, part| path.join(part))
}

#[test]
fn windsurf_and_devin_targets() {
    let home = TempHome::new();
    let fs = home.fs();
    let project = home.projects().join("app");
    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env(os);
        assert_eq!(
            user_files(WINDSURF, &env, &fs),
            [join(home.home(), ".codeium/windsurf/mcp_config.json")]
        );
        let shared = WINDSURF
            .mcp_targets(&env, &fs, Scope::Project, Some(&project), &NO_PREFS)
            .unwrap();
        assert_eq!(
            files(&shared),
            [join(project.clone(), ".windsurf/mcp_config.json")]
        );
        let err = WINDSURF
            .mcp_targets(&env, &fs, Scope::ProjectLocal, Some(&project), &NO_PREFS)
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ScopeNotSupported);

        let devin_user = match os {
            Os::Windows => join(home.appdata(), "devin/mcp_config.json"),
            Os::Linux | Os::MacOs => join(home.home(), ".config/devin/mcp_config.json"),
        };
        assert_eq!(user_files(DEVIN, &env, &fs), [devin_user]);
        let shared = DEVIN
            .mcp_targets(&env, &fs, Scope::Project, Some(&project), &NO_PREFS)
            .unwrap();
        assert_eq!(
            files(&shared),
            [join(project.clone(), ".devin/mcp_config.json")]
        );
        let private = DEVIN
            .mcp_targets(&env, &fs, Scope::ProjectLocal, Some(&project), &NO_PREFS)
            .unwrap();
        assert_eq!(
            files(&private),
            [join(project.clone(), ".devin/mcp_config.local.json")]
        );
        assert_eq!(private[0].container.segments(), ["mcpServers"]);
    }
    // Devin ignores $XDG_CONFIG_HOME.
    let env = home
        .env_builder(Os::Linux)
        .var("XDG_CONFIG_HOME", home.xdg())
        .build();
    assert_eq!(
        user_files(DEVIN, &env, &fs),
        [join(home.home(), ".config/devin/mcp_config.json")]
    );
}

#[test]
fn ported_targets_are_user_scope_only() {
    let home = TempHome::new();
    let fs = home.fs();
    let project = home.projects().join("app");
    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env(os);
        let mut table = Vec::new();
        for spec in ported_mcp() {
            for scope in [Scope::Project, Scope::ProjectLocal] {
                let err = spec
                    .mcp_targets(&env, &fs, scope, Some(&project), &NO_PREFS)
                    .unwrap_err();
                assert_eq!(err.code(), ErrorCode::ScopeNotSupported, "{}", spec.id);
            }
            let targets = user_files(spec, &env, &fs);
            assert_eq!(targets.len(), 1, "{}", spec.id);
            let shown = targets[0]
                .strip_prefix(home.root())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            table.push(format!("{}: {shown}", spec.id));
        }
        insta::assert_snapshot!(
            format!("ported_user_files_{}", os.as_str()),
            table.join("\n")
        );
        assert!(
            ported::INTELLIJ
                .mcp_targets(&env, &fs, Scope::User, None, &NO_PREFS)
                .is_err()
        );
    }

    // First of several files: the first one that exists.
    let env = home.env(Os::Linux);
    put(&join(home.home(), ".codebuddy.json"), "{}");
    assert_eq!(
        user_files(&ported::CODEBUDDY, &env, &fs),
        [join(home.home(), ".codebuddy.json")]
    );
    put(&join(home.home(), ".codebuddy/mcp.json"), "{}");
    assert_eq!(
        user_files(&ported::CODEBUDDY, &env, &fs),
        [join(home.home(), ".codebuddy/mcp.json")]
    );
    put(&join(home.home(), ".iflow/mcp/config.json"), "{}");
    assert_eq!(
        user_files(&ported::IFLOW, &env, &fs),
        [join(home.home(), ".iflow/mcp/config.json")]
    );

    // Trae also writes each existing profile file.
    for os in [Os::Windows, Os::MacOs] {
        let env = home.env(os);
        let user = join(app_data(&home, os), "Trae/User");
        put(&join(user.clone(), "profiles/b2/mcp.json"), "{}");
        put(&join(user.clone(), "profiles/a1/mcp.json"), "{}");
        std::fs::create_dir_all(join(user.clone(), "profiles/empty")).unwrap();
        let targets = ported::TRAE
            .mcp_targets(&env, &fs, Scope::User, None, &NO_PREFS)
            .unwrap();
        assert_eq!(
            files(&targets),
            [
                join(user.clone(), "mcp.json"),
                join(user.clone(), "profiles/a1/mcp.json"),
                join(user.clone(), "profiles/b2/mcp.json"),
            ]
        );
        let labels: Vec<Option<&str>> = targets.iter().map(|t| t.profile.as_deref()).collect();
        assert_eq!(labels, [None, Some("profiles/a1"), Some("profiles/b2")]);
    }

    // Goose: $GOOSE_PATH_ROOT when absolute, else per OS ($XDG_CONFIG_HOME
    // counts outside Windows).
    let root = home.root().join("goose-root");
    let env = home
        .env_builder(Os::Windows)
        .var("GOOSE_PATH_ROOT", &root)
        .build();
    assert_eq!(
        user_files(&ported::GOOSE, &env, &fs),
        [join(root, "config/config.yaml")]
    );
    let env = home
        .env_builder(Os::Linux)
        .var("GOOSE_PATH_ROOT", "relative")
        .var("XDG_CONFIG_HOME", home.xdg())
        .build();
    assert_eq!(
        user_files(&ported::GOOSE, &env, &fs),
        [join(home.xdg(), "goose/config.yaml")]
    );
}

#[test]
fn tier_d_detection() {
    let home = TempHome::new();
    let fs = home.fs();
    let registry = NoRegistry;
    let mut all = vec![WINDSURF, DEVIN];
    all.extend(ported_clients());
    for os in [Os::Windows, Os::Linux, Os::MacOs] {
        let env = home.env_builder(os).var("PATH", home.bin()).build();
        for spec in &all {
            let found = detect::detect(spec, &env, &fs, &registry);
            assert!(found.supported, "{} on {os:?}", spec.id);
            assert!(!found.detected(), "{} on {os:?}", spec.id);
        }
    }
    let windsurf_dir = join(home.home(), ".codeium/windsurf");
    let goose_dir = join(home.home(), ".config/goose");
    let continue_dir = join(home.home(), ".continue");
    for dir in [&windsurf_dir, &goose_dir, &continue_dir] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let env = home.env(Os::Linux);
    let evidence = |spec: &AdapterSpec| detect::detect(spec, &env, &fs, &registry).evidence;
    assert_eq!(evidence(WINDSURF), [Evidence::Path(windsurf_dir)]);
    assert_eq!(evidence(&ported::GOOSE)[0], Evidence::Path(goose_dir));
    assert_eq!(evidence(&ported::CONTINUE), [Evidence::Path(continue_dir)]);
    put(&home.bin().join("devin.exe"), "");
    let env = home
        .env_builder(Os::Windows)
        .var("PATH", home.bin())
        .var("PATHEXT", ".EXE;.CMD")
        .build();
    assert!(
        detect::detect(DEVIN, &env, &fs, &registry)
            .evidence
            .contains(&Evidence::Executable(home.bin().join("devin.exe")))
    );
}

/// Goose's config file on the host OS.
fn goose_file(fx: &Fx) -> PathBuf {
    if support::host_os() == Os::Windows {
        join(fx.th.appdata(), "Block/goose/config/config.yaml")
    } else {
        join(fx.th.home(), ".config/goose/config.yaml")
    }
}

// ---------------------------------------------------------------------------
// Through the apply engine

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
fn devin_project_and_project_local_files() {
    let fx = Fx::new();
    let root = project(
        &fx,
        "app",
        "app-d3v1",
        "[servers.docs]\ntransport = \"stdio\"\ncommand = \"docs-mcp\"\nclients = [\"devin\", \"windsurf\"]\n",
    );
    fx.manifest(
        &root,
        LOCAL_FILE,
        "schema = 1\nid = \"app-d3v1\"\n\n[servers.notes]\ntransport = \"stdio\"\n\
         command = \"notes-mcp\"\nclients = [\"devin\"]\n",
    );
    fx.trust(&root);
    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");
    let shared = join(root.clone(), ".devin/mcp_config.json");
    let private = join(root.clone(), ".devin/mcp_config.local.json");
    assert_eq!(
        json_entry(&fx.read(&shared), "docs").unwrap()["command"],
        "docs-mcp"
    );
    assert!(json_entry(&fx.read(&shared), "notes").is_none());
    assert_eq!(
        json_entry(&fx.read(&private), "notes").unwrap()["command"],
        "notes-mcp"
    );
    let windsurf_project = join(root.clone(), ".windsurf/mcp_config.json");
    assert!(json_entry(&fx.read(&windsurf_project), "docs").is_some());
    assert!(!fx.plan().has_work(), "{:#?}", fx.plan());
}

#[test]
fn yaml_clients_are_edited_by_span_splice() {
    let fx = Fx::new();
    let goose = goose_file(&fx);
    let original = fixture("goose/user/config.yaml");
    fx.write(&goose, &original);
    let continue_file = join(fx.th.home(), ".continue/config.yaml");
    let continue_original = fixture("continue/user/config.yaml");
    fx.write(&continue_file, &continue_original);
    fx.put_server(store_stdio(
        "github",
        &["goose", "continue", "hermes"],
        &["-y", "server-github"],
    ));
    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");

    // Goose: the new extension is appended inside `extensions`; every
    // other byte is the original's.
    let after = fx.read(&goose).replace("\r\n", "\n");
    let added = "  github:\n    args:\n    - -y\n    - server-github\n    cmd: npx\n    enabled: true\n    name: github\n    type: stdio\n";
    let at = original.find("GOOSE_MODE:").unwrap();
    assert_eq!(
        after,
        format!("{}{added}{}", &original[..at], &original[at..])
    );
    // Continue: an item named `github` at the end of the list.
    let after = fx.read(&continue_file).replace("\r\n", "\n");
    assert!(
        after.ends_with("    url: https://events.example.com/sse\n  - name: github\n    command: npx\n    args:\n      - -y\n      - server-github\n"),
        "{after}\n{report:#?}"
    );
    assert!(
        after
            .starts_with(&continue_original[..continue_original.find("  - name: legacy").unwrap()])
    );
    // Hermes: a new file.
    let hermes = join(fx.th.home(), ".hermes/config.yaml");
    assert_eq!(
        fx.read(&hermes).replace("\r\n", "\n"),
        "mcp_servers:\n  github:\n    command: npx\n    args:\n      - -y\n      - server-github\n"
    );
    assert!(!fx.plan().has_work(), "{:#?}", fx.plan());

    // Removing the server takes exactly those lines out again.
    fx.remove_server("github");
    let (report, _) = fx.apply();
    assert_eq!(
        fx.read(&goose).replace("\r\n", "\n"),
        original,
        "{report:#?}"
    );
    assert_eq!(
        fx.read(&continue_file).replace("\r\n", "\n"),
        continue_original
    );
}

#[test]
fn yaml_anchors_block_the_target_as_read_only_format() {
    let fx = Fx::new();
    let hermes = join(fx.th.home(), ".hermes/config.yaml");
    let original = "base: &base\n  timeout: 120\nmcp_servers:\n  docs:\n    <<: *base\n    command: docs-mcp\n";
    fx.write(&hermes, original);
    fx.put_server(store_stdio("github", &["hermes"], &["-y", "server-github"]));
    let plan = fx.plan();
    let target = plan.targets.iter().find(|t| t.file == hermes).unwrap();
    let problem = target
        .problem
        .as_ref()
        .unwrap_or_else(|| panic!("{target:#?}"));
    assert_eq!(problem.code(), ErrorCode::ReadOnlyFormat, "{problem}");
    fx.apply();
    assert_eq!(fx.read(&hermes), original);
}

#[test]
fn continue_never_replaces_a_config_json_setup() {
    let home = TempHome::new();
    let fs = home.fs();
    let env = home.env(Os::Linux);
    let json_file = join(home.home(), ".continue/config.json");
    let yaml_file = join(home.home(), ".continue/config.yaml");
    put(&json_file, "{ \"models\": [] }\n");
    let err = ported::CONTINUE
        .mcp_targets(&env, &fs, Scope::User, None, &NO_PREFS)
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ReadOnlyFormat, "{err}");
    assert_eq!(err.path(), Some(json_file.as_path()));
    // Once Continue has moved to config.yaml, that file is the target.
    put(
        &yaml_file,
        "name: Local Assistant\nversion: 1.0.0\nschema: v1\n",
    );
    assert_eq!(user_files(&ported::CONTINUE, &env, &fs), [yaml_file]);

    // Through the engine: a warning, and neither file is touched.
    let fx = Fx::new();
    let json_file = join(fx.th.home(), ".continue/config.json");
    let yaml_file = join(fx.th.home(), ".continue/config.yaml");
    fx.write(&json_file, "{ \"models\": [] }\n");
    fx.put_server(store_stdio(
        "github",
        &["continue"],
        &["-y", "server-github"],
    ));
    let plan = fx.plan();
    assert!(
        plan.warnings
            .iter()
            .any(|warning| warning.code() == ErrorCode::ReadOnlyFormat),
        "{plan:#?}"
    );
    assert!(!plan.targets.iter().any(|target| target.file == yaml_file));
    fx.apply();
    assert!(!yaml_file.exists());
    assert_eq!(fx.read(&json_file), "{ \"models\": [] }\n");

    // Without config.json, a new config.yaml gets Continue's required keys.
    std::fs::remove_file(&json_file).unwrap();
    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");
    assert_eq!(
        fx.read(&yaml_file).replace("\r\n", "\n"),
        "name: Local Assistant\nversion: 1.0.0\nschema: v1\nmcpServers:\n  - name: github\n    command: npx\n    args:\n      - -y\n      - server-github\n"
    );
}

#[test]
fn json5_files_are_written_only_when_they_are_jsonc() {
    let fx = Fx::new();
    let openclaw = join(fx.th.home(), ".openclaw/openclaw.json");
    let openclaw_original = fixture("openclaw/user/openclaw.json");
    fx.write(&openclaw, &openclaw_original);
    let codebuddy = join(fx.th.home(), ".codebuddy/.mcp.json");
    let codebuddy_original = fixture("codebuddy/user/.mcp.json");
    fx.write(&codebuddy, &codebuddy_original);
    fx.put_server(store_stdio(
        "github",
        &["openclaw", "codebuddy"],
        &["-y", "server-github"],
    ));

    let plan = fx.plan();
    let target = plan.targets.iter().find(|t| t.file == openclaw).unwrap();
    let problem = target
        .problem
        .as_ref()
        .unwrap_or_else(|| panic!("{target:#?}"));
    assert_eq!(problem.code(), ErrorCode::ReadOnlyFormat, "{problem}");
    let entry = support::entry(&plan, &codebuddy, "github");
    assert_eq!(entry.state, EntryState::PendingCreate, "{entry:#?}");

    fx.apply();
    assert_eq!(fx.read(&openclaw).replace("\r\n", "\n"), openclaw_original);
    let written = fx.read(&codebuddy).replace("\r\n", "\n");
    assert!(written.contains("// CodeBuddy MCP servers"), "{written}");
    assert!(written.contains("\"args\": [\"-y\", \"files-mcp\"], // pinned"));
    let reread = Document::parse(written.as_bytes(), Format::Json5).unwrap();
    assert_eq!(
        reread.get(&["mcpServers"], "github").unwrap(),
        Some(json(
            r#"{"type": "stdio", "command": "npx", "args": ["-y", "server-github"]}"#
        ))
    );
}

#[test]
fn native_disable_uses_each_clients_flag() {
    let fx = Fx::new();
    fx.edit("clients", |tx| {
        tx.write(
            "clients.toml",
            "schema = 1\n[cline]\nnative_disable = true\n[goose]\nnative_disable = true\n",
        )
    });
    fx.put_server(store_stdio(
        "github",
        &["cline", "goose"],
        &["-y", "server-github"],
    ));
    let (report, commit) = fx.apply();
    assert!(commit.is_some(), "{report:#?}");
    // No longer wanted: disabled in place instead of removed.
    fx.put_server(store_stdio(
        "github",
        &["copilot"],
        &["-y", "server-github"],
    ));
    let (report, _) = fx.apply();
    let cline = join(fx.th.home(), ".cline/data/settings/cline_mcp_settings.json");
    assert_eq!(
        json_entry(&fx.read(&cline), "github").unwrap()["disabled"],
        true,
        "{report:#?}"
    );
    let goose = goose_file(&fx);
    let doc = Document::parse(fx.read(&goose).as_bytes(), Format::Yaml).unwrap();
    let entry = doc.get(&["extensions"], "github").unwrap().unwrap();
    assert_eq!(
        entry.as_object().unwrap().get("enabled"),
        Some(&NativeValue::Bool(false))
    );
}

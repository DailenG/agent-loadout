//! Command tests against a temp home (SPEC 19 item 1): `RootedFs` panics on
//! any path outside the temp root, the keyring is in memory, and no `op`
//! or registry is reachable.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_loadout_core::Os;
use agent_loadout_core::paths::loadout_home;
use agent_loadout_core::secrets::delivery::DeliveryEnv;
use agent_loadout_core::store::{LogFilter, Operation, Store};
use agent_loadout_testkit::{MemKeyring, TempHome};

use super::apply::FileRef;
use super::servers::{ClientChoice, KeyValue, PillState, ServerDraft, TransportKind};
use super::{ApplyStatusKind, Core, EntryStateKind, FileScope, apply, servers, tools};

struct Fx {
    th: TempHome,
    core: Core,
    keyring: Arc<MemKeyring>,
}

fn host_os() -> Os {
    if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    }
}

impl Fx {
    fn new() -> Self {
        let th = TempHome::new();
        let env = th.env_builder(host_os()).hostname("test-desk").build();
        let fs = th.fs();
        let home = loadout_home(&env, &fs, Some(&th.root().join("lh"))).unwrap();
        let keyring = Arc::new(MemKeyring::new());
        let launcher = th
            .bin()
            .join(format!("loadout{}", std::env::consts::EXE_SUFFIX));
        let delivery = DeliveryEnv::new(Some(launcher), keyring.clone());
        let core = Core {
            env,
            fs: Box::new(fs),
            home,
            delivery,
        };
        Self { th, core, keyring }
    }

    fn claude(&self) -> PathBuf {
        self.th.home().join(".claude.json")
    }

    fn read(&self, path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    fn store(&self) -> Store<'_> {
        self.core.open_store_for_change().unwrap()
    }

    fn commits(&self) -> usize {
        match self.core.open_store().unwrap() {
            Some(store) => store.log(&LogFilter::default()).unwrap().len(),
            None => 0,
        }
    }

    /// Commits store files without applying (a hand edit, or review mode).
    fn stage(&self, files: &[(&str, &str)]) {
        let store = self.store();
        store
            .transaction(Operation::new("test.edit", "test: edit"), |tx| {
                files
                    .iter()
                    .try_for_each(|(path, text)| tx.write(path, text.to_string()))
            })
            .unwrap();
    }
}

fn draft(id: &str, clients: &[&str]) -> ServerDraft {
    ServerDraft {
        id: id.to_owned(),
        display_name: Some("Fake server".to_owned()),
        description: None,
        transport: TransportKind::Stdio,
        command: Some("npx".to_owned()),
        args: vec!["-y".to_owned(), "fake-mcp".to_owned()],
        url: None,
        cwd: None,
        env: vec![KeyValue {
            key: "LOG_LEVEL".to_owned(),
            value: "info".to_owned(),
        }],
        headers: Vec::new(),
        clients: ClientChoice {
            all_detected: false,
            clients: clients.iter().map(|c| (*c).to_owned()).collect(),
        },
        delivery: None,
        tools_allow: Vec::new(),
        tools_deny: Vec::new(),
    }
}

fn server_file(id: &str, body: &str, clients: &[&str]) -> String {
    let list = clients
        .iter()
        .map(|c| format!("{c:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("schema = 1\nid = \"{id}\"\n{body}\n[enable]\nclients = [{list}]\n")
}

#[test]
fn adding_a_server_applies_in_one_commit_and_can_be_undone() {
    let fx = Fx::new();
    // The store exists already (it is created with its own first commit).
    drop(fx.store());
    let before = fx.commits();
    let result = servers::save(&fx.core, &draft("fake", &["claude-code"]), true).unwrap();

    assert!(result.commit.is_some());
    assert_eq!(
        fx.commits(),
        before + 1,
        "the edit and its apply are one commit"
    );
    let run = result.apply.expect("instant mode applies");
    assert!(run.recorded);
    assert_eq!(run.status, ApplyStatusKind::Done);
    assert_eq!(run.files_written, 1);
    let written = fx.read(&fx.claude());
    assert!(written.contains("\"fake\""), "{written}");

    // History lists the run for "Undo this apply", newest first.
    let runs = apply::applied_runs(&fx.core).unwrap();
    assert_eq!(runs[0].apply_id, run.apply_id);
    assert!(runs[0].latest);

    let undone = apply::undo(&fx.core, &run.apply_id, false).unwrap();
    assert_eq!(undone.apply_id, run.apply_id);
    assert!(undone.tag.contains("pre-rollback"), "{}", undone.tag);
    assert!(!fx.read(&fx.claude()).contains("\"fake\""));
    let list = servers::list(&fx.core).unwrap();
    assert!(list.servers.iter().all(|row| row.id != "fake"));
}

#[test]
fn pills_follow_the_plan() {
    let fx = Fx::new();
    servers::save(&fx.core, &draft("fake", &["claude-code", "codex"]), true).unwrap();
    let pill = |list: &servers::ServerList, client: &str| {
        list.servers[0]
            .pills
            .iter()
            .find(|pill| pill.client == client)
            .cloned()
            .unwrap()
    };

    let list = servers::list(&fx.core).unwrap();
    assert_eq!(pill(&list, "claude-code").state, PillState::Enabled);
    assert!(pill(&list, "claude-code").selected);
    assert_eq!(pill(&list, "codex").state, PillState::Enabled);
    assert_eq!(pill(&list, "cursor").state, PillState::Disabled);
    assert!(!pill(&list, "cursor").selected);

    // Someone edits the written entry: drift.
    let text = fx.read(&fx.claude()).replace("fake-mcp", "fake-mcp-edited");
    std::fs::write(fx.claude(), text).unwrap();
    let list = servers::list(&fx.core).unwrap();
    assert_eq!(pill(&list, "claude-code").state, PillState::Drift);

    // A store edit that is not applied yet: pending.
    fx.stage(&[(
        "servers/later.toml",
        &server_file(
            "later",
            "transport = \"stdio\"\ncommand = \"fake-later\"",
            &["codex"],
        ),
    )]);
    let list = servers::list(&fx.core).unwrap();
    let later = list.servers.iter().find(|row| row.id == "later").unwrap();
    let codex = later.pills.iter().find(|p| p.client == "codex").unwrap();
    assert_eq!(codex.state, PillState::Pending);
}

#[test]
fn a_missing_secret_blocks_the_pill_with_its_reason() {
    let fx = Fx::new();
    let mut draft = draft("needs-secret", &["claude-code"]);
    draft.env.push(KeyValue {
        key: "API_TOKEN".to_owned(),
        value: "${secret:FAKE_TOKEN}".to_owned(),
    });
    let result = servers::save(&fx.core, &draft, true).unwrap();
    assert!(result.commit.is_some(), "the store edit commits either way");

    let list = servers::list(&fx.core).unwrap();
    let row = &list.servers[0];
    let pill = row
        .pills
        .iter()
        .find(|p| p.client == "claude-code")
        .unwrap();
    assert_eq!(pill.state, PillState::Blocked);
    let reason = pill.reason.as_ref().expect("a reason");
    assert!(!row.problems.is_empty());
    assert_eq!(row.problems[0].code, reason.code);
}

#[test]
fn preview_renders_each_client_and_masks_inline_values() {
    const VALUE: &str = "fake-inline-value-0001";
    let fx = Fx::new();
    fx.keyring.insert("loadout", "FAKE_TOKEN", VALUE);
    fx.stage(&[(
        "secrets.toml",
        "schema = 1\n\n[secrets.FAKE_TOKEN]\nref = \"keyring://loadout/FAKE_TOKEN\"\n",
    )]);
    let mut draft = draft("inline", &["claude-code", "codex"]);
    draft.env.push(KeyValue {
        key: "API_TOKEN".to_owned(),
        value: "${secret:FAKE_TOKEN}".to_owned(),
    });
    draft.delivery = Some(servers::DeliveryKind::Inline);

    let previews = servers::preview(&fx.core, &draft).unwrap();
    assert_eq!(previews.len(), 2);
    let claude = previews.iter().find(|p| p.client == "claude-code").unwrap();
    let text = claude.text.as_deref().expect("rendered");
    assert!(text.contains("mcpServers"), "{text}");
    assert!(text.contains("********"), "{text}");
    let codex = previews.iter().find(|p| p.client == "codex").unwrap();
    assert_eq!(codex.format, "toml");
    let json = serde_json::to_string(&previews).unwrap();
    assert!(!json.contains(VALUE), "no secret value crosses IPC: {json}");
}

#[test]
fn paste_anything_detects_the_client_shape() {
    let fx = Fx::new();

    let claude = servers::convert(
        &fx.core,
        r#"{ "mcpServers": { "Fake Search": { "command": "npx", "args": ["-y", "fake-search"], "env": { "LOG_LEVEL": "debug" } } } }"#,
    )
    .unwrap();
    assert_eq!(claude.len(), 1);
    assert_eq!(claude[0].source_client, "claude-code");
    let found = claude[0].draft.as_ref().unwrap();
    assert_eq!(found.id, "fake-search");
    assert_eq!(found.command.as_deref(), Some("npx"));
    assert_eq!(found.args, ["-y", "fake-search"]);
    assert_eq!(found.env[0].value, "debug");

    let codex = servers::convert(
        &fx.core,
        "[mcp_servers.fake]\ncommand = \"fake-mcp\"\nargs = [\"--stdio\"]\n",
    )
    .unwrap();
    assert_eq!(codex[0].source_client, "codex");
    assert_eq!(
        codex[0].draft.as_ref().unwrap().command.as_deref(),
        Some("fake-mcp")
    );

    let bare = servers::convert(&fx.core, r#"{ "url": "https://fake.example.test/mcp" }"#).unwrap();
    assert_eq!(bare[0].name, "server");
    assert_eq!(
        bare[0].draft.as_ref().unwrap().url.as_deref(),
        Some("https://fake.example.test/mcp")
    );

    // A literal under a sensitive name is not taken as it is.
    let literal = "not-a-real-credential-0001";
    let secret = servers::convert(
        &fx.core,
        &format!(r#"{{ "mcpServers": {{ "fake": {{ "command": "fake", "env": {{ "API_TOKEN": "{literal}" }} }} }} }}"#),
    )
    .unwrap();
    assert!(secret[0].draft.is_none());
    assert_eq!(
        secret[0].problem.as_ref().unwrap().code,
        agent_loadout_core::ErrorCode::NeedsSecretExtraction
    );
    assert!(!serde_json::to_string(&secret).unwrap().contains(literal));

    // A flagged argument is named by its position, never by its value.
    let argument = servers::convert(
        &fx.core,
        &format!(r#"{{ "mcpServers": {{ "fake": {{ "command": "fake", "args": ["--stdio", "--api-key={literal}"] }} }} }}"#),
    )
    .unwrap();
    assert!(argument[0].draft.is_none());
    let problem = argument[0].problem.as_ref().unwrap();
    assert_eq!(
        problem.code,
        agent_loadout_core::ErrorCode::NeedsSecretExtraction
    );
    assert!(
        problem.message.contains("argument 2"),
        "{}",
        problem.message
    );
    assert!(!serde_json::to_string(&argument).unwrap().contains(literal));

    let err = servers::convert(&fx.core, "{ not: [config").unwrap_err();
    assert_eq!(err.code, agent_loadout_core::ErrorCode::ParseError);
    // Plain text reads as a YAML scalar: parsed, but no entry in it.
    let err = servers::convert(&fx.core, "this is not config").unwrap_err();
    assert_eq!(err.code, agent_loadout_core::ErrorCode::UnexpectedShape);
}

#[test]
fn edits_keep_comments_and_the_toml_tab_checks_the_id() {
    let fx = Fx::new();
    servers::save(&fx.core, &draft("fake", &["claude-code"]), true).unwrap();
    let detail = servers::get(&fx.core, "fake").unwrap();
    let commented = detail.toml.replace("[enable]", "# kept comment\n[enable]");
    servers::save_toml(&fx.core, "fake", &commented).unwrap();

    // Tab switches: Form edits go into the TOML text, comment kept; TOML
    // edits come back as the Form's draft; a broken text blocks the switch.
    let detail = servers::get(&fx.core, "fake").unwrap();
    let mut form = detail.draft.unwrap();
    form.description = Some("From the form".to_owned());
    let synced = servers::draft_to_toml(&form, &detail.toml).unwrap();
    assert!(synced.contains("# kept comment"), "{synced}");
    assert!(synced.contains("From the form"), "{synced}");
    let back =
        servers::toml_to_draft("fake", &synced.replace("From the form", "From the TOML")).unwrap();
    assert_eq!(back.description.as_deref(), Some("From the TOML"));
    assert_eq!(
        servers::toml_to_draft("fake", "not = [toml")
            .unwrap_err()
            .code,
        agent_loadout_core::ErrorCode::ParseError
    );

    let mut edited = servers::get(&fx.core, "fake").unwrap().draft.unwrap();
    edited.description = Some("Edited in the form".to_owned());
    servers::save(&fx.core, &edited, false).unwrap();
    let toml = servers::get(&fx.core, "fake").unwrap().toml;
    assert!(toml.contains("# kept comment"), "{toml}");
    assert!(toml.contains("Edited in the form"), "{toml}");

    let err = servers::save_toml(
        &fx.core,
        "fake",
        &toml.replace("id = \"fake\"", "id = \"other\""),
    )
    .unwrap_err();
    assert_eq!(err.code, agent_loadout_core::ErrorCode::IdMismatch);
    let err = servers::save_toml(&fx.core, "fake", "not = [toml").unwrap_err();
    assert_eq!(err.code, agent_loadout_core::ErrorCode::ParseError);

    // A hand-broken file (the store refuses to commit one, so it is edited
    // in the working tree): the TOML tab still gets its text and fixes it.
    std::fs::write(
        fx.core.home.store_dir().join("servers").join("fake.toml"),
        "schema = 1\nid = \"fake\"\ntransport = 7\n",
    )
    .unwrap();
    let broken = servers::get(&fx.core, "fake").unwrap();
    assert!(broken.draft.is_none());
    assert!(broken.invalid.is_some());
    assert!(broken.toml.contains("transport = 7"));
    servers::save_toml(&fx.core, "fake", &toml).unwrap();
    assert!(servers::get(&fx.core, "fake").unwrap().draft.is_some());
}

#[test]
fn client_toggles_and_removal_apply_instantly() {
    let fx = Fx::new();
    servers::save(&fx.core, &draft("fake", &["claude-code"]), true).unwrap();
    let off = ClientChoice {
        all_detected: false,
        clients: Vec::new(),
    };
    let before = fx.commits();
    let result = servers::set_clients_for(&fx.core, "fake", &off).unwrap();
    assert_eq!(fx.commits(), before + 1);
    assert_eq!(result.apply.unwrap().files_written, 1);
    assert!(!fx.read(&fx.claude()).contains("\"fake\""));

    servers::set_clients_for(
        &fx.core,
        "fake",
        &ClientChoice {
            all_detected: false,
            clients: vec!["claude-code".to_owned()],
        },
    )
    .unwrap();
    assert!(fx.read(&fx.claude()).contains("\"fake\""));
    servers::remove(&fx.core, &["fake".to_owned()]).unwrap();
    assert!(!fx.read(&fx.claude()).contains("\"fake\""));
    assert!(servers::list(&fx.core).unwrap().servers.is_empty());
}

#[test]
fn the_drawer_shows_pending_files_and_applies_them() {
    let fx = Fx::new();
    fx.stage(&[(
        "servers/fake.toml",
        &server_file(
            "fake",
            "transport = \"stdio\"\ncommand = \"fake-mcp\"",
            &["claude-code"],
        ),
    )]);
    let pending = apply::pending(&fx.core).unwrap();
    assert_eq!(pending.writes, 1);
    assert!(pending.pending >= 1);
    let file = &pending.files[0];
    assert_eq!(file.clients, ["claude-code"]);
    assert_eq!(file.counts.create, 1);
    assert!(file.diff.contains("fake-mcp"), "{}", file.diff);
    // No profile path in the row or the diff headers.
    assert_eq!(file.file, "~/.claude.json");
    let root = fx.th.root().to_string_lossy().into_owned();
    assert!(!file.diff.contains(&root), "{}", file.diff);
    assert!(file.diff.contains("b/~/.claude.json"), "{}", file.diff);
    assert_eq!(file.entries[0].note.state, EntryStateKind::PendingCreate);

    let only_claude = FileRef {
        project_id: None,
        scope: FileScope::User,
        file: "~/.claude.json".to_owned(),
    };
    let result = apply::run(&fx.core, Some(vec![only_claude])).unwrap();
    assert_eq!(result.apply.unwrap().files_written, 1);
    let pending = apply::pending(&fx.core).unwrap();
    assert_eq!(pending.pending, 0);
    assert!(pending.files.is_empty());
}

#[test]
fn tools_list_user_files_from_the_adapter_table() {
    let fx = Fx::new();
    let tools = tools::list(&fx.core).unwrap();
    let claude = tools.iter().find(|tool| tool.id == "claude-code").unwrap();
    assert!(claude.supported);
    assert_eq!(claude.user_files[0].path, "~/.claude.json");
    assert!(!claude.user_files[0].exists);
    assert!(
        claude
            .project_files
            .iter()
            .any(|file| file == "<P>/.mcp.json")
    );
    assert!(claude.transports.stdio);
    // Read-only clients are listed for the Tools page, never offered for
    // enabling on the MCP page.
    assert!(tools.iter().any(|tool| tool.read_only));
    let clients = servers::client_infos(&fx.core);
    assert!(clients.iter().all(|client| client.id != "vscode-legacy"));

    let folder = tools::folder_of(&fx.core, "claude-code").unwrap();
    assert_eq!(folder, fx.th.home());
    assert_eq!(
        tools::folder_of(&fx.core, "nope").unwrap_err().code(),
        agent_loadout_core::ErrorCode::UnknownClient
    );
}

/// `[omp] profiles` in `clients.toml`.
#[test]
fn open_folder_follows_the_listed_profile() {
    let fx = Fx::new();
    fx.stage(&[(
        "clients.toml",
        "schema = 1\n\n[omp]\nprofiles = [\"work\"]\n",
    )]);
    let profile = fx
        .th
        .home()
        .join(".omp")
        .join("profiles")
        .join("work")
        .join("agent");
    std::fs::create_dir_all(&profile).unwrap();

    let tools = tools::list(&fx.core).unwrap();
    let omp = tools.iter().find(|tool| tool.id == "omp").unwrap();
    assert_eq!(
        omp.user_files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["~/.omp/profiles/work/agent/mcp.json"]
    );
    assert_eq!(tools::folder_of(&fx.core, "omp").unwrap(), profile);
}

/// The pills, the Preview tab and a pill toggle use the core's rule: a
/// client forced on in `clients.toml` counts although it is not detected,
/// and a client whose transports do not fit never counts.
#[test]
fn pills_and_preview_follow_the_core_selection() {
    let fx = Fx::new();
    // Claude Code is detected through its user file; Cursor is not
    // installed but forced on; Codex is forced on but has no SSE.
    std::fs::write(fx.claude(), "{}\n").unwrap();
    fx.stage(&[
        (
            "clients.toml",
            "schema = 1\n\n[cursor]\nenabled = true\n\n[codex]\nenabled = true\n",
        ),
        (
            "servers/remote.toml",
            "schema = 1\nid = \"remote\"\ntransport = \"sse\"\nurl = \"https://fake.example.test/sse\"\n\n[enable]\nclients = \"all-detected\"\n",
        ),
    ]);
    apply::run(&fx.core, None).unwrap();
    let cursor_file = fx.th.home().join(".cursor").join("mcp.json");
    assert!(fx.read(&cursor_file).contains("remote"));

    let list = servers::list(&fx.core).unwrap();
    let row = &list.servers[0];
    let pill = |client: &str| row.pills.iter().find(|p| p.client == client).unwrap();
    assert!(pill("claude-code").selected);
    assert_eq!(pill("claude-code").state, PillState::Enabled);
    assert!(pill("cursor").selected, "forced on in clients.toml");
    assert_eq!(pill("cursor").state, PillState::Enabled);
    assert!(!pill("codex").selected, "codex has no SSE");
    assert_eq!(pill("codex").state, PillState::Disabled);

    let draft = servers::get(&fx.core, "remote").unwrap().draft.unwrap();
    let previewed: Vec<String> = servers::preview(&fx.core, &draft)
        .unwrap()
        .into_iter()
        .map(|preview| preview.client)
        .collect();
    assert!(previewed.contains(&"cursor".to_owned()), "{previewed:?}");
    assert!(
        previewed.contains(&"claude-code".to_owned()),
        "{previewed:?}"
    );
    assert!(!previewed.contains(&"codex".to_owned()), "{previewed:?}");

    // Turning Claude Code off (the page's toggle: the selected pills
    // without it) keeps the forced-on client's entry.
    let rest: Vec<String> = row
        .pills
        .iter()
        .filter(|p| p.selected && p.client != "claude-code")
        .map(|p| p.client.clone())
        .collect();
    servers::set_clients_for(
        &fx.core,
        "remote",
        &ClientChoice {
            all_detected: false,
            clients: rest,
        },
    )
    .unwrap();
    assert!(!fx.read(&fx.claude()).contains("remote"));
    assert!(fx.read(&cursor_file).contains("remote"));
}

/// Preview of a stored server keeps what the Form does not show.
#[test]
fn preview_of_a_stored_server_keeps_its_other_fields() {
    let fx = Fx::new();
    let os = host_os().as_str();
    fx.stage(&[(
        "servers/native.toml",
        &format!(
            "schema = 1\nid = \"native\"\ntransport = \"stdio\"\ncommand = \"fake-mcp\"\n\n[enable]\nclients = [\"claude-code\"]\n\n[os.{os}]\ncommand = \"fake-mcp-for-this-os\"\n\n[client.claude-code]\nnative_name = \"FakeNative\"\n"
        ),
    )]);
    let draft = servers::get(&fx.core, "native").unwrap().draft.unwrap();
    let previews = servers::preview(&fx.core, &draft).unwrap();
    assert_eq!(previews.len(), 1);
    assert_eq!(previews[0].name, "FakeNative");
    let text = previews[0].text.as_deref().expect("rendered");
    assert!(text.contains("\"FakeNative\""), "{text}");
    assert!(text.contains("fake-mcp-for-this-os"), "{text}");
}

fn project(fx: &Fx, name: &str, id: &str) -> PathBuf {
    let root = fx.th.projects().join(name);
    let manifest = root.join(".loadout");
    std::fs::create_dir_all(&manifest).unwrap();
    std::fs::write(
        manifest.join("project.toml"),
        format!(
            "schema = 1\nid = \"{id}\"\nname = \"{name}\"\n\n[defaults]\nclients = [\"claude-code\"]\n\n[servers.docs]\ntransport = \"stdio\"\ncommand = \"docs-mcp\"\n"
        ),
    )
    .unwrap();
    let project = agent_loadout_core::layering::Project::load(fx.core.fs.as_ref(), &root)
        .unwrap()
        .unwrap();
    let state = fx.core.home.state_dir();
    let mut trust =
        agent_loadout_core::layering::TrustStore::load(fx.core.fs.as_ref(), &state).unwrap();
    trust.trust(&project);
    trust.save(fx.core.fs.as_ref(), &state).unwrap();
    root
}

/// "Apply selected" writes only the selected file, even when other files
/// look the same (`<P>/.mcp.json` of two projects) or share its client.
#[test]
fn apply_selected_writes_only_the_selected_files() {
    let fx = Fx::new();
    drop(fx.store());
    let alpha = project(&fx, "alpha", "alpha-a1b2");
    let beta = project(&fx, "beta", "beta-c3d4");
    let machine =
        agent_loadout_core::store::Machine::load(fx.core.fs.as_ref(), &fx.core.home.state_dir())
            .unwrap()
            .unwrap()
            .id;
    let mut overlay = format!(
        "schema = 1\nmachine_id = \"{machine}\"\nlabel = \"test-desk\"\nos = \"{}\"\n",
        host_os().as_str()
    );
    for (id, root) in [("alpha-a1b2", &alpha), ("beta-c3d4", &beta)] {
        // A TOML basic string: Debug doubles the backslashes.
        let path = format!("{:?}", root.to_string_lossy());
        overlay.push_str(&format!("\n[projects.{id}]\npath = {path}\n"));
    }
    fx.stage(&[
        (&format!("machines/{machine}.toml"), &overlay),
        (
            "servers/fake.toml",
            &server_file(
                "fake",
                "transport = \"stdio\"\ncommand = \"fake-mcp\"",
                &["claude-code"],
            ),
        ),
    ]);

    let pending = apply::pending(&fx.core).unwrap();
    let shared: Vec<_> = pending
        .files
        .iter()
        .filter(|file| file.file == "<P>/.mcp.json")
        .collect();
    assert_eq!(shared.len(), 2, "{:#?}", pending.files);
    assert_ne!(shared[0].project_id, shared[1].project_id);
    assert_ne!(shared[0].project, shared[1].project);
    assert!(
        pending
            .files
            .iter()
            .any(|file| file.file == "~/.claude.json")
    );

    let result = apply::run(
        &fx.core,
        Some(vec![FileRef {
            project_id: Some("alpha-a1b2".to_owned()),
            scope: FileScope::Project,
            file: "<P>/.mcp.json".to_owned(),
        }]),
    )
    .unwrap();
    assert_eq!(result.apply.unwrap().files_written, 1);
    assert!(fx.read(&alpha.join(".mcp.json")).contains("docs-mcp"));
    assert!(!beta.join(".mcp.json").exists());
    assert!(!fx.read(&fx.claude()).contains("fake-mcp"));
    let pending = apply::pending(&fx.core).unwrap();
    assert_eq!(pending.files.len(), 2, "{:#?}", pending.files);
}

/// Diffs cross IPC: secrets of entries Loadout does not own are masked in
/// the context lines too.
#[test]
fn drawer_diffs_mask_secrets_of_other_entries() {
    let fx = Fx::new();
    // Assembled at run time so no token-shaped literal is in the source.
    let token = format!("{}_{}", "ghp", "a1B2c3D4".repeat(5));
    let password = "not-a-real-password-0001";
    std::fs::write(
        fx.claude(),
        format!(
            "{{\n  \"mcpServers\": {{\n    \"other\": {{\n      \"command\": \"other-mcp\",\n      \"args\": [\"--auth-token={password}\"],\n      \"env\": {{\n        \"DB_PASSWORD\": \"{password}\",\n        \"GITHUB_TOKEN\": \"{token}\"\n      }}\n    }}\n  }}\n}}\n"
        ),
    )
    .unwrap();
    fx.stage(&[(
        "servers/fake.toml",
        &server_file(
            "fake",
            "transport = \"stdio\"\ncommand = \"fake-mcp\"",
            &["claude-code"],
        ),
    )]);
    let pending = apply::pending(&fx.core).unwrap();
    let diff = &pending.files[0].diff;
    assert!(
        diff.contains("GITHUB_TOKEN"),
        "the line is in the context: {diff}"
    );
    assert!(diff.contains("fake-mcp"), "{diff}");
    let json = serde_json::to_string(&pending).unwrap();
    assert!(!json.contains(&token), "{diff}");
    assert!(!json.contains(password), "{diff}");

    // References and plain values stay readable.
    let masked = apply::mask_diff(
        "+  \"API_TOKEN\": \"${API_TOKEN}\",\n+  \"Authorization\": \"Bearer ${env:TOKEN}\",\n+  password = \"hunter2-not-real\"\n+  \"requiresAuth\": true,\n+  - \"--api-key=plain-not-real\"\n+  \"LOG_LEVEL\": \"info\"\n",
    );
    assert!(masked.contains("\"${API_TOKEN}\""), "{masked}");
    assert!(masked.contains("Bearer ${env:TOKEN}"), "{masked}");
    assert!(masked.contains("password = \"********\""), "{masked}");
    assert!(masked.contains("\"requiresAuth\": true"), "{masked}");
    assert!(masked.contains("\"--api-key=********\""), "{masked}");
    assert!(masked.contains("\"LOG_LEVEL\": \"info\""), "{masked}");
}

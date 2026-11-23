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
use super::effective::LayerKind;
use super::projects::{ManifestChangeKind, ManifestLayout, ProjectTrust};
use super::resolve::{EntryRef, ResolveAction};
use super::servers::{ClientChoice, KeyValue, PillState, ServerDraft, TransportKind};
use super::{
    ApplyStatusKind, Core, EntryStateKind, FileScope, apply, effective, projects, resolve, servers,
    tools,
};

pub(super) struct Fx {
    pub(super) th: TempHome,
    pub(super) core: Core,
    pub(super) keyring: Arc<MemKeyring>,
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
    pub(super) fn new() -> Self {
        Self::on_host("test-desk")
    }

    /// A machine whose host name (its default label) is `host`.
    pub(super) fn on_host(host: &str) -> Self {
        Self::build(host, None)
    }

    /// [`Fx::on_host`] with the system git on `PATH`, isolated from the
    /// user's git configuration (testkit `git_env`).
    pub(super) fn with_git(host: &str, git: &Path) -> Self {
        Self::build(host, Some(git))
    }

    fn build(host: &str, git: Option<&Path>) -> Self {
        let th = TempHome::new();
        let mut builder = th.env_builder(host_os()).hostname(host);
        if let Some(git) = git {
            let git = Self::link_git(&th, git);
            builder = agent_loadout_testkit::git_env::with_git_vars(&th, &git, builder);
        }
        let env = builder.build();
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

    /// `git` reached through a link inside the temp root (a junction on
    /// Windows), so `RootedFs` lets the app find it on `PATH`; git itself runs
    /// from its real install through the link.
    fn link_git(th: &TempHome, git: &Path) -> PathBuf {
        let install = git
            .parent()
            .and_then(Path::parent)
            .expect("git's install folder");
        let link = th.root().join("git-install");
        if !link.exists() {
            #[cfg(windows)]
            {
                let status = std::process::Command::new("cmd")
                    .args(["/C", "mklink", "/J"])
                    .arg(&link)
                    .arg(install)
                    .stdout(std::process::Stdio::null())
                    .status()
                    .expect("run mklink");
                assert!(status.success(), "mklink /J failed");
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(install, &link).expect("link git");
        }
        link.join(git.strip_prefix(install).expect("inside the install"))
    }

    pub(super) fn claude(&self) -> PathBuf {
        self.th.home().join(".claude.json")
    }

    pub(super) fn read(&self, path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    pub(super) fn store(&self) -> Store<'_> {
        self.core.open_store_for_change().unwrap()
    }

    pub(super) fn commits(&self) -> usize {
        match self.core.open_store().unwrap() {
            Some(store) => store.log(&LogFilter::default()).unwrap().len(),
            None => 0,
        }
    }

    /// Commits store files without applying (a hand edit, or review mode).
    pub(super) fn stage(&self, files: &[(&str, &str)]) {
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

pub(super) fn draft(id: &str, clients: &[&str]) -> ServerDraft {
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

/// The Tools page links omp's homepage; `open_tool_homepage` opens only
/// table URLs, so an unknown id or a URL passed as an id is refused.
#[test]
fn tool_homepages_come_from_the_fixed_table() {
    let fx = Fx::new();
    let tools = tools::list(&fx.core).unwrap();
    let omp = tools.iter().find(|tool| tool.id == "omp").unwrap();
    assert_eq!(
        omp.homepage.as_deref(),
        Some("https://github.com/can1357/oh-my-pi")
    );
    assert!(
        tools
            .iter()
            .filter(|tool| tool.id != "omp")
            .all(|tool| tool.homepage.is_none())
    );
    assert_eq!(
        tools::homepage_of("omp").unwrap(),
        "https://github.com/can1357/oh-my-pi"
    );
    for refused in ["nope", "claude-code", "https://example.com", ""] {
        assert_eq!(
            tools::homepage_of(refused).unwrap_err().code(),
            agent_loadout_core::ErrorCode::UnknownClient
        );
    }
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

// M16: Projects page, effective matrix, drift resolver.

impl Fx {
    pub(super) fn summaries(&self) -> Vec<String> {
        match self.core.open_store().unwrap() {
            Some(store) => store
                .log(&LogFilter::default())
                .unwrap()
                .into_iter()
                .map(|entry| entry.summary)
                .collect(),
            None => Vec::new(),
        }
    }

    pub(super) fn store_file(&self, relative: &str) -> String {
        use agent_loadout_core::store::StoreRead;
        self.core
            .open_store()
            .unwrap()
            .unwrap()
            .read_file(relative)
            .unwrap()
            .unwrap_or_default()
    }

    /// What crosses IPC must never name the temp root (it stands in for
    /// the user's profile path).
    pub(super) fn assert_no_root(&self, json: &str) {
        let root = self.th.root().to_string_lossy().into_owned();
        for form in [
            root.clone(),
            root.replace('\\', "/"),
            root.replace('\\', "\\\\"),
        ] {
            assert!(!json.contains(&form), "{json}");
        }
    }
}

const APP_MANIFEST: &str = "schema = 1\nid = \"app-a1b2\"\nname = \"app\"\n\n[defaults]\n\
                            clients = [\"claude-code\"]\n\n[servers.github]\nfrom = \"github\"\n\
                            args = [\"--project\"]\n\n[servers.docs]\ntransport = \"stdio\"\n\
                            command = \"docs-mcp\"\n";

/// A folder with a shared manifest that this machine does not know yet.
pub(super) fn unbound_project(fx: &Fx) -> PathBuf {
    let root = fx.th.projects().join("app");
    std::fs::create_dir_all(root.join(".loadout")).unwrap();
    std::fs::write(root.join(".loadout/project.toml"), APP_MANIFEST).unwrap();
    root
}

#[test]
fn init_is_private_by_default_trusted_and_one_commit() {
    let fx = Fx::new();
    drop(fx.store());
    let root = fx.th.projects().join("fresh");
    std::fs::create_dir_all(&root).unwrap();
    let picked = projects::inspect(&fx.core, &root, "folder-1".to_owned()).unwrap();
    assert!(
        picked.manifest.is_none(),
        "no manifest: the page offers Init"
    );
    assert_eq!(picked.name, "fresh");
    // The folder is outside every placeholder root.
    let external = if cfg!(windows) {
        "%EXTERNAL%"
    } else {
        "$EXTERNAL"
    };
    assert_eq!(picked.path, format!("{external}/fresh"));

    let before = fx.commits();
    let done = projects::init(&fx.core, &root, false, None).unwrap();
    assert_eq!(fx.commits(), before + 1, "init is one commit");
    assert!(root.join(".loadout/project.local.toml").exists());
    assert!(!root.join(".loadout/project.toml").exists());
    assert_eq!(done.trust, ProjectTrust::Trusted);
    assert!(done.id.starts_with("fresh-"), "{}", done.id);
    assert!(fx.summaries()[0].starts_with("project: init fresh-"));

    let rows = projects::list(&fx.core).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].layout, Some(ManifestLayout::Private));
    assert_eq!(rows[0].trust, ProjectTrust::Trusted);
    let json = serde_json::to_string(&rows).unwrap();
    fx.assert_no_root(&json);

    // A second init of the same folder is refused, without the path.
    let err = projects::init(&fx.core, &root, true, None).unwrap_err();
    assert_eq!(err.code, agent_loadout_core::ErrorCode::AlreadyExists);
    fx.assert_no_root(&err.message);
}

#[test]
fn picked_folders_are_validated() {
    let fx = Fx::new();
    let home = fx.th.home();
    let code = |path: &Path| {
        projects::inspect(&fx.core, path, "t".to_owned())
            .unwrap_err()
            .code
    };
    use agent_loadout_core::ErrorCode;
    assert_eq!(code(&home), ErrorCode::InvalidPath, "the home folder");
    assert_eq!(code(Path::new("relative/app")), ErrorCode::InvalidPath);
    assert_eq!(
        code(&home.join("a").join("..").join("b")),
        ErrorCode::InvalidPath
    );
    let data = fx.core.home.path.join("store");
    std::fs::create_dir_all(&data).unwrap();
    assert_eq!(code(&data), ErrorCode::InvalidPath, "the data directory");
    assert_eq!(code(&fx.th.projects().join("gone")), ErrorCode::NotFound);
    let err = projects::inspect(&fx.core, &home, "t".to_owned()).unwrap_err();
    fx.assert_no_root(&err.message);
}

/// Add folder binds an existing manifest; trust shows the content (then
/// the diff after a change) and trusts only what was reviewed.
#[test]
fn add_then_trust_review_shows_the_manifest_and_its_diff() {
    let fx = Fx::new();
    fx.stage(&[(
        "servers/github.toml",
        &server_file(
            "github",
            "transport = \"stdio\"\ncommand = \"github-mcp\"",
            &["claude-code"],
        ),
    )]);
    let root = unbound_project(&fx);
    let picked = projects::inspect(&fx.core, &root, "folder-1".to_owned()).unwrap();
    let found = picked.manifest.expect("the manifest is found");
    assert_eq!(found.id, "app-a1b2");
    assert_eq!(found.layout, ManifestLayout::Shared);
    assert!(!found.known);

    let added = projects::add(&fx.core, &root).unwrap();
    assert_eq!(added.trust, ProjectTrust::Untrusted);
    assert!(added.change.apply.is_none(), "untrusted: nothing applied");
    assert!(!root.join(".mcp.json").exists());

    let review = projects::review(&fx.core, "app-a1b2").unwrap();
    assert_eq!(review.files.len(), 1);
    assert_eq!(review.files[0].change, ManifestChangeKind::New);
    assert_eq!(review.files[0].path, "<P>/.loadout/project.toml");
    assert!(
        review.files[0]
            .content
            .as_deref()
            .unwrap()
            .contains("docs-mcp")
    );

    // The manifest changes after the review: the reviewed hash no longer
    // trusts it.
    let changed = APP_MANIFEST.replace("docs-mcp", "docs-mcp-v2");
    std::fs::write(root.join(".loadout/project.toml"), &changed).unwrap();
    let err = projects::trust(&fx.core, "app-a1b2", &review.hash).unwrap_err();
    assert_eq!(err.code, agent_loadout_core::ErrorCode::TrustRequired);
    assert!(!root.join(".mcp.json").exists());

    let review = projects::review(&fx.core, "app-a1b2").unwrap();
    let before = fx.commits();
    let trusted = projects::trust(&fx.core, "app-a1b2", &review.hash).unwrap();
    assert_eq!(
        fx.commits(),
        before + 1,
        "trust and its apply are one commit"
    );
    assert_eq!(trusted.trust, ProjectTrust::Trusted);
    assert!(fx.read(&root.join(".mcp.json")).contains("docs-mcp-v2"));

    // A pull changes the manifest: the review shows a diff, under `<P>`.
    std::fs::write(
        root.join(".loadout/project.toml"),
        changed.replace("docs-mcp-v2", "docs-mcp-v3"),
    )
    .unwrap();
    let rows = projects::list(&fx.core).unwrap();
    assert_eq!(rows[0].trust, ProjectTrust::Changed);
    let review = projects::review(&fx.core, "app-a1b2").unwrap();
    assert_eq!(review.files[0].change, ManifestChangeKind::Changed);
    let diff = review.files[0].diff.as_deref().unwrap();
    assert!(diff.contains("-command = \"docs-mcp-v2\""), "{diff}");
    assert!(diff.contains("+command = \"docs-mcp-v3\""), "{diff}");
    assert!(diff.contains("<P>/.loadout/project.toml") || diff.contains("<P>\\.loadout"));
    fx.assert_no_root(&serde_json::to_string(&review).unwrap());

    // Remove from list: one commit, the owned project entries go.
    let removed = projects::remove(&fx.core, "app-a1b2").unwrap();
    assert!(removed.commit.is_some());
    assert!(projects::list(&fx.core).unwrap().is_empty());
    assert!(!fx.read(&root.join(".mcp.json")).contains("docs-mcp"));
}

/// The trust notice reports the trusted project's own files: another
/// project that still waits for trust is listed, but is not its status (#10).
#[test]
fn trust_status_ignores_another_untrusted_project() {
    let fx = Fx::new();
    fx.stage(&[(
        "servers/github.toml",
        &server_file(
            "github",
            "transport = \"stdio\"\ncommand = \"github-mcp\"",
            &["claude-code"],
        ),
    )]);
    let root = unbound_project(&fx);
    let other = fx.th.projects().join("other");
    std::fs::create_dir_all(other.join(".loadout")).unwrap();
    std::fs::write(
        other.join(".loadout/project.toml"),
        "schema = 1\nid = \"other-c3d4\"\nname = \"other\"\n\n[defaults]\n\
         clients = [\"claude-code\"]\n\n[servers.notes]\ntransport = \"stdio\"\n\
         command = \"notes-mcp\"\n",
    )
    .unwrap();
    projects::add(&fx.core, &root).unwrap();
    projects::add(&fx.core, &other).unwrap();

    let review = projects::review(&fx.core, "app-a1b2").unwrap();
    let trusted = projects::trust(&fx.core, "app-a1b2", &review.hash).unwrap();
    let run = trusted.change.apply.expect("instant mode applies");
    assert!(
        run.files.iter().any(|file| file
            .error
            .as_ref()
            .is_some_and(|error| error.code == agent_loadout_core::ErrorCode::TrustRequired)),
        "{run:?}"
    );
    assert_eq!(run.status, ApplyStatusKind::Done);
    assert!(fx.read(&root.join(".mcp.json")).contains("docs-mcp"));
}

/// The matrix cell carries the effective view's provenance: winner,
/// shadowed entries, other readers of the file, warnings, the snippet.
#[test]
fn matrix_cells_show_provenance() {
    let fx = Fx::new();
    fx.stage(&[(
        "servers/github.toml",
        &server_file(
            "github",
            "transport = \"stdio\"\ncommand = \"github-mcp\"",
            &["claude-code"],
        ),
    )]);
    let root = unbound_project(&fx);
    projects::add(&fx.core, &root).unwrap();
    let hash = projects::review(&fx.core, "app-a1b2").unwrap().hash;
    projects::trust(&fx.core, "app-a1b2", &hash).unwrap();

    let matrix = effective::matrix(&fx.core, "app-a1b2").unwrap();
    assert!(matrix.clients.iter().any(|c| c.id == "claude-code"));
    let cell = |name: &str, client: &str| {
        matrix
            .rows
            .iter()
            .find(|row| row.name == name)
            .unwrap_or_else(|| panic!("row {name}: {:#?}", matrix.rows))
            .cells
            .iter()
            .find(|cell| cell.client == client)
            .cloned()
            .unwrap_or_else(|| panic!("cell {name}/{client}"))
    };
    let github = cell("github", "claude-code");
    assert_eq!(github.winner.layer, LayerKind::Project);
    assert_eq!(github.winner.file, "<P>/.mcp.json");
    assert_eq!(github.source, "store:github+project");
    assert!(github.loaded);
    assert!(
        github
            .shadowed
            .iter()
            .any(|s| s.layer == LayerKind::System && s.file == "~/.claude.json"),
        "{github:#?}"
    );
    assert!(!github.also_visible_to.is_empty(), "{github:#?}");
    assert!(!github.also_visible_to.contains(&"claude-code".to_owned()));
    assert_eq!(github.status, Some(EntryStateKind::Synced));
    let snippet = github.snippet.as_deref().unwrap();
    assert!(snippet.contains("--project"), "{snippet}");
    assert_eq!(cell("docs", "claude-code").source, "project:docs");
    fx.assert_no_root(&serde_json::to_string(&matrix).unwrap());

    // A changed manifest warns on the project's entries.
    let text = format!("{APP_MANIFEST}# pulled\n");
    std::fs::write(root.join(".loadout/project.toml"), text).unwrap();
    let matrix = effective::matrix(&fx.core, "app-a1b2").unwrap();
    let docs = matrix
        .rows
        .iter()
        .find(|row| row.name == "docs")
        .unwrap()
        .cells
        .iter()
        .find(|cell| cell.client == "claude-code")
        .unwrap();
    assert!(
        docs.warnings
            .iter()
            .any(|w| w.code == agent_loadout_core::ErrorCode::TrustRequired),
        "{docs:#?}"
    );
    fx.assert_no_root(&serde_json::to_string(&matrix).unwrap());
}

fn user_entry(name: &str) -> EntryRef {
    EntryRef {
        file: FileRef {
            project_id: None,
            scope: FileScope::User,
            file: "~/.claude.json".to_owned(),
        },
        client: "claude-code".to_owned(),
        name: name.to_owned(),
    }
}

/// Runs `action` with the fingerprint of the detail the resolver shows now.
fn act(
    core: &Core,
    entry: &EntryRef,
    action: ResolveAction,
) -> super::CmdResult<super::ChangeResult> {
    let shown = resolve::detail(core, entry).unwrap().fingerprint;
    resolve::run(core, entry, action, &shown)
}

/// Saves `fake` for Claude Code, then edits its entry by hand: drifted.
fn drifted(fx: &Fx) {
    servers::save(&fx.core, &draft("fake", &["claude-code"]), true).unwrap();
    let text = fx
        .read(&fx.claude())
        .replace("\"fake-mcp\"", "\"fake-mcp-edited\"");
    std::fs::write(fx.claude(), text).unwrap();
}

#[test]
fn resolver_overwrites_a_drifted_entry() {
    let fx = Fx::new();
    drifted(&fx);
    let detail = resolve::detail(&fx.core, &user_entry("fake")).unwrap();
    assert_eq!(detail.state, EntryStateKind::Drifted);
    assert_eq!(
        detail.actions,
        [
            ResolveAction::Overwrite,
            ResolveAction::ImportTheirs,
            ResolveAction::Release,
            ResolveAction::Skip
        ]
    );
    assert_eq!(detail.file, "~/.claude.json");
    let last = detail.last_written.as_deref().unwrap();
    assert!(last.contains("\"fake-mcp\""), "{last}");
    assert!(
        detail
            .current
            .as_deref()
            .unwrap()
            .contains("fake-mcp-edited")
    );
    assert!(detail.desired.as_deref().unwrap().contains("\"fake-mcp\""));

    // Skip changes nothing.
    let before = fx.commits();
    let skipped = act(&fx.core, &user_entry("fake"), ResolveAction::Skip).unwrap();
    assert!(skipped.commit.is_none());
    // An action the state does not allow is refused.
    let err = act(&fx.core, &user_entry("fake"), ResolveAction::Recreate).unwrap_err();
    assert_eq!(err.code, agent_loadout_core::ErrorCode::InvalidValue);

    // The file changes after the resolver showed it: refused, nothing done.
    let shown = detail.fingerprint.clone();
    let text = fx
        .read(&fx.claude())
        .replace("fake-mcp-edited", "fake-mcp-edited-again");
    std::fs::write(fx.claude(), text).unwrap();
    let err = resolve::run(
        &fx.core,
        &user_entry("fake"),
        ResolveAction::ImportTheirs,
        &shown,
    )
    .unwrap_err();
    assert_eq!(err.code, agent_loadout_core::ErrorCode::EntryChanged);
    assert_eq!(fx.commits(), before, "nothing committed");
    assert!(
        !fx.store_file("servers/fake.toml")
            .contains("fake-mcp-edited")
    );
    // The core checks again inside the transaction.
    let store = fx.store();
    let err = agent_loadout_core::apply::resolve::resolve(
        &store,
        &fx.core.apply_env(),
        agent_loadout_core::apply::TargetFilter {
            scope: agent_loadout_core::model::applied::Scope::User,
            project: None,
            file: fx.claude(),
        },
        "claude-code",
        "fake",
        agent_loadout_core::apply::EntryState::Drifted,
        agent_loadout_core::apply::OnConflict::Import,
        &shown,
    )
    .unwrap_err();
    assert_eq!(err.code(), agent_loadout_core::ErrorCode::EntryChanged);
    drop(store);
    assert_eq!(fx.commits(), before);

    let result = act(&fx.core, &user_entry("fake"), ResolveAction::Overwrite).unwrap();
    assert_eq!(fx.commits(), before + 1, "one commit");
    assert!(result.apply.unwrap().files_written == 1);
    let text = fx.read(&fx.claude());
    assert!(
        text.contains("\"fake-mcp\"") && !text.contains("fake-mcp-edited"),
        "{text}"
    );
    assert!(fx.summaries()[0].starts_with("resolve: overwrite claude-code/fake"));
}

#[test]
fn resolver_imports_a_drifted_change_into_the_store() {
    let fx = Fx::new();
    drifted(&fx);
    let before = fx.commits();
    act(&fx.core, &user_entry("fake"), ResolveAction::ImportTheirs).unwrap();
    assert_eq!(fx.commits(), before + 1);
    assert_eq!(fx.summaries()[0], "adopt: claude-code/fake drift");
    assert!(
        fx.store_file("servers/fake.toml")
            .contains("fake-mcp-edited")
    );
    // The file stays as the user left it, and is synced now.
    assert!(fx.read(&fx.claude()).contains("fake-mcp-edited"));
    let pending = apply::pending(&fx.core).unwrap();
    assert_eq!(pending.unresolved, 0, "{:#?}", pending.files);
}

/// Copies one machine's store to another (a sync through a shared
/// remote, as far as these tests need it).
fn copy_store(from: &Fx, to: &Fx) {
    fn copy(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let target = dst.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), &target).unwrap();
            }
        }
    }
    let dst = to.core.home.store_dir();
    if dst.exists() {
        std::fs::remove_dir_all(&dst).unwrap();
    }
    copy(&from.core.home.store_dir(), &dst);
}

/// Release is a fact about this machine: it is recorded in this machine's
/// overlay, and another machine sharing the store keeps managing (and
/// keeps) its own entry.
#[test]
fn resolver_releases_ownership_on_this_machine_only() {
    let b = Fx::new();
    servers::save(&b.core, &draft("fake", &["claude-code"]), true).unwrap();
    assert!(b.read(&b.claude()).contains("\"fake-mcp\""));

    // Machine A gets the store, applies it, then edits its entry.
    let a = Fx::new();
    copy_store(&b, &a);
    apply::run(&a.core, None).unwrap();
    let text = a
        .read(&a.claude())
        .replace("\"fake-mcp\"", "\"fake-mcp-edited\"");
    std::fs::write(a.claude(), text).unwrap();

    act(&a.core, &user_entry("fake"), ResolveAction::Release).unwrap();
    assert_eq!(a.summaries()[0], "resolve: release claude-code/fake");
    // On A: the entry stays as edited, nothing pending, nothing owned.
    assert!(a.read(&a.claude()).contains("fake-mcp-edited"));
    assert_eq!(apply::pending(&a.core).unwrap().pending, 0);
    let ledger =
        std::fs::read_to_string(a.core.home.state_dir().join("ledger.json")).unwrap_or_default();
    assert!(!ledger.contains("\"key\": \"fake\""), "{ledger}");
    // The synced server is untouched; A's overlay records the release.
    let server = a.store_file("servers/fake.toml");
    assert!(server.contains("\"claude-code\""), "{server}");
    let machine =
        agent_loadout_core::store::Machine::load(a.core.fs.as_ref(), &a.core.home.state_dir())
            .unwrap()
            .unwrap()
            .id;
    let overlay = a.store_file(&format!("machines/{machine}.toml"));
    assert!(overlay.contains("[enable.released]"), "{overlay}");

    // Machine B syncs: its unchanged entry is neither removed nor pending.
    copy_store(&a, &b);
    let pending = apply::pending(&b.core).unwrap();
    assert_eq!(pending.pending, 0, "{:#?}", pending.files);
    apply::run(&b.core, None).unwrap();
    assert!(b.read(&b.claude()).contains("\"fake-mcp\""));

    // Enabling the client again on A manages the entry there again.
    servers::set_clients_for(
        &a.core,
        "fake",
        &ClientChoice {
            all_detected: false,
            clients: vec!["claude-code".to_owned()],
        },
    )
    .unwrap();
    assert!(
        !a.store_file(&format!("machines/{machine}.toml"))
            .contains("released")
    );
}

#[test]
fn resolver_recreates_or_respects_a_deletion() {
    let fx = Fx::new();
    servers::save(&fx.core, &draft("fake", &["claude-code"]), true).unwrap();
    std::fs::write(fx.claude(), "{\n  \"mcpServers\": {}\n}\n").unwrap();
    let detail = resolve::detail(&fx.core, &user_entry("fake")).unwrap();
    assert_eq!(detail.state, EntryStateKind::Missing);
    assert_eq!(
        detail.actions,
        [
            ResolveAction::Recreate,
            ResolveAction::RespectDeletion,
            ResolveAction::Skip
        ]
    );
    assert!(detail.current.is_none());

    act(
        &fx.core,
        &user_entry("fake"),
        ResolveAction::RespectDeletion,
    )
    .unwrap();
    assert_eq!(
        fx.summaries()[0],
        "resolve: respect deletion claude-code/fake"
    );
    assert!(
        !fx.store_file("servers/fake.toml")
            .contains("\"claude-code\"")
    );
    assert!(!fx.read(&fx.claude()).contains("fake"));

    // Re-create: enabled again, deleted again, then written back.
    servers::set_clients_for(
        &fx.core,
        "fake",
        &ClientChoice {
            all_detected: false,
            clients: vec!["claude-code".to_owned()],
        },
    )
    .unwrap();
    std::fs::write(fx.claude(), "{\n  \"mcpServers\": {}\n}\n").unwrap();
    act(&fx.core, &user_entry("fake"), ResolveAction::Recreate).unwrap();
    assert!(fx.read(&fx.claude()).contains("\"fake-mcp\""));
}

#[test]
fn resolver_handles_a_collision() {
    let fx = Fx::new();
    std::fs::write(
        fx.claude(),
        "{\n  \"mcpServers\": {\n    \"fake\": { \"command\": \"theirs-mcp\" }\n  }\n}\n",
    )
    .unwrap();
    servers::save(&fx.core, &draft("fake", &["claude-code"]), true).unwrap();
    let detail = resolve::detail(&fx.core, &user_entry("fake")).unwrap();
    assert_eq!(detail.state, EntryStateKind::Collision);
    assert_eq!(
        detail.actions,
        [
            ResolveAction::Overwrite,
            ResolveAction::ImportTheirs,
            ResolveAction::Skip
        ]
    );
    assert!(detail.last_written.is_none(), "never written");
    assert!(detail.current.as_deref().unwrap().contains("theirs-mcp"));

    act(&fx.core, &user_entry("fake"), ResolveAction::ImportTheirs).unwrap();
    assert!(fx.store_file("servers/fake.toml").contains("theirs-mcp"));
    assert!(fx.read(&fx.claude()).contains("theirs-mcp"));
    assert_eq!(apply::pending(&fx.core).unwrap().unresolved, 0);
}

#[test]
fn resolver_adopts_and_overwrites_a_collision() {
    let fx = Fx::new();
    std::fs::write(
        fx.claude(),
        "{\n  \"mcpServers\": {\n    \"fake\": { \"command\": \"theirs-mcp\" }\n  }\n}\n",
    )
    .unwrap();
    servers::save(&fx.core, &draft("fake", &["claude-code"]), true).unwrap();
    act(&fx.core, &user_entry("fake"), ResolveAction::Overwrite).unwrap();
    let text = fx.read(&fx.claude());
    assert!(
        text.contains("\"fake-mcp\"") && !text.contains("theirs-mcp"),
        "{text}"
    );
    assert_eq!(apply::pending(&fx.core).unwrap().pending, 0);
}

/// A conflict (drift plus a store change) shows all three versions: what
/// was last written is rendered again from the store commit the ledger
/// names.
#[test]
fn resolver_shows_three_versions_of_a_conflict() {
    let fx = Fx::new();
    drifted(&fx);
    fx.stage(&[(
        "servers/fake.toml",
        &server_file(
            "fake",
            "transport = \"stdio\"\ncommand = \"npx\"\nargs = [\"-y\", \"fake-mcp-v2\"]",
            &["claude-code"],
        ),
    )]);
    // Another slot's row for the same file, container and key (a project
    // scope, another client) comes first in the ledger: the lookup is by
    // slot and client, so it is not taken.
    let ledger_path = fx.core.home.state_dir().join("ledger.json");
    let mut ledger: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&ledger_path).unwrap()).unwrap();
    let entries = ledger["entries"].as_array_mut().unwrap();
    let mut decoy = entries
        .iter()
        .find(|row| row["key"] == "fake")
        .unwrap()
        .clone();
    decoy["scope"] = "project".into();
    decoy["project_id"] = "decoy-a1b2".into();
    decoy["client"] = "codex".into();
    decoy["written_hash"] = format!("hmac-sha256:{}", "0".repeat(64)).into();
    entries.insert(0, decoy);
    std::fs::write(&ledger_path, serde_json::to_string_pretty(&ledger).unwrap()).unwrap();
    let detail = resolve::detail(&fx.core, &user_entry("fake")).unwrap();
    assert_eq!(detail.state, EntryStateKind::Conflict);
    let last = detail.last_written.as_deref().expect("recovered");
    assert!(
        last.contains("\"fake-mcp\"") && last.contains("LOG_LEVEL"),
        "{last}"
    );
    assert!(
        detail
            .current
            .as_deref()
            .unwrap()
            .contains("fake-mcp-edited")
    );
    assert!(detail.desired.as_deref().unwrap().contains("fake-mcp-v2"));
    assert!(detail.actions.contains(&ResolveAction::Release));
}

/// Project entries come from manifests: no import or release into the
/// store is offered for them.
#[test]
fn project_entries_offer_no_store_edits() {
    let fx = Fx::new();
    let root = unbound_project(&fx);
    fx.stage(&[(
        "servers/github.toml",
        &server_file("github", "transport = \"stdio\"\ncommand = \"gh-mcp\"", &[]),
    )]);
    projects::add(&fx.core, &root).unwrap();
    let hash = projects::review(&fx.core, "app-a1b2").unwrap().hash;
    projects::trust(&fx.core, "app-a1b2", &hash).unwrap();
    let mcp = root.join(".mcp.json");
    let text = fx.read(&mcp).replace("\"docs-mcp\"", "\"docs-mcp-edited\"");
    std::fs::write(&mcp, text).unwrap();
    let entry = EntryRef {
        file: FileRef {
            project_id: Some("app-a1b2".to_owned()),
            scope: FileScope::Project,
            file: "<P>/.mcp.json".to_owned(),
        },
        client: "claude-code".to_owned(),
        name: "docs".to_owned(),
    };
    let detail = resolve::detail(&fx.core, &entry).unwrap();
    assert_eq!(detail.state, EntryStateKind::Drifted);
    assert_eq!(
        detail.actions,
        [ResolveAction::Overwrite, ResolveAction::Skip]
    );
    assert_eq!(detail.file, "<P>/.mcp.json");
    act(&fx.core, &entry, ResolveAction::Overwrite).unwrap();
    assert!(!fx.read(&mcp).contains("docs-mcp-edited"));
}

/// A folder that is a link to the home folder is the home folder: refused
/// when picked, and a known project there is unusable (SPEC 6.1, 17).
#[test]
fn a_link_to_home_is_the_home_folder() {
    let fx = Fx::new();
    let link = fx.th.projects().join("home-link");
    std::fs::create_dir_all(fx.th.projects()).unwrap();
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(fx.th.home(), &link);
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(fx.th.home(), &link);
    if let Err(err) = made {
        eprintln!("skipped a_link_to_home_is_the_home_folder: cannot create a symlink here: {err}");
        return;
    }
    let err = projects::inspect(&fx.core, &link, "t".to_owned()).unwrap_err();
    assert_eq!(
        err.code,
        agent_loadout_core::ErrorCode::InvalidPath,
        "{}",
        err.message
    );
    let err = projects::init(&fx.core, &link, false, None).unwrap_err();
    assert_eq!(err.code, agent_loadout_core::ErrorCode::InvalidPath);
    assert!(!fx.th.home().join(".loadout").exists());
    // Known through the link (an overlay written elsewhere): unusable.
    std::fs::create_dir_all(fx.th.home().join(".loadout")).unwrap();
    std::fs::write(
        fx.th.home().join(".loadout/project.local.toml"),
        "schema = 1\nid = \"linked-a1b2\"\n",
    )
    .unwrap();
    let overlay = agent_loadout_core::model::machine::MachineOverlay {
        projects: Some(
            [(
                "linked-a1b2".to_owned(),
                agent_loadout_core::model::machine::MachineProject {
                    path: link.to_string_lossy().into_owned(),
                    extra: Default::default(),
                },
            )]
            .into(),
        ),
        ..agent_loadout_core::model::machine::MachineOverlay::new(
            "01J0000000000000000000000A",
            "test-desk",
            host_os(),
        )
    };
    let known = agent_loadout_core::layering::known_projects(
        fx.core.fs.as_ref(),
        &fx.core.env,
        Some(&overlay),
        &agent_loadout_core::layering::TrustStore::default(),
    );
    assert!(
        matches!(
            known[0].status,
            agent_loadout_core::layering::ProjectStatus::Failed(_)
        ),
        "{:?}",
        known[0].status
    );
}

/// The synced mirror is not what this machine trusted: after a remove and
/// a new add, the review shows every file in full.
#[test]
fn review_shows_full_content_without_a_trust_record_here() {
    let fx = Fx::new();
    let root = unbound_project(&fx);
    projects::add(&fx.core, &root).unwrap();
    let hash = projects::review(&fx.core, "app-a1b2").unwrap().hash;
    projects::trust(&fx.core, "app-a1b2", &hash).unwrap();
    // Trusted here, unchanged: shown as unchanged.
    let review = projects::review(&fx.core, "app-a1b2").unwrap();
    assert_eq!(review.files[0].change, ManifestChangeKind::Unchanged);

    projects::remove(&fx.core, "app-a1b2").unwrap();
    projects::add(&fx.core, &root).unwrap();
    let review = projects::review(&fx.core, "app-a1b2").unwrap();
    assert_eq!(review.trust, ProjectTrust::Untrusted);
    assert_eq!(review.files[0].change, ManifestChangeKind::New);
    assert!(
        review.files[0]
            .content
            .as_deref()
            .unwrap()
            .contains("docs-mcp")
    );
}

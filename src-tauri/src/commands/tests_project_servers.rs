//! M24: the MCP page at project scope and Discard pending, against a temp
//! home (`RootedFs`, in-memory keyring; nothing outside the temp root).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agent_loadout_core::apply::discard;
use agent_loadout_core::layering::project_servers::{ServerEdit, edit_server};
use agent_loadout_core::layering::{EntryLayer, Project, TrustState, TrustStore};
use agent_loadout_core::model::clients::ClientSelection;
use agent_loadout_core::model::project::ProjectServer;
use agent_loadout_core::model::server::{ServerBody, Transport};
use agent_loadout_core::{ErrorCode, Fs};

use super::apply;
use super::project_servers::{
    self, ManifestLayerKind, ProjectServerDraft, ProjectServerKind, ProjectServerList,
};
use super::projects::{self, ProjectTrust};
use super::servers::{self, ClientChoice, KeyValue, PillState, TransportKind};
use super::tests::{Fx, draft};

fn only(clients: &[&str]) -> Option<ClientChoice> {
    Some(ClientChoice {
        all_detected: false,
        clients: clients.iter().map(|c| (*c).to_owned()).collect(),
    })
}

fn define(name: &str, layer: ManifestLayerKind, command: &str) -> ProjectServerDraft {
    ProjectServerDraft {
        name: name.to_owned(),
        layer,
        kind: ProjectServerKind::Define,
        from: None,
        transport: Some(TransportKind::Stdio),
        command: Some(command.to_owned()),
        args: vec!["--stdio".to_owned()],
        url: None,
        env: vec![KeyValue {
            key: "TOKEN".to_owned(),
            value: "${env:DOCS_TOKEN}".to_owned(),
        }],
        headers: Vec::new(),
        clients: only(&["claude-code"]),
        over_shared: false,
    }
}

/// A shared project, initialized and trusted through the app.
fn shared_project(fx: &Fx) -> (std::path::PathBuf, String) {
    let root = fx.th.projects().join("app");
    std::fs::create_dir_all(&root).unwrap();
    let done = projects::init(&fx.core, &root, true, Some("app")).unwrap();
    (root, done.id)
}

fn list(fx: &Fx, id: &str) -> ProjectServerList {
    let list = project_servers::list(&fx.core, id).unwrap();
    fx.assert_no_root(&serde_json::to_string(&list).unwrap());
    list
}

fn pill(list: &ProjectServerList, name: &str, client: &str) -> (bool, PillState) {
    let row = list
        .servers
        .iter()
        .find(|row| row.name == name)
        .unwrap_or_else(|| panic!("no row {name}: {list:#?}"));
    let index = list
        .clients
        .iter()
        .position(|info| info.id == client)
        .expect("client listed");
    let pill = &row.pills[index];
    (pill.selected, pill.state)
}

#[test]
fn project_scope_lists_edits_and_applies_manifest_servers() {
    let fx = Fx::new();
    servers::save(&fx.core, &draft("github", &["claude-code"]), true).unwrap();
    let (root, id) = shared_project(&fx);
    let manifest = root.join(".loadout/project.toml");
    let mcp = root.join(".mcp.json");

    let listed = list(&fx, &id);
    assert_eq!(listed.project.trust, ProjectTrust::Trusted);
    assert!(
        listed.project.path.ends_with("/app"),
        "{}",
        listed.project.path
    );
    assert_eq!(
        listed.project.layers,
        [ManifestLayerKind::Shared, ManifestLayerKind::Local]
    );
    assert!(listed.servers.is_empty());
    assert_eq!(listed.inherited.len(), 1, "{:#?}", listed.inherited);
    assert_eq!(listed.inherited[0].id, "github");
    assert_eq!(listed.store_servers, ["github"]);

    // Add a project-only server to the shared manifest: one commit that
    // trusts the new content and writes <P>/.mcp.json.
    let before = fx.commits();
    let added = project_servers::save(
        &fx.core,
        &id,
        &listed.project.hash,
        &define("docs", ManifestLayerKind::Shared, "docs-mcp"),
        true,
    )
    .unwrap();
    assert_eq!(
        fx.commits(),
        before + 1,
        "edit, trust and apply are one commit"
    );
    assert!(added.apply.is_some_and(|run| run.files_written >= 1));
    assert!(fx.read(&manifest).contains("[servers.docs]"));
    assert!(fx.read(&mcp).contains("docs-mcp"), "{}", fx.read(&mcp));
    assert_eq!(fx.summaries()[0], format!("project {id}: add docs"));

    let listed = list(&fx, &id);
    assert_eq!(
        listed.project.trust,
        ProjectTrust::Trusted,
        "Loadout trusts its own edit"
    );
    let row = &listed.servers[0];
    assert_eq!(row.name, "docs");
    assert_eq!(row.kind, ProjectServerKind::Define);
    assert_eq!(row.layers, [ManifestLayerKind::Shared]);
    assert_eq!(row.edit_layer, ManifestLayerKind::Shared);
    assert_eq!(row.launch, "docs-mcp --stdio");
    assert_eq!(
        pill(&listed, "docs", "claude-code"),
        (true, PillState::Enabled)
    );
    // Claude Desktop has no project config: hatched, with the reason.
    if let Some(index) = listed.clients.iter().position(|c| c.id == "claude-desktop") {
        assert_eq!(row.pills[index].state, PillState::NotSupported);
        let reason = row.pills[index].reason.as_ref().unwrap();
        assert!(matches!(
            reason.code,
            ErrorCode::ScopeNotSupported | ErrorCode::NotSupportedOnOs
        ));
    }
    assert!(listed.inherited.iter().any(|server| server.id == "github"));

    // The dialog reads the table back; the secret stays a reference.
    let read = project_servers::get(&fx.core, &id, "docs", ManifestLayerKind::Shared).unwrap();
    assert_eq!(read, define("docs", ManifestLayerKind::Shared, "docs-mcp"));

    // Disable All writes `clients = []`; the entry leaves .mcp.json.
    project_servers::set_clients(
        &fx.core,
        &id,
        &listed.project.hash,
        "docs",
        ManifestLayerKind::Shared,
        only(&[]).as_ref(),
    )
    .unwrap();
    assert!(!fx.read(&mcp).contains("docs-mcp"));
    let listed = list(&fx, &id);
    assert_eq!(
        pill(&listed, "docs", "claude-code"),
        (false, PillState::Disabled)
    );
    project_servers::set_clients(
        &fx.core,
        &id,
        &listed.project.hash,
        "docs",
        ManifestLayerKind::Shared,
        only(&["claude-code"]).as_ref(),
    )
    .unwrap();
    assert!(fx.read(&mcp).contains("docs-mcp"));

    // Override the store server privately: project.local.toml is created
    // and kept out of the project's repository; Claude Code gets it in its
    // private target, the shared .mcp.json does not.
    let listed = list(&fx, &id);
    let override_github = ProjectServerDraft {
        name: "github".to_owned(),
        layer: ManifestLayerKind::Local,
        kind: ProjectServerKind::Override,
        from: Some("github".to_owned()),
        transport: None,
        command: None,
        args: vec!["--project-scope".to_owned()],
        url: None,
        env: Vec::new(),
        headers: Vec::new(),
        clients: only(&["claude-code"]),
        over_shared: false,
    };
    project_servers::save(&fx.core, &id, &listed.project.hash, &override_github, true).unwrap();
    let local = fx.read(&root.join(".loadout/project.local.toml"));
    assert!(local.contains("from = \"github\""), "{local}");
    assert!(
        fx.read(&root.join(".loadout/.gitignore"))
            .lines()
            .any(|line| line == "project.local.toml")
    );
    assert!(fx.read(&fx.claude()).contains("--project-scope"));
    assert!(!fx.read(&mcp).contains("--project-scope"));
    let listed = list(&fx, &id);
    let row = listed
        .servers
        .iter()
        .find(|row| row.name == "github")
        .unwrap();
    assert_eq!(row.kind, ProjectServerKind::Override);
    assert_eq!(row.edit_layer, ManifestLayerKind::Local);
    assert!(
        listed.inherited.is_empty(),
        "github is named by the manifest now"
    );

    // An unknown `from` is refused before anything is written.
    let mut unknown = override_github.clone();
    unknown.name = "other".to_owned();
    unknown.from = Some("nope".to_owned());
    let err =
        project_servers::save(&fx.core, &id, &listed.project.hash, &unknown, true).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    // Remove: the table goes, and so does the entry it wrote.
    project_servers::remove(
        &fx.core,
        &id,
        &listed.project.hash,
        "docs",
        ManifestLayerKind::Shared,
    )
    .unwrap();
    assert!(!fx.read(&manifest).contains("[servers.docs]"));
    assert!(!fx.read(&mcp).contains("docs-mcp"));
}

#[test]
fn an_untrusted_or_changed_project_is_read_only() {
    let fx = Fx::new();
    let (root, id) = shared_project(&fx);
    let manifest = root.join(".loadout/project.toml");
    let listed = list(&fx, &id);
    let hash = listed.project.hash.clone();

    // A hash the user was not shown (the file changed since the list).
    let err = project_servers::save(
        &fx.core,
        &id,
        "sha256:stale",
        &define("docs", ManifestLayerKind::Shared, "docs-mcp"),
        true,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::TrustRequired);
    fx.assert_no_root(&err.message);

    // A hand edit (a `git pull`): the project is "changed" and read only
    // until it is reviewed and trusted, whatever hash is sent.
    let pulled = format!(
        "{}\n[servers.pulled]\ntransport = \"stdio\"\ncommand = \"pulled-mcp\"\n",
        fx.read(&manifest)
    );
    std::fs::write(&manifest, &pulled).unwrap();
    let listed = list(&fx, &id);
    assert_eq!(listed.project.trust, ProjectTrust::Changed);
    assert_eq!(listed.servers[0].name, "pulled", "listed, read only");
    let before = fx.commits();
    for hash in [hash.as_str(), listed.project.hash.as_str()] {
        let err = project_servers::save(
            &fx.core,
            &id,
            hash,
            &define("docs", ManifestLayerKind::Shared, "docs-mcp"),
            true,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::TrustRequired, "{}", err.message);
        fx.assert_no_root(&err.message);
    }
    let err = project_servers::remove(
        &fx.core,
        &id,
        &listed.project.hash,
        "pulled",
        ManifestLayerKind::Shared,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::TrustRequired);
    assert_eq!(fx.commits(), before, "nothing committed");
    assert_eq!(fx.read(&manifest), pulled, "the manifest is left as it is");
    assert!(!fx.read(&root.join(".mcp.json")).contains("pulled-mcp"));

    // Review and trust, then edits work again.
    let review = projects::review(&fx.core, &id).unwrap();
    projects::trust(&fx.core, &id, &review.hash).unwrap();
    let listed = list(&fx, &id);
    project_servers::set_clients(
        &fx.core,
        &id,
        &listed.project.hash,
        "pulled",
        ManifestLayerKind::Shared,
        only(&["claude-code"]).as_ref(),
    )
    .unwrap();
    assert!(fx.read(&root.join(".mcp.json")).contains("pulled-mcp"));
}

#[test]
fn a_private_project_writes_only_its_local_manifest() {
    let fx = Fx::new();
    let root = fx.th.projects().join("solo");
    std::fs::create_dir_all(&root).unwrap();
    let id = projects::init(&fx.core, &root, false, None).unwrap().id;
    let listed = list(&fx, &id);
    assert_eq!(listed.project.layers, [ManifestLayerKind::Local]);
    let err = project_servers::save(
        &fx.core,
        &id,
        &listed.project.hash,
        &define("docs", ManifestLayerKind::Shared, "docs-mcp"),
        true,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidValue);
    assert!(!root.join(".loadout/project.toml").exists());

    project_servers::save(
        &fx.core,
        &id,
        &listed.project.hash,
        &define("docs", ManifestLayerKind::Local, "docs-mcp"),
        true,
    )
    .unwrap();
    assert!(
        fx.read(&root.join(".loadout/project.local.toml"))
            .contains("[servers.docs]")
    );
    // A second add of the same name is refused.
    let listed = list(&fx, &id);
    let err = project_servers::save(
        &fx.core,
        &id,
        &listed.project.hash,
        &define("docs", ManifestLayerKind::Local, "docs-mcp"),
        true,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::AlreadyExists);

    // Hide a store server in this folder (a suppression).
    servers::save(&fx.core, &draft("noisy", &["claude-code"]), true).unwrap();
    let listed = list(&fx, &id);
    let hide = ProjectServerDraft {
        name: "noisy".to_owned(),
        layer: ManifestLayerKind::Local,
        kind: ProjectServerKind::Suppress,
        from: None,
        transport: None,
        command: None,
        args: Vec::new(),
        url: None,
        env: Vec::new(),
        headers: Vec::new(),
        clients: None,
        over_shared: false,
    };
    project_servers::save(&fx.core, &id, &listed.project.hash, &hide, true).unwrap();
    let listed = list(&fx, &id);
    let row = listed
        .servers
        .iter()
        .find(|row| row.name == "noisy")
        .unwrap();
    assert_eq!(row.kind, ProjectServerKind::Suppress);
    assert!(row.launch.is_empty());
    let local = fx.read(&root.join(".loadout/project.local.toml"));
    assert!(local.contains("enabled = false"), "{local}");
}

fn review_mode(fx: &Fx) {
    fx.stage(&[("settings.toml", "schema = 1\napply_mode = \"review\"\n")]);
}

#[test]
fn discard_pending_restores_the_last_applied_state_without_touching_clients() {
    let fx = Fx::new();
    servers::save(&fx.core, &draft("fake", &["claude-code"]), true).unwrap();
    servers::save(&fx.core, &draft("kept", &["claude-code"]), true).unwrap();
    review_mode(&fx);
    // Nothing waits for an apply: switching to review mode is a setting.
    assert!(apply::store_changes(&fx.core).unwrap().is_empty());

    let mut changed = draft("fake", &["claude-code"]);
    changed.args = vec!["--changed".to_owned()];
    let saved = servers::save(&fx.core, &changed, false).unwrap();
    assert!(saved.apply.is_none(), "review mode: not applied");
    servers::save(&fx.core, &draft("added", &["claude-code"]), true).unwrap();
    servers::remove(&fx.core, &["kept".to_owned()]).unwrap();
    let client_file = fx.read(&fx.claude());
    assert!(apply::pending(&fx.core).unwrap().writes >= 1);

    let changes = apply::store_changes(&fx.core).unwrap();
    let listed: Vec<(&str, apply::PendingChangeKind)> = changes
        .iter()
        .map(|change| (change.entity.as_str(), change.kind))
        .collect();
    assert_eq!(
        listed,
        [
            ("server/added", apply::PendingChangeKind::Added),
            ("server/fake", apply::PendingChangeKind::Changed),
            ("server/kept", apply::PendingChangeKind::Removed),
        ]
    );

    // One item: only that server goes back.
    let before = fx.commits();
    let one = apply::discard(&fx.core, Some(&["server/added".to_owned()])).unwrap();
    assert_eq!(one.discarded, ["server/added"]);
    assert_eq!(fx.commits(), before + 1);
    assert_eq!(fx.summaries()[0], "discard: server/added");
    assert!(
        one.tag
            .as_deref()
            .is_some_and(|tag| tag.contains("pre-restore"))
    );
    assert!(
        servers::list(&fx.core)
            .unwrap()
            .servers
            .iter()
            .all(|row| row.id != "added")
    );
    assert_eq!(fx.read(&fx.claude()), client_file, "no client file touched");
    assert_eq!(apply::store_changes(&fx.core).unwrap().len(), 2);

    // Something without a pending change is refused.
    let err = apply::discard(&fx.core, Some(&["server/added".to_owned()])).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = apply::discard(&fx.core, Some(&["settings".to_owned()])).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    // All: the store is what this machine last applied; the plan is empty.
    let all = apply::discard(&fx.core, None).unwrap();
    assert_eq!(all.discarded, ["server/fake", "server/kept"]);
    assert_eq!(fx.summaries()[0], "discard: 2 pending changes");
    assert!(apply::store_changes(&fx.core).unwrap().is_empty());
    assert_eq!(apply::pending(&fx.core).unwrap().writes, 0);
    assert_eq!(fx.read(&fx.claude()), client_file, "no client file touched");
    assert!(fx.store_file("servers/kept.toml").contains("kept"));
    assert!(!fx.store_file("servers/fake.toml").contains("--changed"));
    assert!(
        fx.store_file("settings.toml").contains("review"),
        "settings are not discarded"
    );

    // Nothing left: no commit.
    let before = fx.commits();
    let none = apply::discard(&fx.core, None).unwrap();
    assert!(none.discarded.is_empty() && none.commit.is_none());
    assert_eq!(fx.commits(), before);
}

#[test]
fn discard_compares_with_the_last_apply_run() {
    let fx = Fx::new();
    review_mode(&fx);
    // Never applied here: everything since the initial commit is pending.
    servers::save(&fx.core, &draft("fake", &["claude-code"]), true).unwrap();
    assert_eq!(apply::store_changes(&fx.core).unwrap().len(), 1);
    // Applying makes it the last applied state.
    apply::run(&fx.core, None).unwrap();
    assert!(fx.read(&fx.claude()).contains("\"fake\""));
    assert!(apply::store_changes(&fx.core).unwrap().is_empty());
    let mut changed = draft("fake", &["claude-code"]);
    changed.command = Some("fake-mcp-2".to_owned());
    servers::save(&fx.core, &changed, false).unwrap();
    apply::discard(&fx.core, None).unwrap();
    assert!(fx.store_file("servers/fake.toml").contains("npx"));
    assert!(
        fx.read(&fx.claude()).contains("\"fake\""),
        "the applied entry stays"
    );
}

/// The temp root's file system with one hook: the first temp file written
/// into `dir` (the swap of a manifest) runs `hook` first, standing in for
/// another program that edits the manifests at that moment.
struct HookFs<F: Fs> {
    inner: F,
    dir: PathBuf,
    hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl<F: Fs> HookFs<F> {
    fn new(inner: F, dir: &Path, hook: impl FnOnce() + Send + 'static) -> Self {
        Self {
            inner,
            dir: dir.to_path_buf(),
            hook: Mutex::new(Some(Box::new(hook))),
        }
    }
}

impl<F: Fs> Fs for HookFs<F> {
    fn root(&self) -> Option<&Path> {
        self.inner.root()
    }
    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }
    fn is_dir(&self, path: &Path) -> bool {
        self.inner.is_dir(path)
    }
    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        self.inner.read_to_string(path)
    }
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.inner.read(path)
    }
    fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        self.inner.write(path, contents)
    }
    fn write_new(&self, path: &Path, contents: &[u8], like: Option<&Path>) -> std::io::Result<()> {
        if path.parent() == Some(self.dir.as_path()) {
            let hook = self.hook.lock().unwrap().take();
            if let Some(hook) = hook {
                hook();
            }
        }
        self.inner.write_new(path, contents, like)
    }
    fn replace(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        self.inner.replace(from, to)
    }
    fn write_private(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        self.inner.write_private(path, contents)
    }
    fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
        self.inner.create_dir_all(path)
    }
    fn create_private_dir_all(&self, path: &Path) -> std::io::Result<()> {
        self.inner.create_private_dir_all(path)
    }
    fn remove_file(&self, path: &Path) -> std::io::Result<()> {
        self.inner.remove_file(path)
    }
    fn remove_dir_all(&self, path: &Path) -> std::io::Result<()> {
        self.inner.remove_dir_all(path)
    }
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<PathBuf>> {
        self.inner.read_dir(path)
    }
    fn open_lock_file(&self, path: &Path) -> std::io::Result<std::fs::File> {
        self.inner.open_lock_file(path)
    }
    fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.inner.canonicalize(path)
    }
}

fn trust_state(fx: &Fx, root: &Path) -> TrustState {
    let project = Project::load(fx.core.fs.as_ref(), root).unwrap().unwrap();
    TrustStore::load(fx.core.fs.as_ref(), &fx.core.home.state_dir())
        .unwrap()
        .state(&project)
}

fn docs_table() -> ProjectServer {
    ProjectServer {
        body: ServerBody {
            transport: Some(Transport::Stdio),
            command: Some("docs-mcp".to_owned()),
            ..ServerBody::default()
        },
        clients: Some(ClientSelection::List(vec!["claude-code".to_owned()])),
        ..ProjectServer::default()
    }
}

/// Review follow-up 1: a change another program makes while an edit is
/// written is never trusted or applied, and is never overwritten.
#[test]
fn an_edit_never_trusts_or_overwrites_a_concurrent_change() {
    let fx = Fx::new();
    let (root, id) = shared_project(&fx);
    let dir = root.join(".loadout");
    let shared = dir.join("project.toml");
    let local = dir.join("project.local.toml");
    let listed = list(&fx, &id);
    project_servers::save(
        &fx.core,
        &id,
        &listed.project.hash,
        &define("mine", ManifestLayerKind::Local, "mine-mcp"),
        true,
    )
    .unwrap();
    let mcp = root.join(".mcp.json");

    // The other manifest changes, then the same one.
    for same_file in [false, true] {
        let hash = list(&fx, &id).project.hash;
        let shared_before = fx.read(&shared);
        let local_before = fx.read(&local);
        let target = if same_file {
            shared.clone()
        } else {
            local.clone()
        };
        let sneaked = format!(
            "{}\n[servers.sneaky]\ntransport = \"stdio\"\ncommand = \"sneaky-mcp\"\n",
            fx.read(&target)
        );
        let (hook_target, hook_text) = (target.clone(), sneaked.clone());
        let hooked = HookFs::new(fx.th.fs(), &dir, move || {
            std::fs::write(&hook_target, &hook_text).unwrap();
        });
        let ax = agent_loadout_core::apply::ApplyEnv {
            fs: &hooked,
            ..fx.core.apply_env()
        };
        let before = fx.commits();
        let store = fx.store();
        let err = edit_server(
            &store,
            &ax,
            &root,
            &hash,
            EntryLayer::Shared,
            "docs",
            ServerEdit::Set {
                server: Box::new(docs_table()),
                create: true,
            },
        )
        .unwrap_err();
        drop(store);
        assert_eq!(err.code(), ErrorCode::TrustRequired, "{}", err.message());
        assert_eq!(fx.commits(), before, "nothing committed");
        // The other program's change is on disk as it wrote it; this edit
        // is not.
        assert_eq!(fx.read(&target), sneaked, "the concurrent change is kept");
        if same_file {
            assert_eq!(fx.read(&local), local_before);
        } else {
            assert_eq!(fx.read(&shared), shared_before, "the edit is put back");
        }
        assert!(!fx.read(&shared).contains("[servers.docs]"));
        // Not trusted, not applied.
        assert_eq!(trust_state(&fx, &root), TrustState::Changed);
        assert!(!fx.read(&mcp).contains("sneaky-mcp"));
        assert!(!fx.read(&fx.claude()).contains("sneaky-mcp"));

        // The folder goes back to the trusted content for the next round.
        let restored = if same_file {
            &shared_before
        } else {
            &local_before
        };
        std::fs::write(&target, restored).unwrap();
        assert_eq!(trust_state(&fx, &root), TrustState::Trusted);
    }
}

/// Review follow-up (also): a `.gitignore` the failed edit appended to is
/// put back byte for byte.
#[test]
fn a_failed_edit_puts_back_an_appended_gitignore() {
    let fx = Fx::new();
    let (root, id) = shared_project(&fx);
    let dir = root.join(".loadout");
    let ignore = dir.join(".gitignore");
    // A .gitignore without the private file's line (the user edited it).
    std::fs::write(&ignore, "# team rules\n").unwrap();
    let hash = list(&fx, &id).project.hash;
    let shared = dir.join("project.toml");
    let hooked = HookFs::new(fx.th.fs(), &dir, move || {
        let text = std::fs::read_to_string(&shared).unwrap();
        std::fs::write(&shared, format!("{text}\n# pulled\n")).unwrap();
    });
    let ax = agent_loadout_core::apply::ApplyEnv {
        fs: &hooked,
        ..fx.core.apply_env()
    };
    let store = fx.store();
    let err = edit_server(
        &store,
        &ax,
        &root,
        &hash,
        EntryLayer::Local,
        "docs",
        ServerEdit::Set {
            server: Box::new(docs_table()),
            create: true,
        },
    )
    .unwrap_err();
    drop(store);
    assert_eq!(err.code(), ErrorCode::TrustRequired, "{}", err.message());
    assert_eq!(fx.read(&ignore), "# team rules\n");
    assert!(!dir.join("project.local.toml").exists());
}

/// Review follow-up 2: the row carries the manifest's client list as
/// written, not this machine's pill selection (which drops opted-out
/// clients), so a toggle never writes the opt-out into project.toml.
#[test]
fn rows_carry_the_manifest_clients_not_this_machines_selection() {
    let fx = Fx::new();
    let (_, id) = shared_project(&fx);
    let listed = list(&fx, &id);
    let mut docs = define("docs", ManifestLayerKind::Shared, "docs-mcp");
    docs.clients = only(&["claude-code", "cursor"]);
    project_servers::save(&fx.core, &id, &listed.project.hash, &docs, true).unwrap();
    // This machine opts out of Cursor.
    fx.stage(&[("clients.toml", "schema = 1\n\n[cursor]\nenabled = false\n")]);
    let listed = list(&fx, &id);
    let row = &listed.servers[0];
    assert_eq!(row.clients.clients, ["claude-code", "cursor"]);
    assert!(!row.clients.all_detected);
    assert!(!row.uses_defaults);
    assert!(pill(&listed, "docs", "claude-code").0);
    assert!(!pill(&listed, "docs", "cursor").0, "opted out here");
}

/// Review follow-up 3: saving a partial local table unchanged writes
/// nothing (no `transport = "stdio"` over the shared http entry).
#[test]
fn saving_an_unchanged_partial_local_table_changes_nothing() {
    let fx = Fx::new();
    let (root, id) = shared_project(&fx);
    let dir = root.join(".loadout");
    let shared = dir.join("project.toml");
    std::fs::write(
        &shared,
        format!(
            "{}\n[servers.api]\ntransport = \"http\"\nurl = \"https://example.invalid/mcp\"\n",
            fx.read(&shared)
        ),
    )
    .unwrap();
    let local = dir.join("project.local.toml");
    let local_text = "schema = 1\n\n[servers.api]\nclients = [\"claude-code\"]\n";
    std::fs::write(&local, local_text).unwrap();
    let review = projects::review(&fx.core, &id).unwrap();
    projects::trust(&fx.core, &id, &review.hash).unwrap();

    let listed = list(&fx, &id);
    let read = project_servers::get(&fx.core, &id, "api", ManifestLayerKind::Local).unwrap();
    assert!(read.over_shared);
    assert_eq!(read.transport, None);
    let before = fx.commits();
    let saved = project_servers::save(&fx.core, &id, &listed.project.hash, &read, false).unwrap();
    assert!(saved.commit.is_none(), "nothing to save");
    assert_eq!(fx.commits(), before);
    assert_eq!(fx.read(&local), local_text);
    let row = list(&fx, &id)
        .servers
        .into_iter()
        .find(|row| row.name == "api")
        .unwrap();
    assert_eq!(row.transport, Some(TransportKind::Http));
}

/// Review follow-up 5: a commit that lands after the pending changes were
/// read stops the discard; nothing is thrown away.
#[test]
fn discard_refuses_a_store_that_moved_on() {
    let fx = Fx::new();
    servers::save(&fx.core, &draft("fake", &["claude-code"]), true).unwrap();
    review_mode(&fx);
    let mut changed = draft("fake", &["claude-code"]);
    changed.args = vec!["--seen".to_owned()];
    servers::save(&fx.core, &changed, false).unwrap();
    let ax = fx.core.apply_env();
    let store = fx.store();
    let pending = discard::pending(&store, &ax).unwrap().unwrap();
    assert_eq!(pending.changes.len(), 1);
    drop(store);

    // Another edit lands before the discard runs.
    servers::save(&fx.core, &draft("late", &["claude-code"]), true).unwrap();
    let before = fx.commits();
    let store = fx.store();
    let err = discard::discard(&store, &ax, &pending, None).unwrap_err();
    drop(store);
    assert_eq!(err.code(), ErrorCode::FileChangedDuringApply);
    assert_eq!(fx.commits(), before, "nothing discarded");
    assert!(fx.store_file("servers/fake.toml").contains("--seen"));
    assert!(fx.store_file("servers/late.toml").contains("late"));
}

//! The project layer (SPEC 6.2): discovery, merge, `from`, `unset`, trust
//! and the git helpers of 6.5.

mod support;

use std::collections::BTreeMap;
use std::path::Path;

use agent_loadout_core::ErrorCode;
use agent_loadout_core::adapters::adapter;
use agent_loadout_core::adapters::caps::ProjectDisable;
use agent_loadout_core::layering::{
    EntryBody, EntryLayer, LOCAL_FILE, Project, ProjectRepo, SHARED_FILE, TrustState, TrustStore,
    content_hash, deep_merge, discover, path_key, project_entries, remove_path,
    suppression_mechanism,
};
use agent_loadout_core::model::clients::ClientSelection;
use agent_loadout_core::model::machine::MachineOverlay;
use agent_loadout_core::model::server::{Server, ServerOverride, Transport};
use support::{Fx, git_commit, git_init, host_os, stdio};

fn value(text: &str) -> toml::Value {
    toml::Value::Table(toml::from_str(text).unwrap())
}

fn load(fx: &Fx, root: &Path) -> Project {
    Project::load(&fx.fs, root).unwrap().unwrap()
}

/// The resolved server of entry `name`.
fn server<'a>(entries: &'a [agent_loadout_core::layering::ProjectEntry], name: &str) -> &'a Server {
    let entry = entries.iter().find(|entry| entry.name == name).unwrap();
    match &entry.body {
        EntryBody::Server(Ok(server)) => server,
        other => panic!("{name}: {other:#?}"),
    }
}

fn env_of(server: &Server) -> Vec<(&str, &str)> {
    server
        .body
        .env
        .iter()
        .flatten()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect()
}

#[test]
fn deep_merge_merges_tables_and_replaces_the_rest() {
    let mut base = value(
        "command = \"a\"\nargs = [\"1\", \"2\"]\n[env]\nX = \"1\"\nY = \"2\"\n[client.omp]\ntimeout = 5\n",
    );
    deep_merge(
        &mut base,
        &value("args = [\"3\"]\n[env]\nY = \"two\"\nZ = \"3\"\n[client.omp]\nkind = \"x\"\n"),
    );
    assert_eq!(
        base,
        value(
            "command = \"a\"\nargs = [\"3\"]\n[env]\nX = \"1\"\nY = \"two\"\nZ = \"3\"\n\
             [client.omp]\ntimeout = 5\nkind = \"x\"\n"
        )
    );
    assert!(remove_path(&mut base, "env.X"));
    assert!(!remove_path(&mut base, "env.NOPE"));
    assert!(!remove_path(&mut base, "headers.X"));
    assert!(base["env"].get("X").is_none());
}

#[test]
fn local_merges_over_shared_and_unset_removes_inherited_keys() {
    let fx = Fx::new();
    let root = fx.th.projects().join("merge");
    fx.manifest(
        &root,
        SHARED_FILE,
        "schema = 1\nid = \"merge-a1b2\"\n\n[defaults]\nclients = [\"claude-code\"]\n\n\
         [servers.db]\ntransport = \"stdio\"\ncommand = \"db-mcp\"\nargs = [\"--ro\", \"--x\"]\n\
         [servers.db.env]\nURL = \"${env:DB}\"\nDEBUG = \"1\"\n\n\
         [servers.shared-only]\ntransport = \"http\"\nurl = \"https://example.test/mcp\"\n",
    );
    fx.manifest(
        &root,
        LOCAL_FILE,
        "schema = 1\n\n[defaults]\nclients = [\"omp\"]\n\n[servers.db]\nargs = [\"--rw\"]\n\
         unset = [\"env.DEBUG\"]\n[servers.db.env]\nURL = \"${env:MY_DB}\"\n",
    );
    let project = load(&fx, &root);
    assert_eq!(project.id, "merge-a1b2");
    let entries = project_entries(&project, &BTreeMap::new(), &fx.env, None);

    let db = entries.iter().find(|entry| entry.name == "db").unwrap();
    assert_eq!(db.layer, EntryLayer::Local);
    // The local `[defaults]` wins.
    assert_eq!(db.clients, ClientSelection::List(vec!["omp".to_owned()]));
    assert_eq!(db.unset, vec!["env.DEBUG".to_owned()]);
    let db = server(&entries, "db");
    assert_eq!(db.id, "db");
    assert_eq!(db.body.command.as_deref(), Some("db-mcp"));
    // Arrays replace; tables merge; `unset` runs last.
    assert_eq!(db.body.args, Some(vec!["--rw".to_owned()]));
    assert_eq!(env_of(db), vec![("URL", "${env:MY_DB}")]);
    // The shared file's own definition, for clients that hold the merged
    // entry privately.
    let entry = entries.iter().find(|entry| entry.name == "db").unwrap();
    let shared = entry.shared.as_deref().unwrap();
    assert_eq!(shared.layer, EntryLayer::Shared);
    assert_eq!(
        shared.clients,
        ClientSelection::List(vec!["claude-code".to_owned()])
    );
    let EntryBody::Server(Ok(shared)) = &shared.body else {
        panic!("{shared:#?}");
    };
    assert_eq!(
        shared.body.args,
        Some(vec!["--ro".to_owned(), "--x".to_owned()])
    );
    assert_eq!(env_of(shared), vec![("DEBUG", "1"), ("URL", "${env:DB}")]);

    let shared = entries
        .iter()
        .find(|entry| entry.name == "shared-only")
        .unwrap();
    assert_eq!(shared.layer, EntryLayer::Shared);
    assert_eq!(
        server(&entries, "shared-only").body.transport,
        Some(Transport::Http)
    );
}

#[test]
fn from_starts_at_the_store_server_after_os_overrides_and_the_overlay() {
    let fx = Fx::new();
    let mut base = stdio("api", &[], &["serve"]);
    base.body.env = Some(
        [
            ("TOKEN", "${secret:API}"),
            ("MODE", "store"),
            ("DEBUG", "1"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect(),
    );
    let mut os_override = ServerOverride::default();
    os_override.fields.command = Some("api-os".to_owned());
    base.body.os = Some(BTreeMap::from([(
        host_os().as_str().to_owned(),
        os_override,
    )]));
    let mut overlay = MachineOverlay::new("01J0000000000000000000000A", "desk", host_os());
    let mut machine = ServerOverride::default();
    machine.fields.env = Some(BTreeMap::from([("MODE".to_owned(), "machine".to_owned())]));
    overlay.servers = Some(BTreeMap::from([("api".to_owned(), machine)]));
    let servers = BTreeMap::from([("api".to_owned(), base)]);

    let root = fx.th.projects().join("from");
    fx.manifest(
        &root,
        SHARED_FILE,
        "schema = 1\nid = \"from-a1b2\"\n\n[servers.api]\nfrom = \"api\"\nunset = [\"env.DEBUG\"]\n\
         [servers.api.env]\nEXTRA = \"yes\"\n\n[servers.ghost]\nfrom = \"missing\"\n\n\
         [servers.slack]\nenabled = false\n",
    );
    let project = load(&fx, &root);
    let entries = project_entries(&project, &servers, &fx.env, Some(&overlay));
    let api = server(&entries, "api");
    assert_eq!(api.body.command.as_deref(), Some("api-os"));
    assert!(api.body.os.is_none());
    assert_eq!(
        env_of(api),
        vec![
            ("EXTRA", "yes"),
            ("MODE", "machine"),
            ("TOKEN", "${secret:API}")
        ]
    );
    assert!(api.enable.is_none());

    let ghost = entries.iter().find(|entry| entry.name == "ghost").unwrap();
    let EntryBody::Server(Err(err)) = &ghost.body else {
        panic!("{ghost:#?}");
    };
    assert_eq!(err.code(), ErrorCode::UnknownTemplate);

    let slack = entries.iter().find(|entry| entry.name == "slack").unwrap();
    assert_eq!(slack.body, EntryBody::Suppression);
    assert_eq!(slack.clients, ClientSelection::AllDetected);
}

#[test]
fn an_incomplete_definition_blocks_its_entry() {
    let fx = Fx::new();
    let root = fx.th.projects().join("partial");
    fx.manifest(
        &root,
        SHARED_FILE,
        "schema = 1\nid = \"partial-a1b2\"\n\n[servers.half]\ntransport = \"stdio\"\ncommand = \"x\"\n",
    );
    // The local layer removes what makes it complete.
    fx.manifest(
        &root,
        LOCAL_FILE,
        "schema = 1\n\n[servers.half]\nunset = [\"command\"]\n",
    );
    let entries = project_entries(&load(&fx, &root), &BTreeMap::new(), &fx.env, None);
    let EntryBody::Server(Err(err)) = &entries[0].body else {
        panic!("{entries:#?}");
    };
    assert_eq!(
        err.path(),
        Some(root.join(".loadout").join(LOCAL_FILE).as_path())
    );
}

#[test]
fn discovery_finds_the_nearest_manifest_and_ignores_the_home_dir() {
    let fx = Fx::new();
    let outer = fx.th.projects().join("outer");
    let inner = outer.join("packages").join("inner");
    fx.manifest(&outer, SHARED_FILE, "schema = 1\nid = \"outer-a1b2\"\n");
    fx.manifest(&inner, LOCAL_FILE, "schema = 1\nid = \"inner-a1b2\"\n");
    let deep = inner.join("src").join("lib");
    std::fs::create_dir_all(&deep).unwrap();
    assert_eq!(discover(&fx.env, &fx.fs, &deep), Some(inner.clone()));
    assert_eq!(
        discover(&fx.env, &fx.fs, &outer.join("packages")),
        Some(outer.clone())
    );
    let home = fx.th.home();
    fx.manifest(&home, SHARED_FILE, "schema = 1\nid = \"home-a1b2\"\n");
    let plain = home.join("code");
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(discover(&fx.env, &fx.fs, &plain), None);
    assert_eq!(discover(&fx.env, &fx.fs, &home), None);
}

#[test]
fn trust_is_keyed_by_folder_and_content_hash() {
    let fx = Fx::new();
    assert_eq!(
        content_hash(None, None),
        content_hash(None, None),
        "the hash is deterministic"
    );
    assert_ne!(
        content_hash(Some(b"a"), None),
        content_hash(None, Some(b"a"))
    );
    assert!(content_hash(None, None).starts_with("sha256:"));

    let root = fx.th.projects().join("trusted");
    fx.manifest(&root, SHARED_FILE, "schema = 1\nid = \"trusted-a1b2\"\n");
    let project = load(&fx, &root);
    assert_eq!(
        project.hash(),
        content_hash(Some(b"schema = 1\nid = \"trusted-a1b2\"\n"), None)
    );

    let state = fx.home.state_dir();
    let mut store = TrustStore::load(&fx.fs, &state).unwrap();
    assert_eq!(store.state(&project), TrustState::Untrusted);
    store.trust(&project);
    store.save(&fx.fs, &state).unwrap();
    let store = TrustStore::load(&fx.fs, &state).unwrap();
    assert_eq!(store.state(&project), TrustState::Trusted);
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state.join("trust.json")).unwrap()).unwrap();
    assert_eq!(saved["schema"], 1);
    assert_eq!(saved["projects"][0]["id"], "trusted-a1b2");

    // Adding the local file changes the hash.
    fx.manifest(&root, LOCAL_FILE, "schema = 1\n");
    let changed = load(&fx, &root);
    assert_eq!(store.state(&changed), TrustState::Changed);

    let mut store = store;
    store.trust(&changed);
    assert_eq!(store.projects.len(), 1);
    assert_eq!(store.state(&changed), TrustState::Trusted);
    assert!(store.forget("trusted-a1b2"));
    assert_eq!(store.state(&changed), TrustState::Untrusted);

    // Found by folder however the path is spelled.
    store.trust(&changed);
    let mut respelled = changed.clone();
    let text = root.to_string_lossy().into_owned();
    respelled.root = if cfg!(windows) {
        format!(r"\\?\{}\", text.to_uppercase()).into()
    } else {
        format!("{text}/").into()
    };
    assert_eq!(store.state(&respelled), TrustState::Trusted);
    assert_eq!(path_key(&respelled.root), path_key(&root));
    store.trust(&respelled);
    assert_eq!(store.projects.len(), 1);
}

#[test]
fn git_helpers_track_ignore_and_exclude() {
    let fx = Fx::new();
    let root = fx.th.projects().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    assert!(ProjectRepo::discover(&root).unwrap().is_none());
    let repo = git_init(&root);
    fx.write(&root.join(".mcp.json"), "{}\n");
    git_commit(&repo, ".mcp.json");
    let project = ProjectRepo::discover(&root).unwrap().unwrap();
    let omp = root.join(".omp").join("mcp.json");
    assert!(project.is_tracked(&root.join(".mcp.json")).unwrap());
    assert!(!project.is_tracked(&omp).unwrap());
    assert!(!project.is_ignored(&omp).unwrap());
    assert_eq!(project.relative(&omp).as_deref(), Some(".omp/mcp.json"));

    assert!(project.exclude(&fx.fs, &omp).unwrap());
    assert!(project.is_ignored(&omp).unwrap());
    // Idempotent.
    assert!(!project.exclude(&fx.fs, &omp).unwrap());
    let text = std::fs::read_to_string(repo.path().join("info").join("exclude")).unwrap();
    assert_eq!(text.matches("/.omp/mcp.json").count(), 1, "{text}");
    assert_eq!(
        project.exclude_file(),
        repo.path().join("info").join("exclude")
    );
}

#[test]
fn a_linked_worktree_excludes_through_the_common_dir() {
    let fx = Fx::new();
    let root = fx.th.projects().join("main");
    std::fs::create_dir_all(&root).unwrap();
    let repo = git_init(&root);
    fx.write(&root.join("README"), "x\n");
    git_commit(&repo, "README");
    let tree = fx.th.projects().join("tree");
    repo.worktree("tree", &tree, None).unwrap();

    let project = ProjectRepo::discover(&tree).unwrap().unwrap();
    let common = repo.path().join("info").join("exclude");
    assert_eq!(
        path_key(&project.exclude_file()),
        path_key(&common),
        "{:?}",
        project.exclude_file()
    );
    let omp = tree.join(".omp").join("mcp.json");
    assert!(project.exclude(&fx.fs, &omp).unwrap());
    assert!(!project.exclude(&fx.fs, &omp).unwrap());
    let text = std::fs::read_to_string(&common).unwrap();
    assert_eq!(text.matches("/.omp/mcp.json").count(), 1, "{text}");
    assert!(
        ProjectRepo::discover(&tree)
            .unwrap()
            .unwrap()
            .is_ignored(&omp)
            .unwrap()
    );
}

#[test]
fn clients_without_a_project_mechanism_cannot_suppress() {
    let desktop = adapter("claude-desktop").unwrap();
    let err = suppression_mechanism(desktop, "slack").unwrap_err();
    assert_eq!(err.code(), ErrorCode::CannotSuppressInProject);
    assert!(
        err.message()
            .contains("remove claude-desktop from the system enable list")
    );
    assert!(matches!(
        suppression_mechanism(adapter("claude-code").unwrap(), "slack").unwrap(),
        ProjectDisable::ListMember(_)
    ));
    assert!(matches!(
        suppression_mechanism(adapter("codex").unwrap(), "slack").unwrap(),
        ProjectDisable::EnabledFalse { partial: true }
    ));
}

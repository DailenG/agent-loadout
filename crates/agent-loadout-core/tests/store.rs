//! Store, transactions, trailers, history and the cross-process lock
//! (SPEC 4.1, 9.1 to 9.3). Every store lives in a `TempHome`; file access
//! goes through `RootedFs`.

use std::path::Path;
use std::time::{Duration, Instant};

use agent_loadout_core::model::applied::ApplyOutcome;
use agent_loadout_core::model::ids::{self, IdKind};
use agent_loadout_core::model::server::{Server, Transport};
use agent_loadout_core::paths::{LoadoutHome, loadout_home};
use agent_loadout_core::store::{
    ApplyRef, LogFilter, MachineRef, Operation, Scope, Store, StoreLock, Trailers, compose_message,
    message_summary,
};
use agent_loadout_core::{CoreError, Env, ErrorCode, Os};
use agent_loadout_testkit::{RootedFs, TempHome};
use git2::{Repository, Signature, StatusOptions};

/// `TempHome`'s fixed clock: 2026-01-01T00:00:00Z.
const FIXED_SECONDS: i64 = 1_767_225_600;
const CLIENT: &str = "loadout-test/0.0.0";

fn host_os() -> Os {
    if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    }
}

struct Fixture {
    th: TempHome,
    env: Env,
    fs: RootedFs,
    home: LoadoutHome,
}

impl Fixture {
    fn new() -> Self {
        let th = TempHome::new();
        // Angle brackets and a line break cannot go into a git signature.
        let env = th.env_builder(host_os()).hostname("test <desk>\n").build();
        let fs = th.fs();
        let home = loadout_home(&env, &fs, Some(&th.root().join("lh"))).unwrap();
        Self { th, env, fs, home }
    }

    fn store(&self) -> Store<'_> {
        Store::open_or_init(&self.env, &self.fs, &self.home, CLIENT).unwrap()
    }

    /// Inspects the store with git2 directly, as any git client would.
    fn repo(&self) -> Repository {
        Repository::open(self.home.store_dir()).unwrap()
    }

    fn commit_count(&self) -> usize {
        let repo = self.repo();
        let mut walk = repo.revwalk().unwrap();
        walk.push_head().unwrap();
        walk.count()
    }

    fn assert_clean(&self) {
        let repo = self.repo();
        let mut options = StatusOptions::new();
        options.include_untracked(true);
        let statuses = repo.statuses(Some(&mut options)).unwrap();
        let dirty: Vec<String> = statuses
            .iter()
            .map(|entry| format!("{:?} {}", entry.status(), entry.path().unwrap_or("")))
            .collect();
        assert!(dirty.is_empty(), "store is not clean: {dirty:?}");
    }
}

fn stdio_server(id: &str) -> Server {
    let mut server = Server::new(id);
    server.body.transport = Some(Transport::Stdio);
    server.body.command = Some("npx".to_owned());
    server.body.args = Some(vec!["-y".to_owned(), format!("@example/{id}")]);
    server
}

fn add(store: &Store<'_>, id: &str) -> String {
    let operation =
        Operation::new("server.add", format!("server: add {id}")).entity(format!("server/{id}"));
    store
        .transaction(operation, |tx| tx.add_server(stdio_server(id)))
        .unwrap()
        .commit
        .expect("a new server makes a commit")
        .to_string()
}

fn changed_paths(repo: &Repository, commit: &str) -> Vec<String> {
    let commit = repo
        .find_commit(git2::Oid::from_str(commit).unwrap())
        .unwrap();
    let parent = commit.parent(0).unwrap();
    let diff = repo
        .diff_tree_to_tree(
            Some(&parent.tree().unwrap()),
            Some(&commit.tree().unwrap()),
            None,
        )
        .unwrap();
    diff.deltas()
        .map(|delta| {
            delta
                .new_file()
                .path()
                .or(delta.old_file().path())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

#[test]
fn init_creates_main_with_identity_and_one_signed_commit() {
    let fx = Fixture::new();
    assert!(
        Store::open(&fx.env, &fx.fs, &fx.home, CLIENT)
            .unwrap()
            .is_none()
    );
    assert!(!fx.home.path.exists(), "open must not create anything");

    let store = fx.store();
    let store_id = store.store_id().to_owned();
    assert!(ids::is_valid(IdKind::Store, &store_id), "{store_id}");
    assert_eq!(
        store.identity().created_at.to_string(),
        "2026-01-01T00:00:00Z"
    );

    let machine_toml = fx.th.root().join("lh").join("state").join("machine.toml");
    let machine_text = std::fs::read_to_string(&machine_toml).unwrap();
    let machine: toml::Table = machine_text.parse().unwrap();
    let machine_id = machine["machine_id"].as_str().unwrap().to_owned();
    assert!(ids::is_valid(IdKind::Machine, &machine_id));
    assert_eq!(machine["label"].as_str(), Some("test desk"));

    let repo = fx.repo();
    let head = repo.head().unwrap();
    assert_eq!(head.name().unwrap(), "refs/heads/main");
    let commit = head.peel_to_commit().unwrap();
    assert_eq!(commit.parent_count(), 0);
    assert_eq!(
        commit.summary().unwrap(),
        Some(&*format!("init: store {store_id}"))
    );
    for signature in [commit.author(), commit.committer()] {
        assert_eq!(signature.name().unwrap(), "loadout test desk");
        assert_eq!(
            signature.email().unwrap(),
            format!("{machine_id}@loadout.invalid")
        );
        assert_eq!(signature.when().seconds(), FIXED_SECONDS);
    }
    let mut files: Vec<String> = Vec::new();
    commit
        .tree()
        .unwrap()
        .walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
            files.push(format!("{dir}{}", entry.name().unwrap()));
            git2::TreeWalkResult::Ok
        })
        .unwrap();
    files.sort();
    assert_eq!(files, [".gitattributes", "loadout.toml"]);
    let identity = std::fs::read_to_string(store.root().join("loadout.toml")).unwrap();
    assert!(identity.contains(&format!("store_id = \"{store_id}\"")));
    fx.assert_clean();

    let log = store.log(&LogFilter::default()).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].trailers.op.as_deref(), Some("store.init"));
    assert_eq!(log[0].trailers.scope, Some(Scope::System));
    assert_eq!(log[0].trailers.client.as_deref(), Some(CLIENT));
    assert_eq!(log[0].time_rfc3339(), "2026-01-01T00:00:00Z");

    // Reopening finds the same store and commits nothing.
    drop(store);
    assert_eq!(fx.store().store_id(), store_id);
    let opened = Store::open(&fx.env, &fx.fs, &fx.home, CLIENT).unwrap();
    assert_eq!(opened.unwrap().store_id(), store_id);
    assert_eq!(fx.commit_count(), 1);
}

#[test]
fn every_mutation_is_one_commit_and_a_bulk_change_is_one_commit() {
    let fx = Fixture::new();
    let store = fx.store();
    assert_eq!(fx.commit_count(), 1);

    let first = add(&store, "github");
    assert_eq!(fx.commit_count(), 2);
    assert_eq!(store.head().unwrap().to_string(), first);
    assert_eq!(changed_paths(&fx.repo(), &first), ["servers/github.toml"]);
    let on_disk =
        std::fs::read_to_string(store.root().join("servers").join("github.toml")).unwrap();
    assert_eq!(
        store.server("github").unwrap().unwrap().to_text().unwrap(),
        on_disk
    );

    // An import of three servers is one commit naming all three.
    let ids = ["alpha", "beta", "gamma"];
    let operation = ids.iter().fold(
        Operation::new("server.import", "server: import 3 servers"),
        |operation, id| operation.entity(format!("server/{id}")),
    );
    let bulk = store
        .transaction(operation, |tx| {
            ids.iter()
                .try_for_each(|id| tx.add_server(stdio_server(id)))
        })
        .unwrap()
        .commit
        .unwrap()
        .to_string();
    assert_eq!(fx.commit_count(), 3);
    let mut paths = changed_paths(&fx.repo(), &bulk);
    paths.sort();
    assert_eq!(
        paths,
        [
            "servers/alpha.toml",
            "servers/beta.toml",
            "servers/gamma.toml"
        ]
    );
    let log = store.log(&LogFilter::default()).unwrap();
    assert_eq!(
        log[0].trailers.entities,
        ["server/alpha", "server/beta", "server/gamma"]
    );

    // A bulk removal is one commit too.
    let operation = Operation::new("server.remove", "server: remove 2 servers")
        .entity("server/alpha")
        .entity("server/beta");
    let removed = store
        .transaction(operation, |tx| {
            tx.remove_server("alpha")?;
            tx.remove_server("beta")
        })
        .unwrap();
    assert!(removed.commit.is_some());
    assert_eq!(fx.commit_count(), 4);
    let listed: Vec<String> = store
        .servers()
        .unwrap()
        .into_iter()
        .map(|entry| {
            entry.document.unwrap();
            entry.id
        })
        .collect();
    assert_eq!(listed, ["gamma", "github"]);
    fx.assert_clean();
}

#[test]
fn failed_invalid_and_no_op_transactions_commit_nothing() {
    let fx = Fixture::new();
    let store = fx.store();
    add(&store, "github");
    let before = fx.commit_count();
    let server_file = |id: &str| store.root().join("servers").join(format!("{id}.toml"));

    // The closure fails after staging: nothing is written.
    let err = store
        .transaction(Operation::new("server.add", "server: add staged"), |tx| {
            tx.add_server(stdio_server("staged"))?;
            Err::<(), _>(CoreError::new(ErrorCode::InvalidValue, "changed my mind"))
        })
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);
    assert!(!server_file("staged").exists());

    // Validation fails: a stdio server without a command.
    let mut broken = Server::new("broken");
    broken.body.transport = Some(Transport::Stdio);
    let err = store
        .transaction(Operation::new("server.add", "server: add broken"), |tx| {
            tx.add_server(broken)
        })
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::MissingField);
    assert_eq!(err.path(), Some(server_file("broken").as_path()));
    assert!(!server_file("broken").exists());

    // Raw writes are validated too, by path.
    let err = store
        .transaction(Operation::new("server.add", "server: add raw"), |tx| {
            tx.write("servers/raw.toml", "schema = 1\nid = \"other\"\n")
        })
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::IdMismatch);

    let err = store
        .transaction(Operation::new("server.add", "server: add github"), |tx| {
            tx.add_server(stdio_server("github"))
        })
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::AlreadyExists);
    let err = store
        .transaction(
            Operation::new("server.remove", "server: remove nope"),
            |tx| tx.remove_server("nope"),
        )
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);

    // Paths outside the working tree or into .git are refused.
    for path in [
        "../escape.toml",
        ".git/config",
        ".GIT/HEAD",
        "a//b",
        "C:/x",
        r"a\b",
    ] {
        let err = store
            .transaction(Operation::new("test.write", "test: write"), |tx| {
                tx.write(path, "x")
            })
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidPath, "{path}");
    }

    // Writing what is already there, or nothing at all, is not a commit.
    let same = std::fs::read(server_file("github")).unwrap();
    let outcome = store
        .transaction(
            Operation::new("server.update", "server: update github"),
            |tx| tx.write("servers/github.toml", same),
        )
        .unwrap();
    assert_eq!(outcome.commit, None);
    let outcome = store
        .transaction(Operation::new("test.noop", "test: nothing"), |_| Ok(42))
        .unwrap();
    assert_eq!((outcome.value, outcome.commit), (42, None));

    let err = store
        .transaction(Operation::new("Bad Op", "test"), |_| Ok(()))
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);

    assert_eq!(fx.commit_count(), before);
    fx.assert_clean();
}

#[test]
fn trailers_round_trip() {
    let trailers = Trailers {
        op: Some("server.update".to_owned()),
        entities: vec!["server/github".to_owned(), "server/linear".to_owned()],
        scope: Some(Scope::Project("sample-app".to_owned())),
        machine: Some(MachineRef {
            id: "01JCX3R8Q6T5W1V9Y2Z7A4B6C8".to_owned(),
            label: "desk (home) 2".to_owned(),
        }),
        client: Some("loadout-gui/0.1.0".to_owned()),
        apply: Some(ApplyRef {
            id: "01JCX4A0M2N8P6Q4R2S0T8X6V4".to_owned(),
            outcome: ApplyOutcome::Partial,
        }),
        other: vec![("Future".to_owned(), "kept as written".to_owned())],
    };
    let message = compose_message("server: update github (env.GITHUB_TOOLSETS)", &trailers);
    assert_eq!(
        message,
        "server: update github (env.GITHUB_TOOLSETS)\n\
         \n\
         Loadout-Op: server.update\n\
         Loadout-Entity: server/github\n\
         Loadout-Entity: server/linear\n\
         Loadout-Scope: project/sample-app\n\
         Loadout-Machine: 01JCX3R8Q6T5W1V9Y2Z7A4B6C8 (desk (home) 2)\n\
         Loadout-Client: loadout-gui/0.1.0\n\
         Loadout-Apply: 01JCX4A0M2N8P6Q4R2S0T8X6V4 partial\n\
         Loadout-Future: kept as written\n"
    );
    assert_eq!(Trailers::parse(&message), trailers);
    assert_eq!(
        message_summary(&message),
        "server: update github (env.GITHUB_TOOLSETS)"
    );

    // CRLF and foreign trailers in the same paragraph.
    let crlf = message.replace('\n', "\r\n") + "Signed-off-by: someone\r\n";
    assert_eq!(Trailers::parse(&crlf), trailers);

    // Line breaks cannot split a value or forge a trailer.
    let forged = Trailers {
        client: Some("evil\nLoadout-Op: forged".to_owned()),
        ..Trailers::default()
    };
    let parsed = Trailers::parse(&compose_message("summary\nLoadout-Op: x", &forged));
    assert_eq!(parsed.op, None);
    assert_eq!(parsed.client.as_deref(), Some("evil Loadout-Op: forged"));

    // A summary alone, or a last paragraph that is prose, has no trailers.
    assert!(Trailers::parse("Loadout-Op: summary only\n").is_empty());
    assert!(Trailers::parse("summary\n\nLoadout-Op: x\nnot a trailer\n").is_empty());

    // Through a real commit and the log.
    let fx = Fixture::new();
    let store = fx.store();
    let operation = Operation::new("project.update", "project: update sample-app")
        .entity("project/sample-app")
        .scope(Scope::Project("sample-app".to_owned()));
    store
        .transaction(operation, |tx| {
            tx.write("projects/sample-app/notes.md", "hi\n")
        })
        .unwrap();
    let entry = store.log(&LogFilter::default()).unwrap().remove(0);
    let machine_id = entry.author_email.strip_suffix("@loadout.invalid").unwrap();
    assert_eq!(
        entry.trailers,
        Trailers {
            op: Some("project.update".to_owned()),
            entities: vec!["project/sample-app".to_owned()],
            scope: Some(Scope::Project("sample-app".to_owned())),
            machine: Some(MachineRef {
                id: machine_id.to_owned(),
                label: "test desk".to_owned(),
            }),
            client: Some(CLIENT.to_owned()),
            apply: None,
            other: Vec::new(),
        }
    );
    assert_eq!(entry.summary, "project: update sample-app");
}

#[test]
fn history_filters_by_entity_scope_and_limit() {
    let fx = Fixture::new();
    let store = fx.store();
    let add_a = add(&store, "a");
    let add_b = add(&store, "b");
    let project = store
        .transaction(
            Operation::new("project.update", "project: update demo")
                .entity("project/demo")
                .scope(Scope::Project("demo".to_owned())),
            |tx| tx.write("projects/demo/notes.md", "x\n"),
        )
        .unwrap()
        .commit
        .unwrap()
        .to_string();

    // A commit made by another git client: no trailers, found by path.
    let path = store.root().join("servers").join("a.toml");
    let edited = std::fs::read_to_string(&path).unwrap() + "description = \"edited by hand\"\n";
    std::fs::write(&path, edited).unwrap();
    let repo = fx.repo();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("servers/a.toml")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    let when = git2::Time::new(FIXED_SECONDS, 0);
    let hand = Signature::new("someone", "someone@example.invalid", &when).unwrap();
    let by_hand = repo
        .commit(Some("HEAD"), &hand, &hand, "tweak a", &tree, &[&parent])
        .unwrap()
        .to_string();

    let ids = |filter: LogFilter| -> Vec<String> {
        store
            .log(&filter)
            .unwrap()
            .into_iter()
            .map(|entry| entry.id.to_string())
            .collect()
    };
    let entity = |entity: &str| LogFilter {
        entity: Some(entity.to_owned()),
        ..LogFilter::default()
    };
    assert_eq!(ids(entity("server/a")), [by_hand.clone(), add_a]);
    assert_eq!(ids(entity("server/b")), [add_b]);
    assert_eq!(ids(entity("project/demo")), vec![project.clone()]);
    assert!(ids(entity("server/none")).is_empty());
    assert_eq!(
        ids(LogFilter {
            scope: Some(Scope::Project("demo".to_owned())),
            ..LogFilter::default()
        }),
        vec![project.clone()]
    );
    assert_eq!(
        ids(LogFilter {
            limit: Some(2),
            ..LogFilter::default()
        }),
        [by_hand, project]
    );
    assert_eq!(ids(LogFilter::default()).len(), 5);
}

#[test]
fn a_held_lock_makes_others_wait_then_fail_with_locked() {
    let fx = Fixture::new();
    let state = fx.home.state_dir();
    let held = StoreLock::acquire(&fx.fs, &state, Duration::from_secs(1)).unwrap();

    let started = Instant::now();
    let err = StoreLock::acquire(&fx.fs, &state, Duration::from_millis(300)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::Locked);
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert_eq!(err.path(), Some(state.join("loadout.lock").as_path()));

    drop(held);
    StoreLock::acquire(&fx.fs, &state, Duration::from_millis(300)).unwrap();
}

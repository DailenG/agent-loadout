//! History, tags and restore (SPEC 9.3 to 9.5): semantic diff goldens,
//! user and automatic tags with 90-day retention on a fixed clock, store
//! restore (whole, by entity, of a restore, never other machines' files,
//! project manifests written back only when confirmed), and client-file
//! restore from apply backups through the compare-and-swap write.

mod support;

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use agent_loadout_core::adapters::detect::NoRegistry;
use agent_loadout_core::apply::ApplyEnv;
use agent_loadout_core::store::diff::{ChangeKind, render};
use agent_loadout_core::store::restore::{
    self, FileOutcome, RestoreRequest, prepare, restore_files,
};
use agent_loadout_core::store::tags::{AUTO_TAG_RETENTION, TagKind};
use agent_loadout_core::store::{CommitId, Store, StoreRead};
use agent_loadout_core::{ErrorCode, FixedClock, Fs};
use agent_loadout_testkit::RootedFs;
use support::{CLIENT, Fx, host_os, stdio};

const GITHUB_A: &str = r#"schema = 1
id = "github"
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]

[env]
GITHUB_PERSONAL_ACCESS_TOKEN = "${secret:GITHUB_PAT}"
GITHUB_TOOLSETS = "repos"
"#;

const GITHUB_B: &str = r#"schema = 1
id = "github"
tags = ["dev"]
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github@2"]

[env]
GITHUB_PERSONAL_ACCESS_TOKEN = "${secret:GITHUB_PAT}"
GITHUB_TOOLSETS = "repos,issues"
"#;

const OLD: &str = "schema = 1\nid = \"old\"\ntransport = \"stdio\"\ncommand = \"old-mcp\"\n";

const LINEAR: &str = r#"schema = 1
id = "linear"
transport = "http"
url = "https://mcp.example.test/mcp"

[headers]
Authorization = "Bearer ${secret:LINEAR_TOKEN}"
"#;

const SECRETS_A: &str =
    "schema = 1\n\n[secrets.GITHUB_PAT]\nref = \"op://Example Vault/Fake PAT/credential\"\n";
const SECRETS_B: &str =
    "schema = 1\n\n[secrets.GITHUB_PAT]\nref = \"keyring://loadout/GITHUB_PAT\"\n";

/// Commit A, then commit B; returns both ids.
fn two_commits(fx: &Fx) -> (CommitId, CommitId) {
    let a = fx
        .edit("a", |tx| {
            tx.write("servers/github.toml", GITHUB_A)?;
            tx.write("servers/old.toml", OLD)?;
            tx.write("secrets.toml", SECRETS_A)?;
            tx.write("skills/review/SKILL.md", "# Review\n\nRead the diff.\n")
        })
        .unwrap();
    let b = fx
        .edit("b", |tx| {
            tx.write("servers/github.toml", GITHUB_B)?;
            tx.remove("servers/old.toml")?;
            tx.write("servers/linear.toml", LINEAR)?;
            tx.write("secrets.toml", SECRETS_B)?;
            tx.write(
                "skills/review/SKILL.md",
                "# Review\n\nRead the whole diff.\n",
            )
        })
        .unwrap();
    (a, b)
}

/// Every store file at `id` except apply records, with its bytes.
fn tree(store: &Store<'_>, id: &CommitId) -> Vec<(String, Vec<u8>)> {
    let revision = store.revision(id).unwrap();
    revision
        .files()
        .unwrap()
        .into_iter()
        .filter(|file| !file.ends_with(".applied.toml"))
        .map(|file| {
            let bytes = revision.read_bytes(&file).unwrap().unwrap();
            (file, bytes)
        })
        .collect()
}

fn request(target: &CommitId) -> RestoreRequest {
    RestoreRequest {
        target: target.clone(),
        entities: Vec::new(),
        confirmed: false,
        apply: false,
    }
}

#[test]
fn semantic_diff_golden() {
    let fx = Fx::new();
    let (a, b) = two_commits(&fx);
    let store = fx.store();
    let changes = store.semantic_diff(Some(&a), &b).unwrap();
    let kinds: Vec<(&str, ChangeKind)> = changes
        .iter()
        .map(|change| (change.entity.as_str(), change.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("secrets", ChangeKind::Changed),
            ("server/github", ChangeKind::Changed),
            ("server/linear", ChangeKind::Added),
            ("server/old", ChangeKind::Removed),
            ("skill/review", ChangeKind::Changed),
        ]
    );
    insta::assert_snapshot!("semantic_a_to_b", render(&changes));
    // The other way round, and from the empty store.
    insta::assert_snapshot!(
        "semantic_b_to_a",
        render(&store.semantic_diff(Some(&b), &a).unwrap())
    );
    let first = store.semantic_diff(None, &a).unwrap();
    assert!(first.iter().all(|change| change.kind == ChangeKind::Added));

    let raw = store.raw_diff(Some(&a), &b).unwrap();
    assert!(raw.contains("-GITHUB_TOOLSETS = \"repos\""), "{raw}");
    assert!(raw.contains("+GITHUB_TOOLSETS = \"repos,issues\""), "{raw}");
    assert!(store.semantic_diff(Some(&b), &b).unwrap().is_empty());
}

#[test]
fn user_tags_are_annotated_and_never_move() {
    let fx = Fx::new();
    let (a, b) = two_commits(&fx);
    let store = fx.store();
    let tag = store
        .create_user_tag("pre-reset", &a, Some("before the reset"))
        .unwrap();
    assert_eq!(tag.kind, TagKind::User);
    assert_eq!(tag.target, a);
    assert_eq!(tag.message.as_deref(), Some("before the reset"));
    assert_eq!(tag.tagger.as_deref(), Some("loadout test-desk"));
    assert_eq!(store.resolve("pre-reset").unwrap(), a);

    let err = store.create_user_tag("pre-reset", &b, None).unwrap_err();
    assert_eq!(err.code(), ErrorCode::AlreadyExists);
    for bad in [
        "",
        "auto/x",
        "has space",
        "..",
        ".hidden",
        "x.lock",
        &"a".repeat(65),
    ] {
        let err = store.create_user_tag(bad, &b, None).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidValue, "{bad:?}");
    }
    store.create_user_tag("v1.0_rc-2", &b, None).unwrap();
    let names: Vec<String> = store.tags().unwrap().into_iter().map(|t| t.name).collect();
    assert_eq!(names, ["pre-reset", "v1.0_rc-2"]);

    store.delete_tag("pre-reset").unwrap();
    assert_eq!(
        store.delete_tag("pre-reset").unwrap_err().code(),
        ErrorCode::NotFound
    );
}

#[test]
fn auto_tags_are_pruned_after_90_days() {
    let fx = Fx::new();
    two_commits(&fx);
    let start = fx.env.now();
    let fs = fx.th.fs();
    let env_at = |days: u64| {
        fx.th
            .env_builder(host_os())
            .hostname("test-desk")
            .clock(FixedClock(start + Duration::from_secs(days * 86_400)))
            .build()
    };

    let first = fx.store().create_auto_tag("pre-restore").unwrap();
    assert_eq!(first, "auto/pre-restore/20260101T000000Z");
    let again = fx.store().create_auto_tag("pre-restore").unwrap();
    assert_eq!(again, "auto/pre-restore/20260101T000000Z-2");
    fx.store()
        .create_user_tag("keep-me", &fx.store().head().unwrap(), None)
        .unwrap();

    let env = env_at(60);
    let store = Store::open(&env, &fs, &fx.home, CLIENT).unwrap().unwrap();
    let middle = store.create_auto_tag("pre-pull").unwrap();
    let names = |store: &Store<'_>| -> Vec<String> {
        store.tags().unwrap().into_iter().map(|t| t.name).collect()
    };
    assert_eq!(names(&store).len(), 4, "nothing is 90 days old yet");

    let env = env_at(91);
    let store = Store::open(&env, &fs, &fx.home, CLIENT).unwrap().unwrap();
    let last = store.create_auto_tag("pre-rollback").unwrap();
    assert_eq!(
        names(&store),
        vec!["keep-me".to_owned(), middle.clone(), last]
    );
    let middle = store.tag(&middle).unwrap().unwrap();
    assert_eq!(middle.kind, TagKind::Auto);
    assert_eq!(middle.time, Some(start + Duration::from_secs(60 * 86_400)));
    assert_eq!(
        store
            .prune_auto_tags(SystemTime::UNIX_EPOCH, AUTO_TAG_RETENTION)
            .unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn restore_of_a_restore_returns_the_original_tree() {
    let fx = Fx::new();
    let (a, b) = two_commits(&fx);
    let store = fx.store();
    let report = restore::restore(&store, &fx.ax(), &request(&a)).unwrap();
    let tag = report.tag.clone().unwrap();
    assert!(tag.starts_with("auto/pre-restore/"), "{tag}");
    assert_eq!(store.resolve(&tag).unwrap(), b);
    let restored = report.commit.clone().unwrap();
    assert_eq!(tree(&store, &restored), tree(&store, &a));
    // History is kept: B is the parent of the restore.
    assert_eq!(store.first_parent(&restored).unwrap(), Some(b.clone()));
    let (entry, _) = store.entry(&restored).unwrap();
    assert_eq!(entry.trailers.op.as_deref(), Some("store.restore"));
    assert!(
        entry
            .trailers
            .entities
            .contains(&"server/github".to_owned())
    );
    // The working tree follows.
    assert_eq!(
        fx.read(&fx.home.store_dir().join("servers/github.toml")),
        GITHUB_A
    );

    let target = store.resolve(&tag).unwrap();
    let again = restore::restore(&store, &fx.ax(), &request(&target)).unwrap();
    assert_eq!(tree(&store, &again.commit.unwrap()), tree(&store, &b));

    // Nothing to restore: no tag, no commit.
    let head = store.head().unwrap();
    let none = restore::restore(&store, &fx.ax(), &request(&head)).unwrap();
    assert!(none.tag.is_none() && none.commit.is_none());
}

#[test]
fn partial_restore_by_entity_leaves_the_rest() {
    let fx = Fx::new();
    let (a, b) = two_commits(&fx);
    let store = fx.store();
    let mut partial = request(&a);
    partial.entities = vec!["server/github".to_owned(), "secrets".to_owned()];
    let report = restore::restore(&store, &fx.ax(), &partial).unwrap();
    assert_eq!(report.plan.files, ["secrets.toml", "servers/github.toml"]);
    let head = report.commit.unwrap();
    let at_head = store.revision(&head).unwrap();
    let at_b = store.revision(&b).unwrap();
    assert_eq!(
        at_head.read_bytes("servers/github.toml").unwrap().unwrap(),
        GITHUB_A.as_bytes()
    );
    for kept in ["servers/linear.toml", "skills/review/SKILL.md"] {
        assert_eq!(
            at_head.read_bytes(kept).unwrap(),
            at_b.read_bytes(kept).unwrap()
        );
    }
    assert!(at_head.read_bytes("servers/old.toml").unwrap().is_none());

    partial.entities = vec!["nonsense".to_owned()];
    assert_eq!(
        prepare(&store, &fx.machine_id(), &partial)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidValue
    );
}

#[test]
fn restore_never_touches_other_machines_files() {
    const OTHER: &str = "01JCX5B7K3M9N1P5Q7R3S9T1V5";
    let fx = Fx::new();
    let (a, _) = two_commits(&fx);
    let me = fx.machine_id();
    let other_overlay =
        format!("schema = 1\nmachine_id = \"{OTHER}\"\nlabel = \"laptop\"\nos = \"linux\"\n");
    let my_overlay = |label: &str| {
        format!(
            "schema = 1\nmachine_id = \"{me}\"\nlabel = \"{label}\"\nos = \"{}\"\n",
            host_os().as_str()
        )
    };
    fx.edit("machines", |tx| {
        tx.write(&format!("machines/{OTHER}.toml"), other_overlay.clone())?;
        tx.write(&format!("machines/{me}.toml"), my_overlay("desk"))
    });
    let store = fx.store();
    let report = restore::restore(&store, &fx.ax(), &request(&a)).unwrap();
    let head = store.revision(&report.commit.unwrap()).unwrap();
    assert_eq!(
        head.read_file(&format!("machines/{OTHER}.toml")).unwrap(),
        Some(other_overlay)
    );
    // This machine's own overlay is desired state: it goes back to A.
    assert_eq!(
        head.read_file(&format!("machines/{me}.toml")).unwrap(),
        None
    );
}

#[test]
fn shared_manifests_are_written_back_only_when_confirmed() {
    let fx = Fx::new();
    let project = fx.th.projects().join("app");
    std::fs::create_dir_all(project.join(".loadout")).unwrap();
    let me = {
        fx.store();
        fx.machine_id()
    };
    let manifest = |name: &str| format!("schema = 1\nid = \"app-7k2f\"\nname = \"{name}\"\n");
    let local = |name: &str| format!("schema = 1\n\n[servers.{name}]\nenabled = false\n");
    let overlay = format!(
        "schema = 1\nmachine_id = \"{me}\"\nlabel = \"desk\"\nos = \"{}\"\n\n[projects.app-7k2f]\npath = {}\n",
        host_os().as_str(),
        toml::Value::String(project.to_string_lossy().into_owned())
    );
    let a = fx
        .edit("a", |tx| {
            tx.write(&format!("machines/{me}.toml"), overlay.clone())?;
            tx.write("projects/app-7k2f/project.toml", manifest("App"))?;
            tx.write("projects/app-7k2f/project.local.toml", local("first"))
        })
        .unwrap();
    fx.edit("b", |tx| {
        tx.write("projects/app-7k2f/project.toml", manifest("Renamed"))?;
        tx.write("projects/app-7k2f/project.local.toml", local("second"))
    });
    let shared = project.join(".loadout").join("project.toml");
    let private = project.join(".loadout").join("project.local.toml");
    fx.write(&shared, &manifest("Renamed"));
    fx.write(&private, &local("second"));

    let store = fx.store();
    let head = store.head().unwrap();
    let err = restore::restore(&store, &fx.ax(), &request(&a)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfirmationRequired);
    assert!(err.message().contains("project.toml"), "{err}");
    assert_eq!(store.head().unwrap(), head, "nothing committed");
    assert!(store.tags().unwrap().is_empty(), "nothing tagged");
    assert_eq!(fx.read(&shared), manifest("Renamed"));

    let mut confirmed = request(&a);
    confirmed.confirmed = true;
    let report = restore::restore(&store, &fx.ax(), &confirmed).unwrap();
    assert_eq!(report.written, [private.clone(), shared.clone()]);
    assert_eq!(fx.read(&shared), manifest("App"));
    assert_eq!(fx.read(&private), local("first"));

    // Only the private manifest changing needs no confirmation.
    fx.edit("c", |tx| {
        tx.write("projects/app-7k2f/project.local.toml", local("third"))
    });
    let at = store.head().unwrap();
    fx.edit("d", |tx| {
        tx.write("projects/app-7k2f/project.local.toml", local("fourth"))
    });
    fx.write(&private, &local("fourth"));
    let report = restore::restore(&store, &fx.ax(), &request(&at)).unwrap();
    assert_eq!(report.written, std::slice::from_ref(&private));
    assert_eq!(fx.read(&private), local("third"));
}

/// A file system that changes `target` on its `nth` read.
struct RacyFs {
    inner: RootedFs,
    target: PathBuf,
    nth: usize,
    reads: Mutex<usize>,
}

impl Fs for RacyFs {
    fn root(&self) -> Option<&Path> {
        self.inner.root()
    }
    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }
    fn is_dir(&self, path: &Path) -> bool {
        self.inner.is_dir(path)
    }
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.inner.read_to_string(path)
    }
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        if path == self.target {
            let mut reads = self.reads.lock().unwrap();
            *reads += 1;
            if *reads == self.nth {
                std::fs::write(&self.target, "{\"mcpServers\": {}, \"intruder\": true}\n")?;
            }
        }
        self.inner.read(path)
    }
    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.inner.write(path, contents)
    }
    fn write_new(&self, path: &Path, contents: &[u8], like: Option<&Path>) -> io::Result<()> {
        self.inner.write_new(path, contents, like)
    }
    fn replace(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.inner.replace(from, to)
    }
    fn write_private(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.inner.write_private(path, contents)
    }
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.create_dir_all(path)
    }
    fn create_private_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.create_private_dir_all(path)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_file(path)
    }
    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_dir_all(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        self.inner.read_dir(path)
    }
    fn open_lock_file(&self, path: &Path) -> io::Result<File> {
        self.inner.open_lock_file(path)
    }
}

#[test]
fn client_files_restore_from_apply_backups_through_cas() {
    const BEFORE: &str = "{\n  \"theme\": \"dark\",\n  \"mcpServers\": {}\n}\n";
    let fx = Fx::new();
    fx.write(&fx.claude(), BEFORE);
    fx.put_server(stdio(
        "notes",
        &["claude-code"],
        &["-y", "@example/notes-mcp"],
    ));
    let (report, _) = fx.apply();
    let apply_id = report.apply_id.clone();
    let applied = fx.read(&fx.claude());
    assert_ne!(applied, BEFORE);
    assert_eq!(fx.ledger().entries.len(), 1);

    // A racing writer: the file changes between the read and the swap.
    let racy = RacyFs {
        inner: fx.th.fs(),
        target: fx.claude(),
        nth: 2,
        reads: Mutex::new(0),
    };
    let ax = ApplyEnv {
        env: &fx.env,
        fs: &racy,
        home: &fx.home,
        registry: &NoRegistry,
        delivery: &fx.delivery,
    };
    let results = restore_files(&ax, &apply_id, None).unwrap();
    assert_eq!(results.len(), 1);
    match &results[0].outcome {
        FileOutcome::Failed(err) => assert_eq!(err.code(), ErrorCode::FileChangedDuringApply),
        other => panic!("{other:?}"),
    }
    assert!(
        fx.read(&fx.claude()).contains("intruder"),
        "the racing write survives"
    );
    assert_eq!(fx.ledger().entries.len(), 1, "ledger untouched on failure");

    // Without the race: the backed-up bytes come back, the bytes it had are
    // backed up, and the entry the apply created is no longer owned.
    fx.write(&fx.claude(), &applied);
    let results = restore_files(&fx.ax(), &apply_id, Some(&fx.claude())).unwrap();
    let FileOutcome::Restored { previous } = &results[0].outcome else {
        panic!("{:?}", results[0].outcome);
    };
    assert_eq!(fx.read(&fx.claude()), BEFORE);
    assert_eq!(fx.read(previous.as_ref().unwrap()), applied);
    assert!(fx.ledger().entries.is_empty());
    let again = restore_files(&fx.ax(), &apply_id, None).unwrap();
    assert!(matches!(again[0].outcome, FileOutcome::Unchanged));

    // The next apply plans the entry as a new create.
    let plan = fx.plan();
    assert_eq!(
        support::entry(&plan, &fx.claude(), "notes").state,
        agent_loadout_core::apply::EntryState::PendingCreate
    );
    assert_eq!(
        restore_files(&fx.ax(), "01NOSUCHAPPLYRUN0000000000", None)
            .unwrap_err()
            .code(),
        ErrorCode::NotFound
    );
}

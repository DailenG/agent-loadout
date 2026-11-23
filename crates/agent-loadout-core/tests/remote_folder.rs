//! Folder remotes (SPEC 9.6 to 9.9) with two machines in one temp root:
//! convergence through `repo-per-machine`, single-writer subfolders,
//! `machine.json` last, no remote by default, unreachable remotes, mapped
//! drives, incomplete siblings, sync artifacts, conflicts recorded outside
//! the store, mirror and archive exports, and restores from each.

use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use agent_loadout_core::paths::{LoadoutHome, loadout_home};
use agent_loadout_core::remote::config::{FolderMode, RemoteConfig, RemotesFile};
use agent_loadout_core::remote::folder::{self, Source, SourceChoice};
use agent_loadout_core::remote::manifest::MachineManifest;
use agent_loadout_core::remote::unc::{NoUnc, UncResolver};
use agent_loadout_core::store::merge::{MERGE_STATE, MergeOutcome};
use agent_loadout_core::store::{CommitId, Machine, Operation, Store, StoreRead};
use agent_loadout_core::{Env, ErrorCode, FixedClock, Fs, Os};
use agent_loadout_testkit::{RootedFs, TempHome};

const CLIENT: &str = "loadout-test/0.0.0";
const START: Duration = Duration::from_secs(1_767_225_600);

fn host_os() -> Os {
    if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    }
}

fn server(id: &str, command: &str) -> String {
    format!("schema = 1\nid = \"{id}\"\ntransport = \"stdio\"\ncommand = \"{command}\"\n")
}

/// Two machines (`desk-a`, `desk-b`) and a folder remote, in one root.
struct World {
    th: TempHome,
    fs: RootedFs,
    remote: PathBuf,
}

/// One machine: its environment and data directory.
struct Box_ {
    env: Env,
    home: LoadoutHome,
}

impl World {
    fn new() -> Self {
        let th = TempHome::new();
        let fs = th.fs();
        let remote = th.root().join("remote");
        std::fs::create_dir_all(&remote).unwrap();
        Self { th, fs, remote }
    }

    fn machine(&self, label: &str, days: u64) -> Box_ {
        let env = self
            .th
            .env_builder(host_os())
            .hostname(label)
            .clock(FixedClock(
                SystemTime::UNIX_EPOCH + START + Duration::from_secs(days * 86_400),
            ))
            .build();
        let home = loadout_home(&env, &self.fs, Some(&self.th.root().join(label))).unwrap();
        Box_ { env, home }
    }

    fn store<'a>(&'a self, machine: &Box_) -> Store<'a> {
        Store::open_or_init(&machine.env, &self.fs, &machine.home, CLIENT).unwrap()
    }

    fn config(&self, mode: FolderMode) -> RemoteConfig {
        RemoteConfig::folder("share", &self.remote.to_string_lossy(), mode)
    }

    fn machine_dir(&self, store: &Store<'_>) -> PathBuf {
        let id = Machine::load(&self.fs, store.state_dir())
            .unwrap()
            .unwrap()
            .id;
        self.remote.join(store.store_id()).join(id)
    }
}

fn edit(store: &Store<'_>, summary: &str, files: &[(&str, String)]) -> CommitId {
    store
        .transaction(Operation::new("test.edit", summary), |tx| {
            for (path, text) in files {
                tx.write(path, text.clone())?;
            }
            Ok(())
        })
        .unwrap()
        .commit
        .unwrap()
}

/// Every file below `dir` with its bytes.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    out
}

/// Machine B from A's subfolder.
fn bootstrap<'a>(world: &'a World, b: &Box_, from: &Path) -> Store<'a> {
    let SourceChoice::One {
        source: Source::Repo { bare, head, tags },
        ..
    } = folder::open_source(&world.fs, from, None).unwrap()
    else {
        panic!("a repository source");
    };
    Store::init_from_bare(&b.env, &world.fs, &b.home, CLIENT, &bare, &head, &tags).unwrap()
}

#[test]
fn no_remote_exists_after_init() {
    let world = World::new();
    let a = world.machine("desk-a", 0);
    let store = world.store(&a);
    assert!(
        RemotesFile::load(&world.fs, store.state_dir())
            .unwrap()
            .remotes
            .is_empty()
    );
    assert!(!store.state_dir().join("remotes.toml").exists());
}

#[test]
fn two_machines_converge_through_repo_per_machine() {
    let world = World::new();
    let config = world.config(FolderMode::RepoPerMachine);
    let a = world.machine("desk-a", 0);
    let store_a = world.store(&a);
    edit(&store_a, "x", &[("servers/x.toml", server("x", "x-mcp"))]);
    store_a
        .create_user_tag("pre-reset", &store_a.head().unwrap(), None)
        .unwrap();
    folder::push(&store_a, &config, &NoUnc, "0.0.0").unwrap();
    let dir_a = world.machine_dir(&store_a);

    // B starts from A's subfolder: same store, its own machine.
    let b = world.machine("desk-b", 0);
    let store_b = bootstrap(&world, &b, &dir_a);
    assert_eq!(store_b.store_id(), store_a.store_id());
    assert_eq!(store_b.head().unwrap(), store_a.head().unwrap());
    assert!(store_b.tag_exists("pre-reset"));
    assert_eq!(
        std::fs::read_to_string(b.home.store_dir().join("servers/x.toml")).unwrap(),
        server("x", "x-mcp")
    );

    // Both change different things; each push leaves the other's
    // subfolder byte for byte as it was.
    edit(&store_b, "y", &[("servers/y.toml", server("y", "y-mcp"))]);
    edit(&store_a, "z", &[("servers/z.toml", server("z", "z-mcp"))]);
    let before = snapshot(&dir_a);
    folder::push(&store_b, &config, &NoUnc, "0.0.0").unwrap();
    assert_eq!(snapshot(&dir_a), before, "B never writes A's subfolder");
    let dir_b = world.machine_dir(&store_b);
    let before = snapshot(&dir_b);
    folder::push(&store_a, &config, &NoUnc, "0.0.0").unwrap();
    assert_eq!(snapshot(&dir_b), before, "A never writes B's subfolder");
    let manifest = MachineManifest::load(&world.fs, &dir_b).unwrap();
    assert_eq!(manifest.label, "desk-b");
    assert_eq!(
        manifest.head.as_deref(),
        Some(store_b.head().unwrap().as_str())
    );

    // A pulls: a merge commit with both parents and every server.
    let pulled = folder::pull(&store_a, &config, &NoUnc).unwrap();
    assert!(
        pulled
            .tag
            .as_deref()
            .is_some_and(|t| t.starts_with("auto/pre-pull/"))
    );
    assert_eq!(pulled.siblings.len(), 1);
    let Ok(MergeOutcome::Merged(merge)) = &pulled.siblings[0].outcome else {
        panic!("{:?}", pulled.siblings[0].outcome);
    };
    let (entry, _) = store_a.entry(merge).unwrap();
    assert_eq!(entry.summary, "merge: share/desk-b");
    for id in ["x", "y", "z"] {
        assert!(
            store_a
                .read_file(&format!("servers/{id}.toml"))
                .unwrap()
                .is_some(),
            "{id}"
        );
    }
    folder::push(&store_a, &config, &NoUnc, "0.0.0").unwrap();

    // B pulls: a fast-forward to A's merge; then both are the same.
    let pulled = folder::pull(&store_b, &config, &NoUnc).unwrap();
    assert!(matches!(
        pulled.siblings[0].outcome,
        Ok(MergeOutcome::FastForward(_))
    ));
    assert_eq!(store_b.head().unwrap(), store_a.head().unwrap());
    assert!(b.home.store_dir().join("servers/z.toml").exists());
    let again = folder::pull(&store_a, &config, &NoUnc).unwrap();
    assert!(matches!(
        again.siblings[0].outcome,
        Ok(MergeOutcome::UpToDate)
    ));
    assert!(again.tag.is_none(), "nothing incoming, no tag");
}

#[test]
fn conflicts_leave_head_alone_and_are_recorded_outside_the_store() {
    let world = World::new();
    let config = world.config(FolderMode::RepoPerMachine);
    let a = world.machine("desk-a", 0);
    let store_a = world.store(&a);
    edit(&store_a, "x", &[("servers/x.toml", server("x", "x-mcp"))]);
    folder::push(&store_a, &config, &NoUnc, "0.0.0").unwrap();
    let b = world.machine("desk-b", 0);
    let store_b = bootstrap(&world, &b, &world.machine_dir(&store_a));
    edit(&store_a, "a", &[("servers/x.toml", server("x", "from-a"))]);
    edit(&store_b, "b", &[("servers/x.toml", server("x", "from-b"))]);
    folder::push(&store_b, &config, &NoUnc, "0.0.0").unwrap();

    let head = store_a.head().unwrap();
    let pulled = folder::pull(&store_a, &config, &NoUnc).unwrap();
    assert_eq!(pulled.conflicts(), 1);
    let Ok(MergeOutcome::Conflicts { conflicts, .. }) = &pulled.siblings[0].outcome else {
        panic!();
    };
    assert_eq!(conflicts[0].field.as_deref(), Some("command"));
    assert_eq!(conflicts[0].ours.as_deref(), Some("\"from-a\""));
    assert_eq!(conflicts[0].theirs.as_deref(), Some("\"from-b\""));
    assert_eq!(store_a.head().unwrap(), head, "HEAD unchanged");
    let state = store_a.state_dir().join("merge").join(MERGE_STATE);
    let recorded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state).unwrap()).unwrap();
    assert_eq!(recorded["merges"][0]["conflicts"][0]["field"], "command");
    assert!(!a.home.store_dir().join("merge").exists());

    // Another remote without conflicts keeps them; so does the same remote
    // while the sibling's objects are on their way.
    let other = world.th.root().join("usb");
    std::fs::create_dir_all(&other).unwrap();
    let usb = RemoteConfig::folder("usb", &other.to_string_lossy(), FolderMode::RepoPerMachine);
    folder::pull(&store_a, &usb, &NoUnc).unwrap();
    assert!(state.exists(), "another remote's pull keeps the conflicts");
    let newer = edit(&store_b, "b2", &[("servers/y.toml", server("y", "y-mcp"))]);
    let dir_b = world.machine_dir(&store_b);
    let mut manifest = MachineManifest::load(&world.fs, &dir_b).unwrap();
    manifest.head = Some(newer.to_string());
    manifest.write(&world.fs, &dir_b).unwrap();
    let pulled = folder::pull(&store_a, &config, &NoUnc).unwrap();
    assert_eq!(
        pulled.siblings[0].outcome.as_ref().unwrap_err().code(),
        ErrorCode::RemoteIncomplete
    );
    assert!(state.exists(), "an incomplete sibling keeps its conflicts");
    // Local work goes on.
    edit(
        &store_a,
        "more",
        &[("servers/w.toml", server("w", "w-mcp"))],
    );
}

#[test]
fn a_failed_bootstrap_leaves_nothing_and_a_retry_works() {
    let world = World::new();
    let config = world.config(FolderMode::RepoPerMachine);
    let a = world.machine("desk-a", 0);
    let store_a = world.store(&a);
    edit(&store_a, "x", &[("servers/x.toml", server("x", "x-mcp"))]);
    folder::push(&store_a, &config, &NoUnc, "0.0.0").unwrap();
    let dir_a = world.machine_dir(&store_a);
    // The manifest names a commit whose objects have not arrived.
    let newer = edit(&store_a, "y", &[("servers/y.toml", server("y", "y-mcp"))]);
    let pushed = MachineManifest::load(&world.fs, &dir_a).unwrap();
    let mut early = pushed.clone();
    early.head = Some(newer.to_string());
    early.write(&world.fs, &dir_a).unwrap();

    let b = world.machine("desk-b", 0);
    let SourceChoice::One {
        source: Source::Repo { bare, head, tags },
        ..
    } = folder::open_source(&world.fs, &dir_a, None).unwrap()
    else {
        panic!("a repository source");
    };
    let err =
        Store::init_from_bare(&b.env, &world.fs, &b.home, CLIENT, &bare, &head, &tags).unwrap_err();
    assert_eq!(err.code(), ErrorCode::RemoteIncomplete);
    assert!(!b.home.store_dir().exists(), "nothing left behind");
    assert!(
        Store::open(&b.env, &world.fs, &b.home, CLIENT)
            .unwrap()
            .is_none()
    );

    // The objects arrive: the retry works.
    folder::push(&store_a, &config, &NoUnc, "0.0.0").unwrap();
    let store_b = bootstrap(&world, &b, &dir_a);
    assert_eq!(store_b.head().unwrap(), newer);

    // A store folder holding other files is never bootstrapped into.
    let c = world.machine("desk-c", 0);
    std::fs::create_dir_all(c.home.store_dir()).unwrap();
    std::fs::write(c.home.store_dir().join("notes.txt"), "mine").unwrap();
    let SourceChoice::One {
        source: Source::Repo { bare, head, tags },
        ..
    } = folder::open_source(&world.fs, &dir_a, None).unwrap()
    else {
        panic!("a repository source");
    };
    let err =
        Store::init_from_bare(&c.env, &world.fs, &c.home, CLIENT, &bare, &head, &tags).unwrap_err();
    assert_eq!(err.code(), ErrorCode::AlreadyExists);
    assert_eq!(
        std::fs::read_to_string(c.home.store_dir().join("notes.txt")).unwrap(),
        "mine"
    );
}

#[test]
fn a_switch_to_mirror_leaves_the_old_repository_unused() {
    let world = World::new();
    let a = world.machine("desk-a", 0);
    let store_a = world.store(&a);
    edit(&store_a, "x", &[("servers/x.toml", server("x", "x-mcp"))]);
    folder::push(
        &store_a,
        &world.config(FolderMode::RepoPerMachine),
        &NoUnc,
        "0.0.0",
    )
    .unwrap();
    let dir_a = world.machine_dir(&store_a);
    let b = world.machine("desk-b", 0);
    let store_b = bootstrap(&world, &b, &dir_a);
    let b_config = world.config(FolderMode::RepoPerMachine);
    folder::push(&store_b, &b_config, &NoUnc, "0.0.0").unwrap();

    // A now pushes a mirror to the same folder; its store.git stays, stale.
    edit(&store_a, "y", &[("servers/y.toml", server("y", "y-mcp"))]);
    folder::push(&store_a, &world.config(FolderMode::Mirror), &NoUnc, "0.0.0").unwrap();
    assert!(dir_a.join("store.git").is_dir());
    let pulled = folder::pull(&store_b, &b_config, &NoUnc).unwrap();
    assert!(pulled.siblings.is_empty(), "{:?}", pulled.siblings);
    let SourceChoice::One { source, .. } = folder::open_source(&world.fs, &dir_a, None).unwrap()
    else {
        panic!();
    };
    assert!(matches!(source, Source::Mirror { .. }), "{source:?}");
}

#[test]
fn a_store_folder_lists_its_machines() {
    let world = World::new();
    let config = world.config(FolderMode::RepoPerMachine);
    let a = world.machine("desk-a", 0);
    let store_a = world.store(&a);
    folder::push(&store_a, &config, &NoUnc, "0.0.0").unwrap();
    let store_folder = world.remote.join(store_a.store_id());
    let SourceChoice::One { machine, .. } =
        folder::open_source(&world.fs, &store_folder, None).unwrap()
    else {
        panic!("one machine");
    };
    assert_eq!(machine.unwrap().manifest.label, "desk-a");
}

#[test]
fn unreachable_remotes_never_block_local_commits() {
    let world = World::new();
    let a = world.machine("desk-a", 0);
    let store = world.store(&a);
    let config = RemoteConfig::folder(
        "usb",
        &world.th.root().join("unplugged").to_string_lossy(),
        FolderMode::RepoPerMachine,
    );
    let err = folder::push(&store, &config, &NoUnc, "0.0.0").unwrap_err();
    assert_eq!(err.code(), ErrorCode::RemoteUnreachable);
    assert_eq!(
        folder::pull(&store, &config, &NoUnc).unwrap_err().code(),
        ErrorCode::RemoteUnreachable
    );
    assert!(!world.th.root().join("unplugged").exists(), "never created");
    edit(&store, "x", &[("servers/x.toml", server("x", "x-mcp"))]);
}

/// A pretend `WNetGetUniversalNameW`: one mapped drive.
struct FakeUnc {
    drive: PathBuf,
    share: PathBuf,
    asked: Mutex<Vec<PathBuf>>,
}

impl UncResolver for FakeUnc {
    fn universal_name(&self, path: &Path) -> Option<PathBuf> {
        self.asked.lock().unwrap().push(path.to_path_buf());
        // By text: off Windows, `Z:\loadout` is one path component, so
        // `Path::strip_prefix` would never match the drive.
        path.to_str()?
            .strip_prefix(self.drive.to_str()?)
            .map(|rest| self.share.join(rest))
    }
}

#[test]
fn mapped_drive_letters_resolve_to_unc_paths() {
    let world = World::new();
    let fake = FakeUnc {
        drive: PathBuf::from(r"Z:\"),
        share: world.remote.clone(),
        asked: Mutex::new(Vec::new()),
    };
    let config = RemoteConfig::folder("nas", r"Z:\loadout", FolderMode::RepoPerMachine);
    std::fs::create_dir_all(world.remote.join("loadout")).unwrap();
    let root = folder::reachable_root(&world.fs, Os::Windows, &fake, &config).unwrap();
    assert_eq!(root, world.remote.join("loadout"));
    assert_eq!(
        fake.asked.lock().unwrap().as_slice(),
        [PathBuf::from(r"Z:\loadout")]
    );
    // The typed path is what the configuration keeps.
    assert_eq!(config.path.as_deref(), Some(r"Z:\loadout"));
    // Other OSes and paths that are not drive letters are used as typed.
    let plain = world.config(FolderMode::RepoPerMachine);
    folder::reachable_root(&world.fs, Os::Linux, &fake, &plain).unwrap();
    assert_eq!(fake.asked.lock().unwrap().len(), 1);

    #[cfg(windows)]
    {
        let a = world.machine("desk-a", 0);
        let store = world.store(&a);
        let pushed = folder::push(&store, &config, &fake, "0.0.0").unwrap();
        assert!(pushed.dir.starts_with(world.remote.join("loadout")));
        assert!(pushed.dir.join("machine.json").exists());
    }
}

#[test]
fn a_sibling_whose_objects_have_not_arrived_is_incomplete() {
    let world = World::new();
    let config = world.config(FolderMode::RepoPerMachine);
    let a = world.machine("desk-a", 0);
    let store_a = world.store(&a);
    edit(&store_a, "x", &[("servers/x.toml", server("x", "x-mcp"))]);
    folder::push(&store_a, &config, &NoUnc, "0.0.0").unwrap();
    let b = world.machine("desk-b", 0);
    let store_b = bootstrap(&world, &b, &world.machine_dir(&store_a));
    folder::push(&store_b, &config, &NoUnc, "0.0.0").unwrap();
    // B's manifest arrives naming a commit whose objects did not.
    let newer = edit(&store_b, "y", &[("servers/y.toml", server("y", "y-mcp"))]);
    let dir_b = world.machine_dir(&store_b);
    let mut manifest = MachineManifest::load(&world.fs, &dir_b).unwrap();
    manifest.head = Some(newer.to_string());
    manifest.write(&world.fs, &dir_b).unwrap();

    let head = store_a.head().unwrap();
    let pulled = folder::pull(&store_a, &config, &NoUnc).unwrap();
    let err = pulled.siblings[0].outcome.clone().unwrap_err();
    assert_eq!(err.code(), ErrorCode::RemoteIncomplete);
    assert_eq!(store_a.head().unwrap(), head);
    assert!(pulled.tag.is_none());
}

#[test]
fn sync_artifacts_are_ignored_reported_and_left_in_place() {
    let world = World::new();
    let config = world.config(FolderMode::RepoPerMachine);
    let a = world.machine("desk-a", 0);
    let store_a = world.store(&a);
    edit(&store_a, "x", &[("servers/x.toml", server("x", "x-mcp"))]);
    folder::push(&store_a, &config, &NoUnc, "0.0.0").unwrap();
    let b = world.machine("desk-b", 0);
    let store_b = bootstrap(&world, &b, &world.machine_dir(&store_a));
    edit(&store_b, "y", &[("servers/y.toml", server("y", "y-mcp"))]);
    folder::push(&store_b, &config, &NoUnc, "0.0.0").unwrap();
    let dir_b = world.machine_dir(&store_b);
    let head_b = store_b.head().unwrap().to_string();

    let tags = dir_b.join("store.git").join("refs").join("tags");
    std::fs::create_dir_all(&tags).unwrap();
    let planted = [
        tags.join("v1.sync-conflict-20260101-000000-ABCDEFG"),
        tags.join("stray"),
        dir_b.join("machine (conflicted copy 2026-01-01).json"),
        dir_b.join("machine.json.!sync"),
        world
            .remote
            .join(store_a.store_id())
            .join("01JCX5B7K3M9N1P5Q7R3S9T1V5.Conflict")
            .join("machine.json"),
    ];
    for path in &planted {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("{head_b}\n")).unwrap();
    }

    let listing = folder::list(&world.fs, &world.remote, None).unwrap();
    assert_eq!(
        listing.machines.len(),
        2,
        "the conflict copy is not a machine"
    );
    let reported: Vec<&PathBuf> = listing.artifacts.iter().map(|a| &a.path).collect();
    for path in &planted[..4] {
        assert!(
            reported.contains(&path),
            "{} in {reported:?}",
            path.display()
        );
    }
    let resilio = planted[4].parent().unwrap().to_path_buf();
    assert!(reported.contains(&&resilio), "{reported:?}");
    let tool = |path: &Path| {
        listing
            .artifacts
            .iter()
            .find(|a| a.path == path)
            .unwrap()
            .tool
    };
    assert_eq!(tool(&planted[0]), "Syncthing");
    assert_eq!(tool(&planted[1]), "unlisted ref");
    assert_eq!(tool(&planted[2]), "Dropbox");
    assert_eq!(tool(&planted[3]), "Resilio Sync");

    let pulled = folder::pull(&store_a, &config, &NoUnc).unwrap();
    assert!(matches!(
        pulled.siblings[0].outcome,
        Ok(MergeOutcome::FastForward(_))
    ));
    assert!(!pulled.artifacts.is_empty());
    assert!(!store_a.tag_exists("stray"), "unlisted refs are never read");
    for path in &planted {
        assert!(path.exists(), "{} left in place", path.display());
    }
}

#[test]
fn restore_sources_list_machines_by_label() {
    let world = World::new();
    let config = world.config(FolderMode::RepoPerMachine);
    let a = world.machine("desk-a", 0);
    let store_a = world.store(&a);
    folder::push(&store_a, &config, &NoUnc, "0.0.0").unwrap();
    let b = world.machine("desk-b", 0);
    let store_b = bootstrap(&world, &b, &world.machine_dir(&store_a));
    folder::push(&store_b, &config, &NoUnc, "0.0.0").unwrap();
    let id_b = Machine::load(&world.fs, store_b.state_dir())
        .unwrap()
        .unwrap()
        .id;

    let SourceChoice::Choose(machines) =
        folder::open_source(&world.fs, &world.remote, None).unwrap()
    else {
        panic!("two machines to choose from");
    };
    let mut labels: Vec<&str> = machines.iter().map(|m| m.manifest.label.as_str()).collect();
    labels.sort();
    assert_eq!(labels, ["desk-a", "desk-b"]);
    assert!(machines.iter().all(|m| m.subject.is_some()));
    for pick in ["desk-b", id_b.as_str()] {
        let SourceChoice::One { machine, .. } =
            folder::open_source(&world.fs, &world.remote, Some(pick)).unwrap()
        else {
            panic!("{pick}");
        };
        assert_eq!(machine.unwrap().machine_id, id_b, "{pick}");
    }
    // A third machine with the same label: the label is ambiguous.
    let twin = world
        .remote
        .join(store_a.store_id())
        .join("01JCX5B7K3M9N1P5Q7R3S9T1V5");
    std::fs::create_dir_all(&twin).unwrap();
    let mut manifest = MachineManifest::load(&world.fs, &world.machine_dir(&store_b)).unwrap();
    manifest.machine_id = "01JCX5B7K3M9N1P5Q7R3S9T1V5".to_owned();
    manifest.write(&world.fs, &twin).unwrap();
    let err = folder::open_source(&world.fs, &world.remote, Some("desk-b")).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);
    assert!(err.message().contains(&id_b), "{err}");
    assert_eq!(
        folder::open_source(&world.fs, &world.remote, Some("nobody"))
            .unwrap_err()
            .code(),
        ErrorCode::NotFound
    );
}

#[test]
fn mirror_and_archive_exports_restore_new_machines() {
    let world = World::new();
    let a = world.machine("desk-a", 0);
    let store_a = world.store(&a);
    edit(&store_a, "x", &[("servers/x.toml", server("x", "x-mcp"))]);
    let files_a = |store: &Store<'_>| {
        let head = store.head().unwrap();
        let revision = store.revision(&head).unwrap();
        revision
            .files()
            .unwrap()
            .into_iter()
            .filter(|f| !f.starts_with("machines/"))
            .map(|f| {
                let bytes = revision.read_bytes(&f).unwrap().unwrap();
                (f, bytes)
            })
            .collect::<BTreeMap<_, _>>()
    };

    // Mirror: latest/ holds the tree and EXPORT.json.
    let mirror = world.config(FolderMode::Mirror);
    let pushed = folder::push(&store_a, &mirror, &NoUnc, "0.0.0").unwrap();
    let latest = pushed.dir.join("latest");
    assert!(latest.join("servers").join("x.toml").exists());
    assert!(latest.join("EXPORT.json").exists());
    folder::push(&store_a, &mirror, &NoUnc, "0.0.0").unwrap();
    let leftovers: Vec<String> = std::fs::read_dir(&pushed.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        {
            let mut l = leftovers;
            l.sort();
            l
        },
        ["latest", "machine.json"],
        "no temp folders left"
    );
    let SourceChoice::One { source, .. } =
        folder::open_source(&world.fs, &pushed.dir, None).unwrap()
    else {
        panic!();
    };
    assert!(matches!(source, Source::Mirror { .. }));
    // A sync tool's files inside `latest/` are not store files.
    let latest = pushed.dir.join("latest");
    std::fs::write(latest.join("desktop.ini"), "[.ShellClassInfo]\n").unwrap();
    std::fs::write(
        latest
            .join("servers")
            .join("x (conflicted copy 2026-01-01).toml"),
        server("x", "stale"),
    )
    .unwrap();
    let export = folder::export_files(&world.fs, &source).unwrap();
    let files = export.files;
    assert!(!files.contains_key("EXPORT.json"));
    assert!(
        !files
            .keys()
            .any(|f| f.contains("desktop.ini") || f.contains("conflicted"))
    );
    let mut tools: Vec<&str> = export.artifacts.iter().map(|a| a.tool).collect();
    tools.sort_unstable();
    assert_eq!(tools, ["Dropbox", "OS"]);
    let c = world.machine("desk-c", 0);
    let store_c =
        Store::init_from_files(&c.env, &world.fs, &c.home, CLIENT, files, "restore: mirror")
            .unwrap();
    assert_eq!(store_c.store_id(), store_a.store_id());
    assert_eq!(files_a(&store_c), files_a(&store_a));

    // Archive: one zip per push, the oldest beyond `keep` deleted.
    let mut archive = world.config(FolderMode::Archive);
    archive.name = "zips".to_owned();
    archive.keep = Some(2);
    let mut names = Vec::new();
    for day in 1..=3 {
        let later = world.machine("desk-a", day);
        let store = Store::open(&later.env, &world.fs, &a.home, CLIENT)
            .unwrap()
            .unwrap();
        edit(
            &store,
            "more",
            &[("servers/x.toml", server("x", &format!("x-mcp-{day}")))],
        );
        let pushed = folder::push(&store, &archive, &NoUnc, "0.0.0").unwrap();
        names.push(pushed.archive.unwrap());
    }
    let dir = world.machine_dir(&store_a);
    let manifest = MachineManifest::load(&world.fs, &dir).unwrap();
    assert_eq!(manifest.archives, names[1..]);
    assert!(!dir.join("archives").join(&names[0]).exists());
    let source = Source::Zip {
        file: dir.join("archives").join(&names[2]),
    };
    let files = folder::export_files(&world.fs, &source).unwrap().files;
    assert_eq!(
        String::from_utf8(files["servers/x.toml"].clone()).unwrap(),
        server("x", "x-mcp-3")
    );
    let d = world.machine("desk-d", 0);
    let store_d =
        Store::init_from_files(&d.env, &world.fs, &d.home, CLIENT, files, "restore: zip").unwrap();
    assert_eq!(store_d.store_id(), store_a.store_id());
}

/// Records whether `store.git` had its `main` ref when `machine.json` was
/// put in place.
struct OrderFs {
    inner: RootedFs,
    ref_present: Mutex<Vec<bool>>,
}

impl Fs for OrderFs {
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
        self.inner.read(path)
    }
    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.inner.write(path, contents)
    }
    fn write_new(&self, path: &Path, contents: &[u8], like: Option<&Path>) -> io::Result<()> {
        self.inner.write_new(path, contents, like)
    }
    fn replace(&self, from: &Path, to: &Path) -> io::Result<()> {
        if to.file_name().is_some_and(|name| name == "machine.json") {
            let main = to
                .parent()
                .unwrap()
                .join("store.git")
                .join("refs")
                .join("heads")
                .join("main");
            self.ref_present
                .lock()
                .unwrap()
                .push(self.inner.exists(&main));
        }
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
    fn is_regular_file(&self, path: &Path) -> bool {
        self.inner.is_regular_file(path)
    }
    fn overwrite_with_zeros(&self, path: &Path) -> io::Result<()> {
        self.inner.overwrite_with_zeros(path)
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
fn machine_json_is_written_last() {
    let world = World::new();
    let a = world.machine("desk-a", 0);
    world.store(&a);
    let fs = OrderFs {
        inner: world.th.fs(),
        ref_present: Mutex::new(Vec::new()),
    };
    let store = Store::open(&a.env, &fs, &a.home, CLIENT).unwrap().unwrap();
    folder::push(
        &store,
        &world.config(FolderMode::RepoPerMachine),
        &NoUnc,
        "0.0.0",
    )
    .unwrap();
    assert_eq!(fs.ref_present.lock().unwrap().as_slice(), [true]);
}

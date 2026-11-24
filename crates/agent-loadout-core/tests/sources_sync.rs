//! `sources.toml` across two machines (#24, SPEC 9.9): trust lapses that
//! both machines record before they sync merge without a conflict (the
//! earlier lapse stays), and an untrust on one machine against a lapse on
//! the other merges to a valid file without the record (untrust wins).
//! Two machines and a folder remote in one temp root.

use std::path::Path;
use std::time::{Duration, SystemTime};

use agent_loadout_core::paths::{LoadoutHome, loadout_home};
use agent_loadout_core::remote::config::{FolderMode, RemoteConfig};
use agent_loadout_core::remote::folder::{self, Source, SourceChoice};
use agent_loadout_core::remote::unc::NoUnc;
use agent_loadout_core::sources::{
    self, LapseReason, SourceChange, SourceChangeKind, SourceRef, SourceScope, TrustState, TrustVia,
};
use agent_loadout_core::store::merge::MergeOutcome;
use agent_loadout_core::store::{Machine, Store};
use agent_loadout_core::{Env, FixedClock, Os};
use agent_loadout_testkit::{RootedFs, TempHome};

const CLIENT: &str = "loadout-test/0.0.0";
const START: Duration = Duration::from_secs(1_767_225_600);
const URL: &str = "https://github.com/acme/skills";

fn host_os() -> Os {
    if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    }
}

struct World {
    th: TempHome,
    fs: RootedFs,
    config: RemoteConfig,
}

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
        let config = RemoteConfig::folder(
            "share",
            &remote.to_string_lossy(),
            FolderMode::RepoPerMachine,
        );
        Self { th, fs, config }
    }

    /// A machine whose clock is `hours` after the start.
    fn machine(&self, label: &str, hours: u64) -> Box_ {
        let env = self
            .th
            .env_builder(host_os())
            .hostname(label)
            .clock(FixedClock(
                SystemTime::UNIX_EPOCH + START + Duration::from_secs(hours * 3_600),
            ))
            .build();
        let home = loadout_home(&env, &self.fs, Some(&self.th.root().join(label))).unwrap();
        Box_ { env, home }
    }

    fn store<'a>(&'a self, machine: &Box_) -> Store<'a> {
        Store::open_or_init(&machine.env, &self.fs, &machine.home, CLIENT).unwrap()
    }

    fn push(&self, store: &Store<'_>) {
        folder::push(store, &self.config, &NoUnc, "0.0.0").unwrap();
    }

    /// Pulls the other machine's head into `store`; its one outcome.
    fn pull(&self, store: &Store<'_>) -> MergeOutcome {
        let pulled = folder::pull(store, &self.config, &NoUnc).unwrap();
        assert_eq!(pulled.siblings.len(), 1);
        pulled.siblings[0].outcome.clone().unwrap()
    }

    fn machine_dir(&self, store: &Store<'_>) -> std::path::PathBuf {
        let id = Machine::load(&self.fs, store.state_dir())
            .unwrap()
            .unwrap()
            .id;
        self.th
            .root()
            .join("remote")
            .join(store.store_id())
            .join(id)
    }
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

fn rewritten() -> SourceChange {
    SourceChange {
        kind: SourceChangeKind::HistoryRewritten,
        detail: None,
    }
}

#[test]
fn concurrent_lapses_and_an_untrust_merge_cleanly() {
    let world = World::new();
    let source = SourceRef::from_git_url(URL).unwrap();
    let key = source.key().to_owned();
    let a = world.machine("desk-a", 0);
    let store_a = world.store(&a);
    sources::trust(&store_a, &source, SourceScope::Repo, TrustVia::App).unwrap();
    world.push(&store_a);
    let b = world.machine("desk-b", 2);
    let store_b = bootstrap(&world, &b, &world.machine_dir(&store_a));

    // Both machines see the same force-push before they sync; B's clock is
    // later.
    sources::lapse(&store_a, &source, &rewritten())
        .unwrap()
        .unwrap();
    sources::lapse(&store_b, &source, &rewritten())
        .unwrap()
        .unwrap();
    let ours = sources::list(&store_a).unwrap()[0]
        .1
        .lapsed
        .clone()
        .unwrap();
    world.push(&store_b);
    let outcome = world.pull(&store_a);
    assert!(matches!(outcome, MergeOutcome::Merged(_)), "{outcome:?}");
    let records = sources::list(&store_a).unwrap();
    assert_eq!(records.len(), 1);
    let lapse = records[0].1.lapsed.clone().unwrap();
    assert_eq!(lapse.reason, LapseReason::HistoryRewritten);
    assert_eq!(lapse.at, ours.at, "the earlier lapse stays");
    world.push(&store_a);
    assert!(matches!(world.pull(&store_b), MergeOutcome::FastForward(_)));

    // A trusts again; B gets it. Then A revokes while B sees another
    // force-push: the revocation wins and the file stays valid.
    sources::trust(&store_a, &source, SourceScope::Repo, TrustVia::App).unwrap();
    world.push(&store_a);
    assert!(matches!(world.pull(&store_b), MergeOutcome::FastForward(_)));
    sources::untrust(&store_a, &key).unwrap();
    sources::lapse(&store_b, &source, &rewritten())
        .unwrap()
        .unwrap();
    world.push(&store_b);
    let outcome = world.pull(&store_a);
    assert!(matches!(outcome, MergeOutcome::Merged(_)), "{outcome:?}");
    assert!(sources::list(&store_a).unwrap().is_empty());
    assert!(matches!(
        sources::check(&store_a, &source).unwrap().state,
        TrustState::Ask { .. }
    ));
}

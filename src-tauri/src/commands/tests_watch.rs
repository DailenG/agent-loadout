//! Drift watcher tests (SPEC 8.5) against a temp home: real file system
//! events, debounced into one `on_change` call per burst.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use agent_loadout_core::model::applied::Scope as TargetScope;
use agent_loadout_core::skills::link;
use agent_loadout_testkit::TempHome;

use super::tests::{Fx, unbound_project};
use super::watch::{DEBOUNCE, DriftWatch, TargetsChanged, WatchSet};
use super::{Core, projects};

/// Long enough for any file system event to arrive and be debounced.
const ARRIVES: Duration = Duration::from_secs(5);

/// A running watcher over a temp home. Fields drop in order: the watch
/// stops before the temp home goes.
struct Running {
    _watch: DriftWatch,
    events: Receiver<()>,
    core: Arc<Core>,
    th: TempHome,
}

impl Running {
    fn start(fx: Fx) -> Self {
        let Fx { th, core, .. } = fx;
        let core = Arc::new(core);
        let (tx, events) = mpsc::channel();
        let watch = DriftWatch::start(core.clone(), move || {
            let _ = tx.send(());
        })
        .unwrap();
        // The watches are in place; give the backend a moment to arm them.
        std::thread::sleep(Duration::from_millis(200));
        Self {
            _watch: watch,
            events,
            core,
            th,
        }
    }

    /// One event arrives, and no second one follows the same burst.
    fn expect_one(&self, what: &str) {
        self.events
            .recv_timeout(ARRIVES)
            .unwrap_or_else(|_| panic!("no event after {what}"));
        self.expect_none(what);
    }

    fn expect_none(&self, what: &str) {
        assert!(
            self.events.recv_timeout(DEBOUNCE * 3).is_err(),
            "an extra event after {what}"
        );
    }
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
}

/// The event names no path and no content: its payload is `null`.
#[test]
fn the_event_carries_nothing() {
    assert_eq!(serde_json::to_string(&TargetsChanged).unwrap(), "null");
}

/// The set covers user files, project files, manifest and skill folders,
/// and the store and state folders.
#[test]
fn the_set_covers_every_target_kind() {
    let fx = Fx::new();
    let root = unbound_project(&fx);
    projects::add(&fx.core, &root).unwrap();
    let set = WatchSet::derive(&fx.core);
    assert!(set.files.contains(&fx.claude()), "{set:?}");
    assert!(set.files.contains(&root.join(".mcp.json")), "{set:?}");
    assert!(set.folders.contains(&root.join(".loadout")), "{set:?}");
    let skills = link::skills_dir(&fx.core.env, "claude-code", TargetScope::User, None).unwrap();
    assert!(set.folders.contains(&skills), "{set:?}");
    let project_skills = link::skills_dir(
        &fx.core.env,
        "claude-code",
        TargetScope::Project,
        Some(&root),
    )
    .unwrap();
    assert!(set.folders.contains(&project_skills), "{set:?}");
    assert!(set.control.contains(&fx.core.home.store_dir()), "{set:?}");
    assert!(set.control.contains(&fx.core.home.state_dir()), "{set:?}");
    let anchors = set.anchors();
    assert!(anchors.contains(&fx.th.home()), "{anchors:?}");
    assert!(anchors.contains(&root), "{anchors:?}");
}

/// Several quick writes to a user file are one event.
#[test]
fn a_burst_of_writes_is_one_event() {
    let run = Running::start(Fx::new());
    let claude = run.th.home().join(".claude.json");
    for n in 0..3 {
        write(&claude, &format!("{{\"n\": {n}}}"));
        std::thread::sleep(Duration::from_millis(50));
    }
    run.expect_one("three writes to ~/.claude.json");
}

/// The e2e case: a hand edit of a known project's `<P>/.mcp.json`.
#[test]
fn a_project_file_edit_is_an_event() {
    let fx = Fx::new();
    let root = unbound_project(&fx);
    projects::add(&fx.core, &root).unwrap();
    let run = Running::start(fx);
    write(&root.join(".mcp.json"), "{\"mcpServers\": {}}\n");
    run.expect_one("an edit of <P>/.mcp.json");
    write(
        &root.join(".loadout/project.toml"),
        "schema = 1\nid = \"app-a1b2\"\nname = \"app\"\n",
    );
    run.expect_one("an edit of the manifest");
}

/// Files next to targets that are not targets send nothing.
#[test]
fn other_files_send_nothing() {
    let run = Running::start(Fx::new());
    write(&run.th.home().join("notes.txt"), "not a client file");
    run.expect_none("a write to ~/notes.txt");
}

/// A skill folder that does not exist yet is watched through its
/// ancestors: creating each folder on the way is an event, and so is a new
/// entry in the skill folder once it exists.
#[test]
fn a_new_skill_folder_is_watched_once_it_exists() {
    let run = Running::start(Fx::new());
    let skills = link::skills_dir(&run.core.env, "claude-code", TargetScope::User, None).unwrap();
    let missing: Vec<PathBuf> = skills
        .ancestors()
        .take_while(|dir| !dir.exists())
        .map(Path::to_path_buf)
        .collect();
    assert!(!missing.is_empty(), "the temp home has no {skills:?} yet");
    for dir in missing.iter().rev() {
        std::fs::create_dir(dir).unwrap();
        run.expect_one(&format!("creating {dir:?}"));
    }
    std::fs::create_dir(skills.join("notes")).unwrap();
    run.expect_one("a new skill folder entry");
}

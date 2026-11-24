//! The Loadout skill for agents in core (#8, SPEC 12.6): downgrade
//! protection (a copy from a newer Loadout is never replaced by `refresh`,
//! `install` or `startup_refresh`), the recheck under the lock (a run that
//! read an older state never overwrites a newer copy written meanwhile),
//! the cached "customized" verdict, non-edits that keep a copy current, and
//! `install` with `reset` keeping the client selection.

mod support;

use std::time::Duration;

use agent_loadout_core::Env;
use agent_loadout_core::adapters::detect::NoRegistry;
use agent_loadout_core::agent::{self, CopyState, Refresh, SKILL_ID};
use agent_loadout_core::remote::git::SystemRunner;
use agent_loadout_core::skills::{LinkKind, SkillsEnv, store};
use agent_loadout_core::store::{Operation, Store};
use support::Fx;

static RUNNER: SystemRunner = SystemRunner {
    idle_timeout: Duration::from_secs(60),
};
static NO_REGISTRY: NoRegistry = NoRegistry;

fn sx<'a>(fx: &'a Fx, env: &'a Env) -> SkillsEnv<'a> {
    SkillsEnv {
        env,
        fs: &fx.fs,
        home: &fx.home,
        registry: &NO_REGISTRY,
        runner: &RUNNER,
        link_kind: LinkKind::Junction,
    }
}

/// Rewrites the store copy as Loadout `version` wrote `text` (recorded
/// with its hash, or with `edited` the bundled hash: an edit afterwards).
fn write_copy(store: &Store<'_>, version: &str, text: &str, edited: bool) {
    store
        .transaction(Operation::new("test.edit", "test: copy"), |tx| {
            let mut files = agent::bundled_files();
            if !edited {
                files.insert("SKILL.md".to_owned(), text.as_bytes().to_vec());
            }
            let mut meta = store::read_meta(tx, SKILL_ID)?.unwrap();
            let bundled = meta.bundled.as_mut().unwrap();
            bundled.version = version.to_owned();
            bundled.hash = agent::normalized_hash(&files);
            store::stage_meta(tx, &meta)?;
            tx.write("skills/loadout/SKILL.md", text)
        })
        .unwrap();
}

fn skill_md(store: &Store<'_>) -> String {
    std::fs::read_to_string(store.root().join("skills/loadout/SKILL.md")).unwrap()
}

fn state(store: &Store<'_>, fx: &Fx, env: &Env) -> CopyState {
    agent::status(Some(store), &sx(fx, env))
        .unwrap()
        .copy
        .unwrap()
        .state
}

#[test]
fn a_copy_from_a_newer_loadout_is_never_replaced() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let store = fx.store();
    agent::install(&store, &sx(&fx, &env), &["claude-code".to_owned()], false).unwrap();
    write_copy(&store, "99.0.0", "from the future\n", false);
    assert_eq!(state(&store, &fx, &env), CopyState::Newer);

    assert!(agent::refresh(&store, &sx(&fx, &env)).unwrap().is_none());
    assert!(
        agent::startup_refresh(&store, &sx(&fx, &env))
            .unwrap()
            .is_none()
    );
    let change = agent::install(&store, &sx(&fx, &env), &["omp".to_owned()], false).unwrap();
    assert!(change.refreshed.is_none());
    assert_eq!(skill_md(&store), "from the future\n");
    assert_eq!(state(&store, &fx, &env), CopyState::Newer);
}

/// Two runs both read an outdated copy; the first updates it to a newer
/// version than the second carries. The second, checking again under the
/// lock, writes nothing and reports nothing.
#[test]
fn a_run_that_read_an_older_state_never_overwrites_a_newer_copy() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let store = fx.store();
    agent::install(&store, &sx(&fx, &env), &["claude-code".to_owned()], false).unwrap();
    write_copy(&store, "0.0.1", "old\n", false);
    let seen = agent::status(Some(&store), &sx(&fx, &env))
        .unwrap()
        .copy
        .unwrap();
    assert_eq!(seen.state, CopyState::Outdated);

    // Meanwhile a newer Loadout updated it.
    write_copy(&store, "99.0.0", "newer\n", false);
    assert!(
        agent::refresh_seen(&store, &sx(&fx, &env), seen.clone())
            .unwrap()
            .is_none()
    );
    assert_eq!(skill_md(&store), "newer\n");

    // Or the same version did: no second commit, no second report.
    write_copy(&store, "0.0.1", "old\n", false);
    let seen = agent::status(Some(&store), &sx(&fx, &env))
        .unwrap()
        .copy
        .unwrap();
    assert!(matches!(
        agent::refresh(&store, &sx(&fx, &env)).unwrap(),
        Some(Refresh::Updated { .. })
    ));
    assert!(
        agent::refresh_seen(&store, &sx(&fx, &env), seen)
            .unwrap()
            .is_none()
    );
}

/// Once found customized for this bundled version, the copy is not opened
/// and hashed again at each store change: even a reverted edit waits for
/// `install` (which checks it) or the next version.
#[test]
fn the_customized_verdict_is_kept_per_version() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let store = fx.store();
    agent::install(&store, &sx(&fx, &env), &["claude-code".to_owned()], false).unwrap();
    write_copy(&store, "0.0.1", "my notes\n", true);
    assert!(matches!(
        agent::startup_refresh(&store, &sx(&fx, &env)).unwrap(),
        Some(Refresh::Customized { .. })
    ));
    // The user reverts the edit: the copy is outdated again, but the
    // verdict stands for this version.
    write_copy(&store, "0.0.1", "old\n", false);
    assert!(
        agent::startup_refresh(&store, &sx(&fx, &env))
            .unwrap()
            .is_none()
    );
    assert_eq!(skill_md(&store), "old\n");
    let change =
        agent::install(&store, &sx(&fx, &env), &["claude-code".to_owned()], false).unwrap();
    assert!(matches!(change.refreshed, Some(Refresh::Updated { .. })));
    assert_eq!(state(&store, &fx, &env), CopyState::Current);
}

/// Junk files and a CRLF resave in the store folder (reachable through a
/// client's link) are not edits: the copy stays current.
#[test]
fn junk_files_and_line_endings_are_not_edits() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let store = fx.store();
    agent::install(&store, &sx(&fx, &env), &["claude-code".to_owned()], false).unwrap();
    let dir = store.root().join("skills").join(SKILL_ID);
    std::fs::write(dir.join(".DS_Store"), b"junk").unwrap();
    std::fs::write(dir.join("Thumbs.db"), b"junk").unwrap();
    let resaved = format!("\u{feff}{}", agent::FILES[0].1.replace('\n', "\r\n"));
    std::fs::write(dir.join("SKILL.md"), resaved).unwrap();
    assert_eq!(state(&store, &fx, &env), CopyState::Current);
}

#[test]
fn reset_replaces_an_edited_copy_and_keeps_its_clients() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let store = fx.store();
    let clients = ["claude-code".to_owned(), "omp".to_owned()];
    agent::install(&store, &sx(&fx, &env), &clients, false).unwrap();
    write_copy(&store, env!("CARGO_PKG_VERSION"), "mine\n", true);
    assert_eq!(state(&store, &fx, &env), CopyState::Customized);

    let change = agent::install(&store, &sx(&fx, &env), &[], true).unwrap();
    assert!(change.reset);
    assert_eq!(skill_md(&store), agent::FILES[0].1);
    assert_eq!(state(&store, &fx, &env), CopyState::Current);
    let meta = store::read_meta(&store, SKILL_ID).unwrap().unwrap();
    let selection = format!("{:?}", meta.enable.unwrap().clients.unwrap());
    assert!(
        selection.contains("claude-code") && selection.contains("omp"),
        "{selection}"
    );
}

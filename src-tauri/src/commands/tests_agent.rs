//! M46: Settings > Agent integration against a temp home (#8): the status
//! with no store, enabling and disabling the Loadout skill per client
//! through the normal skill sync, removing it, and the startup refresh
//! (an older copy updated, an edited one reported once). No path crosses
//! IPC.

use agent_loadout_core::agent::{self, SKILL_ID};
use agent_loadout_core::skills::store;
use agent_loadout_core::store::{Operation, Store};

use super::agent::{AgentSkillNotice, AgentSkillState, refresh, remove, reset, set_client, status};
use super::tests::Fx;

fn assert_no_root(fx: &Fx, json: &str) {
    let root = fx.th.root().to_string_lossy().into_owned();
    for form in [
        root.clone(),
        root.replace('\\', "/"),
        root.replace('\\', "\\\\"),
    ] {
        assert!(!json.contains(&form), "{json}");
    }
}

/// Marks the store copy as written by Loadout 0.0.1, with `text` as its
/// `SKILL.md`; `edited` leaves the recorded hash at the old content.
fn age(fx: &Fx, text: &str, edited: bool) {
    let core = &fx.core;
    let store = Store::open(
        &core.env,
        core.fs.as_ref(),
        &core.home,
        "loadout-test/0.0.0",
    )
    .unwrap()
    .unwrap();
    store
        .transaction(Operation::new("test.edit", "test: age"), |tx| {
            let mut meta = store::read_meta(tx, SKILL_ID)?.unwrap();
            let bundled = meta.bundled.as_mut().unwrap();
            bundled.version = "0.0.1".to_owned();
            if !edited {
                let mut files = agent::bundled_files();
                files.insert("SKILL.md".to_owned(), text.as_bytes().to_vec());
                bundled.hash = agent::normalized_hash(&files);
            }
            store::stage_meta(tx, &meta)?;
            tx.write("skills/loadout/SKILL.md", text)
        })
        .unwrap();
}

fn store_skill_md(fx: &Fx) -> String {
    std::fs::read_to_string(fx.core.home.store_dir().join("skills/loadout/SKILL.md")).unwrap()
}

#[test]
fn status_lists_clients_omp_first_without_a_store() {
    let fx = Fx::new();
    let shown = status(&fx.core).unwrap();
    assert_eq!(shown.bundled_version, env!("CARGO_PKG_VERSION"));
    assert!(shown.copy.is_none());
    assert!(!shown.taken);
    assert_eq!(shown.clients[0].id, "omp");
    assert!(
        shown
            .clients
            .iter()
            .any(|client| client.name == "Claude Code")
    );
    assert!(shown.clients.iter().all(|client| !client.enabled));
    assert!(!fx.core.home.store_dir().exists());
}

#[test]
fn clients_are_enabled_and_disabled_through_the_skill_sync() {
    let fx = Fx::new();
    let change = set_client(&fx.core, "claude-code", true).unwrap();
    assert!(change.commit.is_some());
    let json = serde_json::to_string(&change).unwrap();
    assert_no_root(&fx, &json);
    let link = fx.th.home().join(".claude").join("skills").join(SKILL_ID);
    assert!(link.join("SKILL.md").is_file());

    let shown = status(&fx.core).unwrap();
    let copy = shown.copy.as_ref().unwrap();
    assert_eq!(copy.state, AgentSkillState::Current);
    assert!(
        shown
            .clients
            .iter()
            .any(|client| client.id == "claude-code" && client.enabled)
    );
    assert_no_root(&fx, &serde_json::to_string(&shown).unwrap());

    set_client(&fx.core, "omp", true).unwrap();
    set_client(&fx.core, "claude-code", false).unwrap();
    assert!(std::fs::symlink_metadata(&link).is_err());
    let shown = status(&fx.core).unwrap();
    assert!(
        shown
            .clients
            .iter()
            .any(|client| client.id == "omp" && client.enabled)
    );

    remove(&fx.core).unwrap();
    assert!(status(&fx.core).unwrap().copy.is_none());
    // Unknown clients are refused.
    let err = set_client(&fx.core, "nope", true).unwrap_err();
    assert_eq!(err.code, agent_loadout_core::ErrorCode::UnknownClient);
}

#[test]
fn startup_refresh_updates_an_older_copy_and_reports_an_edited_one_once() {
    let fx = Fx::new();
    assert!(refresh(&fx.core).unwrap().is_none());
    set_client(&fx.core, "claude-code", true).unwrap();
    assert!(refresh(&fx.core).unwrap().is_none());

    age(&fx, "old text\n", false);
    assert_eq!(
        status(&fx.core).unwrap().copy.unwrap().state,
        AgentSkillState::Outdated
    );
    match refresh(&fx.core).unwrap() {
        Some(AgentSkillNotice::Updated {
            from,
            to,
            needs_attention,
        }) => {
            assert!(!needs_attention);
            assert_eq!(from, "0.0.1");
            assert_eq!(to, env!("CARGO_PKG_VERSION"));
        }
        other => panic!("{other:?}"),
    }
    assert_ne!(store_skill_md(&fx), "old text\n");

    age(&fx, "my notes\n", true);
    assert!(matches!(
        refresh(&fx.core).unwrap(),
        Some(AgentSkillNotice::Customized { .. })
    ));
    assert!(refresh(&fx.core).unwrap().is_none());
    assert_eq!(store_skill_md(&fx), "my notes\n");
    assert_eq!(
        status(&fx.core).unwrap().copy.unwrap().state,
        AgentSkillState::Customized
    );
}

/// The update's skill sync result reaches the notice: a copy the user
/// changed in a client folder needs a decision.
#[test]
fn the_startup_update_says_when_a_client_folder_needs_a_decision() {
    let fx = Fx::new();
    set_client(&fx.core, "claude-code", true).unwrap();
    super::skills::set_method(&fx.core, SKILL_ID, super::skills::SkillMethod::Copy).unwrap();
    let copy = fx.th.home().join(".claude").join("skills").join(SKILL_ID);
    std::fs::write(copy.join("SKILL.md"), "changed in place\n").unwrap();
    age(&fx, "old text\n", false);
    match refresh(&fx.core).unwrap() {
        Some(AgentSkillNotice::Updated {
            needs_attention, ..
        }) => assert!(needs_attention),
        other => panic!("{other:?}"),
    }
}

/// "Restore Loadout's version" replaces an edited copy and keeps the
/// clients it is enabled for.
#[test]
fn reset_restores_the_bundled_copy_for_the_same_clients() {
    let fx = Fx::new();
    set_client(&fx.core, "claude-code", true).unwrap();
    age(&fx, "my notes\n", true);
    let change = reset(&fx.core).unwrap();
    assert!(change.commit.is_some());
    assert_no_root(&fx, &serde_json::to_string(&change).unwrap());
    assert_eq!(store_skill_md(&fx), agent::FILES[0].1);
    let shown = status(&fx.core).unwrap();
    assert_eq!(shown.copy.unwrap().state, AgentSkillState::Current);
    assert!(
        shown
            .clients
            .iter()
            .any(|client| client.id == "claude-code" && client.enabled)
    );
}

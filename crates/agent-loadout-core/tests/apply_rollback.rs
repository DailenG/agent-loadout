//! One-click rollback (SPEC 8.7): a new commit that restores the desired
//! state from before the run, applied through the normal planner.

mod support;

use agent_loadout_core::ErrorCode;
use agent_loadout_core::apply::EntryState;
use agent_loadout_core::apply::rollback::{self, ApplySelector};
use agent_loadout_core::model::TomlDocument;
use agent_loadout_core::model::applied::{AppliedRecord, ApplyOutcome, ApplyTrigger};
use support::{Fx, json_entry, result_entry, stdio};

const NOTES_ARGS: [&str; 2] = ["-y", "@example/notes-mcp"];

fn rollback(fx: &Fx, selector: &str, confirmed: bool) -> rollback::RollbackReport {
    rollback::rollback(
        &fx.store(),
        &fx.ax(),
        &ApplySelector::parse(selector),
        confirmed,
    )
    .unwrap()
}

fn add_scratch_by_hand(fx: &Fx) {
    let mut value: serde_json::Value = serde_json::from_str(&fx.read(&fx.claude())).unwrap();
    value["mcpServers"]["scratch"] = serde_json::json!({"type": "stdio", "command": "scratch"});
    fx.write(&fx.claude(), &serde_json::to_string_pretty(&value).unwrap());
}

#[test]
fn rollback_removes_created_entries_and_keeps_later_foreign_ones() {
    let fx = Fx::new();
    fx.write(&fx.claude(), "{\n  \"theme\": \"dark\"\n}\n");
    let (applied, _) = fx.instant("server: add notes", |tx| {
        tx.add_server(stdio("notes", &["claude-code"], &NOTES_ARGS))
    });
    assert!(json_entry(&fx.read(&fx.claude()), "notes").is_some());
    add_scratch_by_hand(&fx);
    let commits = fx.commit_count();
    let head = fx.store().head().unwrap();

    let report = rollback(&fx, &applied.apply_id, false);
    assert_eq!(report.plan.apply_id, applied.apply_id);
    assert!(report.plan.latest);
    assert_eq!(report.plan.files, vec!["servers/notes.toml".to_owned()]);
    assert!(report.tag.starts_with("auto/pre-rollback/"));
    assert_eq!(fx.store().tag_target(&report.tag).unwrap(), Some(head));

    // One new commit; history is not rewritten.
    assert_eq!(fx.commit_count(), commits + 1);
    let log = fx.log();
    assert_eq!(log[0].id, report.commit.clone().unwrap());
    assert_eq!(
        log[0].summary,
        format!("rollback: apply {}", applied.apply_id)
    );
    assert_eq!(log[0].trailers.op.as_deref(), Some("apply.rollback"));
    assert!(
        log[0]
            .trailers
            .entities
            .contains(&"server/notes".to_owned())
    );
    let apply = log[0].trailers.apply.clone().unwrap();
    assert_eq!(
        (apply.id.as_str(), apply.outcome),
        (report.apply.apply_id.as_str(), ApplyOutcome::Ok)
    );

    // The created entry is gone, the later foreign entry and the rest stay.
    let text = fx.read(&fx.claude());
    assert!(json_entry(&text, "notes").is_none(), "{text}");
    assert!(json_entry(&text, "scratch").is_some(), "{text}");
    assert!(text.contains("\"theme\": \"dark\""));
    assert!(
        fx.store()
            .read_to_string("servers/notes.toml")
            .unwrap()
            .is_none()
    );
    assert!(fx.ledger().entries.is_empty());

    let machine = fx.machine_id();
    let record = fx
        .store()
        .read_to_string(&format!("machines/{machine}.applied.toml"))
        .unwrap()
        .unwrap();
    let record = TomlDocument::<AppliedRecord>::parse(&record)
        .unwrap()
        .into_value();
    assert_eq!(record.trigger, ApplyTrigger::Rollback);
    assert_eq!(record.targets[0].remove, 1);
}

#[test]
fn rolling_back_a_rollback_restores_the_original() {
    let fx = Fx::new();
    let (applied, _) = fx.instant("server: add notes", |tx| {
        tx.add_server(stdio("notes", &["claude-code"], &NOTES_ARGS))
    });
    let original = fx.read(&fx.claude());
    let server = fx.store().read_to_string("servers/notes.toml").unwrap();
    rollback(&fx, &applied.apply_id, false);
    assert!(json_entry(&fx.read(&fx.claude()), "notes").is_none());

    let again = rollback(&fx, "last", false);
    assert_eq!(again.plan.files, vec!["servers/notes.toml".to_owned()]);
    assert_eq!(fx.read(&fx.claude()), original);
    assert_eq!(
        fx.store().read_to_string("servers/notes.toml").unwrap(),
        server
    );
    assert_eq!(fx.ledger().entries.len(), 1);
}

#[test]
fn rollback_of_a_review_apply_restores_the_previously_applied_state() {
    let fx = Fx::new();
    // Applied state 1: docs.
    fx.put_server(stdio(
        "docs",
        &["claude-code"],
        &["-y", "@example/docs-mcp"],
    ));
    fx.apply();
    // Edits applied together later: notes added, docs changed.
    fx.put_server(stdio("notes", &["claude-code"], &NOTES_ARGS));
    fx.put_server(stdio(
        "docs",
        &["claude-code"],
        &["-y", "@example/docs-mcp", "--v2"],
    ));
    let (applied, _) = fx.apply();

    let report = rollback(&fx, &applied.apply_id, false);
    assert_eq!(
        report.plan.files,
        vec![
            "servers/docs.toml".to_owned(),
            "servers/notes.toml".to_owned()
        ]
    );
    let text = fx.read(&fx.claude());
    assert!(json_entry(&text, "notes").is_none());
    assert_eq!(
        json_entry(&text, "docs").unwrap()["args"],
        serde_json::json!(["-y", "@example/docs-mcp"])
    );
}

#[test]
fn entries_changed_since_the_apply_are_kept_and_reported() {
    let fx = Fx::new();
    let (applied, _) = fx.instant("server: add notes", |tx| {
        tx.add_server(stdio("notes", &["claude-code"], &NOTES_ARGS))
    });
    let edited = fx
        .read(&fx.claude())
        .replace("notes-mcp", "notes-hand-edit");
    fx.write(&fx.claude(), &edited);
    let report = rollback(&fx, &applied.apply_id, false);
    assert_eq!(
        fx.read(&fx.claude()),
        edited,
        "a drifted entry is not removed"
    );
    let notes = result_entry(&report.apply, &fx.claude(), "notes");
    assert_eq!(notes.state, EntryState::Drifted);
    assert!(notes.unresolved);
    assert_eq!(report.apply.outcome, ApplyOutcome::Blocked);
}

#[test]
fn rolling_back_an_older_apply_over_later_changes_needs_confirmation() {
    let fx = Fx::new();
    let (first, _) = fx.instant("server: add notes", |tx| {
        tx.add_server(stdio("notes", &["claude-code"], &NOTES_ARGS))
    });
    fx.instant("server: update notes", |tx| {
        let text = agent_loadout_core::model::TomlDocument::new(stdio(
            "notes",
            &["claude-code"],
            &["-y", "@example/notes-mcp", "--v2"],
        ))
        .to_text()?;
        tx.write("servers/notes.toml", text)
    });
    let commits = fx.commit_count();
    let err = rollback::rollback(
        &fx.store(),
        &fx.ax(),
        &ApplySelector::Id(first.apply_id.clone()),
        false,
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::ConfirmationRequired);
    assert_eq!(
        fx.commit_count(),
        commits,
        "nothing happens without confirmation"
    );
    let plan = rollback::prepare(
        &fx.store(),
        &fx.machine_id(),
        &ApplySelector::Id(first.apply_id.clone()),
    )
    .unwrap();
    assert_eq!(plan.changed_since, vec!["servers/notes.toml".to_owned()]);

    rollback(&fx, &first.apply_id, true);
    assert!(json_entry(&fx.read(&fx.claude()), "notes").is_none());

    let missing = rollback::rollback(
        &fx.store(),
        &fx.ax(),
        &ApplySelector::Id("01JCX4A0M2N8P6Q4R2S0T8U6V4".to_owned()),
        true,
    )
    .unwrap_err();
    assert_eq!(missing.code(), ErrorCode::NotFound);
}

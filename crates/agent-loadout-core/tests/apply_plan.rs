//! The planning table (SPEC 8.3): one scenario per row, through real
//! store edits, real client files in a `TempHome` and the ledger an apply
//! leaves behind. Planning itself never writes.

mod support;

use agent_loadout_core::ErrorCode;
use agent_loadout_core::apply::{Action, Decision, EntryState, OnConflict, PlanOptions};
use agent_loadout_core::model::clients::ClientSelection;
use agent_loadout_core::model::server::{Delivery, DeliveryMode, Transport};
use support::{Fx, entry, json_entry, stdio};

const NOTES_ARGS: [&str; 2] = ["-y", "@example/notes-mcp"];

/// The entry Loadout writes for `stdio("notes", ..)` in `~/.claude.json`.
const NOTES_ENTRY: &str = r#"{
  "mcpServers": {
    "notes": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "@example/notes-mcp"]
    }
  }
}
"#;

fn notes(fx: &Fx) {
    fx.put_server(stdio("notes", &["claude-code"], &NOTES_ARGS));
}

fn assert_row(fx: &Fx, name: &str, state: EntryState, action: Action) {
    let plan = fx.plan();
    let entry = entry(&plan, &fx.claude(), name);
    assert_eq!(
        (entry.state, entry.action),
        (state, action),
        "entry {name}: {entry:#?}"
    );
}

#[test]
fn row_desired_only_creates() {
    let fx = Fx::new();
    notes(&fx);
    assert_row(&fx, "notes", EntryState::PendingCreate, Action::Create);
    let plan = fx.plan();
    let target = &plan.targets[0];
    assert!(target.before.is_none());
    let after = String::from_utf8(target.after.clone().unwrap()).unwrap();
    assert_eq!(
        json_entry(&after, "notes").unwrap(),
        serde_json::json!({"type": "stdio", "command": "npx", "args": ["-y", "@example/notes-mcp"]})
    );
    assert!(
        target.diff().contains("+    \"notes\": {"),
        "{}",
        target.diff()
    );
    assert!(!fx.claude().exists(), "planning never writes");
}

#[test]
fn row_owned_but_deleted_is_missing_and_recreated() {
    let fx = Fx::new();
    notes(&fx);
    fx.apply();
    fx.write(&fx.claude(), "{\n  \"mcpServers\": {}\n}\n");
    assert_row(&fx, "notes", EntryState::Missing, Action::Create);

    // `skip` keeps it missing; `import` respects the deletion.
    let skip = fx.plan_with(&PlanOptions {
        on_conflict: OnConflict::Skip,
        ..PlanOptions::default()
    });
    assert_eq!(entry(&skip, &fx.claude(), "notes").action, Action::Skip);
    let import = fx.plan_with(&PlanOptions {
        on_conflict: OnConflict::Import,
        ..PlanOptions::default()
    });
    assert_eq!(entry(&import, &fx.claude(), "notes").action, Action::Import);
}

#[test]
fn row_identical_foreign_entry_is_adopted_silently() {
    let fx = Fx::new();
    fx.write(&fx.claude(), NOTES_ENTRY);
    notes(&fx);
    assert_row(&fx, "notes", EntryState::Synced, Action::Adopt);
    assert!(
        fx.plan().targets[0].after.is_none(),
        "adopting writes nothing"
    );
}

#[test]
fn row_different_foreign_entry_collides() {
    let fx = Fx::new();
    fx.write(
        &fx.claude(),
        &NOTES_ENTRY.replace("notes-mcp", "notes-fork"),
    );
    notes(&fx);
    let plan = fx.plan();
    let notes = entry(&plan, &fx.claude(), "notes");
    assert_eq!(
        (notes.state, notes.action),
        (EntryState::Collision, Action::Skip)
    );
    assert!(notes.unresolved, "ask is the default");
    assert!(plan.targets[0].after.is_none());

    let overwrite = fx.plan_with(&PlanOptions {
        on_conflict: OnConflict::Overwrite,
        ..PlanOptions::default()
    });
    assert_eq!(
        entry(&overwrite, &fx.claude(), "notes").action,
        Action::AdoptOverwrite
    );
    // A per-entry decision wins over the default.
    let decided = fx.plan_with(&PlanOptions {
        on_conflict: OnConflict::Overwrite,
        decisions: vec![Decision {
            client: "claude-code".to_owned(),
            name: "notes".to_owned(),
            resolution: OnConflict::Skip,
        }],
        ..PlanOptions::default()
    });
    let skipped = entry(&decided, &fx.claude(), "notes");
    assert_eq!(skipped.action, Action::Skip);
    assert!(!skipped.unresolved);
}

#[test]
fn row_owned_unchanged_and_desired_unchanged_is_synced() {
    let fx = Fx::new();
    notes(&fx);
    fx.apply();
    assert_row(&fx, "notes", EntryState::Synced, Action::None);
    assert!(!fx.plan().has_work());
}

#[test]
fn row_owned_unchanged_and_desired_changed_updates() {
    let fx = Fx::new();
    notes(&fx);
    fx.apply();
    fx.put_server(stdio(
        "notes",
        &["claude-code"],
        &["-y", "@example/notes-mcp", "--verbose"],
    ));
    assert_row(&fx, "notes", EntryState::PendingUpdate, Action::Update);
}

#[test]
fn row_changed_to_what_is_desired_refreshes_the_ledger() {
    let fx = Fx::new();
    notes(&fx);
    fx.apply();
    let args = ["-y", "@example/notes-mcp", "--verbose"];
    fx.put_server(stdio("notes", &["claude-code"], &args));
    let text = fx.read(&fx.claude()).replace(
        "\"@example/notes-mcp\"]",
        "\"@example/notes-mcp\", \"--verbose\"]",
    );
    fx.write(&fx.claude(), &text);
    assert_row(&fx, "notes", EntryState::Synced, Action::Refresh);
    assert!(fx.plan().targets[0].after.is_none());
}

#[test]
fn row_drift_with_desired_change_is_a_conflict() {
    let fx = Fx::new();
    notes(&fx);
    fx.apply();
    let edited = fx
        .read(&fx.claude())
        .replace("notes-mcp", "notes-hand-edit");
    fx.write(&fx.claude(), &edited);
    // Desired unchanged: drifted.
    assert_row(&fx, "notes", EntryState::Drifted, Action::Skip);
    // Desired changed too: conflict.
    fx.put_server(stdio(
        "notes",
        &["claude-code"],
        &["-y", "@example/notes-mcp", "--v2"],
    ));
    let plan = fx.plan();
    let notes = entry(&plan, &fx.claude(), "notes");
    assert_eq!(
        (notes.state, notes.action),
        (EntryState::Conflict, Action::Skip)
    );
    assert!(notes.unresolved);
    let overwrite = fx.plan_with(&PlanOptions {
        on_conflict: OnConflict::Overwrite,
        ..PlanOptions::default()
    });
    assert_eq!(
        entry(&overwrite, &fx.claude(), "notes").action,
        Action::Update
    );
    let import = fx.plan_with(&PlanOptions {
        on_conflict: OnConflict::Import,
        ..PlanOptions::default()
    });
    assert_eq!(entry(&import, &fx.claude(), "notes").action, Action::Import);
}

#[test]
fn row_no_longer_desired_and_unchanged_is_removed_or_disabled() {
    let fx = Fx::new();
    fx.put_server(stdio("notes", &["claude-code", "omp"], &NOTES_ARGS));
    fx.apply();
    fx.put_server(stdio("notes", &[], &NOTES_ARGS));
    assert_row(&fx, "notes", EntryState::PendingRemove, Action::Remove);
    let plan = fx.plan();
    assert_eq!(entry(&plan, &fx.omp(), "notes").action, Action::Remove);

    // omp disables natively when clients.toml asks; Claude Code has no
    // native disable and still removes.
    fx.edit("prefs", |tx| {
        tx.write(
            "clients.toml",
            "schema = 1\n[omp]\nnative_disable = true\n[claude-code]\nnative_disable = true\n",
        )
    });
    let plan = fx.plan();
    let omp = entry(&plan, &fx.omp(), "notes");
    assert_eq!(
        (omp.state, omp.action),
        (EntryState::PendingRemove, Action::Disable)
    );
    assert_eq!(
        omp.written
            .as_ref()
            .unwrap()
            .as_object()
            .unwrap()
            .get("enabled"),
        Some(&agent_loadout_core::formats::NativeValue::Bool(false))
    );
    assert_eq!(entry(&plan, &fx.claude(), "notes").action, Action::Remove);

    // Once disabled, it is synced.
    fx.apply();
    let plan = fx.plan();
    assert_eq!(entry(&plan, &fx.omp(), "notes").state, EntryState::Synced);
    assert!(!plan.has_work());
}

#[test]
fn row_no_longer_desired_and_changed_is_a_drifted_orphan() {
    let fx = Fx::new();
    notes(&fx);
    fx.apply();
    let edited = fx
        .read(&fx.claude())
        .replace("notes-mcp", "notes-hand-edit");
    fx.write(&fx.claude(), &edited);
    fx.put_server(stdio("notes", &[], &NOTES_ARGS));
    let plan = fx.plan();
    let notes = entry(&plan, &fx.claude(), "notes");
    assert_eq!(
        (notes.state, notes.action),
        (EntryState::Drifted, Action::Skip)
    );
    assert!(notes.unresolved);
    let overwrite = fx.plan_with(&PlanOptions {
        on_conflict: OnConflict::Overwrite,
        ..PlanOptions::default()
    });
    assert_eq!(
        entry(&overwrite, &fx.claude(), "notes").action,
        Action::Remove
    );
    let import = fx.plan_with(&PlanOptions {
        on_conflict: OnConflict::Import,
        ..PlanOptions::default()
    });
    assert_eq!(
        entry(&import, &fx.claude(), "notes").action,
        Action::Release
    );
}

#[test]
fn row_owned_entry_gone_and_not_desired_drops_the_ledger_record() {
    let fx = Fx::new();
    notes(&fx);
    fx.apply();
    fx.write(&fx.claude(), "{}\n");
    fx.remove_server("notes");
    assert_row(&fx, "notes", EntryState::Synced, Action::Forget);
    let plan = fx.plan();
    assert!(plan.targets[0].after.is_none());
    fx.apply();
    assert!(fx.ledger().entries.is_empty());
    assert_eq!(fx.read(&fx.claude()), "{}\n", "forgetting writes nothing");
}

#[test]
fn row_foreign_entry_is_untouched() {
    let fx = Fx::new();
    fx.write(
        &fx.claude(),
        &NOTES_ENTRY.replace("\"notes\"", "\"scratch\""),
    );
    notes(&fx);
    assert_row(&fx, "scratch", EntryState::Foreign, Action::None);
    assert!(!entry(&fx.plan(), &fx.claude(), "scratch").unresolved);
}

#[test]
fn invalid_server_files_never_remove_owned_entries() {
    let fx = Fx::new();
    notes(&fx);
    fx.apply();
    // A broken hand edit of the server file (working tree only).
    let path = fx.home.store_dir().join("servers").join("notes.toml");
    std::fs::write(&path, "schema = 1\nid = \"notes\"\ntransport = \n").unwrap();
    let plan = fx.plan();
    let notes = entry(&plan, &fx.claude(), "notes");
    assert_eq!(notes.state, EntryState::Blocked);
    assert_eq!(
        notes.problem.as_ref().unwrap().code(),
        ErrorCode::ParseError
    );
    assert!(plan.targets[0].after.is_none());
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.code() == ErrorCode::ParseError)
    );
}

#[test]
fn unwritable_entries_are_blocked_never_dropped() {
    let fx = Fx::new();
    // SSE on Codex, a secret reference, an env reference in a field Codex
    // only forwards, and `launch` delivery that this build cannot render.
    let mut events = stdio("events", &["codex", "claude-code"], &[]);
    events.body.transport = Some(Transport::Sse);
    events.body.command = None;
    events.body.args = None;
    events.body.url = Some("https://events.example.com/sse".to_owned());
    fx.put_server(events);
    let mut secret = stdio("vault", &["claude-code"], &NOTES_ARGS);
    secret.body.env = Some([("VAULT_TOKEN".to_owned(), "${secret:VAULT_TOKEN}".to_owned())].into());
    fx.put_server(secret);
    let mut forwarded = stdio(
        "forwarded",
        &["claude-code", "codex"],
        &["--root", "${env:NOTES_DIR}"],
    );
    forwarded.body.env = Some([("NOTES_TOKEN".to_owned(), "${env:NOTES_TOKEN}".to_owned())].into());
    fx.put_server(forwarded);
    let mut launch = stdio(
        "launched",
        &["claude-code"],
        &["--root", "${env:NOTES_DIR}"],
    );
    launch.body.delivery = Some(Delivery {
        mode: Some(DeliveryMode::Launch),
        ..Delivery::default()
    });
    fx.put_server(launch);
    fx.put_server(stdio("ok", &["claude-code"], &NOTES_ARGS));

    let plan = fx.plan();
    let codex = fx.th.home().join(".codex").join("config.toml");
    let blocked = |file: &std::path::Path, name: &str| {
        let entry = entry(&plan, file, name);
        assert_eq!(entry.state, EntryState::Blocked, "{name}: {entry:#?}");
        entry.problem.as_ref().unwrap().code()
    };
    assert_eq!(blocked(&codex, "events"), ErrorCode::UnsupportedTransport);
    assert_eq!(
        entry(&plan, &fx.claude(), "events").state,
        EntryState::PendingCreate
    );
    assert_eq!(
        blocked(&fx.claude(), "vault"),
        ErrorCode::DeliveryUnavailable
    );
    assert_eq!(blocked(&codex, "forwarded"), ErrorCode::DeliveryUnavailable);
    assert_eq!(
        blocked(&fx.claude(), "launched"),
        ErrorCode::DeliveryUnavailable
    );

    // Claude Code expands `${VAR}` in args and env: env delivery.
    let forwarded = entry(&plan, &fx.claude(), "forwarded");
    assert_eq!(forwarded.delivery, Some("env"));
    let after = String::from_utf8(
        plan.targets
            .iter()
            .find(|t| t.file == fx.claude())
            .unwrap()
            .after
            .clone()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        json_entry(&after, "forwarded").unwrap(),
        serde_json::json!({
            "type": "stdio",
            "command": "npx",
            "args": ["--root", "${NOTES_DIR}"],
            "env": {"NOTES_TOKEN": "${NOTES_TOKEN}"}
        })
    );
    assert!(json_entry(&after, "vault").is_none());
    assert!(json_entry(&after, "ok").is_some());
}

#[test]
fn clients_without_an_adapter_or_excluded_are_reported_not_written() {
    let fx = Fx::new();
    fx.put_server(stdio(
        "notes",
        &["claude-code", "cursor", "warp"],
        &NOTES_ARGS,
    ));
    let plan = fx.plan();
    let codes: Vec<ErrorCode> = plan.warnings.iter().map(|w| w.code()).collect();
    assert!(codes.contains(&ErrorCode::NoAdapter), "{codes:?}");
    assert!(codes.contains(&ErrorCode::UnknownClient), "{codes:?}");
    assert_eq!(plan.targets.len(), 1);

    // `enabled = false` in clients.toml opts a client out.
    fx.edit("prefs", |tx| {
        tx.write(
            "clients.toml",
            "schema = 1\n[claude-code]\nenabled = false\n",
        )
    });
    assert!(fx.plan().targets.is_empty());

    // `all-detected` picks detected clients only (nothing is installed).
    fx.edit("prefs", |tx| tx.remove("clients.toml"));
    let mut all = stdio("notes", &[], &NOTES_ARGS);
    all.enable = Some(agent_loadout_core::model::server::Enable {
        clients: Some(ClientSelection::AllDetected),
        ..Default::default()
    });
    fx.put_server(all);
    assert!(fx.plan().targets.is_empty());
    std::fs::create_dir_all(fx.th.home().join(".omp")).unwrap();
    let plan = fx.plan();
    assert_eq!(plan.targets.len(), 1);
    assert_eq!(plan.targets[0].file, fx.omp());
}

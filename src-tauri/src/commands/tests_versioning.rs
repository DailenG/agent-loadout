//! History, remotes and secrets command tests (M17, SPEC 13.5, 13.6, 11.9)
//! against temp homes: `RootedFs` panics on any path outside a temp root,
//! the keyring is in memory, no `op` is on `PATH`, and folder remotes are
//! folders inside the temp root (copied between machines the way a sync
//! tool or a USB drive would carry them).

use std::path::{Path, PathBuf};

use agent_loadout_core::local_settings::LOCAL_SETTINGS_FILE;
use agent_loadout_core::remote::config::REMOTES_FILE;

use super::history::{self, HistoryFilter, HistoryScope};
use super::remotes::{self, FolderModeDto, RestoreFromKind, SourceRef};
use super::secrets::{self, ProviderKindDto, ProviderStateDto, SecretScope, SecretStatusKind};
use super::tests::{Fx, draft};
use super::{apply, servers};

const SECRET_VALUE: &str = "fake-keyring-value-0001";

fn all() -> HistoryFilter {
    HistoryFilter {
        scope: HistoryScope::All,
        entity: None,
        limit: None,
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else if std::fs::read(&target).ok() != Some(std::fs::read(entry.path()).unwrap()) {
            // Git objects are read-only and never change: identical files
            // are left alone, as a sync tool would.
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A folder for a folder remote inside the fixture's temp root.
fn backup_folder(fx: &Fx, name: &str) -> PathBuf {
    let folder = fx.th.root().join(name);
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

#[test]
fn tag_change_restore_the_tag_and_apply() {
    let fx = Fx::new();
    servers::save(&fx.core, &draft("fake", &["claude-code"]), true).unwrap();
    let original = fx.read(&fx.claude());
    assert!(original.contains("fake-mcp"), "{original}");

    let tag = history::tag(&fx.core, "pre-change", None, Some("before the change")).unwrap();
    assert_eq!(tag.name, "pre-change");
    // A tag name outside `^[A-Za-z0-9._-]{1,64}$` is refused.
    assert!(history::tag(&fx.core, "bad name", None, None).is_err());

    let mut changed = draft("fake", &["claude-code"]);
    changed.args = vec!["-y".to_owned(), "fake-mcp-changed".to_owned()];
    servers::save(&fx.core, &changed, false).unwrap();
    assert!(fx.read(&fx.claude()).contains("fake-mcp-changed"));

    // The list shows the tag on the commit it names, and the change on top.
    let entries = history::history(&fx.core, &all()).unwrap();
    let tagged = entries
        .iter()
        .find(|entry| entry.tags.contains(&"pre-change".to_owned()))
        .expect("the tagged commit is listed");
    assert_eq!(tagged.id, tag.commit);
    assert_ne!(entries[0].id, tag.commit, "the change is newer");
    assert!(
        entries[0]
            .apply
            .as_ref()
            .is_some_and(|a| a.undoable && a.latest)
    );

    // Restore this version: a preview first, then one commit, no apply.
    let preview = history::preview(&fx.core, "pre-change", &[]).unwrap();
    assert_eq!(preview.changed, vec!["server/fake".to_owned()]);
    assert!(preview.added.is_empty() && preview.removed.is_empty());
    let before = fx.commits();
    let restored = history::restore_version(&fx.core, "pre-change", &[], false).unwrap();
    assert_eq!(fx.commits(), before + 1, "a restore is one commit");
    assert!(
        restored
            .tag
            .as_deref()
            .is_some_and(|t| t.starts_with("auto/pre-restore/"))
    );
    assert!(
        fx.read(&fx.claude()).contains("fake-mcp-changed"),
        "client files wait for the apply"
    );

    // The apply plan the page opens next writes the tagged version back.
    let applied = apply::run(&fx.core, None).unwrap();
    assert!(applied.apply.is_some_and(|run| run.files_written == 1));
    assert_eq!(fx.read(&fx.claude()), original);

    // The restore is undoable by restoring its tag.
    let entries = history::history(&fx.core, &all()).unwrap();
    assert!(entries.iter().any(|e| e.summary.starts_with("restore: ")));
}

#[test]
fn restore_entity_restores_only_that_entity() {
    let fx = Fx::new();
    servers::save(&fx.core, &draft("one", &["claude-code"]), true).unwrap();
    servers::save(&fx.core, &draft("two", &["claude-code"]), true).unwrap();
    let base = history::history(&fx.core, &all()).unwrap()[0].id.clone();
    let mut one = draft("one", &["claude-code"]);
    one.args = vec!["one-changed".to_owned()];
    servers::save(&fx.core, &one, false).unwrap();
    let mut two = draft("two", &["claude-code"]);
    two.args = vec!["two-changed".to_owned()];
    servers::save(&fx.core, &two, false).unwrap();

    let entities = vec!["server/one".to_owned()];
    let preview = history::preview(&fx.core, &base, &entities).unwrap();
    assert_eq!(preview.changed, entities);
    history::restore_version(&fx.core, &base, &entities, false).unwrap();
    apply::run(&fx.core, None).unwrap();
    let text = fx.read(&fx.claude());
    assert!(!text.contains("one-changed"), "{text}");
    assert!(text.contains("two-changed"), "{text}");
}

#[test]
fn undo_this_apply_from_history_is_one_click_for_the_latest() {
    let fx = Fx::new();
    servers::save(&fx.core, &draft("fake", &["claude-code"]), true).unwrap();
    let mut changed = draft("fake", &["claude-code"]);
    changed.args = vec!["fake-mcp-changed".to_owned()];
    servers::save(&fx.core, &changed, false).unwrap();
    let entries = history::history(&fx.core, &all()).unwrap();
    let latest = entries[0].apply.clone().expect("an apply commit");
    let older = entries[1].apply.clone().expect("an apply commit");
    assert!(latest.latest && latest.undoable);
    assert!(!older.latest && older.undoable);

    // The commit detail lists the files the apply touched.
    let detail = history::detail(&fx.core, &entries[0].id).unwrap();
    let applied = detail.applied.expect("apply detail");
    assert_eq!(applied.targets.len(), 1);
    assert_eq!(applied.targets[0].client, "claude-code");
    assert_eq!(applied.targets[0].file, "~/.claude.json");
    let change = detail
        .changes
        .iter()
        .find(|c| c.entity == "server/fake")
        .expect("the server changed");
    assert!(
        change.fields.iter().any(|f| f.field == "args"),
        "{change:?}"
    );

    // Undoing an older apply whose entries a later apply touched asks first.
    let err = apply::undo(&fx.core, &older.apply_id, false).unwrap_err();
    assert_eq!(
        err.code,
        agent_loadout_core::ErrorCode::ConfirmationRequired
    );
    assert!(
        fx.read(&fx.claude()).contains("fake-mcp-changed"),
        "nothing changed"
    );

    // The latest one never asks.
    apply::undo(&fx.core, &latest.apply_id, false).unwrap();
    let text = fx.read(&fx.claude());
    assert!(
        text.contains("\"fake-mcp\"") && !text.contains("fake-mcp-changed"),
        "{text}"
    );
}

#[test]
fn client_file_backups_restore_one_file() {
    let fx = Fx::new();
    std::fs::write(fx.claude(), "{\n  \"theme\": \"dark\"\n}\n").unwrap();
    let saved = servers::save(&fx.core, &draft("fake", &["claude-code"]), true).unwrap();
    let apply_id = saved.apply.unwrap().apply_id;

    let runs = history::file_backups(&fx.core).unwrap();
    let run = runs
        .iter()
        .find(|r| r.apply_id == apply_id)
        .expect("the run");
    assert_eq!(run.files, vec!["~/.claude.json".to_owned()]);

    let results = history::restore_files(&fx.core, &apply_id, Some("~/.claude.json")).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, history::FileRestoreOutcome::Restored);
    assert_eq!(fx.read(&fx.claude()), "{\n  \"theme\": \"dark\"\n}\n");
    assert!(history::restore_files(&fx.core, &apply_id, Some("~/other.json")).is_err());
}

#[test]
fn not_now_leaves_the_store_local_only() {
    let fx = Fx::new();
    let state = fx.core.home.state_dir();
    let overview = remotes::overview(&fx.core).unwrap();
    assert!(overview.remotes.is_empty(), "no remote exists by default");
    assert!(overview.offer_backup_remote, "the first run offers one");

    remotes::dismiss_offer(&fx.core).unwrap();
    let overview = remotes::overview(&fx.core).unwrap();
    assert!(!overview.offer_backup_remote, "asked once");
    assert!(
        overview.remotes.is_empty(),
        "Settings shows No backup remote"
    );
    assert!(
        !state.join(REMOTES_FILE).exists(),
        "remotes.toml is not written"
    );
    assert!(state.join(LOCAL_SETTINGS_FILE).exists());

    // Adding one from Settings later works as from the step.
    let folder = backup_folder(&fx, "usb");
    remotes::add_folder(
        &fx.core,
        "usb",
        &folder,
        FolderModeDto::RepoPerMachine,
        false,
    )
    .unwrap();
    assert_eq!(remotes::overview(&fx.core).unwrap().remotes.len(), 1);
}

#[test]
fn adding_from_the_first_run_step_answers_it() {
    let fx = Fx::new();
    let folder = backup_folder(&fx, "nas");
    remotes::add_folder(&fx.core, "nas", &folder, FolderModeDto::Mirror, true).unwrap();
    remotes::remove(&fx.core, "nas").unwrap();
    let overview = remotes::overview(&fx.core).unwrap();
    assert!(overview.remotes.is_empty());
    assert!(!overview.offer_backup_remote, "the step is not shown again");
}

#[test]
fn a_remote_folder_inside_the_data_folder_is_refused() {
    let fx = Fx::new();
    drop(fx.store());
    let inside = fx.core.home.path.join("backups-here");
    std::fs::create_dir_all(&inside).unwrap();
    let err = remotes::add_folder(
        &fx.core,
        "self",
        &inside,
        FolderModeDto::RepoPerMachine,
        false,
    )
    .unwrap_err();
    assert_eq!(err.code, agent_loadout_core::ErrorCode::InvalidPath);
    let err = remotes::add_folder(
        &fx.core,
        "relative",
        Path::new("relative/folder"),
        FolderModeDto::RepoPerMachine,
        false,
    )
    .unwrap_err();
    assert_eq!(err.code, agent_loadout_core::ErrorCode::InvalidPath);
    assert!(remotes::overview(&fx.core).unwrap().remotes.is_empty());
}

#[test]
fn folder_remote_status_follows_pushes_and_reachability() {
    let fx = Fx::new();
    servers::save(&fx.core, &draft("fake", &["claude-code"]), true).unwrap();
    let folder = backup_folder(&fx, "usb");
    let added = remotes::add_folder(
        &fx.core,
        "usb",
        &folder,
        FolderModeDto::RepoPerMachine,
        false,
    )
    .unwrap();
    let status = &added.remote.status;
    assert_eq!(status.reachable, Some(true));
    assert_eq!(status.ahead, None, "never pushed");
    assert!(
        added.remote.location.ends_with("usb"),
        "{}",
        added.remote.location
    );

    let pushes = remotes::push(&fx.core, None).unwrap();
    assert!(pushes[0].ok, "{:?}", pushes[0].error);
    let status = remotes::overview(&fx.core).unwrap().remotes[0]
        .status
        .clone();
    assert_eq!((status.ahead, status.behind), (Some(0), Some(0)), "in sync");
    assert!(status.pushed_at.is_some());
    assert!(status.last_error.is_none());

    // A local change: one commit ahead.
    servers::save(&fx.core, &draft("other", &["claude-code"]), true).unwrap();
    let status = remotes::overview(&fx.core).unwrap().remotes[0]
        .status
        .clone();
    assert_eq!(status.ahead, Some(1));

    // The drive is unplugged: unreachable, and the push error is kept.
    let away = fx.th.root().join("usb-unplugged");
    std::fs::rename(&folder, &away).unwrap();
    let status = remotes::overview(&fx.core).unwrap().remotes[0]
        .status
        .clone();
    assert_eq!(status.reachable, Some(false));
    let pushes = remotes::push(&fx.core, Some("usb")).unwrap();
    assert!(!pushes[0].ok);
    let status = remotes::overview(&fx.core).unwrap().remotes[0]
        .status
        .clone();
    let last = status.last_error.expect("the failure is recorded");
    assert_eq!(
        last.error.code,
        agent_loadout_core::ErrorCode::RemoteUnreachable
    );
    // Local work goes on.
    servers::save(&fx.core, &draft("third", &["claude-code"]), true).unwrap();

    // Plugged in again: the next push clears the error.
    std::fs::rename(&away, &folder).unwrap();
    assert!(remotes::push(&fx.core, Some("usb")).unwrap()[0].ok);
    let status = remotes::overview(&fx.core).unwrap().remotes[0]
        .status
        .clone();
    assert!(status.last_error.is_none());
    assert_eq!(status.ahead, Some(0));
}

/// Machine A with a server that needs a keyring secret, pushed to a folder
/// remote; returns the remote folder.
fn machine_with_secret(fx: &Fx) -> PathBuf {
    fx.keyring.insert("loadout", "FAKE_TOKEN", SECRET_VALUE);
    fx.stage(&[(
        "secrets.toml",
        "schema = 1\n\n[secrets.FAKE_TOKEN]\nref = \"keyring://loadout/FAKE_TOKEN\"\n",
    )]);
    let mut server = draft("needs-secret", &["claude-code"]);
    server.env.push(servers::KeyValue {
        key: "API_TOKEN".to_owned(),
        value: "${secret:FAKE_TOKEN}".to_owned(),
    });
    servers::save(&fx.core, &server, true).unwrap();
    let folder = backup_folder(fx, "remote");
    remotes::add_folder(
        &fx.core,
        "usb",
        &folder,
        FolderModeDto::RepoPerMachine,
        false,
    )
    .unwrap();
    assert!(remotes::push(&fx.core, None).unwrap()[0].ok);
    folder
}

#[test]
fn restore_from_remote_lists_machines_and_shows_missing_secrets() {
    // Two machines push to the same folder remote.
    let desk = Fx::on_host("desk");
    let remote = machine_with_secret(&desk);
    let laptop = Fx::on_host("laptop");
    let carried = laptop.th.root().join("remote");
    copy_dir(&remote, &carried);
    let listing = remotes::machines(&laptop.core, &SourceRef::Folder(carried.clone())).unwrap();
    assert_eq!(listing.machines.len(), 1);
    let desk_id = listing.machines[0].machine_id.clone();
    remotes::restore(&laptop.core, &SourceRef::Folder(carried.clone()), None).unwrap();
    remotes::add_folder(
        &laptop.core,
        "usb",
        &carried,
        FolderModeDto::RepoPerMachine,
        false,
    )
    .unwrap();
    servers::save(&laptop.core, &draft("laptop-only", &["claude-code"]), true).unwrap();
    assert!(remotes::push(&laptop.core, None).unwrap()[0].ok);

    // The desk sees the laptop's commits as incoming.
    copy_dir(&carried, &remote);
    let status = remotes::overview(&desk.core).unwrap().remotes[0]
        .status
        .clone();
    assert!(status.behind.is_some_and(|n| n >= 1), "{status:?}");

    // A fresh machine lists both and restores the one picked.
    let fresh = Fx::on_host("fresh");
    let folder = fresh.th.root().join("remote");
    copy_dir(&carried, &folder);
    let source = SourceRef::Folder(folder.clone());
    let listing = remotes::machines(&fresh.core, &source).unwrap();
    assert!(!listing.direct);
    let mut labels: Vec<&str> = listing.machines.iter().map(|m| m.label.as_str()).collect();
    labels.sort_unstable();
    assert_eq!(labels, ["desk", "laptop"]);
    assert!(
        listing
            .machines
            .iter()
            .all(|m| !m.same_store && !m.this_machine)
    );
    let desk_row = listing
        .machines
        .iter()
        .find(|m| m.machine_id == desk_id)
        .unwrap();
    assert!(desk_row.subject.is_some());

    // Without a pick the restore is refused; with one, the store is created.
    assert!(remotes::restore(&fresh.core, &source, None).is_err());
    assert!(fresh.core.open_store().unwrap().is_none());
    let result = remotes::restore(&fresh.core, &source, Some(&desk_id)).unwrap();
    assert_eq!(result.kind, RestoreFromKind::Created);
    assert!(result.from.contains("desk"), "{}", result.from);
    assert!(
        result.apply.is_none(),
        "client files wait for the apply plan"
    );
    let list = servers::list(&fresh.core).unwrap();
    assert!(list.servers.iter().any(|s| s.id == "needs-secret"));
    assert!(
        list.servers.iter().all(|s| s.id != "laptop-only"),
        "the desk's store"
    );

    // The missing-secret checklist: the keyring entry is on the desk only.
    let checks = secrets::check(&fresh.core, &[]).unwrap();
    let check = checks
        .iter()
        .find(|c| c.alias == "FAKE_TOKEN")
        .expect("listed");
    assert_eq!(check.status, SecretStatusKind::Missing);
    assert_eq!(check.needed_by.len(), 1);
    assert_eq!(check.needed_by[0].server, "needs-secret");
    assert_eq!(check.needed_by[0].clients, vec!["claude-code".to_owned()]);
    assert!(check.blocking);

    // Quick fix: the value into this machine's keyring, one commit.
    let before = fresh.commits();
    let change = secrets::set_keyring_value(
        &fresh.core,
        "FAKE_TOKEN",
        SECRET_VALUE,
        SecretScope::ThisMachine,
    )
    .unwrap();
    assert_eq!(fresh.commits(), before + 1);
    let checks = secrets::check(&fresh.core, &["FAKE_TOKEN".to_owned()]).unwrap();
    assert_eq!(checks[0].status, SecretStatusKind::Resolved);
    let json = serde_json::to_string(&(change, checks)).unwrap();
    assert!(!json.contains(SECRET_VALUE), "no value crosses IPC: {json}");
}

#[test]
fn restore_from_a_configured_remote_into_an_existing_store_merges() {
    let desk = Fx::on_host("desk");
    let remote = machine_with_secret(&desk);
    let laptop = Fx::on_host("laptop");
    let carried = laptop.th.root().join("remote");
    copy_dir(&remote, &carried);
    remotes::restore(&laptop.core, &SourceRef::Folder(carried.clone()), None).unwrap();
    servers::save(&desk.core, &draft("later", &["claude-code"]), true).unwrap();
    assert!(remotes::push(&desk.core, None).unwrap()[0].ok);
    copy_dir(&remote, &carried);
    remotes::add_folder(
        &laptop.core,
        "usb",
        &carried,
        FolderModeDto::RepoPerMachine,
        false,
    )
    .unwrap();

    let source = SourceRef::Remote("usb".to_owned());
    let listing = remotes::machines(&laptop.core, &source).unwrap();
    let desk_row = listing.machines.iter().find(|m| m.label == "desk").unwrap();
    assert!(desk_row.same_store);
    let result = remotes::restore(&laptop.core, &source, Some(&desk_row.machine_id)).unwrap();
    assert_eq!(result.kind, RestoreFromKind::Merged);
    assert!(
        servers::list(&laptop.core)
            .unwrap()
            .servers
            .iter()
            .any(|s| s.id == "later")
    );
    // A git remote holds no machine folders.
    let err = remotes::machines(&laptop.core, &SourceRef::Remote("nope".to_owned())).unwrap_err();
    assert_eq!(err.code, agent_loadout_core::ErrorCode::NotFound);
}

#[test]
fn without_the_1password_cli_the_provider_is_not_found() {
    let fx = Fx::new();
    let providers = secrets::provider_list(&fx.core);
    let op = providers
        .iter()
        .find(|p| p.kind == ProviderKindDto::Onepassword)
        .unwrap();
    assert_eq!(op.state, ProviderStateDto::Unavailable);
    assert!(op.features.iter().all(|f| !f.available));
    let keyring = providers
        .iter()
        .find(|p| p.kind == ProviderKindDto::Keyring)
        .unwrap();
    assert_eq!(keyring.state, ProviderStateDto::Available);

    // op:// aliases are provider_unavailable on this machine only; a keyring
    // reference for this machine fixes it.
    fx.stage(&[(
        "secrets.toml",
        "schema = 1\n\n[secrets.FAKE_OP]\nref = \"op://Fake/Item/credential\"\n",
    )]);
    let checks = secrets::check(&fx.core, &[]).unwrap();
    assert_eq!(checks[0].status, SecretStatusKind::ProviderUnavailable);
    secrets::set_keyring_value(&fx.core, "FAKE_OP", SECRET_VALUE, SecretScope::ThisMachine)
        .unwrap();
    let overview = secrets::overview(&fx.core).unwrap();
    let alias = &overview.aliases[0];
    assert_eq!(alias.reference, "keyring://loadout/FAKE_OP");
    assert_eq!(
        alias.store_reference.as_deref(),
        Some("op://Fake/Item/credential"),
        "the shared reference is unchanged"
    );
    assert_eq!(
        secrets::check(&fx.core, &[]).unwrap()[0].status,
        SecretStatusKind::Resolved
    );
    // Mapping an Environment needs the CLI.
    assert!(secrets::environment_names(&fx.core, "fakeenvironmentid0000000000").is_err());
}

#[test]
fn environment_variable_fix_and_service_account() {
    let fx = Fx::new();
    secrets::set_reference(
        &fx.core,
        "FAKE_ENV",
        "env:FAKE_ENV_VAR",
        SecretScope::Store,
        None,
    )
    .unwrap();
    let checks = secrets::check(&fx.core, &[]).unwrap();
    assert_eq!(checks[0].reference.as_deref(), Some("env:FAKE_ENV_VAR"));
    assert!(
        secrets::set_reference(&fx.core, "bad alias", "env:X", SecretScope::Store, None).is_err()
    );

    assert!(!secrets::overview(&fx.core).unwrap().service_account_set);
    secrets::set_service_account(&fx.core, "fake-service-token").unwrap();
    assert!(secrets::overview(&fx.core).unwrap().service_account_set);
    assert!(secrets::clear_service_account(&fx.core).unwrap());
    assert!(!secrets::overview(&fx.core).unwrap().service_account_set);
}

#[test]
fn git_remote_urls_are_masked_and_tokens_go_one_way() {
    let fx = Fx::new();
    let added = remotes::add_git(
        &fx.core,
        "origin",
        "https://user:fake-password@git.example.invalid/store.git",
        remotes::GitAuthDto::Token,
        Some("someone"),
        false,
    )
    .unwrap();
    assert!(!added.remote.location.contains("fake-password"));
    assert_eq!(added.remote.token_stored, Some(false));
    remotes::set_token(&fx.core, "origin", "fake-token-value").unwrap();
    let overview = remotes::overview(&fx.core).unwrap();
    assert_eq!(overview.remotes[0].token_stored, Some(true));
    let json = serde_json::to_string(&overview).unwrap();
    assert!(!json.contains("fake-token-value") && !json.contains("fake-password"));
    let removed = remotes::remove(&fx.core, "origin").unwrap();
    assert!(removed.token_deleted);
    // gh auth only for GitHub URLs.
    assert!(
        remotes::add_git(
            &fx.core,
            "other",
            "https://git.example.invalid/x.git",
            remotes::GitAuthDto::Gh,
            None,
            false
        )
        .is_err()
    );
}

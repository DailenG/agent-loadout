//! Execution and apply commits (SPEC 8.4, 8.6, 8.7, 9.2): compare-and-swap
//! re-plans, zero-write re-applies, byte-identical foreign content,
//! silent adoption, one commit per run with `Loadout-Apply`, user-only
//! backups, journals and imports.

mod support;

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agent_loadout_core::adapters::detect::NoRegistry;
use agent_loadout_core::apply::backup::{self, encode_path};
use agent_loadout_core::apply::import;
use agent_loadout_core::apply::journal;
use agent_loadout_core::apply::ledger::Origin;
use agent_loadout_core::apply::render::DesiredState;
use agent_loadout_core::apply::{
    self, Action, ApplyEnv, ApplyRequest, EntryState, OnConflict, PlanOptions,
};
use agent_loadout_core::model::TomlDocument;
use agent_loadout_core::model::applied::{AppliedRecord, ApplyOutcome, ApplyTrigger};
use agent_loadout_core::store::Operation;
use agent_loadout_core::{ErrorCode, Fs};
use agent_loadout_testkit::RootedFs;
use support::{Fx, json_entry, result_entry, stdio};

const NOTES_ARGS: [&str; 2] = ["-y", "@example/notes-mcp"];
const CLAUDE_FIXTURE: &str =
    include_str!("../../../fixtures/clients/claude-code/user/.claude.json");

static NO_REGISTRY: NoRegistry = NoRegistry;

fn files_in(dir: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            let mut paths: Vec<PathBuf> = entries.map(|e| e.unwrap().path()).collect();
            paths.sort();
            paths
        }
        Err(_) => Vec::new(),
    }
}

#[test]
fn second_apply_makes_zero_writes() {
    let fx = Fx::new();
    fx.write(&fx.claude(), CLAUDE_FIXTURE);
    fx.put_server(stdio("loadout-notes", &["claude-code", "omp"], &NOTES_ARGS));
    let (first, commit) = fx.apply();
    assert_eq!(first.outcome, ApplyOutcome::Ok);
    assert_eq!(first.writes(), 2);
    assert!(commit.is_some());

    let claude = std::fs::read(fx.claude()).unwrap();
    let omp = std::fs::read(fx.omp()).unwrap();
    let ledger = fx.ledger();
    let commits = fx.commit_count();
    let backups = files_in(&fx.home.backups_dir().join(backup::FILES_DIR));

    let (second, commit) = fx.apply();
    assert_eq!(second.writes(), 0);
    assert!(
        commit.is_none(),
        "an apply with zero writes makes no commit"
    );
    assert!(!second.recorded);
    assert_eq!(fx.commit_count(), commits);
    assert_eq!(std::fs::read(fx.claude()).unwrap(), claude);
    assert_eq!(std::fs::read(fx.omp()).unwrap(), omp);
    assert_eq!(fx.ledger(), ledger);
    assert_eq!(
        files_in(&fx.home.backups_dir().join(backup::FILES_DIR)),
        backups
    );
    for target in &second.targets {
        assert!(
            target.entries.iter().all(|e| e.action == Action::None),
            "{target:#?}"
        );
    }
    // No temp files are left next to the targets.
    for dir in [fx.th.home(), fx.omp().parent().unwrap().to_path_buf()] {
        assert!(
            files_in(&dir)
                .iter()
                .all(|p| !p.to_string_lossy().ends_with(".tmp")),
            "{:?}",
            files_in(&dir)
        );
    }
}

#[test]
fn foreign_entries_and_unrelated_keys_stay_byte_identical() {
    let fx = Fx::new();
    fx.write(&fx.claude(), CLAUDE_FIXTURE);
    let before: serde_json::Value = serde_json::from_str(CLAUDE_FIXTURE).unwrap();

    fx.put_server(stdio("loadout-notes", &["claude-code"], &NOTES_ARGS));
    fx.apply();
    let created = fx.read(&fx.claude());
    let mut after: serde_json::Value = serde_json::from_str(&created).unwrap();
    assert!(after["mcpServers"]["loadout-notes"].is_object());
    after["mcpServers"]
        .as_object_mut()
        .unwrap()
        .remove("loadout-notes");
    assert_eq!(after, before, "only the owned entry was added");
    // The original text is a prefix-and-suffix of the new text: nothing
    // else was reformatted.
    let tail = CLAUDE_FIXTURE.rfind("\n  }").unwrap();
    assert!(created.starts_with(&CLAUDE_FIXTURE[..CLAUDE_FIXTURE.find("\"mcpServers\"").unwrap()]));
    assert!(created.ends_with(&CLAUDE_FIXTURE[tail..]));

    fx.put_server(stdio(
        "loadout-notes",
        &["claude-code"],
        &["-y", "@example/notes-mcp", "--v2"],
    ));
    fx.apply();
    fx.put_server(stdio("loadout-notes", &[], &NOTES_ARGS));
    fx.apply();
    assert_eq!(
        fx.read(&fx.claude()),
        CLAUDE_FIXTURE,
        "create, update, remove: original bytes"
    );
    assert!(fx.ledger().entries.is_empty());
}

#[test]
fn identical_entries_are_adopted_without_a_write() {
    let fx = Fx::new();
    let text = "{\n  \"mcpServers\": {\n    \"notes\": {\n      \"type\": \"stdio\",\n      \"command\": \"npx\",\n      \"args\": [\"-y\", \"@example/notes-mcp\"]\n    }\n  }\n}\n";
    fx.write(&fx.claude(), text);
    fx.put_server(stdio("notes", &["claude-code"], &NOTES_ARGS));
    let commits = fx.commit_count();
    let (report, commit) = fx.apply();
    assert_eq!(report.outcome, ApplyOutcome::Ok);
    assert_eq!(
        result_entry(&report, &fx.claude(), "notes").action,
        Action::Adopt
    );
    assert!(commit.is_none());
    assert_eq!(fx.commit_count(), commits);
    assert_eq!(fx.read(&fx.claude()), text);
    let ledger = fx.ledger();
    assert_eq!(ledger.entries.len(), 1);
    assert_eq!(ledger.entries[0].origin, Some(Origin::Adopted));
    assert_eq!(ledger.entries[0].container, "/mcpServers");
    assert!(
        ledger.entries[0]
            .written_hash
            .as_ref()
            .unwrap()
            .starts_with("hmac-sha256:")
    );

    // Owned now: a store change updates it, and the origin stays.
    fx.put_server(stdio(
        "notes",
        &["claude-code"],
        &["-y", "@example/notes-mcp", "--v2"],
    ));
    let (report, _) = fx.apply();
    assert_eq!(
        result_entry(&report, &fx.claude(), "notes").action,
        Action::Update
    );
    assert_eq!(fx.ledger().entries[0].origin, Some(Origin::Adopted));
}

#[test]
fn each_apply_run_is_one_commit_with_trailers_and_an_apply_record() {
    let fx = Fx::new();
    fx.put_server(stdio("notes", &["claude-code", "omp"], &NOTES_ARGS));
    let base = fx.store().head().unwrap();
    let commits = fx.commit_count();
    let (report, commit) = fx.apply();
    let commit = commit.unwrap();
    assert_eq!(fx.commit_count(), commits + 1);

    let log = fx.log();
    let head = &log[0];
    assert_eq!(head.id, commit);
    assert_eq!(head.summary, "apply: 2 files (claude-code, omp)");
    let machine = fx.machine_id();
    assert_eq!(head.trailers.op.as_deref(), Some("apply"));
    // The record's path finds it under `machine/<id>`; the trailers name
    // only what the user changed.
    assert!(
        head.trailers.entities.is_empty(),
        "{:?}",
        head.trailers.entities
    );
    let apply = head.trailers.apply.as_ref().unwrap();
    assert_eq!(
        (apply.id.as_str(), apply.outcome),
        (report.apply_id.as_str(), ApplyOutcome::Ok)
    );

    let text = fx
        .store()
        .read_to_string(&format!("machines/{machine}.applied.toml"))
        .unwrap()
        .unwrap();
    let record = TomlDocument::<AppliedRecord>::parse(&text)
        .unwrap()
        .into_value();
    assert!(record.validate(Some(&machine)).is_valid());
    assert_eq!(record.apply_id, report.apply_id);
    assert_eq!(record.trigger, ApplyTrigger::Review);
    assert_eq!(record.outcome, ApplyOutcome::Ok);
    assert_eq!(record.store_revision, base.as_str());
    let files: Vec<(&str, &str, u32)> = record
        .targets
        .iter()
        .map(|t| (t.client.as_str(), t.file.as_str(), t.create))
        .collect();
    assert_eq!(
        files,
        vec![
            ("claude-code", "~/.claude.json", 1),
            ("omp", "~/.omp/agent/mcp.json", 1)
        ]
    );
    assert!(
        !text.contains(&fx.th.root().to_string_lossy().into_owned()),
        "no personal paths"
    );

    // Instant mode: the edit and its apply are one commit.
    let commits = fx.commit_count();
    let (report, commit) = fx.instant("server: add docs", |tx| {
        tx.add_server(stdio(
            "docs",
            &["claude-code"],
            &["-y", "@example/docs-mcp"],
        ))
    });
    assert!(commit.is_some());
    assert_eq!(fx.commit_count(), commits + 1);
    let head = &fx.log()[0];
    assert_eq!(head.summary, "server: add docs");
    assert_eq!(head.trailers.op.as_deref(), Some("test.edit"));
    assert_eq!(head.trailers.apply.as_ref().unwrap().id, report.apply_id);
    assert_eq!(report.trigger, ApplyTrigger::Instant);
    assert!(json_entry(&fx.read(&fx.claude()), "docs").is_some());
    let text = fx
        .store()
        .read_to_string(&format!("machines/{machine}.applied.toml"))
        .unwrap()
        .unwrap();
    assert!(text.contains("trigger = \"instant\""), "{text}");
}

#[test]
fn a_failed_file_does_not_stop_the_others() {
    let fx = Fx::new();
    fx.write(&fx.claude(), "{ \"mcpServers\": { broken\n");
    fx.put_server(stdio("notes", &["claude-code", "omp"], &NOTES_ARGS));
    let (report, commit) = fx.apply();
    assert_eq!(report.outcome, ApplyOutcome::Partial);
    let claude = report
        .targets
        .iter()
        .find(|t| t.file == fx.claude())
        .unwrap();
    assert_eq!(claude.outcome, ApplyOutcome::Blocked);
    assert_eq!(claude.error.as_ref().unwrap().code(), ErrorCode::ParseError);
    assert_eq!(fx.read(&fx.claude()), "{ \"mcpServers\": { broken\n");
    let omp = report.targets.iter().find(|t| t.file == fx.omp()).unwrap();
    assert_eq!(omp.outcome, ApplyOutcome::Ok);
    assert!(commit.is_some());
    let apply = fx.log()[0].trailers.apply.clone().unwrap();
    assert_eq!(apply.outcome, ApplyOutcome::Partial);
}

/// A file system that edits the target behind the apply's back.
struct RacyFs {
    inner: RootedFs,
    target: PathBuf,
    trigger: Trigger,
    reads: Mutex<usize>,
    injected: Mutex<usize>,
}

#[derive(Clone, Copy, PartialEq)]
enum Trigger {
    /// Before the second read (the executor's re-read after planning).
    SecondRead,
    /// Before every read after the first.
    EveryRead,
    /// When the temp file is written (between re-read and replace).
    TempWrite,
}

impl RacyFs {
    fn inject(&self) {
        let mut injected = self.injected.lock().unwrap();
        *injected += 1;
        let text = std::fs::read_to_string(&self.target).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value["mcpServers"][format!("intruder-{injected}")] =
            serde_json::json!({"type": "stdio", "command": "intruder"});
        std::fs::write(&self.target, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }
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
            let reads = {
                let mut reads = self.reads.lock().unwrap();
                *reads += 1;
                *reads
            };
            let inject = match self.trigger {
                Trigger::SecondRead => reads == 2,
                Trigger::EveryRead => reads >= 2,
                Trigger::TempWrite => false,
            };
            if inject {
                self.inject();
            }
        }
        self.inner.read(path)
    }
    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.inner.write(path, contents)
    }
    fn write_new(&self, path: &Path, contents: &[u8], like: Option<&Path>) -> io::Result<()> {
        self.inner.write_new(path, contents, like)?;
        let temp_of_target = path.parent() == self.target.parent()
            && path.to_string_lossy().ends_with(".tmp")
            && path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("..claude.json.loadout-"));
        if self.trigger == Trigger::TempWrite
            && temp_of_target
            && *self.injected.lock().unwrap() == 0
        {
            self.inject();
        }
        Ok(())
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

fn racy_apply(fx: &Fx, trigger: Trigger) -> apply::ApplyReport {
    let racy = RacyFs {
        inner: fx.th.fs(),
        target: fx.claude(),
        trigger,
        reads: Mutex::new(0),
        injected: Mutex::new(0),
    };
    let ax = ApplyEnv {
        env: &fx.env,
        fs: &racy,
        home: &fx.home,
        registry: &NO_REGISTRY,
    };
    let request = ApplyRequest {
        options: PlanOptions::default(),
        trigger: ApplyTrigger::Review,
    };
    apply::run(&fx.store(), &ax, &request).unwrap().0
}

#[test]
fn cas_race_replans_against_the_new_bytes() {
    for trigger in [Trigger::SecondRead, Trigger::TempWrite] {
        let fx = Fx::new();
        fx.write(&fx.claude(), "{\n  \"mcpServers\": {}\n}\n");
        fx.put_server(stdio("notes", &["claude-code"], &NOTES_ARGS));
        let report = racy_apply(&fx, trigger);
        assert_eq!(report.outcome, ApplyOutcome::Ok, "{report:#?}");
        let text = fx.read(&fx.claude());
        assert!(
            json_entry(&text, "intruder-1").is_some(),
            "the concurrent edit survives: {text}"
        );
        assert!(json_entry(&text, "notes").is_some(), "{text}");
        let notes = result_entry(&report, &fx.claude(), "notes");
        assert_eq!(notes.action, Action::Create);
        let intruder = result_entry(&report, &fx.claude(), "intruder-1");
        assert_eq!(
            intruder.state,
            EntryState::Foreign,
            "re-planned with the new entry"
        );
        assert!(
            files_in(&fx.th.home())
                .iter()
                .all(|p| !p.to_string_lossy().ends_with(".tmp")),
            "temp files are removed"
        );
    }
}

#[test]
fn a_file_that_keeps_changing_fails_alone() {
    let fx = Fx::new();
    fx.write(&fx.claude(), "{\n  \"mcpServers\": {}\n}\n");
    fx.put_server(stdio("notes", &["claude-code", "omp"], &NOTES_ARGS));
    let report = racy_apply(&fx, Trigger::EveryRead);
    assert_eq!(report.outcome, ApplyOutcome::Partial);
    let claude = report
        .targets
        .iter()
        .find(|t| t.file == fx.claude())
        .unwrap();
    assert_eq!(claude.outcome, ApplyOutcome::Failed);
    assert_eq!(
        claude.error.as_ref().unwrap().code(),
        ErrorCode::FileChangedDuringApply
    );
    assert!(json_entry(&fx.read(&fx.claude()), "notes").is_none());
    assert!(json_entry(&fx.read(&fx.omp()), "notes").is_some());
    assert!(fx.ledger().entries.iter().all(|e| e.client == "omp"));
}

#[test]
fn backups_are_written_with_user_only_permissions() {
    let fx = Fx::new();
    fx.write(&fx.claude(), CLAUDE_FIXTURE);
    fx.put_server(stdio("notes-two", &["claude-code"], &NOTES_ARGS));
    let (report, _) = fx.apply();
    let target = &report.targets[0];
    let backup = target
        .backup
        .clone()
        .expect("an existing file is backed up");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), CLAUDE_FIXTURE);
    assert_eq!(
        backup.file_name().unwrap().to_string_lossy(),
        encode_path(&fx.claude())
    );
    assert_eq!(
        backup::decode_path(&encode_path(&fx.claude())).unwrap(),
        fx.claude().to_string_lossy()
    );
    let run = backup.parent().unwrap();
    assert!(
        run.file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(&format!("-{}", report.apply_id))
    );
    assert!(run.starts_with(fx.home.backups_dir().join("files")));
    for path in [
        backup.as_path(),
        run,
        &fx.home.state_dir().join("ledger.key"),
    ] {
        assert_user_only(path);
    }
    assert_eq!(
        std::fs::read(fx.home.state_dir().join("ledger.key"))
            .unwrap()
            .len(),
        32
    );
}

#[cfg(unix)]
fn assert_user_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let expected = if path.is_dir() { 0o700 } else { 0o600 };
    assert_eq!(mode, expected, "{}", path.display());
}

#[cfg(windows)]
fn assert_user_only(path: &Path) {
    use windows_permissions::constants::{SeObjectType, SecurityInformation};
    use windows_permissions::wrappers;
    let sd = wrappers::GetNamedSecurityInfo(
        path.as_os_str(),
        SeObjectType::SE_FILE_OBJECT,
        SecurityInformation::Dacl | SecurityInformation::Owner,
    )
    .unwrap();
    let sddl = sd.as_sddl().unwrap().to_string_lossy().into_owned();
    let dacl = sd.dacl().unwrap();
    assert_eq!(dacl.len(), 1, "{}: {sddl}", path.display());
    assert_eq!(dacl.get_ace(0).unwrap().sid(), sd.owner(), "{sddl}");
    let flags = &sddl[sddl.find("D:").unwrap() + 2..];
    assert!(flags.starts_with('P'), "protected DACL: {sddl}");
}

#[cfg(windows)]
#[test]
fn a_file_held_open_is_retried_then_reported_locked() {
    use std::os::windows::fs::OpenOptionsExt;
    use std::time::Duration;
    const FILE_SHARE_READ: u32 = 1;

    let fx = Fx::new();
    fx.write(&fx.claude(), "{\n  \"mcpServers\": {}\n}\n");
    fx.put_server(stdio("notes", &["claude-code"], &NOTES_ARGS));

    // Released after a moment: the replace is retried and succeeds.
    let held = File::options()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(fx.claude())
        .unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        drop(held);
    });
    let (report, _) = fx.apply();
    release.join().unwrap();
    assert_eq!(report.outcome, ApplyOutcome::Ok, "{report:#?}");

    // Held throughout: `file_locked`, and the file is untouched.
    fx.put_server(stdio(
        "notes",
        &["claude-code"],
        &["-y", "@example/notes-mcp", "--v2"],
    ));
    let before = fx.read(&fx.claude());
    let held = File::options()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(fx.claude())
        .unwrap();
    let (report, commit) = fx.apply();
    drop(held);
    assert_eq!(report.outcome, ApplyOutcome::Failed);
    assert_eq!(
        report.targets[0].error.as_ref().unwrap().code(),
        ErrorCode::FileLocked
    );
    assert!(commit.is_none());
    assert_eq!(fx.read(&fx.claude()), before);
}

#[test]
fn journals_record_every_file_and_report_unfinished_runs() {
    let fx = Fx::new();
    fx.put_server(stdio("notes", &["claude-code"], &NOTES_ARGS));
    let (report, _) = fx.apply();
    let state = fx.home.state_dir();
    assert!(journal::unfinished(&fx.fs, &state).unwrap().is_empty());
    let result = journal::result(&fx.fs, &state, &report.apply_id)
        .unwrap()
        .unwrap();
    assert!(result.finished);
    assert_eq!(result.outcome, Some(ApplyOutcome::Ok));
    assert_eq!(result.files.len(), 1);
    assert!(result.files[0].written);
    assert_eq!(result.files[0].create, 1);

    // A crash leaves the journal behind; it is reported, not replayed.
    let started = journal::Journal::start(
        &fx.fs,
        &state,
        "01JCX4A0M2N8P6Q4R2S0T8U6V4",
        ApplyTrigger::Review,
        "2026-01-01T00:00:00Z".to_owned(),
    )
    .unwrap();
    let unfinished = journal::unfinished(&fx.fs, &state).unwrap();
    assert_eq!(unfinished, vec![started]);
}

#[test]
fn import_scans_collapses_identical_entries_and_adopts_them() {
    let fx = Fx::new();
    let docs = r#"{"type": "stdio", "command": "npx", "args": ["-y", "@example/docs-mcp"], "env": {"DOCS_TOKEN": "${DOCS_TOKEN}"}}"#;
    fx.write(
        &fx.claude(),
        &format!(
            "{{\n  \"mcpServers\": {{\n    \"docs\": {docs},\n    \"chatty\": {{\"type\": \"stdio\", \"command\": \"chatty\", \"env\": {{\"LOG_LEVEL\": \"info\"}}}}\n  }}\n}}\n"
        ),
    );
    fx.write(
        &fx.omp(),
        &format!("{{\n  \"mcpServers\": {{\n    \"docs\": {docs}\n  }}\n}}\n"),
    );
    let claude_before = fx.read(&fx.claude());
    let omp_before = fx.read(&fx.omp());

    let store = fx.store();
    let ax = fx.ax();
    let desired = DesiredState::load(&store, None).unwrap();
    let (candidates, _) = import::scan(&ax, &desired, &fx.ledger(), None).unwrap();
    let docs = candidates.iter().find(|c| c.name == "docs").unwrap();
    assert!(docs.identical);
    assert_eq!(
        docs.sources.iter().map(|s| s.client).collect::<Vec<_>>(),
        ["claude-code", "omp"]
    );
    let def = docs.sources[0].def.as_ref().unwrap();
    assert_eq!(
        def.fields.env.as_ref().unwrap()["DOCS_TOKEN"],
        "${env:DOCS_TOKEN}"
    );
    let chatty = candidates.iter().find(|c| c.name == "chatty").unwrap();
    assert_eq!(
        chatty.sources[0].def.as_ref().unwrap_err().code(),
        ErrorCode::NeedsSecretExtraction
    );

    let committed = store
        .transaction(Operation::new("import", "import: 1 server"), |tx| {
            let imported = import::stage_import(tx, &candidates, None, false)?;
            let report = apply::apply_in(
                tx,
                &ax,
                &ApplyRequest {
                    options: PlanOptions::default(),
                    trigger: ApplyTrigger::Instant,
                },
            )?;
            Ok((imported, report))
        })
        .unwrap();
    let (imported, report) = committed.value;
    assert_eq!(imported.imported, vec!["docs".to_owned()]);
    assert_eq!(imported.skipped.len(), 1);
    assert_eq!(report.writes(), 0, "{report:#?}");
    assert_eq!(fx.read(&fx.claude()), claude_before);
    assert_eq!(fx.read(&fx.omp()), omp_before);
    let ledger = fx.ledger();
    assert_eq!(ledger.entries.len(), 2);
    assert!(
        ledger
            .entries
            .iter()
            .all(|e| e.origin == Some(Origin::Adopted))
    );
    let server = fx
        .store()
        .read_to_string("servers/docs.toml")
        .unwrap()
        .unwrap();
    assert!(
        server.contains("DOCS_TOKEN = \"${env:DOCS_TOKEN}\""),
        "{server}"
    );
    assert!(
        server.contains("origin = \"imported:claude-code\""),
        "{server}"
    );
}

#[test]
fn import_their_change_turns_drift_into_a_store_edit() {
    let fx = Fx::new();
    fx.put_server(stdio("notes", &["claude-code"], &NOTES_ARGS));
    fx.apply();
    let edited = fx
        .read(&fx.claude())
        .replace("notes-mcp\"", "notes-mcp\", \"--fast\"");
    fx.write(&fx.claude(), &edited);

    let (report, commit) = fx.apply_with(PlanOptions {
        on_conflict: OnConflict::Import,
        ..PlanOptions::default()
    });
    assert_eq!(report.imported, vec!["notes".to_owned()]);
    assert_eq!(
        result_entry(&report, &fx.claude(), "notes").action,
        Action::Refresh
    );
    assert!(commit.is_some(), "the store edit commits");
    assert_eq!(fx.read(&fx.claude()), edited, "the file is not written");
    let server = fx
        .store()
        .read_to_string("servers/notes.toml")
        .unwrap()
        .unwrap();
    assert!(server.contains("--fast"), "{server}");
    let head = &fx.log()[0];
    assert!(head.trailers.entities.contains(&"server/notes".to_owned()));
    assert!(head.trailers.apply.is_some());
    assert!(fx.plan().targets.iter().all(|t| !t.has_work()));
}

//! Store commands and the helpers every command shares: JSON output, error
//! reporting, apply reports and exit codes (SPEC 14).

pub mod adopt;
pub mod agent;
pub mod apply;
pub mod backup;
pub mod catalog;
pub mod diff;
pub mod effective;
pub mod env;
pub mod exec;
pub mod git_credential;
pub mod history;
pub mod import;
pub mod migrate;
pub mod project;
pub mod remote;
pub mod restore;
pub mod rollback;
pub mod scanner;
pub mod secrets;
pub mod server;
pub mod show;
pub mod skills;
pub mod source;
pub mod status;
pub mod tag;

use std::path::Path;

use agent_loadout_core::access::ReadOnlyKind;
use agent_loadout_core::adapters::detect::NoRegistry;
pub use agent_loadout_core::apply::instant_apply;
use agent_loadout_core::apply::{ApplyEnv, ApplyReport, ApplyStatus, EntryPlan, TargetResult};
use agent_loadout_core::model::Diagnostic;
use agent_loadout_core::model::applied::ApplyOutcome;
use agent_loadout_core::paths::{LoadoutHome, SyncRoot, SyncRootKind};
use agent_loadout_core::secrets::delivery::DeliveryEnv;
use agent_loadout_core::secrets::providers::keyring::KeyringBackend;
use agent_loadout_core::secrets::providers::onepassword::OpCli;
use agent_loadout_core::store::{CommitId, Store};
use agent_loadout_core::{CoreError, Env, ErrorCode, Fs, RealFs, brand};
use serde_json::{Value, json};

/// Version of every `--json` document.
pub const JSON_SCHEMA: u32 = 1;

/// Exit code for pending changes or drift (`status --check`,
/// `diff --exit-code`, SPEC 14).
pub const EXIT_PENDING: u8 = 3;
/// Exit code for entries that need a decision (SPEC 14).
pub const EXIT_UNRESOLVED: u8 = 4;
/// Exit code for entries or aliases blocked by missing secrets (SPEC 14).
pub const EXIT_MISSING_SECRETS: u8 = 5;
/// Exit code for a lock timeout (SPEC 14).
const EXIT_LOCKED: u8 = 6;
/// Exit code for project targets blocked until the manifest is trusted
/// (`trust_required`, SPEC 14 and 17).
pub const EXIT_TRUST_REQUIRED: u8 = 7;
/// Exit code for an install or update the built-in scan blocked
/// (`scan_blocked`: a `dangerous` verdict without `--allow-dangerous`,
/// SPEC 14 and 17).
pub const EXIT_SCAN_BLOCKED: u8 = 8;

static REAL_FS: RealFs = RealFs;
/// The CLI has no Windows registry probe yet (M4 note); detection uses
/// paths and `PATH` only.
static NO_REGISTRY: NoRegistry = NoRegistry;

/// What a store command runs against.
#[derive(Debug)]
pub struct Context {
    pub env: Env,
    pub home: LoadoutHome,
    pub json: bool,
    /// `--yes`: confirmations are answered yes.
    pub yes: bool,
    /// The keyring, the 1Password CLI and this binary as the launcher.
    pub delivery: DeliveryEnv,
}

/// This machine's delivery environment: the running binary as the
/// `loadout exec` launcher, and the CLI's keyring.
pub fn delivery_env(env: &Env) -> DeliveryEnv {
    let launcher = std::env::current_exe().ok();
    DeliveryEnv::new(launcher, crate::keyring::keyring(env))
}

impl Context {
    pub fn fs(&self) -> &'static dyn Fs {
        &REAL_FS
    }

    pub fn keyring(&self) -> &dyn KeyringBackend {
        self.delivery.keyring()
    }

    /// The 1Password CLI (detected on first use).
    pub fn op(&self) -> &OpCli {
        self.delivery.op(&self.env, self.fs())
    }

    /// What an apply runs against.
    pub fn apply_env(&self) -> ApplyEnv<'_> {
        ApplyEnv {
            env: &self.env,
            fs: self.fs(),
            home: &self.home,
            registry: &NO_REGISTRY,
            delivery: &self.delivery,
        }
    }

    /// The store, or `None` when it does not exist yet. Creates nothing.
    pub fn open_store(&self) -> Result<Option<Store<'static>>, CoreError> {
        Store::open(&self.env, self.fs(), &self.home, &client_name())
    }

    /// The store, created on first use. Warns about a sync folder first,
    /// as every mutating command does (SPEC 4.5), then brings an older
    /// bundled Loadout skill up to date (SPEC 12.6): only commands that
    /// change the store get here, so read commands never write.
    pub fn open_store_for_change(&self) -> Result<Store<'static>, CoreError> {
        if let Some(root) = &self.home.sync_root {
            eprintln!("warning[sync_root_detected]: {}", sync_root_warning(root));
        }
        let store = Store::open_or_init(&self.env, self.fs(), &self.home, &client_name())?;
        agent::refresh_before_change(self, &store);
        Ok(store)
    }
}

/// The store, or `not_found` (`there is no store yet, so <what>`).
pub fn require_store(ctx: &Context, what: &str) -> Result<Store<'static>, CoreError> {
    ctx.open_store()?.ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotFound,
            format!("there is no store yet, so {what}"),
        )
    })
}

/// `Loadout-Client` trailer value: `loadout-cli/<version>`.
pub fn client_name() -> String {
    format!("{}/{}", brand::CLI_CLIENT_NAME, env!("CARGO_PKG_VERSION"))
}

/// Process exit code for a failed command: 4 for a needed confirmation,
/// 5 for a missing secret, 6 for `locked`, 7 for an untrusted project
/// manifest, 8 for content the built-in scan blocked, else 1.
pub fn exit_code(err: &CoreError) -> u8 {
    match err.code() {
        ErrorCode::Locked => EXIT_LOCKED,
        ErrorCode::ConfirmationRequired => EXIT_UNRESOLVED,
        ErrorCode::MissingSecret => EXIT_MISSING_SECRETS,
        ErrorCode::TrustRequired => EXIT_TRUST_REQUIRED,
        ErrorCode::ScanBlocked => EXIT_SCAN_BLOCKED,
        _ => 1,
    }
}

/// The exit code of an apply run ([`ApplyReport::status`]).
pub fn apply_exit_code(report: &ApplyReport) -> u8 {
    status_exit_code(report.status())
}

/// 1 when a file failed, 4 when entries need a decision, 5 when entries are
/// blocked by missing secrets, 7 when a project waits for `loadout project
/// trust`, else 0.
pub fn status_exit_code(status: ApplyStatus) -> u8 {
    match status {
        ApplyStatus::Done => 0,
        ApplyStatus::FileFailed => 1,
        ApplyStatus::NeedsDecision => EXIT_UNRESOLVED,
        ApplyStatus::MissingSecrets => EXIT_MISSING_SECRETS,
        ApplyStatus::TrustRequired => EXIT_TRUST_REQUIRED,
    }
}

pub fn outcome_str(outcome: ApplyOutcome) -> &'static str {
    match outcome {
        ApplyOutcome::Ok => "ok",
        ApplyOutcome::Partial => "partial",
        ApplyOutcome::Blocked => "blocked",
        ApplyOutcome::Failed => "failed",
    }
}

pub fn entry_json(entry: &EntryPlan) -> Value {
    json!({
        "name": entry.name,
        "client": entry.client,
        "server": entry.server_id,
        "state": entry.state.as_str(),
        "action": entry.action.as_str(),
        "unresolved": entry.unresolved,
        "delivery": entry.delivery,
        "problem": entry.problem.as_ref().map(error_json),
    })
}

fn target_result_json(target: &TargetResult) -> Value {
    json!({
        "clients": target.clients,
        "project_id": target.project_id,
        "file": path_json(&target.file),
        "outcome": outcome_str(target.outcome),
        "written": target.written,
        "create": target.counts.create,
        "update": target.counts.update,
        "remove": target.counts.remove,
        "disable": target.counts.disable,
        "error": target.error.as_ref().map(error_json),
        "read_only": read_only_json(target.read_only),
        "backup": target.backup.as_deref().map(path_json),
        "entries": target.entries.iter().map(entry_json).collect::<Vec<_>>(),
        "warnings": target.warnings.iter().map(error_json).collect::<Vec<_>>(),
    })
}

/// A file's read-only state in `--json` output (SPEC 8.2
/// `read_only_target`): `{"kind": "acl"}`, or `null` when writable or
/// unknown.
pub fn read_only_json(kind: Option<ReadOnlyKind>) -> Value {
    kind.map_or(Value::Null, |kind| json!({ "kind": kind.as_str() }))
}

/// `  read-only (<kind>)` after a file in text output; empty when
/// writable or unknown.
pub fn read_only_note(kind: Option<ReadOnlyKind>) -> String {
    kind.map(|kind| format!("  read-only ({kind})"))
        .unwrap_or_default()
}

/// An apply run as JSON (`apply`, `rollback`, `adopt`, `import`, `server`).
pub fn report_json(report: &ApplyReport, commit: Option<&CommitId>) -> Value {
    json!({
        "apply_id": report.apply_id,
        "trigger": report.trigger,
        "outcome": outcome_str(report.outcome),
        "recorded": report.recorded,
        "commit": commit.map(CommitId::to_string),
        "written": report.writes(),
        "unresolved": report.unresolved(),
        "imported": report.imported,
        "targets": report.targets.iter().map(target_result_json).collect::<Vec<_>>(),
        "warnings": report.warnings.iter().map(error_json).collect::<Vec<_>>(),
    })
}

/// An apply run for people: one line per file that changed or needs
/// attention, then entries that need a decision, then warnings (stderr).
pub fn print_report(report: &ApplyReport, commit: Option<&CommitId>) {
    for target in &report.targets {
        let counts = target.counts;
        let attention = target.outcome != ApplyOutcome::Ok;
        if !target.written && !attention {
            continue;
        }
        println!(
            "{:<8} {}  ({}; +{} ~{} -{} !{}){}",
            outcome_str(target.outcome),
            target.file.display(),
            target.clients.join(", "),
            counts.create,
            counts.update,
            counts.remove,
            counts.disable,
            read_only_note(target.read_only)
        );
        if let Some(err) = &target.error {
            println!("         error[{}]: {err}", err.code());
        }
        for entry in &target.entries {
            if entry.unresolved || entry.problem.is_some() {
                let problem = entry
                    .problem
                    .as_ref()
                    .map(|err| format!(": {}[{}]", err.message(), err.code()))
                    .unwrap_or_default();
                let hint = if entry.unresolved {
                    " (needs a decision: --on-conflict or adopt)"
                } else {
                    ""
                };
                println!(
                    "         {} {}{problem}{hint}",
                    entry.state.as_str(),
                    entry.name
                );
            }
        }
        for warning in &target.warnings {
            println!(
                "         warning[{}]: {}",
                warning.code(),
                warning.message()
            );
        }
    }
    for warning in &report.warnings {
        eprintln!("warning[{}]: {warning}", warning.code());
    }
    let commit = commit.map_or_else(
        || "no commit".to_owned(),
        |c| format!("commit {}", c.short()),
    );
    println!(
        "Apply {}: {}, {} file(s) written ({commit}).",
        report.apply_id,
        outcome_str(report.outcome),
        report.writes()
    );
}

pub fn report_error(err: &CoreError, json: bool) {
    if json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "error": error_json(err),
        }));
    } else {
        eprintln!("error[{}]: {err}", err.code());
    }
}

pub fn error_json(err: &CoreError) -> Value {
    json!({
        "code": err.code().as_str(),
        "message": err.message(),
        "path": err.path().map_or(Value::Null, path_json),
    })
}

/// Validation warnings go to stderr; they never fail a command.
pub fn report_warnings<'a>(warnings: impl IntoIterator<Item = &'a Diagnostic>) {
    for warning in warnings {
        eprintln!("warning[{}]: {warning}", warning.code);
    }
}

pub fn sync_root_warning(root: &SyncRoot) -> String {
    format!(
        "{} is inside a {} folder ({}). Sync tools corrupt live git repos and lock files; move it with --home or {}.",
        brand::HOME_ENV_VAR,
        sync_kind_label(root.kind),
        root.root.display(),
        brand::HOME_ENV_VAR,
    )
}

fn sync_kind_label(kind: SyncRootKind) -> &'static str {
    match kind {
        SyncRootKind::Resilio => "Resilio Sync",
        SyncRootKind::Syncthing => "Syncthing",
        SyncRootKind::OneDrive => "OneDrive",
        SyncRootKind::Dropbox => "Dropbox",
        SyncRootKind::GoogleDrive => "Google Drive",
    }
}

pub fn path_json(path: &Path) -> Value {
    Value::String(path.as_os_str().to_string_lossy().into_owned())
}

pub fn print_json(doc: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(doc).expect("JSON values always serialize")
    );
}

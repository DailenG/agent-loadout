//! Store commands and the helpers every command shares: JSON output, error
//! reporting and exit codes (SPEC 14).

pub mod history;
pub mod server;

use std::path::Path;

use agent_loadout_core::model::Diagnostic;
use agent_loadout_core::paths::{LoadoutHome, SyncRoot, SyncRootKind};
use agent_loadout_core::store::Store;
use agent_loadout_core::{CoreError, Env, ErrorCode, Fs, RealFs, brand};
use serde_json::{Value, json};

/// Version of every `--json` document.
pub const JSON_SCHEMA: u32 = 1;

/// Exit code for a lock timeout (SPEC 14).
const EXIT_LOCKED: u8 = 6;

static REAL_FS: RealFs = RealFs;

/// What a store command runs against.
#[derive(Debug)]
pub struct Context {
    pub env: Env,
    pub home: LoadoutHome,
    pub json: bool,
}

impl Context {
    fn fs(&self) -> &'static dyn Fs {
        &REAL_FS
    }

    /// The store, or `None` when it does not exist yet. Creates nothing.
    pub fn open_store(&self) -> Result<Option<Store<'static>>, CoreError> {
        Store::open(&self.env, self.fs(), &self.home, &client_name())
    }

    /// The store, created on first use. Warns about a sync folder first,
    /// as every mutating command does (SPEC 4.5).
    pub fn open_store_for_change(&self) -> Result<Store<'static>, CoreError> {
        if let Some(root) = &self.home.sync_root {
            eprintln!("warning[sync_root_detected]: {}", sync_root_warning(root));
        }
        Store::open_or_init(&self.env, self.fs(), &self.home, &client_name())
    }
}

/// `Loadout-Client` trailer value: `loadout-cli/<version>`.
pub fn client_name() -> String {
    format!("{}/{}", brand::CLI_CLIENT_NAME, env!("CARGO_PKG_VERSION"))
}

/// Process exit code for a failed command: 6 for `locked`, else 1.
pub fn exit_code(err: &CoreError) -> u8 {
    match err.code() {
        ErrorCode::Locked => EXIT_LOCKED,
        _ => 1,
    }
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

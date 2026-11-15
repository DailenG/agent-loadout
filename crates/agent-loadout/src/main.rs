//! `loadout` CLI. Thin over `agent-loadout-core`; the only place that builds
//! an [`Env`] from the running process.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agent_loadout_core::paths::{self, LoadoutHome, SyncRoot, SyncRootKind};
use agent_loadout_core::{CoreError, Env, ErrorCode, KnownFolder, Os, RealFs, SystemClock, brand};
use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};

/// Version of every `--json` document.
const JSON_SCHEMA: u32 = 1;

#[derive(Debug, Parser)]
#[command(
    name = brand::BIN_NAME,
    version,
    about = "Manage MCP servers, skills and secret references for AI coding clients"
)]
struct Cli {
    /// Data directory to use instead of LOADOUT_HOME or the per-OS default.
    #[arg(long, global = true, value_name = "PATH")]
    home: Option<PathBuf>,
    /// Print machine-readable JSON to stdout.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check the installation and report problems.
    Doctor(DoctorArgs),
}

#[derive(Debug, Args)]
struct DoctorArgs {
    /// Also report the resolved OS folders and data directory layout.
    #[arg(long)]
    paths: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            report_error(&err, cli.json);
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<(), CoreError> {
    let env = process_env()?;
    let flag = cli.home.as_deref().map(absolute).transpose()?;
    match &cli.command {
        Command::Doctor(args) => doctor(&env, flag.as_deref(), args, cli.json),
    }
}

/// Builds the core `Env` from the process. On Windows the profile and app
/// data folders come from `USERPROFILE`, `LOCALAPPDATA` and `APPDATA` when
/// set (hermetic tests set them), else from the Known Folders; `HOME` is
/// never used there.
fn process_env() -> Result<Env, CoreError> {
    let os = if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    };
    let var_path = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let (home, local_app_data, roaming_app_data) = match os {
        Os::Windows => (
            var_path("USERPROFILE").or_else(dirs::home_dir),
            var_path("LOCALAPPDATA").or_else(dirs::data_local_dir),
            var_path("APPDATA").or_else(dirs::data_dir),
        ),
        Os::Linux | Os::MacOs => (var_path("HOME").or_else(dirs::home_dir), None, None),
    };
    let home = home.ok_or_else(|| {
        CoreError::new(
            ErrorCode::ProfileNotFound,
            "cannot determine the user profile directory",
        )
    })?;

    let mut builder = Env::builder(os, home).clock(SystemClock);
    if let Some(path) = local_app_data {
        builder = builder.known_folder(KnownFolder::LocalAppData, path);
    }
    if let Some(path) = roaming_app_data {
        builder = builder.known_folder(KnownFolder::RoamingAppData, path);
    }
    for (name, value) in std::env::vars_os() {
        if let Some(name) = name.to_str() {
            builder = builder.var(name, value);
        }
    }
    Ok(builder.build())
}

fn absolute(path: &Path) -> Result<PathBuf, CoreError> {
    std::path::absolute(path).map_err(|err| {
        CoreError::new(ErrorCode::InvalidPath, format!("invalid --home: {err}")).with_path(path)
    })
}

fn doctor(env: &Env, flag: Option<&Path>, args: &DoctorArgs, json: bool) -> Result<(), CoreError> {
    let home = paths::loadout_home(env, &RealFs, flag)?;
    let findings: Vec<&SyncRoot> = home.sync_root.iter().collect();

    if json {
        let mut doc = json!({
            "schema": JSON_SCHEMA,
            "findings": findings.iter().map(|root| sync_root_finding_json(root)).collect::<Vec<_>>(),
        });
        if args.paths {
            doc["paths"] = paths_json(env, &home);
        }
        print_json(&doc);
        return Ok(());
    }

    if args.paths {
        for (label, value) in paths_table(env, &home) {
            println!("{label:<18}{value}");
        }
        println!();
    }
    if findings.is_empty() {
        println!("No problems found.");
    }
    for root in findings {
        println!(
            "warning[sync_root_detected]: {} is inside a {} folder ({}). Sync tools corrupt live git repos and lock files; move it with --home or {}.",
            brand::HOME_ENV_VAR,
            sync_kind_label(root.kind),
            root.root.display(),
            brand::HOME_ENV_VAR,
        );
    }
    Ok(())
}

fn paths_table(env: &Env, home: &LoadoutHome) -> Vec<(&'static str, String)> {
    let mut rows = vec![
        ("os", env.os().as_str().to_owned()),
        ("profile", env.home().display().to_string()),
    ];
    for (label, folder) in known_folders() {
        if let Some(path) = env.known_folder(folder) {
            rows.push((label, path.display().to_string()));
        }
    }
    rows.push((
        "loadout_home",
        format!("{} ({})", home.path.display(), home.source.as_str()),
    ));
    rows.extend(layout(home).map(|(label, path)| (label, path.display().to_string())));
    rows
}

fn paths_json(env: &Env, home: &LoadoutHome) -> Value {
    let mut doc = json!({
        "os": env.os().as_str(),
        "profile": path_json(env.home()),
        "loadout_home": path_json(&home.path),
        "loadout_home_source": home.source.as_str(),
    });
    for (label, folder) in known_folders() {
        doc[label] = env.known_folder(folder).map_or(Value::Null, path_json);
    }
    for (label, path) in layout(home) {
        doc[label] = path_json(&path);
    }
    doc
}

fn known_folders() -> [(&'static str, KnownFolder); 2] {
    [
        ("local_app_data", KnownFolder::LocalAppData),
        ("roaming_app_data", KnownFolder::RoamingAppData),
    ]
}

fn layout(home: &LoadoutHome) -> impl Iterator<Item = (&'static str, PathBuf)> {
    [
        ("store", home.store_dir()),
        ("state", home.state_dir()),
        ("backups", home.backups_dir()),
        ("cache", home.cache_dir()),
        ("imports", home.imports_dir()),
        ("logs", home.logs_dir()),
    ]
    .into_iter()
}

fn sync_root_finding_json(root: &SyncRoot) -> Value {
    json!({
        "code": "sync_root_detected",
        "severity": "warning",
        "kind": root.kind.as_str(),
        "root": path_json(&root.root),
    })
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

fn path_json(path: &Path) -> Value {
    Value::String(path.as_os_str().to_string_lossy().into_owned())
}

fn print_json(doc: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(doc).expect("JSON values always serialize")
    );
}

fn report_error(err: &CoreError, json: bool) {
    if json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "error": {
                "code": err.code().as_str(),
                "message": err.message(),
                "path": err.path().map_or(Value::Null, path_json),
            },
        }));
    } else {
        eprintln!("error[{}]: {err}", err.code());
    }
}

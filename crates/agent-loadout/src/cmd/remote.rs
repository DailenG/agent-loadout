//! `loadout remote add folder|ls|rm|push|pull|machines` (SPEC 9.6 to 9.9,
//! 14). Remotes are machine-local (`state/remotes.toml`); none exists until
//! one is added. Git remotes arrive with M13.
//!
//! Exit codes: a remote that cannot be reached is an error for `push` and
//! `pull` themselves (1), never for local work; `pull` exits 4 when fields
//! conflict (`state/merge/MERGE_STATE.json` lists them).

use std::path::Path;

use agent_loadout_core::remote::config::{FolderMode, RemoteConfig, RemotesFile};
use agent_loadout_core::remote::folder::{
    self, MachineEntry, PullReport, PushOutcome, SiblingPull,
};
use agent_loadout_core::remote::sync_artifacts::Artifact;
use agent_loadout_core::remote::unc::SystemUnc;
use agent_loadout_core::store::Store;
use agent_loadout_core::store::diff::{ChangeKind, EntityChange};
use agent_loadout_core::store::merge::MergeOutcome;
use agent_loadout_core::{CoreError, ErrorCode, brand};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use super::{
    Context, EXIT_UNRESOLVED, JSON_SCHEMA, error_json, path_json, print_json, require_store,
};

static UNC: SystemUnc = SystemUnc;

#[derive(Debug, Args)]
pub struct RemoteArgs {
    #[command(subcommand)]
    command: RemoteCommand,
}

#[derive(Debug, Subcommand)]
enum RemoteCommand {
    /// Add a remote.
    Add {
        #[command(subcommand)]
        kind: AddKind,
    },
    /// List this machine's remotes.
    Ls,
    /// Remove a remote (its files stay where they are).
    Rm { name: String },
    /// Push the store to one remote [default: every remote].
    Push { name: Option<String> },
    /// Pull and merge other machines' changes [default: every remote].
    Pull { name: Option<String> },
    /// List the machines a folder remote holds.
    Machines { name: String },
}

#[derive(Debug, Subcommand)]
enum AddKind {
    /// A local or network folder (USB drive, NAS share, synced folder).
    Folder {
        name: String,
        path: std::path::PathBuf,
        /// repo-per-machine (default), mirror or archive.
        #[arg(long, default_value = "repo-per-machine")]
        mode: String,
        /// Archive mode: archives to keep [default: 10].
        #[arg(long)]
        keep: Option<u32>,
    },
}

pub fn run(ctx: &Context, args: &RemoteArgs) -> Result<u8, CoreError> {
    match &args.command {
        RemoteCommand::Add {
            kind:
                AddKind::Folder {
                    name,
                    path,
                    mode,
                    keep,
                },
        } => add_folder(ctx, name, path, mode, *keep).map(|()| 0),
        RemoteCommand::Ls => list(ctx).map(|()| 0),
        RemoteCommand::Rm { name } => remove(ctx, name).map(|()| 0),
        RemoteCommand::Push { name } => push(ctx, name.as_deref()),
        RemoteCommand::Pull { name } => pull(ctx, name.as_deref()),
        RemoteCommand::Machines { name } => machines(ctx, name).map(|()| 0),
    }
}

fn remotes(ctx: &Context) -> Result<RemotesFile, CoreError> {
    RemotesFile::load(ctx.fs(), &ctx.home.state_dir())
}

fn add_folder(
    ctx: &Context,
    name: &str,
    path: &Path,
    mode: &str,
    keep: Option<u32>,
) -> Result<(), CoreError> {
    let mode = FolderMode::parse(mode)?;
    let path = std::path::absolute(path).map_err(|err| {
        CoreError::new(ErrorCode::InvalidPath, format!("invalid path: {err}")).with_path(path)
    })?;
    if path.starts_with(&ctx.home.path) {
        return Err(CoreError::new(
            ErrorCode::InvalidPath,
            format!("a remote cannot live inside {}", brand::HOME_ENV_VAR),
        )
        .with_path(&path));
    }
    let mut remote = RemoteConfig::folder(name, &path.to_string_lossy(), mode);
    remote.keep = keep;
    let mut file = remotes(ctx)?;
    file.add(remote)?;
    file.save(ctx.fs(), &ctx.home.state_dir())?;
    let reachable = ctx.fs().is_dir(&path);
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "added": name,
            "path": path_json(&path),
            "mode": mode.as_str(),
            "reachable": reachable,
        }));
    } else {
        println!(
            "Added folder remote {name} at {} ({}).",
            path.display(),
            mode.as_str()
        );
        if !reachable {
            eprintln!(
                "warning[remote_unreachable]: {} is not there now; pushes try again later",
                path.display()
            );
        }
    }
    Ok(())
}

fn list(ctx: &Context) -> Result<(), CoreError> {
    let file = remotes(ctx)?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "remotes": file.remotes.iter().map(remote_json).collect::<Vec<_>>(),
        }));
        return Ok(());
    }
    if file.remotes.is_empty() {
        println!("No backup remote.");
    }
    for remote in &file.remotes {
        println!(
            "{:<20} {:<7} {:<17} {}",
            remote.name,
            remote.kind.as_str(),
            remote.folder_mode().as_str(),
            remote
                .path
                .as_deref()
                .or(remote.url.as_deref())
                .unwrap_or_default()
        );
    }
    Ok(())
}

fn remote_json(remote: &RemoteConfig) -> Value {
    json!({
        "name": remote.name,
        "kind": remote.kind.as_str(),
        "path": remote.path,
        "url": remote.url,
        "mode": remote.mode.map(FolderMode::as_str),
        "push": remote.push,
        "pull": remote.pull,
    })
}

fn remove(ctx: &Context, name: &str) -> Result<(), CoreError> {
    let mut file = remotes(ctx)?;
    file.remove(name)?;
    file.save(ctx.fs(), &ctx.home.state_dir())?;
    if ctx.json {
        print_json(&json!({ "schema": JSON_SCHEMA, "removed": name }));
    } else {
        println!("Removed remote {name}; its files were left in place.");
    }
    Ok(())
}

/// One push: the remote's name and how it went.
pub type PushResult = (String, Result<PushOutcome, CoreError>);

/// Pushes to the selected remotes; one result per remote.
pub fn push_to(
    store: &Store<'_>,
    file: &RemotesFile,
    name: Option<&str>,
) -> Result<Vec<PushResult>, CoreError> {
    Ok(file
        .select(name)?
        .into_iter()
        .map(|remote| {
            let result = folder::push(store, remote, &UNC, env!("CARGO_PKG_VERSION"));
            (remote.name.clone(), result)
        })
        .collect())
}

pub fn push_json(results: &[PushResult]) -> Value {
    Value::Array(
        results
            .iter()
            .map(|(name, result)| match result {
                Ok(outcome) => json!({
                    "remote": name,
                    "ok": true,
                    "dir": path_json(&outcome.dir),
                    "mode": outcome.mode.as_str(),
                    "head": outcome.head.as_str(),
                    "archive": outcome.archive,
                }),
                Err(err) => json!({ "remote": name, "ok": false, "error": error_json(err) }),
            })
            .collect(),
    )
}

pub fn print_pushes(results: &[PushResult]) {
    for (name, result) in results {
        match result {
            Ok(outcome) => println!(
                "Pushed {} to {name} ({}, {}).",
                outcome.head.short(),
                outcome.mode.as_str(),
                outcome.dir.display()
            ),
            Err(err) => eprintln!("warning[{}]: {name}: {err}", err.code()),
        }
    }
}

fn push(ctx: &Context, name: Option<&str>) -> Result<u8, CoreError> {
    let store = require_store(ctx, "nothing to push")?;
    let file = remotes(ctx)?;
    if file.remotes.is_empty() {
        return Err(no_remote());
    }
    let results = push_to(&store, &file, name)?;
    if ctx.json {
        print_json(&json!({ "schema": JSON_SCHEMA, "pushes": push_json(&results) }));
    } else {
        print_pushes(&results);
    }
    Ok(if results.iter().any(|(_, r)| r.is_err()) {
        1
    } else {
        0
    })
}

fn no_remote() -> CoreError {
    CoreError::new(
        ErrorCode::NotFound,
        format!(
            "no backup remote; add one with `{} remote add folder <NAME> <PATH>`",
            brand::BIN_NAME
        ),
    )
}

fn pull(ctx: &Context, name: Option<&str>) -> Result<u8, CoreError> {
    let store = require_store(ctx, "nothing to merge into")?;
    let file = remotes(ctx)?;
    if file.remotes.is_empty() {
        return Err(no_remote());
    }
    let mut reports = Vec::new();
    for remote in file.select(name)? {
        reports.push((remote.name.clone(), folder::pull(&store, remote, &UNC)));
    }
    let conflicts = reports
        .iter()
        .any(|(_, r)| r.as_ref().is_ok_and(|r| r.conflicts() > 0));
    let failed = reports.iter().any(|(_, r)| r.is_err());
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "pulls": reports.iter().map(|(name, result)| match result {
                Ok(report) => pull_json(&store, report),
                Err(err) => json!({ "remote": name, "ok": false, "error": error_json(err) }),
            }).collect::<Vec<_>>(),
        }));
    } else {
        for (name, result) in &reports {
            match result {
                Ok(report) => print_pull(&store, report),
                Err(err) => eprintln!("warning[{}]: {name}: {err}", err.code()),
            }
        }
    }
    Ok(if conflicts {
        EXIT_UNRESOLVED
    } else if failed {
        1
    } else {
        0
    })
}

fn outcome_json(sibling: &SiblingPull) -> Value {
    let (outcome, extra) = match &sibling.outcome {
        Ok(MergeOutcome::UpToDate) => ("up_to_date", Value::Null),
        Ok(MergeOutcome::FastForward(id)) => ("fast_forward", json!(id.as_str())),
        Ok(MergeOutcome::Merged(id)) => ("merged", json!(id.as_str())),
        Ok(MergeOutcome::Conflicts { conflicts, .. }) => (
            "conflicts",
            Value::Array(
                conflicts
                    .iter()
                    .map(|c| json!({ "file": c.file, "field": c.field }))
                    .collect(),
            ),
        ),
        Err(err) => ("error", error_json(err)),
    };
    json!({
        "machine": sibling.machine_id,
        "label": sibling.label,
        "outcome": outcome,
        "detail": extra,
    })
}

fn artifacts_json(artifacts: &[Artifact]) -> Value {
    Value::Array(
        artifacts
            .iter()
            .map(|a| json!({ "code": "sync_artifact_ignored", "path": path_json(&a.path), "tool": a.tool }))
            .collect(),
    )
}

/// Servers and skills a pull changed.
fn changed(store: &Store<'_>, report: &PullReport) -> (usize, usize) {
    if report.head_before == report.head_after {
        return (0, 0);
    }
    let changes: Vec<EntityChange> = store
        .semantic_diff(Some(&report.head_before), &report.head_after)
        .unwrap_or_default();
    let mut servers: Vec<&str> = Vec::new();
    let mut skills: Vec<&str> = Vec::new();
    for change in &changes {
        if change.kind == ChangeKind::Changed && change.fields.is_empty() && change.text.is_none() {
            continue;
        }
        let list = if change.entity.starts_with("server/") {
            &mut servers
        } else if change.entity.starts_with("skill/") {
            &mut skills
        } else {
            continue;
        };
        if !list.contains(&change.entity.as_str()) {
            list.push(&change.entity);
        }
    }
    (servers.len(), skills.len())
}

fn pull_json(store: &Store<'_>, report: &PullReport) -> Value {
    let (servers, skills) = changed(store, report);
    json!({
        "remote": report.remote,
        "ok": true,
        "tag": report.tag,
        "head_before": report.head_before.as_str(),
        "head_after": report.head_after.as_str(),
        "machines": report.siblings.iter().map(outcome_json).collect::<Vec<_>>(),
        "tags_imported": report.tags_imported,
        "changed_servers": servers,
        "changed_skills": skills,
        "conflicts": report.conflicts(),
        "warnings": artifacts_json(&report.artifacts),
    })
}

fn print_pull(store: &Store<'_>, report: &PullReport) {
    for sibling in &report.siblings {
        let line = match &sibling.outcome {
            Ok(MergeOutcome::UpToDate) => "up to date".to_owned(),
            Ok(MergeOutcome::FastForward(id)) => format!("fast-forward to {}", id.short()),
            Ok(MergeOutcome::Merged(id)) => format!("merged as {}", id.short()),
            Ok(MergeOutcome::Conflicts { conflicts, .. }) => format!(
                "{} conflicting field(s), nothing merged; see state/merge/MERGE_STATE.json",
                conflicts.len()
            ),
            Err(err) => format!("error[{}]: {err}", err.code()),
        };
        println!(
            "{}/{} ({}): {line}",
            report.remote, sibling.label, sibling.machine_id
        );
    }
    for artifact in &report.artifacts {
        eprintln!(
            "info[sync_artifact_ignored]: {} ({}); left in place",
            artifact.path.display(),
            artifact.tool
        );
    }
    let (servers, skills) = changed(store, report);
    if report.head_before != report.head_after {
        println!(
            "{servers} server(s), {skills} skill(s) changed by pull. Review with `{bin} diff` and \
             apply with `{bin} apply`.",
            bin = brand::BIN_NAME
        );
    }
}

pub fn machine_json(entry: &MachineEntry) -> Value {
    json!({
        "store_id": entry.store_id,
        "machine_id": entry.machine_id,
        "label": entry.manifest.label,
        "os": entry.manifest.os,
        "pushed_at": entry.manifest.pushed_at,
        "head": entry.manifest.head,
        "subject": entry.subject,
        "dir": path_json(&entry.dir),
    })
}

pub fn print_machines(entries: &[MachineEntry]) {
    for entry in entries {
        println!(
            "{}  {:<16} {:<8} {}  {}",
            entry.machine_id,
            entry.manifest.label,
            entry.manifest.os,
            entry.manifest.pushed_at,
            entry.subject.as_deref().unwrap_or("")
        );
    }
}

fn machines(ctx: &Context, name: &str) -> Result<(), CoreError> {
    let file = remotes(ctx)?;
    let remote = file
        .get(name)
        .ok_or_else(|| CoreError::new(ErrorCode::NotFound, format!("no remote named {name:?}")))?;
    let root = folder::reachable_root(ctx.fs(), ctx.env.os(), &UNC, remote)?;
    let listing = folder::list(ctx.fs(), &root, None)?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "machines": listing.machines.iter().map(machine_json).collect::<Vec<_>>(),
            "warnings": artifacts_json(&listing.artifacts),
        }));
        return Ok(());
    }
    if listing.machines.is_empty() {
        println!("No machines in {name}.");
    }
    print_machines(&listing.machines);
    for artifact in &listing.artifacts {
        eprintln!(
            "info[sync_artifact_ignored]: {} ({})",
            artifact.path.display(),
            artifact.tool
        );
    }
    Ok(())
}

//! `loadout restore <REV|TAG> [--path ENTITY]... [--apply]` and `loadout
//! restore --files <APPLY-ID> [--file PATH]` (SPEC 9.5, 14).
//!
//! A store restore is a new commit (tagged `auto/pre-restore/<ts>` first);
//! client files change only with `--apply`, or at the next `loadout
//! apply`. A shared project manifest is written back to its project folder
//! only with `--yes` (exit code 4 without it). `--files` writes the client
//! file bytes an apply run backed up.

use std::path::PathBuf;

use agent_loadout_core::store::restore::{
    self, FileOutcome, FileRestore, RestoreReport, RestoreRequest,
};
use agent_loadout_core::{CoreError, ErrorCode, brand};
use clap::Args;
use serde_json::{Value, json};

use super::{
    Context, JSON_SCHEMA, apply_exit_code, error_json, path_json, print_json, print_report,
    report_json, require_store,
};

#[derive(Debug, Args)]
pub struct RestoreArgs {
    /// Commit (full or abbreviated id) or tag to restore.
    #[arg(value_name = "REV|TAG", required_unless_present = "files")]
    rev: Option<String>,
    /// Restore only this entity (repeatable): server/<id>, secrets,
    /// clients, settings, project/<id>, skill/<id>, machine/<id>.
    #[arg(long = "path", value_name = "ENTITY", conflicts_with = "files")]
    paths: Vec<String>,
    /// Apply the restored state in the same commit.
    #[arg(long, conflicts_with = "files")]
    apply: bool,
    /// Restore client files from the backups of this apply run instead.
    #[arg(long, value_name = "APPLY-ID", conflicts_with = "rev")]
    files: Option<String>,
    /// With --files: only this client file.
    #[arg(long, value_name = "PATH", requires = "files")]
    file: Option<PathBuf>,
}

pub fn run(ctx: &Context, args: &RestoreArgs) -> Result<u8, CoreError> {
    if let Some(apply_id) = &args.files {
        return files(ctx, apply_id, args.file.as_deref());
    }
    let store = require_store(ctx, "nothing to restore")?;
    if let Some(root) = &ctx.home.sync_root {
        eprintln!(
            "warning[sync_root_detected]: {}",
            super::sync_root_warning(root)
        );
    }
    let rev = args
        .rev
        .as_deref()
        .expect("clap requires REV without --files");
    let request = RestoreRequest {
        target: store.resolve(rev)?,
        entities: args.paths.clone(),
        confirmed: ctx.yes,
        apply: args.apply,
    };
    let report = restore::restore(&store, &ctx.apply_env(), &request)?;
    print(ctx, &report);
    Ok(report.apply.as_ref().map_or(0, apply_exit_code))
}

fn print(ctx: &Context, report: &RestoreReport) {
    let plan = &report.plan;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "restored": plan.target.as_str(),
            "files": plan.files,
            "tag": report.tag,
            "commit": report.commit.as_ref().map(ToString::to_string),
            "manifests": report.written.iter().map(|path| path_json(path)).collect::<Vec<_>>(),
            "unplaced_manifests": plan.unplaced,
            "apply": report.apply.as_ref().map(|r| report_json(r, report.commit.as_ref())),
        }));
        return;
    }
    if plan.files.is_empty() {
        println!(
            "Nothing to restore: the store already matches {}.",
            plan.target.short()
        );
        return;
    }
    println!(
        "Restored {} from {} (commit {}; tag {}).",
        plan.files.join(", "),
        plan.target.short(),
        report.commit.as_ref().map_or("none", |c| c.short()),
        report.tag.as_deref().unwrap_or("none")
    );
    for path in &report.written {
        println!("Wrote {}.", path.display());
    }
    for file in &plan.unplaced {
        eprintln!(
            "note: {file} was restored in the store; this machine has no folder for its project"
        );
    }
    match &report.apply {
        Some(apply) => print_report(apply, report.commit.as_ref()),
        None => println!(
            "Run `{bin} apply` (or `{bin} diff` first) to update client files.",
            bin = brand::BIN_NAME
        ),
    }
}

fn files(ctx: &Context, apply_id: &str, only: Option<&std::path::Path>) -> Result<u8, CoreError> {
    let only = only
        .map(|path| {
            std::path::absolute(path).map_err(|err| {
                CoreError::new(ErrorCode::InvalidPath, format!("invalid --file: {err}"))
                    .with_path(path)
            })
        })
        .transpose()?;
    let results = restore::restore_files(&ctx.apply_env(), apply_id, only.as_deref())?;
    let failed = results
        .iter()
        .any(|result| matches!(result.outcome, FileOutcome::Failed(_)));
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "apply_id": apply_id,
            "files": results.iter().map(file_json).collect::<Vec<_>>(),
        }));
    } else {
        for result in &results {
            match &result.outcome {
                FileOutcome::Restored { previous } => println!(
                    "restored  {}{}",
                    result.file.display(),
                    previous
                        .as_ref()
                        .map(|p| format!("  (previous bytes in {})", p.display()))
                        .unwrap_or_default()
                ),
                FileOutcome::Unchanged => println!("unchanged {}", result.file.display()),
                FileOutcome::Failed(err) => {
                    println!(
                        "failed    {}  error[{}]: {err}",
                        result.file.display(),
                        err.code()
                    );
                }
            }
        }
    }
    Ok(if failed { 1 } else { 0 })
}

fn file_json(result: &FileRestore) -> Value {
    let (outcome, previous, error) = match &result.outcome {
        FileOutcome::Restored { previous } => {
            ("restored", previous.as_deref().map(path_json), None)
        }
        FileOutcome::Unchanged => ("unchanged", None, None),
        FileOutcome::Failed(err) => ("failed", None, Some(error_json(err))),
    };
    json!({
        "file": path_json(&result.file),
        "outcome": outcome,
        "previous": previous,
        "error": error,
    })
}

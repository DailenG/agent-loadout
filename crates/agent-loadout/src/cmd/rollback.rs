//! `loadout rollback <APPLY-ID|last>` (SPEC 8.7, 14): restore the desired
//! state from before an apply run in a new commit and apply it. Rolling
//! back an older run over later changes to the same files needs `--yes`
//! (exit code 4 without it).

use agent_loadout_core::CoreError;
use agent_loadout_core::apply::placeholder_path;
use agent_loadout_core::apply::rollback::{self, ApplySelector};
use clap::Args;
use serde_json::json;

use super::{Context, JSON_SCHEMA, apply_exit_code, print_json, print_report, report_json};

#[derive(Debug, Args)]
pub struct RollbackArgs {
    /// Apply id (from `loadout history`), or `last` for this machine's
    /// latest apply run.
    #[arg(value_name = "APPLY-ID|last")]
    apply: String,
}

pub fn run(ctx: &Context, args: &RollbackArgs) -> Result<u8, CoreError> {
    let store = ctx.open_store()?.ok_or_else(|| {
        CoreError::new(
            agent_loadout_core::ErrorCode::NotFound,
            "there is no store yet, so no apply run to roll back",
        )
    })?;
    if let Some(root) = &ctx.home.sync_root {
        eprintln!(
            "warning[sync_root_detected]: {}",
            super::sync_root_warning(root)
        );
    }
    let report = rollback::rollback(
        &store,
        &ctx.apply_env(),
        &ApplySelector::parse(&args.apply),
        ctx.yes,
    )?;
    let unavailable: Vec<String> = report
        .unavailable_originals
        .iter()
        .map(|file| placeholder_path(&ctx.env, file, None))
        .collect();
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "rolled_back": report.plan.apply_id,
            "restored_from": report.plan.restore_from.as_str(),
            "files": report.plan.files,
            "tag": report.tag,
            "apply": report_json(&report.apply, report.commit.as_ref()),
            "unavailable_originals": unavailable,
        }));
    } else {
        println!(
            "Rolled back apply {} (restored {} from {}; tag {}).",
            report.plan.apply_id,
            if report.plan.files.is_empty() {
                "nothing".to_owned()
            } else {
                report.plan.files.join(", ")
            },
            report.plan.restore_from.short(),
            report.tag
        );
        print_report(&report.apply, report.commit.as_ref());
        for file in &unavailable {
            println!(
                "note: the content {file} had before apply {} is no longer kept: its backup \
                 held old secret values and was removed, so it cannot be restored.",
                report.plan.apply_id
            );
        }
    }
    Ok(apply_exit_code(&report.apply))
}

//! `loadout apply [--client C]... [--dry-run] [--on-conflict MODE]`
//! (SPEC 8.4, 8.7, 14): reconcile the system scope with the store. A run
//! that writes a file is one store commit; a run without writes is none.
//! Project scopes arrive with project manifests.

use agent_loadout_core::CoreError;
use agent_loadout_core::apply::{self, ApplyRequest, OnConflict, PlanOptions};
use agent_loadout_core::model::applied::ApplyTrigger;
use clap::{Args, ValueEnum};
use serde_json::json;

use super::status::{plan_json, print_plan};
use super::{Context, JSON_SCHEMA, apply_exit_code, print_json, print_report, report_json};

#[derive(Debug, Args)]
pub struct ApplyArgs {
    /// Only this client (repeatable).
    #[arg(long = "client", value_name = "CLIENT")]
    clients: Vec<String>,
    /// Print the plan and its diffs; change nothing.
    #[arg(long)]
    dry_run: bool,
    /// What to do with collisions, conflicts and drift.
    #[arg(long, value_enum, default_value = "ask")]
    on_conflict: OnConflictArg,
}

/// `--on-conflict`: `ask` leaves them and exits 4.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OnConflictArg {
    Ask,
    Skip,
    Overwrite,
    Import,
}

impl From<OnConflictArg> for OnConflict {
    fn from(arg: OnConflictArg) -> Self {
        match arg {
            OnConflictArg::Ask => OnConflict::Ask,
            OnConflictArg::Skip => OnConflict::Skip,
            OnConflictArg::Overwrite => OnConflict::Overwrite,
            OnConflictArg::Import => OnConflict::Import,
        }
    }
}

pub fn run(ctx: &Context, args: &ApplyArgs) -> Result<u8, CoreError> {
    let options = PlanOptions {
        clients: (!args.clients.is_empty()).then(|| args.clients.clone()),
        on_conflict: args.on_conflict.into(),
        decisions: Vec::new(),
        ..PlanOptions::default()
    };
    if args.dry_run {
        let store = ctx.open_store()?;
        let plan = apply::preview(store.as_ref(), &ctx.apply_env(), &options)?;
        if ctx.json {
            print_json(&json!({
                "schema": JSON_SCHEMA,
                "dry_run": true,
                "targets": plan_json(&plan),
            }));
        } else {
            print_plan(&plan);
            for target in plan.targets.iter().filter(|t| t.after.is_some()) {
                print!("{}", target.diff());
            }
        }
        return Ok(0);
    }
    let store = ctx.open_store_for_change()?;
    let request = ApplyRequest {
        options,
        trigger: ApplyTrigger::Review,
    };
    let (report, commit) = apply::run(&store, &ctx.apply_env(), &request)?;
    if ctx.json {
        let mut doc = report_json(&report, commit.as_ref());
        doc["schema"] = json!(JSON_SCHEMA);
        print_json(&doc);
    } else {
        print_report(&report, commit.as_ref());
    }
    Ok(apply_exit_code(&report))
}

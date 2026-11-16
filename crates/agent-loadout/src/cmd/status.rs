//! `loadout status [--client C]... [--check]` (SPEC 8.2, 14): the state of
//! every desired, owned and foreign entry of the system scope, plus apply
//! runs that never finished.

use agent_loadout_core::apply::journal::{self, Journal};
use agent_loadout_core::apply::{self, Plan, PlanOptions};
use agent_loadout_core::{CoreError, ErrorCode};
use clap::Args;
use serde_json::{Value, json};

use super::{Context, EXIT_PENDING, JSON_SCHEMA, entry_json, error_json, path_json, print_json};

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Only this client (repeatable).
    #[arg(long = "client", value_name = "CLIENT")]
    clients: Vec<String>,
    /// Exit with code 3 when anything is pending or drifted.
    #[arg(long)]
    check: bool,
}

pub fn run(ctx: &Context, args: &StatusArgs) -> Result<u8, CoreError> {
    let options = PlanOptions {
        clients: (!args.clients.is_empty()).then(|| args.clients.clone()),
        ..PlanOptions::default()
    };
    let store = ctx.open_store()?;
    let ax = ctx.apply_env();
    let plan = apply::preview(store.as_ref(), &ax, &options)?;
    let unfinished = journal::unfinished(ax.fs, &ctx.home.state_dir())?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "targets": plan_json(&plan),
            "warnings": plan.warnings.iter().map(error_json).collect::<Vec<_>>(),
            "unfinished": unfinished.iter().map(journal_json).collect::<Vec<_>>(),
        }));
    } else {
        print_plan(&plan);
        for run in &unfinished {
            let err = CoreError::new(
                ErrorCode::UnfinishedApply,
                format!(
                    "apply {} started {} did not finish ({} file(s) recorded)",
                    run.apply_id,
                    run.started_at,
                    run.files.len()
                ),
            );
            eprintln!("warning[{}]: {err}", err.code());
        }
    }
    let pending = plan
        .targets
        .iter()
        .flat_map(|target| &target.entries)
        .any(|entry| entry.state.is_pending() || entry.unresolved);
    Ok(if args.check && (pending || !unfinished.is_empty()) {
        EXIT_PENDING
    } else {
        0
    })
}

pub fn plan_json(plan: &Plan) -> Vec<Value> {
    plan.targets
        .iter()
        .map(|target| {
            json!({
                "clients": target.clients,
                "scope": target.scope,
                "file": path_json(&target.file),
                "problem": target.problem.as_ref().map(error_json),
                "warnings": target.warnings.iter().map(error_json).collect::<Vec<_>>(),
                "entries": target.entries.iter().map(entry_json).collect::<Vec<_>>(),
            })
        })
        .collect()
}

fn journal_json(run: &Journal) -> Value {
    serde_json::to_value(run).unwrap_or(Value::Null)
}

/// One block per file: its clients, then `state  name  (action)` per entry.
pub fn print_plan(plan: &Plan) {
    if plan.targets.is_empty() {
        println!("Nothing to manage: no server is enabled and nothing is owned.");
    }
    for target in &plan.targets {
        println!("{}  ({})", target.file.display(), target.clients.join(", "));
        if let Some(problem) = &target.problem {
            println!("  blocked[{}]: {problem}", problem.code());
        }
        for warning in &target.warnings {
            println!("  warning[{}]: {}", warning.code(), warning.message());
        }
        for entry in &target.entries {
            let mut line = format!("  {:<15} {}", entry.state.as_str(), entry.name);
            if entry.action != apply::Action::None {
                line.push_str(&format!("  ({})", entry.action.as_str()));
            }
            if let Some(problem) = &entry.problem {
                line.push_str(&format!("  {}[{}]", problem.message(), problem.code()));
            }
            if entry.unresolved {
                line.push_str("  needs a decision");
            }
            println!("{line}");
        }
    }
    for warning in &plan.warnings {
        eprintln!("warning[{}]: {warning}", warning.code());
    }
}

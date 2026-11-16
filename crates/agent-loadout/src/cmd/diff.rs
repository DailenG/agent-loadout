//! `loadout diff [--client C]... [--exit-code]` (SPEC 8.3, 14): the apply
//! plan as unified diffs of each client file. This build writes no secret
//! values (no `inline` delivery), so there is nothing to mask.

use agent_loadout_core::CoreError;
use agent_loadout_core::apply::{self, PlanOptions};
use clap::Args;
use serde_json::json;

use super::{Context, EXIT_PENDING, JSON_SCHEMA, path_json, print_json};

#[derive(Debug, Args)]
pub struct DiffArgs {
    /// Only this client (repeatable).
    #[arg(long = "client", value_name = "CLIENT")]
    clients: Vec<String>,
    /// Exit with code 3 when a file would change.
    #[arg(long)]
    exit_code: bool,
}

pub fn run(ctx: &Context, args: &DiffArgs) -> Result<u8, CoreError> {
    let options = PlanOptions {
        clients: (!args.clients.is_empty()).then(|| args.clients.clone()),
        ..PlanOptions::default()
    };
    let store = ctx.open_store()?;
    let plan = apply::preview(store.as_ref(), &ctx.apply_env(), &options)?;
    let changed: Vec<_> = plan
        .targets
        .iter()
        .filter(|target| target.after.is_some())
        .collect();
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "files": changed.iter().map(|target| json!({
                "file": path_json(&target.file),
                "clients": target.clients,
                "created": target.before.is_none(),
                "diff": target.diff(),
            })).collect::<Vec<_>>(),
        }));
    } else {
        for target in &changed {
            print!("{}", target.diff());
        }
    }
    Ok(if args.exit_code && !changed.is_empty() {
        EXIT_PENDING
    } else {
        0
    })
}

//! `loadout diff [PATH] [--client C]... [--exit-code]` (SPEC 8.3, 14): the
//! apply plan as unified diffs of each client file, with values of
//! `inline` delivery masked (`TargetPlan::diff`). PATH limits it to the
//! project holding that folder.
//!
//! `loadout diff --rev A..B [--raw]` (SPEC 9.3): the semantic store diff
//! between two revisions (`A..` is up to the latest commit), or the
//! unified git diff with `--raw`.

use std::path::PathBuf;

use agent_loadout_core::apply::{self, PlanOptions};
use agent_loadout_core::store::diff;
use agent_loadout_core::{CoreError, ErrorCode};
use clap::Args;
use serde_json::json;

use super::project::selection;
use super::show::changes_json;
use super::{Context, EXIT_PENDING, JSON_SCHEMA, path_json, print_json, require_store};

#[derive(Debug, Args)]
pub struct DiffArgs {
    /// A folder inside the project to diff [default: the user scope and
    /// every known project].
    #[arg(conflicts_with = "rev")]
    path: Option<PathBuf>,
    /// Only this client (repeatable).
    #[arg(long = "client", value_name = "CLIENT", conflicts_with = "rev")]
    clients: Vec<String>,
    /// Exit with code 3 when there is a difference.
    #[arg(long)]
    exit_code: bool,
    /// Compare two store revisions (commits or tags) instead: A..B, or A..
    /// for A against the latest commit.
    #[arg(long, value_name = "A..B")]
    rev: Option<String>,
    /// With --rev: the unified git diff instead of the semantic one.
    #[arg(long, requires = "rev")]
    raw: bool,
}

pub fn run(ctx: &Context, args: &DiffArgs) -> Result<u8, CoreError> {
    if let Some(range) = &args.rev {
        return revisions(ctx, args, range);
    }
    let options = PlanOptions {
        clients: (!args.clients.is_empty()).then(|| args.clients.clone()),
        projects: selection(ctx, args.path.as_deref())?,
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
                "project_id": target.project_id,
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

fn revisions(ctx: &Context, args: &DiffArgs, range: &str) -> Result<u8, CoreError> {
    let store = require_store(ctx, "nothing to compare")?;
    let (from, to) = range.split_once("..").ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidValue,
            format!("expected --rev A..B or A.., got {range:?}"),
        )
    })?;
    let from = store.resolve(from)?;
    let to = if to.is_empty() {
        store.head()?
    } else {
        store.resolve(to)?
    };
    let changed = if args.raw {
        let text = store.raw_diff(Some(&from), &to)?;
        if ctx.json {
            print_json(&json!({
                "schema": JSON_SCHEMA,
                "from": from.as_str(),
                "to": to.as_str(),
                "diff": text,
            }));
        } else {
            print!("{text}");
        }
        !text.is_empty()
    } else {
        let changes = store.semantic_diff(Some(&from), &to)?;
        if ctx.json {
            print_json(&json!({
                "schema": JSON_SCHEMA,
                "from": from.as_str(),
                "to": to.as_str(),
                "changes": changes_json(&changes),
            }));
        } else {
            print!("{}", diff::render(&changes));
        }
        !changes.is_empty()
    };
    Ok(if args.exit_code && changed {
        EXIT_PENDING
    } else {
        0
    })
}

//! `loadout backup [-m MSG] [--tag NAME]` (SPEC 9.6, 14): make sure the
//! store's state is committed (hand edits in the store folder become one
//! commit, after validation and the leak scan; nothing to do when clean),
//! then optionally tag it. Pushing to remotes arrives with remotes (M12).

use agent_loadout_core::CoreError;
use agent_loadout_core::store::Operation;
use clap::Args;
use serde_json::json;

use super::tag::tag_json;
use super::{Context, JSON_SCHEMA, print_json};

#[derive(Debug, Args)]
pub struct BackupArgs {
    /// Commit message for hand edits [default: "backup: N files"].
    #[arg(short, long, value_name = "MSG")]
    message: Option<String>,
    /// Also tag the result.
    #[arg(long, value_name = "NAME")]
    tag: Option<String>,
}

pub fn run(ctx: &Context, args: &BackupArgs) -> Result<(), CoreError> {
    let store = ctx.open_store_for_change()?;
    let pending = store.worktree_changes()?;
    let summary = args
        .message
        .clone()
        .unwrap_or_else(|| format!("backup: {} file(s)", pending.len()));
    let commit = if pending.is_empty() {
        None
    } else {
        store.commit_worktree(Operation::new("store.backup", summary))?
    };
    let head = store.head()?;
    let tag = match &args.tag {
        Some(name) => Some(store.create_user_tag(name, &head, args.message.as_deref())?),
        None => None,
    };
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "commit": commit.as_ref().map(ToString::to_string),
            "files": pending,
            "head": head.as_str(),
            "tag": tag.as_ref().map(tag_json),
        }));
        return Ok(());
    }
    match &commit {
        Some(commit) => println!(
            "Committed {} hand-edited file(s) as {}.",
            pending.len(),
            commit.short()
        ),
        None => println!("Nothing to commit; the store is at {}.", head.short()),
    }
    if let Some(tag) = &tag {
        println!("Tagged {} as {}.", tag.target.short(), tag.name);
    }
    Ok(())
}

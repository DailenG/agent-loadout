//! `loadout backup [-m MSG] [--tag NAME] [--remote NAME|--all] [--no-push]`
//! (SPEC 9.6, 14): make sure the store's state is committed (hand edits in
//! the store folder become one commit, after validation and the leak scan;
//! nothing to do when clean), optionally tag it, then push to the remotes
//! (every remote unless `--remote` picks one). A remote that cannot be
//! reached is a warning: the local backup is done either way.

use agent_loadout_core::CoreError;
use agent_loadout_core::remote::config::RemotesFile;
use agent_loadout_core::store::Operation;
use clap::Args;
use serde_json::json;

use super::remote::{print_pushes, push_json, push_to};
use super::tag::tag_json;
use super::{Context, JSON_SCHEMA, print_json};

#[derive(Debug, Args)]
pub struct BackupArgs {
    /// Commit message for hand edits [default: "backup: hand edits"].
    #[arg(short, long, value_name = "MSG")]
    message: Option<String>,
    /// Also tag the result.
    #[arg(long, value_name = "NAME")]
    tag: Option<String>,
    /// Push to this remote only.
    #[arg(long, value_name = "NAME", conflicts_with_all = ["all", "no_push"])]
    remote: Option<String>,
    /// Push to every remote (the default).
    #[arg(long, conflicts_with = "no_push")]
    all: bool,
    /// Do not push.
    #[arg(long)]
    no_push: bool,
}

pub fn run(ctx: &Context, args: &BackupArgs) -> Result<(), CoreError> {
    let store = ctx.open_store_for_change()?;
    let summary = args
        .message
        .clone()
        .unwrap_or_else(|| "backup: hand edits".to_owned());
    let (commit, pending) = store.commit_worktree(Operation::new("store.backup", summary))?;
    let head = store.head()?;
    let tag = match &args.tag {
        Some(name) => Some(store.create_user_tag(name, &head, args.message.as_deref())?),
        None => None,
    };
    let pushes = if args.no_push {
        Vec::new()
    } else {
        let remotes = RemotesFile::load(ctx.fs(), &ctx.home.state_dir())?;
        push_to(&store, &remotes, args.remote.as_deref())?
    };
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "commit": commit.as_ref().map(ToString::to_string),
            "files": pending,
            "head": head.as_str(),
            "tag": tag.as_ref().map(tag_json),
            "pushes": push_json(&store, &pushes),
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
    print_pushes(&store, &pushes);
    Ok(())
}

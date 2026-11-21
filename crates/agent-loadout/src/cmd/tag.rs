//! `loadout tag <NAME> [REV] [-m MSG]`, `loadout tag --list`, `loadout tag
//! --delete NAME` (SPEC 9.4, 14).

use agent_loadout_core::store::tags::TagInfo;
use agent_loadout_core::store::utc_datetime;
use agent_loadout_core::{CoreError, ErrorCode};
use clap::Args;
use serde_json::{Value, json};

use super::{Context, JSON_SCHEMA, print_json, require_store};

#[derive(Debug, Args)]
pub struct TagArgs {
    /// Tag name: letters, digits, '.', '_' or '-' (at most 64).
    #[arg(required_unless_present_any = ["list", "delete"])]
    name: Option<String>,
    /// Commit or tag to tag [default: the latest commit].
    #[arg(value_name = "REV")]
    rev: Option<String>,
    /// Tag message.
    #[arg(short, long, value_name = "MSG")]
    message: Option<String>,
    /// List every tag.
    #[arg(long, conflicts_with_all = ["name", "delete", "message"])]
    list: bool,
    /// Delete a tag.
    #[arg(long, value_name = "NAME", conflicts_with_all = ["name", "message"])]
    delete: Option<String>,
}

pub fn run(ctx: &Context, args: &TagArgs) -> Result<(), CoreError> {
    if args.list {
        return list(ctx);
    }
    let store = require_store(ctx, "nothing to tag")?;
    if let Some(name) = &args.delete {
        store.delete_tag(name)?;
        if ctx.json {
            print_json(&json!({ "schema": JSON_SCHEMA, "deleted": name }));
        } else {
            println!("Deleted tag {name}.");
        }
        return Ok(());
    }
    let name = args
        .name
        .as_deref()
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidValue, "a tag name is required"))?;
    let target = match &args.rev {
        Some(rev) => store.resolve(rev)?,
        None => store.head()?,
    };
    let tag = store.create_user_tag(name, &target, args.message.as_deref())?;
    if ctx.json {
        print_json(&json!({ "schema": JSON_SCHEMA, "tag": tag_json(&tag) }));
    } else {
        println!("Tagged {} as {}.", tag.target.short(), tag.name);
    }
    Ok(())
}

fn list(ctx: &Context) -> Result<(), CoreError> {
    let tags = match ctx.open_store()? {
        Some(store) => store.tags()?,
        None => Vec::new(),
    };
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "tags": tags.iter().map(tag_json).collect::<Vec<_>>(),
        }));
        return Ok(());
    }
    if tags.is_empty() {
        println!("No tags.");
    }
    for tag in &tags {
        let message = tag
            .message
            .as_deref()
            .map(|message| format!("  {}", message.lines().next().unwrap_or_default()))
            .unwrap_or_default();
        println!("{:<40} {}{message}", tag.name, tag.target.short());
    }
    Ok(())
}

pub fn tag_json(tag: &TagInfo) -> Value {
    json!({
        "name": tag.name,
        "kind": tag.kind.as_str(),
        "commit": tag.target.as_str(),
        "message": tag.message,
        "tagger": tag.tagger,
        "time": tag.time.map(|time| utc_datetime(time).to_string()),
    })
}

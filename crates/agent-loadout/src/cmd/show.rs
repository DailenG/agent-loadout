//! `loadout show <REV>` (SPEC 9.3, 14): one commit, its trailers and its
//! semantic diff against its first parent.

use agent_loadout_core::CoreError;
use agent_loadout_core::store::diff::{self, EntityChange};
use clap::Args;
use serde_json::{Value, json};

use super::history::entry_json;
use super::{Context, JSON_SCHEMA, print_json, require_store};

#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Commit (full or abbreviated id) or tag.
    #[arg(value_name = "REV")]
    rev: String,
    /// Show the unified git diff instead of the semantic one.
    #[arg(long)]
    raw: bool,
}

pub fn run(ctx: &Context, args: &ShowArgs) -> Result<(), CoreError> {
    let store = require_store(ctx, "nothing to show")?;
    let id = store.resolve(&args.rev)?;
    let (entry, message) = store.entry(&id)?;
    let parent = store.first_parent(&id)?;
    if ctx.json {
        let mut doc = json!({
            "schema": JSON_SCHEMA,
            "commit": entry_json(&entry),
            "message": message,
            "parent": parent.as_ref().map(|p| p.as_str()),
        });
        if args.raw {
            doc["diff"] = Value::String(store.raw_diff(parent.as_ref(), &id)?);
        } else {
            doc["changes"] = changes_json(&store.semantic_diff(parent.as_ref(), &id)?);
        }
        print_json(&doc);
        return Ok(());
    }
    println!("commit {}", entry.id);
    println!("Author: {} <{}>", entry.author_name, entry.author_email);
    println!("Date:   {}", entry.time_rfc3339());
    println!();
    for line in message.trim_end().lines() {
        println!("    {line}");
    }
    println!();
    if args.raw {
        print!("{}", store.raw_diff(parent.as_ref(), &id)?);
    } else {
        print!(
            "{}",
            diff::render(&store.semantic_diff(parent.as_ref(), &id)?)
        );
    }
    Ok(())
}

pub fn changes_json(changes: &[EntityChange]) -> Value {
    Value::Array(
        changes
            .iter()
            .map(|change| {
                json!({
                    "entity": change.entity,
                    "file": change.file,
                    "change": change.kind.as_str(),
                    "fields": change.fields.iter().map(|field| json!({
                        "field": field.field,
                        "before": field.before,
                        "after": field.after,
                    })).collect::<Vec<_>>(),
                    "text": change.text,
                })
            })
            .collect(),
    )
}

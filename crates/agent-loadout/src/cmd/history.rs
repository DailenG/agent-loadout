//! `loadout history [--entity ENTITY] [--limit N]` (SPEC 9.3, 14).

use agent_loadout_core::CoreError;
use agent_loadout_core::store::{LogEntry, LogFilter};
use clap::Args;
use serde_json::{Value, json};

use super::{Context, JSON_SCHEMA, print_json};

#[derive(Debug, Args)]
pub struct HistoryArgs {
    /// Only commits that change this entity (for example server/github).
    #[arg(long, value_name = "ENTITY")]
    entity: Option<String>,
    /// At most N commits, newest first.
    #[arg(long, value_name = "N")]
    limit: Option<usize>,
}

pub fn run(ctx: &Context, args: &HistoryArgs) -> Result<(), CoreError> {
    let filter = LogFilter {
        entity: args.entity.clone(),
        scope: None,
        limit: args.limit,
    };
    let entries = match ctx.open_store()? {
        Some(store) => store.log(&filter)?,
        None => Vec::new(),
    };
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "commits": entries.iter().map(entry_json).collect::<Vec<_>>(),
        }));
        return Ok(());
    }
    if entries.is_empty() {
        println!("No history.");
    }
    for entry in &entries {
        let machine = entry
            .trailers
            .machine
            .as_ref()
            .map_or(entry.author_name.as_str(), |machine| machine.label.as_str());
        println!(
            "{}  {}  {}  {}",
            entry.id.short(),
            entry.time_rfc3339(),
            machine,
            entry.summary
        );
    }
    Ok(())
}

pub fn entry_json(entry: &LogEntry) -> Value {
    let trailers = &entry.trailers;
    json!({
        "id": entry.id.as_str(),
        "time": entry.time_rfc3339(),
        "summary": entry.summary,
        "op": trailers.op,
        "entities": trailers.entities,
        "scope": trailers.scope.as_ref().map(ToString::to_string),
        "machine": trailers.machine.as_ref().map(|machine| json!({
            "id": machine.id,
            "label": machine.label,
        })),
        "client": trailers.client,
        "apply": trailers.apply.as_ref().map(|apply| json!({
            "id": apply.id,
            "outcome": apply.outcome,
        })),
        "author": {
            "name": entry.author_name,
            "email": entry.author_email,
        },
    })
}

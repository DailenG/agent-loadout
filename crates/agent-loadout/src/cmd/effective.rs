//! `loadout effective [PATH] [--client C]...` (SPEC 6.4, 14): every server
//! a client loads in a folder, the layer and file it comes from, what it
//! shadows, its plan state and warnings.

use std::path::PathBuf;

use agent_loadout_core::CoreError;
use agent_loadout_core::layering::{EffectiveEntry, EffectiveView, effective};
use clap::Args;
use serde_json::json;

use super::project::folder;
use super::{Context, JSON_SCHEMA, print_json};

#[derive(Debug, Args)]
pub struct EffectiveArgs {
    /// The folder to look at [default: the current folder].
    path: Option<PathBuf>,
    /// Only this client (repeatable) [default: every client with MCP
    /// support on this OS].
    #[arg(long = "client", value_name = "CLIENT")]
    clients: Vec<String>,
}

pub fn run(ctx: &Context, args: &EffectiveArgs) -> Result<(), CoreError> {
    let folder = folder(args.path.as_deref())?;
    let store = ctx.open_store()?;
    let clients = (!args.clients.is_empty()).then_some(args.clients.as_slice());
    let view = effective(store.as_ref(), &ctx.apply_env(), &folder, clients)?;
    for warning in &view.warnings {
        eprintln!("warning[{}]: {warning}", warning.code());
    }
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "project": view.project,
            "clients": view.clients,
        }));
    } else {
        print_view(&view);
    }
    Ok(())
}

/// The project line, then one table per client and the warnings of its
/// entries.
fn print_view(view: &EffectiveView) {
    match &view.project {
        Some(project) => println!("Project {} ({})", project.id, project.root.display()),
        None => println!("No project here: user scope only."),
    }
    for (client, entries) in &view.clients {
        println!();
        println!("{client}");
        if entries.is_empty() {
            println!("  (no servers)");
            continue;
        }
        let rows: Vec<[String; 6]> = entries.iter().map(row).collect();
        let header = ["NAME", "LOADED", "WINNER", "SHADOWED", "STATUS", "WARNINGS"];
        let mut widths = header.map(str::len);
        for row in &rows {
            for (width, cell) in widths.iter_mut().zip(row) {
                *width = (*width).max(cell.len());
            }
        }
        for cells in std::iter::once(header.map(str::to_owned)).chain(rows) {
            let line: Vec<String> = cells
                .iter()
                .zip(widths)
                .map(|(cell, width)| format!("{cell:<width$}"))
                .collect();
            println!("  {}", line.join("  ").trim_end());
        }
        for entry in entries {
            for warning in &entry.warnings {
                println!("  warning[{}] {}: {warning}", warning.code(), entry.name);
            }
        }
    }
}

fn row(entry: &EffectiveEntry) -> [String; 6] {
    let layer = entry.winner.layer.to_string();
    // `foreign:<file>` and `client-compat:<file>` name the file already.
    let winner = if layer.ends_with(&entry.winner.file) {
        layer
    } else {
        format!("{layer} {}", entry.winner.file)
    };
    let shadowed: Vec<String> = entry
        .shadowed
        .iter()
        .map(|shadowed| shadowed.layer.to_string())
        .collect();
    let warnings: Vec<&str> = entry
        .warnings
        .iter()
        .map(|warning| warning.code().as_str())
        .collect();
    let dash = |text: String| {
        if text.is_empty() {
            "-".to_owned()
        } else {
            text
        }
    };
    [
        entry.name.clone(),
        if entry.loaded { "yes" } else { "no" }.to_owned(),
        winner,
        dash(shadowed.join(", ")),
        entry.status.map_or("-", |state| state.as_str()).to_owned(),
        dash(warnings.join(", ")),
    ]
}

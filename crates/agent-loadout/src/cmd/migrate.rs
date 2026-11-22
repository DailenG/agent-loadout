//! `loadout migrate skilldock [--apply]` (SPEC 15.1, 14): bring SkillDock's
//! `~/.skilldock` into the store. Without `--apply` it is a dry run that
//! prints the report and writes nothing. `--apply` asks for confirmation
//! (`--yes` answers it; without a terminal and without `--yes` it fails
//! with `confirmation_required`), then tags the store
//! `auto/pre-migrate/<ts>` and migrates in one commit. `~/.skilldock` is
//! only read; no client file is written (the first apply is a separate,
//! previewed step).

use std::io::{BufRead, IsTerminal, Write};

use agent_loadout_core::migrate::{self, Offer, Report};
use agent_loadout_core::{CoreError, ErrorCode, brand};
use clap::{Args, Subcommand};
use serde_json::json;

use super::{Context, JSON_SCHEMA, print_json};

#[derive(Debug, Args)]
pub struct MigrateArgs {
    #[command(subcommand)]
    command: MigrateCommand,
}

#[derive(Debug, Subcommand)]
enum MigrateCommand {
    /// From SkillDock (~/.skilldock, read only): servers, skills, tags,
    /// settings, the backup repository.
    Skilldock(SkilldockArgs),
}

#[derive(Debug, Args)]
struct SkilldockArgs {
    /// Migrate (one commit after a pre-migrate tag); without it, only
    /// report what would happen.
    #[arg(long)]
    apply: bool,
}

pub fn run(ctx: &Context, args: &MigrateArgs) -> Result<u8, CoreError> {
    match &args.command {
        MigrateCommand::Skilldock(args) => skilldock(ctx, args),
    }
}

fn skilldock(ctx: &Context, args: &SkilldockArgs) -> Result<u8, CoreError> {
    let ax = ctx.apply_env();
    if !args.apply || !ctx.yes {
        let store = ctx.open_store()?;
        let migration = migrate::plan(store.as_ref(), &ax)?;
        if !args.apply {
            if ctx.json {
                print_json(&json!({
                    "schema": JSON_SCHEMA,
                    "dry_run": true,
                    "report": migration.report,
                    "files": migration.files().collect::<Vec<_>>(),
                }));
            } else {
                print_items(&migration.report);
                println!(
                    "Dry run: nothing was written. Run `{} migrate skilldock --apply` to migrate.",
                    brand::BIN_NAME
                );
            }
            return Ok(0);
        }
        if ctx.json || !std::io::stdin().is_terminal() {
            return Err(CoreError::new(
                ErrorCode::ConfirmationRequired,
                "migrating writes to the store; pass --yes to confirm",
            ));
        }
        print_items(&migration.report);
        if !confirm(&migration.report)? {
            println!("Nothing migrated.");
            return Ok(0);
        }
    }
    let store = ctx.open_store_for_change()?;
    let applied = migrate::apply(&store, &ax)?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "dry_run": false,
            "report": applied.report,
            "tag": applied.tag,
            "post_tag": applied.post_tag,
            "commit": applied.commit.as_ref().map(ToString::to_string),
            "backup_import": applied.backup_import,
        }));
    } else {
        if ctx.yes {
            print_items(&applied.report);
        }
        match &applied.commit {
            Some(commit) => println!(
                "Migrated {} server{} and {} skill{} (commit {}, tag {}).",
                applied.report.servers.len(),
                plural(applied.report.servers.len()),
                applied.report.skills.len(),
                plural(applied.report.skills.len()),
                commit.short(),
                applied.tag
            ),
            None => println!("Nothing left to migrate (tag {}).", applied.tag),
        }
        if let Some(path) = &applied.backup_import {
            println!("SkillDock's backup repository is at <LOADOUT_HOME>/{path}.");
        }
        println!(
            "Client files are unchanged; review the plan with `{bin} apply --dry-run`, then run \
             `{bin} apply`. Keep SkillDock's MCP sync off from now on: both apps writing the same \
             files causes churn. ~/.skilldock is left as it was, for rollback.",
            bin = brand::BIN_NAME
        );
    }
    Ok(0)
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn confirm(report: &Report) -> Result<bool, CoreError> {
    print!(
        "Migrate {} server{} and {} skill{} into the store? [y/N] ",
        report.servers.len(),
        plural(report.servers.len()),
        report.skills.len(),
        plural(report.skills.len())
    );
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|err| CoreError::new(ErrorCode::IoError, format!("cannot read stdin: {err}")))?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "Yes"))
}

/// One line per item, its follow-ups indented, then the counts.
fn print_items(report: &Report) {
    for item in &report.items {
        let id = item
            .id
            .as_ref()
            .filter(|id| **id != item.name)
            .map_or_else(String::new, |id| format!(" -> {id}"));
        let code = item
            .code
            .map_or_else(String::new, |code| format!(" [{code}]"));
        println!(
            "{:<13} {:<12} {}{id}: {}{code}",
            item.status.as_str(),
            item.kind.as_str(),
            item.name,
            item.detail
        );
        for offer in &item.offers {
            match offer {
                Offer::ReplaceWrapper { command, .. } => {
                    println!("{:27}replace the wrapper: {command}", "");
                }
                Offer::ReviewEntry { command, .. } => println!("{:27}overwrite: {command}", ""),
                Offer::SetSecret { command, .. } => println!("{:27}store the value: {command}", ""),
                Offer::MachinePath { .. } => {}
                Offer::KeyringToken { command } => println!("{:27}{command}", ""),
                Offer::AddRemote { url } => {
                    println!("{:27}{} remote add git <NAME> {url}", "", brand::BIN_NAME)
                }
            }
        }
    }
    let (migrated, review, skipped) = report.counts();
    println!(
        "{} item{}: {migrated} migrated, {review} to review, {skipped} skipped; {} value{} for the \
         secret extraction wizard.",
        report.items.len(),
        plural(report.items.len()),
        report.extractions.len(),
        plural(report.extractions.len())
    );
}

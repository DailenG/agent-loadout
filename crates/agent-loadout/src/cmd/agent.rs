//! `loadout agent install|remove|status|skill-path` (#8, SPEC 12.6, 14): the
//! read-only Loadout skill for coding agents, bundled with this binary and
//! installed as a normal store skill (id `loadout`), plus the update of the
//! store copy that commands changing the store make first when the bundled
//! one is newer. Read commands never write.

use agent_loadout_core::CoreError;
use agent_loadout_core::agent::{self, AgentStatus, Change, Refresh};
use agent_loadout_core::skills::SyncReport;
use agent_loadout_core::store::Store;
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use super::skills::{exit_for, print_links, report_json, skills_env};
use super::{Context, JSON_SCHEMA, path_json, print_json};

#[derive(Debug, Args)]
pub struct AgentArgs {
    #[command(subcommand)]
    command: AgentCommand,
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
    /// Install the Loadout skill (read-only; bundled with Loadout) as the
    /// store skill `loadout` and enable it for clients. An older store copy
    /// you did not edit is updated first.
    Install(InstallArgs),
    /// Disable the Loadout skill for clients; without --client, remove it
    /// from the store with its links and copies.
    Remove(ClientArgs),
    /// Show the bundled version, the store copy and the clients it is
    /// enabled for.
    Status,
    /// Print a folder holding the bundled skill (kept in the data
    /// directory's cache), to point an agent at by hand.
    SkillPath,
}

#[derive(Debug, Args)]
struct ClientArgs {
    /// Client id (repeatable).
    #[arg(long = "client", value_name = "CLIENT")]
    clients: Vec<String>,
}

#[derive(Debug, Args)]
struct InstallArgs {
    /// Client id (repeatable) [default: every detected client with a skill
    /// folder; with --reset, the clients it is enabled for stay as they
    /// are].
    #[arg(long = "client", value_name = "CLIENT")]
    clients: Vec<String>,
    /// Replace the store copy with the bundled one even if you edited it
    /// (your edits stay in History).
    #[arg(long)]
    reset: bool,
}

pub fn run(ctx: &Context, args: &AgentArgs) -> Result<u8, CoreError> {
    let sx = skills_env(ctx);
    match &args.command {
        AgentCommand::Install(args) => {
            let store = ctx.open_store_for_change()?;
            let change = agent::install(&store, &sx, &args.clients, args.reset)?;
            changed(ctx, &change, "Installed the Loadout skill")
        }
        AgentCommand::Remove(args) => {
            let Some(store) = ctx.open_store()? else {
                return changed(ctx, &Change::default(), "");
            };
            let change = agent::remove(&store, &sx, &args.clients)?;
            changed(ctx, &change, "")
        }
        AgentCommand::Status => {
            let store = ctx.open_store()?;
            let status = agent::status(store.as_ref(), &sx)?;
            if ctx.json {
                print_json(&status_json(&status));
            } else {
                print_status(&status);
            }
            Ok(0)
        }
        AgentCommand::SkillPath => {
            let dir = agent::materialize(ctx.fs(), &ctx.home)?;
            if ctx.json {
                print_json(&json!({ "schema": JSON_SCHEMA, "path": path_json(&dir) }));
            } else {
                println!("{}", dir.display());
            }
            Ok(0)
        }
    }
}

fn changed(ctx: &Context, change: &Change, installed: &str) -> Result<u8, CoreError> {
    let attention = exit_for(&change.report).max(refresh_exit(change.refreshed.as_ref()));
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "commit": change.commit.as_ref().map(|commit| commit.as_str()),
            "reset": change.reset,
            "removed": change.removed,
            "refreshed": change.refreshed.as_ref().map(refresh_json),
            "links": report_json(&change.report),
        }));
        return Ok(attention);
    }
    if let Some(refreshed) = &change.refreshed {
        println!("{}", refresh_text(refreshed));
        if let Refresh::Updated { report, .. } = refreshed {
            print_links(report);
        }
    }
    let message = if change.removed {
        "Removed the Loadout skill from the store.".to_owned()
    } else if change.reset {
        format!(
            "Replaced the Loadout skill in the store with the bundled copy ({}).",
            agent::BUNDLED_VERSION
        )
    } else if !installed.is_empty() {
        format!("{installed} ({}).", agent::BUNDLED_VERSION)
    } else if change.commit.is_some() {
        "Disabled the Loadout skill for the clients given.".to_owned()
    } else {
        "Nothing to change.".to_owned()
    };
    println!("{message}");
    print_links(&change.report);
    Ok(attention)
}

/// Exit 4 when the update's skill sync left a client folder needing a
/// decision.
fn refresh_exit(refresh: Option<&Refresh>) -> u8 {
    match refresh {
        Some(Refresh::Updated { report, .. }) => exit_for(report),
        _ => 0,
    }
}

fn refresh_json(refresh: &Refresh) -> Value {
    match refresh {
        Refresh::Updated {
            from,
            to,
            commit,
            report,
        } => json!({
            "kind": "updated",
            "from": from,
            "to": to,
            "commit": commit.as_ref().map(|commit| commit.as_str()),
            "links": report_json(report),
        }),
        Refresh::Customized { version, bundled } => json!({
            "kind": "customized",
            "version": version,
            "bundled": bundled,
            "action": agent::RESET_COMMAND,
        }),
    }
}

fn refresh_text(refresh: &Refresh) -> String {
    match refresh {
        Refresh::Updated { from, to, .. } => {
            format!("Updated the Loadout skill in the store from {from} to {to}.")
        }
        Refresh::Customized { version, bundled } => format!(
            "The Loadout skill in the store is customized, not updated (store copy {version}, bundled {bundled}); `{}` replaces it.",
            agent::RESET_COMMAND
        ),
    }
}

fn status_json(status: &AgentStatus) -> Value {
    json!({
        "schema": JSON_SCHEMA,
        "skill": agent::SKILL_ID,
        "bundled_version": status.bundled_version,
        "copy": status.copy.as_ref().map(|copy| json!({
            "version": copy.version,
            "state": copy.state.as_str(),
            "action": copy.state.action(),
        })),
        "taken": status.taken,
        "clients": status.clients.iter().map(|client| json!({
            "client": client.client,
            "detected": client.detected,
            "enabled": client.enabled,
        })).collect::<Vec<_>>(),
    })
}

fn print_status(status: &AgentStatus) {
    println!("Loadout skill {} (bundled)", status.bundled_version);
    let copy = match &status.copy {
        Some(copy) => {
            let action = copy
                .state
                .action()
                .map(|action| format!("; run `{action}` to replace it"))
                .unwrap_or_default();
            format!("{}, {}{action}", copy.version, copy.state.as_str())
        }
        None if status.taken => format!(
            "your own skill uses the id {:?}; remove it (loadout skills rm {0}) or add it again under another id (--id) to install the Loadout skill",
            agent::SKILL_ID
        ),
        None => "not installed".to_owned(),
    };
    println!("Store copy: {copy}");
    for client in &status.clients {
        let state = match (client.enabled, client.detected) {
            (true, _) => "enabled",
            (false, true) => "detected",
            (false, false) => "not detected",
        };
        println!("  {:<12} {state}", client.client);
    }
}

/// The update a store change makes first (#8, SPEC 12.6): when the store
/// copy of the Loadout skill is older than the bundled one, it is updated
/// (one commit), or reported once as customized. Called with the store a
/// changing command just opened; read commands never get here. Notes,
/// client folders needing a decision and failures go to stderr; none of
/// them changes the command's result.
pub fn refresh_before_change(ctx: &Context, store: &Store<'_>) {
    if !agent::written_older(ctx.fs(), &ctx.home) {
        return;
    }
    match agent::startup_refresh(store, &skills_env(ctx)) {
        Ok(Some(refresh)) => {
            eprintln!("note: {}", refresh_text(&refresh));
            if let Refresh::Updated { report, .. } = &refresh {
                eprint_links(report);
            }
        }
        Ok(None) => {}
        Err(err) => eprintln!(
            "warning[{}]: the Loadout skill was not updated: {}",
            err.code(),
            err.message()
        ),
    }
}

/// [`print_links`] on stderr, for notes that must not mix with a
/// command's stdout.
fn eprint_links(report: &SyncReport) {
    for result in report.notable() {
        let detail = match (&result.error, &result.found_target) {
            (Some(err), _) => format!(" [{}] {}", err.code(), err.message()),
            (None, Some(target)) => format!(" -> {}", target.display()),
            _ => String::new(),
        };
        eprintln!(
            "  {:<10} {:<12} {}{detail}",
            result.status.as_str(),
            result.client,
            result.path.display()
        );
    }
    for warning in &report.warnings {
        eprintln!("warning[{}]: {}", warning.code(), warning.message());
    }
}

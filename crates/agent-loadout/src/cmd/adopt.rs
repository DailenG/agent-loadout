//! `loadout adopt [CLIENT/NAME]... [--all-identical] [--theirs]` (SPEC 8.6,
//! 14): take ownership of entries that need a decision. Named entries are
//! overwritten with the store's definition (a collision is adopted), or with
//! `--theirs` their native version becomes the store's (`adopt: <client>/<name>
//! drift`). `--all-identical` enables store servers for every client that
//! already holds an identical entry, which the apply then adopts without a
//! write. One commit, when anything changed.

use agent_loadout_core::apply::import;
use agent_loadout_core::apply::ledger::Ledger;
use agent_loadout_core::apply::render::DesiredState;
use agent_loadout_core::apply::{
    self, Action, ApplyReport, ApplyRequest, Decision, OnConflict, PlanOptions,
};
use agent_loadout_core::model::applied::ApplyTrigger;
use agent_loadout_core::store::Operation;
use agent_loadout_core::{CoreError, ErrorCode};
use clap::Args;
use serde_json::json;

use super::{Context, JSON_SCHEMA, apply_exit_code, print_json, print_report, report_json};

#[derive(Debug, Args)]
pub struct AdoptArgs {
    /// Entries as CLIENT/NAME (for example claude-code/github).
    #[arg(value_name = "CLIENT/NAME", required_unless_present = "all_identical")]
    entries: Vec<String>,
    /// Adopt every foreign entry identical to a store server.
    #[arg(long)]
    all_identical: bool,
    /// Take the client's version into the store instead of overwriting it.
    #[arg(long)]
    theirs: bool,
}

pub fn run(ctx: &Context, args: &AdoptArgs) -> Result<u8, CoreError> {
    let resolution = if args.theirs {
        OnConflict::Import
    } else {
        OnConflict::Overwrite
    };
    let mut decisions = Vec::new();
    for entry in &args.entries {
        let Some((client, name)) = entry
            .split_once('/')
            .filter(|(c, n)| !c.is_empty() && !n.is_empty())
        else {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!("{entry:?} is not CLIENT/NAME"),
            ));
        };
        decisions.push(Decision {
            client: client.to_owned(),
            name: name.to_owned(),
            resolution,
        });
    }
    let store = ctx.open_store_for_change()?;
    let ax = ctx.apply_env();
    let committed = store.transaction(Operation::new("entry.adopt", "adopt"), |tx| {
        let mut adopted = Vec::new();
        if args.all_identical {
            let desired = DesiredState::load(tx, Some(&tx.machine().id.clone()))?;
            let ledger = Ledger::load(ax.fs, &ctx.home.state_dir())?;
            let (candidates, _) = import::scan(&ax, &desired, &ledger, None)?;
            adopted = import::stage_adopt_identical(tx, &ax, &desired, &candidates)?;
        }
        let request = ApplyRequest {
            options: PlanOptions {
                clients: None,
                on_conflict: OnConflict::Ask,
                decisions: decisions.clone(),
                ..PlanOptions::default()
            },
            trigger: ApplyTrigger::Review,
        };
        let report = apply::apply_in(tx, &ax, &request)?;
        let mut named: Vec<String> = adopted.clone();
        named.extend(args.entries.iter().cloned());
        let drift = if args.theirs { " drift" } else { "" };
        tx.set_summary(format!("adopt: {}{drift}", named.join(", ")));
        Ok((adopted, report))
    })?;
    let (adopted, report) = committed.value;
    let commit = committed.commit;
    let untouched = untouched(&report, &decisions);
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "identical": adopted,
            "not_adopted": untouched,
            "apply": report_json(&report, commit.as_ref()),
        }));
    } else {
        for entry in &adopted {
            println!("adopted {entry} (identical)");
        }
        for entry in &untouched {
            eprintln!("warning: {entry} needs no decision or does not exist; nothing adopted");
        }
        print_report(&report, commit.as_ref());
    }
    Ok(apply_exit_code(&report))
}

/// Named entries the run did not act on.
fn untouched(report: &ApplyReport, decisions: &[Decision]) -> Vec<String> {
    decisions
        .iter()
        .filter(|decision| {
            let acted = report.targets.iter().flat_map(|t| &t.entries).any(|entry| {
                entry.client == Some(decision.client.as_str())
                    && entry.name == decision.name
                    && (!matches!(entry.action, Action::None | Action::Skip)
                        || entry
                            .server_id
                            .as_ref()
                            .is_some_and(|id| report.imported.contains(id)))
            });
            !acted
        })
        .map(|decision| format!("{}/{}", decision.client, decision.name))
        .collect()
}

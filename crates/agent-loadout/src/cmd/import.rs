//! `loadout import [--scan] [--client C]... [--only] [NAME]...` (SPEC 8.6,
//! 14): list foreign entries of the clients' user files, or turn them into
//! store servers. In instant mode the import is applied in the same commit,
//! which adopts every entry that renders identically without writing it.

use agent_loadout_core::CoreError;
use agent_loadout_core::apply::import::{self, Candidate};
use agent_loadout_core::apply::ledger::Ledger;
use agent_loadout_core::apply::render::DesiredState;
use agent_loadout_core::apply::{self, ApplyRequest, PlanOptions};
use agent_loadout_core::model::applied::ApplyTrigger;
use agent_loadout_core::model::settings::ApplyMode;
use agent_loadout_core::store::{Machine, Operation};
use clap::Args;
use serde_json::{Value, json};

use super::{
    Context, JSON_SCHEMA, apply_exit_code, error_json, path_json, print_json, print_report,
    report_json,
};

#[derive(Debug, Args)]
pub struct ImportArgs {
    /// List candidates; change nothing.
    #[arg(long)]
    scan: bool,
    /// Only entries of this client (repeatable).
    #[arg(long = "client", value_name = "CLIENT")]
    clients: Vec<String>,
    /// Copy the definitions without enabling them (no ownership taken).
    #[arg(long)]
    only: bool,
    /// Entry names to import; all importable entries when none.
    #[arg(value_name = "NAME")]
    names: Vec<String>,
}

pub fn run(ctx: &Context, args: &ImportArgs) -> Result<u8, CoreError> {
    let clients = (!args.clients.is_empty()).then_some(args.clients.as_slice());
    let ax = ctx.apply_env();
    if args.scan {
        let store = ctx.open_store()?;
        let machine = Machine::load(ax.fs, &ctx.home.state_dir())?;
        let desired = match &store {
            Some(store) => DesiredState::load(store, machine.as_ref().map(|m| m.id.as_str()))?,
            None => DesiredState::empty(),
        };
        let ledger = Ledger::load(ax.fs, &ctx.home.state_dir())?;
        let (candidates, warnings) = import::scan(&ax, &desired, &ledger, clients)?;
        if ctx.json {
            print_json(&json!({
                "schema": JSON_SCHEMA,
                "candidates": candidates.iter().map(candidate_json).collect::<Vec<_>>(),
                "warnings": warnings.iter().map(error_json).collect::<Vec<_>>(),
            }));
        } else {
            print_candidates(&candidates);
            for warning in &warnings {
                eprintln!("warning[{}]: {warning}", warning.code());
            }
        }
        return Ok(0);
    }

    let store = ctx.open_store_for_change()?;
    let names = (!args.names.is_empty()).then_some(args.names.as_slice());
    let committed = store.transaction(Operation::new("server.import", "import"), |tx| {
        let desired = DesiredState::load(tx, Some(&tx.machine().id.clone()))?;
        let ledger = Ledger::load(ax.fs, &ctx.home.state_dir())?;
        let (candidates, warnings) = import::scan(&ax, &desired, &ledger, clients)?;
        let result = import::stage_import(tx, &candidates, names, args.only)?;
        let count = result.imported.len();
        tx.set_summary(format!(
            "import: {count} server{} ({})",
            if count == 1 { "" } else { "s" },
            result.imported.join(", ")
        ));
        let report = if count > 0 && !args.only && desired.apply_mode() == ApplyMode::Instant {
            let request = ApplyRequest {
                options: PlanOptions::default(),
                trigger: ApplyTrigger::Instant,
            };
            Some(apply::apply_in(tx, &ax, &request)?)
        } else {
            None
        };
        Ok((result, report, warnings))
    })?;
    let (result, report, warnings) = committed.value;
    let commit = committed.commit;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "imported": result.imported,
            "skipped": result.skipped.iter().map(|(name, err)| json!({
                "name": name,
                "error": error_json(err),
            })).collect::<Vec<_>>(),
            "commit": commit.as_ref().map(ToString::to_string),
            "apply": report.as_ref().map(|report| report_json(report, commit.as_ref())),
            "warnings": warnings.iter().map(error_json).collect::<Vec<_>>(),
        }));
    } else {
        for warning in &warnings {
            eprintln!("warning[{}]: {warning}", warning.code());
        }
        for (name, err) in &result.skipped {
            println!("skipped {name}: {}[{}]", err.message(), err.code());
        }
        if result.imported.is_empty() {
            println!("Nothing imported.");
        } else {
            println!(
                "Imported {} (commit {}).",
                result.imported.join(", "),
                commit.as_ref().map_or("none", |c| c.short())
            );
        }
        if let Some(report) = &report {
            print_report(report, commit.as_ref());
        }
    }
    Ok(report.as_ref().map_or(0, apply_exit_code))
}

fn candidate_json(candidate: &Candidate) -> Value {
    json!({
        "name": candidate.name,
        "identical": candidate.identical,
        "in_store": candidate.in_store,
        "sources": candidate.sources.iter().map(|source| json!({
            "client": source.client,
            "file": path_json(&source.file),
            "importable": source.def.is_ok(),
            "error": source.def.as_ref().err().map(error_json),
        })).collect::<Vec<_>>(),
    })
}

fn print_candidates(candidates: &[Candidate]) {
    if candidates.is_empty() {
        println!("No foreign entries.");
    }
    for candidate in candidates {
        let mut notes = Vec::new();
        if !candidate.identical {
            notes.push("differs between clients");
        }
        if candidate.in_store {
            notes.push("already in the store");
        }
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!("  ({})", notes.join("; "))
        };
        println!("{}{notes}", candidate.name);
        for source in &candidate.sources {
            match &source.def {
                Ok(_) => println!("  {}  {}", source.client, source.file.display()),
                Err(err) => println!(
                    "  {}  {}  {}[{}]",
                    source.client,
                    source.file.display(),
                    err.message(),
                    err.code()
                ),
            }
        }
    }
}

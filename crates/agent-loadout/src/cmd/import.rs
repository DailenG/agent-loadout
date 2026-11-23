//! `loadout import [--scan] [--client C]... [--scope S] [--project ID]
//! [--only] [--pick NAME=CLIENT]... [--as NAME=CLIENT]... [--skip NAME]...
//! [--extract NAME[:FIELD]=DEST]... [--remove-old-backups[=APPLY-ID]]
//! [NAME]...` (SPEC 8.6, 11.7, 14): list the foreign entries of the
//! clients' user files and known projects' files, or turn them into store
//! servers in one commit. In instant mode the import is applied in the
//! same commit, which adopts every entry that renders identically without
//! writing it. The backups that apply made of files holding an extracted
//! secret keep its old value: they are listed, and removed with
//! `--remove-old-backups` (in the same run, or later for an earlier
//! import's apply id).

use std::path::PathBuf;

use agent_loadout_core::CoreError;
use agent_loadout_core::ErrorCode;
use agent_loadout_core::apply::backup::{self, Purge};
use agent_loadout_core::apply::import::{
    self, Candidate, ImportRequest, ScanFilter, ScanScope, SecretChoice, SecretDestination,
    VariantPreview,
};
use agent_loadout_core::apply::ledger::Ledger;
use agent_loadout_core::apply::placeholder_path;
use agent_loadout_core::apply::render::DesiredState;
use agent_loadout_core::apply::unmanaged::finding_id;
use agent_loadout_core::brand;
use agent_loadout_core::model::applied::Scope;
use agent_loadout_core::store::Operation;
use clap::Args;
use serde_json::{Value, json};

use super::apply::ScopeArg;
use super::{
    Context, JSON_SCHEMA, apply_exit_code, error_json, path_json, print_json, print_report,
    report_json,
};

#[derive(Debug, Args)]
pub struct ImportArgs {
    /// List candidates, their sources, conflicts and secrets; change
    /// nothing.
    #[arg(long)]
    scan: bool,
    /// Only entries of this client (repeatable).
    #[arg(long = "client", value_name = "CLIENT")]
    clients: Vec<String>,
    /// Which files to read: user files (system), the files of the known
    /// projects, or both.
    #[arg(long, value_enum, default_value = "all")]
    scope: ScopeArg,
    /// Only the files of this known project (its id).
    #[arg(long, value_name = "ID", conflicts_with = "scope")]
    project: Option<String>,
    /// Copy the definitions without enabling them (no ownership taken).
    #[arg(long)]
    only: bool,
    /// Import NAME as CLIENT has it, under NAME (repeatable; picks one
    /// definition of an entry that differs between clients). CLIENT may be
    /// a source key from --scan (omp#work, claude-code@PROJECT-ID) for a
    /// client's other file.
    #[arg(long = "pick", value_name = "NAME=CLIENT")]
    picks: Vec<String>,
    /// Import NAME as CLIENT has it, as NAME-CLIENT (repeatable; with
    /// --pick, imports both definitions). CLIENT may be a source key.
    #[arg(long = "as", value_name = "NAME=CLIENT")]
    renames: Vec<String>,
    /// Leave NAME out (repeatable).
    #[arg(long = "skip", value_name = "NAME")]
    skips: Vec<String>,
    /// Where NAME's literal secrets go: keyring, op://VAULT/ITEM/FIELD,
    /// op-env://ENV/NAME or env:VAR. NAME:FIELD (env.KEY, headers.KEY,
    /// args[N], url) sets one field (repeatable).
    #[arg(long = "extract", value_name = "NAME[:FIELD]=DEST")]
    extracts: Vec<String>,
    /// Remove the client file backups that still hold the old values of
    /// extracted secrets: those of this import's apply, or with =APPLY-ID
    /// those of an earlier import (then nothing is imported). Undoing that
    /// import can then no longer bring those files' earlier content back.
    #[arg(
        long,
        value_name = "APPLY-ID",
        num_args = 0..=1,
        require_equals = true,
        conflicts_with = "scan"
    )]
    remove_old_backups: Option<Option<String>>,
    /// Entry names to import; all importable entries when none.
    #[arg(value_name = "NAME")]
    names: Vec<String>,
}

impl ImportArgs {
    fn filter(&self) -> ScanFilter {
        ScanFilter {
            clients: (!self.clients.is_empty()).then(|| self.clients.clone()),
            scope: match (&self.project, self.scope) {
                (Some(id), _) => ScanScope::Project(id.clone()),
                (None, ScopeArg::System) => ScanScope::User,
                (None, ScopeArg::Project) => ScanScope::Projects,
                (None, ScopeArg::All) => ScanScope::All,
            },
        }
    }

    /// The request: every named (or every) candidate with one definition,
    /// then `--pick`/`--as` choices and `--skip`, and `--extract`.
    fn request(&self, candidates: &[Candidate]) -> Result<ImportRequest, CoreError> {
        let names = (!self.names.is_empty()).then_some(self.names.as_slice());
        let mut request = ImportRequest::defaults(candidates, names, self.only);
        let mut chosen: Vec<(String, Vec<import::VariantChoice>)> = Vec::new();
        for (text, rename) in self
            .picks
            .iter()
            .map(|t| (t, false))
            .chain(self.renames.iter().map(|t| (t, true)))
        {
            let (name, client) = text
                .rsplit_once('=')
                .ok_or_else(|| usage(&format!("{text:?} is not NAME=CLIENT")))?;
            let candidate = candidates
                .iter()
                .find(|candidate| candidate.name == name)
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::NotFound,
                        format!("there is no foreign entry {name:?}"),
                    )
                })?;
            let choice = candidate.choice_for(client, rename)?;
            match chosen.iter_mut().find(|(n, _)| n == name) {
                Some((_, picks)) => picks.push(choice),
                None => chosen.push((name.to_owned(), vec![choice])),
            }
        }
        for (name, picks) in chosen {
            request.choose(&name, picks);
        }
        for name in &self.skips {
            request.choose(name, Vec::new());
        }
        for text in &self.extracts {
            request.secrets.push(secret_choice(text)?);
        }
        Ok(request)
    }
}

fn usage(message: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidValue, message.to_owned())
}

/// `NAME=DEST` or `NAME:FIELD=DEST`.
fn secret_choice(text: &str) -> Result<SecretChoice, CoreError> {
    let (target, destination) = text
        .split_once('=')
        .ok_or_else(|| usage(&format!("{text:?} is not NAME[:FIELD]=DEST")))?;
    let is_field = |field: &str| {
        field == "url"
            || field.starts_with("env.")
            || field.starts_with("headers.")
            || field.starts_with("args[")
    };
    let (name, field) = match target.rsplit_once(':') {
        Some((name, field)) if is_field(field) => (name, Some(field.to_owned())),
        _ => (target, None),
    };
    Ok(SecretChoice {
        name: name.to_owned(),
        field,
        destination: SecretDestination::parse(destination)?,
    })
}

pub fn run(ctx: &Context, args: &ImportArgs) -> Result<u8, CoreError> {
    let filter = args.filter();
    let ax = ctx.apply_env();
    if args.scan {
        let store = ctx.open_store()?;
        let scan = import::scan_store(store.as_ref(), &ax, &filter)?;
        let candidates = scan.candidates;
        let previews = import::previews(&candidates, &scan.desired.catalog);
        if ctx.json {
            print_json(&json!({
                "schema": JSON_SCHEMA,
                "candidates": candidates
                    .iter()
                    .zip(&previews)
                    .map(|(candidate, previews)| candidate_json(candidate, previews))
                    .collect::<Vec<_>>(),
                "warnings": scan.warnings.iter().map(error_json).collect::<Vec<_>>(),
            }));
        } else {
            print_candidates(&candidates, &previews);
            for warning in &scan.warnings {
                eprintln!("warning[{}]: {warning}", warning.code());
            }
        }
        return Ok(0);
    }
    if let Some(Some(apply_id)) = &args.remove_old_backups {
        return purge_only(ctx, args, apply_id);
    }

    let store = ctx.open_store_for_change()?;
    let committed = store.transaction(Operation::new("server.import", "import"), |tx| {
        let desired = DesiredState::load(tx, Some(&tx.machine().id.clone()))?;
        let ledger = Ledger::load(ax.fs, &ctx.home.state_dir())?;
        let (candidates, warnings) = import::scan_with(&ax, &desired, &ledger, &filter)?;
        let request = args.request(&candidates)?;
        let mut result = import::stage_import(tx, &ax, &candidates, &request)?;
        let count = result.imported.len();
        tx.set_summary(format!(
            "import: {count} server{} ({})",
            if count == 1 { "" } else { "s" },
            result.imported.join(", ")
        ));
        let applied = import::apply_import(tx, &ax, &mut result)?;
        Ok((result, applied, warnings))
    })?;
    let (result, applied, warnings) = committed.value;
    let commit = committed.commit;
    let report = applied.as_ref().map(|applied| &applied.report);
    let old_backups: &[PathBuf] = applied
        .as_ref()
        .map_or(&[], |applied| &applied.old_value_backups);
    // After the commit: the backups stay until the import is in the store.
    // The import is done either way, so a purge that cannot run is reported
    // per file.
    let purge = match (&args.remove_old_backups, report) {
        (Some(None), Some(report)) if !old_backups.is_empty() => Some(
            backup::purge_old_values(&ax, &report.apply_id).unwrap_or_else(|err| Purge {
                failed: old_backups
                    .iter()
                    .map(|file| (file.clone(), err.clone()))
                    .collect(),
                ..Purge::default()
            }),
        ),
        _ => None,
    };
    let old_backups_json = |apply_id: &str| {
        old_backups
            .iter()
            .map(|file| json!({ "apply_id": apply_id, "file": placeholder_path(ax.env, file, None) }))
            .collect::<Vec<_>>()
    };
    let old_values_in_files: Vec<String> = result
        .old_values_in_files
        .iter()
        .map(|file| placeholder_path(ax.env, file, None))
        .collect();
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "imported": result.imported,
            "definition_only": result.definition_only,
            "skipped": result.skipped.iter().map(|(name, err)| json!({
                "name": name,
                "error": error_json(err),
            })).collect::<Vec<_>>(),
            "aliases": result.aliases.iter().map(|alias| json!({
                "alias": alias.alias,
                "server": alias.server,
                "field": alias.field,
                "destination": alias.destination,
            })).collect::<Vec<_>>(),
            "commit": commit.as_ref().map(ToString::to_string),
            "apply": report.map(|report| report_json(report, commit.as_ref())),
            "backups_with_old_values": report.map_or_else(Vec::new, |report| old_backups_json(&report.apply_id)),
            "removed_backups": purge.as_ref().map(|purge| purge_json(ctx, purge)),
            "old_values_in_files": old_values_in_files,
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
        for id in &result.definition_only {
            println!(
                "{id} was found only in project files: add it to the project's manifest \
                 (`from = \"{id}\"`) to manage it there."
            );
        }
        for alias in &result.aliases {
            println!(
                "secret {} ({} {}): {}",
                alias.alias, alias.server, alias.field, alias.destination
            );
        }
        if let Some(report) = report {
            print_report(report, commit.as_ref());
            match &purge {
                Some(purge) => print_purge(ctx, &report.apply_id, purge),
                None if !old_backups.is_empty() => {
                    let files: Vec<String> = old_backups
                        .iter()
                        .map(|file| placeholder_path(ax.env, file, None))
                        .collect();
                    println!(
                        "The backups this import made of {} keep the previous content, \
                         including the old values of the extracted secrets, so you can undo \
                         the import. Remove them with `{} import --remove-old-backups={}`.",
                        files.join(", "),
                        brand::BIN_NAME,
                        report.apply_id
                    );
                }
                None => {}
            }
        }
        if !old_values_in_files.is_empty() {
            println!(
                "note: the old values of the extracted secrets are still in {} until an apply \
                 rewrites it (review mode, or an entry left to decide). The backups that \
                 apply makes are not marked, so --remove-old-backups cannot remove them: \
                 delete that apply's folder under <LOADOUT_HOME>/backups/files by hand, or \
                 let retention prune it (50 runs or 30 days).",
                old_values_in_files.join(", ")
            );
        }
    }
    let code = report.map_or(0, apply_exit_code);
    Ok(match &purge {
        Some(purge) if purge_failed(purge) && code == 0 => 1,
        _ => code,
    })
}

/// A backup was left as it was, or its zeroed file could not be deleted.
fn purge_failed(purge: &Purge) -> bool {
    !purge.failed.is_empty() || !purge.not_deleted.is_empty()
}

/// `--remove-old-backups=APPLY-ID`: the backups of an earlier import's
/// apply that hold old secret values are removed; nothing is imported.
fn purge_only(ctx: &Context, args: &ImportArgs, apply_id: &str) -> Result<u8, CoreError> {
    if !(args.names.is_empty()
        && args.picks.is_empty()
        && args.renames.is_empty()
        && args.skips.is_empty()
        && args.extracts.is_empty()
        && !args.only)
    {
        return Err(usage(
            "--remove-old-backups=APPLY-ID removes the backups of an earlier import; \
             it imports nothing, so it takes no names or import choices",
        ));
    }
    let purge = backup::purge_old_values(&ctx.apply_env(), apply_id)?;
    if ctx.json {
        let mut doc = purge_json(ctx, &purge);
        doc["schema"] = json!(JSON_SCHEMA);
        doc["apply_id"] = json!(apply_id);
        print_json(&doc);
    } else {
        print_purge(ctx, apply_id, &purge);
    }
    Ok(if purge_failed(&purge) { 1 } else { 0 })
}

/// What a purge removed and what it could not, files as placeholder paths.
fn purge_json(ctx: &Context, purge: &Purge) -> Value {
    json!({
        "removed": purge
            .removed
            .iter()
            .map(|file| placeholder_path(&ctx.env, file, None))
            .collect::<Vec<_>>(),
        "not_deleted": purge.not_deleted.iter().map(|(file, err)| json!({
            "file": placeholder_path(&ctx.env, file, None),
            "error": error_json(err),
        })).collect::<Vec<_>>(),
        "failed": purge.failed.iter().map(|(file, err)| json!({
            "file": placeholder_path(&ctx.env, file, None),
            "error": error_json(err),
        })).collect::<Vec<_>>(),
    })
}

fn print_purge(ctx: &Context, apply_id: &str, purge: &Purge) {
    if purge.removed.is_empty() && purge.failed.is_empty() {
        println!(
            "Apply {apply_id} has no backups marked by an import as holding old secret values."
        );
    }
    for file in &purge.removed {
        println!(
            "Removed the backup of {} that held old secret values (apply {apply_id}).",
            placeholder_path(&ctx.env, file, None)
        );
    }
    for (file, err) in &purge.not_deleted {
        eprintln!(
            "error[{}]: the backup of {} was overwritten with zeros, so the old values are \
             gone, but the zeroed file could not be deleted: {}",
            err.code(),
            placeholder_path(&ctx.env, file, None),
            err.message()
        );
    }
    for (file, err) in &purge.failed {
        eprintln!(
            "error[{}]: cannot remove the backup of {}: {}",
            err.code(),
            placeholder_path(&ctx.env, file, None),
            err.message()
        );
    }
}

fn scope_str(scope: Scope) -> &'static str {
    match scope {
        Scope::User => "user",
        Scope::Project => "project",
        Scope::ProjectLocal => "project-local",
    }
}

fn candidate_json(candidate: &Candidate, previews: &[VariantPreview]) -> Value {
    let variant_of = |index: usize| {
        candidate
            .variants
            .iter()
            .position(|v| v.contains(&index))
            .expect("every source is in a variant")
    };
    json!({
        "name": candidate.name,
        "identical": candidate.identical,
        "conflict": candidate.conflict(),
        "in_store": candidate.in_store,
        "sources": candidate.sources.iter().enumerate().map(|(index, source)| json!({
            "key": source.key(),
            "id": finding_id(source, &candidate.name),
            "client": source.client,
            "scope": scope_str(source.scope),
            "project": source.project.as_ref().map(|p| p.id.clone()),
            "trusted": source.project.as_ref().map(|p| p.trusted),
            "file": path_json(&source.file),
            "variant": variant_of(index),
            "importable": source.def.is_ok(),
            "error": source.def.as_ref().err().map(error_json),
        })).collect::<Vec<_>>(),
        "variants": candidate.variants.iter().zip(previews).enumerate().map(|(v, (indexes, preview))| json!({
            "id": preview.id,
            "sources": indexes.iter().map(|&i| candidate.sources[i].key()).collect::<Vec<_>>(),
            "enables": candidate.enables(v),
            "importable": preview.server.is_ok(),
            "error": preview.server.as_ref().err().map(error_json),
            "secrets": preview.extractions.iter().map(|extraction| json!({
                "field": extraction.field,
                "alias": extraction.alias,
                "reason": extraction.reason,
                "suggested_ref": extraction.suggested_ref,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn print_candidates(candidates: &[Candidate], previews: &[Vec<VariantPreview>]) {
    if candidates.is_empty() {
        println!("No foreign entries.");
    }
    for (candidate, previews) in candidates.iter().zip(previews) {
        let mut notes = Vec::new();
        if candidate.conflict() {
            notes.push("differs between sources");
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
        let several = candidate.variants.len() > 1;
        for (v, (indexes, preview)) in candidate.variants.iter().zip(previews).enumerate() {
            let indent = if several {
                let only = if candidate.enables(v) {
                    ""
                } else {
                    ", definition only: found in no user file Loadout writes"
                };
                println!("  definition {} (as {}{only}):", v + 1, preview.id);
                "    "
            } else {
                "  "
            };
            for &i in indexes {
                let source = &candidate.sources[i];
                let trust = match &source.project {
                    Some(project) if !project.trusted => "  (untrusted project)",
                    _ => "",
                };
                match &source.def {
                    Ok(_) => println!("{indent}{}  {}{trust}", source.key(), source.file.display()),
                    Err(err) => println!(
                        "{indent}{}  {}{trust}  {}[{}]",
                        source.key(),
                        source.file.display(),
                        err.message(),
                        err.code()
                    ),
                }
            }
            if let Err(err) = &preview.server
                && candidate.variant_def(v).is_ok()
            {
                println!("{indent}cannot import: {}[{}]", err.message(), err.code());
            }
            for extraction in &preview.extractions {
                println!(
                    "{indent}secret {}: {} (--extract {}:{}=keyring|op://...|env:VAR)",
                    extraction.field, extraction.reason, candidate.name, extraction.field
                );
            }
        }
    }
}

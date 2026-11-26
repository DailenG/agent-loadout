//! `loadout scanner ls|status|prepare|enable|disable|run|remove` (#30):
//! the optional external scanner, NVIDIA SkillSpector, run locally from
//! the user's own uv or `skillspector`. `loadout skills scan` already
//! names the classification of client skill folders, so the scanners get
//! their own noun; there is one optional scanner, so the subcommands take
//! no scanner name. `remove` cleans up a 0.7.0 Snyk Agent Scan install;
//! `install` and `token` (Snyk's) are hidden and refused.
//!
//! Also the helpers `skills add` and `skills update` use for
//! `--external` / `--no-external`: one bounded run per skill, its report
//! printed beside the built-in scan's.

use agent_loadout_core::scan::Verdict;
use agent_loadout_core::scan::external::skillspector::{
    self, Availability, DISCLOSURE, PYTHON, Runner, Runtime, RuntimeKind, SETUP, VERSION,
};
use agent_loadout_core::scan::external::snyk;
use agent_loadout_core::scan::external::{Choice, ExternalReport, ExternalStatus};
use agent_loadout_core::skills::{SkillFiles, content_hash, store as skill_store};
use agent_loadout_core::{CoreError, ErrorCode};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use super::{Context, JSON_SCHEMA, path_json, print_json, require_store};
use crate::interrupt::Busy;

#[derive(Debug, Args)]
pub struct ScannerArgs {
    #[command(subcommand)]
    command: ScannerCommand,
}

#[derive(Debug, Subcommand)]
enum ScannerCommand {
    /// List the scanners: the built-in scanner (always on) and SkillSpector
    /// (optional, local).
    Ls,
    /// Show SkillSpector on this machine: how it runs (uvx or on PATH),
    /// its version, on or off, the timeout, and what leaves the machine.
    Status,
    /// Download the pinned SkillSpector into uv's cache once (runs `uvx
    /// --python 3.13 --from skillspector==2.13.7 skillspector --version`).
    /// Needs uv on PATH; scans never download anything.
    Prepare,
    /// Run SkillSpector on skill installs and updates on this machine
    /// (locally; see `loadout scanner status`).
    Enable,
    /// Stop running SkillSpector on skill installs and updates.
    Disable,
    /// Scan a library skill (its store content) with SkillSpector now;
    /// reports only. Exits 0 when it was scanned, 1 otherwise.
    Run {
        /// Skill id.
        skill: String,
    },
    /// Delete a Snyk Agent Scan install left by Loadout 0.7.0 and its
    /// settings.
    Remove,
    /// Snyk Agent Scan (disabled): refused.
    #[command(hide = true)]
    Install,
    /// Snyk Agent Scan (disabled): refused.
    #[command(hide = true)]
    Token {
        #[arg(conflicts_with = "clear")]
        reference: Option<String>,
        #[arg(long)]
        clear: bool,
    },
}

pub fn run(ctx: &Context, args: &ScannerArgs) -> Result<u8, CoreError> {
    match &args.command {
        ScannerCommand::Ls => list(ctx),
        ScannerCommand::Status => status(ctx),
        ScannerCommand::Prepare => prepare(ctx),
        ScannerCommand::Enable => switch(ctx, true),
        ScannerCommand::Disable => switch(ctx, false),
        ScannerCommand::Run { skill } => run_on_library(ctx, skill),
        ScannerCommand::Remove => remove(ctx),
        ScannerCommand::Install | ScannerCommand::Token { .. } => Err(CoreError::new(
            ErrorCode::ScannerUnavailable,
            "Snyk Agent Scan is disabled; the optional scanner is SkillSpector, which needs \
             neither an install by Loadout nor a token: see `loadout scanner status`",
        )),
    }
}

/// The 0.7.0 Snyk Agent Scan install `remove` deletes. Debug builds use the
/// file `LOADOUT_TEST_AGENT_SCAN` names (its own size and SHA-256 as the
/// pin) for the CLI tests.
fn snyk_tool(ctx: &Context) -> Result<snyk::Tool, CoreError> {
    #[cfg(debug_assertions)]
    if let Some(path) = ctx
        .env
        .var(agent_loadout_core::brand::TEST_AGENT_SCAN_ENV_VAR)
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        let bytes = std::fs::read(&path).map_err(|err| {
            CoreError::new(
                ErrorCode::IoError,
                format!("cannot read the test scanner: {err}"),
            )
        })?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let asset = snyk::Asset::of(&name, &bytes);
        return Ok(snyk::Tool::new(&ctx.home, snyk::VERSION, asset));
    }
    snyk::Tool::pinned(&ctx.home)
}

fn remove(ctx: &Context) -> Result<u8, CoreError> {
    let removed = snyk::remove(ctx.fs(), &ctx.home, &snyk_tool(ctx)?)?;
    if ctx.json {
        print_json(&json!({ "schema": JSON_SCHEMA, "removed": removed }));
    } else if removed {
        println!("Removed Snyk Agent Scan (from Loadout 0.7.0) and its settings.");
    } else {
        println!("Snyk Agent Scan is not installed.");
    }
    Ok(0)
}

fn scanner_status(ctx: &Context) -> Result<skillspector::Status, CoreError> {
    skillspector::status(&ctx.env, ctx.fs(), &ctx.home)
}

/// `scanner status --json` (also `prepare`, `enable` and `disable`).
fn status_json(status: &skillspector::Status) -> Value {
    let availability = &status.availability;
    let runtime = availability.runtime();
    let (kind, program) = match availability {
        Availability::Ready(runtime) => (Some(runtime.kind), Some(path_json(&runtime.program))),
        Availability::NeedsPrepare { uv } => (Some(RuntimeKind::Uvx), Some(path_json(uv))),
        Availability::NotAvailable => (None, None),
    };
    json!({
        "schema": JSON_SCHEMA,
        "name": skillspector::SCANNER.as_str(),
        "tested_version": VERSION,
        "python": PYTHON,
        "availability": availability.as_str(),
        "runtime": kind.map(RuntimeKind::as_str),
        "program": program,
        "version": runtime.map(|runtime| runtime.version.as_str()),
        "version_tested": runtime.is_none_or(Runtime::tested),
        "note": runtime.and_then(Runtime::note),
        "enabled": status.enabled,
        "timeout_secs": status.timeout.as_secs(),
        "sends": DISCLOSURE,
        "setup": matches!(availability, Availability::NotAvailable).then_some(SETUP),
        "snyk_installed": status.snyk_installed,
    })
}

fn on_off(value: bool) -> &'static str {
    if value { "on " } else { "off" }
}

/// How SkillSpector runs here, in a few words.
fn availability_text(availability: &Availability) -> String {
    match availability {
        Availability::Ready(runtime) => match runtime.kind {
            RuntimeKind::Uvx => format!("SkillSpector {} (ready, uvx)", runtime.version),
            RuntimeKind::Path => format!("SkillSpector {} (ready, on PATH)", runtime.version),
        },
        Availability::NeedsPrepare { .. } => {
            format!("SkillSpector {VERSION} (needs `loadout scanner prepare`)")
        }
        Availability::NotAvailable => {
            format!("SkillSpector {VERSION} (not found; see `loadout scanner status`)")
        }
    }
}

fn list(ctx: &Context) -> Result<u8, CoreError> {
    let status = scanner_status(ctx)?;
    let runtime = status.availability.runtime();
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "scanners": [
                { "name": "builtin", "enabled": true, "availability": "ready", "local": true },
                {
                    "name": skillspector::SCANNER.as_str(),
                    "enabled": status.enabled,
                    "availability": status.availability.as_str(),
                    "runtime": runtime.map(|runtime| runtime.kind.as_str()),
                    "version": runtime.map(|runtime| runtime.version.as_str()),
                    "local": true,
                },
            ],
        }));
        return Ok(0);
    }
    println!("builtin        on   Loadout's own rules; runs locally, nothing leaves this machine");
    println!(
        "{:<14} {}  {}; runs locally",
        skillspector::SCANNER.as_str(),
        on_off(status.enabled),
        availability_text(&status.availability)
    );
    Ok(0)
}

fn status(ctx: &Context) -> Result<u8, CoreError> {
    let status = scanner_status(ctx)?;
    if ctx.json {
        print_json(&status_json(&status));
        return Ok(0);
    }
    println!("SkillSpector (optional, runs locally; tested version {VERSION})");
    match &status.availability {
        Availability::Ready(runtime) => {
            match runtime.kind {
                RuntimeKind::Uvx => println!("  Runtime:   uvx ({})", runtime.command_line()),
                RuntimeKind::Path => {
                    println!("  Runtime:   on PATH ({})", runtime.program.display());
                }
            }
            println!("  Version:   {}", runtime.version);
            if let Some(note) = runtime.note() {
                println!("  Note:      {note}");
            }
        }
        Availability::NeedsPrepare { uv } => {
            println!(
                "  Runtime:   uvx ({}); SkillSpector {VERSION} is not downloaded yet",
                uv.display()
            );
        }
        Availability::NotAvailable => {
            println!("  Runtime:   not found (neither uv nor skillspector is on PATH)")
        }
    }
    println!("  Turned on: {}", if status.enabled { "yes" } else { "no" });
    println!("  Timeout:   {} s", status.timeout.as_secs());
    println!();
    println!("{DISCLOSURE}");
    print_next_step(&status.availability);
    if status.snyk_installed {
        println!();
        println!(
            "A Snyk Agent Scan install from Loadout 0.7.0 remains (it never runs): `loadout \
             scanner remove` deletes it."
        );
    }
    Ok(0)
}

/// What to do when SkillSpector is not ready: prepare it, or set it up.
fn print_next_step(availability: &Availability) {
    match availability {
        Availability::Ready(_) => {}
        Availability::NeedsPrepare { .. } => {
            println!();
            println!("SkillSpector is not downloaded yet: run `loadout scanner prepare` (once).");
        }
        Availability::NotAvailable => {
            println!();
            println!("{SETUP}");
        }
    }
}

fn prepare(ctx: &Context) -> Result<u8, CoreError> {
    if skillspector::find_uv(&ctx.env).is_none() {
        // Nothing to download: a SkillSpector on PATH runs as it is.
        if let Availability::Ready(runtime) = skillspector::detect(&ctx.env, &ctx.home) {
            let status = scanner_status(ctx)?;
            if ctx.json {
                let mut doc = status_json(&status);
                doc["prepared"] = json!(false);
                print_json(&doc);
            } else {
                println!(
                    "uv is not on PATH, and SkillSpector {} on PATH ({}) is ready: there is \
                     nothing to prepare.",
                    runtime.version,
                    runtime.program.display()
                );
                if let Some(note) = runtime.note() {
                    println!("note: {note}");
                }
            }
            return Ok(0);
        }
        return Err(CoreError::new(
            ErrorCode::ScannerUnavailable,
            format!(
                "neither uv nor skillspector is on PATH, so there is nothing to prepare. {SETUP}"
            ),
        ));
    }
    eprintln!(
        "Preparing SkillSpector {VERSION}: uv downloads it and its dependencies (a few hundred \
         MB) from PyPI, and a Python {PYTHON} build when uv has none. This happens once; scans \
         never download anything."
    );
    // Ctrl+C kills uv's whole process tree and removes the run folder.
    let busy = Busy::start();
    let prepared = skillspector::prepare(
        &ctx.env,
        &ctx.home,
        &mut |line| eprintln!("  {line}"),
        Some(busy.flag()),
    );
    drop(busy);
    let runtime = prepared?;
    let status = scanner_status(ctx)?;
    if ctx.json {
        let mut doc = status_json(&status);
        doc["prepared"] = json!(true);
        print_json(&doc);
    } else {
        println!("SkillSpector {} is ready (uvx).", runtime.version);
        if !status.enabled {
            println!("It is off: turn it on with `loadout scanner enable`.");
        }
    }
    Ok(0)
}

fn switch(ctx: &Context, on: bool) -> Result<u8, CoreError> {
    skillspector::set_enabled(ctx.fs(), &ctx.home, on)?;
    let status = scanner_status(ctx)?;
    if ctx.json {
        print_json(&status_json(&status));
        return Ok(0);
    }
    if on {
        println!("SkillSpector is on for skill installs and updates on this machine.");
        println!("{DISCLOSURE}");
        if let Some(note) = status.availability.runtime().and_then(Runtime::note) {
            println!("note: {note}");
        }
        print_next_step(&status.availability);
    } else {
        println!("SkillSpector is off; installs and updates use the built-in scan only.");
    }
    Ok(0)
}

fn run_on_library(ctx: &Context, id: &str) -> Result<u8, CoreError> {
    let settings = skillspector::settings(ctx.fs(), &ctx.home)?;
    if !settings.enabled {
        return Err(CoreError::new(
            ErrorCode::ScannerUnavailable,
            "SkillSpector is off on this machine; turn it on with `loadout scanner enable`, or \
             pass --external to an install or update",
        ));
    }
    let store = require_store(ctx, "there is no skill to scan")?;
    let files = skill_store::read_content(&store, id)?;
    let runner = skillspector::prepare_run(&ctx.env, ctx.fs(), &ctx.home);
    let report = report_for(ctx, &runner, &files, id);
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "id": id,
            "external": external_json(&report),
        }));
    } else {
        print_external(&report, "");
    }
    Ok(match report.status {
        ExternalStatus::Scanned => 0,
        _ => 1,
    })
}

/// `--external` / `--no-external` as a [`Choice`].
pub(super) fn choice(on: bool, off: bool) -> Choice {
    match (on, off) {
        (true, _) => Choice::On,
        (_, true) => Choice::Off,
        _ => Choice::Default,
    }
}

/// A runner when this command should run SkillSpector (`choice`, else the
/// switch): `None` when not; `Some(Err(report))` when it should but cannot
/// (not found, not prepared): every skill gets that report.
pub(super) fn wanted(
    ctx: &Context,
    choice: Choice,
) -> Result<Option<Result<Runner, ExternalReport>>, CoreError> {
    let settings = skillspector::settings(ctx.fs(), &ctx.home)?;
    if !skillspector::wanted(&settings, choice) {
        return Ok(None);
    }
    Ok(Some(skillspector::prepare_run(
        &ctx.env,
        ctx.fs(),
        &ctx.home,
    )))
}

/// One run on `files`, announced on stderr. Ctrl+C (or SIGTERM, SIGHUP)
/// kills the scanner's whole process tree, removes the run folder and ends
/// the command (exit 130).
fn scan(ctx: &Context, runner: &Runner, files: &SkillFiles, name: &str) -> ExternalReport {
    if !ctx.json {
        eprintln!("Running SkillSpector on {name}...");
    }
    let busy = Busy::start();
    let report = runner.scan_files(&ctx.env, &ctx.home, files, Some(busy.flag()));
    drop(busy);
    report
}

/// [`scan`] for a prepared run, or its not-scanned report.
pub(super) fn report_for(
    ctx: &Context,
    runner: &Result<Runner, ExternalReport>,
    files: &SkillFiles,
    name: &str,
) -> ExternalReport {
    match runner {
        Ok(runner) => scan(ctx, runner, files, name),
        Err(report) => {
            let mut report = report.clone();
            report.content_hash = Some(content_hash(files));
            report
        }
    }
}

/// An external report for `--json`.
pub(super) fn external_json(report: &ExternalReport) -> Value {
    json!({
        "scanner": report.scanner,
        "version": report.version,
        "status": report.status.as_str(),
        "reason": report.reason.map(|reason| reason.as_str()),
        // A report that wasn't scanned or wasn't understood has no verdict:
        // `null`, never "clean" (scripts and agents read this field).
        "verdict": (report.status == ExternalStatus::Scanned).then(|| report.verdict.as_str()),
        "incomplete": report.incomplete,
        "detail": report.detail,
        "findings": report.findings.iter().map(|finding| json!({
            "scanner": finding.scanner,
            "rule": finding.rule,
            "severity": finding.severity.as_str(),
            "file": finding.file,
            "line": finding.line,
            "message": finding.message,
            "excerpt": finding.excerpt,
        })).collect::<Vec<_>>(),
    })
}

/// `SkillSpector: <summary>`, then one line per finding.
pub(super) fn print_external(report: &ExternalReport, indent: &str) {
    println!("{indent}{}: {}", report.scanner.name(), report.summary());
    if let Some(detail) = &report.detail
        && report.status != ExternalStatus::Scanned
    {
        println!("{indent}  {detail}");
    }
    for finding in &report.findings {
        let place = match (&finding.file, finding.line) {
            (Some(file), Some(line)) => format!("{file}:{line}"),
            (Some(file), None) => file.clone(),
            _ => String::from("-"),
        };
        println!(
            "{indent}  {:<9} {:<26} {place}  {}",
            finding.severity.as_str(),
            finding.rule,
            finding.message
        );
        if let Some(excerpt) = &finding.excerpt {
            println!("{indent}    > {excerpt}");
        }
    }
    if report.status == ExternalStatus::Scanned && report.verdict == Verdict::Dangerous {
        println!(
            "{indent}  (a dangerous verdict needs --allow-dangerous, as for the built-in scan)"
        );
    }
}

//! `loadout scanner ls|status|install|remove|enable|disable|token|run`
//! (#27): the optional external scanner, Snyk Agent Scan. `loadout skills
//! scan` already names the classification of client skill folders, so the
//! scanners get their own noun; there is one optional scanner, so the
//! subcommands take no scanner name.
//!
//! Also the helpers `skills add` and `skills update` use for
//! `--external` / `--no-external`: one bounded run per skill, its report
//! printed beside the built-in scan's.

use std::io::IsTerminal;

use agent_loadout_core::scan::Verdict;
use agent_loadout_core::scan::external::{
    self, Choice, DISCLOSURE, ExternalReport, ExternalStatus, NotScanned, Runner, Tool,
    UreqDownload,
};
use agent_loadout_core::skills::{SkillFiles, store as skill_store};
use agent_loadout_core::store::{Store, StoreRead};
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
    /// List the scanners: the built-in scanner (always on) and Snyk Agent
    /// Scan (optional).
    Ls,
    /// Show Snyk Agent Scan on this machine: version, installed, on or off,
    /// the token reference, and what it sends to Snyk.
    Status,
    /// Download the pinned Snyk Agent Scan build into the data directory
    /// and verify its SHA-256. Installing does not turn it on.
    Install,
    /// Delete the installed scanner and turn it off (the token reference
    /// is kept).
    Remove,
    /// Run Snyk Agent Scan on skill installs and updates. It sends each
    /// skill to Snyk's analysis API: see `loadout scanner status`.
    Enable,
    /// Stop running Snyk Agent Scan; nothing is sent to Snyk.
    Disable,
    /// Show, set or clear the token reference: a secret alias (see
    /// `loadout secrets`) or env:NAME, keyring://SERVICE/ACCOUNT or
    /// op://VAULT/ITEM/FIELD. Never the token itself.
    Token {
        /// The reference to use.
        #[arg(conflicts_with = "clear")]
        reference: Option<String>,
        /// Clear the reference.
        #[arg(long)]
        clear: bool,
    },
    /// Scan a library skill (its store content) with Snyk Agent Scan now;
    /// reports only. Sends the skill to Snyk.
    Run {
        /// Skill id.
        id: String,
    },
}

pub fn run(ctx: &Context, args: &ScannerArgs) -> Result<u8, CoreError> {
    match &args.command {
        ScannerCommand::Ls => list(ctx),
        ScannerCommand::Status => status(ctx),
        ScannerCommand::Install => install(ctx),
        ScannerCommand::Remove => {
            let removed = external::remove(ctx.fs(), &ctx.home, &tool(ctx)?)?;
            if ctx.json {
                print_json(&json!({ "schema": JSON_SCHEMA, "removed": removed }));
            } else if removed {
                println!("Removed Snyk Agent Scan and turned it off.");
            } else {
                println!("Snyk Agent Scan is not installed.");
            }
            Ok(0)
        }
        ScannerCommand::Enable => switch(ctx, true),
        ScannerCommand::Disable => switch(ctx, false),
        ScannerCommand::Token { reference, clear } => token(ctx, reference.as_deref(), *clear),
        ScannerCommand::Run { id } => run_on_library(ctx, id),
    }
}

/// The pinned scanner; `not_supported_on_os` without a build for this
/// platform. Debug builds run the file `LOADOUT_TEST_AGENT_SCAN` names
/// (its own size and SHA-256 as the pin) for the CLI tests.
fn tool(ctx: &Context) -> Result<Tool, CoreError> {
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
        let asset = external::Asset::of(&name, &bytes);
        return Ok(Tool::new(&ctx.home, external::VERSION, asset));
    }
    Tool::pinned(&ctx.home)
}

fn status_json(status: &external::Status) -> Value {
    json!({
        "name": external::SCANNER,
        "version": status.version,
        "supported": status.supported,
        "asset": status.asset.as_ref().map(|asset| json!({
            "name": asset.name,
            "sha256": asset.sha256,
            "size": asset.size,
        })),
        "installed": status.installed,
        "enabled": status.enabled,
        "token": status.token,
        "binary": status.binary.as_deref().map(path_json),
        "timeout_secs": status.timeout.as_secs(),
    })
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn list(ctx: &Context) -> Result<u8, CoreError> {
    let status = scanner_status(ctx)?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "scanners": [
                { "name": "builtin", "enabled": true, "installed": true, "local": true },
                {
                    "name": external::SCANNER,
                    "enabled": status.enabled,
                    "installed": status.installed,
                    "supported": status.supported,
                    "token": status.token.is_some(),
                    "local": false,
                    "version": status.version,
                },
            ],
        }));
        return Ok(0);
    }
    println!(
        "builtin           on   Loadout's own rules; runs locally, nothing leaves this machine"
    );
    let state = if status.enabled { "on " } else { "off" };
    let installed = match (status.supported, status.installed) {
        (false, _) => "no build for this platform",
        (true, true) => "installed",
        (true, false) => "not installed",
    };
    let token = if status.token.is_some() {
        "token set"
    } else {
        "no token"
    };
    println!(
        "{}   {state}  Snyk Agent Scan {} ({installed}, {token}); sends skills to Snyk",
        external::SCANNER,
        status.version
    );
    Ok(0)
}

fn status(ctx: &Context) -> Result<u8, CoreError> {
    let status = scanner_status(ctx)?;
    if ctx.json {
        let mut doc = status_json(&status);
        doc["schema"] = json!(JSON_SCHEMA);
        doc["sends"] = json!(DISCLOSURE);
        print_json(&doc);
        return Ok(0);
    }
    println!("Snyk Agent Scan {} (optional)", status.version);
    match &status.asset {
        Some(asset) => println!("  Build:     {} ({} bytes)", asset.name, asset.size),
        None => println!("  Build:     none for this platform"),
    }
    println!("  Installed: {}", yes_no(status.installed));
    if let Some(binary) = &status.binary {
        println!("  Binary:    {}", binary.display());
    }
    println!("  Turned on: {}", yes_no(status.enabled));
    println!(
        "  Token:     {}",
        status.token.as_deref().unwrap_or("not set")
    );
    println!("  Timeout:   {} s", status.timeout.as_secs());
    println!();
    println!("{DISCLOSURE}");
    Ok(0)
}

/// [`external::status`] of [`tool`].
fn scanner_status(ctx: &Context) -> Result<external::Status, CoreError> {
    external::status_of(ctx.fs(), &ctx.home, tool(ctx).ok())
}

fn install(ctx: &Context) -> Result<u8, CoreError> {
    let tool = tool(ctx)?;
    // Hashed: a binary changed since its install is downloaded again.
    let already = tool.installed() && tool.verify().is_ok();
    if !already {
        let busy = Busy::start();
        let download = UreqDownload::new(&ctx.env);
        let show = !ctx.json && std::io::stderr().is_terminal();
        let mut last = u64::MAX;
        if !ctx.json {
            eprintln!(
                "Downloading {} ({:.1} MB) from GitHub...",
                tool.asset().name,
                tool.asset().size as f64 / 1_000_000.0
            );
        }
        tool.install(
            &download,
            &mut |received, total| {
                if !show || total == 0 {
                    return;
                }
                let percent = received * 100 / total;
                if percent != last {
                    last = percent;
                    eprint!("\r  {percent:>3}%");
                }
            },
            Some(busy.flag()),
        )
        .inspect_err(|_| {
            if show {
                eprintln!();
            }
        })?;
        if show {
            eprintln!();
        }
        drop(busy);
    }
    let status = scanner_status(ctx)?;
    if ctx.json {
        let mut doc = status_json(&status);
        doc["schema"] = json!(JSON_SCHEMA);
        doc["already_installed"] = json!(already);
        print_json(&doc);
    } else {
        if already {
            println!("Snyk Agent Scan {} is already installed.", status.version);
        } else {
            println!(
                "Installed Snyk Agent Scan {} (SHA-256 verified).",
                status.version
            );
        }
        if !status.enabled {
            println!("It is off: turn it on with `loadout scanner enable`.");
        }
    }
    Ok(0)
}

fn switch(ctx: &Context, on: bool) -> Result<u8, CoreError> {
    external::set_enabled(ctx.fs(), &ctx.home, on)?;
    let status = scanner_status(ctx)?;
    if ctx.json {
        let mut doc = status_json(&status);
        doc["schema"] = json!(JSON_SCHEMA);
        print_json(&doc);
        return Ok(0);
    }
    if on {
        println!("Snyk Agent Scan is on for skill installs and updates on this machine.");
        println!("{DISCLOSURE}");
        if !status.installed {
            println!("It is not installed yet: run `loadout scanner install`.");
        }
        if status.token.is_none() {
            println!("No token is set yet: run `loadout scanner token <REF>`.");
        }
    } else {
        println!("Snyk Agent Scan is off; nothing is sent to Snyk.");
    }
    Ok(0)
}

fn token(ctx: &Context, reference: Option<&str>, clear: bool) -> Result<u8, CoreError> {
    if clear {
        external::set_token(ctx.fs(), &ctx.home, None, None)?;
    } else if let Some(reference) = reference {
        // An alias is checked against the secrets catalog.
        let store = ctx.open_store()?;
        let read = store.as_ref().map(|store| store as &dyn StoreRead);
        external::set_token(ctx.fs(), &ctx.home, read, Some(reference))?;
    }
    let settings = external::settings(ctx.fs(), &ctx.home)?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "token": settings.token,
            "kind": settings
                .token
                .as_deref()
                .and_then(|text| external::TokenRef::parse(text).ok())
                .map(|parsed| parsed.kind()),
        }));
    } else {
        match &settings.token {
            Some(reference) => println!("Token reference: {reference}"),
            None => println!(
                "No token reference. Store the token as a secret (for example `loadout secrets \
                 set SNYK_TOKEN --keyring`, value on stdin), then `loadout scanner token \
                 SNYK_TOKEN`."
            ),
        }
    }
    Ok(0)
}

fn run_on_library(ctx: &Context, id: &str) -> Result<u8, CoreError> {
    let settings = external::settings(ctx.fs(), &ctx.home)?;
    if !settings.enabled {
        return Err(off_error());
    }
    let store = require_store(ctx, "there is no skill to scan")?;
    let files = skill_store::read_content(&store, id)?;
    let report = match prepare(ctx, Some(&store)) {
        Ok(runner) => scan(ctx, &runner, &files, id),
        Err(report) => report,
    };
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

fn off_error() -> CoreError {
    CoreError::new(
        ErrorCode::ScannerUnavailable,
        "Snyk Agent Scan is off on this machine; turn it on with `loadout scanner enable` (it \
         sends the skill to Snyk), or pass --external to an install or update",
    )
}

/// `--external` / `--no-external` as a [`Choice`].
pub(super) fn choice(on: bool, off: bool) -> Choice {
    match (on, off) {
        (true, _) => Choice::On,
        (_, true) => Choice::Off,
        _ => Choice::Default,
    }
}

/// A runner when this command should run the scanner (`choice`, else the
/// switch): `None` when not; `Some(Err(report))` when it should but cannot
/// (not installed, no token): every skill gets that report.
pub(super) fn wanted(
    ctx: &Context,
    store: Option<&Store<'_>>,
    choice: Choice,
) -> Result<Option<Result<Runner, ExternalReport>>, CoreError> {
    let settings = external::settings(ctx.fs(), &ctx.home)?;
    if !external::wanted(&settings, choice) {
        return Ok(None);
    }
    Ok(Some(prepare(ctx, store)))
}

// The `Err` is the not-scanned report, built once per command.
#[allow(clippy::result_large_err)]
fn prepare(ctx: &Context, store: Option<&Store<'_>>) -> Result<Runner, ExternalReport> {
    let Ok(tool) = tool(ctx) else {
        return Err(ExternalReport::not_scanned(
            external::VERSION,
            NotScanned::UnsupportedPlatform,
            None,
        ));
    };
    external::prepare_run_with(tool, &ctx.env, ctx.fs(), &ctx.home, store, &ctx.delivery)
}

/// One run on `files`, announced on stderr (it sends the skill to Snyk).
/// Ctrl+C stops the scanner's whole process tree, removes the run folder
/// and ends the command (exit 130).
pub(super) fn scan(
    ctx: &Context,
    runner: &Runner,
    files: &SkillFiles,
    name: &str,
) -> ExternalReport {
    if !ctx.json {
        eprintln!("Running Snyk Agent Scan on {name} (sends the skill to Snyk)...");
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
            report.content_hash = Some(agent_loadout_core::skills::content_hash(files));
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

/// `Snyk Agent Scan: <summary>`, then one line per finding.
pub(super) fn print_external(report: &ExternalReport, indent: &str) {
    println!("{indent}Snyk Agent Scan: {}", report.summary());
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
    if report.verdict == Verdict::Dangerous {
        println!(
            "{indent}  (a dangerous verdict needs --allow-dangerous, as for the built-in scan)"
        );
    }
}

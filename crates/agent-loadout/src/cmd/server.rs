//! `loadout server ls|show|add|rm|update|checkout` (SPEC 14). Every change
//! is one store commit; `rm` with several ids is one commit too. In instant
//! mode (the default, SPEC 8.7) the change is applied in that same commit.
//!
//! `add --from` drafts servers from a git repository or a local folder
//! (#5): nothing runs while drafting; saving goes through the source trust
//! prompt (or `--trust-source`) and the built-in scan (`--allow-dangerous`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use agent_loadout_core::apply::Touched;
use agent_loadout_core::catalog;
use agent_loadout_core::mcp_source::{self, Draft, Pick, ResolveRequest, Resolved};
use agent_loadout_core::model::clients::ClientSelection;
use agent_loadout_core::model::ids::{self, IdKind};
use agent_loadout_core::model::server::{Enable, Meta, Server, ServerSource, Transport};
use agent_loadout_core::remote::git::{IDLE_TIMEOUT, SystemRunner};
use agent_loadout_core::scan::ScanReport;
use agent_loadout_core::skills::install::{Decisions, FileChange, Source, UpdateStatus};
use agent_loadout_core::skills::{LinkKind, SkillsEnv};
use agent_loadout_core::sources::SourceRef;
use agent_loadout_core::store::{Operation, ServerEntry, Store, StoreRead};
use agent_loadout_core::{CoreError, ErrorCode};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Value, json};

use super::skills::{print_scan, scan_json};
use super::source::{Answer, Answers, TrustSourceArg, cancelled};
use super::{
    Context, EXIT_SCAN_BLOCKED, JSON_SCHEMA, apply_exit_code, error_json, instant_apply,
    print_json, print_report, report_json, report_warnings,
};

static RUNNER: SystemRunner = SystemRunner {
    idle_timeout: IDLE_TIMEOUT,
};

#[derive(Debug, Args)]
pub struct ServerArgs {
    #[command(subcommand)]
    command: ServerCommand,
}

#[derive(Debug, Subcommand)]
enum ServerCommand {
    /// List servers.
    Ls,
    /// Print a server definition.
    Show {
        /// Server id.
        id: String,
    },
    /// Add a server, or (--from) draft servers from a git repository or
    /// a local folder.
    Add(Box<AddArgs>),
    /// Remove servers; several ids make one commit.
    Rm {
        /// Server ids.
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Update servers added with --from (all, or ID) to what their source
    /// gives now; --check only reports.
    Update {
        id: Option<String>,
        /// Report available updates; change nothing.
        #[arg(long)]
        check: bool,
        /// Answer the source trust prompt up front (see `server add`).
        #[arg(
            long,
            value_enum,
            value_name = "ONCE|REPO|OWNER|HOST",
            num_args = 0..=1,
            default_missing_value = "once",
            conflicts_with = "check"
        )]
        trust_source: Option<TrustSourceArg>,
        /// Update even when the built-in scan finds dangerous content.
        #[arg(long, conflicts_with = "check")]
        allow_dangerous: bool,
    },
    /// Re-create or bring up to date this machine's checkout of a server
    /// added with --from that runs from its repository
    /// (`LOADOUT_HOME/servers/<ID>`), at the commit its file names. The
    /// same gates as `add --from`: source trust, then the scan.
    Checkout {
        id: String,
        /// Answer the source trust prompt up front (see `server add`).
        #[arg(
            long,
            value_enum,
            value_name = "ONCE|REPO|OWNER|HOST",
            num_args = 0..=1,
            default_missing_value = "once"
        )]
        trust_source: Option<TrustSourceArg>,
        /// Check out even when the built-in scan finds dangerous content.
        #[arg(long)]
        allow_dangerous: bool,
    },
}

#[derive(Debug, Args)]
struct AddArgs {
    /// Server id: 1 to 64 characters from a-z, 0-9, '.', '_', '-'. With
    /// --from: the id of the one draft saved [default: from its name].
    #[arg(required_unless_present = "from")]
    id: Option<String>,
    /// Endpoint of an http or sse server.
    #[arg(long)]
    url: Option<String>,
    /// Transport [default: stdio, or http with --url].
    #[arg(long, value_enum)]
    transport: Option<TransportArg>,
    /// Environment variable of a stdio server; the value may be a reference
    /// such as ${secret:ALIAS} and is never a secret itself.
    #[arg(long = "env", value_name = "KEY=VALUE", value_parser = parse_pair)]
    env: Vec<(String, String)>,
    /// HTTP header of an http or sse server; the value may hold references.
    #[arg(long = "header", value_name = "NAME=VALUE", value_parser = parse_pair)]
    headers: Vec<(String, String)>,
    /// Working directory of a stdio server.
    #[arg(long)]
    cwd: Option<String>,
    #[arg(long)]
    display_name: Option<String>,
    #[arg(long)]
    description: Option<String>,
    /// Tag (repeatable).
    #[arg(long = "tag", value_name = "TAG")]
    tags: Vec<String>,
    /// Enable at system scope for this client (repeatable).
    #[arg(
        long = "client",
        value_name = "CLIENT",
        conflicts_with = "all_detected"
    )]
    clients: Vec<String>,
    /// Enable at system scope for every detected client.
    #[arg(long)]
    all_detected: bool,
    /// Command and arguments of a stdio server, after `--`.
    #[arg(last = true, value_name = "COMMAND")]
    command: Vec<String>,
    /// Draft servers from a git repository (https, ssh, file://, a bare
    /// repository) or a local folder: server.json, an MCPB manifest.json,
    /// committed client configs, or the repository as a package. Or from
    /// a catalog entry: catalog:<SOURCE>/<ID> (`loadout catalog search`).
    #[arg(
        long,
        value_name = "GIT-URL|PATH|catalog:SOURCE/ID",
        conflicts_with_all = ["url", "transport", "env", "headers", "cwd", "command"]
    )]
    from: Option<String>,
    /// Folder inside the git repository.
    #[arg(long, requires = "from")]
    path: Option<String>,
    /// Branch, tag or commit of the git repository; for a registry
    /// catalog entry, the version (pins it: updates do not follow).
    #[arg(long = "ref", value_name = "REF", requires = "from")]
    git_ref: Option<String>,
    /// The draft to save, by key or id (repeatable) [default: the only
    /// draft].
    #[arg(long = "pick", value_name = "KEY|ID", requires = "from")]
    picks: Vec<String>,
    /// Save every draft.
    #[arg(long, requires = "from", conflicts_with = "picks")]
    all: bool,
    /// List the drafts, their inputs and scan findings; save nothing (the
    /// clone is removed).
    #[arg(long, requires = "from")]
    dry_run: bool,
    /// Answer the source trust prompt up front: once (this save only), or
    /// always for the repo, its owner or its host. Only for a source you
    /// trust: a server drafted from it runs its commands. --yes never
    /// answers the trust prompt.
    #[arg(
        long,
        value_enum,
        value_name = "ONCE|REPO|OWNER|HOST",
        num_args = 0..=1,
        default_missing_value = "once",
        requires = "from"
    )]
    trust_source: Option<TrustSourceArg>,
    /// Save even when the built-in scan finds dangerous content (review
    /// the findings first, with --dry-run).
    #[arg(long, requires = "from")]
    allow_dangerous: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TransportArg {
    Stdio,
    Http,
    Sse,
}

fn parse_pair(text: &str) -> Result<(String, String), String> {
    match text.split_once('=') {
        Some((key, value)) if !key.is_empty() => Ok((key.to_owned(), value.to_owned())),
        _ => Err(format!("expected KEY=VALUE, got {text:?}")),
    }
}

/// `add` and `rm` exit like `loadout apply` when their instant apply left
/// entries blocked (5), unresolved (4) or a file failed (1); the store
/// edit itself is committed either way.
pub fn run(ctx: &Context, args: &ServerArgs) -> Result<u8, CoreError> {
    // Temporary clones a crash or an interrupted prompt left behind.
    mcp_source::sweep_temp_clones(&sx(ctx), std::time::Duration::from_secs(60 * 60));
    match &args.command {
        ServerCommand::Ls => list(ctx).map(|()| 0),
        ServerCommand::Show { id } => show(ctx, id).map(|()| 0),
        ServerCommand::Add(args) if args.from.is_some() => add_from(ctx, args),
        ServerCommand::Add(args) => add(ctx, args),
        ServerCommand::Rm { ids } => remove(ctx, ids),
        ServerCommand::Update {
            id,
            check,
            trust_source,
            allow_dangerous,
        } => update(ctx, id.as_deref(), *check, *trust_source, *allow_dangerous),
        ServerCommand::Checkout {
            id,
            trust_source,
            allow_dangerous,
        } => checkout(ctx, id, *trust_source, *allow_dangerous),
    }
}

fn sx(ctx: &Context) -> SkillsEnv<'_> {
    SkillsEnv {
        env: &ctx.env,
        fs: ctx.fs(),
        home: &ctx.home,
        registry: ctx.apply_env().registry,
        runner: &RUNNER,
        link_kind: LinkKind::Junction,
    }
}

fn list(ctx: &Context) -> Result<(), CoreError> {
    let entries = match ctx.open_store()? {
        Some(store) => store.servers()?,
        None => Vec::new(),
    };
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "servers": entries.iter().map(entry_json).collect::<Vec<_>>(),
        }));
        return Ok(());
    }
    if entries.is_empty() {
        println!("No servers.");
        return Ok(());
    }
    let rows: Vec<[String; 4]> = entries.iter().map(entry_row).collect();
    let header = [
        "ID".to_owned(),
        "TRANSPORT".to_owned(),
        "TARGET".to_owned(),
        "CLIENTS".to_owned(),
    ];
    let widths: Vec<usize> = (0..3)
        .map(|column| {
            rows.iter()
                .chain([&header])
                .map(|row| row[column].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for row in [&header].into_iter().chain(&rows) {
        println!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {}",
            row[0],
            row[1],
            row[2],
            row[3],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
        );
    }
    Ok(())
}

fn entry_row(entry: &ServerEntry) -> [String; 4] {
    match &entry.document {
        Ok(doc) => {
            let server = doc.value();
            let body = &server.body;
            let target = match (&body.url, &body.command) {
                (Some(url), _) => url.clone(),
                (None, Some(command)) => std::iter::once(command)
                    .chain(body.args.iter().flatten())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" "),
                (None, None) => String::new(),
            };
            let clients = match server.enable.as_ref().and_then(|e| e.clients.as_ref()) {
                Some(ClientSelection::AllDetected) => "all-detected".to_owned(),
                Some(ClientSelection::List(ids)) => ids.join(", "),
                None => String::new(),
            };
            [
                entry.id.clone(),
                body.transport.map_or("", Transport::as_str).to_owned(),
                target,
                clients,
            ]
        }
        Err(err) => [
            entry.id.clone(),
            "invalid".to_owned(),
            format!("error[{}]: {}", err.code(), err.message()),
            String::new(),
        ],
    }
}

fn entry_json(entry: &ServerEntry) -> Value {
    match &entry.document {
        Ok(doc) => {
            let server = doc.value();
            let body = &server.body;
            let clients = match server.enable.as_ref().and_then(|e| e.clients.as_ref()) {
                Some(selection) => serde_json::to_value(selection).unwrap_or(Value::Null),
                None => Value::Null,
            };
            json!({
                "id": entry.id,
                "valid": true,
                "display_name": body.display_name,
                "transport": body.transport.map(Transport::as_str),
                "command": body.command,
                "args": body.args,
                "url": body.url,
                "clients": clients,
            })
        }
        Err(err) => json!({
            "id": entry.id,
            "valid": false,
            "error": error_json(err),
        }),
    }
}

fn show(ctx: &Context, id: &str) -> Result<(), CoreError> {
    ids::check(IdKind::Server, id)?;
    let store = ctx.open_store()?.ok_or_else(|| not_found(id))?;
    if ctx.json {
        let doc = store.server(id)?.ok_or_else(|| not_found(id))?;
        let server = serde_json::to_value(doc.value()).map_err(|err| {
            CoreError::new(
                ErrorCode::InvalidValue,
                format!("cannot render JSON: {err}"),
            )
        })?;
        print_json(&json!({ "schema": JSON_SCHEMA, "server": server }));
    } else {
        let relative = agent_loadout_core::store::server_path(id)?;
        let text = store
            .read_to_string(&relative)?
            .ok_or_else(|| not_found(id))?;
        print!("{text}");
    }
    Ok(())
}

fn add(ctx: &Context, args: &AddArgs) -> Result<u8, CoreError> {
    let id = args.id.clone().unwrap_or_default();
    let server = build_server(&id, args);
    // Checked before the store is opened, so a bad definition creates nothing.
    let diagnostics = server.validate(Some(&id));
    if let Some(err) = diagnostics.first_error(None) {
        return Err(err);
    }
    report_warnings(diagnostics.warnings());

    let store = ctx.open_store_for_change()?;
    let operation =
        Operation::new("server.add", format!("server: add {id}")).entity(format!("server/{id}"));
    let ax = ctx.apply_env();
    let committed = store.transaction(operation, |tx| {
        tx.add_server(server)?;
        instant_apply(tx, &ax, &Touched::Servers(vec![id.clone()]))
    })?;
    let report = committed.value;
    let commit = committed.commit.as_ref().map(|commit| commit.to_string());
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "server": id,
            "commit": commit,
            "apply": report.as_ref().map(|r| report_json(r, committed.commit.as_ref())),
        }));
    } else {
        println!(
            "Added server {id} (commit {}).",
            committed.commit.as_ref().map_or("none", |c| c.short())
        );
        if let Some(report) = &report {
            print_report(report, committed.commit.as_ref());
        }
    }
    Ok(report.as_ref().map_or(0, apply_exit_code))
}

fn build_server(id: &str, args: &AddArgs) -> Server {
    let transport = match (args.transport, &args.url) {
        (Some(TransportArg::Stdio), _) | (None, None) => Transport::Stdio,
        (Some(TransportArg::Http), _) | (None, Some(_)) => Transport::Http,
        (Some(TransportArg::Sse), _) => Transport::Sse,
    };
    let map = |pairs: &[(String, String)]| {
        (!pairs.is_empty()).then(|| pairs.iter().cloned().collect::<BTreeMap<_, _>>())
    };
    let mut server = Server::new(id.to_owned());
    let body = &mut server.body;
    body.transport = Some(transport);
    body.command = args.command.first().cloned();
    body.args = (args.command.len() > 1).then(|| args.command[1..].to_vec());
    body.url = args.url.clone();
    body.cwd = args.cwd.clone();
    body.env = map(&args.env);
    body.headers = map(&args.headers);
    body.display_name = args.display_name.clone();
    body.description = args.description.clone();
    body.tags = (!args.tags.is_empty()).then(|| args.tags.clone());
    body.meta = Some(Meta {
        origin: Some("manual".to_owned()),
        ..Meta::default()
    });
    server.enable = clients(args).map(|clients| Enable {
        clients: Some(clients),
        ..Enable::default()
    });
    server
}

/// The `--client`/`--all-detected` choice.
fn clients(args: &AddArgs) -> Option<ClientSelection> {
    if args.all_detected {
        Some(ClientSelection::AllDetected)
    } else {
        (!args.clients.is_empty()).then(|| ClientSelection::List(args.clients.clone()))
    }
}

fn remove(ctx: &Context, ids: &[String]) -> Result<u8, CoreError> {
    let mut unique: Vec<String> = Vec::new();
    for id in ids {
        ids::check(IdKind::Server, id)?;
        if !unique.contains(id) {
            unique.push(id.clone());
        }
    }
    let store = ctx.open_store()?.ok_or_else(|| not_found(&unique[0]))?;
    let summary = match unique.as_slice() {
        [one] => format!("server: remove {one}"),
        many => format!(
            "server: remove {} servers ({})",
            many.len(),
            many.join(", ")
        ),
    };
    let operation = unique
        .iter()
        .fold(Operation::new("server.remove", summary), |operation, id| {
            operation.entity(format!("server/{id}"))
        });
    let ax = ctx.apply_env();
    let sx = sx(ctx);
    // Machine-local checkouts of servers added with --from go with them:
    // moved aside under the store lock, deleted once the removal commits.
    let mut removal = None;
    let result = store.transaction(operation, |tx| {
        unique.iter().try_for_each(|id| tx.remove_server(id))?;
        removal = Some(mcp_source::CheckoutRemoval::begin(&sx, &unique)?);
        instant_apply(tx, &ax, &Touched::Servers(unique.clone()))
    });
    if let Some(removal) = removal {
        removal.finish(sx.fs, result.is_ok());
    }
    let committed = result?;
    let report = committed.value;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "servers": unique,
            "commit": committed.commit.as_ref().map(|commit| commit.to_string()),
            "apply": report.as_ref().map(|r| report_json(r, committed.commit.as_ref())),
        }));
    } else {
        println!(
            "Removed server {} (commit {}).",
            unique.join(", "),
            committed.commit.as_ref().map_or("none", |c| c.short())
        );
        if let Some(report) = &report {
            print_report(report, committed.commit.as_ref());
        }
    }
    Ok(report.as_ref().map_or(0, apply_exit_code))
}

fn not_found(id: &str) -> CoreError {
    CoreError::new(ErrorCode::NotFound, format!("server {id:?} does not exist"))
}

/// The trust answer for `source`, or the error a cancelled prompt ends
/// with.
fn trust_answer(
    ctx: &Context,
    answers: &mut Answers,
    store: &Store<'_>,
    source: &SourceRef,
    flag: Option<TrustSourceArg>,
) -> Result<Answer, CoreError> {
    let answer = answers.decide(ctx, Some(store), source, flag)?;
    if answer == Answer::Cancelled {
        return Err(cancelled(source));
    }
    Ok(answer)
}

fn decisions(answer: Answer, moved: Option<Answer>, allow_dangerous: bool) -> Decisions {
    let (trust, via) = answer.grant();
    Decisions {
        allow_dangerous,
        trust,
        moved_trust: moved.and_then(|moved| moved.grant().0),
        via,
    }
}

fn source_json(source: &ServerSource) -> Value {
    json!({
        "kind": source.kind.as_str(),
        "url": source.url,
        "folder": source.folder,
        "path": source.path,
        "ref": source.git_ref,
        "commit": source.commit,
        "catalog": source.catalog,
        "version": source.version,
    })
}

fn launch(server: &Server) -> String {
    let body = &server.body;
    match (&body.url, &body.command) {
        (Some(url), _) => url.clone(),
        (None, Some(command)) => std::iter::once(command)
            .chain(body.args.iter().flatten())
            .cloned()
            .collect::<Vec<_>>()
            .join(" "),
        (None, None) => String::new(),
    }
}

fn lines_json(server: &Server) -> Value {
    json!(
        mcp_source::launch_lines(server)
            .iter()
            .map(|line| json!({"os": line.os, "line": line.line}))
            .collect::<Vec<_>>()
    )
}

fn draft_json(draft: &Draft) -> Value {
    let source = draft.provenance();
    json!({
        "key": draft.key,
        "kind": draft.kind.as_str(),
        "id": draft.server.id,
        "file": source.file,
        "converter": source.converter,
        "entry": source.entry,
        "launch": launch(&draft.server),
        "launch_lines": lines_json(&draft.server),
        "runs_repo_code": draft.runs_repo_code,
        "checkout": draft.checkout,
        "pinned": draft.pinned,
        "setup": draft.setup,
        "also_in": draft.also_in,
        "inputs": draft.inputs.iter().map(|input| json!({
            "field": input.field,
            "description": input.description,
            "secret": input.secret,
            "required": input.required,
            "alias": input.alias,
            "value_dropped": input.value_dropped,
        })).collect::<Vec<_>>(),
        "scan": scan_json(&draft.scan),
    })
}

/// A setup step as a person types it: the store's `${home}` and
/// `${loadout_home}` written out (they are not shell variables).
fn setup_line(ctx: &Context, step: &str) -> String {
    mcp_source::setup_text(
        step,
        &ctx.env.home().to_string_lossy(),
        &ctx.home.path.to_string_lossy(),
    )
}

fn print_drafts(ctx: &Context, resolved: &Resolved) {
    let source = resolved.source();
    let place = source
        .url
        .as_deref()
        .or(source.folder.as_deref())
        .unwrap_or_default();
    match (&source.commit, &source.version) {
        (Some(commit), _) => println!("Source: {place} at {}", &commit[..commit.len().min(12)]),
        (None, Some(version)) => println!("Source: {place}, version {version}"),
        (None, None) => println!("Source: {place}"),
    }
    if let Some(entry) = &source.catalog {
        println!(
            "Catalog entry: {}{}",
            catalog::REF_PREFIX,
            super::catalog::plain(entry, 200)
        );
    }
    if !resolved.skipped_folders().is_empty() {
        println!(
            "Not read or scanned (dependencies and builds): {}",
            resolved.skipped_folders().join(", ")
        );
    }
    for draft in resolved.drafts() {
        println!();
        println!(
            "{}  ({}, {})",
            draft.key,
            draft.server.id,
            draft.kind.as_str()
        );
        if let Some(own) = &draft.trust_source {
            println!(
                "  Runs from {} (asked about before it is saved).",
                own.key()
            );
        }
        for line in mcp_source::launch_lines(&draft.server) {
            match &line.os {
                None => println!("  {}", line.line),
                Some(os) => println!("  on {os}: {}", line.line),
            }
        }
        if !draft.pinned {
            println!(
                "  Not pinned: the package has no version, so each start can run different code."
            );
        }
        if draft.runs_repo_code {
            println!("  Runs code from this source every time a client starts it.");
        }
        if draft.checkout {
            println!(
                "  Runs from a clone kept in {}/{}.",
                mcp_source::CHECKOUTS_DIR,
                draft.server.id
            );
        }
        for step in &draft.setup {
            println!("  Before it works, run: {}", setup_line(ctx, step));
        }
        for other in &draft.also_in {
            println!("  Also in {other}.");
        }
        for input in &draft.inputs {
            match (&input.alias, input.secret) {
                (Some(alias), true) if input.value_dropped => println!(
                    "  {}: the file held a value; it was not kept. Set secret {alias}.",
                    input.field
                ),
                (Some(alias), true) => println!("  {}: set secret {alias}.", input.field),
                (None, true) => println!(
                    "  {}: the file held a value that looks secret; it was left out.",
                    input.field
                ),
                _ => println!(
                    "  {}: read from the environment{}.",
                    input.field,
                    input
                        .description
                        .as_deref()
                        .map(|d| format!(" ({d})"))
                        .unwrap_or_default()
                ),
            }
        }
        print_scan(&draft.scan, "  ");
    }
    for skipped in resolved.skipped() {
        let entry = skipped
            .entry
            .as_deref()
            .map(|entry| format!(" {entry}"))
            .unwrap_or_default();
        eprintln!(
            "skipped {}{entry}: [{}] {}",
            skipped.file,
            skipped.error.code(),
            skipped.error.message()
        );
    }
}

fn resolved_json(resolved: &Resolved) -> Value {
    json!({
        "source": source_json(resolved.source()),
        "skipped_folders": resolved.skipped_folders(),
        "drafts": resolved.drafts().iter().map(draft_json).collect::<Vec<_>>(),
        "skipped": resolved.skipped().iter().map(|skipped| json!({
            "file": skipped.file,
            "entry": skipped.entry,
            "error": error_json(&skipped.error),
        })).collect::<Vec<_>>(),
    })
}

/// The drafts `args` picks: `--pick` by key or id, `--all`, or the only
/// draft.
fn chosen<'a>(resolved: &'a Resolved, args: &AddArgs) -> Result<Vec<&'a Draft>, CoreError> {
    let drafts = resolved.drafts();
    if drafts.is_empty() {
        return Err(CoreError::new(
            ErrorCode::NotFound,
            "the source holds no MCP server Loadout can draft",
        ));
    }
    if args.all {
        return Ok(drafts.iter().collect());
    }
    if args.picks.is_empty() {
        return match drafts {
            [one] => Ok(vec![one]),
            many => Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!(
                    "the source holds {} drafts; choose with --pick <KEY|ID> or --all: {}",
                    many.len(),
                    many.iter()
                        .map(|draft| draft.key.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
        };
    }
    args.picks
        .iter()
        .map(|wanted| {
            drafts
                .iter()
                .find(|draft| draft.key == *wanted || draft.server.id == *wanted)
                .ok_or_else(|| CoreError::new(ErrorCode::NotFound, format!("no draft {wanted:?}")))
        })
        .collect()
}

/// What `--from` drafts from: a git repository or folder, or a catalog
/// entry (`catalog:<source>/<id>`, #26).
enum Drafting {
    Plain(ResolveRequest),
    Catalog(Box<catalog::McpHandoff>),
}

impl Drafting {
    /// The source asked about before anything is fetched; `None` for a
    /// registry entry (each draft is asked about what it runs).
    fn asked(&self) -> Result<Option<SourceRef>, CoreError> {
        match self {
            Drafting::Plain(request) => match &request.source {
                Source::Git { url, .. } => SourceRef::from_git_url(url).map(Some),
                Source::Local(folder) => SourceRef::from_local_path(folder).map(Some),
            },
            Drafting::Catalog(handoff) => handoff.trust_source(),
        }
    }

    fn resolve(
        &self,
        store: Option<&Store<'_>>,
        sx: &SkillsEnv<'_>,
    ) -> Result<Resolved, CoreError> {
        match self {
            Drafting::Plain(request) => mcp_source::resolve(store, sx, request),
            Drafting::Catalog(handoff) => handoff.resolve(store, sx),
        }
    }
}

/// The catalog entry `text` names, read for the review (needs online
/// catalogs); its notes (deprecated, a look-alike name) go to stderr.
fn catalog_drafting(
    ctx: &Context,
    sx: &SkillsEnv<'_>,
    text: &str,
    args: &AddArgs,
) -> Result<Drafting, CoreError> {
    if args.path.is_some() {
        return Err(super::catalog::not_here("--path"));
    }
    let store = ctx.open_store()?;
    let http = super::catalog::http(ctx);
    let cx = catalog::CatalogEnv { sx, http: &http };
    let read = store.as_ref().map(|s| s as &dyn StoreRead);
    let handoff = catalog::mcp_handoff(&cx, read, text, args.git_ref.as_deref())?;
    let entry = &handoff.entry;
    if !ctx.json {
        let plain = |text: &str| super::catalog::plain(text, 300);
        eprintln!(
            "note: {} from the catalog {}; its text is the publisher's, not Loadout's.",
            plain(&entry.reference()),
            plain(&entry.catalog)
        );
        if entry.deprecated {
            eprintln!(
                "warning: this entry is deprecated{}",
                entry
                    .status_message
                    .as_deref()
                    .map(|m| format!(": {}", plain(m)))
                    .unwrap_or_default()
            );
        }
        if let Some(warning) = &entry.warning {
            eprintln!("warning: {}", plain(warning));
        }
    }
    Ok(Drafting::Catalog(Box::new(handoff)))
}

/// `loadout server add --from`: drafts, the trust prompt, the save.
fn add_from(ctx: &Context, args: &AddArgs) -> Result<u8, CoreError> {
    let text = args.from.as_deref().unwrap_or_default();
    let sx = sx(ctx);
    let drafting = if text.trim_start().starts_with(catalog::REF_PREFIX) {
        catalog_drafting(ctx, &sx, text.trim(), args)?
    } else {
        let mut source = Source::parse(text, sx.fs);
        if let Source::Local(local) = &source {
            let absolute = std::path::absolute(local).map_err(|err| {
                CoreError::new(
                    ErrorCode::InvalidPath,
                    format!("cannot resolve path: {err}"),
                )
                .with_path(local)
            })?;
            source = Source::parse(&absolute.to_string_lossy(), sx.fs);
        }
        let mut request = ResolveRequest::new(source);
        request.path.clone_from(&args.path);
        request.git_ref.clone_from(&args.git_ref);
        Drafting::Plain(request)
    };
    let asked = drafting.asked()?;

    if args.dry_run {
        let store = ctx.open_store()?;
        let resolved = drafting.resolve(store.as_ref(), &sx)?;
        if ctx.json {
            let mut doc = resolved_json(&resolved);
            doc["schema"] = json!(JSON_SCHEMA);
            doc["saved"] = Value::Null;
            print_json(&doc);
        } else {
            print_drafts(ctx, &resolved);
        }
        return Ok(0);
    }

    let store = ctx.open_store_for_change()?;
    // Asked before anything is fetched, like `skills add`.
    let mut answers = Answers::default();
    let answer = match &asked {
        Some(asked) => Some(trust_answer(
            ctx,
            &mut answers,
            &store,
            asked,
            args.trust_source,
        )?),
        None => None,
    };
    let mut resolved = drafting.resolve(Some(&store), &sx)?;
    // The host redirected a moved repository: where the content really
    // comes from is asked about too.
    let moved = match resolved.moved_source() {
        Some(real) => {
            if !ctx.json {
                let from = asked.as_ref().map_or("", SourceRef::key);
                eprintln!("note: {from} moved to {}", real.key());
            }
            Some(trust_answer(
                ctx,
                &mut answers,
                &store,
                real,
                args.trust_source,
            )?)
        }
        None => None,
    };
    let drafts = match chosen(&resolved, args) {
        Ok(drafts) => drafts,
        Err(err) => {
            if !ctx.json {
                print_drafts(ctx, &resolved);
            }
            return Err(err);
        }
    };
    if args.id.is_some() && drafts.len() > 1 {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            "an id names one draft; pick one with --pick",
        ));
    }
    let selection = clients(args);
    // A registry entry's drafts run from their own sources (a package, a
    // remote host): each picked one is asked about.
    let mut own_answer = None;
    let mut grants = Vec::with_capacity(drafts.len());
    for draft in &drafts {
        grants.push(match &draft.trust_source {
            Some(own) => {
                let answer = trust_answer(ctx, &mut answers, &store, own, args.trust_source)?;
                own_answer = own_answer.or(Some(answer));
                answer.grant().0
            }
            None => None,
        });
    }
    let picks: Vec<Pick> = drafts
        .iter()
        .zip(grants)
        .map(|(draft, trust)| {
            let mut server = draft.server.clone();
            if let Some(id) = &args.id {
                server.id.clone_from(id);
            }
            let body = &mut server.body;
            if args.display_name.is_some() {
                body.display_name.clone_from(&args.display_name);
            }
            if args.description.is_some() {
                body.description.clone_from(&args.description);
            }
            if !args.tags.is_empty() {
                body.tags = Some(args.tags.clone());
            }
            server.enable = selection.clone().map(|clients| Enable {
                clients: Some(clients),
                ..Enable::default()
            });
            Pick {
                key: draft.key.clone(),
                server,
                trust,
            }
        })
        .collect();
    let answer = answer.or(own_answer).unwrap_or(Answer::NotNeeded);
    let decisions = decisions(answer, moved, args.allow_dangerous);
    let saved = match mcp_source::save(
        &store,
        &sx,
        &ctx.apply_env(),
        &mut resolved,
        &picks,
        &decisions,
    ) {
        Ok(saved) => saved,
        Err(err) if err.code() == ErrorCode::ScanBlocked => {
            blocked(ctx, &err, &resolved, &picks);
            return Ok(EXIT_SCAN_BLOCKED);
        }
        Err(err) => return Err(err),
    };
    if ctx.json {
        let mut doc = resolved_json(&resolved);
        doc["schema"] = json!(JSON_SCHEMA);
        doc["saved"] = json!(saved.ids);
        doc["commit"] = json!(saved.commit.as_ref().map(ToString::to_string));
        doc["apply"] = json!(
            saved
                .apply
                .as_ref()
                .map(|report| report_json(report, saved.commit.as_ref()))
        );
        print_json(&doc);
    } else {
        print_drafts(ctx, &resolved);
        println!();
        println!(
            "Added server {} (commit {}).",
            saved.ids.join(", "),
            saved.commit.as_ref().map_or("none", |c| c.short())
        );
        if let Some(report) = &saved.apply {
            print_report(report, saved.commit.as_ref());
        }
    }
    Ok(saved.apply.as_ref().map_or(0, apply_exit_code))
}

/// A save the scan blocked: the findings, then the error.
fn blocked(ctx: &Context, err: &CoreError, resolved: &Resolved, picks: &[Pick]) {
    let scans: Vec<&ScanReport> = resolved
        .drafts()
        .iter()
        .filter(|draft| picks.iter().any(|pick| pick.key == draft.key))
        .map(|draft| &draft.scan)
        .collect();
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "error": error_json(err),
            "scans": scans.iter().map(|scan| scan_json(scan)).collect::<Vec<_>>(),
        }));
    } else {
        for scan in scans {
            print_scan(scan, "");
        }
        eprintln!("error[{}]: {err}", err.code());
        eprintln!(
            "Nothing was saved. Review the findings; to save anyway, run again with --allow-dangerous."
        );
    }
}

fn file_json(change: &FileChange) -> Value {
    json!({"path": change.path, "kind": change.kind.as_str(), "script": change.script})
}

/// Ids of the servers added with --from.
fn source_servers(store: &Store<'_>) -> Result<Vec<String>, CoreError> {
    Ok(store
        .servers()?
        .into_iter()
        .filter_map(|ServerEntry { id, document }| {
            let has_source = document.ok()?.value().body.meta.as_ref()?.source.is_some();
            has_source.then_some(id)
        })
        .collect())
}

/// `loadout server update [ID] [--check]`.
fn update(
    ctx: &Context,
    id: Option<&str>,
    check_only: bool,
    trust_source: Option<TrustSourceArg>,
    allow_dangerous: bool,
) -> Result<u8, CoreError> {
    let store = super::require_store(ctx, "server")?;
    let ids = match id {
        Some(id) => {
            ids::check(IdKind::Server, id)?;
            vec![id.to_owned()]
        }
        None => source_servers(&store)?,
    };
    let sx = sx(ctx);
    let ax = ctx.apply_env();
    let mut answers = Answers::default();
    let mut code = 0u8;
    let mut rows = Vec::new();
    for id in &ids {
        // One server's failure is its row; the others are still checked.
        let check = match mcp_source::check_update(&store, &sx, id) {
            Ok(check) => check,
            Err(err) => {
                if !ctx.json {
                    println!("{id}: failed\n  [{}] {}", err.code(), err.message());
                }
                rows.push(json!({ "id": id, "status": "failed", "error": error_json(&err) }));
                code = 1;
                continue;
            }
        };
        let mut row = json!({
            "id": id,
            "status": check.status.as_str(),
            "current": check.current,
            "latest": check.latest,
            "version": check.version,
            "changes": check.changes.iter().map(|c| json!({"field": c.field, "old": c.old, "new": c.new})).collect::<Vec<_>>(),
            "files": check.files.iter().map(file_json).collect::<Vec<_>>(),
            "launch_lines": check.server.as_ref().map(lines_json),
            "runs_repo_code": check.runs_repo_code,
            "setup": check.setup,
            "setup_again": check.setup_again,
            "checkout_out_of_date": check.checkout_out_of_date,
            "scan": check.scan.as_ref().map(scan_json),
            "error": check.error.as_ref().map(error_json),
            "commit": Value::Null,
        });
        if !ctx.json {
            print_check(ctx, &check);
        }
        if matches!(
            check.status,
            UpdateStatus::Unreachable | UpdateStatus::Blocked
        ) {
            code = 1;
        }
        if check_only || check.status != UpdateStatus::Available {
            rows.push(row);
            continue;
        }
        let answer = match &check.trust {
            Some(trust) => trust_answer(ctx, &mut answers, &store, &trust.source, trust_source)?,
            None => Answer::NotNeeded,
        };
        let moved = match &check.moved_trust {
            Some(trust) => Some(trust_answer(
                ctx,
                &mut answers,
                &store,
                &trust.source,
                trust_source,
            )?),
            None => None,
        };
        match mcp_source::update(
            &store,
            &sx,
            &ax,
            id,
            check.latest.as_deref(),
            &decisions(answer, moved, allow_dangerous),
        ) {
            Ok(updated) => {
                row["status"] = json!("updated");
                row["commit"] = json!(updated.commit.as_ref().map(ToString::to_string));
                if let Some(report) = &updated.apply {
                    code = code.max(apply_exit_code(report));
                    if !ctx.json {
                        print_report(report, updated.commit.as_ref());
                    }
                }
                if !ctx.json {
                    println!("Updated server {id}.");
                }
            }
            Err(err) if err.code() == ErrorCode::ScanBlocked => {
                row["status"] = json!("blocked");
                row["error"] = error_json(&err);
                if !ctx.json {
                    eprintln!("error[{}]: {err}", err.code());
                }
                if code == 0 {
                    code = EXIT_SCAN_BLOCKED;
                }
            }
            Err(err) => {
                row["status"] = json!("failed");
                row["error"] = error_json(&err);
                if !ctx.json {
                    eprintln!("error[{}]: {err}", err.code());
                }
                code = 1;
            }
        }
        rows.push(row);
    }
    if ctx.json {
        print_json(&json!({ "schema": JSON_SCHEMA, "servers": rows }));
    } else if ids.is_empty() {
        println!("No server was added from a git repository or folder.");
    }
    Ok(code)
}

fn print_check(ctx: &Context, check: &mcp_source::UpdateCheck) {
    let status = check.status.as_str();
    println!("{}: {status}", check.id);
    for change in &check.changes {
        println!(
            "  {}: {} -> {}",
            change.field,
            change.old.as_deref().unwrap_or("(none)"),
            change.new.as_deref().unwrap_or("(none)")
        );
    }
    for file in &check.files {
        let script = if file.script { "  (runs code)" } else { "" };
        println!("  {} {}{script}", file.kind.as_str(), file.path);
    }
    if check.setup_again {
        println!("  The update replaces the checkout: run the setup steps again afterwards:");
        for step in &check.setup {
            println!("    {}", setup_line(ctx, step));
        }
    }
    if check.checkout_out_of_date {
        println!(
            "  This machine's checkout is missing or out of date; run `loadout server checkout {}`.",
            check.id
        );
    }
    if let Some(scan) = &check.scan {
        print_scan(scan, "  ");
    }
    if let Some(err) = &check.error {
        println!("  [{}] {}", err.code(), err.message());
    }
}

/// `loadout server checkout <ID>`: fetches the repository at the commit
/// the server file names, then the trust prompt (the requested source and
/// a redirect target) and the scan gate, like `add --from`.
fn checkout(
    ctx: &Context,
    id: &str,
    trust_source: Option<TrustSourceArg>,
    allow_dangerous: bool,
) -> Result<u8, CoreError> {
    let store = super::require_store(ctx, "server")?;
    ids::check(IdKind::Server, id)?;
    let sx = sx(ctx);
    let dir: PathBuf = mcp_source::checkout_dir(&sx, id);
    let Some(prepared) = mcp_source::prepare_checkout(&store, &sx, id)? else {
        if ctx.json {
            print_json(
                &json!({ "schema": JSON_SCHEMA, "server": id, "checkout": super::path_json(&dir), "changed": false }),
            );
        } else {
            println!("The checkout of {id} is up to date.");
        }
        return Ok(0);
    };
    if !ctx.json {
        print_scan(prepared.scan(), "");
    }
    let mut answers = Answers::default();
    let answer = match prepared.trust_source() {
        Some(source) => trust_answer(ctx, &mut answers, &store, source, trust_source)?,
        None => Answer::NotNeeded,
    };
    let moved = match prepared.moved_source() {
        Some(source) => Some(trust_answer(
            ctx,
            &mut answers,
            &store,
            source,
            trust_source,
        )?),
        None => None,
    };
    let scan = prepared.scan().clone();
    let report = match mcp_source::restore_checkout(
        &store,
        &sx,
        &ctx.apply_env(),
        prepared,
        &decisions(answer, moved, allow_dangerous),
    ) {
        Ok(report) => report,
        Err(err) if err.code() == ErrorCode::ScanBlocked => {
            if ctx.json {
                print_json(&json!({
                    "schema": JSON_SCHEMA,
                    "error": error_json(&err),
                    "scans": [scan_json(&scan)],
                }));
            } else {
                eprintln!("error[{}]: {err}", err.code());
                eprintln!(
                    "Nothing was checked out. Review the findings; to go ahead anyway, run again with --allow-dangerous."
                );
            }
            return Ok(EXIT_SCAN_BLOCKED);
        }
        Err(err) => return Err(err),
    };
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "server": id,
            "checkout": super::path_json(&dir),
            "changed": true,
            "scan": scan_json(&scan),
            "apply": report.as_ref().map(|r| report_json(r, None)),
        }));
    } else {
        println!("Checkout of {id} is in {}.", dir.display());
        if let Some(report) = &report {
            print_report(report, None);
        }
    }
    Ok(report.as_ref().map_or(0, apply_exit_code))
}

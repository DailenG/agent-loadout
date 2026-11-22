//! `loadout server ls|show|add|rm` (SPEC 14). Every change is one store
//! commit; `rm` with several ids is one commit too. In instant mode (the
//! default, SPEC 8.7) the change is applied in that same commit.

use std::collections::BTreeMap;

use agent_loadout_core::model::clients::ClientSelection;
use agent_loadout_core::model::ids::{self, IdKind};
use agent_loadout_core::model::server::{Enable, Meta, Server, Transport};
use agent_loadout_core::store::{Operation, ServerEntry};
use agent_loadout_core::{CoreError, ErrorCode};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Value, json};

use super::{
    Context, JSON_SCHEMA, apply_exit_code, error_json, instant_apply, print_json, print_report,
    report_json, report_warnings,
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
    /// Add a server.
    Add(Box<AddArgs>),
    /// Remove servers; several ids make one commit.
    Rm {
        /// Server ids.
        #[arg(required = true)]
        ids: Vec<String>,
    },
}

#[derive(Debug, Args)]
struct AddArgs {
    /// Server id: 1 to 64 characters from a-z, 0-9, '.', '_', '-'.
    id: String,
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
    match &args.command {
        ServerCommand::Ls => list(ctx).map(|()| 0),
        ServerCommand::Show { id } => show(ctx, id).map(|()| 0),
        ServerCommand::Add(args) => add(ctx, args),
        ServerCommand::Rm { ids } => remove(ctx, ids),
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
    let server = build_server(args);
    // Checked before the store is opened, so a bad definition creates nothing.
    let diagnostics = server.validate(Some(&args.id));
    if let Some(err) = diagnostics.first_error(None) {
        return Err(err);
    }
    report_warnings(diagnostics.warnings());

    let store = ctx.open_store_for_change()?;
    let id = args.id.clone();
    let operation =
        Operation::new("server.add", format!("server: add {id}")).entity(format!("server/{id}"));
    let ax = ctx.apply_env();
    let committed = store.transaction(operation, |tx| {
        tx.add_server(server)?;
        instant_apply(tx, &ax)
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

fn build_server(args: &AddArgs) -> Server {
    let transport = match (args.transport, &args.url) {
        (Some(TransportArg::Stdio), _) | (None, None) => Transport::Stdio,
        (Some(TransportArg::Http), _) | (None, Some(_)) => Transport::Http,
        (Some(TransportArg::Sse), _) => Transport::Sse,
    };
    let map = |pairs: &[(String, String)]| {
        (!pairs.is_empty()).then(|| pairs.iter().cloned().collect::<BTreeMap<_, _>>())
    };
    let mut server = Server::new(args.id.clone());
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
    let clients = if args.all_detected {
        Some(ClientSelection::AllDetected)
    } else {
        (!args.clients.is_empty()).then(|| ClientSelection::List(args.clients.clone()))
    };
    server.enable = clients.map(|clients| Enable {
        clients: Some(clients),
        ..Enable::default()
    });
    server
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
    let committed = store.transaction(operation, |tx| {
        unique.iter().try_for_each(|id| tx.remove_server(id))?;
        instant_apply(tx, &ax)
    })?;
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

//! `loadout secrets ls|check|providers|set|clear` (SPEC 11, 14).
//!
//! Values never pass through arguments or output: `set --keyring` reads the
//! value from piped stdin, `check` prints statuses only. `providers` only
//! detects (`op --version`); `check` resolves and may make 1Password ask
//! for approval, so it runs on request only (SPEC 11.11).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Read};
use std::sync::Arc;

use agent_loadout_core::brand;
use agent_loadout_core::model::ids::{self, IdKind};
use agent_loadout_core::secrets::SecretValue;
use agent_loadout_core::secrets::catalog::{
    self, AliasStatus, AliasUpdate, Catalog, SetScope, keyring_parts,
};
use agent_loadout_core::secrets::grammar::SecretRef;
use agent_loadout_core::secrets::providers::keyring::{KeyringBackend, OsKeyring};
use agent_loadout_core::secrets::providers::onepassword::OpCli;
use agent_loadout_core::secrets::providers::{self, ProviderInfo, Resolver};
use agent_loadout_core::secrets::scanner::SecretMemory;
use agent_loadout_core::secrets::status::SecretStatus;
use agent_loadout_core::store::{Machine, Operation, Store};
use agent_loadout_core::{CoreError, ErrorCode};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use super::{Context, JSON_SCHEMA, instant_apply, print_json, print_report, report_json};

/// Exit code when a checked alias cannot be resolved (SPEC 14).
const EXIT_MISSING_SECRETS: u8 = 5;

static OS_KEYRING: OsKeyring = OsKeyring;

#[derive(Debug, Args)]
pub struct SecretsArgs {
    #[command(subcommand)]
    command: SecretsCommand,
}

#[derive(Debug, Subcommand)]
enum SecretsCommand {
    /// List aliases with this machine's reference and the servers using them.
    Ls,
    /// Resolve aliases and report their status (never their values); exits
    /// 5 when one cannot be resolved. May prompt for 1Password approval.
    Check {
        /// Aliases to check [default: every alias defined or used].
        aliases: Vec<String>,
    },
    /// Show which secret providers this machine has.
    Providers,
    /// Set an alias's reference.
    Set(SetArgs),
    /// Delete the OS keyring entry of an alias's keyring reference on this
    /// machine; the reference stays.
    Clear {
        /// Secret alias.
        alias: String,
    },
}

#[derive(Debug, Args)]
struct SetArgs {
    /// Secret alias: letters, digits and '_', not starting with a digit.
    alias: String,
    /// Provider reference: env:NAME, op://vault/item/field,
    /// op-env://<environment-id>/<NAME> or keyring://<service>/<account>.
    #[arg(
        long = "ref",
        value_name = "REF",
        required_unless_present = "keyring",
        conflicts_with = "keyring"
    )]
    reference: Option<String>,
    /// Store the value piped on stdin in the OS keyring as
    /// keyring://loadout/<ALIAS> and point the alias there.
    #[arg(long)]
    keyring: bool,
    /// Write the reference to this machine's overlay only (other machines
    /// keep the shared one).
    #[arg(long)]
    this_machine: bool,
    #[arg(long)]
    description: Option<String>,
    /// Environment variable name for env delivery [default: the alias].
    #[arg(long)]
    env_name: Option<String>,
}

pub fn run(ctx: &Context, args: &SecretsArgs) -> Result<u8, CoreError> {
    match &args.command {
        SecretsCommand::Ls => list(ctx).map(|()| 0),
        SecretsCommand::Check { aliases } => check(ctx, aliases),
        SecretsCommand::Providers => providers(ctx).map(|()| 0),
        SecretsCommand::Set(args) => set(ctx, args).map(|()| 0),
        SecretsCommand::Clear { alias } => clear(ctx, alias).map(|()| 0),
    }
}

/// The catalog and alias usage this machine sees; empty without a store.
fn load(
    ctx: &Context,
    store: Option<&Store<'_>>,
) -> Result<(Catalog, BTreeMap<String, BTreeSet<String>>), CoreError> {
    let Some(store) = store else {
        return Ok((Catalog::default(), BTreeMap::new()));
    };
    let machine = Machine::load(ctx.fs(), &ctx.home.state_dir())?.map(|machine| machine.id);
    Ok((
        Catalog::load(store, machine.as_deref())?,
        catalog::usage(store, machine.as_deref())?,
    ))
}

fn memory(store: Option<&Store<'_>>) -> Result<Arc<SecretMemory>, CoreError> {
    match store {
        Some(store) => Ok(store.secret_memory().clone()),
        None => SecretMemory::new().map(Arc::new),
    }
}

fn list(ctx: &Context) -> Result<(), CoreError> {
    let store = ctx.open_store()?;
    let (catalog, usage) = load(ctx, store.as_ref())?;
    let servers = |alias: &str| -> Vec<String> {
        usage
            .get(alias)
            .map(|ids| ids.iter().cloned().collect())
            .unwrap_or_default()
    };
    if ctx.json {
        let aliases: Vec<Value> = catalog
            .iter()
            .map(|alias| {
                json!({
                    "alias": alias.alias,
                    "ref": alias.reference,
                    "source": alias.source.as_str(),
                    "store_ref": alias.store_reference,
                    "description": alias.description,
                    "env_name": alias.env_name(),
                    "servers": servers(&alias.alias),
                })
            })
            .collect();
        print_json(&json!({ "schema": JSON_SCHEMA, "aliases": aliases }));
        return Ok(());
    }
    if catalog.is_empty() {
        println!("No secret aliases.");
        return Ok(());
    }
    for alias in catalog.iter() {
        let machine = match alias.source {
            catalog::RefSource::Machine => " (this machine)",
            catalog::RefSource::Store => "",
        };
        let used = servers(&alias.alias);
        let used = if used.is_empty() {
            String::new()
        } else {
            format!("  used by {}", used.join(", "))
        };
        println!("{}  {}{machine}{used}", alias.alias, alias.reference);
    }
    Ok(())
}

fn check(ctx: &Context, only: &[String]) -> Result<u8, CoreError> {
    let store = ctx.open_store()?;
    let (catalog, usage) = load(ctx, store.as_ref())?;
    let op = OpCli::detect(&ctx.env, ctx.fs());
    let resolver = Resolver::new(&ctx.env, &OS_KEYRING, &op, memory(store.as_ref())?);
    let statuses = catalog::check(&catalog, &usage, &resolver, only);
    let blocked = statuses.iter().any(|status| {
        !matches!(
            status.resolution.status,
            SecretStatus::Resolved | SecretStatus::Unverifiable
        )
    });
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "aliases": statuses.iter().map(status_json).collect::<Vec<_>>(),
        }));
    } else if statuses.is_empty() {
        println!("No secret aliases.");
    } else {
        for status in &statuses {
            let detail = status
                .resolution
                .message
                .as_deref()
                .map(|message| format!("  {message}"))
                .unwrap_or_default();
            println!(
                "{:<22} {}{detail}",
                status.resolution.status.as_str(),
                status.alias
            );
        }
    }
    Ok(if blocked { EXIT_MISSING_SECRETS } else { 0 })
}

fn status_json(status: &AliasStatus) -> Value {
    json!({
        "alias": status.alias,
        "ref": status.reference,
        "source": status.source.map(|source| source.as_str()),
        "status": status.resolution.status.as_str(),
        "message": status.resolution.message,
        "servers": status.servers,
    })
}

fn providers(ctx: &Context) -> Result<(), CoreError> {
    let op = OpCli::detect(&ctx.env, ctx.fs());
    let infos = providers::describe(&OS_KEYRING, &op);
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "providers": infos.iter().map(provider_json).collect::<Vec<_>>(),
        }));
        return Ok(());
    }
    for info in &infos {
        let version = info
            .version
            .as_deref()
            .map(|version| format!(" {version}"))
            .unwrap_or_default();
        println!(
            "{:<12} {:<12} {}{version}",
            info.kind.as_str(),
            info.state.as_str(),
            info.detail
        );
        for (feature, available) in &info.features {
            let mark = if *available { "yes" } else { "no" };
            println!("{:<12}   {:<20} {mark}", "", feature.as_str());
        }
    }
    Ok(())
}

fn provider_json(info: &ProviderInfo) -> Value {
    let features: serde_json::Map<String, Value> = info
        .features
        .iter()
        .map(|(feature, available)| (feature.as_str().to_owned(), Value::Bool(*available)))
        .collect();
    json!({
        "kind": info.kind.as_str(),
        "state": info.state.as_str(),
        "detail": info.detail,
        "version": info.version,
        "features": features,
    })
}

fn set(ctx: &Context, args: &SetArgs) -> Result<(), CoreError> {
    let reference = match (&args.reference, args.keyring) {
        (Some(text), false) => SecretRef::parse(text)?,
        (None, true) => SecretRef::Keyring {
            service: brand::KEYRING_SERVICE.to_owned(),
            account: args.alias.clone(),
        },
        _ => unreachable!("clap requires exactly one of --ref and --keyring"),
    };
    let update = AliasUpdate {
        reference: reference.clone(),
        description: args.description.clone(),
        env_name: args.env_name.clone(),
    };
    // Checked before anything is stored: a bad alias touches nothing.
    ids::check(IdKind::SecretAlias, &args.alias)?;
    let value = if args.keyring {
        Some(read_value()?)
    } else {
        None
    };

    let store = ctx.open_store_for_change()?;
    if let Some(value) = &value {
        let (service, account) = keyring_parts(&reference)?;
        let op = OpCli::NotFound;
        let resolver = Resolver::new(&ctx.env, &OS_KEYRING, &op, store.secret_memory().clone());
        resolver
            .set_keyring(service, account, value)
            .map_err(|resolution| resolution.into_error(&format!("cannot store {}", args.alias)))?;
    }
    let scope = if args.this_machine {
        SetScope::Machine
    } else {
        SetScope::Store
    };
    let alias = &args.alias;
    let summary = match scope {
        SetScope::Store => format!("secrets: set {alias}"),
        SetScope::Machine => format!("secrets: set {alias} on this machine"),
    };
    let ax = ctx.apply_env();
    let committed = store.transaction(Operation::new("secrets.set", summary), |tx| {
        let entity = match scope {
            SetScope::Store => "secrets".to_owned(),
            SetScope::Machine => format!("machine/{}", tx.machine().id),
        };
        tx.add_entity(entity);
        catalog::stage_set(tx, alias, &update, scope)?;
        instant_apply(tx, &ax)
    })?;
    let report = committed.value;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "alias": alias,
            "ref": reference.to_string(),
            "scope": match scope {
                SetScope::Store => "store",
                SetScope::Machine => "machine",
            },
            "keyring": value.is_some(),
            "commit": committed.commit.as_ref().map(ToString::to_string),
            "apply": report.as_ref().map(|r| report_json(r, committed.commit.as_ref())),
        }));
    } else {
        if value.is_some() {
            println!("Stored the value of {alias} in the {}.", OS_KEYRING.name());
        }
        println!(
            "Set {alias} to {reference} (commit {}).",
            committed.commit.as_ref().map_or("none", |c| c.short())
        );
        if let Some(report) = &report {
            print_report(report, committed.commit.as_ref());
        }
    }
    Ok(())
}

/// The value piped on stdin, without its final line break. A terminal is
/// refused, so a value is never echoed on screen.
fn read_value() -> Result<SecretValue, CoreError> {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            "pipe the value on stdin (for example from a password manager); it is never taken \
             from arguments or typed on screen",
        ));
    }
    let mut text = String::new();
    stdin
        .read_to_string(&mut text)
        .map_err(|err| CoreError::new(ErrorCode::IoError, format!("cannot read stdin: {err}")))?;
    let value = text
        .strip_suffix('\n')
        .map(|rest| rest.strip_suffix('\r').unwrap_or(rest))
        .unwrap_or(&text);
    if value.is_empty() {
        return Err(CoreError::new(ErrorCode::InvalidValue, "no value on stdin"));
    }
    Ok(SecretValue::new(value))
}

fn clear(ctx: &Context, alias: &str) -> Result<(), CoreError> {
    let store = ctx.open_store()?;
    let (catalog, _) = load(ctx, store.as_ref())?;
    let entry = catalog.get(alias).ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotFound,
            format!("secret alias {alias:?} is not defined"),
        )
    })?;
    let reference = entry.parsed()?;
    let (service, account) = keyring_parts(&reference)?;
    let op = OpCli::NotFound;
    let resolver = Resolver::new(&ctx.env, &OS_KEYRING, &op, memory(store.as_ref())?);
    let deleted = resolver
        .delete_keyring(service, account)
        .map_err(|resolution| resolution.into_error(&format!("cannot clear {alias}")))?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "alias": alias,
            "ref": reference.to_string(),
            "deleted": deleted,
        }));
    } else if deleted {
        println!("Deleted the keyring entry of {alias} ({reference}).");
    } else {
        println!("{alias} had no keyring entry ({reference}).");
    }
    Ok(())
}

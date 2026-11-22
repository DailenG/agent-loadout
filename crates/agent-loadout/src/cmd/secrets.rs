//! `loadout secrets ls|check|providers|set|clear|map-op-env|op-account|service-account`
//! (SPEC 11, 14).
//!
//! Values never pass through arguments or output: `set --keyring` and
//! `service-account set` read the value from piped stdin, `check` prints
//! statuses only. `providers` only detects (`op --version`); `check`
//! resolves and may make 1Password ask for approval, so it runs on request
//! only (SPEC 11.11). `check` is the missing-secret checklist of SPEC 11.9:
//! each alias with its status, the servers and clients that need it, and
//! the fixes.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Read};
use std::sync::Arc;

use agent_loadout_core::apply::render::DesiredState;
use agent_loadout_core::brand;
use agent_loadout_core::model::ids::{self, IdKind};
use agent_loadout_core::secrets::SecretValue;
use agent_loadout_core::secrets::catalog::{
    self, AliasStatus, AliasUpdate, Catalog, SetScope, keyring_parts,
};
use agent_loadout_core::secrets::delivery::{SERVICE_ACCOUNT_KEYRING_ACCOUNT, op_env};
use agent_loadout_core::secrets::grammar::{SecretRef, is_op_account, is_op_environment_id};
use agent_loadout_core::secrets::providers::onepassword::OpFeature;
use agent_loadout_core::secrets::providers::{self, ProviderInfo, Resolver};
use agent_loadout_core::secrets::scanner::SecretMemory;
use agent_loadout_core::secrets::status::SecretStatus;
use agent_loadout_core::store::{Machine, Operation, Store};
use agent_loadout_core::{CoreError, ErrorCode};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use super::{
    Context, EXIT_MISSING_SECRETS, JSON_SCHEMA, apply_exit_code, instant_apply, print_json,
    print_report, report_json,
};

#[derive(Debug, Args)]
pub struct SecretsArgs {
    #[command(subcommand)]
    command: SecretsCommand,
}

#[derive(Debug, Subcommand)]
enum SecretsCommand {
    /// List aliases with this machine's reference and the servers using them.
    Ls,
    /// The missing-secret checklist: resolve aliases and report their
    /// status (never their values), the servers and clients that need them
    /// and how to fix them; exits 5 when one cannot be resolved. May prompt
    /// for 1Password approval.
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
    /// Map the variables of a 1Password Environment to aliases
    /// (`op-env://<ENV-ID>/<NAME>`) and save the Environment id.
    MapOpEnv(MapOpEnvArgs),
    /// The 1Password account Loadout's `op` calls use, for machines whose
    /// desktop app is signed in to several accounts: show it, set it, or
    /// clear it.
    OpAccount {
        /// Sign-in address, shorthand or account id
        /// (for example example.1password.com).
        #[arg(value_name = "ACCOUNT")]
        account: Option<String>,
        /// Remove the setting.
        #[arg(long, conflicts_with = "account")]
        clear: bool,
    },
    /// The 1Password service account token in the OS keyring, passed by
    /// `loadout exec` to `op` only.
    ServiceAccount {
        #[command(subcommand)]
        command: ServiceAccountCommand,
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

#[derive(Debug, Args)]
struct MapOpEnvArgs {
    /// 1Password Environment id.
    #[arg(value_name = "ENV-ID")]
    environment: String,
    /// Map these variable names (repeatable). Needed below CLI 2.42.0,
    /// which cannot list them.
    #[arg(long = "name", value_name = "NAME")]
    names: Vec<String>,
    /// Map every variable the Environment lists.
    #[arg(long, conflicts_with = "names")]
    all: bool,
    /// The 1Password account holding the Environment (sign-in address,
    /// shorthand or account id); saved for every later `op` call.
    #[arg(long, value_name = "ACCOUNT")]
    account: Option<String>,
}

#[derive(Debug, Subcommand)]
enum ServiceAccountCommand {
    /// Store the token piped on stdin in the OS keyring.
    Set,
    /// Delete the token from the OS keyring.
    Clear,
}

pub fn run(ctx: &Context, args: &SecretsArgs) -> Result<u8, CoreError> {
    match &args.command {
        SecretsCommand::Ls => list(ctx).map(|()| 0),
        SecretsCommand::Check { aliases } => check(ctx, aliases),
        SecretsCommand::Providers => providers(ctx).map(|()| 0),
        SecretsCommand::Set(args) => set(ctx, args),
        SecretsCommand::Clear { alias } => clear(ctx, alias).map(|()| 0),
        SecretsCommand::MapOpEnv(args) => map_op_env(ctx, args),
        SecretsCommand::OpAccount { account, clear } => op_account(ctx, account.as_deref(), *clear),
        SecretsCommand::ServiceAccount { command } => service_account(ctx, command).map(|()| 0),
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
    let machine = machine_id(ctx)?;
    Ok((
        Catalog::load(store, machine.as_deref())?,
        catalog::usage(store, machine.as_deref())?,
    ))
}

fn machine_id(ctx: &Context) -> Result<Option<String>, CoreError> {
    Ok(Machine::load(ctx.fs(), &ctx.home.state_dir())?.map(|machine| machine.id))
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
    let desired = match &store {
        Some(store) => DesiredState::load(store, machine_id(ctx)?.as_deref())?,
        None => DesiredState::empty(),
    };
    let env = op_env(&ctx.env, ctx.keyring(), catalog.op_account.as_deref());
    let resolver = Resolver::new(&env, ctx.keyring(), ctx.op(), memory(store.as_ref())?);
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
            "aliases": statuses
                .iter()
                .map(|status| status_json(status, &desired))
                .collect::<Vec<_>>(),
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
            for (server, clients) in catalog::needed_by(status, &desired) {
                let clients = if clients.is_empty() {
                    "not enabled".to_owned()
                } else {
                    clients.join(", ")
                };
                println!("{:<22}   needed by {server} ({clients})", "");
            }
            for fix in fixes(status) {
                println!("{:<22}   fix: {fix}", "");
            }
        }
    }
    Ok(if blocked { EXIT_MISSING_SECRETS } else { 0 })
}

/// The quick fixes of SPEC 11.9 for an alias that is not resolved; "skip
/// for now" is doing nothing.
fn fixes(status: &AliasStatus) -> Vec<String> {
    let bin = brand::BIN_NAME;
    let alias = &status.alias;
    let set = |machine: &str| {
        vec![
            format!("pipe the value into `{bin} secrets set {alias} --keyring{machine}`"),
            format!("`{bin} secrets set {alias} --ref op://<vault>/<item>/<field>{machine}`"),
            format!(
                "`{bin} secrets set {alias} --ref env:<NAME>{machine}` and set NAME in the clients' environment"
            ),
        ]
    };
    match status.resolution.status {
        SecretStatus::Resolved | SecretStatus::Unverifiable => Vec::new(),
        SecretStatus::ProviderUnavailable | SecretStatus::ProviderTooOld => set(" --this-machine"),
        SecretStatus::Locked => vec![
            "sign in to 1Password or approve its prompt, then check again".to_owned(),
            format!(
                "pipe a 1Password service account token into `{bin} secrets service-account set`"
            ),
        ],
        SecretStatus::Missing | SecretStatus::Error => set(""),
    }
}

fn status_json(status: &AliasStatus, desired: &DesiredState) -> Value {
    let clients: BTreeSet<String> = catalog::needed_by(status, desired)
        .into_iter()
        .flat_map(|(_, clients)| clients)
        .collect();
    json!({
        "alias": status.alias,
        "ref": status.reference,
        "source": status.source.map(|source| source.as_str()),
        "status": status.resolution.status.as_str(),
        "message": status.resolution.message,
        "servers": status.servers,
        "clients": clients,
        "fixes": fixes(status),
    })
}

fn providers(ctx: &Context) -> Result<(), CoreError> {
    let infos = providers::describe(ctx.keyring(), ctx.op());
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

fn set(ctx: &Context, args: &SetArgs) -> Result<u8, CoreError> {
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
        store_in_keyring(ctx, &store, service, account, value).map_err(|err| {
            CoreError::new(err.code(), format!("cannot store {}: {err}", args.alias))
        })?;
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
            println!(
                "Stored the value of {alias} in the {}.",
                ctx.keyring().name()
            );
        }
        println!(
            "Set {alias} to {reference} (commit {}).",
            committed.commit.as_ref().map_or("none", |c| c.short())
        );
        if let Some(report) = &report {
            print_report(report, committed.commit.as_ref());
        }
    }
    Ok(report.as_ref().map_or(0, apply_exit_code))
}

/// Stores `value` in the keyring and remembers it, so the store refuses
/// to commit it.
fn store_in_keyring(
    ctx: &Context,
    store: &Store<'_>,
    service: &str,
    account: &str,
    value: &SecretValue,
) -> Result<(), CoreError> {
    let resolver = Resolver::new(
        &ctx.env,
        ctx.keyring(),
        &providers::onepassword::OpCli::NotFound,
        store.secret_memory().clone(),
    );
    resolver
        .set_keyring(service, account, value)
        .map_err(|resolution| resolution.into_error(account))
}

/// The value piped on stdin, without a leading byte order mark (Windows
/// PowerShell 5.1 pipes one when `$OutputEncoding` is UTF-8 with BOM; gate
/// V2 sandbox run) and without its final line break. A terminal is
/// refused, so a value is never echoed on screen.
pub(crate) fn read_value() -> Result<SecretValue, CoreError> {
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
    let text_ref = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let value = text_ref
        .strip_suffix('\n')
        .map(|rest| rest.strip_suffix('\r').unwrap_or(rest))
        .unwrap_or(text_ref);
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
    let deleted = ctx
        .keyring()
        .delete(service, account)
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

/// `secrets map-op-env <ENV-ID>` (SPEC 11.10): lists the Environment's
/// variable names (CLI 2.42.0+; values are dropped while parsing) and maps
/// the selected ones to aliases named like the variables. Aliases that
/// already point elsewhere are kept and reported.
fn map_op_env(ctx: &Context, args: &MapOpEnvArgs) -> Result<u8, CoreError> {
    let id = &args.environment;
    if !is_op_environment_id(id) {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("invalid 1Password Environment id {id:?}"),
        ));
    }
    for name in &args.names {
        ids::check(IdKind::SecretAlias, name)?;
    }
    if let Some(account) = &args.account
        && !is_op_account(account)
    {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("invalid 1Password account {account:?}"),
        ));
    }
    let account = match &args.account {
        Some(account) => Some(account.clone()),
        None => ctx
            .open_store()?
            .map(|store| Catalog::load(&store, None))
            .transpose()?
            .and_then(|catalog| catalog.op_account),
    };
    let op = ctx.op();
    op.require(OpFeature::Environments)
        .map_err(|resolution| resolution.into_error("1Password Environments"))?;
    let listed = if op.supports(OpFeature::EnvironmentListing) {
        let env = op_env(&ctx.env, ctx.keyring(), account.as_deref());
        Some(
            op.list_environment(&env, id)
                .map_err(|resolution| resolution.into_error(&format!("Environment {id}")))?,
        )
    } else {
        None
    };
    let selected: Vec<String> = match (&listed, args.all) {
        (Some(listed), true) => listed.clone(),
        (None, true) => {
            return Err(CoreError::new(
                ErrorCode::ProviderTooOld,
                "listing Environment variables needs 1Password CLI 2.42.0 or newer; name them \
                 with --name",
            ));
        }
        (Some(listed), false) => {
            if let Some(unknown) = args.names.iter().find(|name| !listed.contains(name)) {
                return Err(CoreError::new(
                    ErrorCode::NotFound,
                    format!("{unknown} is not a variable of 1Password Environment {id}"),
                ));
            }
            args.names.clone()
        }
        (None, false) => args.names.clone(),
    };
    if selected.is_empty() {
        let names = listed.unwrap_or_default();
        if ctx.json {
            print_json(&json!({ "schema": JSON_SCHEMA, "environment": id, "names": names }));
        } else {
            for name in &names {
                println!("{name}");
            }
            eprintln!("Pick names with --name NAME (repeatable) or map them all with --all.");
        }
        return Ok(0);
    }
    for name in &selected {
        ids::check(IdKind::SecretAlias, name)?;
    }

    let store = ctx.open_store_for_change()?;
    let machine = machine_id(ctx)?;
    let catalog = Catalog::load(&store, machine.as_deref())?;
    let mut mapped = Vec::new();
    let mut kept = Vec::new();
    for name in &selected {
        let reference = SecretRef::OnePasswordEnvironment {
            environment: id.clone(),
            name: name.clone(),
        };
        match catalog.get(name).map(|alias| alias.parsed()) {
            Some(Ok(SecretRef::OnePasswordEnvironment { .. })) | None => {
                mapped.push((name.clone(), reference));
            }
            Some(_) => kept.push(name.clone()),
        }
    }
    let ax = ctx.apply_env();
    let summary = format!("secrets: map 1Password Environment {id}");
    let committed = store.transaction(Operation::new("secrets.map-op-env", summary), |tx| {
        tx.add_entity("secrets".to_owned());
        let previous = catalog::stage_environment(tx, id)?;
        if let Some(account) = &args.account {
            catalog::stage_op_account(tx, Some(account))?;
        }
        for (alias, reference) in &mapped {
            let update = AliasUpdate {
                reference: reference.clone(),
                description: None,
                env_name: None,
            };
            catalog::stage_set(tx, alias, &update, SetScope::Store)?;
        }
        Ok((previous, instant_apply(tx, &ax)?))
    })?;
    let (previous, report) = committed.value;
    let aliases: Vec<&str> = mapped.iter().map(|(alias, _)| alias.as_str()).collect();
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "environment": id,
            "replaced_environment": previous,
            "mapped": aliases,
            "kept": kept,
            "commit": committed.commit.as_ref().map(ToString::to_string),
            "apply": report.as_ref().map(|r| report_json(r, committed.commit.as_ref())),
        }));
    } else {
        if let Some(previous) = &previous {
            println!("Replaced 1Password Environment {previous} with {id}.");
        }
        println!(
            "Mapped {} to 1Password Environment {id} (commit {}).",
            aliases.join(", "),
            committed.commit.as_ref().map_or("none", |c| c.short())
        );
        for alias in &kept {
            eprintln!("warning[already_exists]: {alias} already points to another reference; kept");
        }
        if let Some(report) = &report {
            print_report(report, committed.commit.as_ref());
        }
    }
    Ok(report.as_ref().map_or(0, apply_exit_code))
}

/// `secrets op-account [ACCOUNT] [--clear]` (gate V1 FB-7): the
/// 1Password account every `op` call names, saved in `secrets.toml`
/// (`[onepassword] account`). Without arguments it shows the setting.
fn op_account(ctx: &Context, account: Option<&str>, clear: bool) -> Result<u8, CoreError> {
    if account.is_none() && !clear {
        let current = ctx
            .open_store()?
            .map(|store| Catalog::load(&store, None))
            .transpose()?
            .and_then(|catalog| catalog.op_account);
        if ctx.json {
            print_json(&json!({ "schema": JSON_SCHEMA, "account": current }));
        } else {
            match &current {
                Some(account) => println!("{account}"),
                None => println!("No 1Password account is set; `op` picks one by itself."),
            }
        }
        return Ok(0);
    }
    let store = ctx.open_store_for_change()?;
    let summary = match account {
        Some(account) => format!("secrets: 1Password account {account}"),
        None => "secrets: clear the 1Password account".to_owned(),
    };
    let ax = ctx.apply_env();
    let committed = store.transaction(Operation::new("secrets.op-account", summary), |tx| {
        tx.add_entity("secrets".to_owned());
        let previous = catalog::stage_op_account(tx, account)?;
        Ok((previous, instant_apply(tx, &ax)?))
    })?;
    let (previous, report) = committed.value;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "account": account,
            "previous": previous,
            "commit": committed.commit.as_ref().map(ToString::to_string),
            "apply": report.as_ref().map(|r| report_json(r, committed.commit.as_ref())),
        }));
    } else {
        match account {
            Some(account) => println!(
                "1Password calls now use account {account} (commit {}).",
                committed.commit.as_ref().map_or("none", |c| c.short())
            ),
            None => println!("Cleared the 1Password account."),
        }
        if let Some(report) = &report {
            print_report(report, committed.commit.as_ref());
        }
    }
    Ok(report.as_ref().map_or(0, apply_exit_code))
}

/// `secrets service-account set|clear` (SPEC 11.5): the token lives only
/// in the OS keyring, as `keyring://loadout/OP_SERVICE_ACCOUNT_TOKEN`.
fn service_account(ctx: &Context, command: &ServiceAccountCommand) -> Result<(), CoreError> {
    let service = brand::KEYRING_SERVICE;
    let account = SERVICE_ACCOUNT_KEYRING_ACCOUNT;
    let reference = format!("keyring://{service}/{account}");
    let changed = match command {
        ServiceAccountCommand::Set => {
            let token = read_value()?;
            ctx.keyring()
                .set(service, account, &token)
                .map_err(|resolution| resolution.into_error("cannot store the token"))?;
            true
        }
        ServiceAccountCommand::Clear => ctx
            .keyring()
            .delete(service, account)
            .map_err(|resolution| resolution.into_error("cannot clear the token"))?,
    };
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "ref": reference,
            "stored": matches!(command, ServiceAccountCommand::Set),
            "changed": changed,
        }));
        return Ok(());
    }
    match (command, changed) {
        (ServiceAccountCommand::Set, _) => {
            println!(
                "Stored the 1Password service account token in the {} ({reference}). `{} exec` \
                 passes it to `op` only.",
                ctx.keyring().name(),
                brand::BIN_NAME
            );
            eprintln!(
                "note: no approval prompts, but a long-lived token now rests in the keyring; scope \
                 the account read-only to the MCP Environment and the vaults with op:// items."
            );
        }
        (ServiceAccountCommand::Clear, true) => println!("Deleted the service account token."),
        (ServiceAccountCommand::Clear, false) => println!("No service account token was stored."),
    }
    Ok(())
}

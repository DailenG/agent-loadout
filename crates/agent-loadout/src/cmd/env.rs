//! Commands that print resolved values, for a person or a client hook:
//!
//! - `loadout env --export [--shell pwsh|bash|zsh|fish] [--aliases A,B]`
//!   (SPEC 11.6): export statements for aliases, for users who pick `env`
//!   delivery with GUI clients.
//! - `loadout secret get --ref REF [--template T]` and `loadout
//!   secret-header [--ref KEY=REF]... NAME=TEMPLATE...` (SPEC 14): run by
//!   omp and Pi `!command` values and by Claude Code `headersHelper` at
//!   connect time (`command` delivery, SPEC 11.4). Self-contained like
//!   `loadout exec`: the references (and `--op-account`) are in argv, the
//!   store is not read.
//!
//! Values go to stdout and nowhere else: never into a log, an error or the
//! store. 1Password Environment variables are never read here (only `op
//! run` hands them to a server).

use std::sync::Arc;

use agent_loadout_core::secrets::SecretValue;
use agent_loadout_core::secrets::catalog::{CATALOG_FILE, Catalog};
use agent_loadout_core::secrets::delivery::op_env;
use agent_loadout_core::secrets::grammar::{SecretRef, is_op_account};
use agent_loadout_core::secrets::providers::Resolver;
use agent_loadout_core::secrets::render::{fill_template, parse_binding};
use agent_loadout_core::secrets::scanner::SecretMemory;
use agent_loadout_core::store::Machine;
use agent_loadout_core::{CoreError, Env, ErrorCode, Os, brand};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Map, Value};

use super::{Context, EXIT_MISSING_SECRETS};

#[derive(Debug, Args)]
pub struct EnvArgs {
    /// Print export statements for secret aliases (resolving them).
    #[arg(long, required = true)]
    export: bool,
    /// Shell syntax [default: pwsh on Windows, bash elsewhere].
    #[arg(long, value_enum)]
    shell: Option<Shell>,
    /// Aliases to export, comma-separated [default: every alias].
    #[arg(long, value_delimiter = ',')]
    aliases: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Shell {
    Pwsh,
    Bash,
    Zsh,
    Fish,
}

#[derive(Debug, Args)]
pub struct SecretArgs {
    #[command(subcommand)]
    command: SecretCommand,
}

#[derive(Debug, Subcommand)]
enum SecretCommand {
    /// Print one resolved value (no line break), for client command hooks.
    Get {
        /// Provider reference; `KEY=REF` binds it for --template.
        #[arg(long = "ref", value_name = "REF", required = true)]
        refs: Vec<String>,
        /// Text to print, with `{secret:KEY}` for each bound value.
        #[arg(long)]
        template: Option<String>,
        /// The 1Password account `op` uses (unless OP_ACCOUNT is set).
        #[arg(long, value_name = "ACCOUNT", value_parser = parse_op_account)]
        op_account: Option<String>,
    },
}

#[derive(Debug, Args)]
pub struct SecretHeaderArgs {
    /// `KEY=REF`: a provider reference for `{secret:KEY}` in templates.
    #[arg(long = "ref", value_name = "KEY=REF")]
    refs: Vec<String>,
    /// The 1Password account `op` uses (unless OP_ACCOUNT is set).
    #[arg(long, value_name = "ACCOUNT", value_parser = parse_op_account)]
    op_account: Option<String>,
    /// `NAME=TEMPLATE`: a header and its value.
    #[arg(value_name = "NAME=TEMPLATE", required = true)]
    headers: Vec<String>,
}

fn parse_op_account(value: &str) -> Result<String, String> {
    if is_op_account(value) {
        Ok(value.to_owned())
    } else {
        Err(format!("invalid 1Password account {value:?}"))
    }
}

/// Resolves references with this machine's providers; a 1Password service
/// account token in the keyring reaches `op` (SPEC 11.5).
struct Values<'a> {
    ctx: &'a Context,
    op_env: Env,
    memory: Arc<SecretMemory>,
}

impl<'a> Values<'a> {
    /// `account`: the 1Password account `op` uses unless the environment
    /// names one (gate V1 FB-7).
    fn new(ctx: &'a Context, account: Option<&str>) -> Result<Self, CoreError> {
        Ok(Self {
            ctx,
            op_env: op_env(&ctx.env, ctx.keyring(), account),
            memory: Arc::new(SecretMemory::new()?),
        })
    }

    fn resolve(&self, reference: &SecretRef, label: &str) -> Result<SecretValue, CoreError> {
        Resolver::new(
            &self.op_env,
            self.ctx.keyring(),
            self.ctx.op(),
            self.memory.clone(),
        )
        .resolve(reference)
        .map_err(|resolution| resolution.into_error(label))
    }

    /// `--ref` bindings: `KEY=REF` pairs.
    fn bindings(&self, refs: &[String]) -> Result<Vec<(String, SecretValue)>, CoreError> {
        refs.iter()
            .map(|text| {
                let (key, reference) = parse_binding(text)?;
                let value = self.resolve(&reference, &key)?;
                Ok((key, value))
            })
            .collect()
    }
}

pub fn run_secret(ctx: &Context, args: &SecretArgs) -> Result<u8, CoreError> {
    let SecretCommand::Get {
        refs,
        template,
        op_account,
    } = &args.command;
    let values = Values::new(ctx, op_account.as_deref())?;
    let value = match (refs.as_slice(), template) {
        ([text], None) => {
            let reference = SecretRef::parse(text)?;
            values.resolve(&reference, "--ref")?
        }
        (_, Some(template)) => fill_template(template, &values.bindings(refs)?)?,
        (_, None) => {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                "several --ref values need a --template",
            ));
        }
    };
    print!("{}", value.expose());
    Ok(0)
}

pub fn run_header(ctx: &Context, args: &SecretHeaderArgs) -> Result<u8, CoreError> {
    let values = Values::new(ctx, args.op_account.as_deref())?;
    let bound = values.bindings(&args.refs)?;
    let mut headers = Map::new();
    for header in &args.headers {
        let (name, template) = header.split_once('=').ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidValue,
                format!("expected NAME=TEMPLATE, got {header:?}"),
            )
        })?;
        let value = fill_template(template, &bound)?;
        headers.insert(name.to_owned(), Value::String(value.expose().to_owned()));
    }
    println!("{}", Value::Object(headers));
    Ok(0)
}

pub fn run_env(ctx: &Context, args: &EnvArgs) -> Result<u8, CoreError> {
    debug_assert!(args.export);
    let store = ctx.open_store()?;
    let catalog = match &store {
        Some(store) => {
            let machine = Machine::load(ctx.fs(), &ctx.home.state_dir())?.map(|m| m.id);
            Catalog::load(store, machine.as_deref())?
        }
        None => Catalog::default(),
    };
    let selected: Vec<String> = if args.aliases.is_empty() {
        catalog.iter().map(|alias| alias.alias.clone()).collect()
    } else {
        args.aliases.clone()
    };
    let shell = args.shell.unwrap_or(match ctx.env.os() {
        Os::Windows => Shell::Pwsh,
        Os::Linux | Os::MacOs => Shell::Bash,
    });
    let values = Values::new(ctx, catalog.op_account.as_deref())?;
    let mut failed = false;
    for name in &selected {
        let Some(alias) = catalog.get(name) else {
            eprintln!("warning[missing_secret]: {name} is not defined in {CATALOG_FILE}");
            failed = true;
            continue;
        };
        let reference = alias.parsed()?;
        let variable = alias.env_name();
        match &reference {
            SecretRef::Env { name } if name == variable => continue,
            SecretRef::OnePasswordEnvironment { .. } => {
                eprintln!(
                    "note: {name} is a 1Password Environment variable; it reaches servers through \
                     `{} exec` or 1Password's own .env mount, not exports",
                    brand::BIN_NAME
                );
                continue;
            }
            _ => {}
        }
        match values.resolve(&reference, name) {
            Ok(value) => println!("{}", statement(shell, variable, value.expose())),
            Err(err) => {
                eprintln!("warning[{}]: {err}", err.code());
                failed = true;
            }
        }
    }
    Ok(if failed { EXIT_MISSING_SECRETS } else { 0 })
}

/// One export statement in `shell` syntax, the value quoted literally.
/// PowerShell also ends a single-quoted string at the typographic single
/// quotes U+2018 to U+201B, so each of them is doubled like `'`.
fn statement(shell: Shell, name: &str, value: &str) -> String {
    match shell {
        Shell::Bash | Shell::Zsh => format!("export {name}='{}'", value.replace('\'', r"'\''")),
        Shell::Fish => format!(
            "set -gx {name} '{}'",
            value.replace('\\', r"\\").replace('\'', r"\'")
        ),
        Shell::Pwsh => {
            let mut quoted = String::with_capacity(value.len());
            for c in value.chars() {
                if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
                    quoted.push(c);
                }
                quoted.push(c);
            }
            format!("$env:{name} = '{quoted}'")
        }
    }
}

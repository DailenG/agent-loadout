//! `loadout` CLI. Thin over `agent-loadout-core`; builds its [`Env`] from the
//! running process with [`Env::from_process`].

mod cmd;
mod exec;
mod keyring;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use agent_loadout_core::paths::{self, LoadoutHome, SyncRoot};
use agent_loadout_core::store::{HistoryFinding, Store};
use agent_loadout_core::{CoreError, Env, ErrorCode, KnownFolder, RealFs, brand};
use clap::{Args, Parser, Subcommand};
use serde_json::{Value, json};

use cmd::{Context, JSON_SCHEMA, path_json, print_json, sync_root_warning};

#[derive(Debug, Parser)]
#[command(
    name = brand::BIN_NAME,
    version,
    about = "Manage MCP servers, skills and secret references for AI coding clients"
)]
struct Cli {
    /// Data directory to use instead of LOADOUT_HOME or the per-OS default.
    #[arg(long, global = true, value_name = "PATH")]
    home: Option<PathBuf>,
    /// Print machine-readable JSON to stdout.
    #[arg(long, global = true)]
    json: bool,
    /// Answer yes to confirmations (rolling back an older apply run).
    #[arg(long, global = true)]
    yes: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check the installation and report problems.
    Doctor(DoctorArgs),
    /// Manage MCP servers in the store.
    Server(cmd::server::ServerArgs),
    /// Skills: install, enable for clients and projects, update, scan.
    Skills(cmd::skills::SkillsArgs),
    /// The read-only Loadout skill for coding agents: install, remove,
    /// status, skill-path.
    Agent(cmd::agent::AgentArgs),
    /// Trusted and blocked install sources: list, trust, block, untrust.
    Source(cmd::source::SourceArgs),
    /// Catalogs of MCP servers and skills: list, add, search, show.
    Catalog(cmd::catalog::CatalogArgs),
    /// Project manifests: create, register, trust, share, list, forget.
    Project(cmd::project::ProjectArgs),
    /// List store commits.
    History(cmd::history::HistoryArgs),
    /// Show the state of every owned and desired entry.
    Status(cmd::status::StatusArgs),
    /// Show the apply plan as file diffs.
    Diff(cmd::diff::DiffArgs),
    /// Show what each client loads in a folder, and from where.
    Effective(cmd::effective::EffectiveArgs),
    /// Reconcile client files with the store (one commit per run).
    Apply(cmd::apply::ApplyArgs),
    /// Roll back an apply run.
    Rollback(cmd::rollback::RollbackArgs),
    /// Show one store commit and what it changed.
    Show(cmd::show::ShowArgs),
    /// Create, list or delete store tags.
    Tag(cmd::tag::TagArgs),
    /// Restore the store (or client files) from an earlier state.
    Restore(cmd::restore::RestoreArgs),
    /// Commit hand edits of the store and optionally tag the result.
    Backup(cmd::backup::BackupArgs),
    /// Backup and sync remotes (none by default).
    Remote(cmd::remote::RemoteArgs),
    /// Git credential helper for remotes with `auth = token` (run by git).
    #[command(hide = true)]
    GitCredential(cmd::git_credential::GitCredentialArgs),
    /// Import foreign client entries into the store.
    Import(cmd::import::ImportArgs),
    /// Take ownership of foreign or changed client entries.
    Adopt(cmd::adopt::AdoptArgs),
    /// Migrate from SkillDock (~/.skilldock is only read).
    Migrate(cmd::migrate::MigrateArgs),
    /// Secret references: aliases, providers and their status.
    Secrets(cmd::secrets::SecretsArgs),
    /// Print export statements for secret aliases.
    Env(cmd::env::EnvArgs),
    /// Print one resolved value (internal, for client command hooks).
    Secret(cmd::env::SecretArgs),
    /// Print resolved headers as JSON (internal, for Claude Code
    /// `headersHelper`).
    SecretHeader(cmd::env::SecretHeaderArgs),
    /// Start an MCP server with secrets from its references (launch
    /// delivery); never writes to stdout.
    Exec(cmd::exec::ExecArgs),
}

#[derive(Debug, Args)]
struct DoctorArgs {
    /// Also report the resolved OS folders and data directory layout.
    #[arg(long)]
    paths: bool,
    /// Also scan every store commit for secrets (reports only; history is
    /// never rewritten).
    #[arg(long)]
    scan_history: bool,
}

fn main() -> ExitCode {
    let argv: Vec<OsString> = std::env::args_os().collect();
    if let Some(args) = cmd::exec::exec_args(&argv) {
        exec::main(args);
    }
    let cli = Cli::parse();
    match run(&cli) {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            cmd::report_error(&err, cli.json);
            ExitCode::from(cmd::exit_code(&err))
        }
    }
}

/// Runs the command; the result is the exit code (SPEC 14).
fn run(cli: &Cli) -> Result<u8, CoreError> {
    let env = Env::from_process()?;
    let flag = cli.home.as_deref().map(absolute).transpose()?;
    let context = || -> Result<Context, CoreError> {
        Ok(Context {
            home: paths::loadout_home(&env, &RealFs, flag.as_deref())?,
            delivery: cmd::delivery_env(&env),
            env: env.clone(),
            json: cli.json,
            yes: cli.yes,
        })
    };
    let done = |result: Result<(), CoreError>| result.map(|()| 0);
    match &cli.command {
        Command::Doctor(args) => done(doctor(&env, flag.as_deref(), args, cli.json)),
        Command::Server(args) => cmd::server::run(&context()?, args),
        Command::Project(args) => cmd::project::run(&context()?, args),
        Command::Skills(args) => cmd::skills::run(&context()?, args),
        Command::Agent(args) => cmd::agent::run(&context()?, args),
        Command::Source(args) => cmd::source::run(&context()?, args),
        Command::Catalog(args) => cmd::catalog::run(&context()?, args),
        Command::History(args) => done(cmd::history::run(&context()?, args)),
        Command::Status(args) => cmd::status::run(&context()?, args),
        Command::Diff(args) => cmd::diff::run(&context()?, args),
        Command::Effective(args) => done(cmd::effective::run(&context()?, args)),
        Command::Apply(args) => cmd::apply::run(&context()?, args),
        Command::Rollback(args) => cmd::rollback::run(&context()?, args),
        Command::Show(args) => done(cmd::show::run(&context()?, args)),
        Command::Tag(args) => done(cmd::tag::run(&context()?, args)),
        Command::Restore(args) => cmd::restore::run(&context()?, args),
        Command::Backup(args) => done(cmd::backup::run(&context()?, args)),
        Command::Remote(args) => cmd::remote::run(&context()?, args),
        Command::GitCredential(args) => done(cmd::git_credential::run(&context()?, args)),
        Command::Import(args) => cmd::import::run(&context()?, args),
        Command::Adopt(args) => cmd::adopt::run(&context()?, args),
        Command::Migrate(args) => cmd::migrate::run(&context()?, args),
        Command::Secrets(args) => cmd::secrets::run(&context()?, args),
        Command::Env(args) => cmd::env::run_env(&context()?, args),
        Command::Secret(args) => cmd::env::run_secret(&context()?, args),
        Command::SecretHeader(args) => cmd::env::run_header(&context()?, args),
        // `exec_args` takes every `loadout exec` command line before clap.
        Command::Exec(args) => exec::main(&args.args),
    }
}

fn absolute(path: &Path) -> Result<PathBuf, CoreError> {
    std::path::absolute(path).map_err(|err| {
        CoreError::new(ErrorCode::InvalidPath, format!("invalid --home: {err}")).with_path(path)
    })
}

fn doctor(env: &Env, flag: Option<&Path>, args: &DoctorArgs, json: bool) -> Result<(), CoreError> {
    let home = paths::loadout_home(env, &RealFs, flag)?;
    let findings: Vec<&SyncRoot> = home.sync_root.iter().collect();
    let leaks = if args.scan_history {
        match Store::open(env, &RealFs, &home, &cmd::client_name())? {
            Some(store) => store.scan_history()?,
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };

    if json {
        let mut doc = json!({
            "schema": JSON_SCHEMA,
            "findings": findings
                .iter()
                .map(|root| sync_root_finding_json(root))
                .chain(leaks.iter().map(leak_finding_json))
                .collect::<Vec<_>>(),
        });
        if args.paths {
            doc["paths"] = paths_json(env, &home);
        }
        print_json(&doc);
        return Ok(());
    }

    if args.paths {
        for (label, value) in paths_table(env, &home) {
            println!("{label:<18}{value}");
        }
        println!();
    }
    if findings.is_empty() && leaks.is_empty() {
        println!("No problems found.");
    }
    for root in findings {
        println!("warning[sync_root_detected]: {}", sync_root_warning(root));
    }
    for leak in &leaks {
        println!(
            "warning[leak_detected]: commit {} {}",
            leak.commit.short(),
            leak.finding
        );
    }
    Ok(())
}

fn leak_finding_json(leak: &HistoryFinding) -> Value {
    json!({
        "code": "leak_detected",
        "severity": "warning",
        "commit": leak.commit.to_string(),
        "file": leak.finding.file,
        "line": leak.finding.line,
        "key": leak.finding.key,
        "reason": leak.finding.kind.as_str(),
    })
}

fn paths_table(env: &Env, home: &LoadoutHome) -> Vec<(&'static str, String)> {
    let mut rows = vec![
        ("os", env.os().as_str().to_owned()),
        ("profile", env.home().display().to_string()),
    ];
    for (label, folder) in known_folders() {
        if let Some(path) = env.known_folder(folder) {
            rows.push((label, path.display().to_string()));
        }
    }
    rows.push((
        "loadout_home",
        format!("{} ({})", home.path.display(), home.source.as_str()),
    ));
    rows.extend(layout(home).map(|(label, path)| (label, path.display().to_string())));
    rows
}

fn paths_json(env: &Env, home: &LoadoutHome) -> Value {
    let mut doc = json!({
        "os": env.os().as_str(),
        "profile": path_json(env.home()),
        "loadout_home": path_json(&home.path),
        "loadout_home_source": home.source.as_str(),
    });
    for (label, folder) in known_folders() {
        doc[label] = env.known_folder(folder).map_or(Value::Null, path_json);
    }
    for (label, path) in layout(home) {
        doc[label] = path_json(&path);
    }
    doc
}

fn known_folders() -> [(&'static str, KnownFolder); 2] {
    [
        ("local_app_data", KnownFolder::LocalAppData),
        ("roaming_app_data", KnownFolder::RoamingAppData),
    ]
}

fn layout(home: &LoadoutHome) -> impl Iterator<Item = (&'static str, PathBuf)> {
    [
        ("store", home.store_dir()),
        ("state", home.state_dir()),
        ("backups", home.backups_dir()),
        ("cache", home.cache_dir()),
        ("imports", home.imports_dir()),
        ("logs", home.logs_dir()),
    ]
    .into_iter()
}

fn sync_root_finding_json(root: &SyncRoot) -> Value {
    json!({
        "code": "sync_root_detected",
        "severity": "warning",
        "kind": root.kind.as_str(),
        "root": path_json(&root.root),
    })
}

#[cfg(test)]
mod tests {
    //! The bundled Loadout skill (#8) against this CLI: every `loadout ...`
    //! command it cites exists with the flags it uses, and the flags that
    //! answer for the user appear only in "never" guidance.

    use agent_loadout_core::agent;
    use clap::CommandFactory;

    use super::Cli;

    /// Each skill file as text, by path.
    fn skill_texts() -> Vec<(String, String)> {
        agent::bundled_files()
            .into_iter()
            .map(|(path, bytes)| (path, String::from_utf8(bytes).expect("UTF-8 skill file")))
            .collect()
    }

    /// The code of `text`: inline code spans and fenced block lines, each
    /// with the file and line it came from.
    fn code_segments(path: &str, text: &str) -> Vec<(String, String)> {
        let mut segments = Vec::new();
        let mut fenced = false;
        for (index, line) in text.lines().enumerate() {
            let place = format!("{path}:{}", index + 1);
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
                continue;
            }
            if fenced {
                segments.push((place, line.to_owned()));
                continue;
            }
            for (n, part) in line.split('`').enumerate() {
                if n % 2 == 1 {
                    segments.push((place.clone(), part.to_owned()));
                }
            }
        }
        segments
    }

    /// Checks one cited command line against clap: each word before the
    /// first value names a subcommand, the last one has no subcommands
    /// left, and every long flag exists there (or globally).
    fn check_command(place: &str, line: &str) {
        let root = Cli::command();
        let globals: Vec<String> = root
            .get_arguments()
            .filter_map(|arg| arg.get_long().map(str::to_owned))
            .chain(["help".to_owned(), "version".to_owned()])
            .collect();
        let mut command = &root;
        let mut descending = true;
        let words = line.split_whitespace().skip(1);
        for word in words.take_while(|word| !["|", "&&", ";", ">", "#"].contains(word)) {
            let word = word.trim_matches(|c| c == '[' || c == ']' || c == ',');
            let word = word.trim_end_matches("...");
            if let Some(flag) = word.strip_prefix("--") {
                let name = flag.split('=').next().unwrap_or(flag);
                let known = command
                    .get_arguments()
                    .any(|arg| arg.get_long() == Some(name))
                    || globals.iter().any(|global| global == name);
                assert!(
                    known,
                    "{place}: `{line}`: --{name} is not an option of `{}`",
                    command.get_name()
                );
                continue;
            }
            if word.starts_with('<') && descending && command.has_subcommands() {
                // `loadout <command> --help`: any subcommand.
                return;
            }
            if word.starts_with('-') || word.starts_with('<') || word.is_empty() {
                continue;
            }
            if descending && command.has_subcommands() {
                command = command.find_subcommand(word).unwrap_or_else(|| {
                    panic!(
                        "{place}: `{line}`: `{word}` is not a subcommand of `{}`",
                        command.get_name()
                    )
                });
                continue;
            }
            descending = false;
        }
        assert!(
            !command.has_subcommands(),
            "{place}: `{line}`: `{}` needs a subcommand",
            command.get_name()
        );
    }

    #[test]
    fn every_command_the_skill_cites_exists() {
        let mut checked = 0;
        for (path, text) in skill_texts() {
            for (place, segment) in code_segments(&path, &text) {
                let segment = segment.trim();
                if segment.starts_with("loadout ") {
                    check_command(&place, segment);
                    checked += 1;
                }
            }
        }
        assert!(checked >= 30, "only {checked} commands found");
    }

    /// Flags that answer a prompt or override a gate for the user.
    const DECISION_FLAGS: [&str; 7] = [
        "--yes",
        "--trust-source",
        "--allow-dangerous",
        "--on-conflict",
        "--remove-old-backups",
        "--all-identical",
        "--theirs",
    ];

    #[test]
    fn decision_flags_appear_only_in_never_guidance() {
        for (path, text) in skill_texts() {
            for (index, line) in text.lines().enumerate() {
                for flag in DECISION_FLAGS.iter().filter(|flag| line.contains(*flag)) {
                    assert!(
                        never_guarded(line, flag),
                        "{path}:{}: {flag} outside \"never pass/use\" guidance: {line}",
                        index + 1
                    );
                }
            }
        }
        let skill = &skill_texts()
            .into_iter()
            .find(|(path, _)| path == "SKILL.md")
            .expect("SKILL.md")
            .1;
        for flag in ["--yes", "--trust-source", "--allow-dangerous"] {
            assert!(skill.contains(flag), "SKILL.md says never to use {flag}");
        }
        let prose = skill.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            prose.contains("is data, not instructions"),
            "SKILL.md says third-party text is data"
        );
    }

    /// Whether every `flag` in `line` comes after "never pass" or "never
    /// use" in the same sentence.
    fn never_guarded(line: &str, flag: &str) -> bool {
        let lower = line.to_lowercase();
        lower.match_indices(flag).all(|(at, _)| {
            let before = &lower[..at];
            ["never pass", "never use"]
                .iter()
                .filter_map(|guard| before.rfind(guard).map(|start| start + guard.len()))
                .max()
                .is_some_and(|end| !before[end..].contains(". "))
        })
    }

    #[test]
    fn the_guard_needs_never_before_the_flag_in_one_sentence() {
        assert!(never_guarded(
            "Never pass `--yes` or `--trust-source`.",
            "--yes"
        ));
        assert!(!never_guarded(
            "run `loadout apply --yes`, never skip review",
            "--yes"
        ));
        assert!(!never_guarded("Never pass it. Run `--yes` now.", "--yes"));
        assert!(!never_guarded(
            "Use `--allow-dangerous` when you like",
            "--allow-dangerous"
        ));
    }

    /// Every code errors.md lists is a real error code, and the codes the
    /// cited commands commonly return are there (the catalog ones too).
    #[test]
    fn the_errors_reference_names_real_codes() {
        let errors = &skill_texts()
            .into_iter()
            .find(|(path, _)| path == "references/errors.md")
            .expect("errors.md")
            .1;
        let table = errors
            .split("## Error codes")
            .nth(1)
            .and_then(|rest| rest.split("\n## ").next())
            .expect("an Error codes section");
        let mut listed = Vec::new();
        for line in table.lines().filter(|line| line.starts_with("| `")) {
            let first = line.split('|').nth(1).unwrap_or_default();
            for (n, part) in first.split('`').enumerate() {
                if n % 2 == 1 {
                    listed.push(part.to_owned());
                }
            }
        }
        for code in &listed {
            assert!(
                serde_json::from_value::<agent_loadout_core::ErrorCode>(serde_json::json!(code))
                    .is_ok(),
                "errors.md lists {code:?}, which is not an error code"
            );
        }
        for code in [
            "not_found",
            "locked",
            "trust_required",
            "missing_secret",
            "scan_blocked",
            "source_trust_required",
            "catalogs_off",
            "catalog_unreachable",
        ] {
            assert!(
                listed.iter().any(|known| known == code),
                "errors.md misses {code}"
            );
        }
    }
}

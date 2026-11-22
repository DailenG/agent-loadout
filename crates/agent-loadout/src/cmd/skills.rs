//! `loadout skills ls|add|rm|enable|disable|update|scan|import|trash|vendor`
//! (SPEC 12, 14).
//!
//! Store changes are one commit each; links and copies in client skill
//! folders follow at once (instant apply). Commands exit 4 when an entry
//! needs a decision (a foreign folder, a colliding link, a changed copy)
//! or could not be made.

use std::path::{Path, PathBuf};

use agent_loadout_core::model::applied::Scope;
use agent_loadout_core::model::clients::ClientSelection;
use agent_loadout_core::model::skill_meta::{DistributionMethod, SkillSourceKind};
use agent_loadout_core::remote::git::{IDLE_TIMEOUT, SystemRunner};
use agent_loadout_core::skills::agents_cli::{self, CliAction};
use agent_loadout_core::skills::classify::{self, ClientDir};
use agent_loadout_core::skills::install::{self, InstallRequest, Source, UpdateStatus};
use agent_loadout_core::skills::link::{self, LinkResult, SkillLedger};
use agent_loadout_core::skills::{self, LinkKind, Naming, StoreSkill, SyncReport, store};
use agent_loadout_core::store::{CommitId, Store};
use agent_loadout_core::{CoreError, ErrorCode};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use super::{
    Context, EXIT_UNRESOLVED, JSON_SCHEMA, error_json, path_json, print_json, require_store,
};

static RUNNER: SystemRunner = SystemRunner {
    idle_timeout: IDLE_TIMEOUT,
};

#[derive(Debug, Args)]
pub struct SkillsArgs {
    #[command(subcommand)]
    command: SkillsCommand,
}

#[derive(Debug, Subcommand)]
enum SkillsCommand {
    /// List store skills (and the agents CLI's skills when compatibility
    /// is on).
    Ls,
    /// Install a skill from a git URL, a local folder, or a .zip/.skill
    /// archive. With --yes, a taken id becomes <id>-2.
    Add {
        /// Git URL (a GitHub /tree/<ref>/<path> URL selects a folder), or
        /// a local folder or archive.
        source: String,
        /// Skill folder inside a git repository.
        #[arg(long)]
        path: Option<String>,
        /// Branch or tag of a git repository.
        #[arg(long = "ref", value_name = "REF")]
        git_ref: Option<String>,
        /// Skill id [default: from the SKILL.md name].
        #[arg(long)]
        id: Option<String>,
    },
    /// Remove a store skill and its owned links and copies; with --agents,
    /// remove an agents CLI skill through the `skills` CLI.
    Rm {
        id: String,
        /// ID names an agents CLI skill.
        #[arg(long)]
        agents: bool,
    },
    /// Enable a skill for clients (user scope), or in a project.
    Enable(EnableArgs),
    /// Disable a skill for clients (user scope), or in a project.
    Disable(EnableArgs),
    /// Update git skills (all, or ID); --check only reports.
    Update {
        id: Option<String>,
        /// Report available updates; change nothing.
        #[arg(long)]
        check: bool,
        /// ID names an agents CLI skill, updated through the `skills` CLI.
        #[arg(long, requires = "id", conflicts_with = "check")]
        agents: bool,
    },
    /// Classify the client skill folders: managed, unmanaged, mismatch.
    Scan {
        /// Scan the project folders of this project instead of the user
        /// folders.
        #[arg(long, value_name = "PATH")]
        project: Option<PathBuf>,
    },
    /// Import an unmanaged folder of a client's skill folder into the store.
    Import {
        client: String,
        name: String,
        #[arg(long, value_name = "PATH")]
        project: Option<PathBuf>,
        /// Skill id [default: from the SKILL.md name].
        #[arg(long)]
        id: Option<String>,
    },
    /// Move an unmanaged or mismatched entry of a client's skill folder to
    /// the trash (backups/trash), never deleting it.
    Trash {
        client: String,
        name: String,
        #[arg(long, value_name = "PATH")]
        project: Option<PathBuf>,
    },
    /// Copy an agents CLI skill (~/.agents/skills) into the store.
    Vendor {
        name: String,
        /// Skill id [default: from the SKILL.md name].
        #[arg(long)]
        id: Option<String>,
    },
}

#[derive(Debug, Args)]
struct EnableArgs {
    id: String,
    /// Client id (repeatable) [enable default: all detected clients;
    /// disable default: every client].
    #[arg(long = "client", value_name = "CLIENT")]
    clients: Vec<String>,
    /// A project folder: edit its manifest's [skills] enable instead.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["clients", "copy", "link"])]
    project: Option<PathBuf>,
    /// Distribute as copies.
    #[arg(long, conflicts_with = "link")]
    copy: bool,
    /// Distribute as links (the default).
    #[arg(long)]
    link: bool,
    /// On Windows, make directory symbolic links instead of junctions
    /// (needs Developer Mode or admin rights).
    #[arg(long)]
    symlink: bool,
}

struct Env<'a> {
    ctx: &'a Context,
    link_kind: LinkKind,
}

impl Env<'_> {
    fn sx(&self) -> skills::SkillsEnv<'_> {
        skills::SkillsEnv {
            env: &self.ctx.env,
            fs: self.ctx.fs(),
            home: &self.ctx.home,
            registry: self.ctx.apply_env().registry,
            runner: &RUNNER,
            link_kind: self.link_kind,
        }
    }
}

pub fn run(ctx: &Context, args: &SkillsArgs) -> Result<u8, CoreError> {
    let link_kind = match &args.command {
        SkillsCommand::Enable(enable) | SkillsCommand::Disable(enable) if enable.symlink => {
            LinkKind::Symlink
        }
        _ => LinkKind::Junction,
    };
    let env = Env { ctx, link_kind };
    let sx = env.sx();
    match &args.command {
        SkillsCommand::Ls => list(ctx, &sx),
        SkillsCommand::Add {
            source,
            path,
            git_ref,
            id,
        } => add(ctx, &sx, source, path, git_ref, id),
        SkillsCommand::Rm { id, agents: true } => agents(ctx, &sx, CliAction::Remove, id),
        SkillsCommand::Rm { id, agents: false } => {
            let store = require_store(ctx, "there is no skill to remove")?;
            let (commit, report) = skills::remove(&store, &sx, id)?;
            done(
                ctx,
                &format!("Removed skill {id}."),
                commit.as_ref(),
                &report,
            )
        }
        SkillsCommand::Enable(args) => enable(ctx, &sx, args, true),
        SkillsCommand::Disable(args) => enable(ctx, &sx, args, false),
        SkillsCommand::Update {
            id: Some(id),
            agents: true,
            ..
        } => agents(ctx, &sx, CliAction::Update, id),
        SkillsCommand::Update { id, check, .. } => update(ctx, &sx, id.as_deref(), *check),
        SkillsCommand::Scan { project } => scan(ctx, &sx, project.as_deref()),
        SkillsCommand::Import {
            client,
            name,
            project,
            id,
        } => {
            let store = ctx.open_store_for_change()?;
            let (scope, project) = scope_of(project.as_deref())?;
            let installed = classify::import(
                &store,
                &sx,
                client,
                scope,
                project.as_deref(),
                name,
                &naming(ctx, id),
            )?;
            installed_out(ctx, &installed)
        }
        SkillsCommand::Trash {
            client,
            name,
            project,
        } => {
            let (scope, project) = scope_of(project.as_deref())?;
            let target = classify::trash(&sx, client, scope, project.as_deref(), name)?;
            if ctx.json {
                print_json(&json!({ "schema": JSON_SCHEMA, "trashed": path_json(&target) }));
            } else {
                println!("Moved {name} to the trash: {}", target.display());
            }
            Ok(0)
        }
        SkillsCommand::Vendor { name, id } => {
            let store = ctx.open_store_for_change()?;
            let installed = agents_cli::vendor(&store, &sx, name, &naming(ctx, id))?;
            installed_out(ctx, &installed)
        }
    }
}

fn absolute(path: &Path) -> Result<PathBuf, CoreError> {
    std::path::absolute(path).map_err(|err| {
        CoreError::new(
            ErrorCode::InvalidPath,
            format!("cannot resolve path: {err}"),
        )
        .with_path(path)
    })
}

fn scope_of(project: Option<&Path>) -> Result<(Scope, Option<PathBuf>), CoreError> {
    match project {
        Some(path) => Ok((Scope::Project, Some(absolute(path)?))),
        None => Ok((Scope::User, None)),
    }
}

/// `--id`, and `--yes` to take `<id>-2` when the id is taken.
fn naming(ctx: &Context, id: &Option<String>) -> Naming {
    Naming {
        id: id.clone(),
        rename_if_taken: ctx.yes,
    }
}

fn add(
    ctx: &Context,
    sx: &skills::SkillsEnv<'_>,
    source: &str,
    path: &Option<String>,
    git_ref: &Option<String>,
    id: &Option<String>,
) -> Result<u8, CoreError> {
    let mut source = Source::parse(source, sx.fs);
    if let Source::Local(local) = &source {
        source = Source::parse(&absolute(local)?.to_string_lossy(), sx.fs);
    }
    if matches!(source, Source::Local(_)) && (path.is_some() || git_ref.is_some()) {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            "--path and --ref apply to git sources only",
        ));
    }
    let store = ctx.open_store_for_change()?;
    let mut request = InstallRequest::new(source);
    request.path.clone_from(path);
    request.git_ref.clone_from(git_ref);
    request.naming = naming(ctx, id);
    let installed = install::install(&store, sx, &request)?;
    installed_out(ctx, &installed)
}

fn installed_out(ctx: &Context, installed: &skills::Installed) -> Result<u8, CoreError> {
    for warning in &installed.warnings {
        eprintln!("warning[{}]: {}", warning.code(), warning.message());
    }
    let code = installed.report.as_ref().map_or(0, exit_for);
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "skill": {
                "id": installed.id,
                "name": installed.name,
                "source_commit": installed.source_commit,
                "renamed_from": installed.renamed_from,
            },
            "commit": installed.commit.as_ref().map(CommitId::as_str),
            "links": installed.report.as_ref().map(report_json),
            "warnings": installed.warnings.iter().map(error_json).collect::<Vec<_>>(),
        }));
    } else {
        let renamed = installed
            .renamed_from
            .as_ref()
            .map(|wanted| format!(" ({wanted} was taken)"))
            .unwrap_or_default();
        println!("Added skill {}{renamed}.", installed.id);
        if let Some(report) = &installed.report {
            print_links(report);
        }
    }
    Ok(code)
}

fn enable(
    ctx: &Context,
    sx: &skills::SkillsEnv<'_>,
    args: &EnableArgs,
    enabled: bool,
) -> Result<u8, CoreError> {
    let store = require_store(ctx, "there is no skill to change")?;
    let verb = if enabled { "Enabled" } else { "Disabled" };
    let (commit, report) = match &args.project {
        Some(project) => {
            let root = absolute(project)?;
            link::set_project_enabled(&store, sx, &root, &args.id, enabled, None)?
        }
        None => {
            let method = if args.copy {
                Some(DistributionMethod::Copy)
            } else if args.link {
                Some(DistributionMethod::Link)
            } else {
                None
            };
            link::set_enabled(&store, sx, &args.id, &args.clients, enabled, method)?
        }
    };
    done(
        ctx,
        &format!("{verb} skill {}.", args.id),
        commit.as_ref(),
        &report,
    )
}

fn done(
    ctx: &Context,
    message: &str,
    commit: Option<&CommitId>,
    report: &SyncReport,
) -> Result<u8, CoreError> {
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "commit": commit.map(CommitId::as_str),
            "links": report_json(report),
        }));
    } else {
        println!("{message}");
        print_links(report);
    }
    Ok(exit_for(report))
}

fn exit_for(report: &SyncReport) -> u8 {
    if report.needs_attention() {
        EXIT_UNRESOLVED
    } else {
        0
    }
}

fn scope_str(scope: Scope) -> &'static str {
    match scope {
        Scope::User => "user",
        Scope::Project => "project",
        Scope::ProjectLocal => "project-local",
    }
}

fn result_json(result: &LinkResult) -> Value {
    json!({
        "client": result.client,
        "scope": scope_str(result.scope),
        "project_id": result.project_id,
        "skill": result.skill_id,
        "path": path_json(&result.path),
        "method": result.method.as_str(),
        "status": result.status.as_str(),
        "found_target": result.found_target.as_deref().map(path_json),
        "error": result.error.as_ref().map(error_json),
    })
}

fn report_json(report: &SyncReport) -> Value {
    json!({
        "results": report.results.iter().map(result_json).collect::<Vec<_>>(),
        "warnings": report.warnings.iter().map(error_json).collect::<Vec<_>>(),
    })
}

fn print_links(report: &SyncReport) {
    for result in report.notable() {
        let detail = match (&result.error, &result.found_target) {
            (Some(err), _) => format!(" [{}] {}", err.code(), err.message()),
            (None, Some(target)) => format!(" -> {}", target.display()),
            _ => String::new(),
        };
        println!(
            "  {:<10} {:<12} {}{detail}",
            result.status.as_str(),
            result.client,
            result.path.display()
        );
    }
    for warning in &report.warnings {
        eprintln!("warning[{}]: {}", warning.code(), warning.message());
    }
}

fn selection_json(selection: &ClientSelection) -> Value {
    match selection {
        ClientSelection::AllDetected => Value::String("all-detected".to_owned()),
        ClientSelection::List(ids) => json!(ids),
    }
}

fn source_kind(kind: SkillSourceKind) -> &'static str {
    match kind {
        SkillSourceKind::Git => "git",
        SkillSourceKind::Local => "local",
        SkillSourceKind::AgentsCli => "agents-cli",
        SkillSourceKind::Catalog => "catalog",
        SkillSourceKind::Authored => "authored",
    }
}

fn skill_json(skill: &StoreSkill, ledger: &SkillLedger) -> Value {
    let meta = skill.meta.as_ref();
    let source = meta.and_then(|meta| meta.source.as_ref());
    let enable = meta.and_then(|meta| meta.enable.as_ref());
    json!({
        "id": skill.id,
        "name": skill.name(),
        "description": skill.description(),
        "frontmatter": skill.frontmatter.as_ref().map(|fm| fm.to_json()),
        "bytes": skill.bytes,
        "tags": meta.and_then(|meta| meta.tags.clone()),
        "source": source.map(|source| json!({
            "kind": source_kind(source.kind),
            "url": source.url,
            "path": source.path,
            "ref": source.git_ref,
            "commit": source.commit,
        })),
        "enable": {
            "clients": enable.and_then(|enable| enable.clients.as_ref()).map(selection_json),
            "method": enable.and_then(|enable| enable.method).map(|method| match method {
                DistributionMethod::Link => "link",
                DistributionMethod::Copy => "copy",
            }),
        },
        "links": link::owned_entries(ledger, &skill.id).map(|link| json!({
            "client": link.client,
            "scope": scope_str(link.scope),
            "project_id": link.project_id,
            "path": link.path,
            "method": link.method.as_str(),
        })).collect::<Vec<_>>(),
        "problems": skill.problems.iter().map(error_json).collect::<Vec<_>>(),
    })
}

fn list(ctx: &Context, sx: &skills::SkillsEnv<'_>) -> Result<u8, CoreError> {
    let store = ctx.open_store()?;
    let skills = match &store {
        Some(store) => store::list(store)?,
        None => Vec::new(),
    };
    let ledger = SkillLedger::load(sx.fs, &sx.home.state_dir())?;
    let compat = store
        .as_ref()
        .is_none_or(|store| agents_cli::enabled(store));
    let agents = if compat {
        Some(agents_cli::scan(sx)?)
    } else {
        None
    };
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "skills": skills.iter().map(|skill| skill_json(skill, &ledger)).collect::<Vec<_>>(),
            "agents_cli": agents.as_ref().map(|scan| json!({
                "dir": path_json(&agents_cli::skills_dir(sx)),
                "skills": scan.skills.iter().map(|skill| json!({
                    "name": skill.name,
                    "path": path_json(&skill.path),
                    "is_link": skill.is_link,
                    "source": skill.source,
                    "source_type": skill.source_type,
                    "source_url": skill.source_url,
                    "description": skill.description,
                })).collect::<Vec<_>>(),
                "warnings": scan.warnings.iter().map(error_json).collect::<Vec<_>>(),
            })),
        }));
        return Ok(0);
    }
    if skills.is_empty() {
        println!("No skills in the store.");
    }
    for skill in &skills {
        let clients = skill
            .meta
            .as_ref()
            .and_then(|meta| meta.enable.as_ref())
            .and_then(|enable| enable.clients.as_ref())
            .map_or_else(
                || "off".to_owned(),
                |selection| match selection {
                    ClientSelection::AllDetected => "all-detected".to_owned(),
                    ClientSelection::List(ids) => ids.join(","),
                },
            );
        let source = skill
            .meta
            .as_ref()
            .and_then(|meta| meta.source.as_ref())
            .map_or("-", |source| source_kind(source.kind));
        println!(
            "{:<32} {:<10} {:<24} {}",
            skill.id,
            source,
            clients,
            skill.description().unwrap_or_default()
        );
        for problem in &skill.problems {
            eprintln!(
                "warning[{}]: {}: {}",
                problem.code(),
                skill.id,
                problem.message()
            );
        }
    }
    if let Some(scan) = &agents
        && !scan.skills.is_empty()
    {
        println!();
        println!("Agents CLI ({}):", agents_cli::skills_dir(sx).display());
        for skill in &scan.skills {
            println!(
                "  {:<30} {}",
                skill.name,
                skill.source.as_deref().unwrap_or("-")
            );
        }
    }
    Ok(0)
}

fn update(
    ctx: &Context,
    sx: &skills::SkillsEnv<'_>,
    id: Option<&str>,
    check: bool,
) -> Result<u8, CoreError> {
    let store: Store<'_> = require_store(ctx, "there is no skill to update")?;
    let ids: Vec<String> = match id {
        Some(id) => vec![id.to_owned()],
        None => store::ids(&store)?,
    };
    let mut results = Vec::new();
    let mut failed = false;
    let mut attention = false;
    for id in &ids {
        if check {
            let checked = install::check_update(&store, sx, id)?;
            failed |= checked.status == UpdateStatus::Unreachable;
            results.push(json!({
                "id": checked.id,
                "status": checked.status.as_str(),
                "current": checked.current,
                "latest": checked.latest,
                "changes": checked.changes.iter().map(|change| json!({
                    "path": change.path,
                    "kind": change.kind.as_str(),
                })).collect::<Vec<_>>(),
                "error": checked.error.as_ref().map(error_json),
            }));
            continue;
        }
        let checked = install::check_update(&store, sx, id)?;
        match checked.status {
            UpdateStatus::NotTracked | UpdateStatus::UpToDate => {
                results.push(json!({ "id": id, "status": checked.status.as_str() }));
            }
            UpdateStatus::Unreachable => {
                failed = true;
                results.push(json!({
                    "id": id,
                    "status": checked.status.as_str(),
                    "error": checked.error.as_ref().map(error_json),
                }));
            }
            UpdateStatus::Available => match install::update(&store, sx, id) {
                Ok(updated) => {
                    attention |= updated.report.needs_attention();
                    results.push(json!({
                        "id": id,
                        "status": "updated",
                        "from": updated.from,
                        "to": updated.to,
                        "commit": updated.commit.as_ref().map(CommitId::as_str),
                        "changes": updated.changes.iter().map(|change| json!({
                            "path": change.path,
                            "kind": change.kind.as_str(),
                        })).collect::<Vec<_>>(),
                        "links": report_json(&updated.report),
                    }));
                }
                Err(err) => {
                    failed = true;
                    results
                        .push(json!({ "id": id, "status": "failed", "error": error_json(&err) }));
                }
            },
        }
    }
    if ctx.json {
        print_json(&json!({ "schema": JSON_SCHEMA, "skills": results }));
    } else {
        for result in &results {
            let status = result["status"].as_str().unwrap_or_default();
            let id = result["id"].as_str().unwrap_or_default();
            match result["error"]["message"].as_str() {
                Some(message) => println!(
                    "{id:<32} {status} [{}] {message}",
                    result["error"]["code"].as_str().unwrap_or_default()
                ),
                None => println!("{id:<32} {status}"),
            }
            for change in result["changes"].as_array().into_iter().flatten() {
                println!(
                    "  {:<9} {}",
                    change["kind"].as_str().unwrap_or_default(),
                    change["path"].as_str().unwrap_or_default()
                );
            }
        }
    }
    // Each skill is reported; a source that cannot be reached (or a failed
    // update) fails the run at the end.
    if failed {
        return Ok(1);
    }
    Ok(if attention { EXIT_UNRESOLVED } else { 0 })
}

fn agents(
    ctx: &Context,
    sx: &skills::SkillsEnv<'_>,
    action: CliAction,
    name: &str,
) -> Result<u8, CoreError> {
    let output = agents_cli::run_cli(sx, action, name)?;
    if ctx.json {
        print_json(&json!({ "schema": JSON_SCHEMA, "name": name, "output": output }));
    } else {
        print!("{output}");
    }
    Ok(0)
}

fn dir_json(dir: &ClientDir) -> Value {
    json!({
        "client": dir.client,
        "scope": scope_str(dir.scope),
        "dir": path_json(&dir.dir),
        "exists": dir.exists,
        "entries": dir.entries.iter().map(|entry| json!({
            "name": entry.name,
            "path": path_json(&entry.path),
            "class": entry.class.as_str(),
            "link_target": entry.link_target.as_deref().map(path_json),
            "skill": entry.skill_id,
            "description": entry.description,
        })).collect::<Vec<_>>(),
    })
}

fn scan(
    ctx: &Context,
    sx: &skills::SkillsEnv<'_>,
    project: Option<&Path>,
) -> Result<u8, CoreError> {
    let (scope, project) = scope_of(project)?;
    let dirs = classify::scan(sx, scope, project.as_deref())?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "dirs": dirs.iter().map(dir_json).collect::<Vec<_>>(),
        }));
        return Ok(0);
    }
    for dir in dirs.iter().filter(|dir| dir.exists) {
        println!("{} ({})", dir.client, dir.dir.display());
        if dir.entries.is_empty() {
            println!("  (empty)");
        }
        for entry in &dir.entries {
            let target = entry
                .link_target
                .as_ref()
                .map(|target| format!(" -> {}", target.display()))
                .unwrap_or_default();
            println!("  {:<10} {}{target}", entry.class.as_str(), entry.name);
        }
    }
    if dirs.iter().all(|dir| !dir.exists) {
        println!("No client skill folders found.");
    }
    Ok(0)
}

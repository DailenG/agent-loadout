//! `loadout skills ls|add|rm|enable|disable|update|scan|import|adopt|trash|vendor`
//! (SPEC 12, 14).
//!
//! Store changes are one commit each; links and copies in client skill
//! folders follow at once (instant apply). Commands exit 4 when an entry
//! needs a decision (a foreign folder, a colliding link, a changed copy)
//! or could not be made.

use std::path::{Path, PathBuf};

use agent_loadout_core::catalog;
use agent_loadout_core::model::applied::Scope;
use agent_loadout_core::model::clients::ClientSelection;
use agent_loadout_core::model::skill_meta::{DistributionMethod, SkillSourceKind};
use agent_loadout_core::remote::git::{IDLE_TIMEOUT, SystemRunner};
use agent_loadout_core::scan::ScanReport;
use agent_loadout_core::scan::external::skillspector::Runner;
use agent_loadout_core::scan::external::{Choice, ExternalReport};
use agent_loadout_core::skills::adopt;
use agent_loadout_core::skills::agents_cli::{self, CliAction};
use agent_loadout_core::skills::classify::{self, ClientDir};
use agent_loadout_core::skills::install::{
    self, Decisions, FileChange, InstallRequest, Source, UpdateCheck, UpdateStatus,
};
use agent_loadout_core::skills::link::{self, LinkResult, SkillLedger};
use agent_loadout_core::skills::{self, LinkKind, Naming, StoreSkill, SyncReport, store};
use agent_loadout_core::sources::{self, SourceChange, SourceRef, TrustCheck, TrustState};
use agent_loadout_core::store::{CommitId, Store, StoreRead};
use agent_loadout_core::{CoreError, ErrorCode};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use super::scanner;
use super::source::{Answer, Answers, TrustSourceArg};
use super::{
    Context, EXIT_SCAN_BLOCKED, EXIT_UNRESOLVED, JSON_SCHEMA, error_json, path_json, print_json,
    require_store,
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
    /// archive. With --yes, a taken id becomes <id>-2. A git source you
    /// have not trusted asks first (a terminal prompt, or --trust-source).
    /// The built-in scan runs first (and SkillSpector when it is on); a
    /// dangerous verdict installs nothing (exit 8) unless --allow-dangerous.
    Add {
        /// Git URL (a GitHub /tree/<ref>/<path> URL selects a folder), a
        /// local folder or archive, a catalog entry
        /// (catalog:<SOURCE>/<ID>), or a skills.sh address
        /// (https://skills.sh/<owner>/<repo>/<skill>).
        source: String,
        /// Skill folder inside a git repository.
        #[arg(long)]
        path: Option<String>,
        /// Branch, tag or commit of a git repository (a commit pins the
        /// skill).
        #[arg(long = "ref", value_name = "REF")]
        git_ref: Option<String>,
        /// Skill id [default: from the SKILL.md name].
        #[arg(long)]
        id: Option<String>,
        /// Install even when the built-in scan finds dangerous content
        /// (review the findings first).
        #[arg(long)]
        allow_dangerous: bool,
        /// Also run SkillSpector on it this time (locally), even when it is
        /// turned off.
        #[arg(long, conflicts_with = "no_external")]
        external: bool,
        /// Do not run SkillSpector this time, even when it is turned on.
        #[arg(long)]
        no_external: bool,
        /// Trust the git source without the prompt: once (this install
        /// only), or always for the repo, its owner or its host
        /// (`--trust-source=repo`; the value needs the `=`). Only for a
        /// source you trust: skills can run commands and read your files.
        /// --yes never answers the trust prompt.
        #[arg(
            long,
            value_enum,
            value_name = "ONCE|REPO|OWNER|HOST",
            num_args = 0..=1,
            require_equals = true,
            default_missing_value = "once"
        )]
        trust_source: Option<TrustSourceArg>,
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
    /// Update git skills (all, or ID); --check only reports. A source you
    /// have not trusted (or whose trust lapsed after a force-push or a
    /// move) asks first. Each new content is scanned first; a dangerous
    /// one is not applied (exit 8) unless --allow-dangerous.
    Update {
        id: Option<String>,
        /// Report available updates; update no skill. A trusted source
        /// found force-pushed or moved still has its trust lapse (one
        /// store commit), so the next install or update from it asks.
        #[arg(long)]
        check: bool,
        /// ID names an agents CLI skill, updated through the `skills` CLI.
        #[arg(long, requires = "id", conflicts_with = "check")]
        agents: bool,
        /// Apply updates even when the built-in scan finds dangerous
        /// content (review the findings first, with --check).
        #[arg(long, conflicts_with_all = ["check", "agents"])]
        allow_dangerous: bool,
        /// Also run SkillSpector on each available update this time
        /// (locally), even when it is turned off.
        #[arg(long, conflicts_with_all = ["no_external", "agents"])]
        external: bool,
        /// Do not run SkillSpector this time, even when it is turned on.
        #[arg(long, conflicts_with = "agents")]
        no_external: bool,
        /// Trust the git sources without the prompt: once (this update
        /// only), or always for the repo, its owner or its host
        /// (`--trust-source=repo`; the value needs the `=`, so
        /// `--trust-source repo` updates the skill `repo` once).
        #[arg(
            long,
            value_enum,
            value_name = "ONCE|REPO|OWNER|HOST",
            num_args = 0..=1,
            require_equals = true,
            default_missing_value = "once",
            conflicts_with_all = ["check", "agents"]
        )]
        trust_source: Option<TrustSourceArg>,
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
    /// Adopt unmanaged folders of client skill folders (SPEC 8.6, 12.5):
    /// import them into the store, enabled where they were found, and
    /// replace each folder whose content equals the store copy with a
    /// link (the folder moves to backups/trash). Folders with the same
    /// content in several clients become one skill. Without NAME, --all,
    /// --pick or --both, lists what can be adopted.
    Adopt {
        /// Folder names to adopt (each with one content).
        names: Vec<String>,
        /// Adopt every name that has one content.
        #[arg(long)]
        all: bool,
        /// Only this client's skill folder [default: every client].
        #[arg(long, value_name = "CLIENT")]
        tool: Option<String>,
        /// The skill folders of this project (trusted) instead of the user
        /// folders; adopted skills go to its manifest's [skills] enable.
        #[arg(long, value_name = "PATH")]
        project: Option<PathBuf>,
        /// For a name with several contents: adopt the content in CLIENT's
        /// folder (or `store`, the store's copy) as NAME; folders with
        /// other content are kept (repeatable).
        #[arg(long, value_name = "NAME=CLIENT")]
        pick: Vec<String>,
        /// Adopt CLIENT's content (or `store`) as NAME and every other
        /// content as NAME-<client> (repeatable).
        #[arg(long, value_name = "NAME=CLIENT")]
        both: Vec<String>,
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

/// The skills environment of `ctx`, with junctions on Windows.
pub(super) fn skills_env(ctx: &Context) -> skills::SkillsEnv<'_> {
    skills::SkillsEnv {
        env: &ctx.env,
        fs: ctx.fs(),
        home: &ctx.home,
        registry: ctx.apply_env().registry,
        runner: &RUNNER,
        link_kind: LinkKind::Junction,
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
            allow_dangerous,
            external,
            no_external,
            trust_source,
        } => add(
            ctx,
            &sx,
            AddArgs {
                source,
                path,
                git_ref,
                id,
                allow_dangerous: *allow_dangerous,
                external: scanner::choice(*external, *no_external),
                trust_source: *trust_source,
            },
        ),
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
        SkillsCommand::Update {
            id,
            check,
            allow_dangerous,
            external,
            no_external,
            trust_source,
            ..
        } => update(
            ctx,
            &sx,
            id.as_deref(),
            *check,
            *allow_dangerous,
            scanner::choice(*external, *no_external),
            *trust_source,
        ),
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
        SkillsCommand::Adopt {
            names,
            all,
            tool,
            project,
            pick,
            both,
        } => {
            let request = AdoptRequest {
                names,
                all: *all,
                tool: tool.as_deref(),
                pick,
                both,
            };
            adopt_folders(ctx, &sx, project.as_deref(), &request)
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

/// `skills add`'s arguments.
struct AddArgs<'a> {
    source: &'a str,
    path: &'a Option<String>,
    git_ref: &'a Option<String>,
    id: &'a Option<String>,
    allow_dangerous: bool,
    external: Choice,
    trust_source: Option<TrustSourceArg>,
}

fn add(ctx: &Context, sx: &skills::SkillsEnv<'_>, args: AddArgs<'_>) -> Result<u8, CoreError> {
    let AddArgs {
        source,
        path,
        git_ref,
        id,
        allow_dangerous,
        external,
        trust_source,
    } = args;
    let text = source;
    let mut request = if text.trim_start().starts_with(catalog::REF_PREFIX) {
        let store = ctx.open_store()?;
        let http = super::catalog::http(ctx);
        let cx = catalog::CatalogEnv { sx, http: &http };
        let read = store.as_ref().map(|s| s as &dyn StoreRead);
        let (request, entry) =
            catalog::skill_request(&cx, read, text.trim(), git_ref.as_deref(), path.as_deref())?;
        if !ctx.json {
            eprintln!(
                "note: {} from the catalog {}; its text is the publisher's, not Loadout's.",
                super::catalog::plain(&entry.reference(), 300),
                super::catalog::plain(&entry.catalog, 100)
            );
            if let Some(warning) = &entry.warning {
                eprintln!("warning: {}", super::catalog::plain(warning, 300));
            }
        }
        request
    } else if let Some(mapped) = catalog::skills_sh_request(text) {
        let mut request = mapped?;
        request.path.clone_from(path);
        request.git_ref.clone_from(git_ref);
        request
    } else {
        let mut source = Source::parse(text, sx.fs);
        if let Source::Local(local) = &source {
            source = Source::parse(&absolute(local)?.to_string_lossy(), sx.fs);
        }
        if matches!(source, Source::Local(_)) && (path.is_some() || git_ref.is_some()) {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                "--path and --ref apply to git sources only",
            ));
        }
        let mut request = InstallRequest::new(source);
        request.path.clone_from(path);
        request.git_ref.clone_from(git_ref);
        request
    };
    let store = ctx.open_store_for_change()?;
    request.naming = naming(ctx, id);
    let mut answers = Answers::default();
    let asked = match &request.source {
        Source::Git { url, .. } => {
            let asked = SourceRef::from_git_url(url)?;
            let answer = answers.decide(ctx, Some(&store), &asked, trust_source)?;
            if answer == Answer::Cancelled {
                return Err(super::source::cancelled(&asked));
            }
            Some((asked, answer))
        }
        Source::Local(_) => None,
    };
    let (trust, via) = asked
        .as_ref()
        .map_or((None, Default::default()), |(_, answer)| answer.grant());
    // A blocked source is refused before it is contacted.
    if let Some((asked, _)) = &asked {
        sources::would_admit(&store, asked, trust)?;
    }
    request.decisions = Decisions {
        allow_dangerous,
        trust,
        via,
        ..Decisions::default()
    };
    let mut prepared = install::prepare(sx, &request)?;
    // The host redirected a moved repository: the gate asks about where
    // the content really comes from too. --trust-source answers only for
    // the source named on the command line.
    if let (Some((asked, _)), Some(target)) = (&asked, prepared.moved_to()) {
        if !ctx.json {
            eprintln!("note: {} moved to {}", asked.key(), target.key());
        }
        let answer = answers.decide(ctx, Some(&store), target, None)?;
        if answer == Answer::Cancelled {
            return Err(super::source::cancelled(target));
        }
        request.decisions.moved_trust = answer.grant().0;
    }
    // A moved repository's trust is settled first: a blocked or declined
    // source is never scanned.
    if let Some(target) = prepared.moved_to() {
        sources::would_admit(&store, target, request.decisions.moved_trust)?;
    }
    // The optional external scanner: one run on exactly the content that
    // would be installed (local folders and archives too), after the
    // built-in scan.
    if let Some(runner) = scanner::wanted(ctx, external)? {
        let report = scanner::report_for(ctx, &runner, prepared.files(), prepared.name());
        prepared.set_external(Some(report));
    }
    match install::install_prepared(
        &store,
        sx,
        &mut prepared,
        &request.naming,
        &request.decisions,
    ) {
        Ok(installed) => installed_out(ctx, &installed),
        Err(err) if err.code() == ErrorCode::ScanBlocked => {
            blocked_out(ctx, &err, prepared.scan(), prepared.external());
            Ok(EXIT_SCAN_BLOCKED)
        }
        Err(err) => Err(err),
    }
}

/// A changed file, for `--json` (`script`: looks executable, #24).
fn change_json(change: &FileChange) -> Value {
    json!({
        "path": change.path,
        "kind": change.kind.as_str(),
        "script": change.script,
    })
}

/// The trust check of a git skill's source, for `--json`.
fn trust_json(check: &TrustCheck) -> Value {
    let (state, reason) = match &check.state {
        TrustState::Trusted { .. } => ("trusted", None),
        TrustState::Blocked { .. } => ("blocked", None),
        TrustState::Ask { reason, .. } => ("ask", Some(reason.as_str())),
    };
    json!({
        "source": check.source.key(),
        "state": state,
        "reason": reason,
        "official": check.official,
        "insecure": check.source.is_insecure(),
    })
}

fn source_change_json(change: &SourceChange) -> Value {
    json!({ "kind": change.kind.as_str(), "detail": change.detail })
}

/// The trust answers an available update needs, for its source and for
/// the repository it moved to: nothing for a trusted source (and a blocked
/// one, which the gate refuses), else the flag or the prompt, asked once
/// per source in a run. The flag answers only for the skill's own source,
/// never for a repository it moved to.
fn update_answer(
    ctx: &Context,
    store: &Store<'_>,
    answers: &mut Answers,
    checked: &UpdateCheck,
    flag: Option<TrustSourceArg>,
) -> Result<Decisions, CoreError> {
    let Some(trust) = &checked.trust else {
        return Ok(Decisions::default());
    };
    if let Some(change) = &checked.source_change
        && !ctx.json
    {
        let what = match &change.detail {
            Some(detail) => format!("moved to {detail}"),
            None => "its history was rewritten (force-push)".to_owned(),
        };
        eprintln!("note: {} {what}; its trust lapsed", trust.source.key());
    }
    let answer = answers.decide(ctx, Some(store), &trust.source, flag)?;
    if answer == Answer::Cancelled {
        return Err(super::source::cancelled(&trust.source));
    }
    let (trust, via) = answer.grant();
    let mut decisions = Decisions {
        trust,
        via,
        ..Decisions::default()
    };
    if let Some(moved) = &checked.moved_trust {
        let answer = answers.decide(ctx, Some(store), &moved.source, None)?;
        if answer == Answer::Cancelled {
            return Err(super::source::cancelled(&moved.source));
        }
        decisions.moved_trust = answer.grant().0;
    }
    Ok(decisions)
}

/// The scan of an install or update, for `--json`.
pub(super) fn scan_json(report: &ScanReport) -> Value {
    json!({
        "verdict": report.verdict.as_str(),
        "rules_version": report.rules_version,
        "custom_rules": report.custom_rules,
        "cached": report.cached,
        "warnings": report.warnings,
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

/// The findings of a scan, one per line, after the rule-loading warnings
/// (on stderr) and a note when custom rules are in use; nothing for a
/// clean scan with the built-in rules.
pub(super) fn print_scan(report: &ScanReport, indent: &str) {
    for warning in &report.warnings {
        eprintln!("warning[scan_rules]: {warning}");
    }
    if report.custom_rules {
        println!("{indent}Scan: custom rules in use (state/scan-rules.toml)");
    }
    if report.findings.is_empty() {
        return;
    }
    let count = report.findings.len();
    let plural = if count == 1 { "" } else { "s" };
    println!(
        "{indent}Scan: {} ({count} finding{plural})",
        report.verdict.as_str()
    );
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
}

/// An install a scan blocked: the findings, then the error.
fn blocked_out(
    ctx: &Context,
    err: &CoreError,
    report: &ScanReport,
    external: Option<&ExternalReport>,
) {
    if ctx.json {
        let mut doc = json!({
            "schema": JSON_SCHEMA,
            "error": error_json(err),
            "scan": scan_json(report),
        });
        if let Some(external) = external {
            doc["external"] = scanner::external_json(external);
        }
        print_json(&doc);
    } else {
        print_scan(report, "");
        if let Some(external) = external {
            scanner::print_external(external, "");
        }
        eprintln!("error[{}]: {err}", err.code());
        eprintln!(
            "Nothing was installed. Review the findings; to install anyway, run again with --allow-dangerous."
        );
    }
}

fn installed_out(ctx: &Context, installed: &skills::Installed) -> Result<u8, CoreError> {
    for warning in &installed.warnings {
        eprintln!("warning[{}]: {}", warning.code(), warning.message());
    }
    let code = installed.report.as_ref().map_or(0, exit_for);
    if ctx.json {
        let mut doc = json!({
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
            "scan": scan_json(&installed.scan),
        });
        if let Some(external) = &installed.external {
            doc["external"] = scanner::external_json(external);
        }
        print_json(&doc);
    } else {
        let renamed = installed
            .renamed_from
            .as_ref()
            .map(|wanted| format!(" ({wanted} was taken)"))
            .unwrap_or_default();
        println!("Added skill {}{renamed}.", installed.id);
        print_scan(&installed.scan, "");
        if let Some(external) = &installed.external {
            scanner::print_external(external, "");
        }
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

pub(super) fn exit_for(report: &SyncReport) -> u8 {
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

pub(super) fn report_json(report: &SyncReport) -> Value {
    json!({
        "results": report.results.iter().map(result_json).collect::<Vec<_>>(),
        "warnings": report.warnings.iter().map(error_json).collect::<Vec<_>>(),
    })
}

pub(super) fn print_links(report: &SyncReport) {
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
    // The Loadout skill is marked by `[bundled]` alone (no `[source]`).
    let bundled = meta.and_then(|meta| meta.bundled.as_ref());
    let enable = meta.and_then(|meta| meta.enable.as_ref());
    json!({
        "id": skill.id,
        "name": skill.name(),
        "description": skill.description(),
        "frontmatter": skill.frontmatter.as_ref().map(|fm| fm.to_json()),
        "bytes": skill.bytes,
        "tags": meta.and_then(|meta| meta.tags.clone()),
        "source": match (source, bundled) {
            (_, Some(bundled)) => json!({
                "kind": "bundled",
                "url": null,
                "path": null,
                "ref": null,
                "commit": null,
                "version": bundled.version,
            }),
            (Some(source), None) => json!({
                "kind": source_kind(source.kind),
                "url": source.url,
                "path": source.path,
                "ref": source.git_ref,
                "commit": source.commit,
            }),
            (None, None) => Value::Null,
        },
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

/// The trust checks of an available update: its source's and the one of
/// the repository it moved to.
fn trust_checks(checked: &UpdateCheck) -> impl Iterator<Item = &TrustCheck> {
    checked.trust.iter().chain(checked.moved_trust.iter())
}

/// Every source of the update is trusted already (nothing to ask).
fn admitted_without_asking(checked: &UpdateCheck) -> bool {
    trust_checks(checked).all(|check| matches!(check.state, TrustState::Trusted { .. }))
}

/// A source of the update is blocked (the update's gate refuses it).
fn source_blocked(checked: &UpdateCheck) -> bool {
    trust_checks(checked).any(|check| matches!(check.state, TrustState::Blocked { .. }))
}

/// The external report of an available update's content (the cache
/// clone as the check left it), when this run uses the scanner; the run
/// is prepared (SkillSpector found) once, on the first update that needs
/// it.
fn external_for_update(
    ctx: &Context,
    store: &Store<'_>,
    sx: &skills::SkillsEnv<'_>,
    id: &str,
    choice: Choice,
    runner: &mut Option<Option<Result<Runner, ExternalReport>>>,
) -> Result<Option<ExternalReport>, CoreError> {
    if runner.is_none() {
        *runner = Some(scanner::wanted(ctx, choice)?);
    }
    let Some(Some(runner)) = runner.as_ref() else {
        return Ok(None);
    };
    let Some(files) = install::cached_latest(store, sx, id)? else {
        return Ok(None);
    };
    Ok(Some(scanner::report_for(ctx, runner, &files, id)))
}

/// One skill's result: its JSON, and the scans the human output prints.
type UpdateResult = (Value, Option<ScanReport>, Option<ExternalReport>);

fn with_external(mut result: Value, external: Option<&ExternalReport>) -> Value {
    if let Some(external) = external {
        result["external"] = scanner::external_json(external);
    }
    result
}

fn update(
    ctx: &Context,
    sx: &skills::SkillsEnv<'_>,
    id: Option<&str>,
    check: bool,
    allow_dangerous: bool,
    external: Choice,
    trust_source: Option<TrustSourceArg>,
) -> Result<u8, CoreError> {
    let store: Store<'_> = require_store(ctx, "there is no skill to update")?;
    let ids: Vec<String> = match id {
        Some(id) => vec![id.to_owned()],
        None => store::ids(&store)?,
    };
    let mut answers = Answers::default();
    let mut runner = None;
    let mut results: Vec<UpdateResult> = Vec::new();
    let mut failed = false;
    let mut blocked = false;
    let mut attention = false;
    for id in &ids {
        let checked = install::check_update(&store, sx, id)?;
        // `--check`: one external run per available update of a trusted
        // source, on the content the check fetched (an untrusted or
        // blocked source is never scanned before the user trusts it). An
        // update runs it after the trust answer, below.
        let external_report = if check
            && checked.status == UpdateStatus::Available
            && admitted_without_asking(&checked)
        {
            external_for_update(ctx, &store, sx, id, external, &mut runner)?
        } else {
            None
        };
        if check {
            failed |= checked.status == UpdateStatus::Unreachable;
            results.push((
                with_external(
                    json!({
                        "id": checked.id,
                        "status": checked.status.as_str(),
                        "current": checked.current,
                        "latest": checked.latest,
                        "changes": checked.changes.iter().map(change_json).collect::<Vec<_>>(),
                        "error": checked.error.as_ref().map(error_json),
                        "scan": checked.scan.as_ref().map(scan_json),
                        "trust": checked.trust.as_ref().map(trust_json),
                        "source_change": checked.source_change.as_ref().map(source_change_json),
                        "moved_trust": checked.moved_trust.as_ref().map(trust_json),
                    }),
                    external_report.as_ref(),
                ),
                checked.scan,
                external_report,
            ));
            continue;
        }
        match checked.status {
            UpdateStatus::NotTracked | UpdateStatus::UpToDate => {
                results.push((
                    json!({ "id": id, "status": checked.status.as_str() }),
                    None,
                    None,
                ));
            }
            UpdateStatus::Unreachable | UpdateStatus::Blocked => {
                failed = true;
                results.push((
                    json!({
                        "id": id,
                        "status": checked.status.as_str(),
                        "error": checked.error.as_ref().map(error_json),
                    }),
                    None,
                    None,
                ));
            }
            // The content the check fetched and scanned is the content
            // written: no second fetch between the scans and the update.
            // The external run comes after the trust answer.
            UpdateStatus::Available => {
                let mut external_report = external_report;
                let outcome = update_answer(ctx, &store, &mut answers, &checked, trust_source)
                    .and_then(|answered| {
                        // A blocked source is refused by the update's
                        // gate: never scanned first.
                        if !source_blocked(&checked) {
                            external_report =
                                external_for_update(ctx, &store, sx, id, external, &mut runner)?;
                        }
                        let decisions = Decisions {
                            allow_dangerous,
                            external: external_report.clone(),
                            ..answered
                        };
                        install::update_reviewed(
                            &store,
                            sx,
                            id,
                            checked.latest.as_deref().unwrap_or_default(),
                            &decisions,
                        )
                    });
                match outcome {
                    Ok(updated) => {
                        attention |= updated.report.needs_attention();
                        results.push((
                            with_external(
                                json!({
                                    "id": id,
                                    "status": "updated",
                                    "from": updated.from,
                                    "to": updated.to,
                                    "commit": updated.commit.as_ref().map(CommitId::as_str),
                                    "changes": updated.changes.iter().map(change_json).collect::<Vec<_>>(),
                                    "links": report_json(&updated.report),
                                    "scan": scan_json(&updated.scan),
                                }),
                                updated.external.as_ref(),
                            ),
                            Some(updated.scan),
                            updated.external,
                        ));
                    }
                    Err(err) if err.code() == ErrorCode::ScanBlocked => {
                        blocked = true;
                        results.push((
                            with_external(
                                json!({
                                    "id": id,
                                    "status": "blocked",
                                    "latest": checked.latest,
                                    "error": error_json(&err),
                                    "scan": checked.scan.as_ref().map(scan_json),
                                }),
                                external_report.as_ref(),
                            ),
                            checked.scan,
                            external_report,
                        ));
                    }
                    Err(err) => {
                        failed = true;
                        results.push((
                            json!({ "id": id, "status": "failed", "error": error_json(&err) }),
                            None,
                            None,
                        ));
                    }
                }
            }
        }
    }
    if ctx.json {
        let skills: Vec<&Value> = results.iter().map(|(result, _, _)| result).collect();
        print_json(&json!({ "schema": JSON_SCHEMA, "skills": skills }));
    } else {
        for (result, scan, external) in &results {
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
            if let Some(scan) = scan {
                print_scan(scan, "  ");
            }
            if let Some(external) = external {
                scanner::print_external(external, "  ");
            }
        }
        if blocked {
            eprintln!(
                "A dangerous update was not applied. Review the findings; to apply it anyway, run again with --allow-dangerous."
            );
        }
    }
    // Each skill is reported; a source that cannot be reached (or a failed
    // update) fails the run at the end, then an update the scan blocked.
    if failed {
        return Ok(1);
    }
    if blocked {
        return Ok(EXIT_SCAN_BLOCKED);
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

/// What `skills adopt` was asked.
struct AdoptRequest<'a> {
    names: &'a [String],
    all: bool,
    tool: Option<&'a str>,
    pick: &'a [String],
    both: &'a [String],
}

impl AdoptRequest<'_> {
    fn lists_only(&self) -> bool {
        self.names.is_empty() && !self.all && self.pick.is_empty() && self.both.is_empty()
    }
}

/// The content of `name` that `which` names: `store` for the store's copy,
/// else the content in that client's folder.
fn variant_of(scan: &adopt::UnmanagedScan, name: &str, which: &str) -> Result<String, CoreError> {
    let candidate = scan.candidate(name).ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotFound,
            format!("there is no unmanaged skill folder {name:?}"),
        )
    })?;
    candidate
        .variants
        .iter()
        .find(|variant| {
            if which == "store" {
                variant.in_store
            } else {
                variant.clients().any(|client| client == which)
            }
        })
        .map(|variant| variant.hash.clone())
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidValue,
                format!("{name:?} has no content in {which:?}; see `skills adopt` for the list"),
            )
        })
}

fn choices(
    scan: &adopt::UnmanagedScan,
    request: &AdoptRequest<'_>,
) -> Result<Vec<adopt::AdoptChoice>, CoreError> {
    let mut out: Vec<adopt::AdoptChoice> = if request.all {
        adopt::default_choices(scan)
    } else {
        Vec::new()
    };
    let mut set = |name: &str, action: adopt::AdoptAction| {
        out.retain(|choice| choice.name != name);
        out.push(adopt::AdoptChoice {
            name: name.to_owned(),
            action,
        });
    };
    for name in request.names {
        let candidate = scan.candidate(name).ok_or_else(|| {
            CoreError::new(
                ErrorCode::NotFound,
                format!("there is no unmanaged skill folder {name:?}"),
            )
        })?;
        if candidate.is_conflict() {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!(
                    "{name:?} has several contents; choose with --pick {name}=<client> or --both \
                     {name}=<client>"
                ),
            ));
        }
        set(
            name,
            adopt::AdoptAction::Pick {
                variant: candidate.variants[0].hash.clone(),
            },
        );
    }
    for (list, both) in [(request.pick, false), (request.both, true)] {
        for spec in list {
            let (name, which) = spec.split_once('=').ok_or_else(|| {
                CoreError::new(
                    ErrorCode::InvalidValue,
                    format!("{spec:?} is not NAME=CLIENT (or NAME=store)"),
                )
            })?;
            let variant = variant_of(scan, name, which)?;
            let action = if both {
                adopt::AdoptAction::Both { primary: variant }
            } else {
                adopt::AdoptAction::Pick { variant }
            };
            set(name, action);
        }
    }
    Ok(out)
}

fn problem_json(problem: &adopt::FolderProblem) -> Value {
    json!({
        "client": problem.client,
        "name": problem.name,
        "path": path_json(&problem.path),
        "error": error_json(&problem.error),
    })
}

fn adopt_folders(
    ctx: &Context,
    sx: &skills::SkillsEnv<'_>,
    project: Option<&Path>,
    request: &AdoptRequest<'_>,
) -> Result<u8, CoreError> {
    let (scope, project) = scope_of(project)?;
    if request.lists_only() {
        let store = ctx.open_store()?;
        let scan = adopt::scan(store.as_ref(), sx, scope, project.as_deref(), request.tool)?;
        return adopt_list(ctx, &scan);
    }
    let store = ctx.open_store_for_change()?;
    let scan = adopt::scan(Some(&store), sx, scope, project.as_deref(), request.tool)?;
    let choices = choices(&scan, request)?;
    let report = adopt::adopt(
        &store,
        sx,
        scope,
        project.as_deref(),
        request.tool,
        &choices,
    )?;
    let code = if report.needs_attention() {
        EXIT_UNRESOLVED
    } else {
        0
    };
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "commit": report.commit.as_ref().map(CommitId::as_str),
            "added": report.added,
            "reused": report.reused,
            "folders": report.folders.iter().map(|folder| json!({
                "client": folder.client,
                "name": folder.name,
                "path": path_json(&folder.path),
                "skill": folder.skill_id,
                "status": folder.status.as_str(),
                "trash": folder.trash.as_deref().map(path_json),
                "error": folder.error.as_ref().map(error_json),
            })).collect::<Vec<_>>(),
            "problems": report.problems.iter().map(problem_json).collect::<Vec<_>>(),
            "links": report_json(&report.sync),
        }));
        return Ok(code);
    }
    if !report.added.is_empty() {
        println!("Added to the store: {}.", report.added.join(", "));
    }
    if !report.reused.is_empty() {
        println!("Already in the store: {}.", report.reused.join(", "));
    }
    for folder in &report.folders {
        let detail = match (&folder.error, &folder.skill_id) {
            (Some(err), _) => format!(" [{}] {}", err.code(), err.message()),
            (None, Some(id)) if *id != folder.name => format!(" as {id}"),
            _ => String::new(),
        };
        println!(
            "  {:<9} {:<12} {}{detail}",
            folder.status.as_str(),
            folder.client,
            folder.path.display()
        );
    }
    for problem in &report.problems {
        eprintln!(
            "warning[{}]: {}: {}",
            problem.error.code(),
            problem.path.display(),
            problem.error.message()
        );
    }
    print_links(&report.sync);
    if report.folders.is_empty() {
        println!("Nothing to adopt.");
    }
    Ok(code)
}

fn adopt_list(ctx: &Context, scan: &adopt::UnmanagedScan) -> Result<u8, CoreError> {
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "scope": scope_str(scan.scope),
            "candidates": scan.candidates.iter().map(|candidate| json!({
                "name": candidate.name,
                "conflict": candidate.is_conflict(),
                "variants": candidate.variants.iter().map(|variant| json!({
                    "hash": variant.hash,
                    "clients": variant.clients().collect::<Vec<_>>(),
                    "folders": variant.folders.iter().map(|folder| path_json(&folder.path)).collect::<Vec<_>>(),
                    "in_store": variant.in_store,
                    "description": variant.description,
                    "bytes": variant.bytes,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "problems": scan.problems.iter().map(problem_json).collect::<Vec<_>>(),
        }));
        return Ok(0);
    }
    if scan.candidates.is_empty() {
        println!("No unmanaged skill folders.");
    }
    for candidate in &scan.candidates {
        let state = if candidate.is_conflict() {
            "conflict"
        } else {
            "ready"
        };
        println!("{:<32} {state}", candidate.name);
        for variant in &candidate.variants {
            let mut owners: Vec<&str> = variant.clients().collect();
            if variant.in_store {
                owners.push("store");
            }
            println!(
                "  {}  {}",
                &variant.hash[..variant.hash.len().min(19)],
                owners.join(", ")
            );
        }
    }
    for problem in &scan.problems {
        eprintln!(
            "warning[{}]: {}: {}",
            problem.error.code(),
            problem.path.display(),
            problem.error.message()
        );
    }
    if scan.candidates.iter().any(adopt::Candidate::is_conflict) {
        println!(
            "Choose for a conflict with --pick NAME=<client|store> or --both NAME=<client|store>."
        );
    }
    Ok(0)
}

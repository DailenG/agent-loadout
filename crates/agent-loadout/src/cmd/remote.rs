//! `loadout remote add folder|git`, `ls`, `rm`, `push`, `pull`, `machines`,
//! `token` and `create-github` (SPEC 9.6 to 9.9, 14). Remotes are
//! machine-local (`state/remotes.toml`); none exists until one is added.
//!
//! Exit codes: a remote that cannot be reached is an error for `push` and
//! `pull` themselves (1), never for local work; `pull` exits 4 when fields
//! conflict (`state/merge/MERGE_STATE.json` lists them).

use std::io::IsTerminal;
use std::path::Path;

use agent_loadout_core::remote::config::{
    FolderMode, RemoteConfig, RemoteKind, RemotesFile, check_name,
};
use agent_loadout_core::remote::credential::token_account;
use agent_loadout_core::remote::folder::{
    self, MachineEntry, PullReport, PushOutcome, SiblingPull,
};
use agent_loadout_core::remote::gh;
use agent_loadout_core::remote::git::{self, GitAuth, GitCli, GitPush, SystemRunner};
use agent_loadout_core::remote::sync_artifacts::Artifact;
use agent_loadout_core::remote::unc::SystemUnc;
use agent_loadout_core::store::Store;
use agent_loadout_core::store::diff::{ChangeKind, EntityChange};
use agent_loadout_core::store::merge::MergeOutcome;
use agent_loadout_core::{CoreError, ErrorCode, brand};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use super::{
    Context, EXIT_UNRESOLVED, JSON_SCHEMA, error_json, path_json, print_json, require_store,
};

static UNC: SystemUnc = SystemUnc;
static RUNNER: SystemRunner = SystemRunner {
    idle_timeout: git::IDLE_TIMEOUT,
};

#[derive(Debug, Args)]
pub struct RemoteArgs {
    #[command(subcommand)]
    command: RemoteCommand,
}

#[derive(Debug, Subcommand)]
enum RemoteCommand {
    /// Add a remote.
    Add {
        #[command(subcommand)]
        kind: AddKind,
    },
    /// List this machine's remotes.
    Ls,
    /// Remove a remote (its files stay where they are; a stored token is
    /// deleted).
    Rm { name: String },
    /// Push the store to one remote [default: every remote].
    Push { name: Option<String> },
    /// Pull and merge other machines' changes [default: every remote].
    Pull { name: Option<String> },
    /// List the machines a folder remote holds.
    Machines { name: String },
    /// Store the token piped on stdin for a git remote with `auth = token`.
    Token {
        name: String,
        /// Delete the stored token instead.
        #[arg(long)]
        clear: bool,
    },
    /// Create a private GitHub repository with the GitHub CLI and add it as
    /// a git remote (`auth = gh`).
    CreateGithub {
        /// Repository name: NAME or OWNER/NAME.
        repo: String,
        /// Name of the remote to add.
        #[arg(long, default_value = "github")]
        name: String,
    },
}

#[derive(Debug, Subcommand)]
enum AddKind {
    /// A local or network folder (USB drive, NAS share, synced folder).
    Folder {
        name: String,
        path: std::path::PathBuf,
        /// repo-per-machine (default), mirror or archive.
        #[arg(long, default_value = "repo-per-machine")]
        mode: String,
        /// Archive mode: archives to keep [default: 10].
        #[arg(long)]
        keep: Option<u32>,
    },
    /// Any git URL (https, ssh or scp-style), through the system git.
    Git {
        name: String,
        url: String,
        /// system (default: your git's own credentials), gh (GitHub CLI)
        /// or token (a token stored with `remote token`).
        #[arg(long, default_value = "system")]
        auth: String,
        /// With --auth token: the username sent with the token
        /// [default: x-access-token].
        #[arg(long)]
        username: Option<String>,
    },
}

pub fn run(ctx: &Context, args: &RemoteArgs) -> Result<u8, CoreError> {
    match &args.command {
        RemoteCommand::Add {
            kind:
                AddKind::Folder {
                    name,
                    path,
                    mode,
                    keep,
                },
        } => add_folder(ctx, name, path, mode, *keep).map(|()| 0),
        RemoteCommand::Add {
            kind:
                AddKind::Git {
                    name,
                    url,
                    auth,
                    username,
                },
        } => add_git(ctx, name, url, auth, username.as_deref()).map(|()| 0),
        RemoteCommand::Ls => list(ctx).map(|()| 0),
        RemoteCommand::Rm { name } => remove(ctx, name).map(|()| 0),
        RemoteCommand::Push { name } => push(ctx, name.as_deref()),
        RemoteCommand::Pull { name } => pull(ctx, name.as_deref()),
        RemoteCommand::Machines { name } => machines(ctx, name).map(|()| 0),
        RemoteCommand::Token { name, clear } => token(ctx, name, *clear).map(|()| 0),
        RemoteCommand::CreateGithub { repo, name } => create_github(ctx, repo, name).map(|()| 0),
    }
}

fn remotes(ctx: &Context) -> Result<RemotesFile, CoreError> {
    RemotesFile::load(ctx.fs(), &ctx.home.state_dir())
}

fn add_folder(
    ctx: &Context,
    name: &str,
    path: &Path,
    mode: &str,
    keep: Option<u32>,
) -> Result<(), CoreError> {
    let mode = FolderMode::parse(mode)?;
    let path = std::path::absolute(path).map_err(|err| {
        CoreError::new(ErrorCode::InvalidPath, format!("invalid path: {err}")).with_path(path)
    })?;
    if path.starts_with(&ctx.home.path) {
        return Err(CoreError::new(
            ErrorCode::InvalidPath,
            format!("a remote cannot live inside {}", brand::HOME_ENV_VAR),
        )
        .with_path(&path));
    }
    let mut remote = RemoteConfig::folder(name, &path.to_string_lossy(), mode);
    remote.keep = keep;
    let mut file = remotes(ctx)?;
    file.add(remote)?;
    file.save(ctx.fs(), &ctx.home.state_dir())?;
    let reachable = ctx.fs().is_dir(&path);
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "added": name,
            "path": path_json(&path),
            "mode": mode.as_str(),
            "reachable": reachable,
        }));
    } else {
        println!(
            "Added folder remote {name} at {} ({}).",
            path.display(),
            mode.as_str()
        );
        if !reachable {
            eprintln!(
                "warning[remote_unreachable]: {} is not there now; pushes try again later",
                path.display()
            );
        }
    }
    Ok(())
}

fn add_git(
    ctx: &Context,
    name: &str,
    url: &str,
    auth: &str,
    username: Option<&str>,
) -> Result<(), CoreError> {
    let auth = GitAuth::parse(Some(auth))?;
    if url.trim().is_empty() || url.starts_with('-') {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("invalid git URL {url:?}"),
        ));
    }
    if auth == GitAuth::Gh && !git::is_github_url(url) {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            "auth gh is for github.com URLs; use system or token for other hosts",
        ));
    }
    let mut remote = RemoteConfig::git(name, url, auth);
    remote.username = username.map(str::to_owned);
    let mut file = remotes(ctx)?;
    file.add(remote)?;
    file.save(ctx.fs(), &ctx.home.state_dir())?;
    let mut warnings = Vec::new();
    if git::locate(&ctx.env, ctx.fs()).is_none() {
        warnings.push(format!("git is not on PATH; pushes to {name} need it"));
    }
    if auth == GitAuth::Gh && gh::locate(&ctx.env, ctx.fs()).is_none() {
        warnings.push("the GitHub CLI (gh) is not on PATH".to_owned());
    }
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "added": name,
            "url": url,
            "auth": auth.as_str(),
            "warnings": warnings
                .iter()
                .map(|message| json!({ "code": "tool_not_found", "message": message }))
                .collect::<Vec<_>>(),
        }));
        return Ok(());
    }
    println!("Added git remote {name} at {url} (auth {}).", auth.as_str());
    for message in &warnings {
        eprintln!("warning[tool_not_found]: {message}");
    }
    if auth == GitAuth::Token {
        println!(
            "Next: pipe the token into `{} remote token {name}`.",
            brand::BIN_NAME
        );
    }
    Ok(())
}

fn list(ctx: &Context) -> Result<(), CoreError> {
    let file = remotes(ctx)?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "remotes": file.remotes.iter().map(remote_json).collect::<Vec<_>>(),
        }));
        return Ok(());
    }
    if file.remotes.is_empty() {
        println!("No backup remote.");
    }
    for remote in &file.remotes {
        let detail = match remote.kind {
            RemoteKind::Folder => remote.folder_mode().as_str().to_owned(),
            RemoteKind::Git => format!("auth {}", remote.auth.as_deref().unwrap_or("system")),
        };
        println!(
            "{:<20} {:<7} {:<17} {}",
            remote.name,
            remote.kind.as_str(),
            detail,
            remote
                .path
                .as_deref()
                .or(remote.url.as_deref())
                .unwrap_or_default()
        );
    }
    Ok(())
}

fn remote_json(remote: &RemoteConfig) -> Value {
    json!({
        "name": remote.name,
        "kind": remote.kind.as_str(),
        "path": remote.path,
        "url": remote.url,
        "mode": remote.mode.map(FolderMode::as_str),
        "auth": remote.auth,
        "username": remote.username,
        "push": remote.push,
        "pull": remote.pull,
    })
}

fn remove(ctx: &Context, name: &str) -> Result<(), CoreError> {
    let mut file = remotes(ctx)?;
    let removed = file.remove(name)?;
    file.save(ctx.fs(), &ctx.home.state_dir())?;
    let token_deleted = removed.kind == RemoteKind::Git
        && ctx
            .keyring()
            .delete(brand::KEYRING_SERVICE, &token_account(name))
            .unwrap_or(false);
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "removed": name,
            "token_deleted": token_deleted,
        }));
    } else {
        println!("Removed remote {name}; its files were left in place.");
        if token_deleted {
            println!("Deleted its token from the keyring.");
        }
    }
    Ok(())
}

fn git_remote<'a>(file: &'a RemotesFile, name: &str) -> Result<&'a RemoteConfig, CoreError> {
    let remote = file
        .get(name)
        .ok_or_else(|| CoreError::new(ErrorCode::NotFound, format!("no remote named {name:?}")))?;
    if remote.kind != RemoteKind::Git {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{name} is not a git remote"),
        ));
    }
    Ok(remote)
}

fn token(ctx: &Context, name: &str, clear: bool) -> Result<(), CoreError> {
    let file = remotes(ctx)?;
    let remote = git_remote(&file, name)?;
    let account = token_account(name);
    if clear {
        let deleted = ctx
            .keyring()
            .delete(brand::KEYRING_SERVICE, &account)
            .map_err(|resolution| resolution.into_error(&account))?;
        if ctx.json {
            print_json(&json!({ "schema": JSON_SCHEMA, "remote": name, "deleted": deleted }));
        } else if deleted {
            println!("Deleted the token of {name}.");
        } else {
            println!("{name} had no token stored.");
        }
        return Ok(());
    }
    if GitAuth::parse(remote.auth.as_deref())? != GitAuth::Token {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{name} does not use auth token"),
        ));
    }
    let value = super::secrets::read_value()?;
    ctx.keyring()
        .set(brand::KEYRING_SERVICE, &account, &value)
        .map_err(|resolution| resolution.into_error(&account))?;
    if ctx.json {
        print_json(&json!({ "schema": JSON_SCHEMA, "remote": name, "stored": true }));
    } else {
        println!("Stored the token of {name} in {}.", ctx.keyring().name());
    }
    Ok(())
}

fn create_github(ctx: &Context, repo: &str, name: &str) -> Result<(), CoreError> {
    // Checked before gh creates anything, so a bad name leaves no
    // repository behind.
    check_name(name)?;
    let mut file = remotes(ctx)?;
    if file.get(name).is_some() {
        return Err(CoreError::new(
            ErrorCode::AlreadyExists,
            format!("a remote named {name:?} exists already"),
        ));
    }
    let gh_path = gh::locate(&ctx.env, ctx.fs()).ok_or_else(|| {
        CoreError::new(
            ErrorCode::ToolNotFound,
            "the GitHub CLI (gh) is not on PATH; create the repository on any host and add it \
             with `remote add git`",
        )
    })?;
    let url = gh::create_private_repo(&ctx.env, &gh_path, &RUNNER, repo)?;
    file.add(RemoteConfig::git(name, &url, GitAuth::Gh))?;
    file.save(ctx.fs(), &ctx.home.state_dir())?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "created": url,
            "added": name,
            "auth": GitAuth::Gh.as_str(),
        }));
    } else {
        println!("Created private repository {url} and added it as git remote {name} (auth gh).");
    }
    Ok(())
}

/// What one push did.
pub enum Pushed {
    Folder(PushOutcome),
    Git(GitPush),
}

/// One push: the remote's name and how it went.
pub type PushResult = (String, Result<Pushed, CoreError>);

/// Git's progress, shown on a terminal only, on one line that ends when
/// the guard drops.
struct Progress {
    show: bool,
    shown: bool,
}

impl Progress {
    fn new() -> Self {
        Self {
            show: std::io::stderr().is_terminal(),
            shown: false,
        }
    }

    fn line(&mut self, line: &str) {
        if self.show {
            eprint!("\r{line}\x1b[K");
            self.shown = true;
        }
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        if self.shown {
            eprintln!();
        }
    }
}

fn git_cli(store: &Store<'_>) -> Result<GitCli<'static>, CoreError> {
    GitCli::from_env(
        store.env(),
        store.fs(),
        &RUNNER,
        std::env::current_exe().ok(),
    )
}

/// Pushes to the selected remotes; one result per remote.
pub fn push_to(
    store: &Store<'_>,
    file: &RemotesFile,
    name: Option<&str>,
) -> Result<Vec<PushResult>, CoreError> {
    Ok(file
        .select(name)?
        .into_iter()
        .map(|remote| {
            let result = match remote.kind {
                RemoteKind::Folder => {
                    folder::push(store, remote, &UNC, env!("CARGO_PKG_VERSION")).map(Pushed::Folder)
                }
                RemoteKind::Git => git_cli(store).and_then(|cli| {
                    let mut progress = Progress::new();
                    git::push(store, &cli, remote, &mut |line| progress.line(line)).map(Pushed::Git)
                }),
            };
            (remote.name.clone(), result)
        })
        .collect())
}

pub fn push_json(store: &Store<'_>, results: &[PushResult]) -> Value {
    Value::Array(
        results
            .iter()
            .map(|(name, result)| match result {
                Ok(Pushed::Folder(outcome)) => json!({
                    "remote": name,
                    "ok": true,
                    "kind": "folder",
                    "dir": path_json(&outcome.dir),
                    "mode": outcome.mode.as_str(),
                    "head": outcome.head.as_str(),
                    "archive": outcome.archive,
                }),
                Ok(Pushed::Git(outcome)) => json!({
                    "remote": name,
                    "ok": true,
                    "kind": "git",
                    "head": outcome.head.as_str(),
                    "pulled": outcome.pulled.as_ref().map(|report| pull_json(store, report)),
                }),
                Err(err) => json!({ "remote": name, "ok": false, "error": error_json(err) }),
            })
            .collect(),
    )
}

pub fn print_pushes(store: &Store<'_>, results: &[PushResult]) {
    for (name, result) in results {
        match result {
            Ok(Pushed::Folder(outcome)) => println!(
                "Pushed {} to {name} ({}, {}).",
                outcome.head.short(),
                outcome.mode.as_str(),
                outcome.dir.display()
            ),
            Ok(Pushed::Git(outcome)) => {
                if let Some(report) = &outcome.pulled {
                    println!("{name} had changes this machine lacked; pulled them first:");
                    print_pull(store, report);
                }
                println!("Pushed {} to {name}.", outcome.head.short());
            }
            Err(err) => eprintln!("warning[{}]: {name}: {err}", err.code()),
        }
    }
}

fn push(ctx: &Context, name: Option<&str>) -> Result<u8, CoreError> {
    let store = require_store(ctx, "nothing to push")?;
    let file = remotes(ctx)?;
    if file.remotes.is_empty() {
        return Err(no_remote());
    }
    let results = push_to(&store, &file, name)?;
    if ctx.json {
        print_json(&json!({ "schema": JSON_SCHEMA, "pushes": push_json(&store, &results) }));
    } else {
        print_pushes(&store, &results);
    }
    // A push stopped by conflicting changes on the remote is unresolved
    // (4, SPEC 14); any other failure is 1.
    let failed = results.iter().filter_map(|(_, r)| r.as_ref().err());
    Ok(
        if failed
            .clone()
            .any(|err| err.code() == ErrorCode::MergeConflict)
        {
            EXIT_UNRESOLVED
        } else if failed.count() > 0 {
            1
        } else {
            0
        },
    )
}

fn no_remote() -> CoreError {
    CoreError::new(
        ErrorCode::NotFound,
        format!(
            "no backup remote; add one with `{bin} remote add folder <NAME> <PATH>` or `{bin} \
             remote add git <NAME> <URL>`",
            bin = brand::BIN_NAME
        ),
    )
}

fn pull(ctx: &Context, name: Option<&str>) -> Result<u8, CoreError> {
    let store = require_store(ctx, "nothing to merge into")?;
    let file = remotes(ctx)?;
    if file.remotes.is_empty() {
        return Err(no_remote());
    }
    let mut reports = Vec::new();
    for remote in file.select(name)? {
        let report = match remote.kind {
            RemoteKind::Folder => folder::pull(&store, remote, &UNC),
            RemoteKind::Git => git_cli(&store).and_then(|cli| {
                let mut progress = Progress::new();
                git::pull(&store, &cli, remote, &mut |line| progress.line(line))
            }),
        };
        reports.push((remote.name.clone(), report));
    }
    let conflicts = reports
        .iter()
        .any(|(_, r)| r.as_ref().is_ok_and(|r| r.conflicts() > 0));
    // A remote that failed, or a machine whose merge failed for another
    // reason than objects still on their way (`remote_incomplete`).
    let failed = reports.iter().any(|(_, r)| match r {
        Err(_) => true,
        Ok(report) => report.siblings.iter().any(|s| {
            s.outcome
                .as_ref()
                .is_err_and(|err| err.code() != ErrorCode::RemoteIncomplete)
        }),
    });
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "pulls": reports.iter().map(|(name, result)| match result {
                Ok(report) => pull_json(&store, report),
                Err(err) => json!({ "remote": name, "ok": false, "error": error_json(err) }),
            }).collect::<Vec<_>>(),
        }));
    } else {
        for (name, result) in &reports {
            match result {
                Ok(report) => print_pull(&store, report),
                Err(err) => eprintln!("warning[{}]: {name}: {err}", err.code()),
            }
        }
    }
    Ok(if conflicts {
        EXIT_UNRESOLVED
    } else if failed {
        1
    } else {
        0
    })
}

fn outcome_json(sibling: &SiblingPull) -> Value {
    let (outcome, extra) = match &sibling.outcome {
        Ok(MergeOutcome::UpToDate) => ("up_to_date", Value::Null),
        Ok(MergeOutcome::FastForward(id)) => ("fast_forward", json!(id.as_str())),
        Ok(MergeOutcome::Merged(id)) => ("merged", json!(id.as_str())),
        Ok(MergeOutcome::Conflicts { conflicts, .. }) => (
            "conflicts",
            Value::Array(
                conflicts
                    .iter()
                    .map(|c| json!({ "file": c.file, "field": c.field }))
                    .collect(),
            ),
        ),
        Err(err) => ("error", error_json(err)),
    };
    json!({
        "machine": sibling.machine_id,
        "label": sibling.label,
        "outcome": outcome,
        "detail": extra,
    })
}

fn artifacts_json(artifacts: &[Artifact]) -> Value {
    Value::Array(
        artifacts
            .iter()
            .map(|a| json!({ "code": "sync_artifact_ignored", "path": path_json(&a.path), "tool": a.tool }))
            .collect(),
    )
}

/// Servers and skills a pull changed.
fn changed(store: &Store<'_>, report: &PullReport) -> (usize, usize) {
    if report.head_before == report.head_after {
        return (0, 0);
    }
    let changes: Vec<EntityChange> = store
        .semantic_diff(Some(&report.head_before), &report.head_after)
        .unwrap_or_default();
    let mut servers: Vec<&str> = Vec::new();
    let mut skills: Vec<&str> = Vec::new();
    for change in &changes {
        if change.kind == ChangeKind::Changed && change.fields.is_empty() && change.text.is_none() {
            continue;
        }
        let list = if change.entity.starts_with("server/") {
            &mut servers
        } else if change.entity.starts_with("skill/") {
            &mut skills
        } else {
            continue;
        };
        if !list.contains(&change.entity.as_str()) {
            list.push(&change.entity);
        }
    }
    (servers.len(), skills.len())
}

fn pull_json(store: &Store<'_>, report: &PullReport) -> Value {
    let (servers, skills) = changed(store, report);
    json!({
        "remote": report.remote,
        "ok": true,
        "tag": report.tag,
        "head_before": report.head_before.as_str(),
        "head_after": report.head_after.as_str(),
        "machines": report.siblings.iter().map(outcome_json).collect::<Vec<_>>(),
        "tags_imported": report.tags_imported,
        "tags_moved": report.tags_moved,
        "changed_servers": servers,
        "changed_skills": skills,
        "conflicts": report.conflicts(),
        "warnings": artifacts_json(&report.artifacts),
    })
}

fn print_pull(store: &Store<'_>, report: &PullReport) {
    for tag in &report.tags_moved {
        eprintln!(
            "warning[tag_moved]: {}: tag {tag} now points where the remote's tag of that name does",
            report.remote
        );
    }
    for sibling in &report.siblings {
        let line = match &sibling.outcome {
            Ok(MergeOutcome::UpToDate) => "up to date".to_owned(),
            Ok(MergeOutcome::FastForward(id)) => format!("fast-forward to {}", id.short()),
            Ok(MergeOutcome::Merged(id)) => format!("merged as {}", id.short()),
            Ok(MergeOutcome::Conflicts { conflicts, .. }) => format!(
                "{} conflicting field(s), nothing merged; see state/merge/MERGE_STATE.json",
                conflicts.len()
            ),
            Err(err) => format!("error[{}]: {err}", err.code()),
        };
        match &sibling.machine_id {
            Some(id) => println!("{}/{} ({id}): {line}", report.remote, sibling.label),
            None => println!("{}: {line}", report.remote),
        }
    }
    for artifact in &report.artifacts {
        eprintln!(
            "info[sync_artifact_ignored]: {} ({}); left in place",
            artifact.path.display(),
            artifact.tool
        );
    }
    let (servers, skills) = changed(store, report);
    if report.head_before != report.head_after {
        println!(
            "{servers} server(s), {skills} skill(s) changed by pull. Review with `{bin} diff` and \
             apply with `{bin} apply`.",
            bin = brand::BIN_NAME
        );
    }
}

pub fn machine_json(entry: &MachineEntry) -> Value {
    json!({
        "store_id": entry.store_id,
        "machine_id": entry.machine_id,
        "label": entry.manifest.label,
        "os": entry.manifest.os,
        "pushed_at": entry.manifest.pushed_at,
        "head": entry.manifest.head,
        "subject": entry.subject,
        "dir": path_json(&entry.dir),
    })
}

pub fn print_machines(entries: &[MachineEntry]) {
    for entry in entries {
        println!(
            "{}  {:<16} {:<8} {}  {}",
            entry.machine_id,
            entry.manifest.label,
            entry.manifest.os,
            entry.manifest.pushed_at,
            entry.subject.as_deref().unwrap_or("")
        );
    }
}

fn machines(ctx: &Context, name: &str) -> Result<(), CoreError> {
    let file = remotes(ctx)?;
    let remote = file
        .get(name)
        .ok_or_else(|| CoreError::new(ErrorCode::NotFound, format!("no remote named {name:?}")))?;
    if remote.kind != RemoteKind::Folder {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{name} is a git remote; only folder remotes hold machine subfolders"),
        ));
    }
    let root = folder::reachable_root(ctx.fs(), ctx.env.os(), &UNC, remote)?;
    let listing = folder::list(ctx.fs(), &root, None)?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "machines": listing.machines.iter().map(machine_json).collect::<Vec<_>>(),
            "warnings": artifacts_json(&listing.artifacts),
        }));
        return Ok(());
    }
    if listing.machines.is_empty() {
        println!("No machines in {name}.");
    }
    print_machines(&listing.machines);
    for artifact in &listing.artifacts {
        eprintln!(
            "info[sync_artifact_ignored]: {} ({})",
            artifact.path.display(),
            artifact.tool
        );
    }
    Ok(())
}

//! `loadout project init|add|ls|rm|trust|share` (SPEC 5.5, 6, 14, 17).
//!
//! The project folder is the source of truth: its manifests are read from
//! `<P>/.loadout/`. This machine's overlay (`machines/<machine-id>.toml`
//! `[projects.<id>] path`) says which projects it knows, `state/trust.json`
//! which manifest contents it trusts, and the store mirror
//! `projects/<id>/` holds the content last trusted. Every change is one
//! store commit (`Loadout-Scope: project/<id>`); `init`, `rm`, `trust` and
//! `share` apply in that commit in instant mode and exit like `loadout
//! apply` would for that run.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use agent_loadout_core::apply::{ApplyReport, ProjectSelection};
use agent_loadout_core::layering::manifest::manifest_dir;
use agent_loadout_core::layering::{
    KnownProject, LOCAL_FILE, Project, ProjectRepo, ProjectStatus, SHARED_FILE, TrustState,
    TrustStore, content_hash, discover, known_projects, same_path,
};
use agent_loadout_core::model::TomlDocument;
use agent_loadout_core::model::ids::{self, IdKind};
use agent_loadout_core::model::machine::{MachineOverlay, MachineProject};
use agent_loadout_core::model::project::ProjectManifest;
use agent_loadout_core::secrets::catalog::overlay_path;
use agent_loadout_core::store::{
    CommitId, Machine, Operation, Scope, Store, StoreRead, Transaction,
};
use agent_loadout_core::{CoreError, Env, ErrorCode, Fs};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use toml_edit::{DocumentMut, Item, Table};

use super::{
    Context, JSON_SCHEMA, apply_exit_code, error_json, instant_apply, path_json, print_json,
    print_report, report_json, require_store,
};

/// `.loadout/.gitignore` of a shared project: the private file stays out
/// of the project's repository.
const GITIGNORE: &str = ".gitignore";

#[derive(Debug, Args)]
pub struct ProjectArgs {
    #[command(subcommand)]
    command: ProjectCommand,
}

#[derive(Debug, Subcommand)]
enum ProjectCommand {
    /// Create a project manifest in PATH (default: the current folder);
    /// private unless --shared.
    Init {
        /// Project folder [default: the current folder].
        path: Option<PathBuf>,
        /// Write the shared `.loadout/project.toml` (committed with the
        /// project) instead of the private `.loadout/project.local.toml`.
        #[arg(long)]
        shared: bool,
        /// Display name [default: the folder name].
        #[arg(long)]
        name: Option<String>,
    },
    /// Register the project whose manifests are at or above PATH; it is
    /// applied once trusted (`project trust`).
    Add {
        /// A folder inside the project.
        path: PathBuf,
    },
    /// List the projects this machine knows, with their trust state.
    Ls,
    /// Forget a project on this machine and remove its owned entries.
    Rm {
        /// Project id.
        id: String,
    },
    /// Review the manifests at or above PATH and trust them as they are.
    Trust {
        /// A folder inside the project.
        path: PathBuf,
    },
    /// Move private servers from `project.local.toml` into `project.toml`.
    Share {
        /// Project id.
        id: String,
        /// Server to move (repeatable) [default: every private server].
        #[arg(long = "server", value_name = "NAME")]
        servers: Vec<String>,
    },
}

pub fn run(ctx: &Context, args: &ProjectArgs) -> Result<u8, CoreError> {
    match &args.command {
        ProjectCommand::Init { path, shared, name } => {
            init(ctx, path.as_deref(), *shared, name.as_deref())
        }
        ProjectCommand::Add { path } => add(ctx, path).map(|()| 0),
        ProjectCommand::Ls => list(ctx).map(|()| 0),
        ProjectCommand::Rm { id } => remove(ctx, id),
        ProjectCommand::Trust { path } => trust(ctx, path),
        ProjectCommand::Share { id, servers } => share(ctx, id, servers),
    }
}

/// The project selection of `apply`, `status` and `diff`: every known
/// project (and the user scope) without PATH, else the project of PATH.
pub fn selection(ctx: &Context, path: Option<&Path>) -> Result<ProjectSelection, CoreError> {
    match path {
        None => Ok(ProjectSelection::All),
        Some(path) => project_root(ctx, path).map(ProjectSelection::Only),
    }
}

fn init(
    ctx: &Context,
    path: Option<&Path>,
    shared: bool,
    name: Option<&str>,
) -> Result<u8, CoreError> {
    let root = folder(path)?;
    if same_path(&root, ctx.env.home()) {
        return Err(CoreError::new(
            ErrorCode::InvalidPath,
            "a manifest in the home folder would apply to every folder; pick a project folder",
        )
        .with_path(&root));
    }
    let fs = ctx.fs();
    if !fs.is_dir(&root) {
        return Err(
            CoreError::new(ErrorCode::NotFound, "the project folder does not exist")
                .with_path(&root),
        );
    }
    let dir = manifest_dir(&root);
    for file in [SHARED_FILE, LOCAL_FILE] {
        let existing = dir.join(file);
        if fs.exists(&existing) {
            return Err(CoreError::new(
                ErrorCode::AlreadyExists,
                "this folder already has a project manifest; use `project add` or `project trust`",
            )
            .with_path(existing));
        }
    }
    let folder_name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let id = new_project_id(&ctx.env, &folder_name)?;
    let mut manifest = ProjectManifest::new(Some(id.clone()));
    manifest.name = Some(
        name.map(str::to_owned)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| {
                if folder_name.is_empty() {
                    id.clone()
                } else {
                    folder_name.clone()
                }
            }),
    );
    let text = TomlDocument::new(manifest).to_text()?;

    // The store opens (and warns about a sync folder) before the folder is
    // touched.
    let store = ctx.open_store_for_change()?;
    let file = dir.join(if shared { SHARED_FILE } else { LOCAL_FILE });
    let mut created = vec![file.clone()];
    create_dir(fs, &dir)?;
    write_file(fs, &file, text.as_bytes())?;
    let mut excluded = None;
    let staged = (|| {
        if shared {
            let ignore = dir.join(GITIGNORE);
            if !fs.exists(&ignore) {
                created.push(ignore.clone());
            }
            ensure_line(fs, &ignore, LOCAL_FILE)?;
        } else {
            excluded = exclude_manifest_dir(fs, &root)?;
        }
        let project = load_project(fs, &root)?;
        let summary = format!("project: init {id}");
        commit_project(ctx, &store, "project.init", summary, &id, true, |tx| {
            set_known(tx, ctx.fs(), &project.id, &root)?;
            mirror(tx, &project)?;
            save_trust(ctx, &project)
        })
    })();
    let (report, commit) = match staged {
        Ok(done) => done,
        Err(err) => {
            // Nothing was committed: leave the folder as it was.
            for path in &created {
                let _ = fs.remove_file(path);
            }
            let _ = std::fs::remove_dir(&dir);
            return Err(err);
        }
    };
    let layout = if shared { "shared" } else { "private" };
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "project": {
                "id": id,
                "path": path_json(&root),
                "layout": layout,
                "file": path_json(&file),
            },
            "excluded": excluded.as_deref().map(path_json),
            "commit": commit.as_ref().map(CommitId::to_string),
            "apply": report.as_ref().map(|r| report_json(r, commit.as_ref())),
        }));
    } else {
        println!(
            "Created {layout} project {id} in {} (commit {}).",
            root.display(),
            short(commit.as_ref())
        );
        println!("  {}", file.display());
        if let Some(exclude) = &excluded {
            println!("  .loadout/ is hidden from git in {}", exclude.display());
        }
        if let Some(report) = &report {
            print_report(report, commit.as_ref());
        }
    }
    Ok(report.as_ref().map_or(0, apply_exit_code))
}

fn add(ctx: &Context, path: &Path) -> Result<(), CoreError> {
    let root = project_root(ctx, path)?;
    let fs = ctx.fs();
    let project = load_project(fs, &root)?;
    warn_all(&project.warnings);
    let store = ctx.open_store_for_change()?;
    let id = project.id.clone();
    let summary = format!("project: add {id}");
    let (_, commit) = commit_project(ctx, &store, "project.add", summary, &id, false, |tx| {
        set_known(tx, fs, &id, &root)
    })?;
    let trust = TrustStore::load(fs, &ctx.home.state_dir())?.state(&project);
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "project": project_json(fs, &id, &root),
            "trust": trust.as_str(),
            "commit": commit.as_ref().map(CommitId::to_string),
        }));
    } else {
        println!(
            "Added project {id} at {} (commit {}).",
            root.display(),
            short(commit.as_ref())
        );
        if trust != TrustState::Trusted {
            println!(
                "Review its manifests, then run `{} project trust {}` to apply it.",
                agent_loadout_core::brand::BIN_NAME,
                root.display()
            );
        }
    }
    Ok(())
}

fn list(ctx: &Context) -> Result<(), CoreError> {
    let fs = ctx.fs();
    let overlay = match ctx.open_store()? {
        Some(store) => this_overlay(ctx, &store)?,
        None => None,
    };
    let trust = TrustStore::load(fs, &ctx.home.state_dir())?;
    let known = known_projects(fs, &ctx.env, overlay.as_ref(), &trust);
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "projects": known.iter().map(|project| {
                let (state, error) = trust_state(project);
                json!({
                    "id": project.id,
                    "path": path_json(&project.root),
                    "layout": layout(fs, &project.root),
                    "trust": state,
                    "error": error.map(error_json),
                })
            }).collect::<Vec<_>>(),
        }));
        return Ok(());
    }
    if known.is_empty() {
        println!("No projects.");
        return Ok(());
    }
    let header = ["ID", "PATH", "LAYOUT", "TRUST"].map(str::to_owned);
    let table: Vec<[String; 4]> = known
        .iter()
        .map(|project| {
            [
                project.id.clone(),
                project.root.display().to_string(),
                layout(fs, &project.root).unwrap_or("-").to_owned(),
                trust_state(project).0.to_owned(),
            ]
        })
        .collect();
    let widths: Vec<usize> = (0..3)
        .map(|column| {
            table
                .iter()
                .chain([&header])
                .map(|row| row[column].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for row in [&header].into_iter().chain(&table) {
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
    for project in &known {
        if let (_, Some(err)) = trust_state(project) {
            eprintln!("warning[{}]: project {}: {err}", err.code(), project.id);
        }
    }
    Ok(())
}

/// `trusted`, `untrusted`, `changed`, `missing` (the folder is gone) or
/// `invalid` (with the error: no usable manifest).
fn trust_state(project: &KnownProject) -> (&'static str, Option<&CoreError>) {
    match &project.status {
        ProjectStatus::Missing => ("missing", None),
        ProjectStatus::Failed(err) => ("invalid", Some(err)),
        ProjectStatus::Loaded { trust, .. } => (trust.as_str(), None),
    }
}

fn remove(ctx: &Context, id: &str) -> Result<u8, CoreError> {
    ids::check(IdKind::Project, id)?;
    let store = require_store(ctx, "no project is known")?;
    let summary = format!("project: remove {id}");
    let mut root = None;
    let (report, commit) =
        commit_project(ctx, &store, "project.remove", summary, id, true, |tx| {
            let mut doc = overlay_doc(tx)?;
            let projects = &mut doc.value_mut().projects;
            let known = projects.as_mut().and_then(|map| map.remove(id));
            let Some(known) = known else {
                return Err(unknown_project(id));
            };
            if projects.as_ref().is_some_and(BTreeMap::is_empty) {
                *projects = None;
            }
            root = Some(PathBuf::from(known.path));
            let relative = overlay_path(&tx.machine().id)?;
            tx.write(&relative, doc.to_text()?)?;
            let state = ctx.home.state_dir();
            let mut trust = TrustStore::load(ctx.fs(), &state)?;
            if trust.forget(id) {
                trust.save(ctx.fs(), &state)?;
            }
            Ok(())
        })?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "project": id,
            "path": root.as_deref().map(path_json),
            "commit": commit.as_ref().map(CommitId::to_string),
            "apply": report.as_ref().map(|r| report_json(r, commit.as_ref())),
        }));
    } else {
        println!(
            "Removed project {id} from this machine (commit {}).",
            short(commit.as_ref())
        );
        if let Some(report) = &report {
            print_report(report, commit.as_ref());
        }
    }
    Ok(report.as_ref().map_or(0, apply_exit_code))
}

/// A manifest file as the trust review shows it.
struct Review {
    file: &'static str,
    path: PathBuf,
    /// `new`, `changed`, `unchanged` or `removed`, against the mirror.
    change: &'static str,
    content: Option<String>,
    diff: Option<String>,
}

fn trust(ctx: &Context, path: &Path) -> Result<u8, CoreError> {
    let root = project_root(ctx, path)?;
    let fs = ctx.fs();
    let project = load_project(fs, &root)?;
    warn_all(&project.warnings);
    let store = ctx.open_store_for_change()?;
    let reviews = review(&store, &project)?;
    if !ctx.json {
        for review in &reviews {
            match review.change {
                "new" => {
                    println!("=== {} (new)", review.path.display());
                    print!("{}", review.content.as_deref().unwrap_or_default());
                }
                "changed" => print!("{}", review.diff.as_deref().unwrap_or_default()),
                change => println!("=== {} ({change})", review.path.display()),
            }
        }
    }
    let id = project.id.clone();
    let summary = format!("project: trust {id}");
    let (report, commit) =
        commit_project(ctx, &store, "project.trust", summary, &id, true, |tx| {
            set_known(tx, ctx.fs(), &id, &root)?;
            mirror(tx, &project)?;
            save_trust(ctx, &project)
        })?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "project": project_json(fs, &id, &root),
            "files": reviews.iter().map(|review| json!({
                "file": review.file,
                "path": path_json(&review.path),
                "change": review.change,
                "content": review.content,
                "diff": review.diff,
            })).collect::<Vec<_>>(),
            "commit": commit.as_ref().map(CommitId::to_string),
            "apply": report.as_ref().map(|r| report_json(r, commit.as_ref())),
        }));
    } else {
        println!(
            "Trusted project {id} at {} (commit {}).",
            root.display(),
            short(commit.as_ref())
        );
        if let Some(report) = &report {
            print_report(report, commit.as_ref());
        }
    }
    Ok(report.as_ref().map_or(0, apply_exit_code))
}

/// Each manifest file against the store mirror (the content last trusted).
fn review(store: &Store<'_>, project: &Project) -> Result<Vec<Review>, CoreError> {
    let dir = manifest_dir(&project.root);
    let mut reviews = Vec::new();
    for (file, manifest) in [
        (SHARED_FILE, project.shared.as_ref()),
        (LOCAL_FILE, project.local.as_ref()),
    ] {
        let trusted = store.read_to_string(&mirror_path(&project.id, file))?;
        let current = manifest.map(|m| String::from_utf8_lossy(&m.bytes).into_owned());
        let path = dir.join(file);
        let (change, diff) = match (&trusted, &current) {
            (None, None) => continue,
            (None, Some(_)) => ("new", None),
            (Some(_), None) => ("removed", None),
            (Some(old), Some(new)) if old == new => ("unchanged", None),
            (Some(old), Some(new)) => {
                let diff = similar::TextDiff::from_lines(old.as_str(), new.as_str())
                    .unified_diff()
                    .header(&format!("trusted {file}"), &path.display().to_string())
                    .to_string();
                ("changed", Some(diff))
            }
        };
        reviews.push(Review {
            file,
            path,
            change,
            content: current,
            diff,
        });
    }
    Ok(reviews)
}

fn share(ctx: &Context, id: &str, servers: &[String]) -> Result<u8, CoreError> {
    ids::check(IdKind::Project, id)?;
    let fs = ctx.fs();
    let store = require_store(ctx, "no project is known")?;
    let overlay = this_overlay(ctx, &store)?;
    let trust = TrustStore::load(fs, &ctx.home.state_dir())?;
    let known = known_projects(fs, &ctx.env, overlay.as_ref(), &trust);
    let known = known
        .iter()
        .find(|project| project.id == id)
        .ok_or_else(|| unknown_project(id))?;
    let root = known.root.clone();
    match &known.status {
        ProjectStatus::Missing => {
            return Err(
                CoreError::new(ErrorCode::NotFound, "the project folder does not exist")
                    .with_path(&root),
            );
        }
        ProjectStatus::Failed(err) => return Err(err.clone()),
        ProjectStatus::Loaded { .. } => {
            // Sharing re-trusts what it writes, so it must start from
            // content that is trusted now.
            if let Some(err) = known.trust_error() {
                return Err(err);
            }
        }
    }

    let dir = manifest_dir(&root);
    let local_path = dir.join(LOCAL_FILE);
    let shared_path = dir.join(SHARED_FILE);
    let local_before = read_optional(fs, &local_path)?.ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotFound,
            "the project has no project.local.toml; nothing is private",
        )
        .with_path(&local_path)
    })?;
    let shared_before = read_optional(fs, &shared_path)?;
    if let Some(project) = known.project() {
        check_unchanged(project, shared_before.as_deref(), &local_before)?;
    }
    let moved = move_servers(id, &local_before, shared_before.as_deref(), servers)?;

    let created_shared = shared_before.is_none();
    write_file(fs, &shared_path, moved.shared.as_bytes())?;
    write_file(fs, &local_path, moved.local.as_bytes())?;
    let staged = (|| {
        let project = load_project(fs, &root)?;
        let mut notes = Vec::new();
        if created_shared {
            ensure_line(fs, &dir.join(GITIGNORE), LOCAL_FILE)?;
            if let Some(note) = unexclude_manifest_dir(fs, &root)? {
                notes.push(note);
            }
        }
        let summary = format!("project: share {} in {id}", moved.names.join(", "));
        let done = commit_project(ctx, &store, "project.share", summary, id, true, |tx| {
            mirror(tx, &project)?;
            save_trust(ctx, &project)
        })?;
        Ok((done, notes))
    })();
    let ((report, commit), notes) = match staged {
        Ok(done) => done,
        Err(err) => {
            // Nothing was committed: put both files back.
            let _ = write_file(fs, &local_path, local_before.as_bytes());
            match &shared_before {
                Some(text) => {
                    let _ = write_file(fs, &shared_path, text.as_bytes());
                }
                None => {
                    let _ = fs.remove_file(&shared_path);
                }
            }
            return Err(err);
        }
    };
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "project": project_json(fs, id, &root),
            "servers": moved.names,
            "created": created_shared,
            "commit": commit.as_ref().map(CommitId::to_string),
            "apply": report.as_ref().map(|r| report_json(r, commit.as_ref())),
        }));
    } else {
        println!(
            "Moved {} into {} (commit {}).",
            moved.names.join(", "),
            shared_path.display(),
            short(commit.as_ref())
        );
        for note in &notes {
            println!("  {note}");
        }
        if let Some(report) = &report {
            print_report(report, commit.as_ref());
        }
    }
    Ok(report.as_ref().map_or(0, apply_exit_code))
}

/// Both manifest texts after [`move_servers`].
struct Moved {
    local: String,
    shared: String,
    names: Vec<String>,
}

/// `trust_required` when the manifest texts about to be rewritten are not
/// the content `project` was loaded (and found trusted) with: a file
/// changed in between must be reviewed, not trusted along with the share.
fn check_unchanged(project: &Project, shared: Option<&str>, local: &str) -> Result<(), CoreError> {
    if content_hash(shared.map(str::as_bytes), Some(local.as_bytes())) == project.hash() {
        return Ok(());
    }
    Err(CoreError::new(
        ErrorCode::TrustRequired,
        format!(
            "the manifests of project {:?} changed while sharing; review them, then run `{} \
             project trust {}`",
            project.name(),
            agent_loadout_core::brand::BIN_NAME,
            project.root.display()
        ),
    )
    .with_path(manifest_dir(&project.root)))
}

/// Moves `[servers.<name>]` tables (with their sub-tables and the comment
/// block directly above each header) from the local text into the shared
/// one, creating the shared file when absent (`schema`, `id`, and the local
/// `name`). Without `wanted`, every local server the shared file does not
/// define moves; local tables of shared names are overrides and stay. The
/// local file keeps its `id` unless nothing but `schema` and `id` is left.
fn move_servers(
    id: &str,
    local: &str,
    shared: Option<&str>,
    wanted: &[String],
) -> Result<Moved, CoreError> {
    let parse = |text: &str, file: &str| {
        text.parse::<DocumentMut>().map_err(|err| {
            CoreError::new(
                ErrorCode::ParseError,
                format!("{file} does not parse: {err}"),
            )
        })
    };
    let server_names = |doc: &DocumentMut| -> Vec<String> {
        doc.get("servers")
            .and_then(Item::as_table_like)
            .map(|servers| servers.iter().map(|(name, _)| name.to_owned()).collect())
            .unwrap_or_default()
    };
    let mut local_doc = parse(local, LOCAL_FILE)?;
    let shared_doc = shared.map(|text| parse(text, SHARED_FILE)).transpose()?;
    let available = server_names(&local_doc);
    let defined = shared_doc.as_ref().map(server_names).unwrap_or_default();
    let names: Vec<String> = if wanted.is_empty() {
        available
            .iter()
            .filter(|name| !defined.contains(name))
            .cloned()
            .collect()
    } else {
        let mut names = Vec::new();
        for name in wanted {
            if !available.contains(name) {
                return Err(CoreError::new(
                    ErrorCode::NotFound,
                    format!("project.local.toml has no server {name:?}"),
                ));
            }
            if defined.contains(name) {
                return Err(CoreError::new(
                    ErrorCode::AlreadyExists,
                    format!(
                        "project.toml already defines server {name:?}; the local table is a \
                         private override of it"
                    ),
                ));
            }
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names
    };
    if names.is_empty() {
        return Err(CoreError::new(
            ErrorCode::NotFound,
            if available.is_empty() {
                "project.local.toml has no servers to share"
            } else {
                "every server in project.local.toml overrides one in project.toml; overrides \
                 stay private"
            },
        ));
    }
    let mut shared_doc = match shared_doc {
        Some(doc) => doc,
        None => {
            let mut doc = DocumentMut::new();
            doc["schema"] = toml_edit::value(1);
            doc["id"] = toml_edit::value(id);
            if let Some(name) = local_doc.remove("name") {
                doc["name"] = name;
            }
            doc
        }
    };
    let shared_servers = shared_doc
        .entry("servers")
        .or_insert_with(|| {
            let mut table = Table::new();
            table.set_implicit(true);
            Item::Table(table)
        })
        .as_table_like_mut()
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::UnexpectedShape,
                "`servers` in project.toml is not a table",
            )
        })?;
    let local_servers = local_doc
        .get_mut("servers")
        .and_then(Item::as_table_like_mut)
        .expect("servers were listed above");
    // Comments above a moved header that a blank line separates from it
    // belong to what came before; they stay in the local file.
    let mut left_behind: Vec<(isize, String)> = Vec::new();
    for name in &names {
        let mut item = local_servers
            .remove(name)
            .expect("names come from this table");
        if let Item::Table(table) = &mut item {
            let prefix = table
                .decor()
                .prefix()
                .and_then(|raw| raw.as_str())
                .unwrap_or_default()
                .to_owned();
            let (kept, attached) = split_comments(&prefix);
            table.decor_mut().set_prefix(format!("\n{attached}"));
            if let (Some(position), Some(kept)) = (table.position(), kept) {
                left_behind.push((position, kept));
            }
        }
        clear_positions(&mut item);
        shared_servers.insert(name, item);
    }
    if local_servers.is_empty() {
        local_doc.remove("servers");
    }
    // Latest first, so text left by earlier tables lands in front of it.
    left_behind.sort_by_key(|(position, _)| std::cmp::Reverse(*position));
    for (position, text) in left_behind {
        attach_after(&mut local_doc, position, &text);
    }
    let only_identity = local_doc
        .iter()
        .all(|(key, _)| key == "schema" || key == "id");
    if only_identity {
        local_doc.remove("id");
    }
    Ok(Moved {
        local: local_doc.to_string(),
        shared: shared_doc.to_string(),
        names,
    })
}

/// Splits a header's prefix decor at its last blank line: the text before
/// it (`None` when only whitespace) and the comment block directly above
/// the header.
fn split_comments(prefix: &str) -> (Option<String>, String) {
    let lines: Vec<&str> = prefix.split_inclusive('\n').collect();
    let Some(blank) = lines
        .iter()
        .rposition(|line| line.ends_with('\n') && line.trim().is_empty())
    else {
        return (None, prefix.to_owned());
    };
    let kept = lines[..blank].concat();
    let attached = lines[blank + 1..].concat();
    ((!kept.trim().is_empty()).then_some(kept), attached)
}

/// Puts `text` in front of the first table header after document position
/// `position`, or at the end of the document when none follows.
fn attach_after(doc: &mut DocumentMut, position: isize, text: &str) {
    let mut headers = Vec::new();
    header_tables(doc.as_table(), &mut Vec::new(), &mut headers);
    let next = headers
        .into_iter()
        .filter(|(at, _)| *at > position)
        .min_by_key(|(at, _)| *at);
    match next {
        Some((_, path)) => {
            let mut table = doc.as_table_mut();
            for key in &path {
                table = table
                    .get_mut(key)
                    .and_then(Item::as_table_mut)
                    .expect("the path was collected from this document");
            }
            let prefix = table
                .decor()
                .prefix()
                .and_then(|raw| raw.as_str())
                .unwrap_or_default()
                .to_owned();
            // One blank line between the kept comments and the next block.
            let gap = if prefix.starts_with('\n') { "" } else { "\n" };
            table.decor_mut().set_prefix(format!("{text}{gap}{prefix}"));
        }
        None => {
            let trailing = doc.trailing().as_str().unwrap_or_default().to_owned();
            doc.set_trailing(format!("{text}{trailing}"));
        }
    }
}

/// Document positions and key paths of every table with a `[header]`.
fn header_tables(table: &Table, path: &mut Vec<String>, out: &mut Vec<(isize, Vec<String>)>) {
    for (key, item) in table.iter() {
        if let Item::Table(child) = item {
            path.push(key.to_owned());
            if !child.is_implicit()
                && !child.is_dotted()
                && let Some(position) = child.position()
            {
                out.push((position, path.clone()));
            }
            header_tables(child, path, out);
            path.pop();
        }
    }
}

/// Lets moved tables take their place in the destination document's order
/// (after the tables before them) instead of their source positions.
fn clear_positions(item: &mut Item) {
    match item {
        Item::Table(table) => {
            table.set_position(None);
            for (_, child) in table.iter_mut() {
                clear_positions(child);
            }
        }
        Item::ArrayOfTables(array) => {
            for table in array.iter_mut() {
                table.set_position(None);
                for (_, child) in table.iter_mut() {
                    clear_positions(child);
                }
            }
        }
        Item::None | Item::Value(_) => {}
    }
}

/// Stages one project commit; with `apply`, the instant apply runs in it.
/// `state/trust.json` reflects committed operations only: when the
/// transaction fails (staging, validation, the leak scanner), the trust
/// file as it was before is written back.
fn commit_project(
    ctx: &Context,
    store: &Store<'_>,
    op: &str,
    summary: String,
    id: &str,
    apply: bool,
    stage: impl FnOnce(&mut Transaction<'_>) -> Result<(), CoreError>,
) -> Result<(Option<ApplyReport>, Option<CommitId>), CoreError> {
    let operation = Operation::new(op, summary)
        .entity(format!("project/{id}"))
        .scope(Scope::Project(id.to_owned()));
    let ax = ctx.apply_env();
    let state = ctx.home.state_dir();
    let previous = TrustStore::load(ctx.fs(), &state).ok();
    let committed = store.transaction(operation, |tx| {
        stage(tx)?;
        if apply {
            instant_apply(tx, &ax)
        } else {
            Ok(None)
        }
    });
    match committed {
        Ok(committed) => Ok((committed.value, committed.commit)),
        Err(err) => {
            if let Some(previous) = previous
                && let Err(restore) = previous.save(ctx.fs(), &state)
            {
                eprintln!(
                    "warning[{}]: cannot restore the trust file: {restore}",
                    restore.code()
                );
            }
            Err(err)
        }
    }
}

/// This machine's overlay, staged or new.
fn overlay_doc(tx: &Transaction<'_>) -> Result<TomlDocument<MachineOverlay>, CoreError> {
    let machine = tx.machine().clone();
    let relative = overlay_path(&machine.id)?;
    match tx.read_file(&relative)? {
        Some(text) => TomlDocument::parse(&text)
            .map_err(|diagnostic| diagnostic.to_core_error(Some(Path::new(&relative)))),
        None => Ok(TomlDocument::new(MachineOverlay::new(
            &machine.id,
            &machine.label,
            tx.store().env().os(),
        ))),
    }
}

/// Records `[projects.<id>] path` in this machine's overlay. Another id
/// recorded for the same folder (its manifest id changed) is dropped. When
/// `id` is known at another folder that still holds a manifest with this
/// id (a second checkout of the same project), the change is refused; a
/// folder that is gone or now holds another project is repointed (moved).
fn set_known(
    tx: &mut Transaction<'_>,
    fs: &dyn Fs,
    id: &str,
    root: &Path,
) -> Result<(), CoreError> {
    let mut doc = overlay_doc(tx)?;
    let projects = doc.value_mut().projects.get_or_insert_with(BTreeMap::new);
    if let Some(known) = projects.get(id) {
        let other = Path::new(&known.path);
        let checkout = !same_path(other, root)
            && matches!(Project::load(fs, other), Ok(Some(project)) if project.id == id);
        if checkout {
            return Err(CoreError::new(
                ErrorCode::AlreadyExists,
                format!(
                    "project {id:?} is already known at {}, which still holds its manifest; \
                     one machine applies a project from one folder. To use {} instead, run \
                     `{} project rm {id}` first",
                    other.display(),
                    root.display(),
                    agent_loadout_core::brand::BIN_NAME,
                ),
            )
            .with_path(root));
        }
    }
    projects.retain(|known, project| known == id || !same_path(Path::new(&project.path), root));
    let path = root.to_string_lossy().into_owned();
    projects
        .entry(id.to_owned())
        .and_modify(|project| project.path.clone_from(&path))
        .or_insert_with(|| MachineProject {
            path,
            extra: toml::Table::new(),
        });
    let relative = overlay_path(&tx.machine().id)?;
    tx.write(&relative, doc.to_text()?)
}

/// This machine's overlay from the store, when both exist.
fn this_overlay(ctx: &Context, store: &Store<'_>) -> Result<Option<MachineOverlay>, CoreError> {
    let Some(machine) = Machine::load(ctx.fs(), &ctx.home.state_dir())? else {
        return Ok(None);
    };
    let relative = overlay_path(&machine.id)?;
    let Some(text) = store.read_to_string(&relative)? else {
        return Ok(None);
    };
    TomlDocument::<MachineOverlay>::parse(&text)
        .map(|doc| Some(doc.into_value()))
        .map_err(|diagnostic| diagnostic.to_core_error(Some(Path::new(&relative))))
}

fn mirror_path(id: &str, file: &str) -> String {
    format!("projects/{id}/{file}")
}

/// Stages the store mirror `projects/<id>/` as exactly the folder's
/// manifests (the content being trusted).
fn mirror(tx: &mut Transaction<'_>, project: &Project) -> Result<(), CoreError> {
    for (file, manifest) in [
        (SHARED_FILE, project.shared.as_ref()),
        (LOCAL_FILE, project.local.as_ref()),
    ] {
        let relative = mirror_path(&project.id, file);
        match manifest {
            Some(manifest) => tx.write(&relative, manifest.bytes.clone())?,
            None if tx.exists(&relative)? => tx.remove(&relative)?,
            None => {}
        }
    }
    Ok(())
}

fn save_trust(ctx: &Context, project: &Project) -> Result<(), CoreError> {
    let state = ctx.home.state_dir();
    let mut trust = TrustStore::load(ctx.fs(), &state)?;
    trust.trust(project);
    trust.save(ctx.fs(), &state)
}

fn load_project(fs: &dyn Fs, root: &Path) -> Result<Project, CoreError> {
    Project::load(fs, root)?.ok_or_else(|| {
        CoreError::new(ErrorCode::NotFound, "the folder has no project manifest")
            .with_path(manifest_dir(root))
    })
}

/// The project folder of `path`: the nearest folder at or above it holding
/// a manifest, spelled as this machine's overlay records it.
fn project_root(ctx: &Context, path: &Path) -> Result<PathBuf, CoreError> {
    let folder = folder(Some(path))?;
    let root = discover(&ctx.env, ctx.fs(), &folder).ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotFound,
            "no project manifest (.loadout/project.toml or .loadout/project.local.toml) at or \
             above this folder",
        )
        .with_path(&folder)
    })?;
    let overlay = match ctx.open_store()? {
        Some(store) => this_overlay(ctx, &store)?,
        None => None,
    };
    let known = overlay
        .iter()
        .flat_map(|overlay| overlay.projects.iter().flatten())
        .map(|(_, project)| PathBuf::from(&project.path))
        .find(|known| same_path(known, &root));
    Ok(known.unwrap_or(root))
}

/// `path` (or the current folder) as an absolute path, with `.` and `..`
/// resolved.
pub fn folder(path: Option<&Path>) -> Result<PathBuf, CoreError> {
    let raw = match path {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir().map_err(|err| {
            CoreError::new(
                ErrorCode::InvalidPath,
                format!("cannot read the current folder: {err}"),
            )
        })?,
    };
    let absolute = std::path::absolute(&raw).map_err(|err| {
        CoreError::new(ErrorCode::InvalidPath, format!("invalid path: {err}")).with_path(&raw)
    })?;
    let mut normalized = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    Ok(normalized)
}

/// `<slug of the folder name>-<4 random [a-z0-9]>` (SPEC 5.5).
fn new_project_id(env: &Env, folder_name: &str) -> Result<String, CoreError> {
    let mut slug = String::new();
    for ch in folder_name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            slug.push(ch);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.truncate(ids::MAX_ID_LEN - 5);
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "project" } else { slug };
    // The random tail of a ULID: Crockford base32, so lowercase it is
    // [a-z0-9].
    let random = ids::new_ulid(env.now())?;
    let suffix = random[random.len() - 4..].to_ascii_lowercase();
    let id = format!("{slug}-{suffix}");
    ids::check(IdKind::Project, &id)?;
    Ok(id)
}

/// `private`, `shared` or `shared+local` from the manifest files present.
fn layout(fs: &dyn Fs, root: &Path) -> Option<&'static str> {
    let dir = manifest_dir(root);
    match (
        fs.exists(&dir.join(SHARED_FILE)),
        fs.exists(&dir.join(LOCAL_FILE)),
    ) {
        (true, true) => Some("shared+local"),
        (true, false) => Some("shared"),
        (false, true) => Some("private"),
        (false, false) => None,
    }
}

fn project_json(fs: &dyn Fs, id: &str, root: &Path) -> Value {
    json!({
        "id": id,
        "path": path_json(root),
        "layout": layout(fs, root),
    })
}

/// Hides `<P>/.loadout/` from the repository holding P through its
/// `info/exclude` (a private project, SPEC 5.5). Returns the exclude file
/// when it was changed.
fn exclude_manifest_dir(fs: &dyn Fs, root: &Path) -> Result<Option<PathBuf>, CoreError> {
    let Some((repo, pattern)) = manifest_dir_pattern(root)? else {
        return Ok(None);
    };
    if repo.is_ignored(&manifest_dir(root).join(LOCAL_FILE))? {
        return Ok(None);
    }
    let file = repo.exclude_file();
    Ok(ensure_line(fs, &file, &pattern)?.then_some(file))
}

/// Drops the `info/exclude` line [`exclude_manifest_dir`] wrote, so a
/// newly shared `project.toml` can be committed; `.loadout/.gitignore`
/// keeps the private file out. Returns a note for the user.
fn unexclude_manifest_dir(fs: &dyn Fs, root: &Path) -> Result<Option<String>, CoreError> {
    let Some((repo, pattern)) = manifest_dir_pattern(root)? else {
        return Ok(None);
    };
    let file = repo.exclude_file();
    if let Some(text) = read_optional(fs, &file)?
        && text.lines().any(|line| line.trim() == pattern)
    {
        let kept: String = text
            .lines()
            .filter(|line| line.trim() != pattern)
            .map(|line| format!("{line}\n"))
            .collect();
        write_file(fs, &file, kept.as_bytes())?;
    }
    let shared = manifest_dir(root).join(SHARED_FILE);
    Ok(Some(if repo.is_ignored(&shared)? {
        format!(
            "{} is ignored by git; change the ignore rules to commit it",
            shared.display()
        )
    } else {
        format!("commit {} with the project", shared.display())
    }))
}

/// The repository holding `root` and the `/<path>/.loadout/` pattern for
/// its manifest folder.
fn manifest_dir_pattern(root: &Path) -> Result<Option<(ProjectRepo, String)>, CoreError> {
    let Some(repo) = ProjectRepo::discover(root)? else {
        return Ok(None);
    };
    Ok(repo
        .relative(&manifest_dir(root))
        .map(|relative| (repo, format!("/{relative}/"))))
}

/// Appends `line` to the text file `path` (created when absent) unless it
/// already holds it; returns whether the file changed.
fn ensure_line(fs: &dyn Fs, path: &Path, line: &str) -> Result<bool, CoreError> {
    let mut text = read_optional(fs, path)?.unwrap_or_default();
    if text.lines().any(|existing| existing.trim() == line) {
        return Ok(false);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    if let Some(dir) = path.parent() {
        create_dir(fs, dir)?;
    }
    write_file(fs, path, text.as_bytes())?;
    Ok(true)
}

fn read_optional(fs: &dyn Fs, path: &Path) -> Result<Option<String>, CoreError> {
    if !fs.exists(path) {
        return Ok(None);
    }
    fs.read_to_string(path).map(Some).map_err(|err| {
        CoreError::new(ErrorCode::IoError, format!("cannot read file: {err}")).with_path(path)
    })
}

fn write_file(fs: &dyn Fs, path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    fs.write(path, bytes).map_err(|err| {
        CoreError::new(ErrorCode::IoError, format!("cannot write file: {err}")).with_path(path)
    })
}

fn create_dir(fs: &dyn Fs, dir: &Path) -> Result<(), CoreError> {
    fs.create_dir_all(dir).map_err(|err| {
        CoreError::new(
            ErrorCode::IoError,
            format!("cannot create directory: {err}"),
        )
        .with_path(dir)
    })
}

fn unknown_project(id: &str) -> CoreError {
    CoreError::new(
        ErrorCode::NotFound,
        format!("project {id:?} is not known on this machine"),
    )
}

fn short(commit: Option<&CommitId>) -> &str {
    commit.map_or("none", CommitId::short)
}

/// Manifest validation warnings go to stderr.
fn warn_all(warnings: &[CoreError]) {
    for warning in warnings {
        eprintln!("warning[{}]: {warning}", warning.code());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL: &str = "schema = 1\nid = \"app-ab12\"\n\n\
                         [servers.docs]\ncommand = \"docs-mcp\"\nargs = [\"--verbose\"]\n\
                         # old = \"a private note\"\n\n\
                         # The team's search.\n[servers.search]\ncommand = \"search-mcp\"\n\n\
                         [servers.notes]\ncommand = \"notes-mcp\"\n";
    const SHARED: &str = "schema = 1\nid = \"app-ab12\"\n\n\
                          [servers.docs]\ncommand = \"docs-mcp\"\n";

    #[test]
    fn default_share_leaves_overrides_of_shared_servers_local() {
        let moved = move_servers("app-ab12", LOCAL, Some(SHARED), &[]).unwrap();
        assert_eq!(moved.names, ["search", "notes"]);
        assert!(moved.local.contains("[servers.docs]"), "{}", moved.local);
        assert!(!moved.local.contains("search-mcp"), "{}", moved.local);
        // The existing shared file is kept as it was, plus the new tables.
        assert!(moved.shared.starts_with(SHARED), "{}", moved.shared);
        assert!(moved.shared.contains("[servers.notes]"), "{}", moved.shared);

        // Naming an override explicitly is a conflict.
        let err = move_servers("app-ab12", LOCAL, Some(SHARED), &["docs".to_owned()])
            .err()
            .unwrap();
        assert_eq!(err.code(), ErrorCode::AlreadyExists);
        // Only overrides left: nothing to share.
        let only = "schema = 1\nid = \"app-ab12\"\n\n[servers.docs]\nargs = [\"-v\"]\n";
        let err = move_servers("app-ab12", only, Some(SHARED), &[])
            .err()
            .unwrap();
        assert_eq!(err.code(), ErrorCode::NotFound);
        assert!(err.message().contains("overrides"), "{err}");
    }

    #[test]
    fn only_the_comment_block_attached_to_a_header_moves() {
        let moved = move_servers("app-ab12", LOCAL, None, &["search".to_owned()]).unwrap();
        assert!(
            moved
                .shared
                .contains("\n# The team's search.\n[servers.search]\n"),
            "{}",
            moved.shared
        );
        assert!(!moved.shared.contains("private note"), "{}", moved.shared);
        // The detached comment stays in the local file, before what follows.
        assert!(
            moved
                .local
                .contains("args = [\"--verbose\"]\n# old = \"a private note\"\n\n[servers.notes]"),
            "{}",
            moved.local
        );
        // Moving the last table leaves the detached text at the end.
        let last = "schema = 1\nid = \"app-ab12\"\n\n[servers.a]\ncommand = \"a\"\n\
                    # trailing note\n\n[servers.b]\ncommand = \"b\"\n";
        let moved = move_servers("app-ab12", last, None, &["b".to_owned()]).unwrap();
        assert!(moved.local.contains("# trailing note"), "{}", moved.local);
        assert!(!moved.shared.contains("trailing note"), "{}", moved.shared);
    }

    #[test]
    fn split_comments_cuts_at_the_last_blank_line() {
        assert_eq!(
            split_comments("\n# a\n\n# b\n"),
            (Some("\n# a\n".to_owned()), "# b\n".to_owned())
        );
        assert_eq!(split_comments("\n# b\n"), (None, "# b\n".to_owned()));
        assert_eq!(split_comments("# b\n"), (None, "# b\n".to_owned()));
    }

    #[test]
    fn share_refuses_manifests_that_changed_since_loading() {
        let th = agent_loadout_testkit::TempHome::new();
        let root = th.projects().join("app");
        let manifests = manifest_dir(&root);
        std::fs::create_dir_all(&manifests).unwrap();
        let local = "schema = 1\nid = \"app-ab12\"\n";
        std::fs::write(manifests.join(LOCAL_FILE), local).unwrap();
        let project = Project::load(&agent_loadout_core::RealFs, &root)
            .unwrap()
            .unwrap();
        check_unchanged(&project, None, local).unwrap();
        let err = check_unchanged(&project, Some(SHARED), local)
            .err()
            .unwrap();
        assert_eq!(err.code(), ErrorCode::TrustRequired);
        let edited = format!("{local}name = \"edited\"\n");
        let err = check_unchanged(&project, None, &edited).err().unwrap();
        assert_eq!(err.code(), ErrorCode::TrustRequired);
    }
}

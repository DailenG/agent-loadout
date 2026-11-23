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

use std::path::{Component, Path, PathBuf};

use agent_loadout_core::apply::{ProjectSelection, Touched};
use agent_loadout_core::layering::manifest::manifest_dir;
use agent_loadout_core::layering::projects::{
    self, GITIGNORE, ManifestChange, ensure_line, load_project, mirror, read_optional, save_trust,
    this_overlay, unexclude_manifest_dir, unknown_project, write_file,
};
use agent_loadout_core::layering::{
    KnownProject, LOCAL_FILE, Project, ProjectStatus, SHARED_FILE, TrustState, TrustStore,
    content_hash, known_projects,
};
use agent_loadout_core::model::ids::{self, IdKind};
use agent_loadout_core::model::machine::MachineOverlay;
use agent_loadout_core::store::{CommitId, Store};
use agent_loadout_core::{CoreError, ErrorCode, Fs};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use toml_edit::{DocumentMut, Item, Table};

use super::{
    Context, JSON_SCHEMA, apply_exit_code, error_json, path_json, print_json, print_report,
    report_json, require_store, status_exit_code,
};

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
    let plan = projects::prepare_init(&ctx.env, ctx.fs(), &root, name)?;
    // The store opens (and warns about a sync folder) before the folder is
    // touched.
    let store = ctx.open_store_for_change()?;
    let done = projects::init(&store, &ctx.apply_env(), plan, shared)?;
    let (id, file, excluded) = (&done.id, &done.file, &done.excluded);
    let (report, commit) = (&done.report, &done.commit);
    let layout = if shared { "shared" } else { "private" };
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "project": {
                "id": id,
                "path": path_json(&root),
                "layout": layout,
                "file": path_json(file),
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
        if let Some(exclude) = excluded {
            println!("  .loadout/ is hidden from git in {}", exclude.display());
        }
        if let Some(report) = report {
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
    let commit = projects::add(&store, &ctx.apply_env(), &project)?;
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
    let overlay = overlay(ctx)?;
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
    let removed = projects::remove(&store, &ctx.apply_env(), id)?;
    let (report, commit) = (&removed.report, &removed.commit);
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "project": id,
            "path": removed.root.as_deref().map(path_json),
            "commit": commit.as_ref().map(CommitId::to_string),
            "apply": report.as_ref().map(|r| report_json(r, commit.as_ref())),
        }));
    } else {
        println!(
            "Removed project {id} from this machine (commit {}).",
            short(commit.as_ref())
        );
        if let Some(report) = report {
            print_report(report, commit.as_ref());
        }
    }
    Ok(report.as_ref().map_or(0, apply_exit_code))
}

fn trust(ctx: &Context, path: &Path) -> Result<u8, CoreError> {
    let root = project_root(ctx, path)?;
    let fs = ctx.fs();
    let project = load_project(fs, &root)?;
    warn_all(&project.warnings);
    let store = ctx.open_store_for_change()?;
    let trust_store = TrustStore::load(fs, &ctx.home.state_dir())?;
    let reviews = projects::review(&store, &project, &trust_store)?;
    if !ctx.json {
        for review in &reviews {
            match review.change {
                ManifestChange::New => {
                    println!("=== {} (new)", review.path.display());
                    print!("{}", review.content.as_deref().unwrap_or_default());
                }
                ManifestChange::Changed => print!("{}", review.diff.as_deref().unwrap_or_default()),
                change => println!("=== {} ({})", review.path.display(), change.as_str()),
            }
        }
    }
    let id = project.id.clone();
    let (report, commit) = projects::trust(&store, &ctx.apply_env(), &project, None)?;
    if ctx.json {
        print_json(&json!({
            "schema": JSON_SCHEMA,
            "project": project_json(fs, &id, &root),
            "files": reviews.iter().map(|review| json!({
                "file": review.file,
                "path": path_json(&review.path),
                "change": review.change.as_str(),
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
    // The instant apply covers every project; another project waiting for
    // trust is not this command's failure (#10).
    Ok(report
        .as_ref()
        .map_or(0, |report| status_exit_code(report.project_status(&id))))
}

fn share(ctx: &Context, id: &str, servers: &[String]) -> Result<u8, CoreError> {
    ids::check(IdKind::Project, id)?;
    let fs = ctx.fs();
    let store = require_store(ctx, "no project is known")?;
    let overlay = this_overlay(&store, fs, &ctx.home.state_dir())?;
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
    let ax = ctx.apply_env();
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
        let touched = Some(Touched::Project(root.clone()));
        let done =
            projects::commit_project(&store, &ax, "project.share", summary, id, touched, |tx| {
                mirror(tx, &project)?;
                save_trust(&ax, &project)
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

/// This machine's overlay, when the store and the machine exist.
fn overlay(ctx: &Context) -> Result<Option<MachineOverlay>, CoreError> {
    match ctx.open_store()? {
        Some(store) => this_overlay(&store, ctx.fs(), &ctx.home.state_dir()),
        None => Ok(None),
    }
}

/// The project folder of `path`: the nearest folder at or above it holding
/// a manifest, spelled as this machine's overlay records it.
fn project_root(ctx: &Context, path: &Path) -> Result<PathBuf, CoreError> {
    let folder = folder(Some(path))?;
    let store: Option<Store<'_>> = ctx.open_store()?;
    projects::project_root(store.as_ref(), &ctx.apply_env(), &folder)
}

/// `private`, `shared` or `shared+local` from the manifest files present.
fn layout(fs: &dyn Fs, root: &Path) -> Option<&'static str> {
    projects::layout(fs, root).map(projects::Layout::as_str)
}

fn project_json(fs: &dyn Fs, id: &str, root: &Path) -> Value {
    json!({
        "id": id,
        "path": path_json(root),
        "layout": layout(fs, root),
    })
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

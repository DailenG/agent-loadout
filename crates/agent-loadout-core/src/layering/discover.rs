//! Discover projects (#3): folders the clients on this machine already
//! know, offered for the Projects page and `loadout project discover`.
//!
//! Sources, each a documented field of a client's own file (docs/adapters.md
//! gives the files' locations):
//!
//! - Claude Code: the keys of `projects` in `~/.claude.json`
//!   (`$CLAUDE_CONFIG_DIR/.claude.json`), the folders it was started in.
//! - Codex: `[projects."<folder>"]` tables of `config.toml` with
//!   `trust_level = "trusted"` (folders the user trusted in Codex).
//! - omp and Pi: the `cwd` of the session header (the first JSON line with
//!   `"type": "session"`) of the newest session file in each folder of
//!   `<agent dir>/sessions/`. Only the first lines are read.
//!
//! VS Code and Cursor keep recent workspaces in a SQLite database
//! (`state.vscdb`) whose keys are not documented, so they are not read.
//!
//! Discovery reads only. A candidate is a folder that exists, is not the
//! home folder, a drive root or inside Loadout's data directory, and is not
//! a project this machine knows yet. [`add`] binds the chosen ones in one
//! store commit and never trusts them (SPEC 17: trust stays an explicit
//! step).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::adapters::adapter;
use crate::env::Env;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::layering::manifest::manifest_dir;
use crate::layering::projects::{
    self, exclude_manifest_dir, load_project, prepare_init, set_known, this_overlay,
};
use crate::layering::{Project, TrustStore, known_projects, path_key, same_folder, same_path};
use crate::paths::LoadoutHome;
use crate::store::{CommitId, Operation, Scope, Store};

/// Most bytes read from the start of a session file.
const SESSION_HEAD_BYTES: u64 = 64 * 1024;
/// Most lines read from the start of a session file.
const SESSION_HEAD_LINES: usize = 8;

/// A folder the clients know that this machine does not know as a project.
#[derive(Debug, Clone)]
pub struct Discovered {
    /// Names the candidate in [`add`]: `d-` and 12 hex digits of the
    /// SHA-256 of the folder's [`path_key`], stable across scans.
    pub id: String,
    pub root: PathBuf,
    /// The folder's own name.
    pub name: String,
    /// Client ids that know the folder, in adapter order.
    pub clients: Vec<&'static str>,
    /// The folder's manifest, when it has one: adding binds it.
    pub manifest: Option<Box<Project>>,
    /// Why the folder cannot be added (a manifest that does not load).
    pub problem: Option<CoreError>,
}

/// What [`discover`] found.
#[derive(Debug, Clone, Default)]
pub struct Discovery {
    /// In folder order.
    pub candidates: Vec<Discovered>,
    /// Client files that could not be read or parsed (the rest still is).
    pub warnings: Vec<CoreError>,
}

/// The candidate id of a folder.
pub fn candidate_id(root: &Path) -> String {
    let digest = Sha256::digest(path_key(root).as_bytes());
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("d-{hex}")
}

/// Folders the clients know, minus the projects this machine knows.
/// Reads only.
pub fn discover(
    store: Option<&Store<'_>>,
    env: &Env,
    fs: &dyn Fs,
    home: &LoadoutHome,
) -> Result<Discovery, CoreError> {
    let mut warnings = Vec::new();
    let mut found: Vec<(&'static str, PathBuf)> = Vec::new();
    for (client, read) in SOURCES {
        match read(env, fs) {
            Ok(folders) => found.extend(folders.into_iter().map(|folder| (*client, folder))),
            Err(err) => warnings.push(err),
        }
    }

    let state = home.state_dir();
    let overlay = match store {
        Some(store) => this_overlay(store, fs, &state)?,
        None => None,
    };
    let trust = TrustStore::load(fs, &state)?;
    let known = known_projects(fs, env, overlay.as_ref(), &trust);
    let data = crate::layering::resolved(fs, &home.path);

    let mut by_key: BTreeMap<String, Discovered> = BTreeMap::new();
    for (client, root) in found {
        let key = path_key(&root);
        if let Some(candidate) = by_key.get_mut(&key) {
            if !candidate.clients.contains(&client) {
                candidate.clients.push(client);
            }
            continue;
        }
        let real = crate::layering::resolved(fs, &root);
        let inside_data = |path: &Path| {
            path.ancestors()
                .any(|ancestor| same_path(ancestor, &data) || same_path(ancestor, &home.path))
        };
        if !root.is_absolute()
            || !fs.is_dir(&root)
            || root.parent().is_none()
            || real.parent().is_none()
            || same_folder(fs, &root, env.home())
            || inside_data(&root)
            || inside_data(&real)
            || known
                .iter()
                .any(|project| same_folder(fs, &project.root, &root))
        {
            continue;
        }
        let (manifest, problem) = match Project::load(fs, &root) {
            Ok(project) => (project.map(Box::new), None),
            Err(err) => (None, Some(err)),
        };
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        by_key.insert(
            key,
            Discovered {
                id: candidate_id(&root),
                root,
                name,
                clients: vec![client],
                manifest,
                problem,
            },
        );
    }
    let order = |client: &str| SOURCES.iter().position(|(id, _)| *id == client);
    let mut candidates: Vec<Discovered> = by_key.into_values().collect();
    for candidate in &mut candidates {
        candidate.clients.sort_by_key(|client| order(client));
    }
    Ok(Discovery {
        candidates,
        warnings,
    })
}

type Reader = fn(&Env, &dyn Fs) -> Result<Vec<PathBuf>, CoreError>;

/// Each source, in adapter order.
static SOURCES: &[(&str, Reader)] = &[
    ("claude-code", claude_code),
    ("omp", omp_sessions),
    ("pi", pi_sessions),
    ("codex", codex),
];

fn omp_sessions(env: &Env, fs: &dyn Fs) -> Result<Vec<PathBuf>, CoreError> {
    sessions(env, fs, "omp")
}

fn pi_sessions(env: &Env, fs: &dyn Fs) -> Result<Vec<PathBuf>, CoreError> {
    sessions(env, fs, "pi")
}

/// The first user file of `client`'s adapter.
fn user_file(env: &Env, client: &str) -> Option<PathBuf> {
    adapter(client)?
        .mcp
        .as_ref()?
        .user
        .first()?
        .resolve(env, None)
}

fn read_text(fs: &dyn Fs, path: &Path) -> Result<Option<String>, CoreError> {
    if !fs.exists(path) {
        return Ok(None);
    }
    fs.read_to_string(path).map(Some).map_err(|err| {
        CoreError::new(ErrorCode::IoError, format!("cannot read file: {err}")).with_path(path)
    })
}

/// Claude Code: the keys of `projects` in `~/.claude.json`.
fn claude_code(env: &Env, fs: &dyn Fs) -> Result<Vec<PathBuf>, CoreError> {
    let Some(file) = user_file(env, "claude-code") else {
        return Ok(Vec::new());
    };
    let Some(text) = read_text(fs, &file)? else {
        return Ok(Vec::new());
    };
    let value: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|err| {
            CoreError::new(
                ErrorCode::ParseError,
                format!("the file does not parse: {err}"),
            )
            .with_path(&file)
        })?;
    Ok(value
        .get("projects")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flat_map(|projects| projects.keys())
        .map(PathBuf::from)
        .collect())
}

/// Codex: `[projects."<folder>"]` with `trust_level = "trusted"`.
fn codex(env: &Env, fs: &dyn Fs) -> Result<Vec<PathBuf>, CoreError> {
    let Some(file) = user_file(env, "codex") else {
        return Ok(Vec::new());
    };
    let Some(text) = read_text(fs, &file)? else {
        return Ok(Vec::new());
    };
    let table: toml::Table = text.parse().map_err(|err| {
        CoreError::new(
            ErrorCode::ParseError,
            format!("the file does not parse: {err}"),
        )
        .with_path(&file)
    })?;
    Ok(table
        .get("projects")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flatten()
        .filter(|(_, project)| {
            project.get("trust_level").and_then(toml::Value::as_str) == Some("trusted")
        })
        .map(|(folder, _)| PathBuf::from(folder))
        .collect())
}

/// omp and Pi: the session header's `cwd` of the newest session file in
/// each folder of `<agent dir>/sessions/`.
fn sessions(env: &Env, fs: &dyn Fs, client: &str) -> Result<Vec<PathBuf>, CoreError> {
    let Some(dir) = user_file(env, client)
        .as_deref()
        .and_then(Path::parent)
        .map(|agent| agent.join("sessions"))
    else {
        return Ok(Vec::new());
    };
    if !fs.is_dir(&dir) {
        return Ok(Vec::new());
    }
    let list = |dir: &Path| {
        fs.read_dir(dir).map_err(|err| {
            CoreError::new(ErrorCode::IoError, format!("cannot list directory: {err}"))
                .with_path(dir)
        })
    };
    let mut out = Vec::new();
    for folder in list(&dir)? {
        if !fs.is_dir(&folder) {
            continue;
        }
        // Session files start with their creation time: the newest sorts
        // last.
        let newest = list(&folder)?
            .into_iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .max();
        if let Some(file) = newest
            && let Some(cwd) = session_cwd(fs, &file)
        {
            out.push(cwd);
        }
    }
    Ok(out)
}

/// The `cwd` of the first `"type": "session"` line among the first lines
/// of `file`; `None` when there is none or the file cannot be read.
fn session_cwd(fs: &dyn Fs, file: &Path) -> Option<PathBuf> {
    crate::skills::confine(fs, file).ok()?;
    let reader = BufReader::new(std::fs::File::open(file).ok()?.take(SESSION_HEAD_BYTES));
    reader
        .lines()
        .take(SESSION_HEAD_LINES)
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
        .find(|value| value.get("type").and_then(serde_json::Value::as_str) == Some("session"))
        .and_then(|header| header.get("cwd")?.as_str().map(PathBuf::from))
}

/// One folder [`add`] bound.
#[derive(Debug, Clone)]
pub struct AddedProject {
    /// The candidate id.
    pub candidate: String,
    pub project_id: String,
    pub root: PathBuf,
    /// The folder had no manifest: a private `project.local.toml` was
    /// written (as `project init` does, without trusting it).
    pub created_manifest: bool,
}

/// One folder [`add`] could not bind.
#[derive(Debug, Clone)]
pub struct NotAdded {
    pub candidate: String,
    pub root: PathBuf,
    pub error: CoreError,
}

/// What [`add`] did.
#[derive(Debug, Clone, Default)]
pub struct DiscoveredAdded {
    pub commit: Option<CommitId>,
    pub added: Vec<AddedProject>,
    pub failed: Vec<NotAdded>,
}

/// Binds the discovered folders `ids` to this machine in one store commit,
/// as Add folder does, and trusts none of them (each waits for its review,
/// SPEC 17). A folder without a manifest gets a private
/// `project.local.toml` first (hidden from its repository, as `project
/// init` writes it); a folder that cannot be bound (its manifest does not
/// load, its project id is known at another folder) is reported and the
/// others are still added. An id no candidate has is `entry_changed`.
pub fn add(
    store: &Store<'_>,
    env: &Env,
    fs: &dyn Fs,
    home: &LoadoutHome,
    ids: &[String],
) -> Result<DiscoveredAdded, CoreError> {
    let discovery = discover(Some(store), env, fs, home)?;
    let mut chosen: Vec<&Discovered> = Vec::new();
    for id in ids {
        let candidate = discovery
            .candidates
            .iter()
            .find(|candidate| &candidate.id == id)
            .ok_or_else(|| {
                CoreError::new(
                    ErrorCode::EntryChanged,
                    format!(
                        "no discovered folder has id {id:?} (known now, or gone); discover again"
                    ),
                )
            })?;
        if !chosen.iter().any(|known| known.id == candidate.id) {
            chosen.push(candidate);
        }
    }

    let mut out = DiscoveredAdded::default();
    let mut loaded: Vec<(&Discovered, Project, bool)> = Vec::new();
    let mut created: Vec<(PathBuf, bool)> = Vec::new();
    for candidate in chosen {
        let fail = |error: CoreError| NotAdded {
            candidate: candidate.id.clone(),
            root: candidate.root.clone(),
            error,
        };
        if let Some(problem) = &candidate.problem {
            out.failed.push(fail(problem.clone()));
            continue;
        }
        if let Some(project) = &candidate.manifest {
            loaded.push((candidate, project.as_ref().clone(), false));
            continue;
        }
        match write_private_manifest(env, fs, &candidate.root) {
            Ok((project, dir_existed)) => {
                created.push((candidate.root.clone(), dir_existed));
                loaded.push((candidate, project, true));
            }
            Err(err) => out.failed.push(fail(err)),
        }
    }
    if loaded.is_empty() {
        return Ok(out);
    }

    let mut operation = Operation::new("project.add", "project: add");
    if let [(_, project, _)] = loaded.as_slice() {
        operation = operation.scope(Scope::Project(project.id.clone()));
    }
    let committed = store.transaction(operation, |tx| {
        let mut refused = Vec::new();
        let mut accepted = Vec::new();
        for (index, (_, project, _)) in loaded.iter().enumerate() {
            // `set_known` refuses before it stages anything.
            match set_known(tx, fs, &project.id, &project.root) {
                Ok(()) => {
                    tx.add_entity(format!("project/{}", project.id));
                    accepted.push(project.id.as_str());
                }
                Err(err) => refused.push((index, err)),
            }
        }
        tx.set_summary(match accepted.as_slice() {
            [id] => format!("project: add {id}"),
            _ => format!("project: add {} discovered projects", accepted.len()),
        });
        Ok(refused)
    });
    let committed = match committed {
        Ok(committed) => committed,
        Err(err) => {
            for (root, dir_existed) in &created {
                remove_manifest(fs, root, *dir_existed);
            }
            return Err(err);
        }
    };
    out.commit = committed.commit;
    let refused = committed.value;
    for (index, (candidate, project, wrote)) in loaded.into_iter().enumerate() {
        if let Some((_, err)) = refused.iter().find(|(at, _)| *at == index) {
            if wrote
                && let Some((root, dir_existed)) = created
                    .iter()
                    .find(|(root, _)| same_path(root, &project.root))
            {
                remove_manifest(fs, root, *dir_existed);
            }
            out.failed.push(NotAdded {
                candidate: candidate.id.clone(),
                root: candidate.root.clone(),
                error: err.clone(),
            });
            continue;
        }
        out.added.push(AddedProject {
            candidate: candidate.id.clone(),
            project_id: project.id,
            root: candidate.root.clone(),
            created_manifest: wrote,
        });
    }
    Ok(out)
}

/// Writes a private manifest in `root` as `project init` does (hidden from
/// the folder's repository) and loads it; whether `.loadout/` existed.
fn write_private_manifest(
    env: &Env,
    fs: &dyn Fs,
    root: &Path,
) -> Result<(Project, bool), CoreError> {
    let plan = prepare_init(env, fs, root, None)?;
    let dir = manifest_dir(root);
    let dir_existed = fs.is_dir(&dir);
    fs.create_dir_all(&dir).map_err(|err| {
        CoreError::new(
            ErrorCode::IoError,
            format!("cannot create directory: {err}"),
        )
        .with_path(&dir)
    })?;
    let file = dir.join(crate::layering::LOCAL_FILE);
    projects::write_file(fs, &file, plan.text.as_bytes())?;
    let loaded = exclude_manifest_dir(fs, root).and_then(|_| load_project(fs, root));
    match loaded {
        Ok(project) => Ok((project, dir_existed)),
        Err(err) => {
            remove_manifest(fs, root, dir_existed);
            Err(err)
        }
    }
}

/// Removes the private manifest [`write_private_manifest`] wrote, and
/// `.loadout/` when it was made for it and is empty now.
fn remove_manifest(fs: &dyn Fs, root: &Path, dir_existed: bool) {
    let dir = manifest_dir(root);
    let _ = fs.remove_file(&dir.join(crate::layering::LOCAL_FILE));
    if !dir_existed && fs.read_dir(&dir).is_ok_and(|entries| entries.is_empty()) {
        let _ = fs.remove_dir_all(&dir);
    }
}

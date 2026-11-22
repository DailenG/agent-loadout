//! The post-migration scan (SPEC 15.1): what the migrated servers and
//! skills meet in client files and skill folders. Read-only; nothing here
//! writes a client file.
//!
//! - [`adopt_identical`]: a client whose user file already holds an entry
//!   identical to a migrated server (SkillDock's `importedFromAppIds`, or
//!   an entry SkillDock wrote) gets the server enabled, so the first apply
//!   adopts it without a write.
//! - [`mcp_entries`]: every client entry of a migrated server, planned as
//!   the first apply would: adopted, created, a SkillDock wrapper to
//!   replace (`collision`), a different entry to review, or blocked (a
//!   secret to set first).
//! - [`library_links`]: links in client skill folders that point into
//!   SkillDock's library; they are the truth for skill enablement and are
//!   re-pointed to the store copy by the next skills sync.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::adapters::adapter;
use crate::adapters::mapper::WriteCx;
use crate::apply::ApplyEnv;
use crate::apply::import::{self, Candidate};
use crate::apply::ledger::{Ledger, LedgerKey};
use crate::apply::placeholder_path;
use crate::apply::plan::{self, Action, EntryState, PlanOptions, ScopeFilter};
use crate::apply::render::{DesiredState, native_name, render};
use crate::brand;
use crate::error::CoreError;
use crate::formats::NativeValue;
use crate::fs::Fs;
use crate::layering::same_path;
use crate::model::applied::Scope;
use crate::model::clients::ClientSelection;
use crate::paths::normalize;
use crate::secrets::delivery::Sharing;
use crate::secrets::scanner::SecretMemory;
use crate::skills::link::{CLIENT_SKILLS, LinkMethod, skills_dir};
use crate::skills::{Entry, entry};

use super::skilldock::wrapper_kind;
use super::{Item, ItemKind, ItemStatus, Offer};

/// What the scan knows about one migrated server.
#[derive(Debug, Clone, Default)]
pub struct MigratedServer {
    /// SkillDock's `managedAppIds`.
    pub managed: Vec<String>,
    /// SkillDock's `importedFromAppIds`.
    pub imported_from: Vec<String>,
}

fn enabled_for(selection: Option<&ClientSelection>, client: &str) -> bool {
    match selection {
        Some(ClientSelection::List(ids)) => ids.iter().any(|id| id == client),
        Some(ClientSelection::AllDetected) => true,
        None => false,
    }
}

fn is_read_only(client: &str) -> bool {
    adapter(client)
        .and_then(|spec| spec.mcp.as_ref())
        .is_none_or(|mcp| mcp.read_only)
}

/// `command` and `args` of a native entry, when it has them.
fn launch(value: &NativeValue) -> (Option<&str>, Vec<String>) {
    let Some(map) = value.as_object() else {
        return (None, Vec::new());
    };
    let command = map.get("command").and_then(NativeValue::as_str);
    let args = map
        .get("args")
        .and_then(NativeValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(|arg| arg.as_str().map(str::to_owned))
        .collect();
    (command, args)
}

/// The SkillDock wrapper a native entry is, if any.
pub fn native_wrapper(value: &NativeValue) -> Option<&'static str> {
    let (command, args) = launch(value);
    wrapper_kind(command, &args)
}

/// Enables migrated servers for every client whose user file holds an
/// identical foreign entry under the server's native name. Returns
/// `(server id, client)` pairs.
pub fn adopt_identical(
    ax: &ApplyEnv<'_>,
    memory: Arc<SecretMemory>,
    desired: &mut DesiredState,
    migrated: &BTreeMap<String, MigratedServer>,
    candidates: &[Candidate],
) -> Vec<(String, String)> {
    let cx = ax.render_cx(memory);
    let mut adopted = Vec::new();
    for candidate in candidates {
        for source in &candidate.sources {
            if is_read_only(source.client) {
                continue;
            }
            let Some(spec) = adapter(source.client) else {
                continue;
            };
            let Some(server) = desired
                .servers
                .values()
                .filter(|server| migrated.contains_key(&server.id))
                .find(|server| native_name(server, spec.id) == candidate.name)
            else {
                continue;
            };
            if enabled_for(
                server.enable.as_ref().and_then(|e| e.clients.as_ref()),
                spec.id,
            ) {
                continue;
            }
            let Ok(rendered) = render(&cx, desired, server, spec, Sharing::Private) else {
                continue;
            };
            let mcp = spec.mcp.as_ref().expect("rendered clients have MCP");
            let write = WriteCx {
                os: ax.env.os(),
                existing: Some(&source.value),
            };
            let Ok(native) = mcp.mapper.to_native(&rendered.server, &write) else {
                continue;
            };
            if mcp.mapper.canonical(&native) != mcp.mapper.canonical(&source.value) {
                continue;
            }
            let id = server.id.clone();
            let server = desired.servers.get_mut(&id).expect("found above");
            let enable = server.enable.get_or_insert_with(Default::default);
            match &mut enable.clients {
                Some(ClientSelection::List(ids)) => ids.push(spec.id.to_owned()),
                _ => enable.clients = Some(ClientSelection::List(vec![spec.id.to_owned()])),
            }
            adopted.push((id, spec.id.to_owned()));
        }
    }
    adopted
}

/// Foreign entries of every client's user file (no ledger record).
pub fn candidates(
    ax: &ApplyEnv<'_>,
    desired: &DesiredState,
    ledger: &Ledger,
) -> Result<(Vec<Candidate>, Vec<CoreError>), CoreError> {
    import::scan(ax, desired, ledger, None)
}

/// One report item per client entry of a migrated server, as the first
/// apply would plan it, plus foreign entries under a migrated server's name
/// that stay foreign.
#[allow(clippy::too_many_arguments)]
pub fn mcp_entries(
    ax: &ApplyEnv<'_>,
    memory: Arc<SecretMemory>,
    desired: &DesiredState,
    ledger: &Ledger,
    key: &LedgerKey,
    migrated: &BTreeMap<String, MigratedServer>,
    candidates: &[Candidate],
    adopted: &[(String, String)],
) -> Result<Vec<Item>, CoreError> {
    let mut items = Vec::new();
    if migrated.is_empty() {
        return Ok(items);
    }
    let options = PlanOptions {
        scope: ScopeFilter::System,
        ..PlanOptions::default()
    };
    let plan = plan::plan(&ax.render_cx(memory), desired, ledger, key, &options)?;
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for target in &plan.targets {
        let file = placeholder_path(ax.env, &target.file, None);
        let ours: Vec<_> = target
            .entries
            .iter()
            .filter(|entry| {
                entry
                    .server_id
                    .as_ref()
                    .is_some_and(|id| migrated.contains_key(id))
            })
            .collect();
        if ours.is_empty() {
            continue;
        }
        if let Some(problem) = &target.problem {
            items.push(Item::new(
                ItemKind::ClientEntry,
                target.clients.join(", "),
                ItemStatus::NeedsReview,
                format!("{file} cannot be planned: {}", problem.message()),
                Some(problem.code().as_str()),
            ));
            continue;
        }
        for entry in ours {
            let id = entry.server_id.clone().expect("filtered above");
            let client = entry.client.unwrap_or_default();
            seen.insert((client.to_owned(), entry.name.clone()));
            let name = format!("{client}/{}", entry.name);
            let info = &migrated[&id];
            let was_managed = info.managed.iter().any(|c| c == client);
            let was_found = adopted.iter().any(|(s, c)| s == &id && c == client);
            let (status, detail, code, offers) = match (entry.state, entry.action) {
                (_, Action::Adopt) => (
                    ItemStatus::Migrated,
                    format!(
                        "identical entry in {file}{}; recorded as adopted on the first apply, \
                         without a write",
                        if was_found && !was_managed {
                            " (found by the scan)"
                        } else {
                            ""
                        }
                    ),
                    None,
                    Vec::new(),
                ),
                (_, Action::Create) => (
                    ItemStatus::Migrated,
                    format!("written to {file} on the first apply"),
                    None,
                    Vec::new(),
                ),
                (EntryState::Synced, _) => (
                    ItemStatus::Migrated,
                    format!("already in place in {file}"),
                    None,
                    Vec::new(),
                ),
                (EntryState::Collision, _) => {
                    let adopt = Offer::ReplaceWrapper {
                        client: client.to_owned(),
                        name: entry.name.clone(),
                        command: format!("{} adopt {name}", brand::BIN_NAME),
                    };
                    match entry.observed.as_ref().and_then(native_wrapper) {
                        Some(kind) => (
                            ItemStatus::NeedsReview,
                            format!(
                                "{file} holds SkillDock's {kind} wrapper; replace it with the \
                                 native definition"
                            ),
                            Some("collision"),
                            vec![adopt],
                        ),
                        None => (
                            ItemStatus::NeedsReview,
                            format!(
                                "{file} holds a different entry{}; overwrite it, or keep the \
                                 client's version with `{} adopt {name} --theirs`",
                                if was_managed {
                                    " (SkillDock managed it)"
                                } else {
                                    ""
                                },
                                brand::BIN_NAME
                            ),
                            Some("collision"),
                            vec![Offer::ReviewEntry {
                                client: client.to_owned(),
                                name: entry.name.clone(),
                                command: format!("{} adopt {name}", brand::BIN_NAME),
                            }],
                        ),
                    }
                }
                (EntryState::Blocked, _) => {
                    let problem = entry
                        .problem
                        .as_ref()
                        .map_or_else(|| "blocked".to_owned(), |p| p.message().to_owned());
                    let code = entry.problem.as_ref().map(|p| p.code().as_str());
                    match entry.observed.as_ref().and_then(native_wrapper) {
                        // The wrapper can be replaced once the definition
                        // renders for this client.
                        Some(kind) => (
                            ItemStatus::NeedsReview,
                            format!(
                                "{file} holds SkillDock's {kind} wrapper; the native definition \
                                 cannot be written there yet: {problem}"
                            ),
                            code,
                            vec![Offer::ReplaceWrapper {
                                client: client.to_owned(),
                                name: entry.name.clone(),
                                command: format!("{} adopt {name}", brand::BIN_NAME),
                            }],
                        ),
                        None => (
                            ItemStatus::NeedsReview,
                            format!("not applied to {file} yet: {problem}"),
                            code,
                            Vec::new(),
                        ),
                    }
                }
                (state, action) => (
                    ItemStatus::NeedsReview,
                    format!(
                        "{file}: {} ({})",
                        state.as_str().replace('_', " "),
                        action.as_str()
                    ),
                    Some(state.as_str()),
                    Vec::new(),
                ),
            };
            let mut item = Item::new(ItemKind::ClientEntry, name, status, detail, code);
            item.offers = offers;
            items.push(item);
        }
    }
    // Entries under a migrated name in clients the server is not enabled
    // for: left foreign (SPEC 15.1 `importedFromAppIds`).
    for candidate in candidates {
        for source in &candidate.sources {
            if seen.contains(&(source.client.to_owned(), candidate.name.clone())) {
                continue;
            }
            let Some(server) = desired
                .servers
                .values()
                .filter(|server| migrated.contains_key(&server.id))
                .find(|server| native_name(server, source.client) == candidate.name)
            else {
                continue;
            };
            let file = placeholder_path(ax.env, &source.file, None);
            let name = format!("{}/{}", source.client, candidate.name);
            let detail = match native_wrapper(&source.value) {
                Some(kind) => format!(
                    "{file} holds SkillDock's {kind} wrapper for {:?}, which is not enabled for \
                     {}; enable it there and replace the wrapper, or remove the entry",
                    server.id, source.client
                ),
                None => format!(
                    "{file} holds a different {:?} entry; left foreign (enable the server for {} \
                     to manage it)",
                    server.id, source.client
                ),
            };
            items.push(Item::new(
                ItemKind::ClientEntry,
                name,
                ItemStatus::NeedsReview,
                detail,
                Some("foreign"),
            ));
        }
    }
    Ok(items)
}

/// A link in a client skill folder that points into SkillDock's library.
#[derive(Debug, Clone)]
pub struct FoundLink {
    pub client: &'static str,
    /// The link, absolute.
    pub path: PathBuf,
    /// The link with a placeholder root (`~/.claude/skills/<name>`).
    pub display: String,
    pub method: LinkMethod,
    /// Where it points (`~/.skilldock/skills/<name>`), absolute.
    pub target: PathBuf,
}

/// Links in the user skill folders of Loadout's skill clients (table 7.6)
/// that point at a folder directly in `library`, by that folder's name.
/// Read-only; links are not followed.
pub fn library_links(
    ax: &ApplyEnv<'_>,
    library: &Path,
) -> Result<BTreeMap<String, Vec<FoundLink>>, CoreError> {
    let fs: &dyn Fs = ax.fs;
    let mut out: BTreeMap<String, Vec<FoundLink>> = BTreeMap::new();
    for client in &CLIENT_SKILLS {
        let Ok(dir) = skills_dir(ax.env, client.client, Scope::User, None) else {
            continue;
        };
        if !fs.is_dir(&dir) {
            continue;
        }
        let Ok(mut paths) = fs.read_dir(&dir) else {
            continue;
        };
        paths.sort();
        for path in paths {
            let Ok(Entry::Link(target)) = entry(fs, &path) else {
                continue;
            };
            let target = normalize(&target);
            let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
                continue;
            };
            if !same_path(parent, library) {
                continue;
            }
            out.entry(name.to_string_lossy().into_owned())
                .or_default()
                .push(FoundLink {
                    client: client.client,
                    display: placeholder_path(ax.env, &path, None),
                    method: link_method(&path),
                    path,
                    target,
                });
        }
    }
    Ok(out)
}

#[cfg(windows)]
fn link_method(path: &Path) -> LinkMethod {
    if junction::exists(path).unwrap_or(false) {
        LinkMethod::Junction
    } else {
        LinkMethod::Symlink
    }
}

#[cfg(not(windows))]
fn link_method(_path: &Path) -> LinkMethod {
    LinkMethod::Symlink
}

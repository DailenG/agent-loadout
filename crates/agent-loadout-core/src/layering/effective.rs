//! The effective view (SPEC 6.4): for a folder `F` and a client `C`, every
//! server name `C` loads in `F`, which file it comes from and what it
//! shadows.
//!
//! Computed from the client files as they are plus the entries the plan
//! would create. A name is a candidate in every file and container `C`
//! reads: first `C`'s own layers in [`Caps::precedence`] order (the user
//! file is `system`, the shared project file `project`, the private target
//! `project-local`; an entry Loadout does not own is `foreign:<file>`),
//! then the other tools' files of [`Caps::compat_reads`]
//! (`client-compat:<file>`); a compatibility file that one host of `C`
//! prefers ([`CompatSource::outranks`]: VS Code's `.vscode/mcp.json` for
//! Copilot) comes right before the layer it outranks. The first candidate
//! wins and the others are shadowed. For a client whose own layers coexist
//! ([`ProjectSemantics::Coexist`]: Cursor), every live entry of its own
//! files is a loaded row of its own, and these rows carry
//! `duplicate_across_layers`. An entry the plan cannot write (`blocked`)
//! never wins over one that is there; a name only such entries hold is
//! reported with `loaded: false`.
//!
//! [`Caps::precedence`]: crate::adapters::caps::Caps::precedence
//! [`Caps::compat_reads`]: crate::adapters::caps::Caps::compat_reads

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use crate::adapters::caps::{CompatSource, Outrank, ProjectDisable, ProjectSemantics, Source};
use crate::adapters::detect::replacing_extension;
use crate::adapters::{ADAPTERS, AdapterSpec, adapter};
use crate::apply::cache::CachedPlan;
use crate::apply::exec::PlanInputs;
use crate::apply::ledger::{EntryKind, LedgerEntry, container_pointer};
use crate::apply::plan::{
    self, Action, EntryPlan, EntryState, Plan, PlanOptions, ProjectSelection, ScopeFilter,
    TargetPlan, client_enable_required, list_members, read_target, unset_not_expressible,
};
use crate::apply::render::{Detected, RenderCx, server_transport, writable};
use crate::apply::{ApplyEnv, placeholder_path};
use crate::env::Os;
use crate::error::{CoreError, ErrorCode};
use crate::formats::{Document, Format, NativeValue};
use crate::layering::merge::{EntryBody, ProjectEntry, project_entries, suppression_mechanism};
use crate::layering::{Project, TrustStore, discover, same_path, trust_required};
use crate::model::applied::Scope;
use crate::paths::STATE_DIR;
use crate::store::Store;

/// Where an entry a client loads lives (SPEC 6.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layer {
    /// The client's user file, an entry Loadout owns.
    System,
    /// The client's shared project file, an entry Loadout owns.
    Project,
    /// The client's private project target, an entry Loadout owns.
    ProjectLocal,
    /// An entry Loadout does not own in one of the client's files (the
    /// file, as in [`Winner::file`]).
    Foreign(String),
    /// An entry the client reads from another tool's file.
    ClientCompat(String),
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Layer::System => f.write_str("system"),
            Layer::Project => f.write_str("project"),
            Layer::ProjectLocal => f.write_str("project-local"),
            Layer::Foreign(file) => write!(f, "foreign:{file}"),
            Layer::ClientCompat(file) => write!(f, "client-compat:{file}"),
        }
    }
}

impl Serialize for Layer {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// The entry a client uses for a name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Winner {
    pub layer: Layer,
    /// The file with a placeholder root (`~/.claude.json`, `<P>/.mcp.json`).
    pub file: String,
    /// `store:<id>` (a system entry), `store:<id>+project` (a project
    /// override with `from`, or a suppression), `project:<name>` (defined
    /// by the project only) or `foreign`.
    pub source: String,
}

/// A same-named entry the winner hides (or, for a client that merges
/// project entries field by field, merges over).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Shadowed {
    pub layer: Layer,
    pub file: String,
}

/// One server name a client loads, or would load, in the folder.
#[derive(Debug, Clone, Serialize)]
pub struct EffectiveEntry {
    pub name: String,
    /// Whether the client starts it: not suppressed, not disabled, and not
    /// only planned in a blocked target.
    pub loaded: bool,
    pub winner: Winner,
    pub shadowed: Vec<Shadowed>,
    /// Other clients with an adapter that read the winner's file and
    /// container (SPEC 6.5), by id.
    pub also_visible_to: Vec<&'static str>,
    /// The winner's plan state, when Loadout plans its file; `foreign` for
    /// an entry it does not own.
    #[serde(serialize_with = "state_json")]
    pub status: Option<EntryState>,
    /// `duplicate_via_compat`, `duplicate_across_layers`,
    /// `cannot_suppress_in_project`,
    /// `unset_not_expressible`, `trust_required`,
    /// `client_mcp_replaced_by_extension`, `client_enable_required` and the
    /// problems the plan has with this name's entries.
    #[serde(serialize_with = "warnings_json")]
    pub warnings: Vec<CoreError>,
    /// The winner's native entry as its file writes it, under the
    /// innermost container key, with the values the plan writes or held
    /// inline masked ([`TargetPlan::snippet`]); `None` when it cannot be
    /// rendered. Other literals are the caller's to mask before showing
    /// it. Not part of the JSON view.
    #[serde(skip)]
    pub snippet: Option<String>,
}

/// The project of the folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectRef {
    pub id: String,
    pub root: PathBuf,
}

/// The effective view of a folder.
#[derive(Debug, Clone, Serialize)]
pub struct EffectiveView {
    /// The nearest project at or above the folder, if any.
    pub project: Option<ProjectRef>,
    /// Entries per client id, by name; a client whose own layers coexist
    /// (Cursor) has one row per file for a name in several of them.
    pub clients: BTreeMap<&'static str, Vec<EffectiveEntry>>,
    /// Files that could not be read or parsed, and other problems outside
    /// any entry.
    #[serde(skip)]
    pub warnings: Vec<CoreError>,
}

fn state_json<S: Serializer>(state: &Option<EntryState>, serializer: S) -> Result<S::Ok, S::Error> {
    match state {
        Some(state) => serializer.serialize_str(state.as_str()),
        None => serializer.serialize_none(),
    }
}

fn warnings_json<S: Serializer>(warnings: &[CoreError], serializer: S) -> Result<S::Ok, S::Error> {
    struct Warning<'a>(&'a CoreError);
    impl Serialize for Warning<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut out = serializer.serialize_struct("Warning", 2)?;
            out.serialize_field("code", self.0.code().as_str())?;
            out.serialize_field("message", self.0.message())?;
            out.end()
        }
    }
    serializer.collect_seq(warnings.iter().map(Warning))
}

/// The effective view of `folder` for `clients` (every client with MCP
/// support on this OS that Loadout writes when `None`). Plans the user
/// files and, when the folder's project is known on this machine, that
/// project, against the store's working tree; reads files and never writes
/// anything.
///
/// Errors: `unknown_client`, `no_adapter` or `not_supported_on_os` for a
/// requested client; an invalid manifest; a store that does not load.
pub fn effective(
    store: Option<&Store<'_>>,
    ax: &ApplyEnv<'_>,
    folder: &Path,
    clients: Option<&[String]>,
) -> Result<EffectiveView, CoreError> {
    let specs = selected(ax, clients)?;
    let inputs = PlanInputs::load(store, ax)?;
    view_of(ax, &inputs, specs, folder, None)
}

/// [`effective`] from a plan of every scope and project made already (the
/// app's cached plan, [`crate::apply::cache`]), so the view plans nothing.
pub fn effective_planned(
    ax: &ApplyEnv<'_>,
    cached: &CachedPlan,
    folder: &Path,
    clients: Option<&[String]>,
) -> Result<EffectiveView, CoreError> {
    let specs = selected(ax, clients)?;
    view_of(ax, &cached.inputs, specs, folder, Some(&cached.plan))
}

/// The clients of a view: `clients`, or every client with MCP support on
/// this OS that Loadout writes.
fn selected(
    ax: &ApplyEnv<'_>,
    clients: Option<&[String]>,
) -> Result<Vec<&'static AdapterSpec>, CoreError> {
    let os = ax.env.os();
    match clients {
        Some(ids) => ids.iter().map(|id| writable(id, ax.env)).collect(),
        None => Ok(ADAPTERS
            .iter()
            .copied()
            .filter(|spec| on_os(spec, os) && !read_only(spec))
            .collect()),
    }
}

/// The view of `folder` for `specs` from `inputs`: from `full`, a plan of
/// every scope and project, when given; else from one planner run for the
/// user files and the folder's project.
fn view_of(
    ax: &ApplyEnv<'_>,
    inputs: &PlanInputs,
    specs: Vec<&'static AdapterSpec>,
    folder: &Path,
    full: Option<&Plan>,
) -> Result<EffectiveView, CoreError> {
    let cx = ax.render_cx(inputs.memory.clone());
    let mut warnings = Vec::new();
    // The project, at the path this machine knows it by when it does (the
    // overlay's spelling), so that planned files compare equal; and the
    // ids this machine knows at that folder.
    let mut known: Vec<&str> = Vec::new();
    let project = match discover(ax.env, ax.fs, folder) {
        Some(found) => {
            let projects = inputs
                .desired
                .overlay
                .as_ref()
                .and_then(|overlay| overlay.projects.as_ref());
            let root = projects.and_then(|projects| {
                projects
                    .values()
                    .map(|project| PathBuf::from(&project.path))
                    .find(|path| same_path(path, &found))
            });
            if let (Some(root), Some(projects)) = (&root, projects) {
                known = projects
                    .iter()
                    .filter(|(_, project)| same_path(Path::new(&project.path), root))
                    .map(|(id, _)| id.as_str())
                    .collect();
            }
            Project::load(ax.fs, root.as_deref().unwrap_or(&found))?
        }
        None => None,
    };

    let mut selections = vec![PlanOptions {
        scope: ScopeFilter::System,
        ..PlanOptions::default()
    }];
    if let Some(project) = project.as_ref().filter(|_| !known.is_empty()) {
        selections.push(PlanOptions {
            scope: ScopeFilter::Project,
            projects: ProjectSelection::Only(project.root.clone()),
            ..PlanOptions::default()
        });
    }
    let plans = match full {
        Some(_) => Vec::new(),
        None => plan::plan_each(
            &cx,
            &inputs.desired,
            &inputs.ledger,
            &inputs.key,
            &selections,
        )?,
    };
    let planned: Vec<&TargetPlan> = match full {
        // What the two plans hold, in the same order (user files first):
        // the user targets, and those of the folder's project, which a plan
        // of the folder selects by the ids known there.
        Some(full) => full
            .targets
            .iter()
            .filter(|target| {
                target.scope == Scope::User
                    || (selections.len() > 1
                        && target
                            .project_id
                            .as_deref()
                            .is_some_and(|id| known.contains(&id)))
            })
            .collect(),
        None => plans.iter().flat_map(|plan| &plan.targets).collect(),
    };
    let mut entries = Vec::new();
    let mut trust = None;
    if let Some(project) = &project {
        warnings.extend(project.warnings.iter().cloned());
        let state_dir = ax.home.path.join(STATE_DIR);
        let store = TrustStore::load(ax.fs, &state_dir).unwrap_or_else(|err| {
            warnings.push(err);
            TrustStore::default()
        });
        trust = trust_required(project, &project.root, store.state(project));
        entries = project_entries(
            project,
            &inputs.desired.servers,
            ax.env,
            inputs.desired.overlay.as_ref(),
        );
    }

    let mut view = View {
        cx: &cx,
        inputs,
        root: project.as_ref().map(|project| project.root.as_path()),
        planned,
        owners: Owners::of(&inputs.ledger.entries),
        entries: &entries,
        trust,
        docs: BTreeMap::new(),
        displayed: RefCell::new(HashMap::new()),
        detected: Detected::default(),
        readers: None,
        warnings,
    };
    let mut out = BTreeMap::new();
    for spec in specs {
        let rows = view.client(spec);
        out.insert(spec.id, rows);
    }
    let warnings = view.warnings;
    Ok(EffectiveView {
        project: project.map(|project| ProjectRef {
            id: project.id,
            root: project.root,
        }),
        clients: out,
        warnings,
    })
}

fn on_os(spec: &AdapterSpec, os: Os) -> bool {
    spec.mcp.as_ref().is_some_and(|mcp| mcp.oses.contains(&os))
}

/// A client Loadout only reads (`vscode-legacy`): never a row of its own,
/// only a compatibility source of others.
fn read_only(spec: &AdapterSpec) -> bool {
    spec.mcp.as_ref().is_some_and(|mcp| mcp.read_only)
}

const fn scope_of(source: Source) -> Scope {
    match source {
        Source::User => Scope::User,
        Source::Project => Scope::Project,
        Source::ProjectLocal => Scope::ProjectLocal,
    }
}

/// One file and container a client reads entries from.
struct SourceFile {
    scope: Scope,
    /// Another tool's file (SPEC 6.4 `client-compat`).
    compat: bool,
    /// Where another tool's file ranks, when above one of the client's own
    /// layers.
    outrank: Option<Outrank>,
    file: PathBuf,
    format: Format,
    keys: Vec<String>,
    pointer: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Presence {
    /// In the file now.
    File,
    /// Not there yet; the plan creates it.
    Planned,
    /// Not there; the plan cannot write it.
    Blocked,
}

/// One entry of a name in one source.
struct Candidate {
    layer: Layer,
    file: PathBuf,
    pointer: String,
    compat: bool,
    outrank: Option<Outrank>,
    presence: Presence,
    value: Option<NativeValue>,
    source: String,
    status: Option<EntryState>,
    problem: Option<CoreError>,
    snippet: Option<String>,
}

/// What a client's rows share, by entry name.
struct ClientCx {
    /// Names hidden through a list (Claude Code `disabledMcpServers`).
    suppressed: Vec<String>,
    member_problems: Vec<(String, CoreError)>,
    project_warnings: Vec<(String, CoreError)>,
    /// `client_mcp_replaced_by_extension`, for every row.
    extension: Option<CoreError>,
}

/// A client id and the files and containers (pointers) it reads.
type Readers = Vec<(&'static str, Vec<(PathBuf, String)>)>;

/// The first ledger entry of each file, container and key, whatever its
/// slot: one pass over the ledger instead of one per candidate (#14).
struct Owners<'a>(HashMap<(&'a str, &'a str, &'a str), &'a LedgerEntry>);

impl<'a> Owners<'a> {
    /// Server entries (not list members) of `entries`.
    fn of(entries: &'a [LedgerEntry]) -> Self {
        let mut map = HashMap::new();
        for row in entries.iter().filter(|row| row.kind == EntryKind::McpEntry) {
            map.entry((row.file.as_str(), row.container.as_str(), row.key.as_str()))
                .or_insert(row);
        }
        Self(map)
    }

    fn get(&self, file: &str, container: &str, key: &str) -> Option<&'a LedgerEntry> {
        self.0.get(&(file, container, key)).copied()
    }
}

struct View<'a> {
    cx: &'a RenderCx<'a>,
    inputs: &'a PlanInputs,
    root: Option<&'a Path>,
    /// The planned targets of the user files and the folder's project.
    planned: Vec<&'a TargetPlan>,
    owners: Owners<'a>,
    entries: &'a [ProjectEntry],
    /// `trust_required` when the folder's project is not trusted as it is.
    trust: Option<CoreError>,
    /// Parsed files; `None` for a file that does not exist or does not
    /// parse.
    docs: BTreeMap<PathBuf, Option<Document>>,
    /// [`View::display`] of each file shown so far.
    displayed: RefCell<HashMap<PathBuf, String>>,
    detected: Detected,
    /// The files and containers each client reads (also visible to).
    readers: Option<Readers>,
    warnings: Vec<CoreError>,
}

impl<'a> View<'a> {
    /// The entries of `spec`, by name.
    fn client(&mut self, spec: &'static AdapterSpec) -> Vec<EffectiveEntry> {
        let mcp = spec
            .mcp
            .as_ref()
            .expect("selected clients have MCP support");
        let sources = self.sources(spec, true);
        let mut rows: BTreeMap<String, Vec<Candidate>> = BTreeMap::new();
        for source in &sources {
            self.candidates(source, &mut rows);
        }
        let (suppressed, member_problems) = self.suppressed(spec);
        let has_project_files = !mcp.project.is_empty() || !mcp.project_local.is_empty();
        let cx = ClientCx {
            suppressed,
            member_problems,
            project_warnings: self.project_warnings(spec, has_project_files),
            // An extension replacing the client's MCP support (Pi
            // `pi-mcp-adapter`), installed for the user or for this project.
            extension: replacing_extension(spec, self.cx.env, self.cx.fs, self.root),
        };
        let coexist = mcp.caps.project_semantics == ProjectSemantics::Coexist;

        let mut out = Vec::with_capacity(rows.len());
        for (name, candidates) in rows {
            let live = |candidate: &&Candidate| candidate.presence != Presence::Blocked;
            let first = candidates.iter().find(live).unwrap_or(&candidates[0]);
            // The entries the client runs: the first live one and, for a
            // client whose own layers coexist (Cursor), every other live
            // entry of its own files too, each as a server of its own.
            let mut heads = vec![first];
            if coexist {
                heads.extend(
                    candidates
                        .iter()
                        .filter(live)
                        .filter(|candidate| !candidate.compat && !std::ptr::eq(*candidate, first)),
                );
            }
            for (index, winner) in heads.iter().copied().enumerate() {
                // A further head is a row of its own; the first row holds
                // every other candidate.
                let group: Vec<&Candidate> = if index == 0 {
                    candidates
                        .iter()
                        .filter(|candidate| {
                            !heads[1..]
                                .iter()
                                .any(|head| std::ptr::eq(*head, *candidate))
                        })
                        .collect()
                } else {
                    vec![winner]
                };
                let entry = self.row(spec, &cx, &name, winner, &group, &heads);
                out.push(entry);
            }
        }
        out
    }

    /// The row of `name` whose entry `winner` the client runs; `group` holds
    /// it and the candidates it shadows (or blocks), `heads` every entry of
    /// the name the client runs.
    fn row(
        &mut self,
        spec: &'static AdapterSpec,
        cx: &ClientCx,
        name: &str,
        winner: &Candidate,
        group: &[&Candidate],
        heads: &[&Candidate],
    ) -> EffectiveEntry {
        let mcp = spec
            .mcp
            .as_ref()
            .expect("selected clients have MCP support");
        let live: Vec<&Candidate> = group
            .iter()
            .copied()
            .filter(|candidate| candidate.presence != Presence::Blocked)
            .collect();
        let enabled = if mcp.caps.project_semantics == ProjectSemantics::DeepMerge {
            // Fields merge across the client's own layers: the highest one
            // that sets `enabled` decides.
            live.iter()
                .filter(|candidate| !candidate.compat)
                .find_map(|candidate| enabled_of(candidate.value.as_ref()))
                .or_else(|| enabled_of(winner.value.as_ref()))
        } else {
            enabled_of(winner.value.as_ref())
        };
        let loaded = winner.presence != Presence::Blocked
            && enabled != Some(false)
            && !cx.suppressed.iter().any(|member| member == name);

        let mut warnings: Vec<CoreError> = Vec::new();
        let native = live.iter().find(|candidate| !candidate.compat);
        let compat = live.iter().find(|candidate| candidate.compat);
        if let (Some(native), Some(compat)) = (native, compat) {
            let message = match winner.outrank.filter(|_| winner.compat) {
                // One host runs the compatibility entry, the others the
                // client's own (VS Code and the Copilot CLI, gate V2 VSL-3).
                Some(outrank) => format!(
                    "{} runs {name:?} from {} over the same-named entry in {}; {} reads only \
                     {} of the two and runs that entry",
                    outrank.host,
                    self.display(&winner.file),
                    self.display(&native.file),
                    outrank.others,
                    self.display(&native.file)
                ),
                None => format!(
                    "{} loads {name:?} from {} and also, as a compatibility source, from {}",
                    spec.display_name,
                    self.display(&native.file),
                    self.display(&compat.file)
                ),
            };
            push_unique(
                &mut warnings,
                CoreError::new(ErrorCode::DuplicateViaCompat, message).with_path(&compat.file),
            );
        }
        if heads.len() > 1 {
            let files: Vec<String> = heads.iter().map(|head| self.display(&head.file)).collect();
            push_unique(
                &mut warnings,
                CoreError::new(
                    ErrorCode::DuplicateAcrossLayers,
                    format!(
                        "{} runs a separate server named {name:?} from each of {}: same-named \
                         entries of its own files do not replace each other",
                        spec.display_name,
                        files.join(" and ")
                    ),
                )
                .with_path(&winner.file),
            );
        }
        for problem in group.iter().filter_map(|c| c.problem.as_ref()) {
            push_unique(&mut warnings, problem.clone());
        }
        for (member, problem) in &cx.member_problems {
            if member == name {
                push_unique(&mut warnings, problem.clone());
            }
        }
        for (entry, warning) in &cx.project_warnings {
            if entry == name {
                push_unique(&mut warnings, warning.clone());
            }
        }
        if let Some(extension) = &cx.extension {
            push_unique(&mut warnings, extension.clone());
        }
        // Loadout's own project entry of a client that lists new project
        // servers as disabled (Cursor): "loaded" assumes the user enabled it
        // there once.
        if loaded
            && !winner.compat
            && matches!(winner.layer, Layer::Project | Layer::ProjectLocal)
            && let Some(hint) = client_enable_required(spec, &[name], &winner.file)
        {
            push_unique(&mut warnings, hint);
        }

        let shadowed = live
            .iter()
            .filter(|candidate| !std::ptr::eq(**candidate, winner))
            .map(|candidate| Shadowed {
                layer: candidate.layer.clone(),
                file: self.display(&candidate.file),
            })
            .collect();
        let also_visible_to = self.readers_of(spec.id, &winner.file, &winner.pointer);
        EffectiveEntry {
            name: name.to_owned(),
            loaded,
            winner: Winner {
                layer: winner.layer.clone(),
                file: self.display(&winner.file),
                source: winner.source.clone(),
            },
            shadowed,
            also_visible_to,
            status: winner.status,
            warnings,
            snippet: winner.snippet.clone(),
        }
    }

    /// The files and containers `spec` reads, highest precedence first:
    /// its own layers, each after the compatibility sources that outrank
    /// it, then its other compatibility sources (those with an adapter in
    /// this build). Problems are reported when `warn`.
    fn sources(&mut self, spec: &'static AdapterSpec, warn: bool) -> Vec<SourceFile> {
        let Some(mcp) = spec.mcp.as_ref() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for source in mcp.caps.precedence {
            for compat in mcp.caps.compat_reads {
                if compat
                    .outranks
                    .is_some_and(|outrank| outrank.source == *source)
                {
                    self.push_compat(compat, &mut out);
                }
            }
            self.push_sources(spec, scope_of(*source), false, None, warn, &mut out);
        }
        for compat in mcp.caps.compat_reads {
            if compat.outranks.is_none() {
                self.push_compat(compat, &mut out);
            }
        }
        out
    }

    /// Adds the files and containers of compatibility source `compat`.
    fn push_compat(&mut self, compat: &CompatSource, out: &mut Vec<SourceFile>) {
        if let Some(other) = adapter(compat.client) {
            let scope = scope_of(compat.source);
            self.push_sources(other, scope, true, compat.outranks, false, out);
        }
    }

    /// Adds the files and containers of `spec` in `scope`: another tool's
    /// file when `compat`, ranked as `outrank` says.
    fn push_sources(
        &mut self,
        spec: &'static AdapterSpec,
        scope: Scope,
        compat: bool,
        outrank: Option<Outrank>,
        warn: bool,
        out: &mut Vec<SourceFile>,
    ) {
        let os = self.cx.env.os();
        let Some(mcp) = spec.mcp.as_ref().filter(|_| on_os(spec, os)) else {
            return;
        };
        let root = match (scope, self.root) {
            (Scope::User, _) => None,
            (_, Some(root)) => Some(root),
            (_, None) => return,
        };
        let prefs = self.inputs.desired.prefs(spec.id);
        let target = match spec.mcp_targets(self.cx.env, self.cx.fs, scope, root, &prefs) {
            // The first user file: the default profile (omp), never an MSIX
            // copy.
            Ok(mut targets) if !targets.is_empty() => targets.swap_remove(0),
            Ok(_) => return,
            Err(err) => {
                if warn {
                    push_unique(&mut self.warnings, err);
                }
                return;
            }
        };
        let legacy = match scope {
            Scope::ProjectLocal => &[][..],
            Scope::User | Scope::Project => mcp.legacy_containers,
        };
        for container in std::iter::once(target.container).chain(legacy.iter().copied()) {
            let keys = match container.keys(os, root) {
                Ok(keys) => keys,
                Err(err) => {
                    if warn {
                        push_unique(&mut self.warnings, err.with_path(&target.file));
                    }
                    continue;
                }
            };
            let pointer = container_pointer(&keys);
            if out
                .iter()
                .any(|source| source.file == target.file && source.pointer == pointer)
            {
                continue;
            }
            out.push(SourceFile {
                scope,
                compat,
                outrank,
                file: target.file.clone(),
                format: target.format,
                keys,
                pointer,
            });
        }
    }

    /// Adds the candidates of `source`: the entries in the file, then the
    /// entries the plan creates there or cannot write.
    fn candidates(&mut self, source: &SourceFile, rows: &mut BTreeMap<String, Vec<Candidate>>) {
        let present = self.entries_in(source);
        let file = source.file.to_string_lossy();
        let targets = self.targets(&source.file, &source.pointer, EntryKind::McpEntry);
        // The first planned entry of each name, by target, then entry.
        let mut planned_entries: HashMap<&str, (&TargetPlan, &EntryPlan)> = HashMap::new();
        for target in &targets {
            for entry in &target.entries {
                planned_entries
                    .entry(entry.name.as_str())
                    .or_insert((*target, entry));
            }
        }
        let planned_entry = |name: &str| planned_entries.get(name).copied();
        let display = self.display(&source.file);
        for (name, value) in &present {
            let planned = planned_entry(name);
            // Any slot: a file the client reads may be owned through another
            // client's scope (a compatibility source).
            let owned = self.owners.get(&file, &source.pointer, name);
            // Loadout's entry: owned, or desired here and equal to what is
            // there (adopted on apply).
            let ours: Option<(Scope, String)> = match (owned, planned) {
                (Some(row), _) => Some((
                    row.scope,
                    row.server_id.clone().unwrap_or_else(|| name.clone()),
                )),
                (None, Some((target, entry)))
                    if entry.desired.is_some() && entry.state != EntryState::Collision =>
                {
                    Some((
                        target.scope,
                        entry.server_id.clone().unwrap_or_else(|| name.clone()),
                    ))
                }
                _ => None,
            };
            let status = match (planned, &ours) {
                (Some((_, entry)), _) => Some(entry.state),
                (None, None) => Some(EntryState::Foreign),
                (None, Some(_)) => None,
            };
            // Masked by the file's plan when Loadout plans this file.
            let snippet = match planned
                .map(|(target, _)| target)
                .or(targets.first().copied())
            {
                Some(target) => target.snippet(name, value),
                None => unplanned_snippet(source, name, value),
            };
            rows.entry(name.clone()).or_default().push(Candidate {
                layer: layer(source, ours.is_some(), &display),
                file: source.file.clone(),
                pointer: source.pointer.clone(),
                compat: source.compat,
                outrank: source.outrank,
                presence: Presence::File,
                value: Some(value.clone()),
                source: ours.as_ref().map_or_else(
                    || "foreign".to_owned(),
                    |(scope, id)| self.source_name(*scope, id),
                ),
                status,
                problem: planned.and_then(|(_, entry)| entry.problem.clone()),
                snippet,
            });
        }
        for target in &targets {
            for entry in &target.entries {
                if present.iter().any(|(name, _)| *name == entry.name) {
                    continue;
                }
                let presence = if entry.action == Action::Create {
                    Presence::Planned
                } else if entry.state == EntryState::Blocked {
                    Presence::Blocked
                } else {
                    continue;
                };
                let id = entry.server_id.as_deref().unwrap_or(&entry.name);
                let snippet = entry
                    .written
                    .as_ref()
                    .and_then(|value| target.snippet(&entry.name, value));
                rows.entry(entry.name.clone()).or_default().push(Candidate {
                    layer: layer(source, true, &display),
                    file: source.file.clone(),
                    pointer: source.pointer.clone(),
                    compat: source.compat,
                    outrank: source.outrank,
                    presence,
                    value: entry.written.clone(),
                    source: self.source_name(target.scope, id),
                    status: Some(entry.state),
                    problem: entry.problem.clone(),
                    snippet,
                });
            }
        }
    }

    /// Reads and parses `file` once; a file that cannot be read or parsed
    /// is reported and counts as empty.
    fn load(&mut self, file: &Path, format: Format) {
        if self.docs.contains_key(file) {
            return;
        }
        let doc = read_target(self.cx.fs, file)
            .and_then(|bytes| {
                bytes
                    .map(|bytes| Document::parse(&bytes, format))
                    .transpose()
            })
            .unwrap_or_else(|err| {
                push_unique(&mut self.warnings, err.with_path(file));
                None
            });
        self.docs.insert(file.to_path_buf(), doc);
    }

    /// The entries in `source` now, in file order.
    fn entries_in(&mut self, source: &SourceFile) -> Vec<(String, NativeValue)> {
        self.load(&source.file, source.format);
        let Some(Some(doc)) = self.docs.get(&source.file) else {
            return Vec::new();
        };
        let keys: Vec<&str> = source.keys.iter().map(String::as_str).collect();
        let listed = doc.list(&keys).and_then(|names| {
            let mut out = Vec::with_capacity(names.len());
            for name in names {
                if let Some(value) = doc.get(&keys, &name)? {
                    out.push((name, value));
                }
            }
            Ok(out)
        });
        listed.unwrap_or_else(|err| {
            push_unique(&mut self.warnings, err.with_path(&source.file));
            Vec::new()
        })
    }

    /// The planned targets of `file` and `pointer` of `kind`.
    fn targets(&self, file: &Path, pointer: &str, kind: EntryKind) -> Vec<&'a TargetPlan> {
        self.planned
            .iter()
            .copied()
            .filter(|target| {
                target.file == file && target.pointer == pointer && target.kind == kind
            })
            .collect()
    }

    /// The `source` of an entry Loadout owns or plans for server `id` in
    /// `scope`.
    fn source_name(&self, scope: Scope, id: &str) -> String {
        if scope == Scope::User {
            return format!("store:{id}");
        }
        match self.entries.iter().find(|entry| entry.name == id) {
            Some(ProjectEntry {
                body: EntryBody::Suppression,
                ..
            }) => format!("store:{id}+project"),
            Some(ProjectEntry {
                from: Some(from), ..
            }) => format!("store:{from}+project"),
            _ => format!("project:{id}"),
        }
    }

    /// Names `spec` hides in the folder through a list (Claude Code
    /// `disabledMcpServers`): the members there now plus those the plan
    /// adds; and the problems of members the plan cannot add.
    fn suppressed(
        &mut self,
        spec: &'static AdapterSpec,
    ) -> (Vec<String>, Vec<(String, CoreError)>) {
        let mut members = Vec::new();
        let mut problems = Vec::new();
        let Some(root) = self.root else {
            return (members, problems);
        };
        let listed = spec
            .mcp
            .as_ref()
            .and_then(|mcp| mcp.caps.project_disable)
            .is_some_and(|mechanism| matches!(mechanism, ProjectDisable::ListMember(_)));
        if !listed {
            return (members, problems);
        }
        let prefs = self.inputs.desired.prefs(spec.id);
        let target = match spec.suppression_target(self.cx.env, self.cx.fs, root, &prefs) {
            Ok(Some(target)) => target,
            Ok(None) => return (members, problems),
            Err(err) => {
                push_unique(&mut self.warnings, err);
                return (members, problems);
            }
        };
        let keys = match target.container_keys() {
            Ok(keys) => keys,
            Err(err) => {
                push_unique(&mut self.warnings, err.with_path(&target.file));
                return (members, problems);
            }
        };
        let pointer = container_pointer(&keys);
        self.load(&target.file, target.format);
        if let Some(Some(doc)) = self.docs.get(&target.file) {
            let list: Vec<&str> = keys.iter().map(String::as_str).collect();
            match list_members(doc, &list) {
                Ok(listed) => members = listed,
                Err(err) => push_unique(&mut self.warnings, err.with_path(&target.file)),
            }
        }
        for target in self.targets(&target.file, &pointer, EntryKind::ListMember) {
            for entry in &target.entries {
                if entry.action == Action::Create && !members.contains(&entry.name) {
                    members.push(entry.name.clone());
                }
                if let Some(problem) = &entry.problem {
                    problems.push((entry.name.clone(), problem.clone()));
                }
            }
        }
        (members, problems)
    }

    /// Warnings the folder's project raises for `spec`, by entry name:
    /// `trust_required` and `unset_not_expressible` for a client with
    /// project files, `cannot_suppress_in_project` for a suppression it
    /// cannot express.
    fn project_warnings(
        &mut self,
        spec: &'static AdapterSpec,
        has_project_files: bool,
    ) -> Vec<(String, CoreError)> {
        let mut out = Vec::new();
        let Some(root) = self.root else {
            return out;
        };
        let entries = self.entries;
        for entry in entries {
            let transport = match &entry.body {
                EntryBody::Server(Ok(server)) => server_transport(server, self.cx.env),
                EntryBody::Server(Err(_)) => None,
                EntryBody::Suppression => self
                    .inputs
                    .desired
                    .servers
                    .get(&entry.name)
                    .and_then(|server| server_transport(server, self.cx.env)),
            };
            let selected = self
                .detected
                .select(self.cx, &self.inputs.desired, &entry.clients, transport)
                .iter()
                .any(|client| client == spec.id);
            if !selected {
                continue;
            }
            if let EntryBody::Suppression = entry.body
                && let Err(warning) = suppression_mechanism(spec, &entry.name)
            {
                out.push((entry.name.clone(), warning));
            }
            if !has_project_files {
                continue;
            }
            if let Some(trust) = &self.trust {
                out.push((entry.name.clone(), trust.clone()));
            }
            let deep_merge = spec
                .mcp
                .as_ref()
                .is_some_and(|mcp| mcp.caps.project_semantics == ProjectSemantics::DeepMerge);
            if deep_merge
                && !entry.unset.is_empty()
                && matches!(entry.body, EntryBody::Server(Ok(_)))
            {
                let prefs = self.inputs.desired.prefs(spec.id);
                let file = spec
                    .mcp_targets(self.cx.env, self.cx.fs, Scope::Project, Some(root), &prefs)
                    .ok()
                    .and_then(|targets| targets.into_iter().next())
                    .map_or_else(|| root.to_path_buf(), |target| target.file);
                out.extend(
                    unset_not_expressible(spec, entry, &file)
                        .map(|warning| (entry.name.clone(), warning)),
                );
            }
        }
        out
    }

    /// Other clients that read `file` and `pointer`, by id.
    fn readers_of(&mut self, client: &str, file: &Path, pointer: &str) -> Vec<&'static str> {
        if self.readers.is_none() {
            let os = self.cx.env.os();
            let mut readers = Vec::new();
            for spec in ADAPTERS.iter().copied().filter(|spec| on_os(spec, os)) {
                let files = self
                    .sources(spec, false)
                    .into_iter()
                    .map(|source| (source.file, source.pointer))
                    .collect();
                readers.push((spec.id, files));
            }
            self.readers = Some(readers);
        }
        let mut out: Vec<&'static str> = self
            .readers
            .iter()
            .flatten()
            .filter(|(id, files)| {
                *id != client
                    && files
                        .iter()
                        .any(|(other, at)| other == file && at == pointer)
            })
            .map(|(id, _)| *id)
            .collect();
        out.sort_unstable();
        out
    }

    /// `file` with a placeholder root (`~`, `<P>`, `%APPDATA%`).
    fn display(&self, file: &Path) -> String {
        self.displayed
            .borrow_mut()
            .entry(file.to_path_buf())
            .or_insert_with(|| placeholder_path(self.cx.env, file, self.root))
            .clone()
    }
}

/// The layer of an entry in `source`: Loadout's own (`ours`) by scope,
/// else foreign; any entry of another tool's file is `client-compat`.
fn layer(source: &SourceFile, ours: bool, display: &str) -> Layer {
    if source.compat {
        return Layer::ClientCompat(display.to_owned());
    }
    if !ours {
        return Layer::Foreign(display.to_owned());
    }
    match source.scope {
        Scope::User => Layer::System,
        Scope::Project => Layer::Project,
        Scope::ProjectLocal => Layer::ProjectLocal,
    }
}

/// The snippet of an entry in a file Loadout does not plan: rendered as
/// the file writes it under the innermost container key, unmasked.
fn unplanned_snippet(source: &SourceFile, name: &str, value: &NativeValue) -> Option<String> {
    let keys: Vec<&str> = source.keys.last().map(String::as_str).into_iter().collect();
    let mut doc = Document::empty(source.format);
    doc.upsert(&keys, name, value).ok()?;
    Some(String::from_utf8_lossy(&doc.to_bytes()).into_owned())
}

/// The `enabled` key of a native entry, when it holds a boolean.
fn enabled_of(value: Option<&NativeValue>) -> Option<bool> {
    match value?.as_object()?.get("enabled")? {
        NativeValue::Bool(enabled) => Some(*enabled),
        _ => None,
    }
}

fn push_unique(warnings: &mut Vec<CoreError>, warning: CoreError) {
    if !warnings
        .iter()
        .any(|w| w.code() == warning.code() && w.message() == warning.message())
    {
        warnings.push(warning);
    }
}

//! The planner (SPEC 8.3): desired state, native files and the ledger in;
//! per-file operations, the resulting text and a state per entry out.
//! Planning reads files and never writes anything.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use similar::TextDiff;

use crate::adapters::caps::{
    DisableMech, ProjectDisable, ProjectSemantics, TimeUnit, TimeoutField, TrustGate,
};
use crate::adapters::detect::replacing_extension;
use crate::adapters::mapper::{Canonical, EntryMapper, WriteCx};
use crate::adapters::spec::{ContainerPath, McpSpec};
use crate::adapters::{ADAPTERS, AdapterSpec, Target, adapter};
use crate::apply::ledger::{
    EntryKind, Ledger, LedgerEntry, LedgerKey, LedgerSlot, container_pointer,
};
use crate::apply::render::{
    DesiredEntry, DesiredState, Detected, RenderCx, RenderTarget, Rendering, native_name,
    render_for, system_entries, writable,
};
use crate::env::{Env, Os};
use crate::error::{CoreError, ErrorCode};
use crate::formats::{Document, NativeValue};
use crate::layering::manifest::normalize_path_text;
use crate::layering::{
    self, EntryBody, EntryLayer, KnownProject, Project, ProjectEntry, ProjectRepo, ProjectStatus,
    TrustStore,
};
use crate::model::applied::Scope;
use crate::model::clients::ClientSelection;
use crate::model::server::Server;
use crate::paths::STATE_DIR;
use crate::secrets::SecretValue;
use crate::secrets::delivery::Sharing;

/// What an entry is, compared with what is desired and what was written
/// (SPEC 8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryState {
    /// Native equals desired and equals what was written.
    Synced,
    PendingCreate,
    PendingUpdate,
    PendingRemove,
    /// Owned, changed since it was written, desired unchanged.
    Drifted,
    /// Owned and desired, but someone deleted it.
    Missing,
    /// Changed since it was written, and desired changed too.
    Conflict,
    /// Not owned; never touched.
    Foreign,
    /// A foreign entry where a desired entry should go.
    Collision,
    /// Cannot be planned; see [`EntryPlan::problem`].
    Blocked,
}

impl EntryState {
    pub const fn as_str(self) -> &'static str {
        match self {
            EntryState::Synced => "synced",
            EntryState::PendingCreate => "pending_create",
            EntryState::PendingUpdate => "pending_update",
            EntryState::PendingRemove => "pending_remove",
            EntryState::Drifted => "drifted",
            EntryState::Missing => "missing",
            EntryState::Conflict => "conflict",
            EntryState::Foreign => "foreign",
            EntryState::Collision => "collision",
            EntryState::Blocked => "blocked",
        }
    }

    /// States `loadout status --check` reports as pending or drift.
    pub const fn is_pending(self) -> bool {
        !matches!(
            self,
            EntryState::Synced | EntryState::Foreign | EntryState::Blocked
        )
    }
}

/// What the executor does with an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// Nothing.
    None,
    /// Write a new entry; the ledger records it as created.
    Create,
    /// Rewrite an owned entry.
    Update,
    /// Delete an owned, unchanged entry.
    Remove,
    /// Disable an owned entry natively (`enabled = false`).
    Disable,
    /// Record an identical foreign entry in the ledger; no write.
    Adopt,
    /// Overwrite a colliding foreign entry and record it as adopted.
    AdoptOverwrite,
    /// Record the current hash of an entry that already equals desired.
    Refresh,
    /// Drop the ledger record of an entry that no longer exists.
    Forget,
    /// Drop the ledger record and leave the entry to the user.
    Release,
    /// Turn the native entry into the store's desired state (a store edit);
    /// for a deleted entry, stop enabling the server for that client.
    Import,
    /// A decision is needed or was declined; nothing happens.
    Skip,
}

impl Action {
    pub const fn as_str(self) -> &'static str {
        match self {
            Action::None => "none",
            Action::Create => "create",
            Action::Update => "update",
            Action::Remove => "remove",
            Action::Disable => "disable",
            Action::Adopt => "adopt",
            Action::AdoptOverwrite => "adopt_overwrite",
            Action::Refresh => "refresh",
            Action::Forget => "forget",
            Action::Release => "release",
            Action::Import => "import",
            Action::Skip => "skip",
        }
    }

    /// Whether the action changes the file.
    pub const fn writes(self) -> bool {
        matches!(
            self,
            Action::Create
                | Action::Update
                | Action::Remove
                | Action::Disable
                | Action::AdoptOverwrite
        )
    }
}

/// How to resolve entries that need a decision (`collision`, `conflict`,
/// `drifted`, and `missing` for `import`): CLI `--on-conflict`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OnConflict {
    /// Leave them and report them as unresolved (exit code 4).
    #[default]
    Ask,
    /// Leave them; resolved by declining.
    Skip,
    /// Write desired over them (adopting a collision); remove a drifted
    /// orphan anyway.
    Overwrite,
    /// Take the native entry into the store; release a drifted orphan;
    /// for a deleted entry, respect the deletion.
    Import,
    /// Release ownership of a drifted or conflicting entry: the entry
    /// stays in the file as the user left it, Loadout forgets it, and a
    /// store server stops being enabled for that client (so the entry is
    /// foreign from then on); a drifted orphan is released. Not offered by
    /// `--on-conflict`; a per-entry decision (the drift resolver, SPEC
    /// 13.4). Collisions and deleted entries have nothing to release and
    /// are skipped.
    Release,
}

impl OnConflict {
    pub const fn as_str(self) -> &'static str {
        match self {
            OnConflict::Ask => "ask",
            OnConflict::Skip => "skip",
            OnConflict::Overwrite => "overwrite",
            OnConflict::Import => "import",
            OnConflict::Release => "release",
        }
    }
}

/// A resolution for one entry (`loadout adopt <client>/<name>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub client: String,
    pub name: String,
    pub resolution: OnConflict,
}

/// Which scopes a plan covers (CLI `--scope`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ScopeFilter {
    /// User files only.
    System,
    /// Project and project-local targets only.
    Project,
    #[default]
    All,
}

impl ScopeFilter {
    pub const fn as_str(self) -> &'static str {
        match self {
            ScopeFilter::System => "system",
            ScopeFilter::Project => "project",
            ScopeFilter::All => "all",
        }
    }

    const fn system(self) -> bool {
        matches!(self, ScopeFilter::System | ScopeFilter::All)
    }

    const fn project(self) -> bool {
        matches!(self, ScopeFilter::Project | ScopeFilter::All)
    }
}

/// Which projects a plan covers (CLI `apply [PATH]`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProjectSelection {
    /// User scope and every known project whose folder exists here; owned
    /// entries of projects this machine no longer knows are removed.
    #[default]
    All,
    /// Only the targets of the known project rooted at this folder.
    Only(PathBuf),
}

/// One target file a plan may be narrowed to (the GUI's "Apply selected").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetFilter {
    pub scope: Scope,
    /// The project folder of a project or project-local target; `None` for
    /// a user file and for the clean-up target of a project this machine no
    /// longer knows.
    pub project: Option<PathBuf>,
    pub file: PathBuf,
}

impl TargetFilter {
    fn matches(&self, target: &Target) -> bool {
        self.scope == target.scope
            && layering::same_path(&self.file, &target.file)
            && match (&self.project, &target.project) {
                (None, None) => true,
                (Some(a), Some(b)) => layering::same_path(a, b),
                _ => false,
            }
    }
}

/// Inputs that select and steer a plan.
#[derive(Debug, Clone, Default)]
pub struct PlanOptions {
    /// Only these clients; all when `None`.
    pub clients: Option<Vec<String>>,
    pub on_conflict: OnConflict,
    /// Per-entry resolutions; they win over [`PlanOptions::on_conflict`].
    pub decisions: Vec<Decision>,
    pub scope: ScopeFilter,
    pub projects: ProjectSelection,
    /// Only these target files (within the other filters); all when
    /// `None`.
    pub targets: Option<Vec<TargetFilter>>,
}

impl PlanOptions {
    fn resolution(&self, client: Option<&str>, name: &str) -> OnConflict {
        self.decisions
            .iter()
            .find(|decision| Some(decision.client.as_str()) == client && decision.name == name)
            .map_or(self.on_conflict, |decision| decision.resolution)
    }

    fn selects(&self, client: &str) -> bool {
        self.clients
            .as_ref()
            .is_none_or(|clients| clients.iter().any(|id| id == client))
    }

    fn selects_target(&self, target: &Target) -> bool {
        self.targets
            .as_ref()
            .is_none_or(|targets| targets.iter().any(|filter| filter.matches(target)))
    }

    /// Whether user files are planned.
    fn plans_user(&self) -> bool {
        self.scope.system() && self.projects == ProjectSelection::All
    }

    /// Whether the known project `project` is planned.
    fn plans_project(&self, project: &KnownProject) -> bool {
        self.scope.project()
            && match &self.projects {
                ProjectSelection::All => !matches!(project.status, ProjectStatus::Missing),
                ProjectSelection::Only(root) => layering::same_path(&project.root, root),
            }
    }
}

/// One entry of a target.
#[derive(Debug, Clone)]
pub struct EntryPlan {
    /// The native entry key.
    pub name: String,
    /// The client the entry is desired for or owned by; `None` for a
    /// foreign entry.
    pub client: Option<&'static str>,
    pub server_id: Option<String>,
    pub state: EntryState,
    pub action: Action,
    /// The entry needs a decision that was not given.
    pub unresolved: bool,
    /// Why the entry is blocked, or what it collides with.
    pub problem: Option<CoreError>,
    /// The entry as desired, in native form.
    pub desired: Option<NativeValue>,
    /// The entry in the file now.
    pub observed: Option<NativeValue>,
    /// What the ledger records once the action is done.
    pub written: Option<NativeValue>,
    pub delivery: Option<&'static str>,
}

/// One native file and container.
#[derive(Debug, Clone)]
pub struct TargetPlan {
    /// Every client whose entries go to this container, in table order.
    pub clients: Vec<&'static str>,
    pub scope: Scope,
    /// The project of a project or project-local target.
    pub project_id: Option<String>,
    /// The project folder `<P>`, when known.
    pub project: Option<PathBuf>,
    /// Server entries of a container, or members of a list (Claude Code
    /// `disabledMcpServers`).
    pub kind: EntryKind,
    pub file: PathBuf,
    /// Container keys in the file.
    pub container: Vec<String>,
    /// [`container_pointer`] of [`TargetPlan::container`].
    pub pointer: String,
    /// The file bytes planned against; `None` when it does not exist.
    pub before: Option<Vec<u8>>,
    /// The bytes to write; `None` when the file stays as it is.
    pub after: Option<Vec<u8>>,
    pub entries: Vec<EntryPlan>,
    /// The whole target is blocked (`parse_error`, `unexpected_shape`,
    /// `trust_required`).
    pub problem: Option<CoreError>,
    pub warnings: Vec<CoreError>,
    /// Values the file holds now in entries written inline, masked in
    /// [`TargetPlan::diff`].
    pub(crate) masked: Vec<SecretValue>,
    pub(crate) input: TargetInput,
}

/// What a target was planned from, kept so the executor can re-plan a file
/// that changed underneath (SPEC 8.4 step 1).
#[derive(Debug, Clone)]
pub(crate) struct TargetInput {
    pub(crate) target: Target,
    pub(crate) clients: Vec<&'static str>,
    pub(crate) desired: Vec<DesiredEntry>,
    /// Desired list members, for a [`EntryKind::ListMember`] target.
    pub(crate) members: Vec<DesiredMember>,
    pub(crate) native_disable: BTreeMap<&'static str, bool>,
    pub(crate) invalid_servers: Vec<String>,
    pub(crate) project_id: Option<String>,
    pub(crate) kind: EntryKind,
    /// Container keys recorded in the ledger, for a target planned only to
    /// clean up owned entries; `None` resolves [`Target::container_keys`].
    pub(crate) keys: Option<Vec<String>>,
    /// Blocks the whole target (`trust_required`).
    pub(crate) blocked: Option<CoreError>,
    /// Hide the file through the project repository's `info/exclude` once
    /// it is written (a private entry in an untracked project file, SPEC
    /// 6.5).
    pub(crate) exclude: bool,
}

/// A string a list target should hold (a server name in Claude Code's
/// `disabledMcpServers`).
#[derive(Debug, Clone)]
pub(crate) struct DesiredMember {
    pub(crate) client: &'static str,
    pub(crate) server_id: String,
    pub(crate) name: String,
}

impl TargetInput {
    fn new(target: Target, client: &'static str, native_disable: bool) -> Self {
        Self {
            target,
            clients: vec![client],
            desired: Vec::new(),
            members: Vec::new(),
            native_disable: BTreeMap::from([(client, native_disable)]),
            invalid_servers: Vec::new(),
            project_id: None,
            kind: EntryKind::McpEntry,
            keys: None,
            blocked: None,
            exclude: false,
        }
    }

    /// The container keys entries go in.
    fn container_keys(&self) -> Result<Vec<String>, CoreError> {
        match &self.keys {
            Some(keys) => Ok(keys.clone()),
            None => self.target.container_keys(),
        }
    }

    /// Whether `target` is the same file and container (for the same
    /// project and kind).
    fn same(&self, target: &Target, kind: EntryKind) -> bool {
        self.kind == kind
            && self.target.file == target.file
            && self.target.container == target.container
            && self.target.project == target.project
    }

    /// Adds `client`'s share of a target that `same` matched.
    fn join(&mut self, client: &'static str, native_disable: bool, warnings: Vec<CoreError>) {
        if !self.clients.contains(&client) {
            self.clients.push(client);
            self.target.warnings.extend(warnings);
        }
        self.native_disable.insert(client, native_disable);
    }
}

impl EntryPlan {
    /// Whether the entry is blocked because a secret cannot be had on this
    /// machine (SPEC 11.9, 14 exit code 5).
    pub fn blocked_by_secret(&self) -> bool {
        self.problem.as_ref().is_some_and(|err| {
            matches!(
                err.code(),
                ErrorCode::MissingSecret
                    | ErrorCode::SecretLocked
                    | ErrorCode::ProviderUnavailable
                    | ErrorCode::ProviderTooOld
            )
        })
    }
}

impl TargetPlan {
    /// The client named in reports: the first one.
    pub fn client(&self) -> &'static str {
        self.clients[0]
    }

    /// Counts of planned write actions: create, update, remove, disable.
    pub fn counts(&self) -> OpCounts {
        let mut counts = OpCounts::default();
        for entry in &self.entries {
            match entry.action {
                Action::Create | Action::AdoptOverwrite => counts.create += 1,
                Action::Update => counts.update += 1,
                Action::Remove => counts.remove += 1,
                Action::Disable => counts.disable += 1,
                _ => {}
            }
        }
        counts
    }

    /// Whether anything (file or ledger) would change.
    pub fn has_work(&self) -> bool {
        self.after.is_some()
            || self.entries.iter().any(|entry| {
                matches!(
                    entry.action,
                    Action::Adopt
                        | Action::Refresh
                        | Action::Forget
                        | Action::Release
                        | Action::Import
                )
            })
    }

    /// Entries that need a decision that was not given.
    pub fn unresolved(&self) -> usize {
        self.entries.iter().filter(|entry| entry.unresolved).count()
    }

    /// Whether some entry or the whole target is blocked.
    pub fn is_blocked(&self) -> bool {
        self.problem.is_some()
            || self
                .entries
                .iter()
                .any(|entry| entry.state == EntryState::Blocked)
    }

    /// The change as a unified diff of the file text; empty when the file
    /// stays as it is. Values written by `inline` delivery are masked
    /// (SPEC 8.3).
    pub fn diff(&self) -> String {
        let Some(after) = &self.after else {
            return String::new();
        };
        let before = self.mask(&String::from_utf8_lossy(
            self.before.as_deref().unwrap_or_default(),
        ));
        let after = self.mask(&String::from_utf8_lossy(after));
        let name = self.file.to_string_lossy();
        let old = if self.before.is_some() {
            format!("a/{name}")
        } else {
            "/dev/null".to_owned()
        };
        TextDiff::from_lines(before.as_str(), after.as_str())
            .unified_diff()
            .context_radius(3)
            .header(&old, &format!("b/{name}"))
            .to_string()
    }

    /// `text` with every value to hide replaced, as written and as a JSON
    /// or TOML string escape: the inline values this plan writes become
    /// `********`; the values an inline entry holds now that this plan does
    /// not write again (a rotation, a mode change, a removal) become
    /// `******** (before)`, so a changed value still shows as a change.
    /// Values shorter than four characters are left alone: masking them
    /// would garble the diff without hiding anything.
    pub fn mask(&self, text: &str) -> String {
        let mut text = text.to_owned();
        let writes = self.input.desired.iter().flat_map(|entry| &entry.inline);
        let values = writes
            .map(|value| (value, "********"))
            .chain(self.masked.iter().map(|value| (value, "******** (before)")));
        for (value, placeholder) in values {
            let raw = value.expose();
            if raw.chars().count() < 4 {
                continue;
            }
            let escaped = serde_json::to_string(raw).expect("a string serializes");
            let escaped = &escaped[1..escaped.len() - 1];
            for form in [escaped, raw] {
                text = text.replace(form, placeholder);
            }
        }
        text
    }

    /// Entry `name` with native value `value`, alone under the innermost
    /// key of this target's container, as the file format writes it
    /// (`{"mcpServers": {...}}`, `[mcp_servers.x]`), with
    /// [`TargetPlan::mask`] applied: the snippet the effective view and the
    /// drift resolver show. Outer container keys are left out (Claude
    /// Code's per-project map is keyed by the folder path). `None` when the
    /// format cannot hold it.
    pub fn snippet(&self, name: &str, value: &NativeValue) -> Option<String> {
        let keys: Vec<&str> = self
            .container
            .last()
            .map(String::as_str)
            .into_iter()
            .collect();
        let mut doc = Document::empty(self.input.target.format);
        doc.upsert(&keys, name, value).ok()?;
        Some(self.mask(&String::from_utf8_lossy(&doc.to_bytes())))
    }
}

/// Of an entry that is or was written inline, the values its current
/// native form may hold in plain text: every string under an environment
/// or header map, and every other string the desired entry does not hold
/// as it is.
fn old_inline_values(entry: &EntryPlan) -> Vec<SecretValue> {
    let Some(observed) = &entry.observed else {
        return Vec::new();
    };
    let mut kept = Vec::new();
    if let Some(desired) = &entry.desired {
        leaves(desired, false, &mut |text, _| kept.push(text.to_owned()));
    }
    let mut out = Vec::new();
    leaves(observed, false, &mut |text, secret_map| {
        if secret_map || !kept.iter().any(|k| k == text) {
            out.push(SecretValue::new(text));
        }
    });
    out
}

/// Calls `f` with every string leaf of `value` and whether it sits in an
/// environment or header map.
fn leaves(value: &NativeValue, secret_map: bool, f: &mut dyn FnMut(&str, bool)) {
    match value {
        NativeValue::String(text) => f(text, secret_map),
        NativeValue::Array(items) => {
            for item in items {
                leaves(item, secret_map, f);
            }
        }
        NativeValue::Object(map) => {
            for (key, item) in map.iter() {
                let secret =
                    secret_map || matches!(key, "env" | "headers" | "http_headers" | "environment");
                leaves(item, secret, f);
            }
        }
        _ => {}
    }
}

/// Planned create, update, remove and disable operations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpCounts {
    pub create: u32,
    pub update: u32,
    pub remove: u32,
    pub disable: u32,
}

impl OpCounts {
    pub fn total(&self) -> u32 {
        self.create + self.update + self.remove + self.disable
    }
}

/// A whole plan.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub targets: Vec<TargetPlan>,
    /// Problems outside any target: clients that cannot be written here,
    /// server files that do not load.
    pub warnings: Vec<CoreError>,
}

impl Plan {
    /// Targets whose file would change.
    pub fn writes(&self) -> usize {
        self.targets
            .iter()
            .filter(|target| target.after.is_some())
            .count()
    }

    pub fn unresolved(&self) -> usize {
        self.targets.iter().map(TargetPlan::unresolved).sum()
    }

    /// Whether applying it would change anything (files or ledger).
    pub fn has_work(&self) -> bool {
        self.targets.iter().any(TargetPlan::has_work)
    }
}

/// Plans the selected scopes: the user files of every selected client that
/// has desired or owned entries on this machine, and the project and
/// project-local targets of the selected known projects (SPEC 6).
pub fn plan(
    cx: &RenderCx<'_>,
    desired: &DesiredState,
    ledger: &Ledger,
    key: &LedgerKey,
    options: &PlanOptions,
) -> Result<Plan, CoreError> {
    let rendering = system_entries(cx, desired);
    let mut plan = Plan {
        targets: Vec::new(),
        warnings: rendering.warnings.clone(),
    };
    for (id, err) in &desired.invalid {
        plan.warnings.push(CoreError::new(
            err.code(),
            format!("server {id:?} is not applied, and entries it owns are left alone: {err}"),
        ));
    }
    let mut inputs: Vec<TargetInput> = Vec::new();
    if options.plans_user() {
        user_inputs(
            cx,
            desired,
            &rendering,
            ledger,
            options,
            &mut plan,
            &mut inputs,
        );
    }
    if options.scope.project() {
        let mut planner = ProjectPlanner {
            cx,
            desired,
            rendering: &rendering,
            ledger,
            options,
            detected: Detected::default(),
            warnings: Vec::new(),
            inputs: Vec::new(),
        };
        planner.plan_projects();
        plan.warnings.append(&mut planner.warnings);
        inputs.append(&mut planner.inputs);
    }
    inputs.retain(|input| options.selects_target(&input.target));
    for input in inputs {
        let bytes = read_target(cx.fs, &input.target.file)?;
        plan.targets
            .push(plan_target(cx, input, bytes, ledger, key, options));
    }
    Ok(plan)
}

/// The user-file targets (SPEC 6.1 system layer).
fn user_inputs(
    cx: &RenderCx<'_>,
    desired: &DesiredState,
    rendering: &Rendering,
    ledger: &Ledger,
    options: &PlanOptions,
    plan: &mut Plan,
    inputs: &mut Vec<TargetInput>,
) {
    let owned: Vec<&str> = ledger
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::McpEntry && entry.scope == Scope::User)
        .map(|entry| entry.client.as_str())
        .collect();
    let clients: Vec<&'static AdapterSpec> = ADAPTERS
        .iter()
        .copied()
        .filter(|spec| rendering.clients.contains(&spec.id) || owned.contains(&spec.id))
        .filter(|spec| options.selects(spec.id))
        .collect();

    for spec in clients {
        let prefs = desired.prefs(spec.id);
        let targets = match spec.mcp_targets(cx.env, cx.fs, Scope::User, None, &prefs) {
            Ok(targets) => targets,
            Err(err) => {
                plan.warnings.push(err);
                continue;
            }
        };
        let desired_here: Vec<DesiredEntry> = rendering
            .entries
            .iter()
            .filter(|entry| entry.client == spec.id)
            .cloned()
            .collect();
        let native_disable = prefs.native_disable == Some(true);
        let extension = replacing_extension(spec, cx.env, cx.fs, None);
        for mut target in targets {
            target.warnings.extend(extension.clone());
            match inputs
                .iter_mut()
                .find(|input| input.same(&target, EntryKind::McpEntry))
            {
                Some(input) => {
                    input.join(spec.id, native_disable, target.warnings);
                    input.desired.extend(desired_here.iter().cloned());
                }
                None => {
                    let mut input = TargetInput::new(target, spec.id, native_disable);
                    input.desired = desired_here.clone();
                    input.invalid_servers = desired.invalid.keys().cloned().collect();
                    inputs.push(input);
                }
            }
        }
    }
}

/// Builds the project and project-local targets of the selected known
/// projects, and the clean-up targets of projects this machine no longer
/// knows.
struct ProjectPlanner<'a, 'c> {
    cx: &'a RenderCx<'c>,
    desired: &'a DesiredState,
    /// The system layer, for suppressions (only of servers enabled at
    /// system scope) and their renderings.
    rendering: &'a Rendering,
    ledger: &'a Ledger,
    options: &'a PlanOptions,
    detected: Detected,
    warnings: Vec<CoreError>,
    inputs: Vec<TargetInput>,
}

/// One project being planned.
struct ProjectCx<'p> {
    id: &'p str,
    root: &'p Path,
    /// `trust_required` when the manifests are not trusted as they are.
    blocked: Option<CoreError>,
}

impl ProjectPlanner<'_, '_> {
    fn plan_projects(&mut self) {
        let state_dir = self.cx.loadout_home.join(STATE_DIR);
        let trust = TrustStore::load(self.cx.fs, &state_dir).unwrap_or_else(|err| {
            // Nothing is trusted then: every project is blocked, user files
            // still apply.
            self.warnings.push(err);
            TrustStore::default()
        });
        let known = layering::known_projects(
            self.cx.fs,
            self.cx.env,
            self.desired.overlay.as_ref(),
            &trust,
        );
        if let ProjectSelection::Only(root) = &self.options.projects {
            match known
                .iter()
                .find(|project| layering::same_path(&project.root, root))
            {
                None => self.warnings.push(
                    CoreError::new(
                        ErrorCode::NotFound,
                        "this folder is not a project known on this machine",
                    )
                    .with_path(root),
                ),
                Some(project) if matches!(project.status, ProjectStatus::Missing) => {
                    self.warnings.push(
                        CoreError::new(ErrorCode::NotFound, "the project folder does not exist")
                            .with_path(root),
                    );
                }
                Some(_) => {}
            }
        }
        let options = self.options;
        for project in known
            .iter()
            .filter(|project| options.plans_project(project))
        {
            match &project.status {
                ProjectStatus::Missing => {}
                ProjectStatus::Failed(err) => self.warnings.push(err.clone()),
                ProjectStatus::Loaded {
                    project: loaded, ..
                } => {
                    self.warnings.extend(loaded.warnings.iter().cloned());
                    let pcx = ProjectCx {
                        id: &project.id,
                        root: &project.root,
                        blocked: project.trust_error(),
                    };
                    let start = self.inputs.len();
                    let mut held = self.plan_project(&pcx, loaded);
                    // What a server that does not load owns here (its
                    // overrides, its suppressions) is left alone too.
                    held.extend(self.desired.invalid.keys().cloned());
                    for input in &mut self.inputs[start..] {
                        input.project_id = Some(project.id.clone());
                        input.blocked.clone_from(&pcx.blocked);
                        input.invalid_servers.extend(held.iter().cloned());
                    }
                    self.cleanup(
                        |id| id == project.id,
                        Some(&project.root),
                        pcx.blocked.as_ref(),
                        &held,
                    );
                }
            }
        }
        if self.options.projects == ProjectSelection::All {
            let known_ids: Vec<&str> = known.iter().map(|project| project.id.as_str()).collect();
            let invalid: Vec<String> = self.desired.invalid.keys().cloned().collect();
            self.cleanup(|id| !known_ids.contains(&id), None, None, &invalid);
        }
    }

    /// The targets of a loaded project's entries. Returns the names of
    /// entries that cannot be resolved: what this project owns for them is
    /// left alone (never removed) until they are fixed.
    fn plan_project(&mut self, pcx: &ProjectCx<'_>, project: &Project) -> Vec<String> {
        let entries = layering::project_entries(
            project,
            &self.desired.servers,
            self.cx.env,
            self.desired.overlay.as_ref(),
        );
        let mut warned: Vec<String> = Vec::new();
        let mut held = Vec::new();
        for entry in &entries {
            if matches!(entry.body, EntryBody::Server(Err(_))) {
                held.push(entry.name.clone());
            }
            self.plan_entry(pcx, entry, false, &mut warned);
            // A client holding the merged entry privately keeps the shared
            // definition in the shared file, which stays as committed.
            if let Some(shared) = entry
                .shared
                .as_deref()
                .filter(|shared| matches!(shared.body, EntryBody::Server(_)))
            {
                self.plan_entry(pcx, shared, true, &mut warned);
            }
        }
        held
    }

    /// Plans `entry` for each of its clients; with `private_only`, only for
    /// clients that have a private project target.
    fn plan_entry(
        &mut self,
        pcx: &ProjectCx<'_>,
        entry: &ProjectEntry,
        private_only: bool,
        warned: &mut Vec<String>,
    ) {
        let clients = match &entry.body {
            EntryBody::Server(Err(_)) => self.error_clients(pcx, entry),
            EntryBody::Server(Ok(_)) | EntryBody::Suppression => {
                self.detected
                    .project_selection(self.cx, self.desired, entry)
            }
        };
        for client in clients {
            if !self.options.selects(&client) || self.desired.excluded(&client) {
                continue;
            }
            let spec = match writable(&client, self.cx.env) {
                Ok(spec) => spec,
                Err(err) => {
                    if !warned.contains(&client) {
                        warned.push(client);
                        self.warnings.push(err);
                    }
                    continue;
                }
            };
            if private_only
                && spec
                    .mcp
                    .as_ref()
                    .is_none_or(|mcp| mcp.project_local.is_empty())
            {
                continue;
            }
            match &entry.body {
                EntryBody::Server(server) => self.server_entry(pcx, entry, server, spec),
                EntryBody::Suppression => self.suppression(pcx, entry, spec),
            }
        }
    }

    /// The clients of an entry that cannot be resolved, so it is reported
    /// blocked wherever it would go: its client list, or for
    /// `all-detected` the detected clients with project files (no
    /// transport to fit), plus every client this project owns an entry of
    /// that name for.
    fn error_clients(&mut self, pcx: &ProjectCx<'_>, entry: &ProjectEntry) -> Vec<String> {
        let mut clients = match &entry.clients {
            ClientSelection::List(ids) => ids.clone(),
            ClientSelection::AllDetected => {
                let mut ids = Vec::new();
                for spec in ADAPTERS.iter().copied() {
                    let has_files = spec.mcp.as_ref().is_some_and(|mcp| {
                        !mcp.read_only
                            && mcp.oses.contains(&self.cx.env.os())
                            && !(mcp.project.is_empty() && mcp.project_local.is_empty())
                    });
                    if has_files && self.detected.detected(self.cx, self.desired, spec) {
                        ids.push(spec.id.to_owned());
                    }
                }
                ids
            }
        };
        for row in &self.ledger.entries {
            let owns = row.kind == EntryKind::McpEntry
                && row.project_id.as_deref() == Some(pcx.id)
                && (row.server_id.as_deref() == Some(entry.name.as_str()) || row.key == entry.name);
            if owns && !clients.contains(&row.client) {
                clients.push(row.client.clone());
            }
        }
        clients
    }

    /// The target file of an entry from `layer` for `spec` (SPEC 6.1, 6.5):
    /// the shared project file for a shared entry; the private target for
    /// a local one, else the project file written privately. `None` (with
    /// a warning) when the client has no such file.
    fn placement(
        &mut self,
        pcx: &ProjectCx<'_>,
        spec: &'static AdapterSpec,
        mcp: &McpSpec,
        layer: EntryLayer,
    ) -> Option<(Target, Sharing)> {
        let (scope, sharing) = match layer {
            EntryLayer::Shared => (Scope::Project, Sharing::Shared),
            EntryLayer::Local if mcp.project_local.is_empty() => (Scope::Project, Sharing::Private),
            EntryLayer::Local => (Scope::ProjectLocal, Sharing::Private),
        };
        let prefs = self.desired.prefs(spec.id);
        match spec.mcp_targets(self.cx.env, self.cx.fs, scope, Some(pcx.root), &prefs) {
            Ok(mut targets) if !targets.is_empty() => {
                let mut target = targets.swap_remove(0);
                // A user or project install of an extension replacing the
                // client's MCP support (Pi `pi-mcp-adapter`, gate V2 PI-8).
                target.warnings.extend(replacing_extension(
                    spec,
                    self.cx.env,
                    self.cx.fs,
                    Some(pcx.root),
                ));
                Some((target, sharing))
            }
            Ok(_) => None,
            Err(err) => {
                self.warnings.push(err);
                None
            }
        }
    }

    /// SPEC 6.5 for a private entry in `target`, a project file: blocked
    /// with `private_in_tracked_file` when the project's repository tracks
    /// the file; otherwise whether to hide it through `info/exclude`.
    fn private_file(
        pcx: &ProjectCx<'_>,
        spec: &AdapterSpec,
        target: &Target,
    ) -> Result<bool, CoreError> {
        if target.scope != Scope::Project {
            return Ok(false);
        }
        let Some(repo) = ProjectRepo::discover(pcx.root)? else {
            return Ok(false);
        };
        if repo.is_tracked(&target.file)? {
            let file = repo
                .relative(&target.file)
                .unwrap_or_else(|| target.file.to_string_lossy().into_owned());
            return Err(CoreError::new(
                ErrorCode::PrivateInTrackedFile,
                format!(
                    "a project.local.toml entry would go to {file}, which git tracks; remove {} \
                     from this entry's clients, or untrack {file}",
                    spec.id
                ),
            )
            .with_path(&target.file));
        }
        Ok(!repo.is_ignored(&target.file)?)
    }

    fn server_entry(
        &mut self,
        pcx: &ProjectCx<'_>,
        entry: &ProjectEntry,
        server: &Result<Box<Server>, CoreError>,
        spec: &'static AdapterSpec,
    ) {
        let mcp = spec
            .mcp
            .as_ref()
            .expect("writable clients have MCP support");
        // Claude Desktop has no project files: it never gets project
        // entries.
        if mcp.project.is_empty() && mcp.project_local.is_empty() {
            return;
        }
        let Some((target, sharing)) = self.placement(pcx, spec, mcp, entry.layer) else {
            return;
        };
        let mut exclude = false;
        let mut want = DesiredEntry {
            client: spec.id,
            server_id: entry.name.clone(),
            name: server.as_ref().map_or_else(
                |_| entry.name.clone(),
                |server| native_name(server, spec.id),
            ),
            server: Err(CoreError::new(ErrorCode::InvalidValue, "not rendered")),
            delivery: None,
            inline: Vec::new(),
            warnings: Vec::new(),
            native: None,
        };
        let ready = match (&pcx.blocked, server) {
            (Some(err), _) | (None, Err(err)) => Err(err.clone()),
            (None, Ok(server)) => match entry.layer {
                EntryLayer::Local => Self::private_file(pcx, spec, &target).map(|hide| {
                    exclude = hide;
                    server
                }),
                EntryLayer::Shared => Ok(server),
            },
        };
        want.server = ready.and_then(|server| {
            let out = render_for(
                self.cx,
                self.desired,
                server,
                spec,
                RenderTarget {
                    sharing,
                    project: Some(pcx.root),
                    overlay: false,
                },
            )?;
            want.delivery = out.delivery.map(|mode| mode.as_str());
            want.inline = out.inline;
            want.warnings = out.warnings;
            let mut rendered = out.server;
            rendered.enabled = entry.enabled;
            Ok(rendered)
        });
        if want.server.is_ok() && mcp.caps.project_semantics == ProjectSemantics::DeepMerge {
            want.warnings
                .extend(unset_not_expressible(spec, entry, &target.file));
        }
        let native_disable = self.desired.prefs(spec.id).native_disable == Some(true);
        let input = self.input_for(target, spec.id, native_disable, EntryKind::McpEntry);
        input.exclude |= exclude;
        input.desired.push(want);
    }

    /// SPEC 6.3: hides system server `entry.name` from `spec` in this
    /// project, when the server is enabled for it at system scope.
    fn suppression(
        &mut self,
        pcx: &ProjectCx<'_>,
        entry: &ProjectEntry,
        spec: &'static AdapterSpec,
    ) {
        let Some(server) = self.desired.servers.get(&entry.name) else {
            return;
        };
        let Some(system) = self
            .rendering
            .entries
            .iter()
            .find(|want| want.client == spec.id && want.server_id == entry.name)
        else {
            return;
        };
        let mechanism = match layering::suppression_mechanism(spec, &entry.name) {
            Ok(mechanism) => mechanism,
            Err(warning) => {
                self.warnings.push(warning);
                return;
            }
        };
        let mcp = spec
            .mcp
            .as_ref()
            .expect("writable clients have MCP support");
        let native_disable = self.desired.prefs(spec.id).native_disable == Some(true);
        let name = native_name(server, spec.id);
        match mechanism {
            ProjectDisable::ListMember(_) => {
                let prefs = self.desired.prefs(spec.id);
                let target =
                    match spec.suppression_target(self.cx.env, self.cx.fs, pcx.root, &prefs) {
                        Ok(Some(target)) => target,
                        Ok(None) => return,
                        Err(err) => {
                            self.warnings.push(err);
                            return;
                        }
                    };
                // A private suppression listed in a shared project file
                // (Gemini `mcp.excluded`, SPEC 6.5).
                let mut exclude = false;
                if pcx.blocked.is_none() && matches!(entry.layer, EntryLayer::Local) {
                    match Self::private_file(pcx, spec, &target) {
                        Ok(hide) => exclude = hide,
                        Err(err) => {
                            self.warnings.push(err);
                            return;
                        }
                    }
                }
                let input = self.input_for(target, spec.id, native_disable, EntryKind::ListMember);
                input.exclude |= exclude;
                input.members.push(DesiredMember {
                    client: spec.id,
                    server_id: entry.name.clone(),
                    name,
                });
            }
            ProjectDisable::EnabledFalse { partial } => {
                let Some((target, sharing)) = self.placement(pcx, spec, mcp, entry.layer) else {
                    return;
                };
                let mut exclude = false;
                let mut want = DesiredEntry {
                    client: spec.id,
                    server_id: entry.name.clone(),
                    name,
                    server: Err(CoreError::new(ErrorCode::InvalidValue, "not rendered")),
                    delivery: None,
                    inline: Vec::new(),
                    warnings: Vec::new(),
                    native: None,
                };
                let ready = match (&pcx.blocked, entry.layer) {
                    (Some(err), _) => Err(err.clone()),
                    (None, EntryLayer::Local) => {
                        Self::private_file(pcx, spec, &target).map(|hide| exclude = hide)
                    }
                    (None, EntryLayer::Shared) => Ok(()),
                };
                want.server = ready.and_then(|()| match (&system.server, partial) {
                    // A partial entry holds nothing of the server.
                    (Ok(rendered), true) => Ok(rendered.clone()),
                    _ => {
                        let out = render_for(
                            self.cx,
                            self.desired,
                            server,
                            spec,
                            RenderTarget {
                                sharing,
                                project: Some(pcx.root),
                                overlay: true,
                            },
                        )?;
                        want.delivery = out.delivery.map(|mode| mode.as_str());
                        want.inline = out.inline;
                        want.warnings = out.warnings;
                        Ok(out.server)
                    }
                });
                if let Ok(rendered) = &want.server {
                    match mcp.suppression_entry(rendered, self.cx.env.os()) {
                        Ok(native) => want.native = native,
                        Err(err) => want.server = Err(err.into()),
                    }
                }
                if partial {
                    want.delivery = None;
                    want.inline.clear();
                }
                let input = self.input_for(target, spec.id, native_disable, EntryKind::McpEntry);
                input.exclude |= exclude;
                input.desired.push(want);
            }
        }
    }

    /// The input of `target`, joining one planned already.
    fn input_for(
        &mut self,
        target: Target,
        client: &'static str,
        native_disable: bool,
        kind: EntryKind,
    ) -> &mut TargetInput {
        let index = match self
            .inputs
            .iter()
            .position(|input| input.same(&target, kind))
        {
            Some(index) => {
                self.inputs[index].join(client, native_disable, target.warnings);
                index
            }
            None => {
                let mut input = TargetInput::new(target, client, native_disable);
                input.kind = kind;
                self.inputs.push(input);
                self.inputs.len() - 1
            }
        };
        &mut self.inputs[index]
    }

    /// Targets for owned project entries that no planned target covers:
    /// entries of a project this machine no longer knows (`root` is
    /// `None`), or left in a file the project no longer uses. Unchanged
    /// owned entries are removed there, like any entry no longer desired,
    /// except those of the `held` names (entries that cannot be resolved
    /// now), which are left alone. A planned target of a shared file (SPEC
    /// 6.5) that covers an entry another client owns gets that client
    /// joined, so its entries there are cleaned up too.
    fn cleanup(
        &mut self,
        project: impl Fn(&str) -> bool,
        root: Option<&Path>,
        blocked: Option<&CoreError>,
        held: &[String],
    ) {
        type Covered = (Scope, Option<String>, String, String, EntryKind);
        let covered: Vec<Covered> = self
            .inputs
            .iter()
            .map(|input| {
                let pointer = input
                    .container_keys()
                    .map(|keys| container_pointer(&keys))
                    .unwrap_or_default();
                (
                    input.target.scope,
                    input.project_id.clone(),
                    input.target.file.to_string_lossy().into_owned(),
                    pointer,
                    input.kind,
                )
            })
            .collect();
        let mut orphans: Vec<TargetInput> = Vec::new();
        for row in &self.ledger.entries {
            let Some(id) = row.project_id.as_deref() else {
                continue;
            };
            if row.scope == Scope::User || !project(id) {
                continue;
            }
            let Some(spec) = adapter(&row.client) else {
                continue;
            };
            let Some(mcp) = spec.mcp.as_ref() else {
                continue;
            };
            if !self.options.selects(spec.id) {
                continue;
            }
            let native_disable = self.desired.prefs(spec.id).native_disable == Some(true);
            let covering = covered
                .iter()
                .position(|(scope, project_id, file, pointer, kind)| {
                    *scope == row.scope
                        && project_id.as_deref() == Some(id)
                        && *file == row.file
                        && !pointer.is_empty()
                        && *pointer == row.container
                        && *kind == row.kind
                });
            if let Some(index) = covering {
                self.inputs[index].join(spec.id, native_disable, Vec::new());
                continue;
            }
            let same = orphans.iter_mut().find(|input| {
                input.target.scope == row.scope
                    && input.project_id.as_deref() == Some(id)
                    && input.target.file.to_string_lossy() == row.file
                    && input.keys.as_deref().map(container_pointer).as_deref()
                        == Some(row.container.as_str())
                    && input.kind == row.kind
            });
            if let Some(input) = same {
                input.join(spec.id, native_disable, Vec::new());
                continue;
            }
            let target = Target {
                client: spec.id,
                scope: row.scope,
                os: self.cx.env.os(),
                file: PathBuf::from(&row.file),
                format: mcp.format,
                container: match row.scope {
                    Scope::ProjectLocal => mcp.project_local_container,
                    _ => mcp.container,
                },
                project: root.map(Path::to_path_buf),
                profile: None,
                skeleton: mcp.skeleton,
                warnings: Vec::new(),
            };
            let mut input = TargetInput::new(target, spec.id, native_disable);
            input.kind = row.kind;
            input.keys = Some(pointer_keys(&row.container));
            input.project_id = Some(id.to_owned());
            input.blocked = blocked.cloned();
            input.invalid_servers = held.to_vec();
            orphans.push(input);
        }
        self.inputs.append(&mut orphans);
    }
}

/// `unset_not_expressible` for `entry` on `spec` (SPEC 6.3): a client that
/// merges project entries over user entries field by field cannot drop
/// what the user entry sets. `None` when the entry unsets nothing.
pub(crate) fn unset_not_expressible(
    spec: &AdapterSpec,
    entry: &ProjectEntry,
    file: &Path,
) -> Option<CoreError> {
    if entry.unset.is_empty() {
        return None;
    }
    Some(
        CoreError::new(
            ErrorCode::UnsetNotExpressible,
            format!(
                "{} merges project entries over user entries field by field, so `unset` ({}) \
                 of {:?} cannot remove what the user entry sets; the entry is written anyway",
                spec.display_name,
                entry.unset.join(", "),
                entry.name
            ),
        )
        .with_path(file),
    )
}

/// `client_enable_required` for `names` in a project file of `spec`, when
/// the client lists new project servers as disabled until each is enabled
/// once in its UI (Cursor, gate V2 CUR-2). `None` for any other client or
/// no names.
pub(crate) fn client_enable_required(
    spec: &AdapterSpec,
    names: &[&str],
    file: &Path,
) -> Option<CoreError> {
    let TrustGate::ClientEnable(place) = spec.mcp.as_ref()?.caps.trust_gate? else {
        return None;
    };
    if names.is_empty() {
        return None;
    }
    let names: Vec<String> = names.iter().map(|name| format!("{name:?}")).collect();
    Some(
        CoreError::new(
            ErrorCode::ClientEnableRequired,
            format!(
                "{} lists new servers of this project file as Disabled until each is enabled \
                 in {place} (once per server; Loadout cannot read that toggle): {}",
                spec.display_name,
                names.join(", ")
            ),
        )
        .with_path(file),
    )
}

/// [`client_enable_required`] for each client of a project target, naming
/// the entries Loadout writes or keeps owning there.
fn client_enable_hints(plan: &TargetPlan) -> Vec<CoreError> {
    if plan.scope == Scope::User {
        return Vec::new();
    }
    plan.clients
        .iter()
        .filter_map(|&client| {
            let names: Vec<&str> = plan
                .entries
                .iter()
                .filter(|entry| {
                    entry.client == Some(client)
                        && entry.desired.is_some()
                        && entry.state != EntryState::Blocked
                        && !matches!(
                            entry.action,
                            Action::Skip | Action::Import | Action::Release
                        )
                })
                .map(|entry| entry.name.as_str())
                .collect();
            client_enable_required(adapter(client)?, &names, &plan.file)
        })
        .collect()
}

/// The keys of an RFC 6901 JSON pointer ([`container_pointer`] reversed).
fn pointer_keys(pointer: &str) -> Vec<String> {
    pointer
        .split('/')
        .skip(1)
        .map(|key| key.replace("~1", "/").replace("~0", "~"))
        .collect()
}

/// Whether two Claude Code project keys name the same folder: equal after
/// unifying separators, a trailing separator and, on Windows, case.
fn same_folder(os: Os, a: &str, b: &str) -> bool {
    let windows = os == Os::Windows;
    normalize_path_text(windows, a) == normalize_path_text(windows, b)
}

/// The bytes of a target file; `None` when it does not exist.
pub(crate) fn read_target(
    fs: &dyn crate::fs::Fs,
    file: &std::path::Path,
) -> Result<Option<Vec<u8>>, CoreError> {
    if !fs.exists(file) {
        return Ok(None);
    }
    fs.read(file).map(Some).map_err(|err| {
        CoreError::new(ErrorCode::IoError, format!("cannot read file: {err}")).with_path(file)
    })
}

fn mapper_of(client: &str) -> Option<&'static dyn EntryMapper> {
    adapter(client)
        .and_then(|spec| spec.mcp.as_ref())
        .map(|mcp| mcp.mapper)
}

/// Plans one target against `bytes`, the current file content.
pub(crate) fn plan_target(
    cx: &RenderCx<'_>,
    input: TargetInput,
    bytes: Option<Vec<u8>>,
    ledger: &Ledger,
    key: &LedgerKey,
    options: &PlanOptions,
) -> TargetPlan {
    let target = &input.target;
    let mut plan = TargetPlan {
        clients: input.clients.clone(),
        scope: target.scope,
        project_id: input.project_id.clone(),
        project: target.project.clone(),
        kind: input.kind,
        file: target.file.clone(),
        container: Vec::new(),
        pointer: String::new(),
        before: bytes.clone(),
        after: None,
        entries: Vec::new(),
        problem: None,
        warnings: target
            .warnings
            .iter()
            .chain(input.desired.iter().flat_map(|entry| &entry.warnings))
            .cloned()
            .collect(),
        masked: Vec::new(),
        input: input.clone(),
    };
    let keys = match input.container_keys() {
        Ok(keys) => keys,
        Err(err) => return blocked_target(plan, err.with_path(&target.file)),
    };
    plan.pointer = container_pointer(&keys);
    plan.container = keys.clone();
    if let Some(err) = &input.blocked {
        return blocked_target(plan, err.clone());
    }
    let parsed = match &bytes {
        Some(bytes) => Document::parse(bytes, target.format),
        None => Ok(target.new_document()),
    };
    let mut doc = match parsed {
        Ok(doc) => doc,
        Err(err) => return blocked_target(plan, err.with_path(&target.file)),
    };
    if let Some(warning) = key_spelling(target, &doc, &keys) {
        plan.warnings.push(warning);
    }
    if input.kind == EntryKind::ListMember {
        return plan_list(plan, doc, bytes, ledger, options);
    }
    let container: Vec<&str> = keys.iter().map(String::as_str).collect();
    let observed_names = match doc.list(&container) {
        Ok(names) => names,
        Err(err) => return blocked_target(plan, err.with_path(&target.file)),
    };
    let file = target.file.to_string_lossy().into_owned();
    let (pointer, project_id) = (plan.pointer.clone(), plan.project_id.clone());
    let slot = LedgerSlot {
        scope: plan.scope,
        project_id: project_id.as_deref(),
        file: &file,
        container: &pointer,
    };

    // Names in file order, then new desired names, then ledger-only names.
    let mut names: Vec<String> = observed_names.clone();
    for entry in &input.desired {
        if !names.contains(&entry.name) {
            names.push(entry.name.clone());
        }
    }
    for entry in ledger.in_container(slot) {
        if entry.kind == EntryKind::McpEntry && !names.contains(&entry.key) {
            names.push(entry.key.clone());
        }
    }
    let foreign_names: Vec<String> = observed_names
        .iter()
        .filter(|name| ledger.get(&slot, name).is_none())
        .cloned()
        .collect();

    for name in names {
        let observed = if observed_names.contains(&name) {
            match doc.get(&container, &name) {
                Ok(value) => value,
                Err(err) => {
                    plan.entries.push(blocked_entry(&name, None, None, err));
                    continue;
                }
            }
        } else {
            None
        };
        let owned = ledger
            .get(&slot, &name)
            .filter(|entry| entry.kind == EntryKind::McpEntry);
        let wanted: Vec<&DesiredEntry> = input
            .desired
            .iter()
            .filter(|entry| entry.name == name)
            .collect();
        let entry = plan_entry(
            cx,
            &input,
            &name,
            &foreign_names,
            wanted,
            observed,
            owned,
            key,
            options,
        );
        if let Some(entry) = entry {
            let was_inline = owned.and_then(|l| l.delivery.as_deref()) == Some("inline");
            if was_inline || entry.delivery == Some("inline") {
                plan.masked.extend(old_inline_values(&entry));
            }
            plan.entries.push(entry);
        }
    }

    // Apply the write actions to the document.
    let mut changed = false;
    for entry in &plan.entries {
        let result = match entry.action {
            Action::Create | Action::Update | Action::AdoptOverwrite | Action::Disable => {
                let value = entry.written.as_ref().expect("write actions carry a value");
                doc.upsert(&container, &entry.name, value)
            }
            Action::Remove => doc.remove(&container, &entry.name),
            _ => Ok(false),
        };
        match result {
            Ok(edited) => changed |= edited,
            Err(err) => return blocked_target(plan, err.with_path(&target.file)),
        }
    }
    if changed {
        let after = doc.to_bytes();
        if bytes.as_deref() != Some(after.as_slice()) {
            plan.after = Some(after);
        }
    }
    plan.warnings.extend(client_enable_hints(&plan));
    plan
}

/// `timeout_unit_conflict` when `native` sets a timeout key that the
/// clients reading `target`'s user file take in different units: with
/// `PI_CODING_AGENT_DIR` set, omp and Pi read one `mcp.json`, omp's
/// `timeout` in milliseconds and Pi's in seconds (gate V2 OMP-10).
fn timeout_unit_conflict(env: &Env, target: &Target, native: &NativeValue) -> Option<CoreError> {
    let entry = native.as_object()?;
    if target.scope != Scope::User {
        return None;
    }
    let readers: Vec<(&'static str, &'static [TimeoutField])> = ADAPTERS
        .iter()
        .filter_map(|spec| {
            let mcp = spec.mcp.as_ref()?;
            let reads = mcp.oses.contains(&env.os())
                && mcp
                    .user
                    .iter()
                    .filter_map(|location| location.resolve(env, None))
                    .any(|file| layering::same_path(&file, &target.file));
            reads.then_some((spec.id, mcp.caps.timeouts))
        })
        .collect();
    let mut keys = readers
        .iter()
        .flat_map(|(_, timeouts)| timeouts.iter())
        .map(|timeout| timeout.key)
        .filter(|key| entry.contains_key(key));
    keys.find_map(|key| {
        let units: Vec<(&str, TimeUnit)> = readers
            .iter()
            .filter_map(|(client, timeouts)| {
                let field = timeouts.iter().find(|timeout| timeout.key == key)?;
                Some((*client, field.unit))
            })
            .collect();
        if units.iter().all(|(_, unit)| *unit == units[0].1) {
            return None;
        }
        let clients: Vec<&str> = units.iter().map(|(client, _)| *client).collect();
        let described: Vec<String> = units
            .iter()
            .map(|(client, unit)| format!("{client} in {}", unit.name()))
            .collect();
        Some(
            CoreError::new(
                ErrorCode::TimeoutUnitConflict,
                format!(
                    "clients {} read this file and take `{key}` in different units ({}); \
                     remove `{key}` from this server's `[client.<id>]` tables, or give the \
                     clients separate files",
                    clients.join(" and "),
                    described.join(", ")
                ),
            )
            .with_path(&target.file),
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn plan_entry(
    cx: &RenderCx<'_>,
    input: &TargetInput,
    name: &str,
    foreign_names: &[String],
    wanted: Vec<&DesiredEntry>,
    observed: Option<NativeValue>,
    owned: Option<&LedgerEntry>,
    key: &LedgerKey,
    options: &PlanOptions,
) -> Option<EntryPlan> {
    let os = cx.env.os();
    let mut entry = EntryPlan {
        name: name.to_owned(),
        client: None,
        server_id: None,
        state: EntryState::Foreign,
        action: Action::None,
        unresolved: false,
        problem: None,
        desired: None,
        observed: observed.clone(),
        written: None,
        delivery: None,
    };

    // The desired native entry, if any.
    let mut desired: Option<(&'static str, NativeValue)> = None;
    if let Some(first) = wanted.first() {
        entry.client = Some(first.client);
        entry.server_id = Some(first.server_id.clone());
        entry.delivery = first.delivery;
        let mut natives = Vec::new();
        for want in &wanted {
            let native = match (&want.server, &want.native) {
                (Ok(_), Some(native)) => Ok(native.clone()),
                (Ok(server), None) => mapper_of(want.client)
                    .ok_or_else(|| CoreError::new(ErrorCode::NoAdapter, "no mapper"))
                    .and_then(|mapper| {
                        mapper
                            .to_native(
                                server,
                                &WriteCx {
                                    os,
                                    existing: observed.as_ref(),
                                },
                            )
                            .map_err(CoreError::from)
                    }),
                (Err(err), _) => Err(err.clone()),
            };
            match native {
                Ok(native) => natives.push((want.client, native)),
                Err(err) => {
                    entry.state = EntryState::Blocked;
                    entry.problem = Some(err);
                    return Some(entry);
                }
            }
        }
        if let Some(problem) = natives
            .iter()
            .find_map(|(_, native)| timeout_unit_conflict(cx.env, &input.target, native))
        {
            entry.state = EntryState::Blocked;
            entry.problem = Some(problem);
            return Some(entry);
        }
        let (client, native) = natives.remove(0);
        if natives.iter().any(|(_, other)| *other != native) {
            entry.state = EntryState::Blocked;
            entry.problem = Some(CoreError::new(
                ErrorCode::Collision,
                format!(
                    "clients {} share this file and want different entries named {name:?}",
                    wanted
                        .iter()
                        .map(|w| w.client)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
            return Some(entry);
        }
        entry.desired = Some(native.clone());
        desired = Some((client, native));
    }

    let resolution = |entry: &EntryPlan| options.resolution(entry.client, name);
    match (desired, observed, owned) {
        (Some((client, d)), None, None) => {
            // A foreign entry the client counts as the same server (Pi).
            let collisions = adapter(client)
                .map(|spec| spec.name_collisions(name, foreign_names))
                .unwrap_or_default();
            if let Some(collision) = collisions.into_iter().next() {
                entry.state = EntryState::Collision;
                entry.action = Action::Skip;
                entry.unresolved = resolution(&entry) == OnConflict::Ask;
                entry.problem = Some(collision);
            } else {
                entry.state = EntryState::PendingCreate;
                entry.action = Action::Create;
                entry.written = Some(d);
            }
        }
        (Some((_, d)), None, Some(_)) => {
            entry.state = EntryState::Missing;
            match resolution(&entry) {
                OnConflict::Skip | OnConflict::Release => entry.action = Action::Skip,
                OnConflict::Import => entry.action = Action::Import,
                OnConflict::Ask | OnConflict::Overwrite => {
                    entry.action = Action::Create;
                    entry.written = Some(d);
                }
            }
        }
        (Some((client, d)), Some(o), None) => {
            let mapper = mapper_of(client)?;
            if mapper.canonical(&o) == mapper.canonical(&d) {
                entry.state = EntryState::Synced;
                entry.action = Action::Adopt;
                entry.written = Some(o);
            } else {
                entry.state = EntryState::Collision;
                let resolved = resolution(&entry);
                decide(&mut entry, resolved, Action::AdoptOverwrite, d, false);
            }
        }
        (Some((client, d)), Some(o), Some(l)) => {
            let mapper = mapper_of(client)?;
            let (canon_o, canon_d) = (mapper.canonical(&o), mapper.canonical(&d));
            let hash_o = key.hash(&canon_o);
            if Some(&hash_o) == l.written_hash.as_ref() {
                if canon_o == canon_d {
                    entry.state = EntryState::Synced;
                } else {
                    entry.state = EntryState::PendingUpdate;
                    entry.action = Action::Update;
                    entry.written = Some(d);
                }
            } else if canon_o == canon_d {
                entry.state = EntryState::Synced;
                entry.action = Action::Refresh;
                entry.written = Some(o);
            } else {
                entry.state = if l.written_hash.as_ref() == Some(&key.hash(&canon_d)) {
                    EntryState::Drifted
                } else {
                    EntryState::Conflict
                };
                let resolved = resolution(&entry);
                decide(&mut entry, resolved, Action::Update, d, true);
            }
        }
        (None, Some(o), Some(l)) => {
            entry.client = adapter(&l.client).map(|spec| spec.id);
            entry.server_id = l.server_id.clone();
            let client = entry.client?;
            if !input.clients.contains(&client) {
                return None;
            }
            if l.server_id
                .as_ref()
                .is_some_and(|id| input.invalid_servers.contains(id))
            {
                entry.state = EntryState::Blocked;
                entry.problem = Some(not_loading(l.server_id.as_deref()));
                return Some(entry);
            }
            let mapper = mapper_of(client)?;
            let canon_o = mapper.canonical(&o);
            if Some(&key.hash(&canon_o)) == l.written_hash.as_ref() {
                let disabled = input
                    .native_disable
                    .get(client)
                    .copied()
                    .unwrap_or(false)
                    .then(|| disabled_value(client, &o))
                    .flatten();
                match disabled {
                    Some(value) if mapper.canonical(&value) == canon_o => {
                        entry.state = EntryState::Synced;
                    }
                    Some(value) => {
                        entry.state = EntryState::PendingRemove;
                        entry.action = Action::Disable;
                        entry.written = Some(value);
                    }
                    None => {
                        entry.state = EntryState::PendingRemove;
                        entry.action = Action::Remove;
                    }
                }
            } else {
                entry.state = EntryState::Drifted;
                match resolution(&entry) {
                    OnConflict::Overwrite => entry.action = Action::Remove,
                    OnConflict::Import | OnConflict::Release => entry.action = Action::Release,
                    OnConflict::Skip => entry.action = Action::Skip,
                    OnConflict::Ask => {
                        entry.action = Action::Skip;
                        entry.unresolved = true;
                    }
                }
            }
        }
        (None, None, Some(l)) => {
            entry.client = adapter(&l.client).map(|spec| spec.id);
            entry.server_id = l.server_id.clone();
            if !entry
                .client
                .is_some_and(|client| input.clients.contains(&client))
            {
                return None;
            }
            entry.state = EntryState::Synced;
            entry.action = Action::Forget;
        }
        (None, Some(_), None) => {
            entry.state = EntryState::Foreign;
        }
        (None, None, None) => return None,
    }
    Some(entry)
}

/// Sets the action of an entry that needs a decision; `owned` when
/// Loadout owns it (drifted, conflict), so it can be released.
fn decide(
    entry: &mut EntryPlan,
    resolution: OnConflict,
    overwrite: Action,
    desired: NativeValue,
    owned: bool,
) {
    match resolution {
        OnConflict::Overwrite => {
            entry.action = overwrite;
            entry.written = Some(desired);
        }
        OnConflict::Import => entry.action = Action::Import,
        OnConflict::Release if owned => entry.action = Action::Release,
        OnConflict::Skip | OnConflict::Release => entry.action = Action::Skip,
        OnConflict::Ask => {
            entry.action = Action::Skip;
            entry.unresolved = true;
        }
    }
}

/// `value` disabled through the client's native mechanism; `None` when the
/// client has none that lives in the entry.
fn disabled_value(client: &str, value: &NativeValue) -> Option<NativeValue> {
    let mech = adapter(client)?.mcp.as_ref()?.caps.native_disable?;
    let mut entry = value.as_object()?.clone();
    match mech {
        DisableMech::EnabledFalse => {
            entry.insert("enabled", NativeValue::Bool(false));
        }
        DisableMech::DisabledTrue => {
            entry.insert("disabled", NativeValue::Bool(true));
        }
        DisableMech::ListMember(_) => return None,
    }
    Some(NativeValue::Object(entry))
}

/// Plans a list target (Claude Code `disabledMcpServers`): desired members
/// are added and owned, owned members no longer desired are removed.
/// Members the user added are left alone and not reported.
fn plan_list(
    mut plan: TargetPlan,
    mut doc: Document,
    bytes: Option<Vec<u8>>,
    ledger: &Ledger,
    options: &PlanOptions,
) -> TargetPlan {
    let keys = plan.container.clone();
    let list: Vec<&str> = keys.iter().map(String::as_str).collect();
    let observed = match list_members(&doc, &list) {
        Ok(members) => members,
        Err(err) => {
            let err = err.with_path(&plan.file);
            return blocked_target(plan, err);
        }
    };
    let file = plan.file.to_string_lossy().into_owned();
    let (pointer, project_id) = (plan.pointer.clone(), plan.project_id.clone());
    let slot = LedgerSlot {
        scope: plan.scope,
        project_id: project_id.as_deref(),
        file: &file,
        container: &pointer,
    };
    let mut names: Vec<String> = Vec::new();
    for member in &plan.input.members {
        if !names.contains(&member.name) {
            names.push(member.name.clone());
        }
    }
    for row in ledger.in_container(slot) {
        if row.kind == EntryKind::ListMember && !names.contains(&row.key) {
            names.push(row.key.clone());
        }
    }
    for name in names {
        let want = plan.input.members.iter().find(|member| member.name == name);
        let owned = ledger
            .get(&slot, &name)
            .filter(|row| row.kind == EntryKind::ListMember);
        let client = match (want, owned) {
            (Some(member), _) => Some(member.client),
            (None, Some(row)) => adapter(&row.client)
                .map(|spec| spec.id)
                .filter(|client| plan.input.clients.contains(client)),
            (None, None) => None,
        };
        let Some(client) = client else {
            continue;
        };
        let present = observed.contains(&name);
        let member = NativeValue::String(name.clone());
        let mut entry = EntryPlan {
            name: name.clone(),
            client: Some(client),
            server_id: want
                .map(|member| member.server_id.clone())
                .or_else(|| owned.and_then(|row| row.server_id.clone())),
            state: EntryState::Synced,
            action: Action::None,
            unresolved: false,
            problem: None,
            desired: want.map(|_| member.clone()),
            observed: present.then(|| member.clone()),
            written: None,
            delivery: None,
        };
        match (want.is_some(), present, owned.is_some()) {
            (true, true, false) => {
                entry.action = Action::Adopt;
                entry.written = Some(member);
            }
            (true, true, true) => {}
            (true, false, false) => {
                entry.state = EntryState::PendingCreate;
                entry.action = Action::Create;
                entry.written = Some(member);
            }
            (true, false, true) => {
                entry.state = EntryState::Missing;
                // A member has nothing to import: write it again unless
                // the entry is skipped.
                if options.resolution(Some(client), &name) == OnConflict::Skip {
                    entry.action = Action::Skip;
                } else {
                    entry.action = Action::Create;
                    entry.written = Some(member);
                }
            }
            (false, true, true)
                if entry
                    .server_id
                    .as_ref()
                    .is_some_and(|id| plan.input.invalid_servers.contains(id)) =>
            {
                entry.state = EntryState::Blocked;
                entry.problem = Some(not_loading(entry.server_id.as_deref()));
            }
            (false, true, true) => {
                entry.state = EntryState::PendingRemove;
                entry.action = Action::Remove;
            }
            (false, false, true) => entry.action = Action::Forget,
            (false, _, false) => continue,
        }
        plan.entries.push(entry);
    }
    let mut changed = false;
    for entry in &plan.entries {
        let result = match entry.action {
            Action::Create => doc.set_list_member(&list, &entry.name, true),
            Action::Remove => doc.set_list_member(&list, &entry.name, false),
            _ => Ok(false),
        };
        match result {
            Ok(edited) => changed |= edited,
            Err(err) => {
                let err = err.with_path(&plan.file);
                return blocked_target(plan, err);
            }
        }
    }
    if changed {
        let after = doc.to_bytes();
        if bytes.as_deref() != Some(after.as_slice()) {
            plan.after = Some(after);
        }
    }
    plan
}

/// The string members of the list at `list`; empty when it is absent.
pub(crate) fn list_members(doc: &Document, list: &[&str]) -> Result<Vec<String>, CoreError> {
    let Some((last, parent)) = list.split_last() else {
        return Ok(Vec::new());
    };
    match doc.get(parent, last)? {
        None => Ok(Vec::new()),
        Some(NativeValue::Array(items)) => Ok(items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect()),
        Some(_) => Err(CoreError::new(
            ErrorCode::UnexpectedShape,
            format!("`{}` is not a list", list.join(".")),
        )),
    }
}

/// Gate V1 FB-1: `project_key_spelling` when the map a project-keyed
/// container lives in already holds the same folder under another key.
fn key_spelling(target: &Target, doc: &Document, keys: &[String]) -> Option<CoreError> {
    let index = target
        .container
        .segments()
        .iter()
        .position(|segment| *segment == ContainerPath::PROJECT_KEY)?;
    let ours = keys.get(index)?;
    let parent: Vec<&str> = keys[..index].iter().map(String::as_str).collect();
    let other = doc
        .list(&parent)
        .ok()?
        .into_iter()
        .find(|key| key != ours && same_folder(target.os, key, ours))?;
    Some(
        CoreError::new(
            ErrorCode::ProjectKeySpelling,
            format!(
                "`{}` holds this folder as {other:?} too; {} writes {ours:?}, and the client \
                 may read the other key",
                parent.join("."),
                crate::brand::PRODUCT_NAME
            ),
        )
        .with_path(&target.file),
    )
}

fn blocked_target(mut plan: TargetPlan, err: CoreError) -> TargetPlan {
    let input = &plan.input;
    plan.entries = input
        .desired
        .iter()
        .map(|want| {
            let mut entry = blocked_entry(
                &want.name,
                Some(want.client),
                Some(&want.server_id),
                err.clone(),
            );
            entry.delivery = want.delivery;
            entry
        })
        .chain(input.members.iter().map(|member| {
            blocked_entry(
                &member.name,
                Some(member.client),
                Some(&member.server_id),
                err.clone(),
            )
        }))
        .collect();
    plan.problem = Some(err);
    plan.after = None;
    plan
}

/// Why an owned entry of server `id` is left alone: its file does not
/// load, so whether it is still wanted is unknown.
fn not_loading(id: Option<&str>) -> CoreError {
    CoreError::new(
        ErrorCode::ParseError,
        format!(
            "server {:?} does not load; its entry is left alone",
            id.unwrap_or_default()
        ),
    )
}

fn blocked_entry(
    name: &str,
    client: Option<&'static str>,
    server_id: Option<&String>,
    err: CoreError,
) -> EntryPlan {
    EntryPlan {
        name: name.to_owned(),
        client,
        server_id: server_id.cloned(),
        state: EntryState::Blocked,
        action: Action::None,
        unresolved: false,
        problem: Some(err),
        desired: None,
        observed: None,
        written: None,
        delivery: None,
    }
}

/// The canonical form a client's mapper gives `value`.
pub fn canonical(client: &str, value: &NativeValue) -> Option<Canonical> {
    mapper_of(client).map(|mapper| mapper.canonical(value))
}

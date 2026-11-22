//! The planner (SPEC 8.3): desired state, native files and the ledger in;
//! per-file operations, the resulting text and a state per entry out.
//! Planning reads files and never writes anything.

use std::collections::BTreeMap;
use std::path::PathBuf;

use similar::TextDiff;

use crate::adapters::caps::DisableMech;
use crate::adapters::detect::replacing_extension;
use crate::adapters::mapper::{Canonical, EntryMapper, WriteCx};
use crate::adapters::{ADAPTERS, AdapterSpec, Target, adapter};
use crate::apply::ledger::{EntryKind, Ledger, LedgerEntry, LedgerKey, container_pointer};
use crate::apply::render::{DesiredEntry, DesiredState, RenderCx, system_entries};
use crate::error::{CoreError, ErrorCode};
use crate::formats::{Document, NativeValue};
use crate::model::applied::Scope;
use crate::secrets::SecretValue;

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
}

impl OnConflict {
    pub const fn as_str(self) -> &'static str {
        match self {
            OnConflict::Ask => "ask",
            OnConflict::Skip => "skip",
            OnConflict::Overwrite => "overwrite",
            OnConflict::Import => "import",
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

/// Inputs that select and steer a plan.
#[derive(Debug, Clone, Default)]
pub struct PlanOptions {
    /// Only these clients; all when `None`.
    pub clients: Option<Vec<String>>,
    pub on_conflict: OnConflict,
    /// Per-entry resolutions; they win over [`PlanOptions::on_conflict`].
    pub decisions: Vec<Decision>,
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
    /// The whole target is blocked (`parse_error`, `unexpected_shape`).
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
    pub(crate) native_disable: BTreeMap<&'static str, bool>,
    pub(crate) invalid_servers: Vec<String>,
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
    fn mask(&self, text: &str) -> String {
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

/// Plans the system scope (user files) of every selected client that has
/// desired entries or owned entries on this machine.
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

    let mut inputs: Vec<TargetInput> = Vec::new();
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
            let same = inputs.iter_mut().find(|input| {
                input.target.file == target.file && input.target.container == target.container
            });
            match same {
                Some(input) => {
                    input.clients.push(spec.id);
                    input.desired.extend(desired_here.iter().cloned());
                    input.native_disable.insert(spec.id, native_disable);
                    input.target.warnings.extend(target.warnings);
                }
                None => inputs.push(TargetInput {
                    target,
                    clients: vec![spec.id],
                    desired: desired_here.clone(),
                    native_disable: BTreeMap::from([(spec.id, native_disable)]),
                    invalid_servers: desired.invalid.keys().cloned().collect(),
                }),
            }
        }
    }
    for input in inputs {
        let bytes = read_target(cx.fs, &input.target.file)?;
        plan.targets
            .push(plan_target(cx, input, bytes, ledger, key, options));
    }
    Ok(plan)
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
    let parsed = match &bytes {
        Some(bytes) => Document::parse(bytes, target.format),
        None => Ok(target.new_document()),
    };
    let mut doc = match parsed {
        Ok(doc) => doc,
        Err(err) => return blocked_target(plan, err.with_path(&target.file)),
    };
    let keys = match target.container_keys() {
        Ok(keys) => keys,
        Err(err) => return blocked_target(plan, err.with_path(&target.file)),
    };
    let container: Vec<&str> = keys.iter().map(String::as_str).collect();
    let observed_names = match doc.list(&container) {
        Ok(names) => names,
        Err(err) => return blocked_target(plan, err.with_path(&target.file)),
    };
    plan.pointer = container_pointer(&keys);
    plan.container = keys.clone();
    let file = target.file.to_string_lossy().into_owned();

    // Names in file order, then new desired names, then ledger-only names.
    let mut names: Vec<String> = observed_names.clone();
    for entry in &input.desired {
        if !names.contains(&entry.name) {
            names.push(entry.name.clone());
        }
    }
    for entry in ledger.in_container(&file, &plan.pointer) {
        if entry.kind == EntryKind::McpEntry && !names.contains(&entry.key) {
            names.push(entry.key.clone());
        }
    }
    let foreign_names: Vec<String> = observed_names
        .iter()
        .filter(|name| ledger.get(&file, &plan.pointer, name).is_none())
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
            .get(&file, &plan.pointer, &name)
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
    plan
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
            let native = match &want.server {
                Ok(server) => mapper_of(want.client)
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
                Err(err) => Err(err.clone()),
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
                OnConflict::Skip => entry.action = Action::Skip,
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
                decide(&mut entry, resolved, Action::AdoptOverwrite, d);
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
                decide(&mut entry, resolved, Action::Update, d);
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
                entry.problem = Some(CoreError::new(
                    ErrorCode::ParseError,
                    format!(
                        "server {:?} does not load; its entry is left alone",
                        l.server_id.as_deref().unwrap_or_default()
                    ),
                ));
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
                    OnConflict::Import => entry.action = Action::Release,
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

/// Sets the action of an entry that needs a decision.
fn decide(entry: &mut EntryPlan, resolution: OnConflict, overwrite: Action, desired: NativeValue) {
    match resolution {
        OnConflict::Overwrite => {
            entry.action = overwrite;
            entry.written = Some(desired);
        }
        OnConflict::Import => entry.action = Action::Import,
        OnConflict::Skip => entry.action = Action::Skip,
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

fn blocked_target(mut plan: TargetPlan, err: CoreError) -> TargetPlan {
    plan.entries = plan
        .input
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
        .collect();
    plan.problem = Some(err);
    plan.after = None;
    plan
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

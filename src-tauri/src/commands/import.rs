//! Import of foreign client entries (SPEC 8.6, 11.7; #3, #9): the scan of
//! every detected client's user files and every known project's files, the
//! import of the chosen definitions as one commit with its instant apply,
//! and the unmanaged-entry summary with this machine's acknowledgments.
//!
//! Paths go out under their placeholder roots (`~`, `<P>`); values never
//! go out: a literal secret is shown as the `${secret:<alias>}` reference
//! it becomes, and a keyring destination takes the value from the client
//! file inside core, so no choice carries one.

use agent_loadout_core::apply::import::{
    self, Candidate, ImportRequest, ScanFilter, ScanScope, SecretChoice, SecretDestination,
    VariantChoice, VariantPreview,
};
use agent_loadout_core::apply::ledger::Ledger;
use agent_loadout_core::apply::placeholder_path;
use agent_loadout_core::apply::render::DesiredState;
use agent_loadout_core::apply::unmanaged::{self, Seen, finding_id};
use agent_loadout_core::secrets::extract::env_reference;
use agent_loadout_core::store::Operation;
use agent_loadout_core::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use specta::Type;

use super::servers::{ServerDraft, draft_of};
use super::{
    AppState, ChangeResult, CmdResult, Core, FileScope, IpcError, blocking, count, scrubbed,
};

/// Which client files a scan reads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImportScope {
    /// User files and every known project's files.
    All,
    /// User files only.
    User,
    /// Every known project's files.
    Projects,
    /// One known project's files.
    Project { id: String },
}

/// The core scope of `scope` (none: everything).
fn scan_scope(scope: Option<ImportScope>) -> ScanScope {
    match scope {
        None | Some(ImportScope::All) => ScanScope::All,
        Some(ImportScope::User) => ScanScope::User,
        Some(ImportScope::Projects) => ScanScope::Projects,
        Some(ImportScope::Project { id }) => ScanScope::Project(id),
    }
}

/// One file a foreign entry was found in.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportSource {
    /// Picks this source's definition (`ImportPick::source`):
    /// `<client>`, `<client>@<project-id>` or `<client>@<project-id>/local`.
    pub key: String,
    /// The finding (`acknowledge_unmanaged`).
    pub id: String,
    pub client: String,
    pub scope: FileScope,
    pub project_id: Option<String>,
    /// For a project file: its manifests are trusted on this machine.
    pub trusted: Option<bool>,
    /// Under its placeholder root.
    pub file: String,
}

/// A literal that may be a secret: it needs a destination.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportSecret {
    /// `env.NAME`, `headers.NAME`, `args[N]` or `url`.
    pub field: String,
    /// The alias it gets when imported under the suggested id.
    pub suggested_alias: String,
    /// Why it counts as sensitive (`sensitive_key`, `token_format`, ...).
    pub reason: String,
}

/// One definition of an entry, with every source that has it.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportVariant {
    /// The `source` of an `ImportPick` for this definition.
    pub key: String,
    /// The store id it gets by default: the name, or `name-<client>` for
    /// a second definition.
    pub suggested_id: String,
    /// The server as it would be stored, `${secret:<alias>}` in place of
    /// each literal secret; `None` when it cannot be imported.
    pub draft: Option<ServerDraft>,
    /// Why it cannot be imported.
    pub error: Option<IpcError>,
    /// Found in a user file Loadout writes, so importing enables it for
    /// those clients. Otherwise (project files only) the import is a
    /// definition only; offer "add to the project's manifest"
    /// (`save_project_server` with `from`, after a trust review for an
    /// untrusted project).
    pub enables: bool,
    pub secrets: Vec<ImportSecret>,
    pub sources: Vec<ImportSource>,
}

/// Foreign entries sharing one name.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportCandidate {
    pub name: String,
    /// The store has a server with this id already.
    pub in_store: bool,
    /// More than one definition can be imported: pick one, or import
    /// another as `name-<client>`.
    pub conflict: bool,
    pub variants: Vec<ImportVariant>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportScan {
    pub candidates: Vec<ImportCandidate>,
    pub warnings: Vec<IpcError>,
}

fn file_of(core: &Core, source: &import::Source) -> String {
    placeholder_path(
        &core.env,
        &source.file,
        source.project.as_ref().map(|p| p.root.as_path()),
    )
}

fn candidate_of(
    core: &Core,
    candidate: &Candidate,
    previews: &[VariantPreview],
) -> ImportCandidate {
    let variants = candidate
        .variants
        .iter()
        .zip(previews)
        .enumerate()
        .map(|(v, (indexes, preview))| {
            let sources: Vec<ImportSource> = indexes
                .iter()
                .map(|&i| {
                    let source = &candidate.sources[i];
                    ImportSource {
                        key: source.key(),
                        id: finding_id(source, &candidate.name),
                        client: source.client.to_owned(),
                        scope: source.scope.into(),
                        project_id: source.project.as_ref().map(|p| p.id.clone()),
                        trusted: source.project.as_ref().map(|p| p.trusted),
                        file: file_of(core, source),
                    }
                })
                .collect();
            ImportVariant {
                key: sources[0].key.clone(),
                suggested_id: preview.id.clone(),
                draft: preview.server.as_ref().ok().map(draft_of),
                error: preview
                    .server
                    .as_ref()
                    .err()
                    .map(|err| scrubbed(&core.env, err, None)),
                enables: candidate.enables(v),
                secrets: preview
                    .extractions
                    .iter()
                    .map(|extraction| ImportSecret {
                        field: extraction.field.clone(),
                        suggested_alias: extraction.alias.clone(),
                        reason: extraction.reason.to_owned(),
                    })
                    .collect(),
                sources,
            }
        })
        .collect();
    ImportCandidate {
        name: candidate.name.clone(),
        in_store: candidate.in_store,
        conflict: candidate.conflict(),
        variants,
    }
}

/// The scan: foreign entries grouped by name, each definition with its
/// sources, what it would store and the secrets it needs a destination
/// for. Reads only; creates nothing.
pub fn scan(core: &Core, scope: Option<ImportScope>) -> CmdResult<ImportScan> {
    let store = core.open_store()?;
    let filter = ScanFilter {
        clients: None,
        scope: scan_scope(scope),
    };
    let scan = import::scan_store(store.as_ref(), &core.apply_env(), &filter)
        .map_err(|err| scrubbed(&core.env, &err, None))?;
    let previews = import::previews(&scan.candidates, &scan.desired.catalog);
    Ok(ImportScan {
        candidates: scan
            .candidates
            .iter()
            .zip(&previews)
            .map(|(candidate, previews)| candidate_of(core, candidate, previews))
            .collect(),
        warnings: scan
            .warnings
            .iter()
            .map(|err| scrubbed(&core.env, err, None))
            .collect(),
    })
}

/// One definition to import under `id`.
#[derive(Debug, Clone, Deserialize, Type)]
pub struct ImportPick {
    /// The candidate's name.
    pub name: String,
    /// Any source key of the definition (`ImportVariant::key`).
    pub source: String,
    /// The store id: the name, or `name-<client>` for a second definition.
    pub id: String,
}

/// Where a literal secret goes (SPEC 11.7). No value: a keyring
/// destination takes it from the client file inside core.
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImportSecretTarget {
    /// The OS keyring (`keyring://loadout/<alias>`).
    Keyring,
    /// A 1Password item (`op://vault/item/field`, or
    /// `op://vault/item/section/field`) or Environment variable
    /// (`op-env://<environment-id>/<NAME>`).
    Onepassword { reference: String },
    /// An environment variable name (`env:NAME`).
    Env { name: String },
}

/// The destination of one candidate's secrets: one `field`, or with
/// `field` `None` each one no other choice names.
#[derive(Debug, Clone, Deserialize, Type)]
pub struct ImportSecretChoice {
    pub name: String,
    pub field: Option<String>,
    pub target: ImportSecretTarget,
}

/// What to import. A candidate without a pick is skipped; a definition
/// with a literal secret and no destination for it is skipped
/// (`needs_secret_extraction`).
#[derive(Debug, Clone, Deserialize, Type)]
pub struct ImportChoices {
    pub picks: Vec<ImportPick>,
    pub secrets: Vec<ImportSecretChoice>,
    /// Copy the definitions without enabling them (no ownership taken).
    pub only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SecretDestinationKind {
    Keyring,
    Onepassword,
    Env,
}

/// A secret alias the import created.
#[derive(Debug, Clone, Serialize, Type)]
pub struct ImportedSecret {
    pub alias: String,
    pub server: String,
    pub field: String,
    pub destination: SecretDestinationKind,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct ImportSkipped {
    pub name: String,
    pub error: IpcError,
}

/// What the import did: one commit, applied with it in instant mode.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ImportApplied {
    /// New server ids.
    pub imported: Vec<String>,
    /// New servers found only in project files: definitions, enabled
    /// nowhere; a project's manifest can name them (`from`).
    pub definition_only: Vec<String>,
    pub skipped: Vec<ImportSkipped>,
    pub secrets: Vec<ImportedSecret>,
    pub change: ChangeResult,
}

fn destination_of(target: ImportSecretTarget) -> Result<SecretDestination, CoreError> {
    Ok(match target {
        ImportSecretTarget::Keyring => SecretDestination::Keyring,
        ImportSecretTarget::Onepassword { reference } => {
            let reference = reference.trim();
            if !(reference.starts_with("op://") || reference.starts_with("op-env://")) {
                return Err(CoreError::new(
                    ErrorCode::InvalidValue,
                    "a 1Password reference is op://vault/item/field or \
                     op-env://<environment>/<NAME>",
                ));
            }
            SecretDestination::parse(reference)?
        }
        ImportSecretTarget::Env { name } => SecretDestination::Reference(env_reference(&name)?),
    })
}

fn kind_of(destination: &str) -> SecretDestinationKind {
    match destination {
        "keyring" => SecretDestinationKind::Keyring,
        "env" => SecretDestinationKind::Env,
        _ => SecretDestinationKind::Onepassword,
    }
}

/// Imports the chosen definitions in one commit (with its instant apply):
/// entries that render identically are adopted without a write, the
/// others are rewritten (instant mode) or wait in Pending changes (review
/// mode). The scan runs again inside the transaction; a pick whose source
/// is gone is skipped.
pub fn apply_choices(core: &Core, choices: ImportChoices) -> CmdResult<ImportApplied> {
    let fail = |err: &CoreError| scrubbed(&core.env, err, None);
    let request = ImportRequest {
        picks: choices
            .picks
            .into_iter()
            .map(|pick| VariantChoice {
                name: pick.name,
                source: pick.source,
                id: pick.id,
            })
            .collect(),
        secrets: choices
            .secrets
            .into_iter()
            .map(|choice| {
                Ok(SecretChoice {
                    name: choice.name,
                    field: choice.field,
                    destination: destination_of(choice.target)?,
                })
            })
            .collect::<Result<_, CoreError>>()
            .map_err(|err| fail(&err))?,
        only: choices.only,
        unchosen: Vec::new(),
    };
    if request.picks.is_empty() {
        return Err(fail(&CoreError::new(
            ErrorCode::InvalidValue,
            "nothing is chosen to import",
        )));
    }
    let store = core.open_store_for_change()?;
    let ax = core.apply_env();
    let committed = store
        .transaction(Operation::new("server.import", "import"), |tx| {
            let desired = DesiredState::load(tx, Some(&tx.machine().id.clone()))?;
            let ledger = Ledger::load(ax.fs, &core.home.state_dir())?;
            let (candidates, _) =
                import::scan_with(&ax, &desired, &ledger, &ScanFilter::default())?;
            let result = import::stage_import(tx, &ax, &candidates, &request)?;
            let n = result.imported.len();
            tx.set_summary(format!(
                "import: {n} server{} ({})",
                if n == 1 { "" } else { "s" },
                result.imported.join(", ")
            ));
            let report = import::apply_import(tx, &ax, &result)?;
            Ok((result, report))
        })
        .map_err(|err| fail(&err))?;
    let (result, report) = committed.value;
    Ok(ImportApplied {
        imported: result.imported,
        definition_only: result.definition_only,
        skipped: result
            .skipped
            .iter()
            .map(|(name, err)| ImportSkipped {
                name: name.clone(),
                error: fail(err),
            })
            .collect(),
        secrets: result
            .aliases
            .into_iter()
            .map(|alias| ImportedSecret {
                destination: kind_of(alias.destination),
                alias: alias.alias,
                server: alias.server,
                field: alias.field,
            })
            .collect(),
        change: ChangeResult::new(&core.env, committed.commit.as_ref(), report.as_ref()),
    })
}

/// One unmanaged entry.
#[derive(Debug, Clone, Serialize, Type)]
pub struct UnmanagedEntry {
    /// The finding (`acknowledge_unmanaged`).
    pub id: String,
    pub name: String,
    /// Not acknowledged on this machine yet.
    pub new: bool,
}

/// The unmanaged entries of one client in one scope (one project).
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UnmanagedGroup {
    pub client: String,
    pub scope: FileScope,
    pub project_id: Option<String>,
    /// Under their placeholder roots.
    pub files: Vec<String>,
    pub entries: Vec<UnmanagedEntry>,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct UnmanagedSummary {
    pub groups: Vec<UnmanagedGroup>,
    pub total: u32,
    /// Entries not acknowledged on this machine yet.
    pub new: u32,
}

/// Unmanaged entries per client and scope, each marked new unless this
/// machine acknowledged it. With `only_new`, only new entries (and the
/// groups holding one).
pub fn summary(
    core: &Core,
    scope: Option<ImportScope>,
    only_new: bool,
) -> CmdResult<UnmanagedSummary> {
    let fail = |err: CoreError| scrubbed(&core.env, &err, None);
    let store = core.open_store()?;
    let filter = ScanFilter {
        clients: None,
        scope: scan_scope(scope),
    };
    let scan = import::scan_store(store.as_ref(), &core.apply_env(), &filter).map_err(fail)?;
    let seen = Seen::load(core.fs.as_ref(), &core.home.state_dir()).map_err(fail)?;
    let mut total = 0;
    let mut new = 0;
    let groups = unmanaged::groups(unmanaged::findings(&scan.candidates), &seen)
        .into_iter()
        .filter_map(|group| {
            let root = group.project_root.as_deref();
            let entries: Vec<UnmanagedEntry> = group
                .entries
                .into_iter()
                .filter(|(_, is_new)| *is_new || !only_new)
                .map(|(finding, is_new)| UnmanagedEntry {
                    id: finding.id,
                    name: finding.name,
                    new: is_new,
                })
                .collect();
            if entries.is_empty() {
                return None;
            }
            total += entries.len();
            new += entries.iter().filter(|entry| entry.new).count();
            Some(UnmanagedGroup {
                client: group.client.to_owned(),
                scope: group.scope.into(),
                project_id: group.project_id,
                files: group
                    .files
                    .iter()
                    .map(|file| placeholder_path(&core.env, file, root))
                    .collect(),
                entries,
            })
        })
        .collect();
    Ok(UnmanagedSummary {
        groups,
        total: count(total),
        new: count(new),
    })
}

/// Acknowledges findings on this machine (`state/unmanaged-seen.json`),
/// so the notice does not show them again. Returns how many were new.
pub fn acknowledge(core: &Core, ids: &[String]) -> CmdResult<u32> {
    let state = core.home.state_dir();
    let mut seen = Seen::load(core.fs.as_ref(), &state)?;
    let added = seen
        .acknowledge(core.fs.as_ref(), &state, ids)
        .map_err(|err| scrubbed(&core.env, &err, None))?;
    Ok(count(added))
}

#[tauri::command]
#[specta::specta]
pub async fn scan_import(
    state: tauri::State<'_, AppState>,
    scope: Option<ImportScope>,
) -> CmdResult<ImportScan> {
    blocking(&state, move |core| scan(core, scope)).await
}

#[tauri::command]
#[specta::specta]
pub async fn apply_import(
    state: tauri::State<'_, AppState>,
    choices: ImportChoices,
) -> CmdResult<ImportApplied> {
    blocking(&state, move |core| apply_choices(core, choices)).await
}

#[tauri::command]
#[specta::specta]
pub async fn unmanaged_summary(
    state: tauri::State<'_, AppState>,
    scope: Option<ImportScope>,
) -> CmdResult<UnmanagedSummary> {
    blocking(&state, move |core| summary(core, scope, false)).await
}

#[tauri::command]
#[specta::specta]
pub async fn new_unmanaged(state: tauri::State<'_, AppState>) -> CmdResult<UnmanagedSummary> {
    blocking(&state, move |core| summary(core, None, true)).await
}

#[tauri::command]
#[specta::specta]
pub async fn acknowledge_unmanaged(
    state: tauri::State<'_, AppState>,
    ids: Vec<String>,
) -> CmdResult<u32> {
    blocking(&state, move |core| acknowledge(core, &ids)).await
}

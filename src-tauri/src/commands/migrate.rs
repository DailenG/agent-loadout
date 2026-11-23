//! Migration wizard commands (SPEC 15.1): the dry-run report of SkillDock's
//! `~/.skilldock` (read only), the migration itself (one commit after an
//! `auto/pre-migrate/<ts>` tag), and "Replace SkillDock wrapper" for a
//! wrapped client entry afterwards. The source is the default
//! `~/.skilldock` only; no command takes a path, and paths go out under
//! their placeholder roots. Report details never hold a value (core routes
//! plaintext values to the extraction list by alias); secrets themselves
//! are set through the secrets commands (`set_secret_keyring_value`,
//! `set_secret_reference`).

use agent_loadout_core::apply::resolve::{filter_of, fingerprint, resolve};
use agent_loadout_core::apply::{
    self, Action, EntryState, OnConflict, PlanOptions, placeholder_path,
};
use agent_loadout_core::migrate::{self, ItemKind, ItemStatus, Offer, Report};
use agent_loadout_core::model::applied::Scope as TargetScope;
use agent_loadout_core::secrets::SecretValue;
use agent_loadout_core::secrets::extract::{
    ExtractionChoice, ExtractionTarget, env_reference, onepassword_reference,
};
use agent_loadout_core::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use specta::Type;

use super::{AppState, ChangeResult, CmdResult, Core, IpcError, blocking, count, scrub, scrubbed};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MigrationItemKind {
    Server,
    Skill,
    Setting,
    ClientEntry,
    SkillLink,
    Credential,
    Backup,
    Plugin,
    Skillhub,
    File,
}

impl From<ItemKind> for MigrationItemKind {
    fn from(kind: ItemKind) -> Self {
        match kind {
            ItemKind::Server => Self::Server,
            ItemKind::Skill => Self::Skill,
            ItemKind::Setting => Self::Setting,
            ItemKind::ClientEntry => Self::ClientEntry,
            ItemKind::SkillLink => Self::SkillLink,
            ItemKind::Credential => Self::Credential,
            ItemKind::Backup => Self::Backup,
            ItemKind::Plugin => Self::Plugin,
            ItemKind::Skillhub => Self::Skillhub,
            ItemKind::File => Self::File,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MigrationItemStatus {
    Migrated,
    NeedsReview,
    Skipped,
}

impl From<ItemStatus> for MigrationItemStatus {
    fn from(status: ItemStatus) -> Self {
        match status {
            ItemStatus::Migrated => Self::Migrated,
            ItemStatus::NeedsReview => Self::NeedsReview,
            ItemStatus::Skipped => Self::Skipped,
        }
    }
}

/// A follow-up of a report item. Entries and aliases, never paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MigrationOffer {
    /// "Replace SkillDock wrapper" (`replace_skilldock_wrapper`).
    ReplaceWrapper { client: String, name: String },
    /// A different native entry: decide in Pending changes.
    ReviewEntry { client: String, name: String },
    /// A value replaced by `${secret:<alias>}`: the extraction step.
    SetSecret { alias: String },
    /// An absolute command: machine-specific.
    MachinePath { server: String },
    /// SkillDock's GitHub token: a token remote in Settings.
    KeyringToken,
    /// SkillDock's GitHub backup repository as a git remote (URL with any
    /// userinfo masked).
    AddRemote { url: String },
}

/// One line of the report.
#[derive(Debug, Clone, Serialize, Type)]
pub struct MigrationItem {
    pub kind: MigrationItemKind,
    pub name: String,
    pub status: MigrationItemStatus,
    /// English detail (paths under placeholders).
    pub detail: String,
    /// The error code behind a review or skip.
    pub code: Option<ErrorCode>,
    /// The store id it became.
    pub id: Option<String>,
    pub offers: Vec<MigrationOffer>,
}

/// A plaintext value replaced by a reference; the value itself never
/// leaves core.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MigrationExtraction {
    pub server: String,
    /// `env.NAME`, `headers.NAME`, `args[N]` or `url`.
    pub field: String,
    pub alias: String,
    /// Why it counts as sensitive.
    pub reason: String,
    /// The default destination (`keyring://loadout/<alias>`).
    pub suggested_ref: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MigrationCounts {
    pub migrated: u32,
    pub needs_review: u32,
    pub skipped: u32,
}

/// The report of a dry run or a migration.
#[derive(Debug, Clone, Serialize, Type)]
pub struct MigrationReport {
    /// `~/.skilldock`, under its placeholder root.
    pub source: String,
    pub items: Vec<MigrationItem>,
    pub extractions: Vec<MigrationExtraction>,
    pub servers: Vec<String>,
    pub skills: Vec<String>,
    pub counts: MigrationCounts,
    /// Store files the migration writes (dry run).
    pub files: Vec<String>,
}

/// What the migration did.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MigrationApplied {
    pub report: MigrationReport,
    /// `auto/pre-migrate/<ts>`.
    pub tag: String,
    /// `None` when nothing was left to migrate.
    pub commit: Option<String>,
    pub post_tag: Option<String>,
    /// SkillDock's backup repository was cloned into `imports/` now.
    pub backup_imported: bool,
}

fn offer_of(core: &Core, offer: &Offer) -> MigrationOffer {
    match offer {
        Offer::ReplaceWrapper { client, name, .. } => MigrationOffer::ReplaceWrapper {
            client: client.clone(),
            name: name.clone(),
        },
        Offer::ReviewEntry { client, name, .. } => MigrationOffer::ReviewEntry {
            client: client.clone(),
            name: name.clone(),
        },
        Offer::SetSecret { alias, .. } => MigrationOffer::SetSecret {
            alias: alias.clone(),
        },
        Offer::MachinePath { server } => MigrationOffer::MachinePath {
            server: server.clone(),
        },
        Offer::KeyringToken { .. } => MigrationOffer::KeyringToken,
        Offer::AddRemote { url } => MigrationOffer::AddRemote {
            url: scrub(&core.env, &super::remotes::mask_url(url), None),
        },
    }
}

fn report_of(core: &Core, report: &Report, files: Vec<String>) -> MigrationReport {
    let env = &core.env;
    let (migrated, needs_review, skipped) = report.counts();
    MigrationReport {
        source: placeholder_path(env, &env.home().join(".skilldock"), None),
        items: report
            .items
            .iter()
            .map(|item| MigrationItem {
                kind: item.kind.into(),
                name: scrub(env, &item.name, None),
                status: item.status.into(),
                detail: scrub(env, &item.detail, None),
                code: item.code.and_then(|code| {
                    serde_json::from_value::<ErrorCode>(serde_json::Value::from(code)).ok()
                }),
                id: item.id.clone(),
                offers: item
                    .offers
                    .iter()
                    .map(|offer| offer_of(core, offer))
                    .collect(),
            })
            .collect(),
        extractions: report
            .extractions
            .iter()
            .map(|extraction| MigrationExtraction {
                server: extraction.server.clone(),
                field: extraction.field.clone(),
                alias: extraction.alias.clone(),
                reason: extraction.reason.to_owned(),
                suggested_ref: extraction.suggested_ref.clone(),
            })
            .collect(),
        servers: report.servers.clone(),
        skills: report.skills.clone(),
        counts: MigrationCounts {
            migrated: count(migrated),
            needs_review: count(needs_review),
            skipped: count(skipped),
        },
        files,
    }
}

/// The dry run: what migrating `~/.skilldock` would do. Creates nothing
/// (no store, no state); `not_found` when there is no SkillDock data.
pub fn plan(core: &Core) -> CmdResult<MigrationReport> {
    let store = core.open_store()?;
    let migration = migrate::plan(store.as_ref(), &core.apply_env())
        .map_err(|err| scrubbed(&core.env, &err, None))?;
    let files = migration.files().map(str::to_owned).collect();
    Ok(report_of(core, &migration.report, files))
}

/// Where the wizard sends one extracted value (SPEC 11.7). Deserialized
/// only: a keyring value comes in once and never goes back out.
#[derive(Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MigrationSecretChoice {
    /// The value for the OS keyring (`keyring://loadout/<alias>`).
    Keyring { alias: String, value: String },
    /// A 1Password item reference: `op://vault/item/field`, or
    /// `op://vault/item/section/field`.
    Onepassword { alias: String, reference: String },
    /// An environment variable name (`env:NAME`).
    Env { alias: String, name: String },
}

fn invalid(message: &str) -> CoreError {
    CoreError::new(ErrorCode::InvalidValue, message.to_owned())
}

/// The core choice of `choice`; references are checked by core (an
/// `op://` item reference of three or four non-empty parts, a valid
/// variable name).
fn choice_of(choice: MigrationSecretChoice) -> Result<ExtractionChoice, CoreError> {
    Ok(match choice {
        MigrationSecretChoice::Keyring { alias, value } => {
            if value.is_empty() {
                return Err(invalid("the value is empty"));
            }
            ExtractionChoice {
                alias,
                target: ExtractionTarget::Keyring(SecretValue::new(&value)),
            }
        }
        MigrationSecretChoice::Onepassword { alias, reference } => ExtractionChoice {
            alias,
            target: ExtractionTarget::Reference(onepassword_reference(&reference)?),
        },
        MigrationSecretChoice::Env { alias, name } => ExtractionChoice {
            alias,
            target: ExtractionTarget::Reference(env_reference(&name)?),
        },
    })
}

/// Migrates (the wizard's confirmation was given): one commit, tagged
/// before and after, holding the chosen secret aliases too (keyring values
/// written right before it). No client file is written.
pub fn apply_migration(
    core: &Core,
    choices: Vec<MigrationSecretChoice>,
) -> CmdResult<MigrationApplied> {
    let choices = choices
        .into_iter()
        .map(choice_of)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| scrubbed(&core.env, &err, None))?;
    let store = core.open_store_for_change()?;
    let applied = migrate::apply_with_choices(&store, &core.apply_env(), &choices)
        .map_err(|err| scrubbed(&core.env, &err, None))?;
    Ok(MigrationApplied {
        report: report_of(core, &applied.report, Vec::new()),
        tag: applied.tag,
        commit: applied.commit.map(|commit| commit.to_string()),
        post_tag: applied.post_tag,
        backup_imported: applied.backup_import.is_some(),
    })
}

/// "Replace SkillDock wrapper": the user-scope entry `name` of `client`
/// must be a collision whose native value is a SkillDock wrapper
/// (`mcp-filter`, `mcp-remote`), else `invalid_value`; it is overwritten
/// with the store's native definition, and only its file is applied (one
/// commit, like the resolver's Overwrite). `not_found` when there is no
/// such entry; `invalid_value` when the apply did not act on it.
pub fn replace_wrapper(core: &Core, client: &str, name: &str) -> CmdResult<ChangeResult> {
    let fail = |err: CoreError| scrubbed(&core.env, &err, None);
    if client.is_empty() || name.is_empty() {
        return Err(fail(invalid("a client and an entry name are needed")));
    }
    let ax = core.apply_env();
    let store = core.open_store()?.ok_or_else(|| {
        IpcError::from(CoreError::new(ErrorCode::NotFound, "there is no store yet"))
    })?;
    let plan = apply::preview(Some(&store), &ax, &PlanOptions::default()).map_err(fail)?;
    let (target, entry) = plan
        .targets
        .iter()
        .filter(|target| target.scope == TargetScope::User)
        .find_map(|target| {
            target
                .entries
                .iter()
                .find(|entry| entry.name == name && entry.client == Some(client))
                .map(|entry| (target, entry))
        })
        .ok_or_else(|| {
            fail(CoreError::new(
                ErrorCode::NotFound,
                format!("{client}/{name} is not in the pending plan"),
            ))
        })?;
    let wrapped = entry
        .observed
        .as_ref()
        .and_then(migrate::scan::native_wrapper)
        .is_some();
    if entry.state != EntryState::Collision || !wrapped {
        return Err(fail(invalid(&format!(
            "{client}/{name} is not a SkillDock wrapper that collides with a store server"
        ))));
    }
    let shown = fingerprint(&ax, entry).map_err(fail)?;
    drop(store);
    let store = core.open_store_for_change()?;
    let (report, commit) = resolve(
        &store,
        &ax,
        filter_of(target),
        client,
        name,
        entry.state,
        OnConflict::Overwrite,
        &shown,
    )
    .map_err(fail)?;
    let acted = report.targets.iter().flat_map(|t| &t.entries).any(|e| {
        e.name == name
            && e.client == Some(client)
            && !matches!(e.action, Action::None | Action::Skip)
    });
    if !acted {
        return Err(fail(invalid(&format!(
            "{client}/{name} could not be replaced; see Pending changes"
        ))));
    }
    Ok(ChangeResult::new(&core.env, commit.as_ref(), Some(&report)))
}

#[tauri::command]
#[specta::specta]
pub async fn plan_skilldock_migration(
    state: tauri::State<'_, AppState>,
) -> CmdResult<MigrationReport> {
    blocking(&state, plan).await
}

#[tauri::command]
#[specta::specta]
pub async fn apply_skilldock_migration(
    state: tauri::State<'_, AppState>,
    choices: Vec<MigrationSecretChoice>,
) -> CmdResult<MigrationApplied> {
    blocking(&state, move |core| apply_migration(core, choices)).await
}

#[tauri::command]
#[specta::specta]
pub async fn replace_skilldock_wrapper(
    state: tauri::State<'_, AppState>,
    client: String,
    name: String,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| replace_wrapper(core, &client, &name)).await
}

//! Settings > Secrets and the missing-secret checklist (SPEC 13.6 item 5,
//! 11.9 to 11.11): providers (detection only), the alias list, "Check all"
//! (which resolves and may make 1Password ask for approval, so it runs on
//! request only), the quick fixes (a reference, a value into the keyring,
//! an environment variable; on this machine only when asked), the
//! 1Password service account token, the 1Password account, and "Map a
//! 1Password Environment".
//!
//! No value ever comes back: statuses only. Values go one way, into the
//! keyring. Every store change is one commit applied at once (instant
//! mode), like the CLI's `secrets set`.

use std::collections::BTreeSet;
use std::sync::Arc;

use agent_loadout_core::apply::instant_apply;
use agent_loadout_core::apply::render::DesiredState;
use agent_loadout_core::model::ids::{self, IdKind};
use agent_loadout_core::secrets::SecretValue;
use agent_loadout_core::secrets::catalog::{
    self, AliasUpdate, Catalog, RefSource, SetScope, keyring_parts,
};
use agent_loadout_core::secrets::delivery::{SERVICE_ACCOUNT_KEYRING_ACCOUNT, op_env};
use agent_loadout_core::secrets::grammar::{SecretRef, is_op_account, is_op_environment_id};
use agent_loadout_core::secrets::providers::onepassword::OpFeature;
use agent_loadout_core::secrets::providers::{self, ProviderKind, ProviderState, Resolver};
use agent_loadout_core::secrets::scanner::SecretMemory;
use agent_loadout_core::secrets::status::SecretStatus;
use agent_loadout_core::store::{Machine, Operation, Store};
use agent_loadout_core::{CoreError, ErrorCode, brand};
use serde::{Deserialize, Serialize};
use specta::Type;

use super::{AppState, ChangeResult, CmdResult, Core, blocking, scrub};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKindDto {
    Env,
    Keyring,
    Onepassword,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStateDto {
    Available,
    /// "1Password CLI not found" (SPEC 11.11).
    Unavailable,
    /// Installed, but its version could not be read.
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum OpFeatureDto {
    ItemRead,
    Environments,
    EnvironmentListing,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderFeature {
    pub feature: OpFeatureDto,
    pub available: bool,
}

/// One secret provider of this machine. Detection only: nothing signs in.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEntry {
    pub kind: ProviderKindDto,
    pub state: ProviderStateDto,
    pub version: Option<String>,
    /// The keyring backend's name ("Windows Credential Manager").
    pub backend: Option<String>,
    pub features: Vec<ProviderFeature>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RefSourceDto {
    /// `secrets.toml`, shared by every machine.
    Store,
    /// This machine's overlay.
    Machine,
}

impl From<RefSource> for RefSourceDto {
    fn from(source: RefSource) -> Self {
        match source {
            RefSource::Store => Self::Store,
            RefSource::Machine => Self::Machine,
        }
    }
}

/// An alias as the list shows it, without resolving it.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AliasEntry {
    pub alias: String,
    /// The reference in force here (`op://...`, `keyring://...`, `env:X`).
    pub reference: String,
    pub source: RefSourceDto,
    /// The shared reference, when this machine overrides it.
    pub store_reference: Option<String>,
    pub description: Option<String>,
    pub env_name: String,
    pub servers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SecretsOverview {
    pub aliases: Vec<AliasEntry>,
    /// Aliases servers use that no catalog defines.
    pub undefined: Vec<String>,
    /// `[onepassword] environment`.
    pub environment: Option<String>,
    /// `[onepassword] account`.
    pub op_account: Option<String>,
    pub service_account_set: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SecretStatusKind {
    Resolved,
    Missing,
    Locked,
    ProviderUnavailable,
    ProviderTooOld,
    Unverifiable,
    Error,
}

impl From<SecretStatus> for SecretStatusKind {
    fn from(status: SecretStatus) -> Self {
        match status {
            SecretStatus::Resolved => Self::Resolved,
            SecretStatus::Missing => Self::Missing,
            SecretStatus::Locked => Self::Locked,
            SecretStatus::ProviderUnavailable => Self::ProviderUnavailable,
            SecretStatus::ProviderTooOld => Self::ProviderTooOld,
            SecretStatus::Unverifiable => Self::Unverifiable,
            SecretStatus::Error => Self::Error,
        }
    }
}

/// A server that needs an alias, with the clients it is enabled for.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NeededBy {
    pub server: String,
    /// Client ids; `all-detected` for every detected client; empty when
    /// the server is not enabled.
    pub clients: Vec<String>,
}

/// One row of the missing-secret checklist (SPEC 11.9).
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AliasCheck {
    pub alias: String,
    pub reference: Option<String>,
    pub source: Option<RefSourceDto>,
    pub status: SecretStatusKind,
    /// English detail; never a value.
    pub message: Option<String>,
    pub needed_by: Vec<NeededBy>,
    /// Used by a server enabled for at least one client: apply blocks those
    /// entries while the alias is missing.
    pub blocking: bool,
}

/// Where a fix writes the reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SecretScope {
    /// `secrets.toml`: every machine.
    Store,
    /// This machine's overlay (SPEC 5.6), e.g. "Use keyring on this machine".
    ThisMachine,
}

impl From<SecretScope> for SetScope {
    fn from(scope: SecretScope) -> Self {
        match scope {
            SecretScope::Store => SetScope::Store,
            SecretScope::ThisMachine => SetScope::Machine,
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OpEnvironmentNames {
    /// `None` below 1Password CLI 2.42.0: the user types the names.
    pub names: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MapResult {
    pub mapped: Vec<String>,
    /// Aliases that point elsewhere already; left as they are.
    pub kept: Vec<String>,
    /// The Environment this one replaced.
    pub replaced: Option<String>,
    pub change: ChangeResult,
}

fn machine_id(core: &Core) -> Result<Option<String>, CoreError> {
    Ok(Machine::load(core.fs.as_ref(), &core.home.state_dir())?.map(|machine| machine.id))
}

fn memory(store: Option<&Store<'_>>) -> Result<Arc<SecretMemory>, CoreError> {
    match store {
        Some(store) => Ok(store.secret_memory().clone()),
        None => SecretMemory::new().map(Arc::new),
    }
}

fn op_feature(feature: OpFeature) -> OpFeatureDto {
    match feature {
        OpFeature::ItemRead => OpFeatureDto::ItemRead,
        OpFeature::Environments => OpFeatureDto::Environments,
        OpFeature::EnvironmentListing => OpFeatureDto::EnvironmentListing,
    }
}

/// The providers of this machine (`loadout secrets providers`).
pub fn provider_list(core: &Core) -> Vec<ProviderEntry> {
    let keyring = core.delivery.keyring();
    providers::describe(keyring, core.delivery.op(&core.env, core.fs.as_ref()))
        .into_iter()
        .map(|info| ProviderEntry {
            kind: match info.kind {
                ProviderKind::Env => ProviderKindDto::Env,
                ProviderKind::Keyring => ProviderKindDto::Keyring,
                ProviderKind::OnePassword => ProviderKindDto::Onepassword,
            },
            state: match info.state {
                ProviderState::Available => ProviderStateDto::Available,
                ProviderState::Unavailable => ProviderStateDto::Unavailable,
                ProviderState::Error => ProviderStateDto::Error,
            },
            version: info.version,
            backend: (info.kind == ProviderKind::Keyring).then(|| keyring.name().to_owned()),
            features: info
                .features
                .iter()
                .map(|(feature, available)| ProviderFeature {
                    feature: op_feature(*feature),
                    available: *available,
                })
                .collect(),
        })
        .collect()
}

/// The alias list, unresolved.
pub fn overview(core: &Core) -> CmdResult<SecretsOverview> {
    let service_account_set = core
        .delivery
        .keyring()
        .get(brand::KEYRING_SERVICE, SERVICE_ACCOUNT_KEYRING_ACCOUNT)
        .is_ok_and(|value| value.is_some());
    let Some(store) = core.open_store()? else {
        return Ok(SecretsOverview {
            aliases: Vec::new(),
            undefined: Vec::new(),
            environment: None,
            op_account: None,
            service_account_set,
        });
    };
    let machine = machine_id(core)?;
    let catalog = Catalog::load(&store, machine.as_deref())?;
    let usage = catalog::usage(&store, machine.as_deref())?;
    Ok(SecretsOverview {
        aliases: catalog
            .iter()
            .map(|alias| AliasEntry {
                alias: alias.alias.clone(),
                reference: alias.reference.clone(),
                source: alias.source.into(),
                store_reference: alias.store_reference.clone(),
                description: alias.description.clone(),
                env_name: alias.env_name().to_owned(),
                servers: usage
                    .get(&alias.alias)
                    .map(|ids| ids.iter().cloned().collect())
                    .unwrap_or_default(),
            })
            .collect(),
        undefined: usage
            .keys()
            .filter(|alias| catalog.get(alias).is_none())
            .cloned()
            .collect(),
        environment: catalog.environment.clone(),
        op_account: catalog.op_account.clone(),
        service_account_set,
    })
}

/// The missing-secret checklist: every alias defined or used (or only
/// `only`), resolved, with the servers and clients that need it. May
/// prompt for 1Password approval.
pub fn check(core: &Core, only: &[String]) -> CmdResult<Vec<AliasCheck>> {
    let store = core.open_store()?;
    let machine = machine_id(core)?;
    let (catalog, usage, desired) = match &store {
        Some(store) => (
            Catalog::load(store, machine.as_deref())?,
            catalog::usage(store, machine.as_deref())?,
            DesiredState::load(store, machine.as_deref())?,
        ),
        None => (
            Catalog::default(),
            Default::default(),
            DesiredState::empty(),
        ),
    };
    let keyring = core.delivery.keyring();
    let env = op_env(&core.env, keyring, catalog.op_account.as_deref());
    let resolver = Resolver::new(
        &env,
        keyring,
        core.delivery.op(&core.env, core.fs.as_ref()),
        memory(store.as_ref())?,
    );
    Ok(catalog::check(&catalog, &usage, &resolver, only)
        .iter()
        .map(|status| {
            let needed_by: Vec<NeededBy> = catalog::needed_by(status, &desired)
                .into_iter()
                .map(|(server, clients)| NeededBy { server, clients })
                .collect();
            AliasCheck {
                alias: status.alias.clone(),
                reference: status.reference.clone(),
                source: status.source.map(Into::into),
                status: status.resolution.status.into(),
                message: status
                    .resolution
                    .message
                    .as_deref()
                    .map(|message| scrub(&core.env, message, None)),
                blocking: needed_by.iter().any(|n| !n.clients.is_empty()),
                needed_by,
            }
        })
        .collect())
}

/// Commits `update` for `alias` (one commit, applied at once in instant
/// mode).
fn commit_alias(
    core: &Core,
    store: &Store<'_>,
    alias: &str,
    update: &AliasUpdate,
    scope: SecretScope,
) -> CmdResult<ChangeResult> {
    let scope: SetScope = scope.into();
    let summary = match scope {
        SetScope::Store => format!("secrets: set {alias}"),
        SetScope::Machine => format!("secrets: set {alias} on this machine"),
    };
    let ax = core.apply_env();
    let committed = store.transaction(Operation::new("secrets.set", summary), |tx| {
        let entity = match scope {
            SetScope::Store => "secrets".to_owned(),
            SetScope::Machine => format!("machine/{}", tx.machine().id),
        };
        tx.add_entity(entity);
        catalog::stage_set(tx, alias, update, scope)?;
        instant_apply(tx, &ax)
    })?;
    Ok(ChangeResult::new(
        &core.env,
        committed.commit.as_ref(),
        committed.value.as_ref(),
    ))
}

/// Points `alias` at `reference` (`op://...`, `op-env://...`, `env:NAME`,
/// `keyring://...`).
pub fn set_reference(
    core: &Core,
    alias: &str,
    reference: &str,
    scope: SecretScope,
    env_name: Option<&str>,
) -> CmdResult<ChangeResult> {
    ids::check(IdKind::SecretAlias, alias)?;
    let update = AliasUpdate {
        reference: SecretRef::parse(reference.trim())?,
        description: None,
        env_name: env_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
    };
    let store = core.open_store_for_change()?;
    commit_alias(core, &store, alias, &update, scope)
}

/// Stores `value` in the OS keyring as `keyring://loadout/<ALIAS>` and
/// points the alias there. The value goes one way.
pub fn set_keyring_value(
    core: &Core,
    alias: &str,
    value: &str,
    scope: SecretScope,
) -> CmdResult<ChangeResult> {
    ids::check(IdKind::SecretAlias, alias)?;
    if value.is_empty() {
        return Err(CoreError::new(ErrorCode::InvalidValue, "the value is empty").into());
    }
    let reference = SecretRef::Keyring {
        service: brand::KEYRING_SERVICE.to_owned(),
        account: alias.to_owned(),
    };
    let store = core.open_store_for_change()?;
    let (service, account) = keyring_parts(&reference)?;
    // Through the resolver, so the store refuses to commit the value.
    let resolver = Resolver::new(
        &core.env,
        core.delivery.keyring(),
        &providers::onepassword::OpCli::NotFound,
        store.secret_memory().clone(),
    );
    resolver
        .set_keyring(service, account, &SecretValue::new(value))
        .map_err(|resolution| resolution.into_error(&format!("cannot store {alias}")))?;
    let update = AliasUpdate {
        reference,
        description: None,
        env_name: None,
    };
    commit_alias(core, &store, alias, &update, scope)
}

/// Deletes the keyring entry of an alias's keyring reference on this
/// machine; the reference stays. `false` when there was none.
pub fn clear_keyring_value(core: &Core, alias: &str) -> CmdResult<bool> {
    let store = core.open_store()?.ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotFound,
            format!("secret alias {alias:?} is not defined"),
        )
    })?;
    let catalog = Catalog::load(&store, machine_id(core)?.as_deref())?;
    let entry = catalog.get(alias).ok_or_else(|| {
        CoreError::new(
            ErrorCode::NotFound,
            format!("secret alias {alias:?} is not defined"),
        )
    })?;
    let reference = entry.parsed()?;
    let (service, account) = keyring_parts(&reference)?;
    Ok(core
        .delivery
        .keyring()
        .delete(service, account)
        .map_err(|resolution| resolution.into_error(&format!("cannot clear {alias}")))?)
}

/// Stores the 1Password service account token (SPEC 11.5); `loadout exec`
/// passes it to `op` only.
pub fn set_service_account(core: &Core, token: &str) -> CmdResult<()> {
    let token = token.trim();
    if token.is_empty() {
        return Err(CoreError::new(ErrorCode::InvalidValue, "the token is empty").into());
    }
    core.delivery
        .keyring()
        .set(
            brand::KEYRING_SERVICE,
            SERVICE_ACCOUNT_KEYRING_ACCOUNT,
            &SecretValue::new(token),
        )
        .map_err(|resolution| resolution.into_error("cannot store the token"))?;
    Ok(())
}

pub fn clear_service_account(core: &Core) -> CmdResult<bool> {
    Ok(core
        .delivery
        .keyring()
        .delete(brand::KEYRING_SERVICE, SERVICE_ACCOUNT_KEYRING_ACCOUNT)
        .map_err(|resolution| resolution.into_error("cannot clear the token"))?)
}

fn check_environment(id: &str) -> Result<(), CoreError> {
    if is_op_environment_id(id) {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("invalid 1Password Environment id {id:?}"),
        ))
    }
}

fn stored_account(core: &Core) -> Result<Option<String>, CoreError> {
    Ok(core
        .open_store()?
        .map(|store| Catalog::load(&store, None))
        .transpose()?
        .and_then(|catalog| catalog.op_account))
}

/// The variable names of a 1Password Environment (values are dropped while
/// parsing); `names: None` below CLI 2.42.0.
pub fn environment_names(core: &Core, id: &str) -> CmdResult<OpEnvironmentNames> {
    check_environment(id)?;
    let op = core.delivery.op(&core.env, core.fs.as_ref());
    op.require(OpFeature::Environments)
        .map_err(|resolution| resolution.into_error("1Password Environments"))?;
    if !op.supports(OpFeature::EnvironmentListing) {
        return Ok(OpEnvironmentNames { names: None });
    }
    let account = stored_account(core)?;
    let env = op_env(&core.env, core.delivery.keyring(), account.as_deref());
    let names = op
        .list_environment(&env, id)
        .map_err(|resolution| resolution.into_error(&format!("Environment {id}")))?;
    Ok(OpEnvironmentNames { names: Some(names) })
}

/// "Map a 1Password Environment" (SPEC 11.10): saves the Environment id
/// and maps `names` to aliases `op-env://<id>/<NAME>`, one commit.
pub fn map_environment(
    core: &Core,
    id: &str,
    names: &[String],
    account: Option<&str>,
) -> CmdResult<MapResult> {
    check_environment(id)?;
    let account = account.map(str::trim).filter(|a| !a.is_empty());
    if let Some(account) = account
        && !is_op_account(account)
    {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("invalid 1Password account {account:?}"),
        )
        .into());
    }
    let names: BTreeSet<&str> = names
        .iter()
        .map(|n| n.trim())
        .filter(|n| !n.is_empty())
        .collect();
    if names.is_empty() {
        return Err(CoreError::new(ErrorCode::InvalidValue, "pick at least one name").into());
    }
    for name in &names {
        ids::check(IdKind::SecretAlias, name)?;
    }
    let store = core.open_store_for_change()?;
    let catalog = Catalog::load(&store, machine_id(core)?.as_deref())?;
    let mut mapped = Vec::new();
    let mut kept = Vec::new();
    for name in names {
        let reference = SecretRef::OnePasswordEnvironment {
            environment: id.to_owned(),
            name: name.to_owned(),
        };
        match catalog.get(name).map(|alias| alias.parsed()) {
            Some(Ok(SecretRef::OnePasswordEnvironment { .. })) | None => {
                mapped.push((name.to_owned(), reference));
            }
            Some(_) => kept.push(name.to_owned()),
        }
    }
    let ax = core.apply_env();
    let summary = format!("secrets: map 1Password Environment {id}");
    let committed = store.transaction(Operation::new("secrets.map-op-env", summary), |tx| {
        tx.add_entity("secrets".to_owned());
        let previous = catalog::stage_environment(tx, id)?;
        if let Some(account) = account {
            catalog::stage_op_account(tx, Some(account))?;
        }
        for (alias, reference) in &mapped {
            let update = AliasUpdate {
                reference: reference.clone(),
                description: None,
                env_name: None,
            };
            catalog::stage_set(tx, alias, &update, SetScope::Store)?;
        }
        Ok((previous, instant_apply(tx, &ax)?))
    })?;
    let (previous, report) = committed.value;
    Ok(MapResult {
        mapped: mapped.into_iter().map(|(alias, _)| alias).collect(),
        kept,
        replaced: previous,
        change: ChangeResult::new(&core.env, committed.commit.as_ref(), report.as_ref()),
    })
}

/// The 1Password account every `op` call names, or `None` to clear it.
pub fn op_account(core: &Core, account: Option<&str>) -> CmdResult<ChangeResult> {
    let account = account.map(str::trim).filter(|a| !a.is_empty());
    if let Some(account) = account
        && !is_op_account(account)
    {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("invalid 1Password account {account:?}"),
        )
        .into());
    }
    let store = core.open_store_for_change()?;
    let summary = match account {
        Some(account) => format!("secrets: 1Password account {account}"),
        None => "secrets: clear the 1Password account".to_owned(),
    };
    let ax = core.apply_env();
    let committed = store.transaction(Operation::new("secrets.op-account", summary), |tx| {
        tx.add_entity("secrets".to_owned());
        catalog::stage_op_account(tx, account)?;
        instant_apply(tx, &ax)
    })?;
    Ok(ChangeResult::new(
        &core.env,
        committed.commit.as_ref(),
        committed.value.as_ref(),
    ))
}

#[tauri::command]
#[specta::specta]
pub async fn secret_providers(state: tauri::State<'_, AppState>) -> CmdResult<Vec<ProviderEntry>> {
    blocking(&state, |core| Ok(provider_list(core))).await
}

#[tauri::command]
#[specta::specta]
pub async fn secrets_overview(state: tauri::State<'_, AppState>) -> CmdResult<SecretsOverview> {
    blocking(&state, overview).await
}

#[tauri::command]
#[specta::specta]
pub async fn check_secrets(
    state: tauri::State<'_, AppState>,
    aliases: Vec<String>,
) -> CmdResult<Vec<AliasCheck>> {
    blocking(&state, move |core| check(core, &aliases)).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_secret_reference(
    state: tauri::State<'_, AppState>,
    alias: String,
    reference: String,
    scope: SecretScope,
    env_name: Option<String>,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| {
        set_reference(core, &alias, &reference, scope, env_name.as_deref())
    })
    .await
}

/// The one-way "set keyring value" input (SPEC 11.1 item 1).
#[tauri::command]
#[specta::specta]
pub async fn set_secret_keyring_value(
    state: tauri::State<'_, AppState>,
    alias: String,
    value: String,
    scope: SecretScope,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| {
        set_keyring_value(core, &alias, &value, scope)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn clear_secret_keyring_value(
    state: tauri::State<'_, AppState>,
    alias: String,
) -> CmdResult<bool> {
    blocking(&state, move |core| clear_keyring_value(core, &alias)).await
}

#[tauri::command]
#[specta::specta]
pub async fn set_op_service_account(
    state: tauri::State<'_, AppState>,
    token: String,
) -> CmdResult<()> {
    blocking(&state, move |core| set_service_account(core, &token)).await
}

#[tauri::command]
#[specta::specta]
pub async fn clear_op_service_account(state: tauri::State<'_, AppState>) -> CmdResult<bool> {
    blocking(&state, clear_service_account).await
}

#[tauri::command]
#[specta::specta]
pub async fn list_op_environment(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CmdResult<OpEnvironmentNames> {
    blocking(&state, move |core| environment_names(core, &id)).await
}

#[tauri::command]
#[specta::specta]
pub async fn map_op_environment(
    state: tauri::State<'_, AppState>,
    id: String,
    names: Vec<String>,
    account: Option<String>,
) -> CmdResult<MapResult> {
    blocking(&state, move |core| {
        map_environment(core, &id, &names, account.as_deref())
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn set_op_account(
    state: tauri::State<'_, AppState>,
    account: Option<String>,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| op_account(core, account.as_deref())).await
}

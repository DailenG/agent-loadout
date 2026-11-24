//! Source trust commands (#24, SPEC 17): what the trust dialog shows about
//! a git source, and Settings > Sources (list, revoke, block). The dialog's
//! answer goes back with the install or update command it was asked for
//! (`trust: TrustGrantArg | null`); nothing here trusts a source on its
//! own. Records are addressed by an opaque id, never by a path; a local
//! source is shown under its placeholder root.

use agent_loadout_core::Env;
use agent_loadout_core::apply::placeholder_path;
use agent_loadout_core::sources::{
    self, AskReason, Lapse, LapseReason, SourceChangeKind, SourceDecision, SourceRecord, SourceRef,
    TrustCheck, TrustGrant, TrustState, TrustVia,
};
use agent_loadout_core::store::{CommitId, Store};
use serde::{Deserialize, Serialize};
use specta::Type;

use super::{AppState, CmdResult, Core, IpcError, blocking, scrubbed};

fn commit_text(commit: Option<&CommitId>) -> Option<String> {
    commit.map(CommitId::to_string)
}

/// What a trust decision covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SourceScope {
    Repo,
    Owner,
    Host,
}

impl From<sources::SourceScope> for SourceScope {
    fn from(scope: sources::SourceScope) -> Self {
        match scope {
            sources::SourceScope::Repo => SourceScope::Repo,
            sources::SourceScope::Owner => SourceScope::Owner,
            sources::SourceScope::Host => SourceScope::Host,
        }
    }
}

impl From<SourceScope> for sources::SourceScope {
    fn from(scope: SourceScope) -> Self {
        match scope {
            SourceScope::Repo => sources::SourceScope::Repo,
            SourceScope::Owner => sources::SourceScope::Owner,
            SourceScope::Host => sources::SourceScope::Host,
        }
    }
}

/// The trust dialog's answer: "Trust this time only" or "Always trust
/// this source" at a scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrustGrantArg {
    Once,
    Always { scope: SourceScope },
}

impl TrustGrantArg {
    pub fn grant(self) -> TrustGrant {
        match self {
            TrustGrantArg::Once => TrustGrant::Once,
            TrustGrantArg::Always { scope } => TrustGrant::Always(scope.into()),
        }
    }
}

/// The core's decisions for an install or update from the app: the
/// dialog's answer, recorded as given in the app.
pub fn grant_of(trust: Option<TrustGrantArg>) -> (Option<TrustGrant>, TrustVia) {
    (trust.map(TrustGrantArg::grant), TrustVia::App)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SourceLapseReason {
    HistoryRewritten,
    Moved,
}

impl From<LapseReason> for SourceLapseReason {
    fn from(reason: LapseReason) -> Self {
        match reason {
            LapseReason::HistoryRewritten => SourceLapseReason::HistoryRewritten,
            LapseReason::Moved => SourceLapseReason::Moved,
        }
    }
}

/// Why trust lapsed, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SourceLapse {
    pub reason: SourceLapseReason,
    pub at: String,
    /// `moved`: where the repository moved to.
    pub detail: Option<String>,
}

fn lapse_of(lapse: &Lapse) -> SourceLapse {
    SourceLapse {
        reason: lapse.reason.into(),
        at: lapse.at.to_string(),
        detail: lapse.detail.clone(),
    }
}

/// What changed about a source since its last fetch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SourceChange {
    pub kind: SourceLapseReason,
    pub detail: Option<String>,
}

pub fn change_of(change: &sources::SourceChange) -> SourceChange {
    SourceChange {
        kind: match change.kind {
            SourceChangeKind::HistoryRewritten => SourceLapseReason::HistoryRewritten,
            SourceChangeKind::Moved => SourceLapseReason::Moved,
        },
        detail: change.detail.clone(),
    }
}

/// One scope "always trust" offers, with the key it would record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SourceScopeOption {
    pub scope: SourceScope,
    pub display: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SourceAskReason {
    FirstTime,
    NotTrusted,
    Lapsed,
}

impl From<AskReason> for SourceAskReason {
    fn from(reason: AskReason) -> Self {
        match reason {
            AskReason::FirstTime => SourceAskReason::FirstTime,
            AskReason::NotTrusted => SourceAskReason::NotTrusted,
            AskReason::Lapsed => SourceAskReason::Lapsed,
        }
    }
}

/// Where the store's decisions leave a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceTrustState {
    Trusted {
        scope: SourceScope,
        display: String,
        #[serde(rename = "decidedAt")]
        decided_at: String,
    },
    Blocked {
        scope: SourceScope,
        display: String,
        #[serde(rename = "decidedAt")]
        decided_at: String,
    },
    Ask {
        reason: SourceAskReason,
        lapse: Option<SourceLapse>,
    },
}

/// What the trust dialog shows about a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SourceTrustInfo {
    /// The repository key; a local source under its placeholder root.
    pub display: String,
    pub official: bool,
    /// Narrowest first; `repo` is the default.
    pub scopes: Vec<SourceScopeOption>,
    pub state: SourceTrustState,
    pub installed_skills: Vec<String>,
    /// Set by update checks and install reviews.
    pub change: Option<SourceChange>,
    /// Fetched over plain `http://` or `git://`: unencrypted, so anyone on
    /// the network path can change what arrives.
    pub insecure: bool,
}

/// A key as the app shows it: a local source's path under its placeholder
/// root, anything else as it is.
fn display_key(env: &Env, key: &str) -> String {
    match SourceRef::from_git_url(key)
        .ok()
        .and_then(|source| source.local_path())
    {
        Some(path) => placeholder_path(env, &path, None),
        None => key.to_owned(),
    }
}

pub fn trust_info(
    env: &Env,
    check: &TrustCheck,
    change: Option<&sources::SourceChange>,
) -> SourceTrustInfo {
    let source = &check.source;
    let state = match &check.state {
        TrustState::Trusted { key, record } => SourceTrustState::Trusted {
            scope: record.scope.into(),
            display: display_key(env, key),
            decided_at: record.decided_at.to_string(),
        },
        TrustState::Blocked { key, record } => SourceTrustState::Blocked {
            scope: record.scope.into(),
            display: display_key(env, key),
            decided_at: record.decided_at.to_string(),
        },
        TrustState::Ask { reason, lapse } => SourceTrustState::Ask {
            reason: (*reason).into(),
            lapse: lapse.as_ref().map(lapse_of),
        },
    };
    SourceTrustInfo {
        display: display_key(env, source.key()),
        official: check.official,
        scopes: check
            .scopes
            .iter()
            .filter_map(|scope| {
                Some(SourceScopeOption {
                    scope: (*scope).into(),
                    display: display_key(env, &source.scope_key(*scope)?),
                })
            })
            .collect(),
        state,
        installed_skills: check.installed_skills.clone(),
        change: change.map(change_of),
        insecure: source.is_insecure(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SourceDecisionKind {
    Trust,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "kebab-case")]
pub enum SourceDecidedVia {
    App,
    CliPrompt,
    CliFlag,
    CliCommand,
}

impl From<TrustVia> for SourceDecidedVia {
    fn from(via: TrustVia) -> Self {
        match via {
            TrustVia::App => SourceDecidedVia::App,
            TrustVia::CliPrompt => SourceDecidedVia::CliPrompt,
            TrustVia::CliFlag => SourceDecidedVia::CliFlag,
            TrustVia::CliCommand => SourceDecidedVia::CliCommand,
        }
    }
}

/// One row of Settings > Sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SourceRecordInfo {
    /// Opaque handle for revoke and block; never a path.
    pub id: String,
    pub display: String,
    pub scope: SourceScope,
    pub decision: SourceDecisionKind,
    pub decided_at: String,
    pub via: Option<SourceDecidedVia>,
    pub machine: Option<String>,
    pub lapse: Option<SourceLapse>,
    pub official: bool,
}

fn record_info(env: &Env, key: &str, record: &SourceRecord) -> SourceRecordInfo {
    let official = sources::OFFICIAL_OWNERS
        .iter()
        .any(|owner| key == format!("{owner}/*") || key.starts_with(&format!("{owner}/")));
    SourceRecordInfo {
        id: sources::record_id(key),
        display: display_key(env, key),
        scope: record.scope.into(),
        decision: match record.decision {
            SourceDecision::Trust => SourceDecisionKind::Trust,
            SourceDecision::Block => SourceDecisionKind::Block,
        },
        decided_at: record.decided_at.to_string(),
        via: record.via.map(Into::into),
        machine: record.machine.clone(),
        lapse: record.lapsed.as_ref().map(lapse_of),
        official,
    }
}

/// Every trusted or blocked source, by key.
pub fn list(core: &Core) -> CmdResult<Vec<SourceRecordInfo>> {
    let env = &core.env;
    let Some(store) = core.open_store()? else {
        return Ok(Vec::new());
    };
    let records = sources::list(&store).map_err(|err| scrubbed(env, &err, None))?;
    Ok(records
        .iter()
        .map(|(key, record)| record_info(env, key, record))
        .collect())
}

fn source_of(core: &Core, url: &str) -> CmdResult<SourceRef> {
    SourceRef::from_git_url(url).map_err(|err| scrubbed(&core.env, &err, None))
}

/// What the trust dialog shows about the source of `url`. Never contacts
/// the source.
pub fn check(core: &Core, url: &str) -> CmdResult<SourceTrustInfo> {
    let source = source_of(core, url)?;
    check_source(core, &source, None)
}

/// What the trust dialog shows about `source`, with what changed about it
/// (an install review through a redirect, an update check). Without a
/// store yet, a first-time source.
pub fn check_source(
    core: &Core,
    source: &SourceRef,
    change: Option<&sources::SourceChange>,
) -> CmdResult<SourceTrustInfo> {
    let env = &core.env;
    let check = match core.open_store()? {
        Some(store) => sources::check(&store, source).map_err(|err| scrubbed(env, &err, None))?,
        None => TrustCheck {
            state: TrustState::Ask {
                reason: AskReason::FirstTime,
                lapse: None,
            },
            scopes: source.scopes(),
            official: source.is_official(),
            installed_skills: Vec::new(),
            source: source.clone(),
        },
    };
    Ok(trust_info(env, &check, change))
}

/// Refuses a blocked `source` (`source_blocked`) before a review fetches
/// it: a blocked source is never contacted. An undecided one goes on (the
/// review shows the dialog).
pub fn refuse_blocked(core: &Core, source: &SourceRef) -> CmdResult<()> {
    if let Some(store) = core.open_store()? {
        sources::refuse_blocked(&store, source).map_err(|err| scrubbed(&core.env, &err, None))?;
    }
    Ok(())
}

fn require_store(core: &Core) -> CmdResult<Store<'_>> {
    core.open_store()?.ok_or_else(|| IpcError {
        code: agent_loadout_core::ErrorCode::NotFound,
        message: "there is no source decision yet".to_owned(),
    })
}

/// Removes the trust or block `id` (Settings > Sources "Revoke"). Nothing
/// is uninstalled; the next install or update from it asks again.
pub fn revoke(core: &Core, id: &str) -> CmdResult<Option<String>> {
    let env = &core.env;
    let store = require_store(core)?;
    let scrub = |err| scrubbed(env, &err, None);
    let key = sources::key_of_id(&store, id).map_err(scrub)?;
    let commit = sources::untrust(&store, &key).map_err(scrub)?;
    Ok(commit_text(commit.as_ref()))
}

/// Turns the listed record `id` into a block at the same scope.
pub fn block(core: &Core, id: &str) -> CmdResult<Option<String>> {
    let env = &core.env;
    let store = require_store(core)?;
    let scrub = |err| scrubbed(env, &err, None);
    let key = sources::key_of_id(&store, id).map_err(scrub)?;
    let commit = sources::block_key(&store, &key, TrustVia::App).map_err(scrub)?;
    Ok(commit_text(commit.as_ref()))
}

/// Blocks the source of `url` at `scope`, with or without a record.
pub fn block_url(core: &Core, url: &str, scope: SourceScope) -> CmdResult<Option<String>> {
    let env = &core.env;
    let source = source_of(core, url)?;
    let store = core.open_store_for_change()?;
    let commit = sources::block(&store, &source, scope.into(), TrustVia::App)
        .map_err(|err| scrubbed(env, &err, None))?;
    Ok(commit_text(commit.as_ref()))
}

#[tauri::command]
#[specta::specta]
pub async fn list_sources(state: tauri::State<'_, AppState>) -> CmdResult<Vec<SourceRecordInfo>> {
    blocking(&state, list).await
}

#[tauri::command]
#[specta::specta]
pub async fn check_source_trust(
    state: tauri::State<'_, AppState>,
    url: String,
) -> CmdResult<SourceTrustInfo> {
    blocking(&state, move |core| check(core, &url)).await
}

#[tauri::command]
#[specta::specta]
pub async fn revoke_source(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CmdResult<Option<String>> {
    blocking(&state, move |core| revoke(core, &id)).await
}

#[tauri::command]
#[specta::specta]
pub async fn block_source(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CmdResult<Option<String>> {
    blocking(&state, move |core| block(core, &id)).await
}

#[tauri::command]
#[specta::specta]
pub async fn block_source_url(
    state: tauri::State<'_, AppState>,
    url: String,
    scope: SourceScope,
) -> CmdResult<Option<String>> {
    blocking(&state, move |core| block_url(core, &url, scope)).await
}

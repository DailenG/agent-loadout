//! Source trust (#24, SPEC 17): installing or updating from a source the
//! user has not trusted asks first, a block wins over any trust, and a
//! repository that was force-pushed, renamed or transferred makes its trust
//! lapse so the user is asked again.
//!
//! Decisions live in `store/sources.toml` ([`SourcesFile`]), keyed by
//! source key ([`SourceRef`]): one repository (`github.com/acme/skills`),
//! one owner (`github.com/acme/*`) or one host (`git.acme.corp/*`). They
//! sync with remotes like other store files.
//!
//! Only a person decides: the app's dialog, an interactive CLI prompt, or
//! the explicit `--trust-source` flag ([`TrustVia`]). [`admit`] is the gate
//! every install path calls before anything from a source is saved.

pub mod detect;
mod key;

use std::path::Path;

use crate::error::{CoreError, ErrorCode};
use crate::model::TomlDocument;
use crate::model::skill_meta::{SkillMeta, SkillSourceKind};
pub use crate::model::sources::{
    Lapse, LapseReason, SourceDecision, SourceRecord, SourceScope, SourcesFile, TrustVia,
    key_shape, without_insecure_prefix,
};
use crate::skills::store::META_DIR;
use crate::store::{CommitId, Operation, Store, StoreRead, Transaction, utc_datetime};
pub use detect::{SourceChange, SourceChangeKind};
pub use key::{AGGREGATOR_HOSTS, OFFICIAL_OWNERS, SourceRef};

/// `store/sources.toml`.
pub const SOURCES_FILE: &str = "sources.toml";
/// The `Loadout-Entity` of source trust commits.
pub const ENTITY: &str = "sources";

/// The user's answer to the trust prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrustGrant {
    /// Install or update this once; the next time asks again.
    Once,
    /// Record trust at this scope, then go ahead.
    Always(SourceScope),
}

/// Why the prompt is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AskReason {
    /// No decision about the source, and nothing installed from it.
    FirstTime,
    /// No decision, but skills from it are installed (trusted once before,
    /// trust revoked, or installed before 0.5.0).
    NotTrusted,
    /// Trusted, but the repository changed under that trust.
    Lapsed,
}

impl AskReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            AskReason::FirstTime => "first_time",
            AskReason::NotTrusted => "not_trusted",
            AskReason::Lapsed => "lapsed",
        }
    }
}

/// Where the store's decisions leave a source.
#[derive(Debug, Clone, PartialEq)]
pub enum TrustState {
    /// The record at `key` trusts it.
    Trusted { key: String, record: SourceRecord },
    /// The record at `key` blocks it.
    Blocked { key: String, record: SourceRecord },
    /// The user must decide.
    Ask {
        reason: AskReason,
        lapse: Option<Lapse>,
    },
}

/// What the trust prompt shows about a source.
#[derive(Debug, Clone, PartialEq)]
pub struct TrustCheck {
    pub source: SourceRef,
    pub state: TrustState,
    /// The scopes "always trust" offers, narrowest first.
    pub scopes: Vec<SourceScope>,
    pub official: bool,
    /// Store skills whose git source has this repository key.
    pub installed_skills: Vec<String>,
}

/// What [`admit`] did.
#[derive(Debug, Clone)]
pub struct Admission {
    pub check: TrustCheck,
    /// The commit recording "always trust", if one was made.
    pub recorded: Option<CommitId>,
}

/// The decisions in the store; empty when there is no `sources.toml`.
pub fn load(read: &dyn StoreRead) -> Result<SourcesFile, CoreError> {
    Ok(load_doc(read)?.into_value())
}

fn load_doc(read: &dyn StoreRead) -> Result<TomlDocument<SourcesFile>, CoreError> {
    let Some(text) = read.read_file(SOURCES_FILE)? else {
        return Ok(TomlDocument::new(SourcesFile::new()));
    };
    let path = Path::new(SOURCES_FILE);
    let doc = TomlDocument::<SourcesFile>::parse(&text)
        .map_err(|diagnostic| diagnostic.to_core_error(Some(path)))?;
    if let Some(err) = doc.value().validate().first_error(Some(path)) {
        return Err(err);
    }
    Ok(doc)
}

/// Every decision, sorted by key.
pub fn list(read: &dyn StoreRead) -> Result<Vec<(String, SourceRecord)>, CoreError> {
    Ok(load(read)?.sources.into_iter().collect())
}

/// An opaque handle of the record at `key` (16 hex digits of its SHA-256),
/// for frontends that must not carry a local path.
pub fn record_id(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(key.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The key of the record whose [`record_id`] is `id`.
pub fn key_of_id(read: &dyn StoreRead, id: &str) -> Result<String, CoreError> {
    load(read)?
        .sources
        .into_keys()
        .find(|key| record_id(key) == id)
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::NotFound,
                "there is no such source decision any more; reload the list",
            )
        })
}

/// Where the store's decisions leave `source`. Read-only; never contacts
/// the source.
pub fn check(read: &dyn StoreRead, source: &SourceRef) -> Result<TrustCheck, CoreError> {
    let file = load(read)?;
    let state = state_of(&file, source);
    let installed_skills = installed_skills(read, source)?;
    let state = match state {
        TrustState::Ask { lapse: None, .. } if !installed_skills.is_empty() => TrustState::Ask {
            reason: AskReason::NotTrusted,
            lapse: None,
        },
        state => state,
    };
    Ok(TrustCheck {
        source: source.clone(),
        state,
        scopes: source.scopes(),
        official: source.is_official(),
        installed_skills,
    })
}

/// Precedence: any matching block wins (a block of `host/...` also covers
/// `http://host/...`); otherwise the most specific matching record decides
/// (a lapsed trust asks); no record asks.
fn state_of(file: &SourcesFile, source: &SourceRef) -> TrustState {
    let decided = |keys: Vec<String>| -> Vec<(String, &SourceRecord)> {
        keys.into_iter()
            .filter_map(|key| file.sources.get(&key).map(|record| (key, record)))
            .collect()
    };
    if let Some((key, record)) = decided(source.block_keys())
        .into_iter()
        .find(|(_, record)| record.decision == SourceDecision::Block)
    {
        return TrustState::Blocked {
            key,
            record: record.clone(),
        };
    }
    let matching = decided(source.matching_keys());
    match matching.first() {
        Some((_, record)) if record.lapsed.is_some() => TrustState::Ask {
            reason: AskReason::Lapsed,
            lapse: record.lapsed.clone(),
        },
        Some((key, record)) => TrustState::Trusted {
            key: key.clone(),
            record: (*record).clone(),
        },
        None => TrustState::Ask {
            reason: AskReason::FirstTime,
            lapse: None,
        },
    }
}

/// Store skills installed from the repository `source` names.
fn installed_skills(read: &dyn StoreRead, source: &SourceRef) -> Result<Vec<String>, CoreError> {
    let mut ids = Vec::new();
    for name in read.list_files(META_DIR)? {
        let Some(id) = name.strip_suffix(".toml") else {
            continue;
        };
        let Some(text) = read.read_file(&format!("{META_DIR}/{name}"))? else {
            continue;
        };
        // A broken metadata file is reported where skills are listed; here
        // it just does not count.
        let Ok(doc) = TomlDocument::<SkillMeta>::parse(&text) else {
            continue;
        };
        let from_source = doc
            .value()
            .source
            .as_ref()
            .filter(|skill_source| skill_source.kind == SkillSourceKind::Git)
            .and_then(|skill_source| skill_source.url.as_deref())
            .and_then(|url| SourceRef::from_git_url(url).ok())
            .is_some_and(|other| other.key() == source.key());
        if from_source {
            ids.push(id.to_owned());
        }
    }
    Ok(ids)
}

fn blocked_error(source: &SourceRef, key: &str) -> CoreError {
    CoreError::new(
        ErrorCode::SourceBlocked,
        format!(
            "the source {} is blocked ({key}); remove the block with `loadout source untrust {key}` \
             or in Settings > Sources",
            source.key()
        ),
    )
}

fn trust_required(source: &SourceRef, reason: AskReason, lapse: Option<&Lapse>) -> CoreError {
    let why = match (reason, lapse) {
        (AskReason::Lapsed, Some(lapse)) => match lapse.reason {
            LapseReason::HistoryRewritten => {
                "its history was rewritten (force-push) since you trusted it".to_owned()
            }
            LapseReason::Moved => format!(
                "it moved to {} since you trusted it",
                lapse.detail.as_deref().unwrap_or("another location")
            ),
        },
        (AskReason::Lapsed, None) => "it changed since you trusted it".to_owned(),
        (AskReason::FirstTime, _) => "it is the first time you install from it".to_owned(),
        (AskReason::NotTrusted, _) => "you have not trusted it".to_owned(),
    };
    CoreError::new(
        ErrorCode::SourceTrustRequired,
        format!(
            "the source {} needs your decision: {why}. Skills and MCP servers can run commands and \
             read your files; only install from sources you trust",
            source.key()
        ),
    )
}

/// The trust gate. Trusted: Ok. Blocked: `source_blocked`. Undecided and
/// no `grant`: `source_trust_required`. [`TrustGrant::Once`] goes ahead
/// without recording anything; [`TrustGrant::Always`] records trust at
/// that scope (one commit) first. A grant never overrides a block.
pub fn admit(
    store: &Store<'_>,
    source: &SourceRef,
    grant: Option<TrustGrant>,
    via: TrustVia,
) -> Result<Admission, CoreError> {
    let check = would_admit(store, source, grant)?;
    let recorded = match (&check.state, grant) {
        (TrustState::Ask { .. }, Some(TrustGrant::Always(scope))) => {
            trust(store, source, scope, via)?
        }
        _ => None,
    };
    Ok(Admission { check, recorded })
}

/// [`admit`] without recording anything: the same errors, so a caller can
/// refuse before fetching a source it would refuse after.
pub fn would_admit(
    read: &dyn StoreRead,
    source: &SourceRef,
    grant: Option<TrustGrant>,
) -> Result<TrustCheck, CoreError> {
    let check = check(read, source)?;
    match (&check.state, grant) {
        (TrustState::Blocked { key, .. }, _) => Err(blocked_error(source, key)),
        (TrustState::Ask { reason, lapse }, None) => {
            Err(trust_required(source, *reason, lapse.as_ref()))
        }
        (TrustState::Ask { .. } | TrustState::Trusted { .. }, _) => Ok(check),
    }
}

/// Refuses a blocked `source` (`source_blocked`) before anything contacts
/// it: an install review fetches only what no block covers. An undecided
/// source goes through (the review asks).
pub fn refuse_blocked(read: &dyn StoreRead, source: &SourceRef) -> Result<TrustCheck, CoreError> {
    let check = check(read, source)?;
    match &check.state {
        TrustState::Blocked { key, .. } => Err(blocked_error(source, key)),
        _ => Ok(check),
    }
}

fn scope_key(source: &SourceRef, scope: SourceScope) -> Result<String, CoreError> {
    source.scope_key(scope).ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidValue,
            format!(
                "the source {} has no {} scope",
                source.key(),
                scope.as_str()
            ),
        )
    })
}

/// Runs one change of `sources.toml`; `None` when nothing changed.
fn change(
    store: &Store<'_>,
    op: &str,
    summary: String,
    edit: impl FnOnce(&mut Transaction<'_>, &mut SourcesFile) -> Result<bool, CoreError>,
) -> Result<Option<CommitId>, CoreError> {
    let operation = Operation::new(op, summary).entity(ENTITY);
    let committed = store.transaction(operation, |tx| {
        let mut doc = load_doc(tx)?;
        if !edit(tx, doc.value_mut())? {
            return Ok(());
        }
        let text = doc.to_text()?;
        tx.write(SOURCES_FILE, text)
    })?;
    Ok(committed.commit)
}

fn new_record(
    tx: &Transaction<'_>,
    decision: SourceDecision,
    scope: SourceScope,
    via: TrustVia,
    url: Option<String>,
) -> SourceRecord {
    SourceRecord {
        decision,
        scope,
        decided_at: utc_datetime(tx.store().env().now()),
        via: Some(via),
        machine: Some(tx.machine().label.clone()),
        url,
        lapsed: None,
        extra: toml::Table::new(),
    }
}

/// Whether a record at `key` covers the repository or owner key `other`
/// (itself, or a key below an owner or host wildcard).
fn covers(key: &str, other: &str) -> bool {
    match key.strip_suffix('*') {
        Some(prefix) => other.starts_with(prefix),
        None => key == other,
    }
}

/// Records `decision` at the source key `key` (one commit): a repository
/// (`host/path`, `file:///path`), an owner (`host/owner/*`) or a host
/// (`host/*`). Trust is refused for an [`AGGREGATOR_HOSTS`] host, and
/// clears every lapse the new trust covers: the user decided again.
/// `url` is shown with the record (repositories).
pub fn record_key(
    store: &Store<'_>,
    key: &str,
    decision: SourceDecision,
    via: TrustVia,
    url: Option<&str>,
) -> Result<Option<CommitId>, CoreError> {
    let scope = match SourceRef::canonical_key(key) {
        Ok((canonical, scope)) if canonical == key => scope,
        Ok((canonical, _)) => {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!("{key:?} is not a canonical source key; it is written {canonical:?}"),
            ));
        }
        Err(_) => {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!(
                    "{key:?} is not a source key: host/path for a repository, host/owner/* for \
                     an owner, host/* for a host"
                ),
            ));
        }
    };
    if decision == SourceDecision::Trust
        && scope == SourceScope::Host
        && AGGREGATOR_HOSTS.contains(&without_insecure_prefix(key).trim_end_matches("/*"))
    {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!(
                "{key} cannot be trusted as a whole: that host serves anyone's repositories; \
                 trust an owner or a repository"
            ),
        ));
    }
    let (op, summary) = match decision {
        SourceDecision::Trust => (
            "source.trust",
            format!("source: trust {key} ({})", scope.as_str()),
        ),
        SourceDecision::Block => ("source.block", format!("source: block {key}")),
    };
    change(store, op, summary, |tx, file| {
        if decision == SourceDecision::Trust {
            for (other, record) in &mut file.sources {
                if record.decision == SourceDecision::Trust && covers(key, other) {
                    record.lapsed = None;
                }
            }
        }
        let record = new_record(tx, decision, scope, via, url.map(str::to_owned));
        file.sources.insert(key.to_owned(), record);
        Ok(true)
    })
}

/// Records trust in `source` at `scope`, one of [`SourceRef::scopes`]
/// (one commit; see [`record_key`]).
pub fn trust(
    store: &Store<'_>,
    source: &SourceRef,
    scope: SourceScope,
    via: TrustVia,
) -> Result<Option<CommitId>, CoreError> {
    if !source.scopes().contains(&scope) {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!(
                "{} cannot be trusted at the {} scope{}",
                source.key(),
                scope.as_str(),
                if scope == SourceScope::Host {
                    ": that host serves anyone's repositories; trust an owner or a repository"
                } else {
                    ""
                }
            ),
        ));
    }
    let key = scope_key(source, scope)?;
    let url = (scope == SourceScope::Repo).then(|| source.url());
    record_key(store, &key, SourceDecision::Trust, via, url)
}

/// Records a block of `source` at `scope` (one commit).
pub fn block(
    store: &Store<'_>,
    source: &SourceRef,
    scope: SourceScope,
    via: TrustVia,
) -> Result<Option<CommitId>, CoreError> {
    let key = scope_key(source, scope)?;
    let url = (scope == SourceScope::Repo).then(|| source.url());
    record_key(store, &key, SourceDecision::Block, via, url)
}

fn missing_key(key: &str) -> CoreError {
    CoreError::new(
        ErrorCode::NotFound,
        format!("there is no decision about the source {key:?}"),
    )
}

/// Turns the record at `key` into a block at the same scope.
pub fn block_key(
    store: &Store<'_>,
    key: &str,
    via: TrustVia,
) -> Result<Option<CommitId>, CoreError> {
    change(
        store,
        "source.block",
        format!("source: block {key}"),
        |tx, file| {
            let Some(old) = file.sources.get(key) else {
                return Err(missing_key(key));
            };
            if old.decision == SourceDecision::Block {
                return Ok(false);
            }
            let mut record = new_record(tx, SourceDecision::Block, old.scope, via, old.url.clone());
            record.extra = old.extra.clone();
            file.sources.insert(key.to_owned(), record);
            Ok(true)
        },
    )
}

/// Removes the trust or block at `key`. Nothing installed is touched; the
/// next install or update from that source asks again.
pub fn untrust(store: &Store<'_>, key: &str) -> Result<Option<CommitId>, CoreError> {
    change(
        store,
        "source.untrust",
        format!("source: untrust {key}"),
        |_, file| match file.sources.remove(key) {
            Some(_) => Ok(true),
            None => Err(missing_key(key)),
        },
    )
}

/// Makes the trust that admits `source` lapse because of `change` (one
/// commit): the repository's record gets a `lapsed` table (a repository
/// trusted through its owner or host gets its own lapsed record). Nothing
/// is written when no trust admits the source (it asks or is blocked
/// anyway) or the trust already lapsed.
pub fn lapse(
    store: &Store<'_>,
    source: &SourceRef,
    found: &SourceChange,
) -> Result<Option<CommitId>, CoreError> {
    let current = load(store)?;
    let TrustState::Trusted { record, .. } = state_of(&current, source) else {
        return Ok(None);
    };
    let reason = found.kind.reason();
    let key = source.key().to_owned();
    change(
        store,
        "source.lapse",
        format!("source: trust lapsed for {key} ({})", reason.as_str()),
        |tx, file| {
            let lapse = Lapse {
                reason,
                at: utc_datetime(tx.store().env().now()),
                detail: found.detail.clone(),
                extra: toml::Table::new(),
            };
            match file.sources.get_mut(&key) {
                Some(own) => own.lapsed = Some(lapse),
                None => {
                    let mut own = record.clone();
                    own.scope = SourceScope::Repo;
                    own.url = Some(source.url().to_owned());
                    own.lapsed = Some(lapse);
                    own.extra = toml::Table::new();
                    file.sources.insert(key.clone(), own);
                }
            }
            Ok(true)
        },
    )
}

#[cfg(test)]
mod tests;

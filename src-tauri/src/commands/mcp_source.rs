//! MCP servers from a Git repository or a local folder (#5, SPEC 13.2):
//! the review of a source's drafts (provenance, inputs, every launch line,
//! scan findings, source trust), the save of the drafts the user picked
//! and edited, the update check with its diff, and the review and restore
//! of a machine-local checkout. A review keeps the fetched source in the
//! app under its token (at most [`Reviews::MAX`]; evicted and discarded
//! ones are dropped off the async thread, and a discarded one never comes
//! back). No absolute path and no secret value crosses IPC: the home folder
//! is `~`, `LOADOUT_HOME` its placeholder, a folder outside home
//! `<folder>`, URLs masked, and a secret literal read from a source file is
//! replaced by a reference and dropped by the core.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use agent_loadout_core::apply::placeholder_path;
use agent_loadout_core::mcp_source::{
    self, CheckoutRemoval, Draft, DraftInput, DraftKind, Pick, PreparedCheckout, ResolveRequest,
    Resolved, Skipped,
};
use agent_loadout_core::model::server::{Server, ServerSource, ServerSourceKind};
use agent_loadout_core::remote::git::{IDLE_TIMEOUT, SystemRunner};
use agent_loadout_core::skills::install::{Decisions, Source};
use agent_loadout_core::skills::{LinkKind, SkillsEnv};
use agent_loadout_core::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use specta::Type;

use super::scan::ScanResult;
use super::servers::{KeyValue, ServerDraft, apply_draft, draft_of};
use super::skills::{SkillFileChange, SkillUpdateStatus};
use super::sources::{self, SourceTrustInfo, TrustGrantArg};
use super::{AppState, ChangeResult, CmdResult, Core, IpcError, NO_REGISTRY, blocking, scrubbed};

static RUNNER: SystemRunner = SystemRunner {
    idle_timeout: IDLE_TIMEOUT,
};

fn sx(core: &Core) -> SkillsEnv<'_> {
    SkillsEnv {
        env: &core.env,
        fs: core.fs.as_ref(),
        home: &core.home,
        registry: &NO_REGISTRY,
        runner: &RUNNER,
        link_kind: LinkKind::Junction,
    }
}

fn scrub(core: &Core, err: &CoreError) -> IpcError {
    scrubbed(&core.env, err, None)
}

/// Moves the machine-local checkouts of servers being removed aside; call
/// inside the removal's transaction, then [`CheckoutRemoval::finish`].
pub(super) fn set_aside_checkouts(
    core: &Core,
    ids: &[String],
) -> Result<CheckoutRemoval, CoreError> {
    CheckoutRemoval::begin(&sx(core), ids)
}

/// Removes temporary clones an earlier run left (reviews still open when
/// the app ended): run once at startup.
pub(crate) fn sweep_clones(core: &Core) {
    mcp_source::sweep_temp_clones(&sx(core), std::time::Duration::from_secs(60 * 60));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum McpDraftKind {
    Registry,
    Bundle,
    ClientConfig,
    Package,
}

impl From<DraftKind> for McpDraftKind {
    fn from(kind: DraftKind) -> Self {
        match kind {
            DraftKind::Registry => Self::Registry,
            DraftKind::Bundle => Self::Bundle,
            DraftKind::ClientConfig => Self::ClientConfig,
            DraftKind::Package => Self::Package,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum McpSourceKind {
    Git,
    Local,
    /// An MCP registry catalog entry (#26).
    Registry,
}

/// Where the drafts come from.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct McpSourceInfo {
    pub kind: McpSourceKind,
    /// The git URL, userinfo masked.
    pub url: Option<String>,
    /// A local folder under its placeholder root (`~/code/weather`), or
    /// `…/<name>` outside the home folder.
    pub folder: Option<String>,
    /// The folder inside the repository.
    pub path: Option<String>,
    pub git_ref: Option<String>,
    pub commit: Option<String>,
    /// The catalog entry it comes from, `<catalog>/<entry id>` (#26).
    pub catalog: Option<String>,
    /// A registry entry's version.
    pub version: Option<String>,
}

/// A value the user provides after saving.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct McpDraftInput {
    /// `env.NAME`, `headers.NAME`, `args[N]`, `url`, `command`, each also
    /// `os.<name>.`-prefixed for an override, or `client.<id>.<key>`.
    pub field: String,
    pub description: Option<String>,
    /// The draft holds `${secret:alias}`: set that secret (a passthrough
    /// value has no alias: it was left out).
    pub secret: bool,
    pub required: bool,
    pub alias: Option<String>,
    /// The file held a literal value here; it was not kept.
    pub value_dropped: bool,
}

impl From<&DraftInput> for McpDraftInput {
    fn from(input: &DraftInput) -> Self {
        Self {
            field: input.field.clone(),
            description: input.description.clone(),
            secret: input.secret,
            required: input.required,
            alias: input.alias.clone(),
            value_dropped: input.value_dropped,
        }
    }
}

/// One launch line: the base definition (`os` null) or an OS override.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct McpLaunchLine {
    /// `windows`, `macos`, `linux`; null for the base definition.
    pub os: Option<String>,
    pub line: String,
}

/// One proposed server.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct McpDraft {
    /// Names the draft in `save_mcp_source_drafts`.
    pub key: String,
    pub kind: McpDraftKind,
    /// The file it came from, relative to the folder.
    pub file: String,
    /// `registry`, `bundle`, `client:<id>`, `npm`, `python`, `docker`.
    pub converter: String,
    pub entry: Option<String>,
    /// The editor's Form (no clients chosen yet).
    pub draft: ServerDraft,
    /// The exact command line or URL, as saved (the base definition).
    pub launch: String,
    /// Every launch line: the base, then each OS override as it runs there.
    pub launch_lines: Vec<McpLaunchLine>,
    pub inputs: Vec<McpDraftInput>,
    /// It runs code from the source every time a client starts it.
    pub runs_repo_code: bool,
    /// It runs from a machine-local clone of the repository.
    pub checkout: bool,
    /// The command names a version or commit; false: "not pinned".
    pub pinned: bool,
    /// Steps the user runs first (`docker build ...`); never run by
    /// Loadout. Paths under their placeholder roots.
    pub setup: Vec<String>,
    /// Other files with an identical definition.
    pub also_in: Vec<String>,
    /// The folder's files and every launch line.
    pub scan: ScanResult,
    /// What this draft runs, when it is gated on its own (a registry
    /// entry's package or remote host): the trust dialog asks per key, and
    /// the answer goes back in its `McpDraftPick.trust`. Null: the
    /// review's `trust` gates it.
    pub trust: Option<SourceTrustInfo>,
}

/// A definition the source holds that cannot be drafted.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct McpSkipped {
    pub file: String,
    pub entry: Option<String>,
    pub problem: IpcError,
}

/// The review of a source.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct McpSourceReview {
    pub token: String,
    pub source: McpSourceInfo,
    pub drafts: Vec<McpDraft>,
    pub skipped: Vec<McpSkipped>,
    /// Dependency and build folders not read or scanned (`node_modules`,
    /// `dist`, ...): shown as an info note.
    pub skipped_folders: Vec<String>,
    /// The trust dialog's view of the source (git sources and folders).
    pub trust: Option<SourceTrustInfo>,
    /// The trust dialog's view of where the host redirected the repository,
    /// when that is another source (asked too; `trust` stays the requested
    /// source).
    pub moved_trust: Option<SourceTrustInfo>,
    /// The folder's files.
    pub scan: ScanResult,
}

/// One draft to save, as the editor has it.
#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct McpDraftPick {
    pub key: String,
    pub draft: ServerDraft,
    /// The answer for the draft's own `trust`, when it asks.
    pub trust: Option<TrustGrantArg>,
}

/// What a save did.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct McpSourceSaved {
    pub ids: Vec<String>,
    pub change: ChangeResult,
    /// The scan the gate checked: the folder's files and every saved
    /// launch line.
    pub scan: ScanResult,
}

/// A launch field an update changes (values as stored: references only).
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct McpFieldChange {
    /// `transport`, `command`, `args`, `url`, `cwd`, `env.NAME`,
    /// `headers.NAME`, each also `os.<name>.`-prefixed.
    pub field: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

/// The update check of a source server.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct McpUpdateCheck {
    pub id: String,
    pub status: SkillUpdateStatus,
    pub current: Option<String>,
    /// The source's commit, or a folder's content hash: pass it back as
    /// `latest` to `update_mcp_source_server`.
    pub latest: Option<String>,
    pub changes: Vec<McpFieldChange>,
    /// Changed files of a checkout.
    pub files: Vec<SkillFileChange>,
    /// The new command line or URL (the base definition).
    pub launch: Option<String>,
    /// Every new launch line, OS overrides included.
    pub launch_lines: Vec<McpLaunchLine>,
    pub runs_repo_code: bool,
    /// The new version's setup steps.
    pub setup: Vec<String>,
    /// The update replaces the checkout: run the setup steps again.
    pub setup_again: bool,
    /// This machine's checkout is missing or not at the commit the server
    /// file names: its entries are blocked (`checkout_out_of_date`) until
    /// `review_mcp_checkout` and `restore_mcp_checkout`.
    pub checkout_out_of_date: bool,
    pub scan: Option<ScanResult>,
    pub trust: Option<SourceTrustInfo>,
    /// Where the host redirected the repository, when another source.
    pub moved_trust: Option<SourceTrustInfo>,
    pub error: Option<IpcError>,
}

/// What restoring this machine's checkout of a server would put in place.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct McpCheckoutReview {
    pub token: String,
    pub id: String,
    /// The commit the server file names.
    pub commit: String,
    pub trust: Option<SourceTrustInfo>,
    pub moved_trust: Option<SourceTrustInfo>,
    /// The repository's files and the server's saved launch lines.
    pub scan: ScanResult,
}

/// A review kept under its token.
enum Review {
    Source(Box<Resolved>),
    Checkout(Box<PreparedCheckout>),
}

/// Reviews kept, at most [`Reviews::MAX`]: the oldest is dropped (and its
/// temporary clone removed, off the async thread) when another arrives.
/// A review discarded while a save holds it is remembered, so the save's
/// failure does not bring it back.
#[derive(Default)]
struct Reviews {
    next: u64,
    items: Vec<(String, Review)>,
    discarded: HashSet<String>,
}

impl Reviews {
    const MAX: usize = 8;

    /// Takes out what is over [`Reviews::MAX`].
    fn over(&mut self) -> Vec<Review> {
        let over = self.items.len().saturating_sub(Self::MAX);
        self.items.drain(..over).map(|(_, review)| review).collect()
    }
}

static REVIEWS: LazyLock<Mutex<Reviews>> = LazyLock::new(Mutex::default);

fn reviews() -> std::sync::MutexGuard<'static, Reviews> {
    REVIEWS.lock().unwrap_or_else(|err| err.into_inner())
}

/// Drops reviews on a worker thread: their temporary clones are removed
/// there, never on the async runtime or under the reviews' lock.
fn drop_off_thread(reviews: Vec<Review>) {
    if !reviews.is_empty() {
        tauri::async_runtime::spawn_blocking(move || drop(reviews));
    }
}

fn remember(review: Review) -> String {
    let (token, evicted) = {
        let mut reviews = reviews();
        reviews.next += 1;
        let token = format!("mcp-review-{}", reviews.next);
        reviews.items.push((token.clone(), review));
        let evicted = reviews.over();
        (token, evicted)
    };
    drop_off_thread(evicted);
    token
}

fn not_known() -> IpcError {
    IpcError {
        code: ErrorCode::NotFound,
        message: "the review is not known any more; review the source again".to_owned(),
    }
}

fn take(token: &str) -> CmdResult<Review> {
    let mut reviews = reviews();
    let index = reviews
        .items
        .iter()
        .position(|(known, _)| known == token)
        .ok_or_else(not_known)?;
    Ok(reviews.items.remove(index).1)
}

/// Puts a review back after a failed save (the cap holds; a review
/// discarded meanwhile is dropped instead).
fn keep(token: String, review: Review) {
    let dropped = {
        let mut reviews = reviews();
        if reviews.discarded.remove(&token) {
            vec![review]
        } else {
            reviews.items.push((token, review));
            reviews.over()
        }
    };
    drop_off_thread(dropped);
}

fn discard(token: &str) {
    let dropped = {
        let mut reviews = reviews();
        match reviews.items.iter().position(|(known, _)| known == token) {
            Some(index) => vec![reviews.items.remove(index).1],
            None => {
                // A save holds it now: it is dropped when the save ends.
                reviews.discarded.insert(token.to_owned());
                Vec::new()
            }
        }
    };
    drop_off_thread(dropped);
}

/// Tells a finished save that its review is spent (no tombstone left).
fn settle(token: &str) {
    reviews().discarded.remove(token);
}

/// A source folder (`${home}/...` or an absolute path) as shown: `~/...`,
/// or `…/<name>` outside the home folder.
fn shown_folder(folder: &str) -> String {
    match folder.strip_prefix("${home}") {
        Some(rest) => format!("~{rest}"),
        None => format!(
            "…/{}",
            folder
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(folder)
        ),
    }
}

/// A value as it crosses IPC: a folder outside home as `<folder>`, then the
/// home folder as `~` and `LOADOUT_HOME` under its placeholder.
fn shown(core: &Core, folder: Option<&str>, text: &str) -> String {
    let mut out = text.to_owned();
    if let Some(folder) = folder.filter(|folder| !folder.starts_with("${home}")) {
        out = out.replace(folder, "<folder>");
    }
    let loadout_home = core.home.path.to_string_lossy().replace('\\', "/");
    out = out.replace(
        &loadout_home,
        &placeholder_path(&core.env, &core.home.path, None),
    );
    super::scrub(&core.env, &out, None)
}

/// A setup step as shown: the store's `${home}` and `${loadout_home}` under
/// their placeholder roots (they are not shell variables).
fn shown_step(core: &Core, folder: Option<&str>, step: &str) -> String {
    let loadout_home = placeholder_path(&core.env, &core.home.path, None);
    shown(
        core,
        folder,
        &mcp_source::setup_text(step, "~", &loadout_home),
    )
}

fn local_folder(source: &ServerSource) -> Option<&str> {
    source.folder.as_deref()
}

fn source_info(core: &Core, source: &ServerSource) -> McpSourceInfo {
    McpSourceInfo {
        kind: match source.kind {
            ServerSourceKind::Git => McpSourceKind::Git,
            ServerSourceKind::Local => McpSourceKind::Local,
            ServerSourceKind::Registry => McpSourceKind::Registry,
        },
        url: source
            .url
            .as_deref()
            .map(|url| super::scrub(&core.env, &super::remotes::mask_url(url), None)),
        folder: source.folder.as_deref().map(shown_folder),
        path: source.path.clone(),
        git_ref: source.git_ref.clone(),
        commit: source.commit.clone(),
        catalog: source.catalog.clone(),
        version: source.version.clone(),
    }
}

fn shown_pairs(core: &Core, folder: Option<&str>, rows: &[KeyValue]) -> Vec<KeyValue> {
    rows.iter()
        .map(|row| KeyValue {
            key: row.key.clone(),
            value: shown(core, folder, &row.value),
        })
        .collect()
}

/// The Form of `server` as it crosses IPC ([`shown`] values).
fn shown_draft(core: &Core, folder: Option<&str>, server: &Server) -> ServerDraft {
    let mut draft = draft_of(server);
    let show = |text: &Option<String>| text.as_deref().map(|text| shown(core, folder, text));
    draft.command = show(&draft.command);
    draft.url = show(&draft.url);
    draft.cwd = show(&draft.cwd);
    draft.args = draft
        .args
        .iter()
        .map(|arg| shown(core, folder, arg))
        .collect();
    draft.env = shown_pairs(core, folder, &draft.env);
    draft.headers = shown_pairs(core, folder, &draft.headers);
    draft
}

/// The edited Form with each value still as shown put back to the stored
/// one, so a placeholder never replaces a real path.
fn restore_shown(
    core: &Core,
    folder: Option<&str>,
    server: &Server,
    edited: &ServerDraft,
) -> ServerDraft {
    let original = draft_of(server);
    let mut draft = edited.clone();
    let back = |edited: &mut Option<String>, original: &Option<String>| {
        if let Some(original) = original
            && edited.as_deref() == Some(shown(core, folder, original).as_str())
        {
            *edited = Some(original.clone());
        }
    };
    back(&mut draft.command, &original.command);
    back(&mut draft.url, &original.url);
    back(&mut draft.cwd, &original.cwd);
    for (arg, original) in draft.args.iter_mut().zip(&original.args) {
        if *arg == shown(core, folder, original) {
            arg.clone_from(original);
        }
    }
    for (rows, originals) in [
        (&mut draft.env, &original.env),
        (&mut draft.headers, &original.headers),
    ] {
        let by_key: BTreeMap<&str, &str> = originals
            .iter()
            .map(|row| (row.key.as_str(), row.value.as_str()))
            .collect();
        for row in rows.iter_mut() {
            if let Some(original) = by_key.get(row.key.as_str())
                && row.value == shown(core, folder, original)
            {
                row.value = (*original).to_owned();
            }
        }
    }
    draft
}

fn launch_lines(core: &Core, folder: Option<&str>, server: &Server) -> Vec<McpLaunchLine> {
    mcp_source::launch_lines(server)
        .into_iter()
        .map(|line| McpLaunchLine {
            os: line.os,
            line: shown(core, folder, &line.line),
        })
        .collect()
}

fn draft_info(core: &Core, draft: &Draft) -> CmdResult<McpDraft> {
    let source = draft.provenance();
    let folder = local_folder(source);
    let lines = launch_lines(core, folder, &draft.server);
    let trust = draft
        .trust_source
        .as_ref()
        .map(|source| sources::check_source(core, source, None))
        .transpose()?;
    Ok(McpDraft {
        key: draft.key.clone(),
        kind: draft.kind.into(),
        file: source.file.clone(),
        converter: source.converter.clone(),
        entry: source.entry.clone(),
        draft: shown_draft(core, folder, &draft.server),
        launch: lines
            .first()
            .map(|line| line.line.clone())
            .unwrap_or_default(),
        launch_lines: lines,
        inputs: draft.inputs.iter().map(McpDraftInput::from).collect(),
        runs_repo_code: draft.runs_repo_code,
        checkout: draft.checkout,
        pinned: draft.pinned,
        setup: draft
            .setup
            .iter()
            .map(|step| shown_step(core, folder, step))
            .collect(),
        also_in: draft.also_in.clone(),
        scan: ScanResult::from(&draft.scan),
        trust,
    })
}

fn skipped_info(core: &Core, skipped: &Skipped) -> McpSkipped {
    McpSkipped {
        file: skipped.file.clone(),
        entry: skipped.entry.clone(),
        problem: scrub(core, &skipped.error),
    }
}

fn review_of(core: &Core, resolved: &Resolved) -> CmdResult<McpSourceReview> {
    let trust = resolved
        .trust_source()
        .map(|source| sources::check_source(core, source, resolved.source_change()))
        .transpose()?;
    let moved_trust = resolved
        .moved_source()
        .map(|source| sources::check_source(core, source, None))
        .transpose()?;
    Ok(McpSourceReview {
        token: String::new(),
        source: source_info(core, resolved.source()),
        drafts: resolved
            .drafts()
            .iter()
            .map(|draft| draft_info(core, draft))
            .collect::<CmdResult<_>>()?,
        skipped: resolved
            .skipped()
            .iter()
            .map(|skipped| skipped_info(core, skipped))
            .collect(),
        skipped_folders: resolved.skipped_folders().to_vec(),
        trust,
        moved_trust,
        scan: ScanResult::from(resolved.files_scan()),
    })
}

fn trimmed(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn resolve_with(core: &Core, request: &ResolveRequest) -> CmdResult<(Resolved, McpSourceReview)> {
    let store = core.open_store()?;
    let resolved =
        mcp_source::resolve(store.as_ref(), &sx(core), request).map_err(|err| scrub(core, &err))?;
    let review = review_of(core, &resolved)?;
    Ok((resolved, review))
}

/// Clones and reads a git source for the review (`url`: https, ssh,
/// `file://`; `path`: a folder inside it; `git_ref`: branch, tag or
/// commit). Nothing goes into the store; a blocked source is not
/// contacted.
pub fn review_git(
    core: &Core,
    url: &str,
    path: Option<String>,
    git_ref: Option<String>,
) -> CmdResult<(Resolved, McpSourceReview)> {
    let url = url.trim();
    let source = Source::parse(url, core.fs.as_ref());
    if !matches!(source, Source::Git { .. }) || Path::new(url).is_absolute() {
        return Err(IpcError {
            code: ErrorCode::InvalidValue,
            message: "not a git URL".to_owned(),
        });
    }
    let mut request = ResolveRequest::new(source);
    request.path = trimmed(path);
    request.git_ref = trimmed(git_ref);
    resolve_with(core, &request)
}

/// Reads a picked folder for the review.
pub fn review_local(core: &Core, folder: &Path) -> CmdResult<(Resolved, McpSourceReview)> {
    resolve_with(
        core,
        &ResolveRequest::new(Source::Local(folder.to_path_buf())),
    )
}

/// The review of drafts a catalog entry resolved into (#26): saved with
/// `save_mcp_source_drafts` like any other.
pub(super) fn review_resolved(
    core: &Core,
    resolved: Resolved,
) -> CmdResult<(Resolved, McpSourceReview)> {
    let review = review_of(core, &resolved)?;
    Ok((resolved, review))
}

/// Keeps a resolved review under a new token.
pub(super) fn remember_resolved(resolved: Resolved) -> String {
    remember(Review::Source(Box::new(resolved)))
}

fn decisions(
    trust: Option<TrustGrantArg>,
    moved_trust: Option<TrustGrantArg>,
    allow_dangerous: bool,
) -> Decisions {
    let (trust, via) = sources::grant_of(trust);
    Decisions {
        allow_dangerous,
        trust,
        moved_trust: sources::grant_of(moved_trust).0,
        via,
    }
}

/// Saves the picked drafts as the editor has them. On an error `resolved`
/// stays usable.
pub fn save(
    core: &Core,
    resolved: &mut Resolved,
    picks: &[McpDraftPick],
    trust: Option<TrustGrantArg>,
    moved_trust: Option<TrustGrantArg>,
    allow_dangerous: bool,
) -> CmdResult<McpSourceSaved> {
    let folder = local_folder(resolved.source()).map(str::to_owned);
    let mut chosen = Vec::with_capacity(picks.len());
    for pick in picks {
        let draft = resolved
            .drafts()
            .iter()
            .find(|draft| draft.key == pick.key)
            .ok_or_else(|| IpcError {
                code: ErrorCode::NotFound,
                message: format!("no draft {:?} in this review", pick.key),
            })?;
        let edited = restore_shown(core, folder.as_deref(), &draft.server, &pick.draft);
        let mut server = draft.server.clone();
        server.id = edited.id.trim().to_owned();
        apply_draft(&mut server, &edited)?;
        chosen.push(Pick {
            key: pick.key.clone(),
            server,
            trust: pick.trust.map(TrustGrantArg::grant),
        });
    }
    let store = core.open_store_for_change()?;
    let saved = mcp_source::save(
        &store,
        &sx(core),
        &core.apply_env(),
        resolved,
        &chosen,
        &decisions(trust, moved_trust, allow_dangerous),
    )
    .map_err(|err| scrub(core, &err))?;
    Ok(McpSourceSaved {
        ids: saved.ids,
        change: ChangeResult::new(&core.env, saved.commit.as_ref(), saved.apply.as_ref()),
        scan: ScanResult::from(&saved.scan),
    })
}

fn require_store(core: &Core) -> CmdResult<agent_loadout_core::store::Store<'_>> {
    core.open_store()?.ok_or_else(|| IpcError {
        code: ErrorCode::NotFound,
        message: "there is no server in the store yet".to_owned(),
    })
}

/// Checks a source server for an update (contacts a git source).
pub fn check_update(core: &Core, id: &str) -> CmdResult<McpUpdateCheck> {
    let store = require_store(core)?;
    let check = mcp_source::check_update(&store, &sx(core), id).map_err(|err| scrub(core, &err))?;
    let trust = check
        .trust
        .as_ref()
        .map(|trust| sources::trust_info(&core.env, trust, check.source_change.as_ref()));
    let moved_trust = check
        .moved_trust
        .as_ref()
        .map(|trust| sources::trust_info(&core.env, trust, None));
    let folder = check
        .server
        .as_ref()
        .and_then(|server| server.body.meta.as_ref())
        .and_then(|meta| meta.source.as_ref())
        .and_then(local_folder)
        .map(str::to_owned);
    let folder = folder.as_deref();
    let lines = check
        .server
        .as_ref()
        .map(|server| launch_lines(core, folder, server))
        .unwrap_or_default();
    Ok(McpUpdateCheck {
        id: check.id,
        status: check.status.into(),
        current: check.current,
        latest: check.latest,
        changes: check
            .changes
            .into_iter()
            .map(|change| McpFieldChange {
                field: change.field,
                old: change.old.map(|text| shown(core, folder, &text)),
                new: change.new.map(|text| shown(core, folder, &text)),
            })
            .collect(),
        files: check
            .files
            .iter()
            .map(|change| SkillFileChange {
                path: change.path.clone(),
                kind: change.kind.into(),
                script: change.script,
            })
            .collect(),
        launch: lines.first().map(|line| line.line.clone()),
        launch_lines: lines,
        runs_repo_code: check.runs_repo_code,
        setup: check
            .setup
            .iter()
            .map(|step| shown_step(core, folder, step))
            .collect(),
        setup_again: check.setup_again,
        checkout_out_of_date: check.checkout_out_of_date,
        scan: check.scan.as_ref().map(ScanResult::from),
        trust,
        moved_trust,
        error: check.error.as_ref().map(|err| scrub(core, err)),
    })
}

/// Updates a source server to `latest`, what its check showed (the commit,
/// or a folder's content hash; `entry_changed` when the source moved on).
pub fn update(
    core: &Core,
    id: &str,
    latest: Option<&str>,
    trust: Option<TrustGrantArg>,
    moved_trust: Option<TrustGrantArg>,
    allow_dangerous: bool,
) -> CmdResult<ChangeResult> {
    let store = require_store(core)?;
    let updated = mcp_source::update(
        &store,
        &sx(core),
        &core.apply_env(),
        id,
        latest,
        &decisions(trust, moved_trust, allow_dangerous),
    )
    .map_err(|err| scrub(core, &err))?;
    Ok(ChangeResult::new(
        &core.env,
        updated.commit.as_ref(),
        updated.apply.as_ref(),
    ))
}

/// Fetches what this machine's checkout of `id` should hold, for the
/// restore review; `None` when it is already current.
pub fn review_checkout(
    core: &Core,
    id: &str,
) -> CmdResult<Option<(PreparedCheckout, McpCheckoutReview)>> {
    let store = require_store(core)?;
    let Some(prepared) =
        mcp_source::prepare_checkout(&store, &sx(core), id).map_err(|err| scrub(core, &err))?
    else {
        return Ok(None);
    };
    let trust = prepared
        .trust_source()
        .map(|source| sources::check_source(core, source, prepared.source_change()))
        .transpose()?;
    let moved_trust = prepared
        .moved_source()
        .map(|source| sources::check_source(core, source, None))
        .transpose()?;
    let review = McpCheckoutReview {
        token: String::new(),
        id: prepared.id().to_owned(),
        commit: prepared.commit().to_owned(),
        trust,
        moved_trust,
        scan: ScanResult::from(prepared.scan()),
    };
    Ok(Some((prepared, review)))
}

/// Restores a reviewed checkout behind the gates of a save.
pub fn restore_checkout(
    core: &Core,
    prepared: PreparedCheckout,
    trust: Option<TrustGrantArg>,
    moved_trust: Option<TrustGrantArg>,
    allow_dangerous: bool,
) -> CmdResult<ChangeResult> {
    let store = require_store(core)?;
    let report = mcp_source::restore_checkout(
        &store,
        &sx(core),
        &core.apply_env(),
        prepared,
        &decisions(trust, moved_trust, allow_dangerous),
    )
    .map_err(|err| scrub(core, &err))?;
    Ok(ChangeResult::new(&core.env, None, report.as_ref()))
}

#[tauri::command]
#[specta::specta]
pub async fn review_mcp_git_source(
    state: tauri::State<'_, AppState>,
    url: String,
    path: Option<String>,
    git_ref: Option<String>,
) -> CmdResult<McpSourceReview> {
    let (resolved, mut review) =
        blocking(&state, move |core| review_git(core, &url, path, git_ref)).await?;
    review.token = remember(Review::Source(Box::new(resolved)));
    Ok(review)
}

/// Reads the folder picked under `token` (`pick_skill_source` with
/// `kind: "folder"`).
#[tauri::command]
#[specta::specta]
pub async fn review_mcp_local_source(
    state: tauri::State<'_, AppState>,
    token: String,
) -> CmdResult<McpSourceReview> {
    let folder = state.picked_folder(&token)?;
    let (resolved, mut review) = blocking(&state, move |core| review_local(core, &folder)).await?;
    review.token = remember(Review::Source(Box::new(resolved)));
    Ok(review)
}

/// Saves the picked drafts. On an error (`source_trust_required`,
/// `scan_blocked`, `already_exists`, ...) the review is kept for a retry,
/// unless it was discarded meanwhile; on success it is spent.
#[tauri::command]
#[specta::specta]
pub async fn save_mcp_source_drafts(
    state: tauri::State<'_, AppState>,
    token: String,
    picks: Vec<McpDraftPick>,
    trust: Option<TrustGrantArg>,
    moved_trust: Option<TrustGrantArg>,
    allow_dangerous: bool,
) -> CmdResult<McpSourceSaved> {
    let Review::Source(mut resolved) = take(&token)? else {
        return Err(not_known());
    };
    let (resolved, result) = blocking(&state, move |core| {
        let result = save(
            core,
            &mut resolved,
            &picks,
            trust,
            moved_trust,
            allow_dangerous,
        );
        Ok((resolved, result))
    })
    .await?;
    if result.is_err() {
        keep(token, Review::Source(resolved));
    } else {
        settle(&token);
        drop_off_thread(vec![Review::Source(resolved)]);
    }
    result
}

/// Drops a review and its temporary clone (the dialog closed).
#[tauri::command]
#[specta::specta]
pub async fn discard_mcp_source_review(token: String) -> CmdResult<()> {
    discard(&token);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn check_mcp_source_update(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CmdResult<McpUpdateCheck> {
    blocking(&state, move |core| check_update(core, &id)).await
}

#[tauri::command]
#[specta::specta]
pub async fn update_mcp_source_server(
    state: tauri::State<'_, AppState>,
    id: String,
    latest: Option<String>,
    trust: Option<TrustGrantArg>,
    moved_trust: Option<TrustGrantArg>,
    allow_dangerous: bool,
) -> CmdResult<ChangeResult> {
    blocking(&state, move |core| {
        update(
            core,
            &id,
            latest.as_deref(),
            trust,
            moved_trust,
            allow_dangerous,
        )
    })
    .await
}

/// Fetches and scans what this machine's checkout of `id` should hold;
/// `null` when it is up to date. Discard with `discard_mcp_source_review`.
#[tauri::command]
#[specta::specta]
pub async fn review_mcp_checkout(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CmdResult<Option<McpCheckoutReview>> {
    let reviewed = blocking(&state, move |core| review_checkout(core, &id)).await?;
    Ok(reviewed.map(|(prepared, mut review)| {
        review.token = remember(Review::Checkout(Box::new(prepared)));
        review
    }))
}

/// Puts the reviewed checkout in place, behind the trust answers and the
/// scan gate; the review is spent either way (review again to retry).
#[tauri::command]
#[specta::specta]
pub async fn restore_mcp_checkout(
    state: tauri::State<'_, AppState>,
    token: String,
    trust: Option<TrustGrantArg>,
    moved_trust: Option<TrustGrantArg>,
    allow_dangerous: bool,
) -> CmdResult<ChangeResult> {
    let Review::Checkout(prepared) = take(&token)? else {
        return Err(not_known());
    };
    settle(&token);
    blocking(&state, move |core| {
        restore_checkout(core, *prepared, trust, moved_trust, allow_dangerous)
    })
    .await
}

#[cfg(test)]
pub(super) mod test_hooks {
    //! The review map, for tests of its bookkeeping.
    use super::*;

    pub fn remember_source(resolved: Resolved) -> String {
        remember(Review::Source(Box::new(resolved)))
    }

    pub fn take_source(token: &str) -> CmdResult<Resolved> {
        match take(token)? {
            Review::Source(resolved) => Ok(*resolved),
            Review::Checkout(_) => Err(not_known()),
        }
    }

    pub fn keep_source(token: String, resolved: Resolved) {
        keep(token, Review::Source(Box::new(resolved)));
    }

    pub fn discard_review(token: &str) {
        discard(token);
    }

    pub fn known(token: &str) -> bool {
        reviews().items.iter().any(|(known, _)| known == token)
    }

    pub fn count() -> usize {
        reviews().items.len()
    }

    pub const MAX: usize = Reviews::MAX;
}

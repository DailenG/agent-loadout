//! The Loadout skill for agents (#8, SPEC 12.6): a read-only skill that
//! teaches coding agents Loadout's read and plan commands. It ships inside
//! this crate (so the CLI works on its own) and as a resource of the app,
//! and is installed as a normal store skill: id [`SKILL_ID`], marked by its
//! `[bundled]` table, enabled per client through the normal skill sync, so
//! it is versioned, reversible and visible like any other skill.
//!
//! Bundled copies carry no `[source]`: 0.5.x reads any `[source] kind` it
//! does not know as a parse error (and refuses a merge holding it), while it
//! keeps an unknown `[bundled]` table as it is.
//!
//! Updates follow Loadout: when the bundled version is newer than the store
//! copy, [`refresh`] replaces the store copy (one commit), unless the user
//! edited it: content whose [`normalized_hash`] differs from `[bundled]
//! hash` is customized and never touched (only [`install`] with `reset`
//! replaces it, when the user asks).

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::local_settings::LocalSettings;
use crate::model::TomlDocument;
use crate::model::clients::ClientSelection;
use crate::model::skill_meta::{BundledCopy, SkillEnable, SkillMeta};
use crate::paths::LoadoutHome;
use crate::skills::link::{self, CLIENT_SKILLS, DetectedClients, client_skills};
use crate::skills::{self, SkillFiles, SkillsEnv, SyncReport, read_folder, store};
use crate::store::{CommitId, Operation, Store};

/// Store id of the Loadout skill.
pub const SKILL_ID: &str = "loadout";
/// The Loadout version the bundled copy ships with.
pub const BUNDLED_VERSION: &str = env!("CARGO_PKG_VERSION");
/// The skill folder inside the app's resources (`tauri.conf.json`
/// `bundle.resources`) and below `cache/` (`loadout agent skill-path`).
pub const RESOURCE_DIR: &str = "agent-skill/loadout";
/// What `loadout agent status` tells the user to run for an outdated copy.
pub const UPDATE_COMMAND: &str = "loadout agent install";
/// What it tells the user to run to replace a customized copy.
pub const RESET_COMMAND: &str = "loadout agent install --reset";

/// The bundled files, by relative path, as the repository holds them (LF
/// line endings, `.gitattributes`).
pub const FILES: [(&str, &str); 4] = [
    ("SKILL.md", include_str!("../agent-skill/loadout/SKILL.md")),
    (
        "references/commands.md",
        include_str!("../agent-skill/loadout/references/commands.md"),
    ),
    (
        "references/errors.md",
        include_str!("../agent-skill/loadout/references/errors.md"),
    ),
    (
        "references/playbooks.md",
        include_str!("../agent-skill/loadout/references/playbooks.md"),
    ),
];

/// The bundled skill's files.
pub fn bundled_files() -> SkillFiles {
    FILES
        .iter()
        .map(|(path, text)| ((*path).to_owned(), text.as_bytes().to_vec()))
        .collect()
}

/// Files an OS or an editor drops next to the user's files: never part of
/// what decides whether the copy was edited.
fn is_junk(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        ".ds_store" | "thumbs.db" | "ehthumbs.db" | "desktop.ini"
    ) || lower.starts_with("._")
        || lower.starts_with(".#")
        || (lower.starts_with('#') && lower.ends_with('#'))
        || lower.ends_with('~')
        || [".swp", ".swo", ".swx", ".bak", ".orig", ".tmp"]
            .iter()
            .any(|suffix| lower.ends_with(suffix))
}

/// `bytes` without a leading UTF-8 byte order mark and with CRLF as LF.
fn normalized_text(bytes: &[u8]) -> Vec<u8> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let mut out = Vec::with_capacity(bytes.len());
    let mut iter = bytes.iter().peekable();
    while let Some(&byte) = iter.next() {
        if byte == b'\r' && iter.peek() == Some(&&b'\n') {
            continue;
        }
        out.push(byte);
    }
    out
}

/// `sha256:<hex>` over the skill's files as an edit would change them:
/// OS and editor junk files left out ([`is_junk`]), a BOM and CRLF line
/// endings normalized. Resaving a file or opening the folder in a file
/// manager is not an edit.
pub fn normalized_hash(files: &SkillFiles) -> String {
    let mut hasher = Sha256::new();
    for (path, bytes) in files.iter().filter(|(path, _)| !is_junk(path)) {
        let bytes = normalized_text(bytes);
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    let digest = hasher.finalize();
    let mut text = String::with_capacity(7 + digest.len() * 2);
    text.push_str("sha256:");
    for byte in digest {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// [`normalized_hash`] of [`bundled_files`].
pub fn bundled_hash() -> String {
    normalized_hash(&bundled_files())
}

/// Whether `meta` describes a copy of the bundled skill (its `[bundled]`
/// table): the origin shown as "Bundled with Loadout".
pub fn is_bundled(meta: &SkillMeta) -> bool {
    meta.bundled.is_some()
}

/// How the store copy compares with the bundled one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyState {
    /// The bundled version, as Loadout wrote it.
    Current,
    /// Written by an older Loadout and unchanged since: the next store
    /// change, `loadout agent install` or app start updates it.
    Outdated,
    /// Edited since Loadout wrote it: never updated (`install --reset`
    /// replaces it on request).
    Customized,
    /// Written by a newer Loadout (a store synced from another machine):
    /// never replaced by an older copy.
    Newer,
}

impl CopyState {
    pub const fn as_str(self) -> &'static str {
        match self {
            CopyState::Current => "current",
            CopyState::Outdated => "outdated",
            CopyState::Customized => "customized",
            CopyState::Newer => "newer",
        }
    }

    /// The command that changes this state, for `loadout agent status`.
    pub const fn action(self) -> Option<&'static str> {
        match self {
            CopyState::Outdated => Some(UPDATE_COMMAND),
            CopyState::Customized => Some(RESET_COMMAND),
            CopyState::Current | CopyState::Newer => None,
        }
    }
}

/// The Loadout skill in the store, as read at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreCopy {
    /// The Loadout version whose copy was written (`[bundled] version`).
    pub version: String,
    pub state: CopyState,
}

/// One client with a skill folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientStatus {
    pub client: &'static str,
    pub detected: bool,
    /// The store copy is enabled for it at user scope.
    pub enabled: bool,
}

/// What [`status`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStatus {
    pub bundled_version: &'static str,
    /// The store copy; `None` when it is not installed.
    pub copy: Option<StoreCopy>,
    /// A store skill with id [`SKILL_ID`] that Loadout did not bundle (the
    /// user's own): it is never installed over, updated or removed here.
    pub taken: bool,
    /// Every client with a skill folder (table 7.6), omp first.
    pub clients: Vec<ClientStatus>,
}

/// What [`refresh`] did.
#[derive(Debug, Clone)]
pub enum Refresh {
    /// The store copy was replaced by the bundled one (one commit); the
    /// skill sync's report says which client folders need a decision.
    Updated {
        from: String,
        to: String,
        commit: Option<CommitId>,
        report: SyncReport,
    },
    /// A newer copy is bundled, but the store copy was edited: it stays as
    /// it is ("customized, not updated").
    Customized { version: String, bundled: String },
}

/// What [`install`] or [`remove`] did.
#[derive(Debug, Clone, Default)]
pub struct Change {
    /// The last store commit made; `None` when nothing changed.
    pub commit: Option<CommitId>,
    /// Links and copies made or removed.
    pub report: SyncReport,
    /// The update [`install`] made first, if any.
    pub refreshed: Option<Refresh>,
    /// A customized or outdated copy was replaced on request (`reset`).
    pub reset: bool,
    /// The skill was removed from the store.
    pub removed: bool,
}

/// The store skill [`SKILL_ID`], split by origin.
enum Found {
    Missing,
    /// Present, but not bundled (or its metadata is missing or invalid).
    Taken,
    Bundled(Box<SkillMeta>),
}

fn find(store: &Store<'_>) -> Result<Found, CoreError> {
    let Some(skill) = store::get(store, SKILL_ID)? else {
        return Ok(Found::Missing);
    };
    Ok(match skill.meta {
        Some(meta) if is_bundled(&meta) => Found::Bundled(Box::new(meta)),
        _ => Found::Taken,
    })
}

/// How the store copy described by `meta` compares with the bundled one,
/// reading its content through `store`.
fn copy_of(store: &Store<'_>, meta: &SkillMeta) -> StoreCopy {
    let written = meta.bundled.as_ref().expect("bundled metadata");
    // Content that cannot be read is not Loadout's to rewrite either.
    let unchanged = store::read_content(store, SKILL_ID)
        .is_ok_and(|files| normalized_hash(&files) == written.hash);
    let state = if !unchanged {
        CopyState::Customized
    } else {
        match compare_versions(&written.version, BUNDLED_VERSION) {
            Ordering::Less => CopyState::Outdated,
            Ordering::Equal => CopyState::Current,
            Ordering::Greater => CopyState::Newer,
        }
    };
    StoreCopy {
        version: written.version.clone(),
        state,
    }
}

/// The clients with skill folders, omp first (#2), then table order.
fn skill_clients() -> impl Iterator<Item = &'static str> {
    let omp = CLIENT_SKILLS
        .iter()
        .map(|entry| entry.client)
        .filter(|client| *client == "omp");
    let rest = CLIENT_SKILLS
        .iter()
        .map(|entry| entry.client)
        .filter(|client| *client != "omp");
    omp.chain(rest)
}

/// The Loadout skill's state: the store copy (none without a store) and
/// every client with a skill folder. Reads only; takes no lock.
pub fn status(store: Option<&Store<'_>>, sx: &SkillsEnv<'_>) -> Result<AgentStatus, CoreError> {
    let found = match store {
        Some(store) => find(store)?,
        None => Found::Missing,
    };
    let detected = DetectedClients::new(sx);
    let enabled: Vec<&'static str> = match &found {
        Found::Bundled(meta) => meta
            .enable
            .as_ref()
            .and_then(|enable| enable.clients.as_ref())
            .map(|selection| detected.selected(selection))
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let clients = skill_clients()
        .map(|client| ClientStatus {
            client,
            detected: detected.is_detected(client),
            enabled: enabled.contains(&client),
        })
        .collect();
    let (copy, taken) = match (&found, store) {
        (Found::Bundled(meta), Some(store)) => (Some(copy_of(store, meta)), false),
        (Found::Taken, _) => (None, true),
        _ => (None, false),
    };
    Ok(AgentStatus {
        bundled_version: BUNDLED_VERSION,
        copy,
        taken,
        clients,
    })
}

/// What to do about a store skill `loadout` that is the user's own: Loadout
/// has no rename, so the way out is removing it or adding it again under
/// another id.
fn taken_hint() -> String {
    format!(
        "the store skill {SKILL_ID:?} is your own, not the one bundled with Loadout, and is never changed here; to use the Loadout skill, remove yours (loadout skills rm {SKILL_ID}) or add it again under another id (loadout skills add <its folder> --id <other id>) first"
    )
}

/// `clients` checked against table 7.6, or every detected client with a
/// skill folder when empty (`invalid_value` when there is none).
fn chosen_clients(sx: &SkillsEnv<'_>, clients: &[String]) -> Result<Vec<String>, CoreError> {
    check_clients(clients)?;
    if !clients.is_empty() {
        return Ok(clients.to_vec());
    }
    let detected = DetectedClients::new(sx);
    let chosen: Vec<String> = skill_clients()
        .filter(|client| detected.is_detected(client))
        .map(str::to_owned)
        .collect();
    if chosen.is_empty() {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            "no client with a skill folder was detected; name one with --client",
        ));
    }
    Ok(chosen)
}

fn check_clients(clients: &[String]) -> Result<(), CoreError> {
    for client in clients {
        if client_skills(client).is_none() {
            let code = if crate::model::clients::known_client(client).is_some() {
                ErrorCode::ScopeNotSupported
            } else {
                ErrorCode::UnknownClient
            };
            return Err(CoreError::new(
                code,
                format!("{client:?} has no skill folder Loadout writes"),
            ));
        }
    }
    Ok(())
}

/// Installs the Loadout skill for `clients`: a missing copy is added and
/// enabled for them (every detected client with a skill folder when empty)
/// in one commit (`skill: add loadout ...`). An existing bundled copy is
/// first brought up to date ([`refresh`]), or with `reset` replaced by the
/// bundled one whatever its state (`skill: reset loadout ...`, its client
/// selection kept), and then enabled for `clients` ([`link::set_enabled`];
/// with `reset` and no clients the selection is left as it is). Links
/// follow at once. A store skill with the same id that Loadout did not
/// bundle is `already_exists`.
pub fn install(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    clients: &[String],
    reset: bool,
) -> Result<Change, CoreError> {
    match find(store)? {
        Found::Taken => Err(CoreError::new(ErrorCode::AlreadyExists, taken_hint())),
        Found::Bundled(_) => {
            check_clients(clients)?;
            let mut change = Change::default();
            if reset {
                let (commit, report) = replace(store, sx, "reset", None)?;
                change.reset = commit.is_some();
                change.commit = commit;
                change.report = report;
            } else {
                change.refreshed = refresh(store, sx)?;
            }
            if reset && clients.is_empty() {
                return Ok(change);
            }
            let clients = chosen_clients(sx, clients)?;
            let (commit, report) = link::set_enabled(store, sx, SKILL_ID, &clients, true, None)?;
            change.commit = commit.or(change.commit);
            change.report.results.extend(report.results);
            change.report.warnings.extend(report.warnings);
            Ok(change)
        }
        Found::Missing => {
            let clients = chosen_clients(sx, clients)?;
            let files = bundled_files();
            let mut sorted = clients.clone();
            sorted.sort();
            sorted.dedup();
            let summary = format!(
                "skill: add {SKILL_ID} (bundled with Loadout {BUNDLED_VERSION}) for {}",
                clients.join(", ")
            );
            let operation =
                Operation::new("skill.add", summary).entity(format!("skill/{SKILL_ID}"));
            let committed = store.transaction(operation, |tx| {
                store::stage_content(tx, SKILL_ID, &files)?;
                let mut meta = bundled_meta(SkillMeta::new(SKILL_ID), &files);
                meta.enable = Some(SkillEnable {
                    clients: Some(ClientSelection::List(sorted)),
                    ..SkillEnable::default()
                });
                store::stage_meta(tx, &meta)
            })?;
            let report = link::sync(store, sx)?;
            Ok(Change {
                commit: committed.commit,
                report,
                ..Change::default()
            })
        }
    }
}

/// `meta` as the bundled copy `files`: name and `[bundled]`, no
/// `[source]` (see the module notes).
fn bundled_meta(mut meta: SkillMeta, files: &SkillFiles) -> SkillMeta {
    meta.name = Some(SKILL_ID.to_owned());
    meta.source = None;
    meta.bundled = Some(BundledCopy {
        version: BUNDLED_VERSION.to_owned(),
        hash: normalized_hash(files),
        extra: toml::Table::new(),
    });
    meta
}

/// Replaces the store copy by the bundled one (one commit, `skill: <verb>
/// loadout to the copy bundled with Loadout <v>`), keeping the rest of its
/// metadata, then syncs client folders. With `seen`, the copy is checked
/// again under the lock and left alone (no commit, `None`) unless it is
/// still [`CopyState::Outdated`] with that version: another run may have
/// updated it, to this version or a newer one, since `seen` was read.
fn replace(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    verb: &str,
    seen: Option<&StoreCopy>,
) -> Result<(Option<CommitId>, SyncReport), CoreError> {
    let files = bundled_files();
    let summary =
        format!("skill: {verb} {SKILL_ID} to the copy bundled with Loadout {BUNDLED_VERSION}");
    let operation =
        Operation::new(format!("skill.{verb}"), summary).entity(format!("skill/{SKILL_ID}"));
    let committed = store.transaction(operation, |tx| {
        let Some(current) = store::read_meta(tx, SKILL_ID)?.filter(is_bundled) else {
            return Ok(false);
        };
        if let Some(seen) = seen
            && copy_of(tx.store(), &current) != *seen
        {
            return Ok(false);
        }
        store::stage_content(tx, SKILL_ID, &files)?;
        store::stage_meta(tx, &bundled_meta(current, &files))?;
        Ok(true)
    })?;
    if !committed.value {
        return Ok((None, SyncReport::default()));
    }
    let report = link::sync(store, sx)?;
    Ok((committed.commit, report))
}

/// Disables the Loadout skill for `clients`, or with none removes it from
/// the store (one commit each; owned links and copies go with it). Nothing
/// to do when it is not installed. A store skill with the same id that
/// Loadout did not bundle is `invalid_value`.
pub fn remove(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    clients: &[String],
) -> Result<Change, CoreError> {
    match find(store)? {
        Found::Missing => Ok(Change::default()),
        Found::Taken => Err(CoreError::new(ErrorCode::InvalidValue, taken_hint())),
        Found::Bundled(_) if clients.is_empty() => {
            let (commit, report) = skills::remove(store, sx, SKILL_ID)?;
            Ok(Change {
                commit,
                report,
                removed: true,
                ..Change::default()
            })
        }
        Found::Bundled(_) => {
            check_clients(clients)?;
            let (commit, report) = link::set_enabled(store, sx, SKILL_ID, clients, false, None)?;
            Ok(Change {
                commit,
                report,
                ..Change::default()
            })
        }
    }
}

/// Brings the store copy up to date: an [`CopyState::Outdated`] copy is
/// replaced by the bundled one (one commit, `skill: update loadout ...`,
/// then the skill sync rewrites copies); a [`CopyState::Customized`] copy
/// older than the bundled version is reported and left alone. `None` when
/// there is nothing to say (not installed, current, newer, or not
/// Loadout's).
pub fn refresh(store: &Store<'_>, sx: &SkillsEnv<'_>) -> Result<Option<Refresh>, CoreError> {
    let Found::Bundled(meta) = find(store)? else {
        return Ok(None);
    };
    refresh_seen(store, sx, copy_of(store, &meta))
}

/// [`refresh`] acting on a state read earlier, as a run that read it
/// before another run took the store lock does: an update is made only if
/// the copy is still the one `seen` describes, checked again under the
/// lock, so a newer copy written meanwhile is never replaced by an older
/// one and two runs never both report an update.
pub fn refresh_seen(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    seen: StoreCopy,
) -> Result<Option<Refresh>, CoreError> {
    match seen.state {
        CopyState::Outdated => {
            let (commit, report) = replace(store, sx, "update", Some(&seen))?;
            Ok(commit.map(|commit| Refresh::Updated {
                from: seen.version,
                to: BUNDLED_VERSION.to_owned(),
                commit: Some(commit),
                report,
            }))
        }
        CopyState::Customized
            if compare_versions(&seen.version, BUNDLED_VERSION) == Ordering::Less =>
        {
            Ok(Some(Refresh::Customized {
                version: seen.version,
                bundled: BUNDLED_VERSION.to_owned(),
            }))
        }
        _ => Ok(None),
    }
}

/// [`refresh`] for a store change (the CLI's changing commands) and the app
/// start: runs only when the store copy's metadata names an older version
/// (a file read otherwise). A copy already found customized for this
/// bundled version (`state/local-settings.toml` `agent_skill_notice`) is
/// not hashed again: "customized, not updated" is said once per bundled
/// version per machine, and `loadout agent install` still checks it.
pub fn startup_refresh(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
) -> Result<Option<Refresh>, CoreError> {
    if !written_older(sx.fs, sx.home) {
        return Ok(None);
    }
    let state_dir = sx.home.state_dir();
    let mut settings = LocalSettings::load(sx.fs, &state_dir)?;
    if settings.agent_skill_notice.as_deref() == Some(BUNDLED_VERSION) {
        return Ok(None);
    }
    let refreshed = refresh(store, sx)?;
    if let Some(Refresh::Customized { bundled, .. }) = &refreshed {
        settings.agent_skill_notice = Some(bundled.clone());
        settings.save(sx.fs, &state_dir)?;
    }
    Ok(refreshed)
}

/// Whether the store's `skill-meta/loadout.toml` describes a bundled copy
/// of an older version, read straight from the working tree (no store
/// open, no lock). Anything unreadable is `false`: [`status`] reports it.
pub fn written_older(fs: &dyn Fs, home: &LoadoutHome) -> bool {
    let path = home
        .store_dir()
        .join(store::META_DIR)
        .join(format!("{SKILL_ID}.toml"));
    let Ok(text) = fs.read_to_string(&path) else {
        return false;
    };
    let Ok(doc) = TomlDocument::<SkillMeta>::parse(&text) else {
        return false;
    };
    doc.value().bundled.as_ref().is_some_and(|bundled| {
        compare_versions(&bundled.version, BUNDLED_VERSION) == Ordering::Less
    })
}

/// A folder holding the bundled copy, for `loadout agent skill-path`:
/// `cache/agent-skill/loadout`, rewritten when its files differ from the
/// bundled ones.
pub fn materialize(fs: &dyn Fs, home: &LoadoutHome) -> Result<PathBuf, CoreError> {
    let dir = RESOURCE_DIR
        .split('/')
        .fold(home.cache_dir(), |path, part| path.join(part));
    let files = bundled_files();
    if fs.is_dir(&dir) && read_folder(fs, &dir).is_ok_and(|found| found == files) {
        return Ok(dir);
    }
    if fs.exists(&dir) {
        fs.remove_dir_all(&dir).map_err(|err| {
            CoreError::new(ErrorCode::IoError, format!("cannot remove folder: {err}"))
                .with_path(&dir)
        })?;
    }
    skills::write_files(fs, &dir, &files)?;
    Ok(dir)
}

/// Whether `dir` holds the bundled files (the app's resource folder),
/// compared as [`normalized_hash`] does.
pub fn is_bundled_copy(fs: &dyn Fs, dir: &Path) -> bool {
    read_folder(fs, dir).is_ok_and(|found| normalized_hash(&found) == bundled_hash())
}

/// Orders `major.minor.patch[-pre][+build]` versions as semver does:
/// numbers compare as numbers (missing or unreadable ones as 0), a
/// pre-release sorts before its release, pre-release identifiers compare
/// one by one (numeric ones as numbers and before text ones, a shorter
/// list first when all else is equal), build metadata is ignored.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    fn parts(version: &str) -> ([u64; 3], Option<&str>) {
        let version = version.trim();
        let version = version.split_once('+').map_or(version, |(core, _)| core);
        let (core, pre) = match version.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (version, None),
        };
        let mut numbers = [0u64; 3];
        for (slot, part) in numbers.iter_mut().zip(core.split('.')) {
            *slot = part.parse().unwrap_or(0);
        }
        (numbers, pre)
    }
    fn identifier(a: &str, b: &str) -> Ordering {
        match (a.parse::<u64>(), b.parse::<u64>()) {
            (Ok(a), Ok(b)) => a.cmp(&b),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => a.cmp(b),
        }
    }
    let (a_numbers, a_pre) = parts(a);
    let (b_numbers, b_pre) = parts(b);
    a_numbers
        .cmp(&b_numbers)
        .then_with(|| match (a_pre, b_pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => {
                let mut a = a.split('.');
                let mut b = b.split('.');
                loop {
                    match (a.next(), b.next()) {
                        (None, None) => return Ordering::Equal,
                        (None, Some(_)) => return Ordering::Less,
                        (Some(_), None) => return Ordering::Greater,
                        (Some(x), Some(y)) => match identifier(x, y) {
                            Ordering::Equal => {}
                            other => return other,
                        },
                    }
                }
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_order_like_semver() {
        assert_eq!(compare_versions("0.5.0", "0.6.0"), Ordering::Less);
        assert_eq!(compare_versions("0.10.0", "0.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0.0-rc.1", "1.0.0"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0+build", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("garbage", "0.0.1"), Ordering::Less);
        // Pre-release identifiers one by one (semver 11.4).
        assert_eq!(
            compare_versions("1.0.0-rc.10", "1.0.0-rc.9"),
            Ordering::Greater
        );
        assert_eq!(
            compare_versions("1.0.0-alpha", "1.0.0-alpha.1"),
            Ordering::Less
        );
        assert_eq!(
            compare_versions("1.0.0-alpha.1", "1.0.0-alpha.beta"),
            Ordering::Less
        );
        assert_eq!(
            compare_versions("1.0.0-beta.2", "1.0.0-beta.11"),
            Ordering::Less
        );
        assert_eq!(
            compare_versions("1.0.0-rc.1", "1.0.0-beta.11"),
            Ordering::Greater
        );
    }

    /// The raw embedded bytes, not the normalized ones: a CRLF checkout
    /// would fail here.
    #[test]
    fn embedded_files_have_lf_endings_and_a_frontmatter_name() {
        for (path, text) in FILES {
            assert!(!text.contains('\r'), "{path} has CR characters");
            assert!(!text.starts_with('\u{feff}'), "{path} starts with a BOM");
        }
        let fm = crate::skills::frontmatter::parse(FILES[0].1)
            .unwrap()
            .unwrap();
        assert_eq!(fm.name(), Some(SKILL_ID));
        assert!(fm.description().is_some_and(|text| !text.is_empty()));
    }

    #[test]
    fn junk_files_line_endings_and_a_bom_are_not_edits() {
        let files = bundled_files();
        let hash = normalized_hash(&files);
        let mut touched = files.clone();
        for junk in [
            ".DS_Store",
            "Thumbs.db",
            "references/desktop.ini",
            ".SKILL.md.swp",
            "SKILL.md~",
            "._SKILL.md",
        ] {
            touched.insert(junk.to_owned(), b"junk".to_vec());
        }
        let skill = files["SKILL.md"].clone();
        let mut resaved = b"\xef\xbb\xbf".to_vec();
        resaved.extend(
            String::from_utf8(skill)
                .unwrap()
                .replace('\n', "\r\n")
                .into_bytes(),
        );
        touched.insert("SKILL.md".to_owned(), resaved);
        assert_eq!(normalized_hash(&touched), hash);
        // A real edit, or a new real file, is one.
        let mut edited = files.clone();
        edited.insert("SKILL.md".to_owned(), b"mine".to_vec());
        assert_ne!(normalized_hash(&edited), hash);
        let mut added = files;
        added.insert("notes.md".to_owned(), b"mine".to_vec());
        assert_ne!(normalized_hash(&added), hash);
    }
}

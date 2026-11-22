//! Folder remotes (SPEC 9.6): a local or network directory holding one
//! subfolder per machine, written only by that machine:
//!
//! ```text
//! <path>/<store_id>/<machine_id>/
//!   machine.json   written last
//!   store.git/     repo-per-machine: a bare repository
//!   latest/        mirror: the HEAD tree and EXPORT.json
//!   archives/      archive: loadout-<yyyymmdd-hhmmss>-<shortsha>.zip
//! ```
//!
//! Readers accept only names of that grammar (ULID store and machine
//! folders, the files above) and only the ids `machine.json` lists;
//! everything else is skipped, and names that match a sync tool's conflict
//! or temp pattern, or refs `machine.json` does not list, are reported as
//! `sync_artifact_ignored`. Nothing a reader skips is ever deleted.
//! A remote that is not there (`remote_unreachable`) never blocks local
//! work; Loadout never creates the remote's own folder or mounts anything.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::env::Os;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::ids::{self, IdKind};
use crate::remote::config::{DEFAULT_KEEP, FolderMode, RemoteConfig, RemoteKind};
use crate::remote::manifest::{EXPORT_FILE, ExportInfo, MachineManifest};
use crate::remote::sync_artifacts::{Artifact, artifact_tool};
use crate::remote::unc::{UncResolver, resolve};
use crate::store::merge::{Files, MergeOutcome, PendingMerge, RecordedConflict, write_merge_state};
use crate::store::{CommitId, Machine, Store, bare_commit_subject, utc_datetime};

const STORE_GIT: &str = "store.git";
const LATEST: &str = "latest";
const ARCHIVES: &str = "archives";

/// The directory a folder remote resolves to now, or `remote_unreachable`.
pub fn reachable_root(
    fs: &dyn Fs,
    os: Os,
    unc: &dyn UncResolver,
    remote: &RemoteConfig,
) -> Result<PathBuf, CoreError> {
    if remote.kind != RemoteKind::Folder {
        return Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{} is a {} remote", remote.name, remote.kind.as_str()),
        ));
    }
    let typed = remote.path.as_deref().ok_or_else(|| {
        CoreError::new(
            ErrorCode::MissingField,
            format!("remote {} has no path", remote.name),
        )
    })?;
    let root = resolve(os, unc, Path::new(typed));
    if !fs.is_dir(&root) {
        return Err(CoreError::new(
            ErrorCode::RemoteUnreachable,
            format!(
                "remote {} is not reachable (drive unplugged or share not mounted?); local work \
                 goes on and the next push tries again",
                remote.name
            ),
        )
        .with_path(&root));
    }
    Ok(root)
}

/// What a push wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushOutcome {
    /// This machine's subfolder.
    pub dir: PathBuf,
    pub mode: FolderMode,
    pub head: CommitId,
    /// The archive written (archive mode).
    pub archive: Option<String>,
}

/// Pushes the store to a folder remote, in its mode.
pub fn push(
    store: &Store<'_>,
    remote: &RemoteConfig,
    unc: &dyn UncResolver,
    app_version: &str,
) -> Result<PushOutcome, CoreError> {
    let fs = store.fs();
    let env = store.env();
    let root = reachable_root(fs, env.os(), unc, remote)?;
    let machine = Machine::load_or_create(env, fs, store.state_dir())?;
    let dir = root.join(store.store_id()).join(&machine.id);
    fs.create_dir_all(&dir)
        .map_err(|err| io("cannot create directory", &dir, &err))?;
    let mode = remote.folder_mode();
    let mut manifest = MachineManifest {
        schema: 1,
        store_id: store.store_id().to_owned(),
        machine_id: machine.id.clone(),
        label: machine.label.clone(),
        os: env.os().as_str().to_owned(),
        app_version: app_version.to_owned(),
        pushed_at: utc_datetime(env.now()).to_string(),
        head: None,
        tags: BTreeMap::new(),
        mirror: None,
        archives: Vec::new(),
    };
    let head = store.head()?;
    let mut archive = None;
    match mode {
        FolderMode::RepoPerMachine => {
            let (head, tags) = store.export_bare(&dir.join(STORE_GIT))?;
            manifest.head = Some(head.to_string());
            manifest.tags = tags;
        }
        FolderMode::Mirror => {
            write_mirror(store, &dir, &head, &machine.id)?;
            manifest.head = Some(head.to_string());
            manifest.mirror = Some(head.to_string());
        }
        FolderMode::Archive => {
            let keep = remote.keep.unwrap_or(DEFAULT_KEEP).max(1) as usize;
            let (name, kept) = write_archive(store, &dir, &head, &machine.id, keep)?;
            manifest.head = Some(head.to_string());
            manifest.archives = kept;
            archive = Some(name);
        }
    }
    manifest.write(fs, &dir)?;
    Ok(PushOutcome {
        dir,
        mode,
        head,
        archive,
    })
}

fn io(what: &str, path: &Path, err: &std::io::Error) -> CoreError {
    CoreError::new(ErrorCode::IoError, format!("{what}: {err}")).with_path(path)
}

/// A random suffix for temp names.
fn suffix() -> Result<String, CoreError> {
    let mut random = [0u8; 4];
    getrandom::fill(&mut random).map_err(|err| {
        CoreError::new(
            ErrorCode::IoError,
            format!("cannot read random bytes: {err}"),
        )
    })?;
    Ok(random.iter().map(|b| format!("{b:02x}")).collect())
}

fn export_info(store: &Store<'_>, head: &CommitId, machine_id: &str) -> Result<Vec<u8>, CoreError> {
    let info = ExportInfo {
        schema: 1,
        commit: head.to_string(),
        time: utc_datetime(store.env().now()).to_string(),
        machine: machine_id.to_owned(),
    };
    serde_json::to_vec_pretty(&info).map_err(|err| {
        CoreError::new(
            ErrorCode::InvalidValue,
            format!("cannot write export: {err}"),
        )
    })
}

/// `latest/`: the HEAD tree and `EXPORT.json`, written into a temp folder
/// and swapped in by renames.
fn write_mirror(
    store: &Store<'_>,
    dir: &Path,
    head: &CommitId,
    machine_id: &str,
) -> Result<(), CoreError> {
    let fs = store.fs();
    let tag = suffix()?;
    let temp = dir.join(format!(".{LATEST}.loadout-{tag}.tmp"));
    let mut files = store.files_at(head)?;
    files.insert(
        EXPORT_FILE.to_owned(),
        export_info(store, head, machine_id)?,
    );
    let written = files.iter().try_for_each(|(relative, bytes)| {
        let path = relative
            .split('/')
            .fold(temp.clone(), |path, part| path.join(part));
        if let Some(parent) = path.parent() {
            fs.create_dir_all(parent)
                .map_err(|err| io("cannot create directory", parent, &err))?;
        }
        fs.write(&path, bytes)
            .map_err(|err| io("cannot write file", &path, &err))
    });
    // Nothing of a failed push stays behind in the synced folder.
    let discard = || {
        let _ = fs.remove_dir_all(&temp);
    };
    if let Err(err) = written {
        discard();
        return Err(err);
    }
    let latest = dir.join(LATEST);
    let old = dir.join(format!(".{LATEST}.loadout-{tag}.old"));
    if fs.exists(&latest)
        && let Err(err) = fs.replace(&latest, &old)
    {
        discard();
        return Err(io("cannot move the old mirror", &latest, &err));
    }
    if let Err(err) = fs.replace(&temp, &latest) {
        if fs.exists(&old) {
            let _ = fs.replace(&old, &latest);
        }
        discard();
        return Err(io("cannot move the mirror in place", &latest, &err));
    }
    if fs.exists(&old) {
        let _ = fs.remove_dir_all(&old);
    }
    Ok(())
}

/// Whether `name` is an archive name this module writes.
fn is_archive_name(name: &str) -> bool {
    name.strip_prefix("loadout-")
        .and_then(|rest| rest.strip_suffix(".zip"))
        .is_some_and(|core| {
            let parts: Vec<&str> = core.split('-').collect();
            parts.len() == 3
                && parts[0].len() == 8
                && parts[1].len() == 6
                && parts[..2]
                    .iter()
                    .all(|p| p.bytes().all(|b| b.is_ascii_digit()))
                && !parts[2].is_empty()
                && parts[2].bytes().all(|b| b.is_ascii_hexdigit())
        })
}

/// A zip of `files`.
fn zip_bytes(files: &Files) -> Result<Vec<u8>, CoreError> {
    let zip_error = |err: zip::result::ZipError| {
        CoreError::new(
            ErrorCode::IoError,
            format!("cannot write the archive: {err}"),
        )
    };
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in files {
        writer
            .start_file(name.as_str(), options)
            .map_err(zip_error)?;
        writer
            .write_all(bytes)
            .map_err(|err| CoreError::new(ErrorCode::IoError, err.to_string()))?;
    }
    Ok(writer.finish().map_err(zip_error)?.into_inner())
}

/// The store at `rev` as a zip with `EXPORT.json` (what `restore --from`
/// reads, SPEC 10 item 5), and its name `loadout-<ts>-<shortsha>.zip`.
pub fn export_zip(
    store: &Store<'_>,
    rev: &CommitId,
    machine_id: &str,
) -> Result<(String, Vec<u8>), CoreError> {
    let stamp = crate::apply::backup::timestamp(store.env().now());
    // 20260101T000000Z -> 20260101-000000
    let stamp = format!("{}-{}", &stamp[..8], &stamp[9..15]);
    let name = format!("loadout-{stamp}-{}.zip", &rev.as_str()[..12]);
    let mut files = store.files_at(rev)?;
    files.insert(EXPORT_FILE.to_owned(), export_info(store, rev, machine_id)?);
    Ok((name, zip_bytes(&files)?))
}

/// `archives/loadout-<ts>-<shortsha>.zip`, then the oldest archives beyond
/// `keep` are deleted. Returns the new name and the archives kept.
fn write_archive(
    store: &Store<'_>,
    dir: &Path,
    head: &CommitId,
    machine_id: &str,
    keep: usize,
) -> Result<(String, Vec<String>), CoreError> {
    let fs = store.fs();
    let archives = dir.join(ARCHIVES);
    fs.create_dir_all(&archives)
        .map_err(|err| io("cannot create directory", &archives, &err))?;
    let (name, bytes) = export_zip(store, head, machine_id)?;
    let temp = archives.join(format!(".{name}.loadout-{}.tmp", suffix()?));
    fs.write_new(&temp, &bytes, None)
        .map_err(|err| io("cannot write the archive", &temp, &err))?;
    let path = archives.join(&name);
    fs.replace(&temp, &path).map_err(|err| {
        let _ = fs.remove_file(&temp);
        io("cannot move the archive in place", &path, &err)
    })?;
    let mut kept: Vec<String> = fs
        .read_dir(&archives)
        .map_err(|err| io("cannot list archives", &archives, &err))?
        .into_iter()
        .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
        .filter(|name| is_archive_name(name))
        .collect();
    kept.sort();
    while kept.len() > keep {
        let oldest = kept.remove(0);
        let _ = fs.remove_file(&archives.join(&oldest));
    }
    Ok((name, kept))
}

/// One machine of a folder remote, from its `machine.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineEntry {
    pub store_id: String,
    pub machine_id: String,
    pub dir: PathBuf,
    pub manifest: MachineManifest,
    /// The subject of the head commit, when the machine has `store.git`.
    pub subject: Option<String>,
}

impl MachineEntry {
    /// Whether the machine's latest push was `repo-per-machine`: a
    /// `store.git` left from before a switch to `mirror` or `archive` does
    /// not hold the head the manifest names.
    pub fn has_repo(&self, fs: &dyn Fs) -> bool {
        self.manifest.head.is_some()
            && self.manifest.mirror.is_none()
            && self.manifest.archives.is_empty()
            && fs.is_dir(&self.dir.join(STORE_GIT))
    }
}

/// What a folder remote holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub machines: Vec<MachineEntry>,
    /// Sync tool artifacts and unlisted refs, never read, never deleted.
    pub artifacts: Vec<Artifact>,
}

/// Lists the machines under `root` (of store `store_id`, or of every
/// store), reporting sync artifacts on the way.
pub fn list(fs: &dyn Fs, root: &Path, store_id: Option<&str>) -> Result<Listing, CoreError> {
    let mut listing = Listing::default();
    for store_dir in read_dir(fs, root)? {
        let name = file_name(&store_dir);
        if !fs.is_dir(&store_dir) || !ids::is_valid(IdKind::Store, &name) {
            report(&mut listing, &store_dir, &name);
            continue;
        }
        if store_id.is_some_and(|id| id != name) {
            continue;
        }
        // A folder a sync tool or its owner removes meanwhile is skipped.
        let Ok(machine_dirs) = fs.read_dir(&store_dir) else {
            continue;
        };
        for machine_dir in machine_dirs {
            let machine = file_name(&machine_dir);
            if !fs.is_dir(&machine_dir) || !ids::is_valid(IdKind::Machine, &machine) {
                report(&mut listing, &machine_dir, &machine);
                continue;
            }
            let manifest = MachineManifest::load(fs, &machine_dir)
                .filter(|m| m.machine_id == machine && m.store_id == name);
            scan_machine_dir(fs, &machine_dir, manifest.as_ref(), &mut listing)?;
            let Some(manifest) = manifest else {
                continue;
            };
            let subject = manifest
                .head
                .as_deref()
                .and_then(|head| bare_commit_subject(&machine_dir.join(STORE_GIT), head));
            listing.machines.push(MachineEntry {
                store_id: name.clone(),
                machine_id: machine,
                dir: machine_dir,
                manifest,
                subject,
            });
        }
    }
    Ok(listing)
}

fn read_dir(fs: &dyn Fs, dir: &Path) -> Result<Vec<PathBuf>, CoreError> {
    fs.read_dir(dir)
        .map_err(|err| io("cannot list directory", dir, &err))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn report(listing: &mut Listing, path: &Path, name: &str) {
    if let Some(tool) = artifact_tool(name)
        && !listing.artifacts.iter().any(|a| a.path == path)
    {
        listing.artifacts.push(Artifact {
            path: path.to_path_buf(),
            tool,
        });
    }
}

/// Reports artifacts anywhere in a machine subfolder, and refs in
/// `store.git` that `machine.json` does not list.
fn scan_machine_dir(
    fs: &dyn Fs,
    dir: &Path,
    manifest: Option<&MachineManifest>,
    listing: &mut Listing,
) -> Result<(), CoreError> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs.read_dir(&current) else {
            continue;
        };
        for path in entries {
            let name = file_name(&path);
            report(listing, &path, &name);
            if fs.is_dir(&path) {
                stack.push(path);
            }
        }
    }
    let refs = dir.join(STORE_GIT).join("refs");
    if !fs.is_dir(&refs) {
        return Ok(());
    }
    let mut stack = vec![refs.clone()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs.read_dir(&current) else {
            continue;
        };
        for path in entries {
            if fs.is_dir(&path) {
                stack.push(path);
                continue;
            }
            let Ok(relative) = path.strip_prefix(dir.join(STORE_GIT)) else {
                continue;
            };
            let refname = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let listed = manifest.is_some_and(|m| {
                refname == format!("refs/heads/{}", crate::store::BRANCH)
                    || refname
                        .strip_prefix("refs/tags/")
                        .is_some_and(|tag| m.tags.contains_key(tag))
            });
            if !listed && !listing.artifacts.iter().any(|a| a.path == path) {
                listing.artifacts.push(Artifact {
                    path,
                    tool: "unlisted ref",
                });
            }
        }
    }
    Ok(())
}

/// What merging one incoming head did: a sibling machine of a folder
/// remote, or a git remote's `main` (no machine).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiblingPull {
    pub machine_id: Option<String>,
    pub label: String,
    /// The merge, or why there was none (`remote_incomplete`).
    pub outcome: Result<MergeOutcome, CoreError>,
}

/// What a pull did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullReport {
    pub remote: String,
    /// `auto/pre-pull/<ts>`, made before the first change.
    pub tag: Option<String>,
    pub head_before: CommitId,
    pub head_after: CommitId,
    pub siblings: Vec<SiblingPull>,
    pub artifacts: Vec<Artifact>,
    /// Tags created from siblings.
    pub tags_imported: Vec<String>,
    /// Existing tags a git fetch moved (SPEC 9.6's `+refs/tags/*`); folder
    /// pulls never move a tag.
    pub tags_moved: Vec<String>,
}

impl PullReport {
    pub fn conflicts(&self) -> usize {
        self.siblings
            .iter()
            .map(|s| match &s.outcome {
                Ok(MergeOutcome::Conflicts { conflicts, .. }) => conflicts.len(),
                _ => 0,
            })
            .sum()
    }
}

/// Pulls every sibling machine of this store from a folder remote and
/// merges each head (SPEC 9.9). Conflicts go to
/// `state/merge/MERGE_STATE.json`; HEAD stays where it was for them.
pub fn pull(
    store: &Store<'_>,
    remote: &RemoteConfig,
    unc: &dyn UncResolver,
) -> Result<PullReport, CoreError> {
    let fs = store.fs();
    let root = reachable_root(fs, store.env().os(), unc, remote)?;
    let machine = Machine::load_or_create(store.env(), fs, store.state_dir())?;
    let listing = list(fs, &root, Some(store.store_id()))?;
    let head_before = store.head()?;
    let mut report = PullReport {
        remote: remote.name.clone(),
        tag: None,
        head_before: head_before.clone(),
        head_after: head_before,
        siblings: Vec::new(),
        artifacts: listing.artifacts,
        tags_imported: Vec::new(),
        tags_moved: Vec::new(),
    };
    let mut pending = Vec::new();
    let mut attempted = Vec::new();
    for sibling in listing.machines {
        if sibling.machine_id == machine.id || !sibling.has_repo(fs) {
            continue;
        }
        let source = format!("{}/{}", remote.name, sibling.machine_id);
        let head = sibling.manifest.head.clone().expect("has_repo checks it");
        let bare = sibling.dir.join(STORE_GIT);
        let mut ids: Vec<String> = vec![head.clone()];
        ids.extend(sibling.manifest.tags.values().cloned());
        let outcome = store.import_objects(&bare, &ids).and_then(|()| {
            report
                .tags_imported
                .extend(store.import_tags(&sibling.manifest.tags)?);
            let incoming = CommitId::from_text(&head)?;
            if report.tag.is_none() && !store.contains(&incoming) {
                report.tag = Some(store.create_auto_tag("pre-pull")?);
            }
            let label = format!("{}/{}", remote.name, sibling.manifest.label);
            store.merge_incoming(&incoming, &label)
        });
        // A sibling that did not merge this time (incomplete, or an
        // error) keeps the conflicts recorded for it before.
        if outcome.is_ok() {
            attempted.push(source.clone());
        }
        if let Ok(MergeOutcome::Conflicts { base, conflicts }) = &outcome {
            pending.push(PendingMerge {
                source,
                incoming: head.clone(),
                base: base.as_ref().map(ToString::to_string),
                conflicts: conflicts.iter().map(RecordedConflict::from).collect(),
            });
        }
        report.siblings.push(SiblingPull {
            machine_id: Some(sibling.machine_id),
            label: sibling.manifest.label,
            outcome,
        });
    }
    write_merge_state(fs, store.state_dir(), &attempted, &pending)?;
    report.head_after = store.head()?;
    Ok(report)
}

/// Where a restore takes a store from (`loadout restore --from`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A bare repository: its `main` (or the listed head) and tags.
    Repo {
        bare: PathBuf,
        head: String,
        tags: BTreeMap<String, String>,
    },
    /// An exported tree (a mirror's `latest/`).
    Mirror { dir: PathBuf },
    /// A zip export.
    Zip { file: PathBuf },
}

/// A path given to `restore --from`: one source, or a folder remote whose
/// machines the user picks from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceChoice {
    One {
        source: Source,
        /// The machine it comes from, when known.
        machine: Option<Box<MachineEntry>>,
    },
    /// More than one machine and no `--machine`.
    Choose(Vec<MachineEntry>),
}

/// Opens `path` for a restore: a zip, a bare repository, a mirror, a
/// machine subfolder, or a folder remote (`machine` picks by id or exact
/// label; an ambiguous label fails and names the ids).
pub fn open_source(
    fs: &dyn Fs,
    path: &Path,
    machine: Option<&str>,
) -> Result<SourceChoice, CoreError> {
    let not_store = || {
        CoreError::new(
            ErrorCode::NotFound,
            "no Loadout store there: expected a folder remote, a machine folder, a bare \
             repository, a mirror or a zip",
        )
        .with_path(path)
    };
    if !fs.exists(path) {
        return Err(
            CoreError::new(ErrorCode::RemoteUnreachable, "the path does not exist").with_path(path),
        );
    }
    if !fs.is_dir(path) {
        return if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
        {
            Ok(SourceChoice::One {
                source: Source::Zip {
                    file: path.to_path_buf(),
                },
                machine: None,
            })
        } else {
            Err(not_store())
        };
    }
    if fs.exists(&path.join("HEAD")) && fs.is_dir(&path.join("objects")) {
        let (head, tags) = crate::store::bare_refs(path).ok_or_else(not_store)?;
        return Ok(SourceChoice::One {
            source: Source::Repo {
                bare: path.to_path_buf(),
                head,
                tags,
            },
            machine: None,
        });
    }
    if fs.exists(&path.join(EXPORT_FILE)) {
        return Ok(SourceChoice::One {
            source: Source::Mirror {
                dir: path.to_path_buf(),
            },
            machine: None,
        });
    }
    if let Some(manifest) = MachineManifest::load(fs, path) {
        let entry = MachineEntry {
            store_id: manifest.store_id.clone(),
            machine_id: manifest.machine_id.clone(),
            dir: path.to_path_buf(),
            subject: None,
            manifest,
        };
        let source = machine_source(fs, &entry).ok_or_else(not_store)?;
        return Ok(SourceChoice::One {
            source,
            machine: Some(Box::new(entry)),
        });
    }
    // A store folder (`<root>/<store_id>`): its machines.
    let name = file_name(path);
    let listing = match path.parent() {
        Some(parent) if ids::is_valid(IdKind::Store, &name) => list(fs, parent, Some(&name))?,
        _ => list(fs, path, None)?,
    };
    let mut candidates = listing.machines;
    if let Some(wanted) = machine {
        let by_id: Vec<MachineEntry> = candidates
            .iter()
            .filter(|m| m.machine_id.eq_ignore_ascii_case(wanted))
            .cloned()
            .collect();
        candidates = if by_id.is_empty() {
            candidates
                .into_iter()
                .filter(|m| m.manifest.label == wanted)
                .collect()
        } else {
            by_id
        };
        match candidates.len() {
            0 => {
                return Err(CoreError::new(
                    ErrorCode::NotFound,
                    format!("no machine {wanted:?} in that folder remote"),
                ));
            }
            1 => {}
            _ => {
                let ids: Vec<&str> = candidates.iter().map(|m| m.machine_id.as_str()).collect();
                return Err(CoreError::new(
                    ErrorCode::InvalidValue,
                    format!(
                        "several machines are labeled {wanted:?}; pick one by id: {}",
                        ids.join(", ")
                    ),
                ));
            }
        }
    }
    match candidates.len() {
        0 => Err(not_store()),
        1 => {
            let entry = candidates.remove(0);
            let source = machine_source(fs, &entry).ok_or_else(not_store)?;
            Ok(SourceChoice::One {
                source,
                machine: Some(Box::new(entry)),
            })
        }
        _ => Ok(SourceChoice::Choose(candidates)),
    }
}

/// The source a machine subfolder offers: its repository, else its mirror,
/// else its newest listed archive.
fn machine_source(fs: &dyn Fs, entry: &MachineEntry) -> Option<Source> {
    if entry.has_repo(fs) {
        return Some(Source::Repo {
            bare: entry.dir.join(STORE_GIT),
            head: entry.manifest.head.clone()?,
            tags: entry.manifest.tags.clone(),
        });
    }
    if entry.manifest.mirror.is_some() && fs.is_dir(&entry.dir.join(LATEST)) {
        return Some(Source::Mirror {
            dir: entry.dir.join(LATEST),
        });
    }
    let newest = entry
        .manifest
        .archives
        .iter()
        .filter(|name| is_archive_name(name))
        .max()?;
    let file = entry.dir.join(ARCHIVES).join(newest);
    fs.exists(&file).then_some(Source::Zip { file })
}

/// The store files of an exported tree (mirror or zip), without
/// `EXPORT.json`, and the sync tool artifacts skipped on the way (a
/// `desktop.ini` or a conflicted copy in `latest/` is not a store file).
/// Paths that are not plain store-relative paths (absolute, `..`, `.git`)
/// are refused (zip-slip).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Export {
    pub files: Files,
    pub artifacts: Vec<Artifact>,
}

pub fn export_files(fs: &dyn Fs, source: &Source) -> Result<Export, CoreError> {
    let mut export = Export::default();
    let mut add = |name: String, bytes: Vec<u8>, origin: PathBuf| -> Result<(), CoreError> {
        let name = name.replace('\\', "/");
        if name == EXPORT_FILE || name.ends_with('/') {
            return Ok(());
        }
        if let Some(tool) = name.split('/').find_map(artifact_tool) {
            export.artifacts.push(Artifact { path: origin, tool });
            return Ok(());
        }
        let plain = !name.is_empty()
            && !name.starts_with('/')
            && !name.contains(':')
            && name
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".." && part != ".git");
        if !plain {
            return Err(CoreError::new(
                ErrorCode::InvalidPath,
                format!("the export holds a path outside the store: {name:?}"),
            ));
        }
        export.files.insert(name, bytes);
        Ok(())
    };
    match source {
        Source::Mirror { dir } => {
            let mut stack = vec![dir.clone()];
            while let Some(current) = stack.pop() {
                for path in read_dir(fs, &current)? {
                    if fs.is_dir(&path) {
                        stack.push(path);
                        continue;
                    }
                    let relative = path
                        .strip_prefix(dir)
                        .expect("below the mirror")
                        .components()
                        .map(|part| part.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/");
                    let bytes = fs
                        .read(&path)
                        .map_err(|err| io("cannot read file", &path, &err))?;
                    add(relative, bytes, path)?;
                }
            }
        }
        Source::Zip { file } => {
            let bytes = fs
                .read(file)
                .map_err(|err| io("cannot read file", file, &err))?;
            let zip_error = |err: zip::result::ZipError| {
                CoreError::new(ErrorCode::ParseError, format!("not a readable zip: {err}"))
                    .with_path(file)
            };
            let mut archive =
                zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(zip_error)?;
            for index in 0..archive.len() {
                let mut entry = archive.by_index(index).map_err(zip_error)?;
                if entry.is_dir() {
                    continue;
                }
                let name = entry.name().map_err(zip_error)?.into_owned();
                let mut content = Vec::new();
                entry
                    .read_to_end(&mut content)
                    .map_err(|err| io("cannot read the zip", file, &err))?;
                let origin = file.join(&name);
                add(name, content, origin)?;
            }
        }
        Source::Repo { .. } => {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                "a repository is restored with its history, not as files",
            ));
        }
    }
    Ok(export)
}

//! Skills (SPEC 12): the store copy of each skill (`store/skills/<id>/`
//! plus `store/skill-meta/<id>.toml`), installs from git or local sources,
//! links and copies in client skill directories (table 7.6), the
//! classification of those directories, and the read-only scan of the
//! agents CLI's `~/.agents/skills`.
//!
//! Store changes are one commit each; links and copies follow right after
//! the commit ([`link::sync`], instant apply) under the store lock, owned
//! through the machine-local ledger.
//!
//! Links and archives need file system operations [`Fs`] does not have
//! (link creation, link metadata, renames). They use `std::fs` directly,
//! after [`confine`] checks the path against the view's root, so a test
//! with a rooted view still cannot reach outside its temp root.

pub mod agents_cli;
pub mod classify;
pub mod frontmatter;
pub mod install;
pub mod link;
pub mod store;

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::adapters::detect::RegistryProbe;
use crate::env::Env;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::paths::{LoadoutHome, normalize};
use crate::remote::git::Runner;

pub use install::{InstallRequest, Installed, Naming, Source, UpdateCheck, UpdateStatus, Updated};
pub use link::{LinkKind, LinkResult, LinkStatus, SkillLedger, SkillLink, SyncReport};
pub use store::StoreSkill;

/// The skill's entry file.
pub const SKILL_FILE: &str = "SKILL.md";
/// Marker file in a copy Loadout made (SPEC 12.2); never part of a skill.
pub const MARKER_FILE: &str = ".loadout-managed";
/// Largest skill (SPEC 12.1).
pub const MAX_SKILL_BYTES: u64 = 100 * 1024 * 1024;
/// Total store skill size that raises a warning (SPEC 12.1).
pub const WARN_TOTAL_BYTES: u64 = 500 * 1024 * 1024;
/// Most files in a `.zip` or `.skill` archive (SPEC 17).
pub const MAX_ARCHIVE_FILES: usize = 500;
/// Largest file in an archive (SPEC 17).
pub const MAX_ARCHIVE_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// Largest total of an archive's files (SPEC 17).
pub const MAX_ARCHIVE_TOTAL_BYTES: u64 = 20 * 1024 * 1024;

/// A skill's files: `/`-separated relative path to bytes.
pub type SkillFiles = BTreeMap<String, Vec<u8>>;

/// What skill operations run against.
#[derive(Clone, Copy)]
pub struct SkillsEnv<'a> {
    pub env: &'a Env,
    pub fs: &'a dyn Fs,
    pub home: &'a LoadoutHome,
    /// Detection for `all-detected` client selections.
    pub registry: &'a dyn RegistryProbe,
    /// Runs the system git (git sources) and the agents `skills` CLI.
    pub runner: &'a dyn Runner,
    /// How links are made on Windows: a junction (default) or a directory
    /// symbolic link (needs Developer Mode or admin rights). Ignored
    /// elsewhere, where links are symbolic links.
    pub link_kind: LinkKind,
}

impl std::fmt::Debug for SkillsEnv<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillsEnv")
            .field("home", &self.home.path)
            .field("link_kind", &self.link_kind)
            .finish_non_exhaustive()
    }
}

/// `invalid_path` when `path` lies outside the view `fs` is confined to.
/// Every direct `std::fs` call in this module checks its paths here first.
pub(crate) fn confine(fs: &dyn Fs, path: &Path) -> Result<(), CoreError> {
    let Some(root) = fs.root() else {
        return Ok(());
    };
    if path.is_absolute() && normalize(path).starts_with(root) {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorCode::InvalidPath,
            "the path is outside the file system view",
        )
        .with_path(path))
    }
}

pub(crate) fn io_error(what: &str, path: &Path, err: &io::Error) -> CoreError {
    CoreError::new(ErrorCode::IoError, format!("{what}: {err}")).with_path(path)
}

/// What is at a path, without following a final link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Entry {
    Missing,
    /// A symbolic link or junction, with its target as stored.
    Link(PathBuf),
    Dir,
    File,
}

pub(crate) fn entry(fs: &dyn Fs, path: &Path) -> Result<Entry, CoreError> {
    confine(fs, path)?;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Entry::Missing),
        Err(err) => return Err(io_error("cannot read file metadata", path, &err)),
    };
    // std counts junctions as links (name-surrogate reparse points), and
    // `read_link` reads their target.
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(path)
            .map_err(|err| io_error("cannot read the link target", path, &err))?;
        return Ok(Entry::Link(plain_target(path, target)));
    }
    Ok(if meta.is_dir() {
        Entry::Dir
    } else {
        Entry::File
    })
}

/// A link target as a plain absolute path: Windows junction targets come
/// back verbatim (`\\?\C:\...`, `\\?\UNC\server\...`), and relative
/// symbolic link targets are relative to the link's folder.
fn plain_target(link: &Path, target: PathBuf) -> PathBuf {
    let text = target.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\")
        && rest.as_bytes().get(1) == Some(&b':')
    {
        return PathBuf::from(rest);
    }
    if target.is_relative()
        && let Some(parent) = link.parent()
    {
        return normalize(&parent.join(&target));
    }
    target
}

/// Reads a skill folder: every file below `dir`, except nested `.git`
/// folders and files (SPEC 12.1) and [`MARKER_FILE`]. A link (file or
/// folder) is followed only when it resolves inside `dir`; one that
/// escapes is `invalid_path` (SPEC 17). More than [`MAX_SKILL_BYTES`] is
/// `invalid_value`.
pub fn read_folder(fs: &dyn Fs, dir: &Path) -> Result<SkillFiles, CoreError> {
    confine(fs, dir)?;
    let root = fs
        .canonicalize(dir)
        .map_err(|err| io_error("cannot resolve folder", dir, &err))?;
    let mut files = SkillFiles::new();
    let mut total = 0u64;
    let mut visited = vec![root.clone()];
    walk(fs, &root, dir, "", &mut files, &mut total, &mut visited)?;
    Ok(files)
}

/// Whether a `.git` folder or file is anywhere below `dir` (links are not
/// followed). [`content_hash`] leaves `.git` out, so a copy the user turned
/// into a repository would otherwise look unchanged.
pub(crate) fn has_nested_git(fs: &dyn Fs, dir: &Path) -> Result<bool, CoreError> {
    let entries = fs
        .read_dir(dir)
        .map_err(|err| io_error("cannot list directory", dir, &err))?;
    for path in entries {
        if path
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case(".git"))
        {
            return Ok(true);
        }
        if entry(fs, &path)? == Entry::Dir && has_nested_git(fs, &path)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn walk(
    fs: &dyn Fs,
    root: &Path,
    dir: &Path,
    prefix: &str,
    files: &mut SkillFiles,
    total: &mut u64,
    visited: &mut Vec<PathBuf>,
) -> Result<(), CoreError> {
    let entries = fs
        .read_dir(dir)
        .map_err(|err| io_error("cannot list directory", dir, &err))?;
    for path in entries {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Err(CoreError::new(
                ErrorCode::InvalidPath,
                "a file name in the skill is not valid Unicode",
            )
            .with_path(&path));
        };
        if name.eq_ignore_ascii_case(".git") || (prefix.is_empty() && name == MARKER_FILE) {
            continue;
        }
        check_name(name, &path)?;
        let relative = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        let is_dir = match entry(fs, &path)? {
            Entry::Missing => continue,
            Entry::Link(_) => {
                let resolved = fs.canonicalize(&path).map_err(|err| {
                    CoreError::new(
                        ErrorCode::InvalidPath,
                        format!("a link in the skill cannot be resolved: {err}"),
                    )
                    .with_path(&path)
                })?;
                if !resolved.starts_with(root) {
                    return Err(CoreError::new(
                        ErrorCode::InvalidPath,
                        "a link in the skill points outside the skill folder",
                    )
                    .with_path(&path));
                }
                let is_dir = fs.is_dir(&path);
                if is_dir {
                    if visited.contains(&resolved) {
                        continue;
                    }
                    visited.push(resolved);
                }
                is_dir
            }
            Entry::Dir => true,
            Entry::File => false,
        };
        if is_dir {
            walk(fs, root, &path, &relative, files, total, visited)?;
        } else {
            // The size first: a huge file is refused before it is read.
            let len = std::fs::metadata(&path)
                .map_err(|err| io_error("cannot read file metadata", &path, &err))?
                .len();
            if *total + len > MAX_SKILL_BYTES {
                return Err(too_large(&path));
            }
            let bytes = fs
                .read(&path)
                .map_err(|err| io_error("cannot read file", &path, &err))?;
            *total += bytes.len() as u64;
            if *total > MAX_SKILL_BYTES {
                return Err(too_large(&path));
            }
            files.insert(relative, bytes);
        }
    }
    Ok(())
}

pub(crate) fn too_large(path: &Path) -> CoreError {
    CoreError::new(
        ErrorCode::InvalidValue,
        format!(
            "the skill is larger than {} MB",
            MAX_SKILL_BYTES / (1024 * 1024)
        ),
    )
    .with_path(path)
}

/// A file name the store can hold on every OS (no `\`, `:`, `.` or `..`).
pub(crate) fn check_name(name: &str, path: &Path) -> Result<(), CoreError> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['\\', ':', '/']) {
        return Err(CoreError::new(
            ErrorCode::InvalidPath,
            format!("{name:?} cannot be stored as a skill file name"),
        )
        .with_path(path));
    }
    Ok(())
}

/// `sha256:<hex>` over a skill's files (paths and bytes, sorted), the
/// content hash of copies in the ledger.
pub fn content_hash(files: &SkillFiles) -> String {
    let mut hasher = Sha256::new();
    for (path, bytes) in files {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    let digest = hasher.finalize();
    let mut text = String::with_capacity(7 + digest.len() * 2);
    text.push_str("sha256:");
    for byte in digest {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// Total size of a skill's files.
pub fn files_size(files: &SkillFiles) -> u64 {
    files.values().map(|bytes| bytes.len() as u64).sum()
}

/// Writes `files` below `dir`, creating folders.
pub(crate) fn write_files(fs: &dyn Fs, dir: &Path, files: &SkillFiles) -> Result<(), CoreError> {
    for (relative, bytes) in files {
        let path = relative
            .split('/')
            .fold(dir.to_path_buf(), |path, part| path.join(part));
        if let Some(parent) = path.parent() {
            fs.create_dir_all(parent)
                .map_err(|err| io_error("cannot create directory", parent, &err))?;
        }
        fs.write(&path, bytes)
            .map_err(|err| io_error("cannot write file", &path, &err))?;
    }
    Ok(())
}

/// A lower-case id from a skill name: runs of other characters become
/// `-`; `None` when nothing is left.
pub fn slug(name: &str) -> Option<String> {
    let mut slug = String::new();
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '.' || ch == '_' {
            slug.push(ch);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let mut slug = slug
        .trim_start_matches(['.', '_', '-'])
        .trim_end_matches('-')
        .to_owned();
    slug.truncate(crate::model::ids::MAX_ID_LEN - 4);
    let slug = slug.trim_end_matches('-').to_owned();
    (!slug.is_empty()).then_some(slug)
}

/// Removes skill `id` from the store (one commit: `skill: remove <id>`),
/// its cache clone, then its owned unchanged links and copies
/// ([`link::sync`]). Changed copies stay in place, no longer owned.
/// Project manifests that still enable it get a warning on sync.
pub fn remove(
    store: &crate::store::Store<'_>,
    sx: &SkillsEnv<'_>,
    id: &str,
) -> Result<(Option<crate::store::CommitId>, SyncReport), CoreError> {
    crate::model::ids::check(crate::model::ids::IdKind::Skill, id)?;
    let operation = crate::store::Operation::new("skill.remove", format!("skill: remove {id}"))
        .entity(format!("skill/{id}"));
    let committed = store.transaction(operation, |tx| {
        if !store::exists(tx, id)? {
            return Err(store::not_found(id));
        }
        store::stage_remove(tx, id)
    })?;
    store::prune_empty_dirs(store, id)?;
    install::forget_source(sx, id)?;
    let report = link::sync(store, sx)?;
    Ok((committed.commit, report))
}

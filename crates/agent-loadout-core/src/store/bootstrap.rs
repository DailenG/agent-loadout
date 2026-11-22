//! Moving store history to and from other repositories without a git
//! transport (SPEC 4.2: git2 local operations only):
//!
//! - [`Store::export_bare`]: HEAD and every tag into a bare repository (a
//!   folder remote's `store.git`), objects first, refs after.
//! - [`Store::import_history`]: the objects reachable from given ids, and
//!   the given tags, out of a bare repository; a missing object is
//!   `remote_incomplete`.
//! - [`Store::init_from_bare`] and [`Store::init_from_files`]: a new store
//!   on a new machine from another machine's history, or from an exported
//!   tree (mirror, zip).
//!
//! Objects are read and written through the object databases directly, so
//! only the ids a manifest lists are ever followed, never a ref a sync
//! tool may have duplicated.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use git2::{ObjectType, Oid, Repository, RepositoryInitOptions};

use crate::env::Env;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::TomlDocument;
use crate::model::ids::{self, IdKind};
use crate::paths::LoadoutHome;
use crate::store::merge::Files;
use crate::store::transaction::Changes;
use crate::store::{
    BRANCH, CommitId, IDENTITY_FILE, LOCK_TIMEOUT, Machine, Operation, Store, StoreIdentity,
    StoreLock, git_error, io_error,
};

/// Copies every object reachable from `start` that `dst` lacks. A missing
/// source object fails with `missing` (its id).
fn copy_reachable(src: &Repository, dst: &Repository, start: &[Oid]) -> Result<(), Oid> {
    let src_odb = src.odb().map_err(|_| Oid::ZERO_SHA1)?;
    let dst_odb = dst.odb().map_err(|_| Oid::ZERO_SHA1)?;
    let mut seen: HashSet<Oid> = HashSet::new();
    let mut stack: Vec<Oid> = start.to_vec();
    while let Some(oid) = stack.pop() {
        if !seen.insert(oid) {
            continue;
        }
        let object = src_odb.read(oid).map_err(|_| oid)?;
        let kind = object.kind();
        if !dst_odb.exists(oid) {
            dst_odb.write(kind, object.data()).map_err(|_| oid)?;
        }
        match kind {
            ObjectType::Commit => {
                let commit = src.find_commit(oid).map_err(|_| oid)?;
                stack.push(commit.tree_id());
                stack.extend(commit.parent_ids());
            }
            ObjectType::Tree => {
                let tree = src.find_tree(oid).map_err(|_| oid)?;
                for entry in tree.iter() {
                    if matches!(entry.kind(), Some(ObjectType::Tree | ObjectType::Blob)) {
                        stack.push(entry.id());
                    }
                }
            }
            ObjectType::Tag => {
                let tag = src.find_tag(oid).map_err(|_| oid)?;
                stack.push(tag.target_id());
            }
            _ => {}
        }
    }
    Ok(())
}

fn open_bare(path: &Path) -> Result<Repository, CoreError> {
    Repository::open_bare(path).map_err(|err| {
        CoreError::new(
            ErrorCode::RemoteIncomplete,
            format!("cannot open the repository: {}", err.message()),
        )
        .with_path(path)
    })
}

fn parse_oid(text: &str) -> Result<Oid, CoreError> {
    Oid::from_str(text).map_err(|_| {
        CoreError::new(
            ErrorCode::InvalidValue,
            format!("{text:?} is not a commit id"),
        )
    })
}

fn incomplete(path: &Path, missing: Oid) -> CoreError {
    CoreError::new(
        ErrorCode::RemoteIncomplete,
        format!(
            "object {missing} has not arrived yet (a sync tool may still be copying); try again later"
        ),
    )
    .with_path(path)
}

/// The subject of commit `oid` in the bare repository at `path`, if it is
/// readable.
pub fn bare_commit_subject(path: &Path, oid: &str) -> Option<String> {
    let repo = Repository::open_bare(path).ok()?;
    let commit = repo.find_commit(Oid::from_str(oid).ok()?).ok()?;
    commit.summary().ok().flatten().map(str::to_owned)
}

/// `main` and the tags (`name -> id`) of the bare repository at `path`,
/// for a restore from a repository given directly (no `machine.json`).
pub fn bare_refs(path: &Path) -> Option<(String, BTreeMap<String, String>)> {
    let repo = Repository::open_bare(path).ok()?;
    let head = repo
        .find_reference(&format!("refs/heads/{BRANCH}"))
        .ok()?
        .target()?
        .to_string();
    let mut tags = BTreeMap::new();
    for name in repo.tag_names(None).ok()?.iter().flatten().flatten() {
        if let Some(oid) = repo
            .find_reference(&format!("refs/tags/{name}"))
            .ok()
            .and_then(|reference| reference.target())
        {
            tags.insert(name.to_owned(), oid.to_string());
        }
    }
    Some((head, tags))
}

/// The store id in `loadout.toml` of commit `head` in the bare repository
/// at `path`, if readable.
pub fn bare_store_id(path: &Path, head: &str) -> Option<String> {
    let repo = Repository::open_bare(path).ok()?;
    identity_at(&repo, Oid::from_str(head).ok()?)
        .ok()
        .map(|identity| identity.store_id)
}

impl Store<'_> {
    /// How many commits reachable from `head` in the bare repository at
    /// `path` are not in HEAD's history (at most `limit`); `None` when the
    /// repository or a commit cannot be read (a sync tool may still be
    /// copying). Reads the bare repository only.
    pub fn bare_incoming(&self, path: &Path, head: &str, limit: usize) -> Option<usize> {
        let repo = Repository::open_bare(path).ok()?;
        let mut pending = vec![Oid::from_str(head).ok()?];
        let mut seen = HashSet::new();
        let mut count = 0;
        while let Some(oid) = pending.pop() {
            if count >= limit {
                break;
            }
            if !seen.insert(oid) || self.contains(&CommitId::from_oid(oid)) {
                continue;
            }
            count += 1;
            pending.extend(repo.find_commit(oid).ok()?.parent_ids());
        }
        Some(count)
    }
}

/// The store id in an exported tree's `loadout.toml`.
pub fn export_store_id(files: &Files) -> Option<String> {
    let text = std::str::from_utf8(files.get(IDENTITY_FILE)?).ok()?;
    let identity = TomlDocument::<StoreIdentity>::parse(text)
        .ok()?
        .into_value();
    Some(identity.store_id)
}

impl Store<'_> {
    /// Writes HEAD and every tag into the bare repository `path`, created
    /// when absent: objects first, then `main` and the tags; tags the store
    /// no longer has are deleted (single writer). Returns HEAD and the tags
    /// as `name -> id`.
    pub(crate) fn export_bare(
        &self,
        path: &Path,
    ) -> Result<(CommitId, BTreeMap<String, String>), CoreError> {
        let bare = if self.fs.exists(&path.join("HEAD")) {
            Repository::open_bare(path).map_err(git_error)?
        } else {
            self.fs
                .create_dir_all(path)
                .map_err(|err| io_error("cannot create directory", path, &err))?;
            let mut options = RepositoryInitOptions::new();
            options
                .bare(true)
                .external_template(false)
                .initial_head(BRANCH);
            Repository::init_opts(path, &options).map_err(git_error)?
        };
        let head = self.head()?;
        let head_oid = parse_oid(head.as_str())?;
        let mut tags = BTreeMap::new();
        let mut starts = vec![head_oid];
        for name in self
            .repo
            .tag_names(None)
            .map_err(git_error)?
            .iter()
            .flatten()
            .flatten()
        {
            if let Ok(reference) = self.repo.find_reference(&format!("refs/tags/{name}"))
                && let Some(oid) = reference.target()
            {
                tags.insert(name.to_owned(), oid.to_string());
                starts.push(oid);
            }
        }
        copy_reachable(&self.repo, &bare, &starts).map_err(|missing| {
            CoreError::new(
                ErrorCode::GitError,
                format!("object {missing} is missing from the store"),
            )
        })?;
        bare.reference(&format!("refs/heads/{BRANCH}"), head_oid, true, "push")
            .map_err(git_error)?;
        for (name, oid) in &tags {
            bare.reference(&format!("refs/tags/{name}"), parse_oid(oid)?, true, "push")
                .map_err(git_error)?;
        }
        let stale: Vec<String> = bare
            .tag_names(None)
            .map_err(git_error)?
            .iter()
            .flatten()
            .flatten()
            .filter(|name| !tags.contains_key(*name))
            .map(str::to_owned)
            .collect();
        for name in stale {
            bare.tag_delete(&name).map_err(git_error)?;
        }
        Ok((head, tags))
    }

    /// Copies the objects reachable from `ids` out of the bare repository
    /// `path`. Fails with `remote_incomplete` when one has not arrived.
    pub(crate) fn import_objects(&self, path: &Path, ids: &[String]) -> Result<(), CoreError> {
        let bare = open_bare(path)?;
        let starts = ids
            .iter()
            .map(|id| parse_oid(id))
            .collect::<Result<Vec<_>, _>>()?;
        copy_reachable(&bare, &self.repo, &starts).map_err(|missing| incomplete(path, missing))
    }

    /// The history behind `ids` and the tags of `tags` out of the bare
    /// repository `path` (`remote_incomplete` when an object is missing).
    /// Returns the tags created.
    pub fn import_history(
        &self,
        path: &Path,
        ids: &[String],
        tags: &BTreeMap<String, String>,
    ) -> Result<Vec<String>, CoreError> {
        self.import_objects(path, ids)?;
        self.import_tags(tags)
    }

    /// Creates the tags of `tags` (`name -> id`) the store lacks; existing
    /// names are left as they are. Returns the names created.
    pub(crate) fn import_tags(
        &self,
        tags: &BTreeMap<String, String>,
    ) -> Result<Vec<String>, CoreError> {
        let mut created = Vec::new();
        for (name, oid) in tags {
            let refname = format!("refs/tags/{name}");
            if !git2::Reference::is_valid_name(&refname) || self.tag_exists(name) {
                continue;
            }
            let oid = parse_oid(oid)?;
            if self.repo.find_object(oid, None).is_err() {
                continue;
            }
            self.repo
                .reference(&refname, oid, false, "pull")
                .map_err(git_error)?;
            created.push(name.clone());
        }
        Ok(created)
    }

    /// A new store in `home` holding the history of `head` from the bare
    /// repository `bare` (another machine's `store.git`). The working tree
    /// is checked out; this machine gets its own identity. On failure
    /// nothing is left behind, so a retry (after `remote_incomplete`)
    /// starts over.
    pub fn init_from_bare<'a>(
        env: &Env,
        fs: &'a dyn Fs,
        home: &LoadoutHome,
        client: &str,
        bare: &Path,
        head: &str,
        tags: &BTreeMap<String, String>,
    ) -> Result<Store<'a>, CoreError> {
        let source = open_bare(bare)?;
        let head_oid = parse_oid(head)?;
        let (repo, state_dir, lock) = create_repo(fs, home)?;
        let built = (|| {
            let mut starts = vec![head_oid];
            starts.extend(tags.values().filter_map(|oid| Oid::from_str(oid).ok()));
            copy_reachable(&source, &repo, &starts).map_err(|missing| incomplete(bare, missing))?;
            // Not a Loadout store: refused before any ref points at it.
            let identity = identity_at(&repo, head_oid)?;
            repo.reference(&format!("refs/heads/{BRANCH}"), head_oid, true, "restore")
                .map_err(git_error)?;
            Machine::load_or_create(env, fs, &state_dir)?;
            let store = Store::from_parts(env, fs, home, client, repo, identity)?;
            store.import_tags(tags)?;
            let files = store.files_at(&CommitId::from_oid(head_oid))?;
            let changes: Changes = files.into_iter().map(|(k, v)| (k, Some(v))).collect();
            store.write_worktree(&changes)?;
            {
                let tree = store
                    .repo
                    .find_commit(head_oid)
                    .and_then(|commit| commit.tree())
                    .map_err(git_error)?;
                let mut index = store.repo.index().map_err(git_error)?;
                index.read_tree(&tree).map_err(git_error)?;
                index.write().map_err(git_error)?;
            }
            Ok(store)
        })();
        if built.is_err() {
            discard_repo(fs, home);
        }
        drop(lock);
        built
    }

    /// A new store in `home` whose first commit holds `files` (an exported
    /// tree: a mirror or a zip). The store identity is the one in `files`.
    /// On failure (validation, the leak scan) nothing is left behind.
    pub fn init_from_files<'a>(
        env: &Env,
        fs: &'a dyn Fs,
        home: &LoadoutHome,
        client: &str,
        files: Files,
        summary: &str,
    ) -> Result<Store<'a>, CoreError> {
        let text = files
            .get(IDENTITY_FILE)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .ok_or_else(|| {
                CoreError::new(
                    ErrorCode::MissingField,
                    format!("the export has no {IDENTITY_FILE}, so it is not a Loadout store"),
                )
            })?;
        let identity = TomlDocument::<StoreIdentity>::parse(text)
            .map_err(|diagnostic| diagnostic.to_core_error(Some(Path::new(IDENTITY_FILE))))?
            .into_value();
        ids::check(IdKind::Store, &identity.store_id)?;
        let (repo, state_dir, lock) = create_repo(fs, home)?;
        let built = (|| {
            let machine = Machine::load_or_create(env, fs, &state_dir)?;
            let store = Store::from_parts(env, fs, home, client, repo, identity)?;
            let changes: Changes = files.into_iter().map(|(k, v)| (k, Some(v))).collect();
            store.commit_changes(&machine, &Operation::new("store.restore", summary), changes)?;
            Ok(store)
        })();
        if built.is_err() {
            discard_repo(fs, home);
        }
        drop(lock);
        built
    }
}

/// An empty, non-bare repository on `main` in `home`'s store folder,
/// with the lock held. Fails with `already_exists` when a store is there,
/// or when the folder holds other files. A repository without any ref (a
/// bootstrap that was cut off) is started over.
fn create_repo(
    fs: &dyn Fs,
    home: &LoadoutHome,
) -> Result<(Repository, std::path::PathBuf, StoreLock), CoreError> {
    let state_dir = home.state_dir();
    let lock = StoreLock::acquire(fs, &state_dir, LOCK_TIMEOUT)?;
    let root = home.store_dir();
    let dot_git = root.join(".git");
    if fs.exists(&dot_git) {
        let unborn = Repository::open(&root).is_ok_and(|repo| {
            repo.head().is_err()
                && repo
                    .references()
                    .is_ok_and(|mut refs| refs.next().is_none())
        });
        if !unborn {
            return Err(
                CoreError::new(ErrorCode::AlreadyExists, "a store exists here already")
                    .with_path(&root),
            );
        }
        fs.remove_dir_all(&dot_git)
            .map_err(|err| io_error("cannot remove directory", &dot_git, &err))?;
    }
    if fs.read_dir(&root).is_ok_and(|entries| !entries.is_empty()) {
        return Err(CoreError::new(
            ErrorCode::AlreadyExists,
            "the store folder holds files but no store; move them away first",
        )
        .with_path(&root));
    }
    fs.create_dir_all(&root)
        .map_err(|err| io_error("cannot create directory", &root, &err))?;
    let mut options = RepositoryInitOptions::new();
    options
        .bare(false)
        .no_reinit(true)
        .mkdir(true)
        .external_template(false)
        .initial_head(BRANCH);
    let repo = Repository::init_opts(&root, &options).map_err(git_error)?;
    Ok((repo, state_dir, lock))
}

/// Removes the store folder a failed bootstrap filled. `create_repo` only
/// proceeds into an absent or empty folder, so nothing else is in it.
fn discard_repo(fs: &dyn Fs, home: &LoadoutHome) {
    let _ = fs.remove_dir_all(&home.store_dir());
}

/// The store identity in `loadout.toml` of commit `oid`.
fn identity_at(repo: &Repository, oid: Oid) -> Result<StoreIdentity, CoreError> {
    let tree = repo
        .find_commit(oid)
        .and_then(|commit| commit.tree())
        .map_err(git_error)?;
    let entry = tree.get_path(Path::new(IDENTITY_FILE)).map_err(|_| {
        CoreError::new(
            ErrorCode::MissingField,
            format!("the history has no {IDENTITY_FILE}, so it is not a Loadout store"),
        )
    })?;
    let blob = repo.find_blob(entry.id()).map_err(git_error)?;
    let text = std::str::from_utf8(blob.content())
        .map_err(|_| CoreError::new(ErrorCode::ParseError, "loadout.toml is not UTF-8"))?;
    let identity = TomlDocument::<StoreIdentity>::parse(text)
        .map_err(|diagnostic| diagnostic.to_core_error(Some(Path::new(IDENTITY_FILE))))?
        .into_value();
    ids::check(IdKind::Store, &identity.store_id)?;
    Ok(identity)
}

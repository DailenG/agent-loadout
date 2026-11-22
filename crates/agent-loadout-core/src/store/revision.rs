//! Reading past revisions of the store: file contents at a commit, the
//! files two commits differ in, first parents and tags. Used by rollback
//! (SPEC 8.7); `restore` and history views build on the same calls.

use std::path::Path;

use git2::{DiffOptions, ObjectType, Oid, Tree};

use crate::error::{CoreError, ErrorCode};
use crate::store::{CommitId, Store, StoreRead, check_relative, git_error};

/// The store as of one commit. Reads come from the commit's tree, never
/// from the working tree.
pub struct Revision<'s> {
    id: CommitId,
    tree: Tree<'s>,
    repo: &'s git2::Repository,
}

impl Revision<'_> {
    pub fn id(&self) -> &CommitId {
        &self.id
    }

    /// The bytes of `relative` at this revision; `None` if absent.
    pub fn read_bytes(&self, relative: &str) -> Result<Option<Vec<u8>>, CoreError> {
        check_relative(relative)?;
        let Ok(entry) = self.tree.get_path(Path::new(relative)) else {
            return Ok(None);
        };
        if entry.kind() != Some(ObjectType::Blob) {
            return Ok(None);
        }
        let blob = self.repo.find_blob(entry.id()).map_err(git_error)?;
        Ok(Some(blob.content().to_vec()))
    }

    /// Every file of this revision, store-relative, sorted.
    pub fn files(&self) -> Result<Vec<String>, CoreError> {
        let mut files = Vec::new();
        self.tree
            .walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
                if entry.kind() == Some(ObjectType::Blob)
                    && let Ok(name) = entry.name()
                {
                    files.push(format!("{dir}{name}"));
                }
                git2::TreeWalkResult::Ok
            })
            .map_err(git_error)?;
        files.sort();
        Ok(files)
    }
}

impl StoreRead for Revision<'_> {
    fn read_file(&self, relative: &str) -> Result<Option<String>, CoreError> {
        match self.read_bytes(relative)? {
            Some(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| {
                CoreError::new(ErrorCode::ParseError, "store file is not UTF-8").with_path(relative)
            }),
            None => Ok(None),
        }
    }

    fn list_files(&self, dir: &str) -> Result<Vec<String>, CoreError> {
        check_relative(dir)?;
        let Ok(entry) = self.tree.get_path(Path::new(dir)) else {
            return Ok(Vec::new());
        };
        let Ok(tree) = self.repo.find_tree(entry.id()) else {
            return Ok(Vec::new());
        };
        let mut names: Vec<String> = tree
            .iter()
            .filter(|entry| entry.kind() == Some(ObjectType::Blob))
            .filter_map(|entry| entry.name().ok().map(str::to_owned))
            .collect();
        names.sort();
        Ok(names)
    }
}

impl Store<'_> {
    /// The commit `id` (full or abbreviated hex) as a full id.
    pub fn resolve(&self, id: &str) -> Result<CommitId, CoreError> {
        let not_found = || CoreError::new(ErrorCode::NotFound, format!("no store commit {id:?}"));
        let object = self.repo.revparse_single(id).map_err(|_| not_found())?;
        let commit = object.peel_to_commit().map_err(|_| not_found())?;
        Ok(CommitId::from_oid(commit.id()))
    }

    /// The store as of commit `id`.
    pub fn revision(&self, id: &CommitId) -> Result<Revision<'_>, CoreError> {
        let commit = self.find_commit(id)?;
        Ok(Revision {
            id: id.clone(),
            tree: commit.tree().map_err(git_error)?,
            repo: &self.repo,
        })
    }

    /// The first parent of commit `id`; `None` for the initial commit.
    pub fn first_parent(&self, id: &CommitId) -> Result<Option<CommitId>, CoreError> {
        let commit = self.find_commit(id)?;
        Ok(commit.parent_ids().next().map(CommitId::from_oid))
    }

    /// Store-relative files whose content differs between `from` and `to`
    /// (added, removed or changed), sorted.
    pub fn changed_files(&self, from: &CommitId, to: &CommitId) -> Result<Vec<String>, CoreError> {
        let old = self.find_commit(from)?.tree().map_err(git_error)?;
        let new = self.find_commit(to)?.tree().map_err(git_error)?;
        let mut options = DiffOptions::new();
        options.ignore_submodules(true);
        let diff = self
            .repo
            .diff_tree_to_tree(Some(&old), Some(&new), Some(&mut options))
            .map_err(git_error)?;
        let mut files: Vec<String> = diff
            .deltas()
            .filter_map(|delta| {
                delta
                    .new_file()
                    .path()
                    .or_else(|| delta.old_file().path())
                    .and_then(Path::to_str)
                    .map(|path| path.replace('\\', "/"))
            })
            .collect();
        files.sort();
        files.dedup();
        Ok(files)
    }

    /// Creates the lightweight tag `name` on commit `target`;
    /// `already_exists` when the tag exists.
    pub fn create_tag(&self, name: &str, target: &CommitId) -> Result<(), CoreError> {
        let refname = format!("refs/tags/{name}");
        if !git2::Reference::is_valid_name(&refname) {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!("{name:?} is not a valid tag name"),
            ));
        }
        if self.repo.find_reference(&refname).is_ok() {
            return Err(CoreError::new(
                ErrorCode::AlreadyExists,
                format!("tag {name:?} already exists"),
            ));
        }
        let oid = Oid::from_str(target.as_str()).map_err(git_error)?;
        self.repo
            .reference(&refname, oid, false, &format!("tag: {name}"))
            .map_err(git_error)?;
        Ok(())
    }

    /// Whether the tag `name` exists.
    pub fn tag_exists(&self, name: &str) -> bool {
        self.repo
            .find_reference(&format!("refs/tags/{name}"))
            .is_ok()
    }

    /// The commit a tag points at.
    pub fn tag_target(&self, name: &str) -> Result<Option<CommitId>, CoreError> {
        match self.repo.find_reference(&format!("refs/tags/{name}")) {
            Ok(reference) => {
                let commit = reference.peel_to_commit().map_err(git_error)?;
                Ok(Some(CommitId::from_oid(commit.id())))
            }
            Err(_) => Ok(None),
        }
    }

    /// Commits reachable from `local` but not `upstream`, and from
    /// `upstream` but not `local` (`ahead`, `behind`). Both must be in the
    /// store.
    pub fn ahead_behind(
        &self,
        local: &CommitId,
        upstream: &CommitId,
    ) -> Result<(usize, usize), CoreError> {
        let local = Oid::from_str(local.as_str()).map_err(git_error)?;
        let upstream = Oid::from_str(upstream.as_str()).map_err(git_error)?;
        self.repo
            .graph_ahead_behind(local, upstream)
            .map_err(git_error)
    }

    fn find_commit(&self, id: &CommitId) -> Result<git2::Commit<'_>, CoreError> {
        let oid = Oid::from_str(id.as_str()).map_err(git_error)?;
        self.repo.find_commit(oid).map_err(git_error)
    }
}

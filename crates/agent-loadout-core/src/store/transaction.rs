//! `Store::transaction` (SPEC 4.1, 9.2): every core mutation stages its
//! file changes, then validates, writes and commits exactly once under the
//! cross-process lock.

use std::collections::BTreeMap;
use std::path::Path;

use git2::{Commit, IndexEntry, IndexTime, Oid, Tree};

use crate::error::{CoreError, ErrorCode};
use crate::model::server::Server;
use crate::model::{StoreDocument, StoreFile, TomlDocument, validate_store_file};
use crate::secrets::scanner::{ALLOWLIST_FILE, Allowlist, leak_error, scan_text};
use crate::store::lock::{LOCK_TIMEOUT, StoreLock};
use crate::store::trailers::{ApplyRef, MachineRef, Scope, Trailers, compose_message};
use crate::store::{
    CommitId, Machine, Store, StoreRead, check_relative, git_error, io_error, server_path,
};

/// What a transaction does, for its commit message and trailers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    op: String,
    summary: String,
    entities: Vec<String>,
    scope: Scope,
    apply: Option<ApplyRef>,
}

impl Operation {
    /// `op` is the `Loadout-Op` value (`server.add`); `summary` the commit
    /// subject (`server: add github`). The scope defaults to `system`.
    pub fn new(op: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            op: op.into(),
            summary: summary.into(),
            entities: Vec::new(),
            scope: Scope::System,
            apply: None,
        }
    }

    /// Adds a `Loadout-Entity` (`server/github`). A bulk change adds one per
    /// entity.
    pub fn entity(mut self, entity: impl Into<String>) -> Self {
        self.entities.push(entity.into());
        self
    }

    pub fn scope(mut self, scope: Scope) -> Self {
        self.scope = scope;
        self
    }

    fn check(&self) -> Result<(), CoreError> {
        let op_ok = !self.op.is_empty()
            && self
                .op
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
        if !op_ok {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!("invalid operation name {:?}", self.op),
            ));
        }
        if self.summary.trim().is_empty() {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                "a commit summary must not be empty",
            ));
        }
        Ok(())
    }

    fn trailers(&self, machine: &Machine, client: &str) -> Trailers {
        Trailers {
            op: Some(self.op.clone()),
            entities: self.entities.clone(),
            scope: Some(self.scope.clone()),
            machine: Some(MachineRef {
                id: machine.id.clone(),
                label: machine.label.clone(),
            }),
            client: Some(client.to_owned()),
            apply: self.apply.clone(),
            other: Vec::new(),
        }
    }
}

/// Result of [`Store::transaction`].
#[derive(Debug)]
pub struct Committed<T> {
    /// What the closure returned.
    pub value: T,
    /// The new commit; `None` when the staged changes left the tree as it was.
    pub commit: Option<CommitId>,
}

/// Staged changes, keyed by store-relative path; `None` removes the file.
pub(crate) type Changes = BTreeMap<String, Option<Vec<u8>>>;

/// Changes staged inside [`Store::transaction`]. Reads see staged changes.
pub struct Transaction<'a> {
    store: &'a Store<'a>,
    machine: Machine,
    operation: Operation,
    changes: Changes,
}

impl Transaction<'_> {
    /// The store this transaction commits to.
    pub fn store(&self) -> &Store<'_> {
        self.store
    }

    /// The machine making the change (the commit author).
    pub fn machine(&self) -> &Machine {
        &self.machine
    }

    /// Replaces the commit subject (an apply run names its files only once
    /// it has run).
    pub fn set_summary(&mut self, summary: impl Into<String>) {
        self.operation.summary = summary.into();
    }

    /// Adds a `Loadout-Entity` trailer, once per entity.
    pub fn add_entity(&mut self, entity: impl Into<String>) {
        let entity = entity.into();
        if !self.operation.entities.contains(&entity) {
            self.operation.entities.push(entity);
        }
    }

    /// Sets the `Loadout-Apply: <apply-id> <outcome>` trailer.
    pub fn set_apply(&mut self, apply: ApplyRef) {
        self.operation.apply = Some(apply);
    }

    /// Whether anything is staged.
    pub fn has_changes(&self) -> bool {
        !self.changes.is_empty()
    }

    /// Validates and leak-scans what is staged so far, as the commit will
    /// (SPEC 5, 11.8), without writing anything. For work inside the
    /// transaction that has effects outside the store (an instant apply).
    pub fn check(&self) -> Result<(), CoreError> {
        self.store.validate_changes(&self.changes)?;
        let message = compose_message(
            &self.operation.summary,
            &self.operation.trailers(&self.machine, &self.store.client),
        );
        self.store.scan_changes(&self.changes, &message)
    }

    /// A store file as this transaction sees it; `None` if absent.
    pub fn read_to_string(&self, relative: &str) -> Result<Option<String>, CoreError> {
        check_relative(relative)?;
        match self.changes.get(relative) {
            Some(Some(bytes)) => String::from_utf8(bytes.clone()).map(Some).map_err(|_| {
                CoreError::new(ErrorCode::InvalidValue, "staged file is not UTF-8")
                    .with_path(relative)
            }),
            Some(None) => Ok(None),
            None => self.store.read_to_string(relative),
        }
    }

    pub fn exists(&self, relative: &str) -> Result<bool, CoreError> {
        check_relative(relative)?;
        match self.changes.get(relative) {
            Some(change) => Ok(change.is_some()),
            None => Ok(self.store.fs.exists(&self.store.path(relative)?)),
        }
    }

    /// Stages `contents` for `relative`.
    pub fn write(&mut self, relative: &str, contents: impl Into<Vec<u8>>) -> Result<(), CoreError> {
        check_relative(relative)?;
        self.changes
            .insert(relative.to_owned(), Some(contents.into()));
        Ok(())
    }

    /// Stages the removal of `relative`.
    pub fn remove(&mut self, relative: &str) -> Result<(), CoreError> {
        check_relative(relative)?;
        self.changes.insert(relative.to_owned(), None);
        Ok(())
    }

    /// Stages a new server; `already_exists` if the id is taken.
    pub fn add_server(&mut self, server: Server) -> Result<(), CoreError> {
        let relative = server_path(&server.id)?;
        if self.exists(&relative)? {
            return Err(CoreError::new(
                ErrorCode::AlreadyExists,
                format!("server {:?} already exists", server.id),
            )
            .with_path(self.store.path(&relative)?));
        }
        let text = TomlDocument::new(server).to_text()?;
        self.write(&relative, text)
    }

    /// Stages the removal of a server; `not_found` if there is none.
    pub fn remove_server(&mut self, id: &str) -> Result<(), CoreError> {
        let relative = server_path(id)?;
        if !self.exists(&relative)? {
            return Err(CoreError::new(
                ErrorCode::NotFound,
                format!("server {id:?} does not exist"),
            ));
        }
        self.remove(&relative)
    }
}

impl StoreRead for Transaction<'_> {
    fn read_file(&self, relative: &str) -> Result<Option<String>, CoreError> {
        self.read_to_string(relative)
    }

    fn list_files(&self, dir: &str) -> Result<Vec<String>, CoreError> {
        let mut names = self.store.list_files(dir)?;
        let prefix = format!("{dir}/");
        for (relative, change) in &self.changes {
            let Some(name) = relative.strip_prefix(&prefix) else {
                continue;
            };
            if name.contains('/') {
                continue;
            }
            match change {
                Some(_) if !names.iter().any(|known| known == name) => names.push(name.to_owned()),
                Some(_) => {}
                None => names.retain(|known| known != name),
            }
        }
        names.sort();
        Ok(names)
    }
}

impl Store<'_> {
    /// Runs one mutation: takes the cross-process lock (waiting up to
    /// [`LOCK_TIMEOUT`]), lets `f` stage changes, validates every staged
    /// store file, runs the leak scanner (SPEC 11.8) over the staged files
    /// and the commit message, writes them to the working tree and creates
    /// exactly one commit with the [`Operation`]'s trailers. When `f`,
    /// validation or the scanner fails nothing is written and nothing is
    /// committed.
    pub fn transaction<'s, T>(
        &'s self,
        operation: Operation,
        f: impl FnOnce(&mut Transaction<'s>) -> Result<T, CoreError>,
    ) -> Result<Committed<T>, CoreError> {
        operation.check()?;
        let _lock = StoreLock::acquire(self.fs, &self.state_dir, LOCK_TIMEOUT)?;
        let machine = Machine::load_or_create(&self.env, self.fs, &self.state_dir)?;
        let mut tx = Transaction {
            store: self,
            machine,
            operation,
            changes: Changes::new(),
        };
        let value = f(&mut tx)?;
        tx.operation.check()?;
        let commit = self.commit_changes(&tx.machine, &tx.operation, tx.changes)?;
        Ok(Committed { value, commit })
    }

    /// Store files edited, added or removed in the working tree since HEAD
    /// (hand edits), store-relative and sorted. A nested repository (shown
    /// by git as a directory) is not a store file and is left out.
    pub fn worktree_changes(&self) -> Result<Vec<String>, CoreError> {
        let mut options = git2::StatusOptions::new();
        options
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_ignored(false)
            .exclude_submodules(true);
        let statuses = self.repo.statuses(Some(&mut options)).map_err(git_error)?;
        let mut files: Vec<String> = statuses
            .iter()
            .filter(|entry| !entry.status().is_ignored())
            .filter_map(|entry| entry.path().ok().map(|path| path.replace('\\', "/")))
            .filter(|path| !path.ends_with('/'))
            .collect();
        files.sort();
        files.dedup();
        Ok(files)
    }

    /// Commits the working tree's hand edits ([`Store::worktree_changes`],
    /// read under the lock) as one commit with their entities as trailers,
    /// after the same validation and leak scan as every transaction.
    /// Returns the commit (`None` when the working tree is clean) and the
    /// files it holds.
    pub fn commit_worktree(
        &self,
        operation: Operation,
    ) -> Result<(Option<CommitId>, Vec<String>), CoreError> {
        operation.check()?;
        let _lock = StoreLock::acquire(self.fs, &self.state_dir, LOCK_TIMEOUT)?;
        let machine = Machine::load_or_create(&self.env, self.fs, &self.state_dir)?;
        let files = self.worktree_changes()?;
        let mut operation = operation;
        for entity in crate::store::diff::trailer_entities(&files) {
            operation = operation.entity(entity);
        }
        let mut changes = Changes::new();
        for relative in &files {
            let path = self.path(relative)?;
            let bytes = if self.fs.exists(&path) {
                Some(
                    self.fs
                        .read(&path)
                        .map_err(|err| io_error("cannot read file", &path, &err))?,
                )
            } else {
                None
            };
            changes.insert(relative.clone(), bytes);
        }
        let commit = self.commit_changes(&machine, &operation, changes)?;
        Ok((commit, files))
    }

    /// Validates, scans, writes and commits `changes`. The caller holds the
    /// lock. The scanner runs before the first blob reaches the object
    /// database.
    pub(crate) fn commit_changes(
        &self,
        machine: &Machine,
        operation: &Operation,
        changes: Changes,
    ) -> Result<Option<CommitId>, CoreError> {
        if changes.is_empty() {
            return Ok(None);
        }
        self.commit_with(machine, operation, changes, None)
    }

    /// A merge commit of HEAD and `merged` with `changes` over HEAD's tree
    /// (SPEC 9.9 step 4): validated, scanned, and made even when the tree
    /// equals HEAD's, so the incoming history is recorded. The caller
    /// holds the lock.
    pub(crate) fn commit_merge(
        &self,
        machine: &Machine,
        operation: &Operation,
        changes: Changes,
        merged: Oid,
    ) -> Result<CommitId, CoreError> {
        let merged = self.repo.find_commit(merged).map_err(git_error)?;
        self.commit_with(machine, operation, changes, Some(&merged))?
            .ok_or_else(|| CoreError::new(ErrorCode::GitError, "the merge commit was not made"))
    }

    fn commit_with(
        &self,
        machine: &Machine,
        operation: &Operation,
        changes: Changes,
        merged: Option<&Commit<'_>>,
    ) -> Result<Option<CommitId>, CoreError> {
        self.validate_changes(&changes)?;
        let message = compose_message(
            &operation.summary,
            &operation.trailers(machine, &self.client),
        );
        self.scan_changes(&changes, &message)?;

        let parent = match self.repo.head() {
            Ok(head) => Some(head.peel_to_commit().map_err(git_error)?),
            Err(_) => None,
        };
        // The new tree is HEAD's tree plus exactly the staged changes, built
        // from the bytes in memory: no worktree scan, no filters.
        let mut index = self.repo.index().map_err(git_error)?;
        match &parent {
            Some(parent) => index.read_tree(&parent.tree().map_err(git_error)?),
            None => index.clear(),
        }
        .map_err(git_error)?;
        for (relative, change) in &changes {
            match change {
                Some(bytes) => index.add_frombuffer(&index_entry(relative), bytes),
                None if index.get_path(Path::new(relative), 0).is_some() => {
                    index.remove_path(Path::new(relative))
                }
                None => Ok(()),
            }
            .map_err(git_error)?;
        }
        let tree_id = index.write_tree().map_err(git_error)?;

        self.write_worktree(&changes)?;
        if merged.is_none()
            && parent
                .as_ref()
                .is_some_and(|parent| parent.tree_id() == tree_id)
        {
            return Ok(None);
        }

        let tree = self.repo.find_tree(tree_id).map_err(git_error)?;
        let oid = self.commit(machine, &message, &tree, parent.as_ref(), merged)?;
        index.write().map_err(git_error)?;
        Ok(Some(CommitId::from_oid(oid)))
    }

    /// The leak scanner over every staged file and the commit message.
    fn scan_changes(&self, changes: &Changes, message: &str) -> Result<(), CoreError> {
        let allow_text = match changes.get(ALLOWLIST_FILE) {
            Some(Some(bytes)) => Some(String::from_utf8_lossy(bytes).into_owned()),
            Some(None) => None,
            None => self.read_to_string(ALLOWLIST_FILE)?,
        };
        let allow = Allowlist::parse(allow_text.as_deref().unwrap_or_default());
        let mut findings = Vec::new();
        for (relative, change) in changes {
            if let Some(bytes) = change {
                let toml = relative.ends_with(".toml");
                findings.extend(scan_text(relative, bytes, toml, &self.secrets, &allow));
            }
        }
        findings.extend(scan_text(
            "commit message",
            message.as_bytes(),
            false,
            &self.secrets,
            &allow,
        ));
        match leak_error(&findings) {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Schema validation of every staged store file (SPEC 5); the first
    /// error aborts. Warnings pass.
    fn validate_changes(&self, changes: &Changes) -> Result<(), CoreError> {
        for (relative, change) in changes {
            let Some(bytes) = change else {
                continue;
            };
            let Some(file) = StoreFile::classify(Path::new(relative)) else {
                continue;
            };
            let path = self.path(relative)?;
            let text = std::str::from_utf8(bytes).map_err(|_| {
                CoreError::new(ErrorCode::InvalidValue, "store files must be UTF-8")
                    .with_path(&path)
            })?;
            let shared_id = match &file {
                StoreFile::Project { id, local: true } => {
                    self.shared_project_id(changes, &format!("projects/{id}/project.toml"))?
                }
                _ => None,
            };
            if let Some(err) =
                validate_store_file(&file, text, shared_id.as_deref()).first_error(Some(&path))
            {
                return Err(err);
            }
        }
        Ok(())
    }

    /// The id in the `project.toml` next to a staged `project.local.toml`.
    fn shared_project_id(
        &self,
        changes: &Changes,
        relative: &str,
    ) -> Result<Option<String>, CoreError> {
        let text = match changes.get(relative) {
            Some(Some(bytes)) => Some(String::from_utf8_lossy(bytes).into_owned()),
            Some(None) => None,
            None => self.read_to_string(relative)?,
        };
        let file = StoreFile::Project {
            id: String::new(),
            local: false,
        };
        Ok(text
            .and_then(|text| StoreDocument::parse(&file, &text).ok())
            .and_then(|doc| match doc {
                StoreDocument::Project(doc) => doc.into_value().id,
                _ => None,
            }))
    }

    pub(crate) fn write_worktree(&self, changes: &Changes) -> Result<(), CoreError> {
        for (relative, change) in changes {
            let path = self.path(relative)?;
            match change {
                Some(bytes) => {
                    if let Some(dir) = path.parent() {
                        self.fs
                            .create_dir_all(dir)
                            .map_err(|err| io_error("cannot create directory", dir, &err))?;
                    }
                    self.fs
                        .write(&path, bytes)
                        .map_err(|err| io_error("cannot write file", &path, &err))?;
                }
                None if self.fs.exists(&path) => self
                    .fs
                    .remove_file(&path)
                    .map_err(|err| io_error("cannot remove file", &path, &err))?,
                None => {}
            }
        }
        Ok(())
    }

    /// Commits `tree` on HEAD (plus `merged`, for a merge commit) with the
    /// machine's signature. libgit2 signs the reflog entries of a commit
    /// with its committer, so no identity from git configuration reaches
    /// `.git/logs` either.
    fn commit(
        &self,
        machine: &Machine,
        message: &str,
        tree: &Tree<'_>,
        parent: Option<&Commit<'_>>,
        merged: Option<&Commit<'_>>,
    ) -> Result<Oid, CoreError> {
        let signature = machine.signature(self.env.now())?;
        let parents: Vec<&Commit<'_>> = parent.into_iter().chain(merged).collect();
        self.repo
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                message,
                tree,
                &parents,
            )
            .map_err(git_error)
    }
}

fn index_entry(relative: &str) -> IndexEntry {
    IndexEntry {
        ctime: IndexTime::new(0, 0),
        mtime: IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode: 0o100_644,
        uid: 0,
        gid: 0,
        file_size: 0,
        id: Oid::ZERO_SHA1,
        flags: 0,
        flags_extended: 0,
        path: relative.as_bytes().to_vec(),
    }
}

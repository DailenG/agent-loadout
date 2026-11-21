//! Pull merges (SPEC 9.9): an incoming head that is an ancestor of HEAD
//! changes nothing, a descendant fast-forwards, anything else merges per
//! entity against the merge base.
//!
//! - TOML files merge per field (tables flattened to key paths, arrays as
//!   one value): changed on one side takes that side, changed identically
//!   takes it, changed differently is a conflict for that field.
//! - Other text (skill content, `.gitattributes`) merges per line through
//!   git's file merge; binary content changed on both sides conflicts.
//! - A file deleted on one side and changed on the other conflicts.
//! - `machines/<id>.toml` and `machines/<id>.applied.toml` are single
//!   writer: this machine's own files keep ours; another machine's come
//!   from the side whose history touched them last. Never a conflict.
//!
//! Nothing is resolved in favor of local silently. With a conflict HEAD
//! stays as it is and the caller records the conflicts.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

use git2::{MergeFileInput, Oid, Sort};
use toml_edit::{DocumentMut, Item, Table};

use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::store::transaction::Changes;
use crate::store::{
    CommitId, LOCK_TIMEOUT, Machine, Operation, Store, StoreLock, git_error, io_error,
};

/// File contents by store-relative path.
pub type Files = BTreeMap<String, Vec<u8>>;

/// One side of a merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Ours,
    Theirs,
}

/// One field (or a whole file, `field: None`) both sides changed
/// differently. Values are TOML text or file text; `None` is absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub file: String,
    pub field: Option<String>,
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
}

/// The merged files, or the conflicts that prevent them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    pub files: Files,
    pub conflicts: Vec<Conflict>,
}

/// What decides the single-writer files.
pub struct MergePolicy<'a> {
    /// This machine: its `machines/<id>*` files always keep ours.
    pub machine_id: &'a str,
    /// For another machine's file both sides changed: the side whose
    /// history touched it last.
    pub newer: &'a dyn Fn(&str) -> Side,
}

/// Merges `ours` and `theirs` against `base` (SPEC 9.9 step 3).
pub fn merge_trees(base: &Files, ours: &Files, theirs: &Files, policy: &MergePolicy<'_>) -> Merged {
    let mut paths: Vec<&String> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();
    paths.sort();
    paths.dedup();
    let mut files = Files::new();
    let mut conflicts = Vec::new();
    for path in paths {
        let (b, o, t) = (base.get(path), ours.get(path), theirs.get(path));
        let pick = |side: Option<&Vec<u8>>, files: &mut Files| {
            if let Some(bytes) = side {
                files.insert(path.clone(), bytes.clone());
            }
        };
        if o == t || t == b {
            pick(o, &mut files);
            continue;
        }
        if o == b {
            pick(t, &mut files);
            continue;
        }
        if let Some(owner) = machine_file_owner(path) {
            let side = if owner == policy.machine_id {
                Side::Ours
            } else {
                (policy.newer)(path)
            };
            pick(if side == Side::Ours { o } else { t }, &mut files);
            continue;
        }
        let (Some(o), Some(t)) = (o, t) else {
            conflicts.push(Conflict {
                file: path.clone(),
                field: None,
                base: b.map(|bytes| text(bytes)),
                ours: o.map(|bytes| text(bytes)),
                theirs: t.map(|bytes| text(bytes)),
            });
            continue;
        };
        let merged = if path.ends_with(".toml") {
            merge_toml(path, b, o, t)
        } else {
            merge_text(path, b, o, t)
        };
        match merged {
            Ok(bytes) => {
                files.insert(path.clone(), bytes);
            }
            Err(found) => conflicts.extend(found),
        }
    }
    Merged { files, conflicts }
}

/// The machine id of a `machines/<id>.toml` or `machines/<id>.applied.toml`.
fn machine_file_owner(path: &str) -> Option<&str> {
    let name = path.strip_prefix("machines/")?;
    name.strip_suffix(".applied.toml")
        .or_else(|| name.strip_suffix(".toml"))
        .filter(|id| !id.contains('/'))
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn whole_file(path: &str, b: Option<&Vec<u8>>, o: &[u8], t: &[u8]) -> Vec<Conflict> {
    vec![Conflict {
        file: path.to_owned(),
        field: None,
        base: b.map(|bytes| text(bytes)),
        ours: Some(text(o)),
        theirs: Some(text(t)),
    }]
}

/// Per-line 3-way merge of text; binary or overlapping changes conflict.
fn merge_text(
    path: &str,
    b: Option<&Vec<u8>>,
    o: &[u8],
    t: &[u8],
) -> Result<Vec<u8>, Vec<Conflict>> {
    let binary = |bytes: &[u8]| std::str::from_utf8(bytes).is_err() || bytes.contains(&0);
    if binary(o) || binary(t) || b.is_some_and(|b| binary(b)) {
        return Err(whole_file(path, b, o, t));
    }
    let empty = Vec::new();
    let mut ancestor = MergeFileInput::new();
    ancestor.content(b.unwrap_or(&empty)).path(path);
    let mut ours = MergeFileInput::new();
    ours.content(o).path(path);
    let mut theirs = MergeFileInput::new();
    theirs.content(t).path(path);
    // `git2::merge_file` does not initialize libgit2 itself (it fails with
    // "no error" before any other git2 call in the process); hashing the
    // base initializes it.
    let _ = git2::Oid::hash_object(git2::ObjectType::Blob, b.unwrap_or(&empty));
    match git2::merge_file(&ancestor, &ours, &theirs, None) {
        Ok(result) if result.is_automergeable() => Ok(result.content().to_vec()),
        _ => Err(whole_file(path, b, o, t)),
    }
}

type Fields = BTreeMap<Vec<String>, toml::Value>;

/// Leaves of a TOML table by key path; arrays and inline values are leaves.
fn flatten(table: &toml::Table) -> Fields {
    fn walk(prefix: &mut Vec<String>, table: &toml::Table, out: &mut Fields) {
        for (key, value) in table {
            prefix.push(key.clone());
            match value {
                toml::Value::Table(inner) => walk(prefix, inner, out),
                other => {
                    out.insert(prefix.clone(), other.clone());
                }
            }
            prefix.pop();
        }
    }
    let mut out = Fields::new();
    walk(&mut Vec::new(), table, &mut out);
    out
}

fn dotted(path: &[String]) -> String {
    path.iter()
        .map(|key| {
            let bare = !key.is_empty()
                && key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
            if bare {
                key.clone()
            } else {
                toml::Value::String(key.clone()).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Per-field 3-way merge of a TOML file, written into ours so its layout
/// and comments stay.
fn merge_toml(
    path: &str,
    b: Option<&Vec<u8>>,
    o: &[u8],
    t: &[u8],
) -> Result<Vec<u8>, Vec<Conflict>> {
    let parse = |bytes: &[u8]| -> Option<Fields> {
        std::str::from_utf8(bytes)
            .ok()?
            .parse::<toml::Table>()
            .ok()
            .map(|table| flatten(&table))
    };
    let base = match b {
        Some(bytes) => parse(bytes),
        None => Some(Fields::new()),
    };
    let (Some(base), Some(ours), Some(theirs)) = (base, parse(o), parse(t)) else {
        return Err(whole_file(path, b, o, t));
    };
    let mut keys: Vec<&Vec<String>> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();
    keys.sort();
    keys.dedup();
    let mut conflicts = Vec::new();
    let mut edits: Vec<(&Vec<String>, Option<&toml::Value>)> = Vec::new();
    for key in keys {
        let (bv, ov, tv) = (base.get(key), ours.get(key), theirs.get(key));
        if ov == tv || tv == bv {
            continue;
        }
        if ov == bv {
            edits.push((key, tv));
            continue;
        }
        conflicts.push(Conflict {
            file: path.to_owned(),
            field: Some(dotted(key)),
            base: bv.map(ToString::to_string),
            ours: ov.map(ToString::to_string),
            theirs: tv.map(ToString::to_string),
        });
    }
    if !conflicts.is_empty() {
        return Err(conflicts);
    }
    let text = std::str::from_utf8(o).expect("parsed above");
    let mut doc: DocumentMut = text.parse().map_err(|_| whole_file(path, b, o, t))?;
    for (key, value) in edits {
        let applied = match value {
            Some(value) => set_path(&mut doc, key, value),
            None => remove_path(&mut doc, key),
        };
        if !applied {
            return Err(whole_file(path, b, o, t));
        }
    }
    let merged = doc.to_string().into_bytes();
    // The result must read back as exactly the merged fields.
    let mut expected = ours.clone();
    for key in base.keys().chain(theirs.keys()) {
        let (bv, ov, tv) = (base.get(key), ours.get(key), theirs.get(key));
        if ov == bv && tv != bv {
            match tv {
                Some(value) => expected.insert(key.clone(), value.clone()),
                None => expected.remove(key),
            };
        }
    }
    match parse(&merged) {
        Some(fields) if fields == expected => Ok(merged),
        _ => Err(whole_file(path, b, o, t)),
    }
}

fn set_path(doc: &mut DocumentMut, path: &[String], value: &toml::Value) -> bool {
    let Some((last, parents)) = path.split_last() else {
        return false;
    };
    let Ok(value) = value.to_string().parse::<toml_edit::Value>() else {
        return false;
    };
    let mut table: &mut dyn toml_edit::TableLike = doc.as_table_mut();
    for key in parents {
        let item = table.entry(key).or_insert_with(|| {
            let mut created = Table::new();
            created.set_implicit(true);
            Item::Table(created)
        });
        match item.as_table_like_mut() {
            Some(inner) => table = inner,
            None => return false,
        }
    }
    match table.get_mut(last) {
        Some(item) if item.is_value() => {
            if let Some(old) = item.as_value() {
                let decor = old.decor().clone();
                let mut value = value;
                *value.decor_mut() = decor;
                *item = Item::Value(value);
            }
        }
        Some(_) => return false,
        None => {
            table.insert(last, Item::Value(value));
        }
    }
    true
}

fn remove_path(doc: &mut DocumentMut, path: &[String]) -> bool {
    fn remove(table: &mut dyn toml_edit::TableLike, path: &[String]) -> bool {
        match path {
            [] => false,
            [last] => table.remove(last).is_some(),
            [first, rest @ ..] => {
                let Some(inner) = table.get_mut(first).and_then(Item::as_table_like_mut) else {
                    return false;
                };
                let removed = remove(inner, rest);
                if removed && inner.is_empty() {
                    table.remove(first);
                }
                removed
            }
        }
    }
    remove(doc.as_table_mut(), path)
}

/// What merging one incoming head did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The incoming head is already in HEAD's history.
    UpToDate,
    /// HEAD moved forward to the incoming head.
    FastForward(CommitId),
    /// A merge commit with both parents.
    Merged(CommitId),
    /// Fields both sides changed differently; HEAD is unchanged.
    Conflicts {
        base: Option<CommitId>,
        conflicts: Vec<Conflict>,
    },
}

impl Store<'_> {
    /// Merges `incoming` (whose objects are in the store already) into
    /// HEAD; `label` names it in the merge commit (`merge: <label>`).
    pub fn merge_incoming(
        &self,
        incoming: &CommitId,
        label: &str,
    ) -> Result<MergeOutcome, CoreError> {
        let theirs_oid = Oid::from_str(incoming.as_str()).map_err(git_error)?;
        self.repo.find_commit(theirs_oid).map_err(|_| {
            CoreError::new(
                ErrorCode::RemoteIncomplete,
                format!("commit {} is not in the store", incoming.short()),
            )
        })?;
        let _lock = StoreLock::acquire(self.fs, &self.state_dir, LOCK_TIMEOUT)?;
        let machine = Machine::load_or_create(&self.env, self.fs, &self.state_dir)?;
        let head = self.head()?;
        let ours_oid = Oid::from_str(head.as_str()).map_err(git_error)?;
        if ours_oid == theirs_oid
            || self
                .repo
                .graph_descendant_of(ours_oid, theirs_oid)
                .map_err(git_error)?
        {
            return Ok(MergeOutcome::UpToDate);
        }
        if self
            .repo
            .graph_descendant_of(theirs_oid, ours_oid)
            .map_err(git_error)?
        {
            self.fast_forward(&head, incoming)?;
            return Ok(MergeOutcome::FastForward(incoming.clone()));
        }
        let base = self.repo.merge_base(ours_oid, theirs_oid).ok();
        let base_files = match base {
            Some(base) => self.files_at(&CommitId::from_oid(base))?,
            None => Files::new(),
        };
        let ours = self.files_at(&head)?;
        let theirs = self.files_at(incoming)?;
        let newer = |path: &str| {
            let ours_time = self.last_touch(ours_oid, path);
            let theirs_time = self.last_touch(theirs_oid, path);
            if ours_time > theirs_time {
                Side::Ours
            } else {
                Side::Theirs
            }
        };
        let merged = merge_trees(
            &base_files,
            &ours,
            &theirs,
            &MergePolicy {
                machine_id: &machine.id,
                newer: &newer,
            },
        );
        if !merged.conflicts.is_empty() {
            return Ok(MergeOutcome::Conflicts {
                base: base.map(CommitId::from_oid),
                conflicts: merged.conflicts,
            });
        }
        let mut changes = Changes::new();
        for path in ours.keys().chain(merged.files.keys()) {
            if ours.get(path) != merged.files.get(path) {
                changes.insert(path.clone(), merged.files.get(path).cloned());
            }
        }
        let operation = Operation::new("store.merge", format!("merge: {label}"));
        let commit = self.commit_merge(&machine, &operation, changes, theirs_oid)?;
        Ok(MergeOutcome::Merged(commit))
    }

    /// Every file of a commit with its bytes.
    pub(crate) fn files_at(&self, id: &CommitId) -> Result<Files, CoreError> {
        let revision = self.revision(id)?;
        let mut files = Files::new();
        for file in revision.files()? {
            if let Some(bytes) = revision.read_bytes(&file)? {
                files.insert(file, bytes);
            }
        }
        Ok(files)
    }

    /// When the history from `start` last changed `path`.
    fn last_touch(&self, start: Oid, path: &str) -> Option<SystemTime> {
        let mut walk = self.repo.revwalk().ok()?;
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME).ok()?;
        walk.push(start).ok()?;
        let paths = [path.to_owned()];
        for oid in walk.flatten() {
            let commit = self.repo.find_commit(oid).ok()?;
            if crate::store::log::touches(&commit, &paths).ok()? {
                return Some(crate::store::log::commit_time(&commit));
            }
        }
        None
    }

    /// Moves `main` from `from` to its descendant `to`, updating the
    /// working tree and the index. The caller holds the lock.
    fn fast_forward(&self, from: &CommitId, to: &CommitId) -> Result<(), CoreError> {
        let old = self.files_at(from)?;
        let new = self.files_at(to)?;
        let mut changes = Changes::new();
        for path in old.keys().chain(new.keys()) {
            if old.get(path) != new.get(path) {
                changes.insert(path.clone(), new.get(path).cloned());
            }
        }
        self.write_worktree(&changes)?;
        let oid = Oid::from_str(to.as_str()).map_err(git_error)?;
        let tree = self
            .repo
            .find_commit(oid)
            .and_then(|commit| commit.tree())
            .map_err(git_error)?;
        let mut index = self.repo.index().map_err(git_error)?;
        index.read_tree(&tree).map_err(git_error)?;
        index.write().map_err(git_error)?;
        self.repo
            .reference(
                &format!("refs/heads/{}", crate::store::BRANCH),
                oid,
                true,
                &format!("fast-forward to {}", to.short()),
            )
            .map_err(git_error)?;
        Ok(())
    }
}

/// `state/merge/MERGE_STATE.json`: the conflicts of the last pull, local
/// only (never in the store), so a sync tool never loops on them.
pub const MERGE_STATE: &str = "MERGE_STATE.json";

/// One incoming head that did not merge.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingMerge {
    /// `<remote>/<machine-id>`.
    pub source: String,
    pub incoming: String,
    pub base: Option<String>,
    pub conflicts: Vec<RecordedConflict>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RecordedConflict {
    pub file: String,
    pub field: Option<String>,
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
}

impl From<&Conflict> for RecordedConflict {
    fn from(conflict: &Conflict) -> Self {
        Self {
            file: conflict.file.clone(),
            field: conflict.field.clone(),
            base: conflict.base.clone(),
            ours: conflict.ours.clone(),
            theirs: conflict.theirs.clone(),
        }
    }
}

/// Writes the pending merges of a pull to `state/merge/`, or removes the
/// file when there are none.
pub fn write_merge_state(
    fs: &dyn Fs,
    state_dir: &Path,
    pending: &[PendingMerge],
) -> Result<(), CoreError> {
    let dir = state_dir.join("merge");
    let path = dir.join(MERGE_STATE);
    if pending.is_empty() {
        if fs.exists(&path) {
            fs.remove_file(&path)
                .map_err(|err| io_error("cannot remove file", &path, &err))?;
        }
        return Ok(());
    }
    let text = serde_json::to_string_pretty(&serde_json::json!({
        "schema": 1,
        "merges": pending,
    }))
    .expect("merge state serializes");
    crate::apply::write_atomic(fs, &path, text.as_bytes())
}

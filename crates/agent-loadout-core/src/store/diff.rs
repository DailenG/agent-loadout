//! Diffs between two store revisions (SPEC 9.3).
//!
//! - **Semantic**: per entity, the fields added, removed or changed
//!   (`env.GITHUB_TOOLSETS: "repos" -> "repos,issues"`). TOML files are
//!   compared field by field (tables, inline ones included, flattened to
//!   dotted paths; arrays as one value); references are shown as written,
//!   since the store holds references only. Other text (skill content) is
//!   a unified diff.
//! - **Raw**: the unified git diff.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use git2::{DiffFormat, DiffOptions, Tree};
use similar::TextDiff;

use crate::error::CoreError;
use crate::store::{CommitId, Store, git_error};

/// How an entity file changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Removed,
    Changed,
}

impl ChangeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            ChangeKind::Added => "added",
            ChangeKind::Removed => "removed",
            ChangeKind::Changed => "changed",
        }
    }
}

/// One field of a TOML entity: absent on the side that lacks it. Values
/// are TOML text (`"repos"`, `["-y", "pkg"]`, `60`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldChange {
    /// Dotted path (`env.GITHUB_TOOLSETS`, `secrets.GITHUB_PAT.ref`).
    pub field: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// One changed store file, named by its entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityChange {
    /// `server/github`, `secrets`, `skill/review`, `machine/<id>`, or the
    /// file name for files of no entity.
    pub entity: String,
    /// Store-relative file.
    pub file: String,
    pub kind: ChangeKind,
    /// Field changes of a TOML file.
    pub fields: Vec<FieldChange>,
    /// Unified diff of a file that is not TOML (or does not parse); a note
    /// for binary content.
    pub text: Option<String>,
}

/// The entity a store file belongs to (the `Loadout-Entity` trailer
/// value); the file name for store-level files.
pub fn entity_of(file: &str) -> String {
    let mut parts = file.split('/');
    let first = parts.next().unwrap_or_default();
    let second = parts.next();
    let stem = |name: &str| name.strip_suffix(".toml").unwrap_or(name).to_owned();
    match (first, second) {
        ("servers", Some(name)) => format!("server/{}", stem(name)),
        ("skill-meta", Some(name)) => format!("skill/{}", stem(name)),
        ("skills", Some(id)) => format!("skill/{id}"),
        ("projects", Some(id)) => format!("project/{id}"),
        ("machines", Some(name)) => {
            let name = name.strip_suffix(".applied.toml").unwrap_or(name);
            format!("machine/{}", stem(name))
        }
        (name, None) => stem(name),
        _ => file.to_owned(),
    }
}

/// The `Loadout-Entity` trailers of a commit changing `files`: each known
/// entity once, in file order.
pub fn trailer_entities(files: &[String]) -> Vec<String> {
    let mut entities: Vec<String> = Vec::new();
    for file in files {
        let entity = entity_of(file);
        if !crate::store::entity_paths(&entity).is_empty() && !entities.contains(&entity) {
            entities.push(entity);
        }
    }
    entities
}

impl Store<'_> {
    /// The semantic diff from `from` (`None`: the empty store) to `to`,
    /// ordered by file.
    pub fn semantic_diff(
        &self,
        from: Option<&CommitId>,
        to: &CommitId,
    ) -> Result<Vec<EntityChange>, CoreError> {
        let old = from.map(|id| self.revision(id)).transpose()?;
        let new = self.revision(to)?;
        let mut changes = Vec::new();
        for file in self.changed_between(from, to)? {
            let before = match &old {
                Some(old) => old.read_bytes(&file)?,
                None => None,
            };
            let after = new.read_bytes(&file)?;
            changes.push(file_change(&file, before.as_deref(), after.as_deref()));
        }
        Ok(changes)
    }

    /// The unified git diff from `from` (`None`: the empty store) to `to`.
    pub fn raw_diff(&self, from: Option<&CommitId>, to: &CommitId) -> Result<String, CoreError> {
        let old = self.tree_of(from)?;
        let new = self.tree_of(Some(to))?;
        let diff = self
            .repo
            .diff_tree_to_tree(old.as_ref(), new.as_ref(), Some(&mut DiffOptions::new()))
            .map_err(git_error)?;
        let mut out = Vec::new();
        diff.print(DiffFormat::Patch, |_, _, line| {
            if matches!(line.origin(), '+' | '-' | ' ') {
                out.push(line.origin() as u8);
            }
            out.extend_from_slice(line.content());
            true
        })
        .map_err(git_error)?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// Files that differ between `from` (`None`: the empty store) and `to`.
    fn changed_between(
        &self,
        from: Option<&CommitId>,
        to: &CommitId,
    ) -> Result<Vec<String>, CoreError> {
        match from {
            Some(from) => self.changed_files(from, to),
            None => self.revision(to)?.files(),
        }
    }

    fn tree_of(&self, id: Option<&CommitId>) -> Result<Option<Tree<'_>>, CoreError> {
        let Some(id) = id else {
            return Ok(None);
        };
        let oid = git2::Oid::from_str(id.as_str()).map_err(git_error)?;
        let commit = self.repo.find_commit(oid).map_err(git_error)?;
        commit.tree().map(Some).map_err(git_error)
    }
}

fn file_change(file: &str, before: Option<&[u8]>, after: Option<&[u8]>) -> EntityChange {
    let kind = match (before, after) {
        (None, _) => ChangeKind::Added,
        (_, None) => ChangeKind::Removed,
        _ => ChangeKind::Changed,
    };
    let mut change = EntityChange {
        entity: entity_of(file),
        file: file.to_owned(),
        kind,
        fields: Vec::new(),
        text: None,
    };
    let text = |bytes: Option<&[u8]>| -> Option<Option<String>> {
        match bytes {
            None => Some(None),
            Some(bytes) => std::str::from_utf8(bytes).ok().map(|s| Some(s.to_owned())),
        }
    };
    let (Some(old), Some(new)) = (text(before), text(after)) else {
        change.text = Some("binary file changed".to_owned());
        return change;
    };
    if Path::new(file).extension().is_some_and(|ext| ext == "toml") {
        let parse = |text: &Option<String>| -> Option<BTreeMap<String, String>> {
            match text {
                None => Some(BTreeMap::new()),
                Some(text) => text
                    .parse::<toml::Table>()
                    .ok()
                    .map(|table| flatten(&table)),
            }
        };
        if let (Some(old_fields), Some(new_fields)) = (parse(&old), parse(&new)) {
            change.fields = field_changes(&old_fields, &new_fields);
            if change.fields.is_empty() {
                change.text = Some("only comments or formatting changed".to_owned());
            }
            return change;
        }
    }
    let (old, new) = (old.unwrap_or_default(), new.unwrap_or_default());
    change.text = Some(
        TextDiff::from_lines(&old, &new)
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{file}"), &format!("b/{file}"))
            .to_string(),
    );
    change
}

/// Leaves of `table` by dotted path; arrays and inline values are leaves.
fn flatten(table: &toml::Table) -> BTreeMap<String, String> {
    fn walk(prefix: &str, table: &toml::Table, out: &mut BTreeMap<String, String>) {
        for (key, value) in table {
            let path = if prefix.is_empty() {
                key_text(key)
            } else {
                format!("{prefix}.{}", key_text(key))
            };
            match value {
                toml::Value::Table(inner) => walk(&path, inner, out),
                other => {
                    out.insert(path, other.to_string());
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    walk("", table, &mut out);
    out
}

/// A key as written in a dotted path: bare when it can be, else quoted.
fn key_text(key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if bare {
        key.to_owned()
    } else {
        toml::Value::String(key.to_owned()).to_string()
    }
}

fn field_changes(
    old: &BTreeMap<String, String>,
    new: &BTreeMap<String, String>,
) -> Vec<FieldChange> {
    let mut paths: Vec<&String> = old.keys().chain(new.keys()).collect();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter(|path| old.get(*path) != new.get(*path))
        .map(|path| FieldChange {
            field: path.clone(),
            before: old.get(path).cloned(),
            after: new.get(path).cloned(),
        })
        .collect()
}

/// The changes as text, one block per file:
///
/// ```text
/// server/github: changed (servers/github.toml)
///   env.GITHUB_TOOLSETS: "repos" -> "repos,issues"
///   + tags = ["dev"]
///   - description = "Old"
/// ```
pub fn render(changes: &[EntityChange]) -> String {
    let mut out = String::new();
    for change in changes {
        let _ = writeln!(
            out,
            "{}: {} ({})",
            change.entity,
            change.kind.as_str(),
            change.file
        );
        for field in &change.fields {
            let _ = match (&field.before, &field.after) {
                (Some(before), Some(after)) => {
                    writeln!(out, "  {}: {before} -> {after}", field.field)
                }
                (None, Some(after)) => writeln!(out, "  + {} = {after}", field.field),
                (Some(before), None) => writeln!(out, "  - {} = {before}", field.field),
                (None, None) => Ok(()),
            };
        }
        if let Some(text) = &change.text {
            for line in text.lines() {
                let _ = writeln!(out, "  {line}");
            }
        }
    }
    out
}

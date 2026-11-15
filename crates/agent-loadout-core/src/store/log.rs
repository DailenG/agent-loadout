//! Store history (SPEC 9.3, log only): commits with op, entities, scope,
//! machine and time, filtered by entity or scope.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use git2::{Commit, Oid, Sort, Tree};

use crate::error::CoreError;
use crate::store::trailers::{Scope, Trailers, message_summary};
use crate::store::{CommitId, Store, git_error, utc_datetime};

/// One store commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub id: CommitId,
    /// Commit time.
    pub time: SystemTime,
    pub author_name: String,
    pub author_email: String,
    /// The first line of the message.
    pub summary: String,
    pub trailers: Trailers,
}

impl LogEntry {
    /// [`Self::time`] as RFC 3339 in UTC (`2026-01-01T00:00:00Z`).
    pub fn time_rfc3339(&self) -> String {
        utc_datetime(self.time).to_string()
    }
}

/// Which commits [`Store::log`] returns. The default is every commit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogFilter {
    /// Commits naming this entity in a `Loadout-Entity` trailer, or changing
    /// one of its files (so hand-made commits are found too).
    pub entity: Option<String>,
    /// Commits whose `Loadout-Scope` is this scope.
    pub scope: Option<Scope>,
    /// At most this many commits, newest first.
    pub limit: Option<usize>,
}

/// Store-relative files and directories holding an entity
/// (`server/github` is `servers/github.toml`). Unknown kinds hold none.
pub fn entity_paths(entity: &str) -> Vec<String> {
    match entity.split_once('/') {
        Some(("server", id)) => vec![format!("servers/{id}.toml")],
        Some(("skill", id)) => vec![format!("skills/{id}"), format!("skill-meta/{id}.toml")],
        Some(("project", id)) => vec![format!("projects/{id}")],
        Some(("machine", id)) => vec![
            format!("machines/{id}.toml"),
            format!("machines/{id}.applied.toml"),
        ],
        None if matches!(entity, "secrets" | "clients" | "settings") => {
            vec![format!("{entity}.toml")]
        }
        _ => Vec::new(),
    }
    .into_iter()
    .filter(|path| !path.split('/').any(|part| part.is_empty() || part == ".."))
    .collect()
}

impl Store<'_> {
    /// Commits reachable from HEAD, newest first, matching `filter`.
    pub fn log(&self, filter: &LogFilter) -> Result<Vec<LogEntry>, CoreError> {
        let mut walk = self.repo.revwalk().map_err(git_error)?;
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)
            .map_err(git_error)?;
        walk.push_head().map_err(git_error)?;
        let paths = filter
            .entity
            .as_deref()
            .map(entity_paths)
            .unwrap_or_default();

        let mut entries = Vec::new();
        for oid in walk {
            if filter.limit.is_some_and(|limit| entries.len() >= limit) {
                break;
            }
            let commit = self
                .repo
                .find_commit(oid.map_err(git_error)?)
                .map_err(git_error)?;
            let message = String::from_utf8_lossy(commit.message_bytes());
            let trailers = Trailers::parse(&message);
            if filter
                .scope
                .as_ref()
                .is_some_and(|scope| trailers.scope.as_ref() != Some(scope))
            {
                continue;
            }
            if let Some(entity) = &filter.entity
                && !trailers.entities.contains(entity)
                && !touches(&commit, &paths)?
            {
                continue;
            }
            let author = commit.author();
            entries.push(LogEntry {
                id: CommitId::from_oid(commit.id()),
                time: commit_time(&commit),
                author_name: String::from_utf8_lossy(author.name_bytes()).into_owned(),
                author_email: String::from_utf8_lossy(author.email_bytes()).into_owned(),
                summary: message_summary(&message).to_owned(),
                trailers,
            });
        }
        Ok(entries)
    }
}

/// Whether `commit` changed any of `paths` relative to its first parent.
fn touches(commit: &Commit<'_>, paths: &[String]) -> Result<bool, CoreError> {
    if paths.is_empty() {
        return Ok(false);
    }
    let tree = commit.tree().map_err(git_error)?;
    let parent_tree = match commit.parent(0) {
        Ok(parent) => Some(parent.tree().map_err(git_error)?),
        Err(_) => None,
    };
    Ok(paths
        .iter()
        .any(|path| entry_id(Some(&tree), path) != entry_id(parent_tree.as_ref(), path)))
}

fn entry_id(tree: Option<&Tree<'_>>, path: &str) -> Option<Oid> {
    tree?.get_path(Path::new(path)).ok().map(|entry| entry.id())
}

fn commit_time(commit: &Commit<'_>) -> SystemTime {
    let seconds = commit.time().seconds();
    match u64::try_from(seconds) {
        Ok(seconds) => UNIX_EPOCH + Duration::from_secs(seconds),
        Err(_) => UNIX_EPOCH,
    }
}

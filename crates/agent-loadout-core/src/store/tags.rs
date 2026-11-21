//! Store tags (SPEC 9.4).
//!
//! - User tags: annotated, named `^[A-Za-z0-9._-]{1,64}$`, with a message
//!   and this machine as tagger. Never pruned.
//! - Automatic tags: `auto/<kind>/<timestamp>` (`auto/pre-restore/...`,
//!   `auto/pre-rollback/...`, `auto/pre-pull/...`), lightweight, pruned
//!   once older than [`AUTO_TAG_RETENTION`] whenever a new one is made.
//!
//! Tags never move: a name in use is `already_exists` (automatic tags get a
//! `-2`, `-3` suffix instead). Tag messages pass the leak scanner.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use git2::{ObjectType, Oid};

use crate::apply::backup;
use crate::error::{CoreError, ErrorCode};
use crate::secrets::scanner::{ALLOWLIST_FILE, Allowlist, leak_error, scan_text};
use crate::store::{CommitId, LOCK_TIMEOUT, Machine, Store, StoreLock, git_error};

/// Prefix of automatic tags.
pub const AUTO_PREFIX: &str = "auto/";
/// How long automatic tags are kept (SPEC 9.4: 90 days).
pub const AUTO_TAG_RETENTION: Duration = Duration::from_secs(90 * 86_400);
/// Longest user tag name.
const MAX_NAME: usize = 64;

/// What made a tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagKind {
    /// Named by a person (`loadout tag`).
    User,
    /// `auto/...`, made before a restore, rollback, pull or migration.
    Auto,
}

impl TagKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            TagKind::User => "user",
            TagKind::Auto => "auto",
        }
    }
}

/// One tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagInfo {
    pub name: String,
    pub kind: TagKind,
    /// The commit the tag points at.
    pub target: CommitId,
    /// The message of an annotated tag.
    pub message: Option<String>,
    /// The tagger's name (`loadout <machine-label>`) of an annotated tag.
    pub tagger: Option<String>,
    /// When the tag was made: the tagger time, or the time in an automatic
    /// tag's name.
    pub time: Option<SystemTime>,
}

/// Whether `name` is a valid user tag name: SPEC 9.4's characters, git's
/// ref rules (no leading `.`, no `..`, no trailing `.` or `.lock`), no
/// leading `-` (it reads as an option), and nothing that reads as a
/// revision instead (`HEAD`, `main`, 4 or more hex digits).
pub fn is_user_tag_name(name: &str) -> bool {
    let hex = name.len() >= 4 && name.bytes().all(|b| b.is_ascii_hexdigit());
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && !name.starts_with(['.', '-'])
        && !name.ends_with('.')
        && !name.ends_with(".lock")
        && !name.contains("..")
        && !hex
        && !matches!(name, "HEAD" | crate::store::BRANCH)
}

impl Store<'_> {
    /// Creates the annotated user tag `name` on `target`, tagged by this
    /// machine. `message` defaults to `tag <name>`.
    pub fn create_user_tag(
        &self,
        name: &str,
        target: &CommitId,
        message: Option<&str>,
    ) -> Result<TagInfo, CoreError> {
        if !is_user_tag_name(name) {
            return Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!(
                    "invalid tag name {name:?}: 1 to {MAX_NAME} letters, digits, '.', '_' or '-' \
                     (automatic tags live under {AUTO_PREFIX})"
                ),
            ));
        }
        let message = message
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map_or_else(|| format!("tag {name}"), str::to_owned);
        let allow = Allowlist::parse(&self.read_to_string(ALLOWLIST_FILE)?.unwrap_or_default());
        // Tag names and messages are pushed to remotes: both are scanned.
        let findings = scan_text(
            "tag",
            format!("{name}\n{message}").as_bytes(),
            false,
            &self.secrets,
            &allow,
        );
        if let Some(err) = leak_error(&findings) {
            return Err(err);
        }
        let _lock = StoreLock::acquire(self.fs, &self.state_dir, LOCK_TIMEOUT)?;
        self.ensure_free(name)?;
        let machine = Machine::load_or_create(&self.env, self.fs, &self.state_dir)?;
        let signature = machine.signature(self.env.now())?;
        let object = self
            .repo
            .find_object(oid(target)?, Some(ObjectType::Commit))
            .map_err(git_error)?;
        self.repo
            .tag(name, &object, &signature, &format!("{message}\n"), false)
            .map_err(git_error)?;
        self.tag(name)?.ok_or_else(|| {
            CoreError::new(ErrorCode::GitError, format!("tag {name:?} was not created"))
        })
    }

    /// Creates `auto/<kind>/<timestamp>` on HEAD; see
    /// [`Store::create_auto_tag_at`].
    pub fn create_auto_tag(&self, kind: &str) -> Result<String, CoreError> {
        self.create_auto_tag_at(kind, &self.head()?)
    }

    /// Creates `auto/<kind>/<timestamp>` (with a `-N` suffix when taken)
    /// on `target` and prunes automatic tags older than
    /// [`AUTO_TAG_RETENTION`] (best effort). Returns the new tag's name.
    pub fn create_auto_tag_at(&self, kind: &str, target: &CommitId) -> Result<String, CoreError> {
        let now = self.env.now();
        let base = format!("{AUTO_PREFIX}{kind}/{}", backup::timestamp(now));
        let name = if self.tag_exists(&base) {
            (2..)
                .map(|n| format!("{base}-{n}"))
                .find(|candidate| !self.tag_exists(candidate))
                .expect("an unused suffix exists")
        } else {
            base
        };
        self.create_tag(&name, target)?;
        self.prune_auto_tags(now, AUTO_TAG_RETENTION)?;
        Ok(name)
    }

    /// Deletes automatic tags made more than `retention` before `now`.
    /// User tags and automatic tags whose time cannot be read are kept.
    /// Returns the deleted names.
    pub fn prune_auto_tags(
        &self,
        now: SystemTime,
        retention: Duration,
    ) -> Result<Vec<String>, CoreError> {
        let mut pruned = Vec::new();
        for tag in self.tags()? {
            let old = tag.kind == TagKind::Auto
                && tag
                    .time
                    .is_some_and(|time| now.duration_since(time).is_ok_and(|age| age > retention));
            if old && self.repo.tag_delete(&tag.name).is_ok() {
                pruned.push(tag.name);
            }
        }
        Ok(pruned)
    }

    /// Every tag that points at a commit, user tags first, then automatic
    /// ones, each sorted by name. Tags that do not resolve to a commit are
    /// left out.
    pub fn tags(&self) -> Result<Vec<TagInfo>, CoreError> {
        let names = self.repo.tag_names(None).map_err(git_error)?;
        let mut tags = Vec::new();
        for name in names.iter().filter_map(|name| name.ok().flatten()) {
            if let Ok(Some(tag)) = self.tag(name) {
                tags.push(tag);
            }
        }
        tags.sort_by(|a, b| {
            (a.kind == TagKind::Auto, &a.name).cmp(&(b.kind == TagKind::Auto, &b.name))
        });
        Ok(tags)
    }

    /// The tag `name`, if it exists.
    pub fn tag(&self, name: &str) -> Result<Option<TagInfo>, CoreError> {
        let Ok(reference) = self.repo.find_reference(&format!("refs/tags/{name}")) else {
            return Ok(None);
        };
        let commit = reference.peel_to_commit().map_err(git_error)?;
        let kind = if name.starts_with(AUTO_PREFIX) {
            TagKind::Auto
        } else {
            TagKind::User
        };
        let annotated = reference
            .target()
            .and_then(|oid| self.repo.find_tag(oid).ok());
        let (message, tagger, tagger_time) = match &annotated {
            Some(tag) => {
                let tagger = tag.tagger();
                (
                    tag.message()
                        .ok()
                        .flatten()
                        .map(|message| message.trim_end().to_owned()),
                    tagger
                        .as_ref()
                        .and_then(|signature| signature.name().ok().map(str::to_owned)),
                    tagger.as_ref().and_then(|signature| {
                        u64::try_from(signature.when().seconds())
                            .ok()
                            .map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds))
                    }),
                )
            }
            None => (None, None, None),
        };
        let time = tagger_time.or_else(|| match kind {
            TagKind::Auto => auto_tag_time(name),
            TagKind::User => None,
        });
        Ok(Some(TagInfo {
            name: name.to_owned(),
            kind,
            target: CommitId::from_oid(commit.id()),
            message,
            tagger,
            time,
        }))
    }

    /// Deletes the tag `name`; `not_found` when there is none.
    pub fn delete_tag(&self, name: &str) -> Result<(), CoreError> {
        let _lock = StoreLock::acquire(self.fs, &self.state_dir, LOCK_TIMEOUT)?;
        if !self.tag_exists(name) {
            return Err(CoreError::new(
                ErrorCode::NotFound,
                format!("no tag {name:?}"),
            ));
        }
        self.repo.tag_delete(name).map_err(git_error)
    }

    fn ensure_free(&self, name: &str) -> Result<(), CoreError> {
        if self.tag_exists(name) {
            return Err(CoreError::new(
                ErrorCode::AlreadyExists,
                format!("tag {name:?} already exists; tags never move"),
            ));
        }
        Ok(())
    }
}

/// The time in `auto/<kind>/<timestamp>[-N]`.
fn auto_tag_time(name: &str) -> Option<SystemTime> {
    let last = name.rsplit('/').next()?;
    let stamp = last.split_once('-').map_or(last, |(stamp, _)| stamp);
    backup::parse_timestamp(stamp)
}

fn oid(id: &CommitId) -> Result<Oid, CoreError> {
    Oid::from_str(id.as_str()).map_err(git_error)
}

//! Store side of skills (SPEC 12.1): content in `skills/<id>/`, metadata
//! in `skill-meta/<id>.toml`, kept apart so agents never see the metadata.

use std::path::{Path, PathBuf};

use crate::error::{CoreError, ErrorCode};
use crate::model::TomlDocument;
use crate::model::ids::{self, IdKind};
use crate::model::settings::Settings;
use crate::model::skill_meta::SkillMeta;
use crate::skills::frontmatter::{self, Frontmatter};
use crate::skills::{SKILL_FILE, SkillFiles, WARN_TOTAL_BYTES, files_size, io_error, read_folder};
use crate::store::{Store, StoreRead, Transaction};

/// Skill content folder in the store.
pub const SKILLS_DIR: &str = "skills";
/// Skill metadata folder in the store.
pub const META_DIR: &str = "skill-meta";
/// Portable settings file in the store.
const SETTINGS_FILE: &str = "settings.toml";

/// `skill-meta/<id>.toml`, after checking `id`.
pub fn meta_path(id: &str) -> Result<String, CoreError> {
    ids::check(IdKind::Skill, id)?;
    Ok(format!("{META_DIR}/{id}.toml"))
}

/// `<store>/skills/<id>`, after checking `id`.
pub fn content_dir(store: &Store<'_>, id: &str) -> Result<PathBuf, CoreError> {
    ids::check(IdKind::Skill, id)?;
    Ok(store.root().join(SKILLS_DIR).join(id))
}

/// One skill in the store.
#[derive(Debug, Clone)]
pub struct StoreSkill {
    pub id: String,
    /// `skill-meta/<id>.toml`, when present and valid.
    pub meta: Option<SkillMeta>,
    /// The `SKILL.md` frontmatter; `None` when the file or its frontmatter
    /// is missing.
    pub frontmatter: Option<Frontmatter>,
    /// Total size of the content.
    pub bytes: u64,
    /// Whether the content folder exists.
    pub has_content: bool,
    /// What is wrong with the skill (no `SKILL.md`, invalid metadata or
    /// frontmatter, no content folder).
    pub problems: Vec<CoreError>,
}

impl StoreSkill {
    /// The name to show: frontmatter `name`, else the metadata's, else the id.
    pub fn name(&self) -> &str {
        self.frontmatter
            .as_ref()
            .and_then(Frontmatter::name)
            .or_else(|| self.meta.as_ref().and_then(|meta| meta.name.as_deref()))
            .unwrap_or(&self.id)
    }

    pub fn description(&self) -> Option<&str> {
        self.frontmatter.as_ref().and_then(Frontmatter::description)
    }
}

/// The metadata of `id` as `read` sees it; `None` when absent.
pub fn read_meta(read: &dyn StoreRead, id: &str) -> Result<Option<SkillMeta>, CoreError> {
    let relative = meta_path(id)?;
    let Some(text) = read.read_file(&relative)? else {
        return Ok(None);
    };
    let doc = TomlDocument::<SkillMeta>::parse(&text)
        .map_err(|diagnostic| diagnostic.to_core_error(Some(Path::new(&relative))))?;
    if let Some(err) = doc
        .value()
        .validate(Some(id))
        .first_error(Some(Path::new(&relative)))
    {
        return Err(err);
    }
    Ok(Some(doc.into_value()))
}

/// Stages `meta` (validated at commit).
pub fn stage_meta(tx: &mut Transaction<'_>, meta: &SkillMeta) -> Result<(), CoreError> {
    let relative = meta_path(&meta.id)?;
    let text = TomlDocument::new(meta.clone()).to_text()?;
    tx.write(&relative, text)
}

/// Stages `files` as the content of `id`: files that are not in `files`
/// any more are removed.
pub fn stage_content(
    tx: &mut Transaction<'_>,
    id: &str,
    files: &SkillFiles,
) -> Result<(), CoreError> {
    for relative in worktree_files(tx.store(), id)? {
        if !files.contains_key(&relative) {
            tx.remove(&format!("{SKILLS_DIR}/{id}/{relative}"))?;
        }
    }
    for (relative, bytes) in files {
        tx.write(&format!("{SKILLS_DIR}/{id}/{relative}"), bytes.clone())?;
    }
    Ok(())
}

/// Stages the removal of the content and metadata of `id`.
pub fn stage_remove(tx: &mut Transaction<'_>, id: &str) -> Result<(), CoreError> {
    for relative in worktree_files(tx.store(), id)? {
        tx.remove(&format!("{SKILLS_DIR}/{id}/{relative}"))?;
    }
    let meta = meta_path(id)?;
    if tx.exists(&meta)? {
        tx.remove(&meta)?;
    }
    Ok(())
}

/// Removes the empty folders that removed files leave in the working tree
/// of `id` (git does not track folders), the skill folder itself too when
/// nothing is left in it.
pub fn prune_empty_dirs(store: &Store<'_>, id: &str) -> Result<(), CoreError> {
    let dir = content_dir(store, id)?;
    if store.fs().is_dir(&dir) && prune(store.fs(), &dir)? {
        store
            .fs()
            .remove_dir_all(&dir)
            .map_err(|err| io_error("cannot remove folder", &dir, &err))?;
    }
    Ok(())
}

/// Removes the empty folders below `dir`; whether `dir` is empty now.
fn prune(fs: &dyn crate::fs::Fs, dir: &Path) -> Result<bool, CoreError> {
    let mut empty = true;
    for path in fs
        .read_dir(dir)
        .map_err(|err| io_error("cannot list directory", dir, &err))?
    {
        if fs.is_dir(&path) && prune(fs, &path)? {
            fs.remove_dir_all(&path)
                .map_err(|err| io_error("cannot remove folder", &path, &err))?;
        } else {
            empty = false;
        }
    }
    Ok(empty)
}

/// Relative paths of the files in the store folder of `id`.
fn worktree_files(store: &Store<'_>, id: &str) -> Result<Vec<String>, CoreError> {
    let dir = content_dir(store, id)?;
    let mut out = Vec::new();
    collect(store, &dir, "", &mut out)?;
    Ok(out)
}

fn collect(
    store: &Store<'_>,
    dir: &Path,
    prefix: &str,
    out: &mut Vec<String>,
) -> Result<(), CoreError> {
    let fs = store.fs();
    if !fs.is_dir(dir) {
        return Ok(());
    }
    let entries = fs
        .read_dir(dir)
        .map_err(|err| io_error("cannot list directory", dir, &err))?;
    for path in entries {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let relative = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        if fs.is_dir(&path) {
            collect(store, &path, &relative, out)?;
        } else {
            out.push(relative);
        }
    }
    Ok(())
}

/// The content of `id` as stored.
pub fn read_content(store: &Store<'_>, id: &str) -> Result<SkillFiles, CoreError> {
    let dir = content_dir(store, id)?;
    if !store.fs().is_dir(&dir) {
        return Err(not_found(id));
    }
    read_folder(store.fs(), &dir)
}

/// Ids of every skill: content folders and metadata files, sorted.
pub fn ids(store: &Store<'_>) -> Result<Vec<String>, CoreError> {
    let fs = store.fs();
    let mut ids: Vec<String> = Vec::new();
    let dir = store.root().join(SKILLS_DIR);
    if fs.is_dir(&dir) {
        for path in fs
            .read_dir(&dir)
            .map_err(|err| io_error("cannot list directory", &dir, &err))?
        {
            if fs.is_dir(&path)
                && let Some(name) = path.file_name().and_then(|name| name.to_str())
                && ids::is_valid(IdKind::Skill, name)
            {
                ids.push(name.to_owned());
            }
        }
    }
    for name in store.list_files(META_DIR)? {
        if let Some(id) = name.strip_suffix(".toml")
            && ids::is_valid(IdKind::Skill, id)
            && !ids.iter().any(|known| known == id)
        {
            ids.push(id.to_owned());
        }
    }
    ids.sort();
    Ok(ids)
}

/// Every skill in the store.
pub fn list(store: &Store<'_>) -> Result<Vec<StoreSkill>, CoreError> {
    ids(store)?.into_iter().map(|id| load(store, &id)).collect()
}

/// The skill `id`; `None` when it has neither content nor metadata.
pub fn get(store: &Store<'_>, id: &str) -> Result<Option<StoreSkill>, CoreError> {
    let dir = content_dir(store, id)?;
    if !store.fs().is_dir(&dir) && !store.fs().exists(&store.path(&meta_path(id)?)?) {
        return Ok(None);
    }
    load(store, id).map(Some)
}

fn load(store: &Store<'_>, id: &str) -> Result<StoreSkill, CoreError> {
    let fs = store.fs();
    let dir = content_dir(store, id)?;
    let mut problems = Vec::new();
    let meta = match read_meta(store, id) {
        Ok(meta) => meta,
        Err(err) => {
            problems.push(err);
            None
        }
    };
    let has_content = fs.is_dir(&dir);
    let mut bytes = 0;
    let mut frontmatter = None;
    if has_content {
        match read_folder(fs, &dir) {
            Ok(files) => {
                bytes = files_size(&files);
                match files.get(SKILL_FILE) {
                    Some(text) => match frontmatter::parse(&String::from_utf8_lossy(text)) {
                        Ok(parsed) => frontmatter = parsed,
                        Err(err) => problems.push(err.with_path(dir.join(SKILL_FILE))),
                    },
                    None => problems.push(missing_skill_file(&dir)),
                }
            }
            Err(err) => problems.push(err),
        }
    } else {
        problems.push(
            CoreError::new(
                ErrorCode::NotFound,
                format!("skill {id:?} has metadata but no content folder"),
            )
            .with_path(&dir),
        );
    }
    Ok(StoreSkill {
        id: id.to_owned(),
        meta,
        frontmatter,
        bytes,
        has_content,
        problems,
    })
}

pub(crate) fn missing_skill_file(dir: &Path) -> CoreError {
    CoreError::new(
        ErrorCode::NotFound,
        format!("the skill has no {SKILL_FILE}"),
    )
    .with_path(dir)
}

pub(crate) fn not_found(id: &str) -> CoreError {
    CoreError::new(ErrorCode::NotFound, format!("skill {id:?} does not exist"))
}

/// Whether `id` is taken (content or metadata).
pub fn exists(read: &Transaction<'_>, id: &str) -> Result<bool, CoreError> {
    let store = read.store();
    Ok(store.fs().is_dir(&content_dir(store, id)?) || read.exists(&meta_path(id)?)?)
}

/// `base`, or `base-2`, `base-3`, ... when taken (SPEC 12.1).
pub fn free_id(read: &Transaction<'_>, base: &str) -> Result<String, CoreError> {
    if !exists(read, base)? {
        return Ok(base.to_owned());
    }
    for n in 2..1000 {
        let candidate = format!("{base}-{n}");
        if ids::is_valid(IdKind::Skill, &candidate) && !exists(read, &candidate)? {
            return Ok(candidate);
        }
    }
    Err(CoreError::new(
        ErrorCode::AlreadyExists,
        format!("no free id for skill {base:?}"),
    ))
}

/// A warning when the store's skills together pass [`WARN_TOTAL_BYTES`].
pub fn total_size_warning(store: &Store<'_>) -> Option<CoreError> {
    let total: u64 = list(store).ok()?.iter().map(|skill| skill.bytes).sum();
    (total > WARN_TOTAL_BYTES).then(|| {
        CoreError::new(
            ErrorCode::InvalidValue,
            format!(
                "store skills take {} MB, more than {} MB",
                total / (1024 * 1024),
                WARN_TOTAL_BYTES / (1024 * 1024)
            ),
        )
    })
}

/// `settings.toml`, or defaults when absent or unreadable.
pub fn settings(read: &dyn StoreRead) -> Settings {
    read.read_file(SETTINGS_FILE)
        .ok()
        .flatten()
        .and_then(|text| TomlDocument::<Settings>::parse(&text).ok())
        .map(TomlDocument::into_value)
        .unwrap_or_default()
}

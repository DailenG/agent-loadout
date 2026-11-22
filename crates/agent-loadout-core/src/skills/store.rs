//! Store side of skills (SPEC 12.1): content in `skills/<id>/`, metadata
//! in `skill-meta/<id>.toml`, kept apart so agents never see the metadata.

use std::path::{Path, PathBuf};

use crate::error::{CoreError, ErrorCode};
use crate::model::TomlDocument;
use crate::model::ids::{self, IdKind};
use crate::model::settings::Settings;
use crate::model::skill_meta::SkillMeta;
use crate::skills::frontmatter::{self, Frontmatter};
use crate::skills::link::remove_link;
use crate::skills::{
    Entry, SKILL_FILE, SkillFiles, WARN_TOTAL_BYTES, entry, files_size, io_error, read_folder,
};
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
    /// Whether the content folder exists (a real folder, not a link) and
    /// reads without error; only such skills are distributed.
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
/// any more are removed. A new file below a link in the store folder (put
/// there by hand or by a pulled commit) is `invalid_path`: writing it would
/// write through the link.
pub fn stage_content(
    tx: &mut Transaction<'_>,
    id: &str,
    files: &SkillFiles,
) -> Result<(), CoreError> {
    let tree = worktree(tx.store(), id)?;
    if let Some(link) = tree.links.iter().find(|link| {
        files
            .keys()
            .any(|path| link.is_empty() || path == *link || path.starts_with(&format!("{link}/")))
    }) {
        let dir = content_dir(tx.store(), id)?;
        return Err(CoreError::new(
            ErrorCode::InvalidPath,
            "the skill's store folder holds a link where a skill file goes; remove the link",
        )
        .with_path(link.split('/').fold(dir, |path, part| path.join(part))));
    }
    for relative in tree.files {
        if !files.contains_key(&relative) {
            tx.remove(&format!("{SKILLS_DIR}/{id}/{relative}"))?;
        }
    }
    for (relative, bytes) in files {
        tx.write(&format!("{SKILLS_DIR}/{id}/{relative}"), bytes.clone())?;
    }
    Ok(())
}

/// Stages the removal of the content and metadata of `id`. Links in the
/// store folder are not store files: [`prune_empty_dirs`] removes them
/// (the link itself) after the commit.
pub fn stage_remove(tx: &mut Transaction<'_>, id: &str) -> Result<(), CoreError> {
    for relative in worktree(tx.store(), id)?.files {
        tx.remove(&format!("{SKILLS_DIR}/{id}/{relative}"))?;
    }
    let meta = meta_path(id)?;
    if tx.exists(&meta)? {
        tx.remove(&meta)?;
    }
    Ok(())
}

/// Removes what removed files leave in the working tree of `id`: empty
/// folders (git does not track folders) and links (each removed as a
/// link, never followed), the skill folder itself too when nothing is
/// left in it.
pub fn prune_empty_dirs(store: &Store<'_>, id: &str) -> Result<(), CoreError> {
    let dir = content_dir(store, id)?;
    let fs = store.fs();
    match entry(fs, &dir)? {
        Entry::Link(_) => remove_link(fs, &dir),
        Entry::Dir if prune(fs, &dir)? => fs
            .remove_dir_all(&dir)
            .map_err(|err| io_error("cannot remove folder", &dir, &err)),
        _ => Ok(()),
    }
}

/// Removes links and empty folders below `dir`; whether `dir` is empty
/// now.
fn prune(fs: &dyn crate::fs::Fs, dir: &Path) -> Result<bool, CoreError> {
    let mut empty = true;
    for path in fs
        .read_dir(dir)
        .map_err(|err| io_error("cannot list directory", dir, &err))?
    {
        match entry(fs, &path)? {
            Entry::Link(_) => remove_link(fs, &path)?,
            Entry::Dir if prune(fs, &path)? => fs
                .remove_dir_all(&path)
                .map_err(|err| io_error("cannot remove folder", &path, &err))?,
            Entry::Missing => {}
            Entry::Dir | Entry::File => empty = false,
        }
    }
    Ok(empty)
}

/// The store folder of a skill as it is on disk: its files, and its links
/// (relative paths; `""` when the folder itself is a link). Links are
/// never followed.
struct Worktree {
    files: Vec<String>,
    links: Vec<String>,
}

fn worktree(store: &Store<'_>, id: &str) -> Result<Worktree, CoreError> {
    let dir = content_dir(store, id)?;
    let mut tree = Worktree {
        files: Vec::new(),
        links: Vec::new(),
    };
    match entry(store.fs(), &dir)? {
        Entry::Link(_) => tree.links.push(String::new()),
        Entry::Dir => collect(store, &dir, "", &mut tree)?,
        Entry::Missing | Entry::File => {}
    }
    Ok(tree)
}

fn collect(
    store: &Store<'_>,
    dir: &Path,
    prefix: &str,
    tree: &mut Worktree,
) -> Result<(), CoreError> {
    let fs = store.fs();
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
        match entry(fs, &path)? {
            Entry::Link(_) => tree.links.push(relative),
            Entry::Dir => collect(store, &path, &relative, tree)?,
            Entry::File => tree.files.push(relative),
            Entry::Missing => {}
        }
    }
    Ok(())
}

/// The content of `id` as stored. A store folder that is itself a link is
/// `invalid_path` (its content lives elsewhere).
pub fn read_content(store: &Store<'_>, id: &str) -> Result<SkillFiles, CoreError> {
    let dir = content_dir(store, id)?;
    match entry(store.fs(), &dir)? {
        Entry::Dir => read_folder(store.fs(), &dir),
        Entry::Link(_) => Err(CoreError::new(
            ErrorCode::InvalidPath,
            format!("the store folder of skill {id:?} is a link"),
        )
        .with_path(&dir)),
        Entry::Missing | Entry::File => Err(not_found(id)),
    }
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
    let found = entry(fs, &dir)?;
    let mut has_content = found == Entry::Dir;
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
            Err(err) => {
                // Unreadable (a link that escapes, too large): never
                // distributed.
                has_content = false;
                problems.push(err);
            }
        }
    } else if let Entry::Link(_) = found {
        problems.push(
            CoreError::new(
                ErrorCode::InvalidPath,
                format!("the store folder of skill {id:?} is a link; it is never distributed"),
            )
            .with_path(&dir),
        );
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

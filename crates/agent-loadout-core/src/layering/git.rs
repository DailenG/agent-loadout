//! The project's git repository, for the private-file rules of SPEC 6.5:
//! whether a client file is tracked or ignored, and hiding an untracked
//! private file through `info/exclude`. git2 needs a real repository on
//! disk, so these work on real paths.

use std::path::{Path, PathBuf};

use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;

/// The work tree holding a project folder. Its folders are spelled like the
/// folder it was discovered from: git2 reports resolved paths (symlinks
/// such as macOS `/var`, Windows 8.3 short names expanded), and files the
/// caller reads or writes through [`Fs`] stay under the spelling it uses.
pub struct ProjectRepo {
    repo: git2::Repository,
    workdir: PathBuf,
    commondir: PathBuf,
}

impl std::fmt::Debug for ProjectRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectRepo")
            .field("workdir", &self.workdir)
            .finish_non_exhaustive()
    }
}

fn git_error(err: git2::Error) -> CoreError {
    CoreError::new(ErrorCode::GitError, err.message().to_owned())
}

impl ProjectRepo {
    /// The repository whose work tree holds `root`; `None` when there is no
    /// repository there or it is bare. Any other failure (a repository git
    /// refuses to open, such as one owned by another user, or a broken
    /// config) is `git_error`: whether a file is tracked is then unknown.
    pub fn discover(root: &Path) -> Result<Option<Self>, CoreError> {
        let repo = match git2::Repository::discover(root) {
            Ok(repo) => repo,
            Err(err) if err.code() == git2::ErrorCode::NotFound => return Ok(None),
            Err(err) => return Err(git_error(err).with_path(root)),
        };
        let Some(workdir) = repo.workdir().map(|dir| spelled_like(dir, root)) else {
            return Ok(None);
        };
        let commondir = spelled_like(repo.commondir(), root);
        Ok(Some(Self {
            repo,
            workdir,
            commondir,
        }))
    }

    /// The work tree root.
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// `path` relative to the work tree, `/`-separated; `None` when it lies
    /// outside.
    pub fn relative(&self, path: &Path) -> Option<String> {
        let rest = match path.strip_prefix(&self.workdir) {
            Ok(rest) => rest.to_path_buf(),
            Err(_) => {
                // Spellings differ (8.3 names, symlinks): compare the
                // canonical forms of the deepest existing ancestors.
                let base = std::fs::canonicalize(&self.workdir).ok()?;
                let (existing, tail) = existing_ancestor(path)?;
                let full = std::fs::canonicalize(existing).ok()?.join(tail);
                full.strip_prefix(&base).ok()?.to_path_buf()
            }
        };
        let parts: Vec<String> = rest
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        (!parts.is_empty()).then(|| parts.join("/"))
    }

    /// Whether the index holds `path` (it is tracked).
    pub fn is_tracked(&self, path: &Path) -> Result<bool, CoreError> {
        let Some(relative) = self.relative(path) else {
            return Ok(false);
        };
        let index = self.repo.index().map_err(git_error)?;
        Ok(index.get_path(Path::new(&relative), 0).is_some())
    }

    /// Whether the index holds `path` or any file below it (a folder with
    /// tracked files).
    pub fn tracks_any_under(&self, path: &Path) -> Result<bool, CoreError> {
        let Some(relative) = self.relative(path) else {
            return Ok(false);
        };
        let prefix = format!("{relative}/");
        let index = self.repo.index().map_err(git_error)?;
        Ok(index.iter().any(|entry| {
            let tracked = String::from_utf8_lossy(&entry.path);
            tracked == relative.as_str() || tracked.starts_with(&prefix)
        }))
    }

    /// Whether the repository's ignore rules match `path`.
    pub fn is_ignored(&self, path: &Path) -> Result<bool, CoreError> {
        let Some(relative) = self.relative(path) else {
            return Ok(false);
        };
        self.repo
            .is_path_ignored(Path::new(&relative))
            .map_err(git_error)
    }

    /// `info/exclude` of the repository. Linked work trees share the one in
    /// the common directory.
    pub fn exclude_file(&self) -> PathBuf {
        self.commondir.join("info").join("exclude")
    }

    /// The files whose content [`ProjectRepo::is_tracked`] and
    /// [`ProjectRepo::is_ignored`] of `path` depend on: the index,
    /// `info/exclude` and the `.gitignore` of each folder from the work
    /// tree root down to `path`'s (git's global excludes file aside).
    pub fn state_files(&self, path: &Path) -> Vec<PathBuf> {
        let gitdir = spelled_like(self.repo.path(), &self.workdir);
        let mut files = vec![gitdir.join("index"), self.exclude_file()];
        let mut dir = self.workdir.clone();
        files.push(dir.join(".gitignore"));
        if let Some(relative) = self.relative(path) {
            let parts: Vec<&str> = relative.split('/').collect();
            for part in &parts[..parts.len().saturating_sub(1)] {
                dir.push(part);
                files.push(dir.join(".gitignore"));
            }
        }
        files
    }

    /// Hides `path` from git by appending `/<relative path>` to
    /// `info/exclude` (creating it). Does nothing when the path is outside
    /// the work tree, already ignored or already listed. Returns whether
    /// the file was changed.
    pub fn exclude(&self, fs: &dyn Fs, path: &Path) -> Result<bool, CoreError> {
        let Some(relative) = self.relative(path) else {
            return Ok(false);
        };
        if self.is_ignored(path)? {
            return Ok(false);
        }
        let pattern = format!("/{relative}");
        let file = self.exclude_file();
        let io = |what: &str, at: &Path, err: std::io::Error| {
            CoreError::new(ErrorCode::IoError, format!("{what}: {err}")).with_path(at)
        };
        let mut text = if fs.exists(&file) {
            fs.read_to_string(&file)
                .map_err(|err| io("cannot read file", &file, err))?
        } else {
            String::new()
        };
        if text
            .lines()
            .any(|line| line.trim() == pattern || line.trim() == relative)
        {
            return Ok(false);
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&pattern);
        text.push('\n');
        crate::apply::write_atomic_project(fs, &file, text.as_bytes())?;
        Ok(true)
    }
}

/// `path`, an existing folder as git2 reports it, spelled from the deepest
/// ancestor-or-self of `root` it lies under once both are resolved; as it
/// is when it already shares a spelling with `root` or cannot be resolved.
fn spelled_like(path: &Path, root: &Path) -> PathBuf {
    if root.starts_with(path) || path.starts_with(root) {
        return path.to_path_buf();
    }
    let Ok(real) = std::fs::canonicalize(path) else {
        return path.to_path_buf();
    };
    for base in root.ancestors() {
        let Ok(real_base) = std::fs::canonicalize(base) else {
            continue;
        };
        if let Ok(rest) = real.strip_prefix(&real_base) {
            return if rest.as_os_str().is_empty() {
                base.to_path_buf()
            } else {
                base.join(rest)
            };
        }
    }
    path.to_path_buf()
}

/// The deepest existing ancestor-or-self of `path` and the rest below it.
fn existing_ancestor(path: &Path) -> Option<(&Path, PathBuf)> {
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    let mut current = path;
    loop {
        if current.exists() {
            let mut rest = PathBuf::new();
            for part in tail.iter().rev() {
                rest.push(part);
            }
            return Some((current, rest));
        }
        tail.push(current.file_name()?);
        current = current.parent()?;
    }
}

//! The project's git repository, for the private-file rules of SPEC 6.5:
//! whether a client file is tracked or ignored, and hiding an untracked
//! private file through `info/exclude`. git2 needs a real repository on
//! disk, so these work on real paths.

use std::path::{Path, PathBuf};

use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;

/// The work tree holding a project folder.
pub struct ProjectRepo {
    repo: git2::Repository,
    workdir: PathBuf,
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
        let Some(workdir) = repo.workdir().map(Path::to_path_buf) else {
            return Ok(None);
        };
        Ok(Some(Self { repo, workdir }))
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
        self.repo.commondir().join("info").join("exclude")
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
        crate::apply::write_atomic(fs, &file, text.as_bytes())?;
        Ok(true)
    }
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

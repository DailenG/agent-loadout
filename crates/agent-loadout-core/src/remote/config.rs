//! `state/remotes.toml` (SPEC 9.6): this machine's remotes. Machine-local
//! and never synced, because URLs, mount paths and auth differ per
//! machine; the store, the spec and the binary carry no remote. No remote
//! exists until the user adds one, and none is ranked: they keep the order
//! they were added in.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::SCHEMA_VERSION;
use crate::remote::git::GitAuth;

/// The file in `state/`.
pub const REMOTES_FILE: &str = "remotes.toml";
/// Archives kept by an `archive` folder remote unless `keep` says otherwise.
pub const DEFAULT_KEEP: u32 = 10;

/// What a remote is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteKind {
    /// Any git URL, through the system git (M13).
    Git,
    /// A local or network directory.
    Folder,
}

impl RemoteKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            RemoteKind::Git => "git",
            RemoteKind::Folder => "folder",
        }
    }
}

/// How a folder remote holds a machine's store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FolderMode {
    /// A bare repository per machine (`store.git`), pulled from.
    #[default]
    RepoPerMachine,
    /// The HEAD tree as plain files (`latest/`), export only.
    Mirror,
    /// Zips of the HEAD tree (`archives/`), export only.
    Archive,
}

impl FolderMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            FolderMode::RepoPerMachine => "repo-per-machine",
            FolderMode::Mirror => "mirror",
            FolderMode::Archive => "archive",
        }
    }

    pub fn parse(text: &str) -> Result<Self, CoreError> {
        match text {
            "repo-per-machine" => Ok(Self::RepoPerMachine),
            "mirror" => Ok(Self::Mirror),
            "archive" => Ok(Self::Archive),
            other => Err(CoreError::new(
                ErrorCode::InvalidValue,
                format!(
                    "unknown folder mode {other:?}; expected repo-per-machine, mirror or archive"
                ),
            )),
        }
    }
}

/// One `[[remote]]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteConfig {
    pub name: String,
    pub kind: RemoteKind,
    /// Folder remotes: the path as the user typed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<FolderMode>,
    /// Archive mode: how many archives to keep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<u32>,
    /// Git remotes (M13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// `manual`, `on-commit` or `interval:15m` (SPEC 9.8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<String>,
    /// `manual`, `on-start` or `interval:15m`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull: Option<String>,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl RemoteConfig {
    /// A folder remote at `path`.
    pub fn folder(name: &str, path: &str, mode: FolderMode) -> Self {
        Self {
            name: name.to_owned(),
            kind: RemoteKind::Folder,
            path: Some(path.to_owned()),
            mode: Some(mode),
            keep: None,
            url: None,
            auth: None,
            username: None,
            push: Some("on-commit".to_owned()),
            pull: Some("manual".to_owned()),
            extra: toml::Table::new(),
        }
    }

    /// A git remote at `url` (SPEC 9.6 example: push on commit, pull on
    /// start).
    pub fn git(name: &str, url: &str, auth: GitAuth) -> Self {
        Self {
            name: name.to_owned(),
            kind: RemoteKind::Git,
            path: None,
            mode: None,
            keep: None,
            url: Some(url.to_owned()),
            auth: Some(auth.as_str().to_owned()),
            username: None,
            push: Some("on-commit".to_owned()),
            pull: Some("on-start".to_owned()),
            extra: toml::Table::new(),
        }
    }

    pub fn folder_mode(&self) -> FolderMode {
        self.mode.unwrap_or_default()
    }
}

/// `state/remotes.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemotesFile {
    pub schema: i64,
    #[serde(default, rename = "remote")]
    pub remotes: Vec<RemoteConfig>,
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl Default for RemotesFile {
    fn default() -> Self {
        Self {
            schema: SCHEMA_VERSION,
            remotes: Vec::new(),
            extra: toml::Table::new(),
        }
    }
}

impl RemotesFile {
    /// The remotes of this machine; none when the file is absent.
    pub fn load(fs: &dyn Fs, state_dir: &Path) -> Result<Self, CoreError> {
        let path = state_dir.join(REMOTES_FILE);
        if !fs.exists(&path) {
            return Ok(Self::default());
        }
        let text = fs.read_to_string(&path).map_err(|err| {
            CoreError::new(ErrorCode::IoError, format!("cannot read file: {err}")).with_path(&path)
        })?;
        let file: Self = toml::from_str(&text).map_err(|err| {
            CoreError::new(ErrorCode::ParseError, err.message().to_owned()).with_path(&path)
        })?;
        if file.schema != SCHEMA_VERSION {
            return Err(CoreError::new(
                ErrorCode::UnsupportedSchema,
                format!("schema {} is not read by this build", file.schema),
            )
            .with_path(&path));
        }
        Ok(file)
    }

    pub fn save(&self, fs: &dyn Fs, state_dir: &Path) -> Result<(), CoreError> {
        let text = toml::to_string_pretty(self).map_err(|err| {
            CoreError::new(
                ErrorCode::InvalidValue,
                format!("cannot write remotes: {err}"),
            )
        })?;
        crate::apply::write_atomic(fs, &state_dir.join(REMOTES_FILE), text.as_bytes())
    }

    pub fn get(&self, name: &str) -> Option<&RemoteConfig> {
        self.remotes.iter().find(|remote| remote.name == name)
    }

    /// Adds a remote; `already_exists` when the name is taken.
    pub fn add(&mut self, remote: RemoteConfig) -> Result<(), CoreError> {
        check_name(&remote.name)?;
        if self.get(&remote.name).is_some() {
            return Err(CoreError::new(
                ErrorCode::AlreadyExists,
                format!("a remote named {:?} exists already", remote.name),
            ));
        }
        self.remotes.push(remote);
        Ok(())
    }

    /// Removes a remote; `not_found` when there is none.
    pub fn remove(&mut self, name: &str) -> Result<RemoteConfig, CoreError> {
        let index = self
            .remotes
            .iter()
            .position(|remote| remote.name == name)
            .ok_or_else(|| not_found(name))?;
        Ok(self.remotes.remove(index))
    }

    /// The named remote, or every remote when `name` is `None`.
    pub fn select(&self, name: Option<&str>) -> Result<Vec<&RemoteConfig>, CoreError> {
        match name {
            Some(name) => self
                .get(name)
                .map(|r| vec![r])
                .ok_or_else(|| not_found(name)),
            None => Ok(self.remotes.iter().collect()),
        }
    }
}

fn not_found(name: &str) -> CoreError {
    CoreError::new(ErrorCode::NotFound, format!("no remote named {name:?}"))
}

/// Remote names: 1 to 64 letters, digits, `.`, `_` or `-`, not starting
/// with `.` or `-`, and valid in a git ref (`refs/remotes/<name>/main`):
/// no `..`, no trailing `.`, no `.lock` ending.
pub fn check_name(name: &str) -> Result<(), CoreError> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && !name.starts_with(['.', '-'])
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.ends_with(".lock");
    if valid {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("invalid remote name {name:?}: letters, digits, '.', '_' or '-'"),
        ))
    }
}

//! Project manifests in a folder (SPEC 5.5, 6.1): discovery of the project
//! root `<P>`, loading and validation of `<P>/.loadout/project.toml` and
//! `<P>/.loadout/project.local.toml`, the project id and the content hash
//! trust is keyed by (SPEC 17).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::brand::PROJECT_DIR;
use crate::env::Env;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::TomlDocument;
use crate::model::project::{ManifestLayer, ProjectManifest};

/// The shared manifest, committed with the project.
pub const SHARED_FILE: &str = "project.toml";
/// The private manifest, never committed.
pub const LOCAL_FILE: &str = "project.local.toml";

/// One manifest file as read from the folder.
#[derive(Debug, Clone, PartialEq)]
pub struct ManifestFile {
    pub path: PathBuf,
    /// The bytes the content hash is computed over.
    pub bytes: Vec<u8>,
    pub manifest: ProjectManifest,
}

/// A project folder and its manifests (at least one of the two).
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    /// `<P>`, absolute.
    pub root: PathBuf,
    /// The shared file's id, else the local file's.
    pub id: String,
    pub shared: Option<ManifestFile>,
    pub local: Option<ManifestFile>,
    /// Validation warnings of both files (`id_mismatch`).
    pub warnings: Vec<CoreError>,
}

/// `<P>/.loadout`.
pub fn manifest_dir(root: &Path) -> PathBuf {
    root.join(PROJECT_DIR)
}

/// The project root for `folder`: the nearest ancestor-or-self directory
/// holding `.loadout/project.toml` or `.loadout/project.local.toml`. A
/// manifest directly in the home directory is ignored (SPEC 6.1), and a
/// folder inside the home directory is never searched above it.
pub fn discover(env: &Env, fs: &dyn Fs, folder: &Path) -> Option<PathBuf> {
    folder
        .ancestors()
        .take_while(|dir| !same_folder(fs, dir, env.home()))
        .find(|dir| {
            let manifests = manifest_dir(dir);
            fs.exists(&manifests.join(SHARED_FILE)) || fs.exists(&manifests.join(LOCAL_FILE))
        })
        .map(Path::to_path_buf)
}

/// The spelling-independent key of a folder path on this machine, for
/// comparing project roots (known projects, trust records, a selected
/// folder). On Windows: without a `\\?\` prefix, `/` separators, lower
/// case, no trailing separator. Elsewhere the path as it is, without a
/// trailing separator.
pub fn path_key(path: &Path) -> String {
    normalize_path_text(cfg!(windows), &path.to_string_lossy())
}

/// Whether `a` and `b` name the same folder ([`path_key`]).
pub fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

/// `path` with symlinks and junctions resolved through `fs`, or as it is
/// when it cannot be resolved (it does not exist).
pub fn resolved(fs: &dyn Fs, path: &Path) -> PathBuf {
    fs.canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Whether `a` and `b` are the same folder once links are resolved: a
/// junction or symlink to the home folder is the home folder (SPEC 6.1,
/// 17).
pub fn same_folder(fs: &dyn Fs, a: &Path, b: &Path) -> bool {
    same_path(a, b) || same_path(&resolved(fs, a), &resolved(fs, b))
}

/// [`path_key`] of a path written as text, with Windows rules when
/// `windows`; also compares Claude Code project keys.
pub(crate) fn normalize_path_text(windows: bool, text: &str) -> String {
    let mut key = if windows {
        let text = match text.strip_prefix(r"\\?\UNC\") {
            Some(rest) => format!(r"\\{rest}"),
            None => text.strip_prefix(r"\\?\").unwrap_or(text).to_owned(),
        };
        text.replace('\\', "/").to_lowercase()
    } else {
        text.to_owned()
    };
    let drive_root = |key: &str| key.len() == 3 && key.as_bytes()[1] == b':';
    while key.len() > 1 && key.ends_with('/') && !(windows && drive_root(&key)) {
        key.pop();
    }
    key
}

/// `sha256:<hex>` over `shared\0<project.toml>\0local\0<project.local.toml>`,
/// an absent file standing as `-absent-` (SPEC 17).
pub fn content_hash(shared: Option<&[u8]>, local: Option<&[u8]>) -> String {
    const ABSENT: &[u8] = b"-absent-";
    let mut hasher = Sha256::new();
    hasher.update(b"shared\0");
    hasher.update(shared.unwrap_or(ABSENT));
    hasher.update(b"\0local\0");
    hasher.update(local.unwrap_or(ABSENT));
    let digest = hasher.finalize();
    let mut text = String::with_capacity(7 + digest.len() * 2);
    text.push_str("sha256:");
    for byte in digest {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

impl Project {
    /// Reads and validates the manifests of `root`; `None` when it has
    /// neither file. A file that does not parse or validate is an error for
    /// the whole project.
    pub fn load(fs: &dyn Fs, root: &Path) -> Result<Option<Self>, CoreError> {
        let dir = manifest_dir(root);
        let shared = read_manifest(fs, &dir.join(SHARED_FILE))?;
        let local = read_manifest(fs, &dir.join(LOCAL_FILE))?;
        if shared.is_none() && local.is_none() {
            return Ok(None);
        }
        let mut warnings = Vec::new();
        let shared_id = shared.as_ref().and_then(|file| file.manifest.id.clone());
        let layers = [
            (shared.as_ref(), ManifestLayer::Shared),
            (
                local.as_ref(),
                ManifestLayer::Local {
                    shared_id: shared.as_ref().map(|_| shared_id.as_deref().unwrap_or("")),
                },
            ),
        ];
        for (file, layer) in layers {
            let Some(file) = file else { continue };
            let diagnostics = file.manifest.validate(layer, None);
            if let Some(err) = diagnostics.first_error(Some(&file.path)) {
                return Err(err);
            }
            warnings.extend(
                diagnostics
                    .warnings()
                    .map(|warning| warning.to_core_error(Some(&file.path))),
            );
        }
        let id = shared_id
            .or_else(|| local.as_ref().and_then(|file| file.manifest.id.clone()))
            .ok_or_else(|| {
                CoreError::new(ErrorCode::MissingField, "the project manifest needs `id`")
                    .with_path(&dir)
            })?;
        Ok(Some(Self {
            root: root.to_path_buf(),
            id,
            shared,
            local,
            warnings,
        }))
    }

    /// The content hash of both files ([`content_hash`]).
    pub fn hash(&self) -> String {
        content_hash(
            self.shared.as_ref().map(|file| file.bytes.as_slice()),
            self.local.as_ref().map(|file| file.bytes.as_slice()),
        )
    }

    /// The project's display name: the manifests' `name` (local over
    /// shared), else its id.
    pub fn name(&self) -> &str {
        [self.local.as_ref(), self.shared.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|file| file.manifest.name.as_deref())
            .unwrap_or(&self.id)
    }
}

fn read_manifest(fs: &dyn Fs, path: &Path) -> Result<Option<ManifestFile>, CoreError> {
    if !fs.exists(path) {
        return Ok(None);
    }
    let bytes = fs.read(path).map_err(|err| {
        CoreError::new(ErrorCode::IoError, format!("cannot read file: {err}")).with_path(path)
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        CoreError::new(ErrorCode::ParseError, "the manifest is not valid UTF-8").with_path(path)
    })?;
    let manifest = TomlDocument::<ProjectManifest>::parse(text)
        .map(TomlDocument::into_value)
        .map_err(|diagnostic| diagnostic.to_core_error(Some(path)))?;
    Ok(Some(ManifestFile {
        path: path.to_path_buf(),
        bytes,
        manifest,
    }))
}

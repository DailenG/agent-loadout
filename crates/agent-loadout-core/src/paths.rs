//! `LOADOUT_HOME` resolution, data layout and sync-root detection (SPEC 4.5).

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf, Prefix};

use crate::brand;
use crate::env::{Env, KnownFolder, Os};
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;

/// The machine-local state directory inside `LOADOUT_HOME`.
pub const STATE_DIR: &str = "state";

/// Where the resolved `LOADOUT_HOME` came from, highest precedence first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeSource {
    /// The `--home` command-line flag.
    Flag,
    /// The `LOADOUT_HOME` environment variable.
    EnvVar,
    /// The per-OS default.
    Default,
}

impl HomeSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            HomeSource::Flag => "flag",
            HomeSource::EnvVar => "env",
            HomeSource::Default => "default",
        }
    }
}

/// File-sync tool owning a folder. Live git repos and lock files must not
/// live inside one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncRootKind {
    Resilio,
    Syncthing,
    OneDrive,
    Dropbox,
    GoogleDrive,
}

impl SyncRootKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            SyncRootKind::Resilio => "resilio",
            SyncRootKind::Syncthing => "syncthing",
            SyncRootKind::OneDrive => "onedrive",
            SyncRootKind::Dropbox => "dropbox",
            SyncRootKind::GoogleDrive => "google_drive",
        }
    }
}

/// A sync folder containing a path. Reported by `loadout doctor` as
/// `sync_root_detected`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRoot {
    pub kind: SyncRootKind,
    /// Top folder managed by the sync tool.
    pub root: PathBuf,
}

/// The resolved data directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadoutHome {
    /// Absolute and lexically normalized.
    pub path: PathBuf,
    pub source: HomeSource,
    /// Set when `path` lies inside a file-sync folder.
    pub sync_root: Option<SyncRoot>,
}

impl LoadoutHome {
    /// Versioned desired state (git repo).
    pub fn store_dir(&self) -> PathBuf {
        self.path.join("store")
    }

    /// Machine-local, unversioned state.
    pub fn state_dir(&self) -> PathBuf {
        self.path.join(STATE_DIR)
    }

    /// Pre-write copies of client files and trash; never pushed.
    pub fn backups_dir(&self) -> PathBuf {
        self.path.join("backups")
    }

    /// Re-creatable caches.
    pub fn cache_dir(&self) -> PathBuf {
        self.path.join("cache")
    }

    /// Migration inputs.
    pub fn imports_dir(&self) -> PathBuf {
        self.path.join("imports")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.path.join("logs")
    }
}

/// Resolves `LOADOUT_HOME`: `flag` (the CLI `--home`), else the
/// `LOADOUT_HOME` variable, else the per-OS default. Overrides must be
/// absolute. The result is checked for an enclosing sync folder.
pub fn loadout_home(env: &Env, fs: &dyn Fs, flag: Option<&Path>) -> Result<LoadoutHome, CoreError> {
    let (path, source) = match (flag, env.var(brand::HOME_ENV_VAR)) {
        (Some(flag), _) => (flag.to_path_buf(), HomeSource::Flag),
        (None, Some(var)) => (PathBuf::from(var), HomeSource::EnvVar),
        (None, None) => (default_loadout_home(env), HomeSource::Default),
    };
    if !path.is_absolute() {
        return Err(CoreError::new(
            ErrorCode::InvalidPath,
            format!("{} must be an absolute path", brand::HOME_ENV_VAR),
        )
        .with_path(path));
    }
    let path = normalize(&path);
    let sync_root = detect_sync_root(env, fs, &path);
    Ok(LoadoutHome {
        path,
        source,
        sync_root,
    })
}

/// The per-OS default data directory. Never reads `HOME` on Windows: the
/// profile comes from [`Env::home`] and the base from `FOLDERID_LocalAppData`.
pub fn default_loadout_home(env: &Env) -> PathBuf {
    match env.os() {
        Os::Windows => env
            .known_folder(KnownFolder::LocalAppData)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| env.home().join("AppData").join("Local"))
            .join(brand::DATA_DIR_WINDOWS),
        Os::Linux => env
            .var("XDG_DATA_HOME")
            .map(PathBuf::from)
            // The XDG spec says relative values are invalid and ignored.
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| env.home().join(".local").join("share"))
            .join(brand::DATA_DIR_LINUX),
        Os::MacOs => env
            .home()
            .join("Library")
            .join("Application Support")
            .join(brand::DATA_DIR_MACOS),
    }
}

/// Finds a file-sync folder containing `path`.
///
/// Walks the ancestors of `path` (stopping at [`Fs::root`]) for a Resilio
/// `.sync` folder holding `ID` or `IgnoreList`, or a Syncthing `.stfolder`.
/// Then compares against provider roots: OneDrive (`OneDrive`,
/// `OneDriveConsumer`, `OneDriveCommercial` variables), Dropbox
/// (`info.json`), Google Drive (`~/My Drive`, `~/Google Drive`, a
/// `<drive>:\My Drive` stream mount) and macOS `~/Library/CloudStorage`.
pub fn detect_sync_root(env: &Env, fs: &dyn Fs, path: &Path) -> Option<SyncRoot> {
    let path = normalize(path);
    marker_root(env.os(), fs, &path).or_else(|| provider_root(env, fs, &path))
}

fn marker_root(os: Os, fs: &dyn Fs, path: &Path) -> Option<SyncRoot> {
    let ceiling = fs.root().map(normalize);
    for dir in path.ancestors() {
        if ceiling
            .as_deref()
            .is_some_and(|ceiling| !is_within(os, dir, ceiling))
        {
            break;
        }
        let resilio = dir.join(".sync");
        if fs.is_dir(&resilio)
            && (fs.exists(&resilio.join("ID")) || fs.exists(&resilio.join("IgnoreList")))
        {
            return Some(sync_root(SyncRootKind::Resilio, dir));
        }
        if fs.exists(&dir.join(".stfolder")) {
            return Some(sync_root(SyncRootKind::Syncthing, dir));
        }
    }
    None
}

fn provider_root(env: &Env, fs: &dyn Fs, path: &Path) -> Option<SyncRoot> {
    let os = env.os();
    let home = env.home();

    let onedrive = ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]
        .into_iter()
        .filter_map(|name| env.var(name))
        .map(|root| (SyncRootKind::OneDrive, PathBuf::from(root)));
    let dropbox = dropbox_info_files(env)
        .into_iter()
        .filter(|info| fs.exists(info))
        .filter_map(|info| fs.read_to_string(&info).ok())
        .flat_map(|text| dropbox_roots(&text))
        .map(|root| (SyncRootKind::Dropbox, root));
    let google = ["My Drive", "Google Drive"]
        .into_iter()
        .map(|name| (SyncRootKind::GoogleDrive, home.join(name)));

    onedrive
        .chain(dropbox)
        .chain(google)
        .filter(|(_, root)| root.is_absolute())
        .map(|(kind, root)| sync_root(kind, &root))
        .find(|candidate| is_within(os, path, &candidate.root))
        .or_else(|| cloud_storage_root(env, path))
        .or_else(|| google_drive_mount(path))
}

fn dropbox_info_files(env: &Env) -> Vec<PathBuf> {
    match env.os() {
        Os::Windows => [KnownFolder::LocalAppData, KnownFolder::RoamingAppData]
            .into_iter()
            .filter_map(|folder| env.known_folder(folder))
            .map(|base| base.join("Dropbox").join("info.json"))
            .collect(),
        Os::Linux | Os::MacOs => vec![env.home().join(".dropbox").join("info.json")],
    }
}

/// Account roots from Dropbox's `info.json` (`{"personal": {"path": ...}, ...}`).
fn dropbox_roots(text: &str) -> Vec<PathBuf> {
    let Ok(serde_json::Value::Object(accounts)) = serde_json::from_str(text) else {
        return Vec::new();
    };
    accounts
        .values()
        .filter_map(|account| account.get("path")?.as_str())
        .map(PathBuf::from)
        .collect()
}

/// macOS File Provider roots: `~/Library/CloudStorage/<Provider>-<account>`.
fn cloud_storage_root(env: &Env, path: &Path) -> Option<SyncRoot> {
    if env.os() != Os::MacOs {
        return None;
    }
    let base = normalize(&env.home().join("Library").join("CloudStorage"));
    let rest = path.strip_prefix(&base).ok()?;
    let Some(Component::Normal(entry)) = rest.components().next() else {
        return None;
    };
    let name = entry.to_string_lossy();
    let kind = [
        ("OneDrive", SyncRootKind::OneDrive),
        ("Dropbox", SyncRootKind::Dropbox),
        ("GoogleDrive", SyncRootKind::GoogleDrive),
    ]
    .into_iter()
    .find_map(|(prefix, kind)| name.starts_with(prefix).then_some(kind))?;
    Some(sync_root(kind, &base.join(entry)))
}

/// Google Drive for desktop in streaming mode mounts `<drive>:\My Drive`.
fn google_drive_mount(path: &Path) -> Option<SyncRoot> {
    path.ancestors()
        .find(|dir| {
            dir.file_name() == Some(OsStr::new("My Drive"))
                && dir.parent().is_some_and(|parent| parent.parent().is_none())
        })
        .map(|dir| sync_root(SyncRootKind::GoogleDrive, dir))
}

fn sync_root(kind: SyncRootKind, root: &Path) -> SyncRoot {
    SyncRoot {
        kind,
        root: normalize(root),
    }
}

/// Lexically normalizes a path: drops `.` and resolves `..` against earlier
/// components without touching the file system. `..` above a root is dropped.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) => {}
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The `\\?\` (extended-length) form of an absolute Windows path, which
/// file system calls accept past `MAX_PATH` (SPEC 16.1): `C:\x` is
/// `\\?\C:\x`, `\\server\share\x` is `\\?\UNC\server\share\x`. The path is
/// normalized first, since the OS takes a `\\?\` path literally (no `/`,
/// `.` or `..` handling). Any other path, and every path off Windows, is
/// returned as it is.
pub fn extended(path: &Path) -> Cow<'_, Path> {
    if !path.is_absolute() {
        return Cow::Borrowed(path);
    }
    let normal = normalize(path);
    let mut components = normal.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Cow::Borrowed(path);
    };
    let mut out = match prefix.kind() {
        Prefix::Disk(letter) => OsString::from(format!(r"\\?\{}:", char::from(letter))),
        Prefix::UNC(server, share) => {
            let mut out = OsString::from(r"\\?\UNC\");
            out.push(server);
            out.push(r"\");
            out.push(share);
            out
        }
        _ => return Cow::Borrowed(path),
    };
    let mut named = false;
    for component in components {
        if let Component::Normal(name) = component {
            out.push(r"\");
            out.push(name);
            named = true;
        }
    }
    if !named {
        out.push(r"\");
    }
    Cow::Owned(PathBuf::from(out))
}

/// `path` without a `\\?\` prefix ([`extended`] undone): `\\?\C:\x` is
/// `C:\x`, `\\?\UNC\server\share\x` is `\\server\share\x`. Any other path,
/// and every path off Windows, is returned as it is.
pub fn plain(path: &Path) -> Cow<'_, Path> {
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Cow::Borrowed(path);
    };
    let head = match prefix.kind() {
        Prefix::VerbatimDisk(letter) => OsString::from(format!("{}:", char::from(letter))),
        Prefix::VerbatimUNC(server, share) => {
            let mut head = OsString::from(r"\\");
            head.push(server);
            head.push(r"\");
            head.push(share);
            head
        }
        _ => return Cow::Borrowed(path),
    };
    let mut out = PathBuf::from(head);
    out.extend(components);
    Cow::Owned(out)
}

/// Whether `path` equals `base` or lies below it, comparing components
/// case-insensitively on Windows and treating `\\?\C:` like `C:`.
fn is_within(os: Os, path: &Path, base: &Path) -> bool {
    let mut path = path.components();
    base.components().all(|expected| {
        path.next()
            .is_some_and(|actual| same_component(os, actual, expected))
    })
}

fn same_component(os: Os, a: Component<'_>, b: Component<'_>) -> bool {
    match os {
        Os::Windows => component_key(a).to_lowercase() == component_key(b).to_lowercase(),
        Os::Linux | Os::MacOs => a == b,
    }
}

fn component_key(component: Component<'_>) -> Cow<'_, str> {
    if let Component::Prefix(prefix) = component {
        match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                return Cow::Owned(format!("{}:", char::from(letter)));
            }
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                return Cow::Owned(format!(
                    r"\\{}\{}",
                    server.to_string_lossy(),
                    share.to_string_lossy()
                ));
            }
            Prefix::Verbatim(_) | Prefix::DeviceNS(_) => {}
        }
    }
    component.as_os_str().to_string_lossy()
}

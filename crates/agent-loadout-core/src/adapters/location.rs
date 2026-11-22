//! Where client files live (SPEC 7.1 `Location`, 7.3), resolved through an
//! [`Env`] and an [`Fs`] only.

use std::path::{Path, PathBuf};

use crate::env::{Env, KnownFolder, Os};
use crate::fs::Fs;

/// A file or directory location relative to a known base. Relative parts
/// use `/` on every OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    /// `~/<rel>`.
    Home(&'static str),
    /// `%APPDATA%` on Windows, `~/Library/Application Support` on macOS,
    /// `$XDG_CONFIG_HOME` (default `~/.config`) on Linux.
    AppData(&'static str),
    /// `%LOCALAPPDATA%`; Windows only.
    LocalAppData(&'static str),
    /// `$XDG_CONFIG_HOME` (default `~/.config`) on every OS, Windows included.
    XdgConfigAllOs(&'static str),
    /// `$<var>/<rel>` when the variable holds an absolute path, else
    /// `<fallback>/<rel>`. With `tilde`, a value of `~`, or one starting
    /// with `~/` (or `~\` on Windows), is first expanded against home, the
    /// way Pi reads `PI_CODING_AGENT_DIR` (gate V2 PI-1).
    EnvDir {
        var: &'static str,
        fallback: &'static Location,
        rel: &'static str,
        tilde: bool,
    },
    /// `~/<name>/<rel>`, where `name` is the value of `var` when it is a
    /// relative path without `..`, else `default` (omp `PI_CONFIG_DIR`, a
    /// directory name under home).
    HomeNamedDir {
        var: &'static str,
        default: &'static str,
        rel: &'static str,
    },
    /// `<P>/<rel>`.
    Project(&'static str),
    /// Every existing path under `base` matching `pattern`, where a segment
    /// may hold `*` wildcards (MSIX package dirs, VS Code profiles).
    Glob {
        base: &'static Location,
        pattern: &'static str,
    },
}

impl Location {
    /// The path this location names, or `None` when it does not exist on
    /// this OS (`LocalAppData` off Windows), needs a project and has none, or
    /// is a [`Location::Glob`] (see [`Location::expand`]).
    pub fn resolve(&self, env: &Env, project: Option<&Path>) -> Option<PathBuf> {
        let (base, rel) = match *self {
            Location::Home(rel) => (env.home().to_path_buf(), rel),
            Location::AppData(rel) => (app_data(env), rel),
            Location::LocalAppData(rel) => (local_app_data(env)?, rel),
            Location::XdgConfigAllOs(rel) => (xdg_config(env), rel),
            Location::EnvDir {
                var,
                fallback,
                rel,
                tilde,
            } => {
                let base = env
                    .var(var)
                    .map(|value| {
                        if tilde {
                            expand_tilde(env, value)
                        } else {
                            PathBuf::from(value)
                        }
                    })
                    .filter(|path| path.is_absolute());
                match base {
                    Some(base) => (base, rel),
                    None => (fallback.resolve(env, project)?, rel),
                }
            }
            Location::HomeNamedDir { var, default, rel } => {
                let name = env
                    .var(var)
                    .map(PathBuf::from)
                    .filter(|name| is_plain_relative(name))
                    .unwrap_or_else(|| PathBuf::from(default));
                (env.home().join(name), rel)
            }
            Location::Project(rel) => (project?.to_path_buf(), rel),
            Location::Glob { .. } => return None,
        };
        Some(join_rel(base, rel))
    }

    /// Every existing path this location names: the resolved path when it
    /// exists, or each match of a [`Location::Glob`] (sorted). Reads
    /// directories only; never writes.
    pub fn expand(&self, env: &Env, fs: &dyn Fs, project: Option<&Path>) -> Vec<PathBuf> {
        match *self {
            Location::Glob { base, pattern } => {
                let Some(base) = base.resolve(env, project) else {
                    return Vec::new();
                };
                let mut found = vec![base];
                for segment in pattern.split('/').filter(|s| !s.is_empty()) {
                    found = found
                        .into_iter()
                        .flat_map(|dir| matching(env.os(), fs, &dir, segment))
                        .collect();
                }
                found
            }
            _ => self
                .resolve(env, project)
                .filter(|path| fs.exists(path))
                .into_iter()
                .collect(),
        }
    }
}

/// `value` with a leading `~` expanded against home: `~` alone, `~/...`,
/// and on Windows `~\...` (Pi's `normalizePath`). Anything else unchanged.
fn expand_tilde(env: &Env, value: &std::ffi::OsStr) -> PathBuf {
    let Some(text) = value.to_str() else {
        return PathBuf::from(value);
    };
    if text == "~" {
        return env.home().to_path_buf();
    }
    let rest = text
        .strip_prefix("~/")
        .or_else(|| text.strip_prefix("~\\").filter(|_| env.os() == Os::Windows));
    match rest {
        Some(rest) => env.home().join(rest),
        None => PathBuf::from(value),
    }
}

/// A non-empty relative path made of plain names only (no root, drive,
/// `.` or `..`), so joining it to a base stays inside the base.
fn is_plain_relative(path: &Path) -> bool {
    path.components().next().is_some()
        && path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

/// `base` joined with the `/`-separated `rel`, one component at a time so
/// the result uses the platform separator.
fn join_rel(mut base: PathBuf, rel: &str) -> PathBuf {
    for part in rel.split('/').filter(|part| !part.is_empty()) {
        base.push(part);
    }
    base
}

fn app_data(env: &Env) -> PathBuf {
    match env.os() {
        Os::Windows => env
            .known_folder(KnownFolder::RoamingAppData)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| env.home().join("AppData").join("Roaming")),
        Os::MacOs => env.home().join("Library").join("Application Support"),
        Os::Linux => xdg_config(env),
    }
}

fn local_app_data(env: &Env) -> Option<PathBuf> {
    (env.os() == Os::Windows).then(|| {
        env.known_folder(KnownFolder::LocalAppData)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| env.home().join("AppData").join("Local"))
    })
}

/// `$XDG_CONFIG_HOME` when absolute (the XDG spec ignores relative values),
/// else `~/.config`.
fn xdg_config(env: &Env) -> PathBuf {
    env.var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| env.home().join(".config"))
}

/// Entries of `dir` matching one glob segment; a segment without `*` is a
/// plain child that must exist.
fn matching(os: Os, fs: &dyn Fs, dir: &Path, segment: &str) -> Vec<PathBuf> {
    if !segment.contains('*') {
        let path = dir.join(segment);
        return if fs.exists(&path) {
            vec![path]
        } else {
            Vec::new()
        };
    }
    if !fs.is_dir(dir) {
        return Vec::new();
    }
    let Ok(entries) = fs.read_dir(dir) else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter(|entry| {
            entry
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| wildcard_match(os, segment, name))
        })
        .collect()
}

/// `*` matches any run of characters; names compare case-insensitively on
/// Windows.
fn wildcard_match(os: Os, pattern: &str, name: &str) -> bool {
    let (pattern, name) = match os {
        Os::Windows => (pattern.to_lowercase(), name.to_lowercase()),
        Os::Linux | Os::MacOs => (pattern.to_owned(), name.to_owned()),
    };
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(index) => rest = &rest[index + part.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// The key of project `project` in a map keyed by project path (Claude Code
/// `projects` in `~/.claude.json`).
///
/// Claude Code keys a project by its working directory as given, with `\`
/// turned into `/`, and reads only that exact spelling: a key differing in
/// case, slash style or a trailing separator is another project to it (gate
/// V1, `docs/verify/claude-code.md` CC-2, CC-3). So on Windows the key is
/// the project path with `/` separators and no trailing separator (a drive
/// root keeps it: `C:/`), in the path's own case; keys of other spellings
/// in the file are left alone. On Linux and macOS the key is the path
/// without a trailing `/`.
pub fn project_key(os: Os, project: &Path) -> String {
    let path = project.to_string_lossy();
    match os {
        Os::Windows => {
            let slashed = path.replace('\\', "/");
            let trimmed = slashed.trim_end_matches('/');
            if trimmed.len() == 2 && is_drive_path(trimmed) {
                format!("{trimmed}/")
            } else {
                trimmed.to_owned()
            }
        }
        Os::Linux | Os::MacOs => match path.trim_end_matches('/') {
            "" => "/".to_owned(),
            trimmed => trimmed.to_owned(),
        },
    }
}

fn is_drive_path(key: &str) -> bool {
    let bytes = key.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

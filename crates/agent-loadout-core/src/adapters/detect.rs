//! Client detection (SPEC 7.7). Advisory and read-only: probes check that
//! paths exist, look executables up on `PATH` and ask a [`RegistryProbe`];
//! nothing is ever written.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::adapters::location::Location;
use crate::adapters::spec::AdapterSpec;
use crate::env::{Env, Os};
use crate::error::{CoreError, ErrorCode};
use crate::formats::NativeValue;
use crate::fs::Fs;

/// One way a client can be found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// A config file or directory exists. A [`Location::Glob`] matches when
    /// any path matches.
    Path(Location),
    /// An executable with this name (no extension) is on `PATH`. On
    /// Windows each `PATHEXT` extension is tried.
    Executable(&'static str),
    /// Windows: an uninstall registry entry with this `DisplayName`.
    UninstallEntry(&'static str),
    /// Windows: an MSIX package directory exists (a [`Location::Glob`]
    /// under `%LOCALAPPDATA%\Packages`).
    MsixPackage(Location),
}

/// Read-only access to the Windows registry. The core never opens the
/// registry itself; the caller passes a real probe on Windows and tests pass
/// a fake.
pub trait RegistryProbe {
    /// Whether an uninstall entry (`HKCU` or `HKLM`, 64- and 32-bit views,
    /// `Software\Microsoft\Windows\CurrentVersion\Uninstall\*`) has this
    /// `DisplayName`.
    fn has_uninstall_entry(&self, display_name: &str) -> bool;
}

/// A registry with nothing in it (non-Windows hosts, or no registry access).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoRegistry;

impl RegistryProbe for NoRegistry {
    fn has_uninstall_entry(&self, _display_name: &str) -> bool {
        false
    }
}

/// What a probe found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evidence {
    Path(PathBuf),
    Executable(PathBuf),
    UninstallEntry(&'static str),
    MsixPackage(PathBuf),
}

/// The result of probing one client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub client: &'static str,
    /// Whether the client has a target on this OS (Claude Desktop has none
    /// on Linux). Probes do not run when it has none.
    pub supported: bool,
    /// Every probe hit, in probe order.
    pub evidence: Vec<Evidence>,
    /// Findings about the installation, such as
    /// `client_mcp_replaced_by_extension`.
    pub warnings: Vec<CoreError>,
}

impl Detection {
    pub fn detected(&self) -> bool {
        self.supported && !self.evidence.is_empty()
    }
}

/// Runs every probe of `spec`, then checks for an extension replacing its
/// MCP support (user-level installs only; see [`replacing_extension`]).
pub fn detect(
    spec: &AdapterSpec,
    env: &Env,
    fs: &dyn Fs,
    registry: &dyn RegistryProbe,
) -> Detection {
    let supported = spec
        .mcp
        .as_ref()
        .is_none_or(|mcp| mcp.oses.contains(&env.os()));
    let mut evidence = Vec::new();
    let mut warnings = Vec::new();
    if supported {
        for probe in spec.detect {
            run(probe, env, fs, registry, &mut evidence);
        }
        warnings.extend(replacing_extension(spec, env, fs, None));
    }
    Detection {
        client: spec.id,
        supported,
        evidence,
        warnings,
    }
}

/// An extension that, once installed, replaces a client's own MCP support
/// (Pi with `pi-mcp-adapter`): the client then no longer loads its MCP file
/// itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpExtension {
    /// Package name (`pi-mcp-adapter`).
    pub package: &'static str,
    /// Paths whose existence means the extension is installed (an
    /// auto-discovered extension dir). [`Location::Project`] entries are
    /// checked only when a project is given.
    pub paths: &'static [Location],
    /// JSON settings files whose top-level `packages` list may name the
    /// package, as a source string (`npm:pi-mcp-adapter@1.2.0`,
    /// `git:github.com/example/pi-mcp-adapter`) or an object with such a
    /// `source`.
    pub package_lists: &'static [Location],
}

/// `client_mcp_replaced_by_extension` when an extension that takes over
/// `spec`'s MCP support is installed for the user or, with `project`, for
/// that project. Read-only: checks paths and reads the settings files named
/// by the extension; an unreadable or malformed settings file counts as not
/// naming it.
pub fn replacing_extension(
    spec: &AdapterSpec,
    env: &Env,
    fs: &dyn Fs,
    project: Option<&Path>,
) -> Option<CoreError> {
    let mcp = spec.mcp.as_ref()?;
    mcp.replaced_by.iter().find_map(|extension| {
        let found = extension
            .paths
            .iter()
            .filter_map(|location| location.resolve(env, project))
            .find(|path| fs.exists(path))
            .or_else(|| {
                extension
                    .package_lists
                    .iter()
                    .filter_map(|location| location.resolve(env, project))
                    .find(|file| lists_package(fs, file, extension.package))
            })?;
        Some(
            CoreError::new(
                ErrorCode::ClientMcpReplacedByExtension,
                format!(
                    "{} is installed and replaces the MCP support of {}; it may not load the servers Loadout writes",
                    extension.package, spec.display_name
                ),
            )
            .with_path(found),
        )
    })
}

/// Whether the JSON settings `file` has a `packages` entry for `package`.
fn lists_package(fs: &dyn Fs, file: &Path, package: &str) -> bool {
    let Ok(text) = fs.read_to_string(file) else {
        return false;
    };
    let Ok(settings) = NativeValue::parse_json(&text) else {
        return false;
    };
    let Some(packages) = settings
        .as_object()
        .and_then(|settings| settings.get("packages"))
        .and_then(NativeValue::as_array)
    else {
        return false;
    };
    packages.iter().any(|entry| {
        let source = match entry {
            NativeValue::String(source) => Some(source.as_str()),
            NativeValue::Object(entry) => entry.get("source").and_then(NativeValue::as_str),
            _ => None,
        };
        source.is_some_and(|source| package_name(source) == package)
    })
}

/// The package name in a package source: the last path segment without a
/// `npm:`/`git:` prefix, version or ref suffix (`@1.2.0`, `@v1`) or `.git`.
fn package_name(source: &str) -> &str {
    let source = source
        .strip_prefix("npm:")
        .or_else(|| source.strip_prefix("git:"))
        .unwrap_or(source);
    let last = source
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(source);
    // `@` past the first character starts a version (`name@1.2.0`); a
    // leading `@` is an npm scope with no slash left after it.
    let last = match last.get(1..).and_then(|rest| rest.find('@')) {
        Some(at) => &last[..at + 1],
        None => last,
    };
    last.strip_suffix(".git").unwrap_or(last)
}

fn run(
    probe: &Probe,
    env: &Env,
    fs: &dyn Fs,
    registry: &dyn RegistryProbe,
    evidence: &mut Vec<Evidence>,
) {
    match *probe {
        Probe::Path(location) => {
            evidence.extend(
                location
                    .expand(env, fs, None)
                    .into_iter()
                    .map(Evidence::Path),
            );
        }
        Probe::Executable(name) => {
            evidence.extend(find_executable(env, fs, name).map(Evidence::Executable));
        }
        Probe::UninstallEntry(display_name) => {
            if env.os() == Os::Windows && registry.has_uninstall_entry(display_name) {
                evidence.push(Evidence::UninstallEntry(display_name));
            }
        }
        Probe::MsixPackage(location) => {
            if env.os() == Os::Windows {
                evidence.extend(
                    location
                        .expand(env, fs, None)
                        .into_iter()
                        .filter(|path| fs.is_dir(path))
                        .map(Evidence::MsixPackage),
                );
            }
        }
    }
}

/// The first `PATH` entry holding `name` (on Windows `name` plus a
/// `PATHEXT` extension). Relative `PATH` entries are skipped.
fn find_executable(env: &Env, fs: &dyn Fs, name: &str) -> Option<PathBuf> {
    let path = env.var("PATH")?.to_str()?;
    let windows = env.os() == Os::Windows;
    let separator = if windows { ';' } else { ':' };
    let extensions: Vec<String> = if windows {
        env.var("PATHEXT")
            .and_then(OsStr::to_str)
            .unwrap_or(".COM;.EXE;.BAT;.CMD")
            .split(';')
            .filter(|ext| !ext.is_empty())
            .map(str::to_ascii_lowercase)
            .collect()
    } else {
        vec![String::new()]
    };
    path.split(separator)
        .map(|dir| Path::new(dir.trim_matches('"')))
        .filter(|dir| dir.is_absolute())
        .flat_map(|dir| {
            extensions
                .iter()
                .map(move |ext| dir.join(format!("{name}{ext}")))
        })
        .find(|candidate| fs.exists(candidate) && !fs.is_dir(candidate))
}

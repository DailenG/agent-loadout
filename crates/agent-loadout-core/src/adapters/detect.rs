//! Client detection (SPEC 7.7). Advisory and read-only: probes check that
//! paths exist, look executables up on `PATH` and ask a [`RegistryProbe`];
//! nothing is ever written.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::adapters::location::Location;
use crate::adapters::spec::AdapterSpec;
use crate::env::{Env, Os};
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
}

impl Detection {
    pub fn detected(&self) -> bool {
        self.supported && !self.evidence.is_empty()
    }
}

/// Runs every probe of `spec`.
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
    if supported {
        for probe in spec.detect {
            run(probe, env, fs, registry, &mut evidence);
        }
    }
    Detection {
        client: spec.id,
        supported,
        evidence,
    }
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

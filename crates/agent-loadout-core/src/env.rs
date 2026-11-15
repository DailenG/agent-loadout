//! Process environment facade: OS, known folders, environment variables, host
//! name and clock. The core reads these only through [`Env`]; the CLI and the app build
//! one from the process, tests build one from a temp root.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

/// Operating system whose conventions apply (paths, variable name casing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Os {
    Windows,
    /// Linux and other XDG-style Unix systems.
    Linux,
    MacOs,
}

impl Os {
    /// Stable `snake_case` name used in CLI and IPC output.
    pub const fn as_str(self) -> &'static str {
        match self {
            Os::Windows => "windows",
            Os::Linux => "linux",
            Os::MacOs => "macos",
        }
    }
}

/// Windows Known Folders besides the profile (which is [`Env::home`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KnownFolder {
    /// `FOLDERID_LocalAppData` (`%LOCALAPPDATA%`).
    LocalAppData,
    /// `FOLDERID_RoamingAppData` (`%APPDATA%`).
    RoamingAppData,
}

/// Source of the current time.
pub trait Clock: fmt::Debug + Send + Sync {
    fn now(&self) -> SystemTime;
}

/// The real system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// A clock frozen at one instant, for deterministic tests.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub SystemTime);

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

/// Everything the core may know about the process it runs in.
#[derive(Debug, Clone)]
pub struct Env {
    os: Os,
    home: PathBuf,
    local_app_data: Option<PathBuf>,
    roaming_app_data: Option<PathBuf>,
    vars: BTreeMap<String, OsString>,
    hostname: Option<String>,
    clock: Arc<dyn Clock>,
}

impl Env {
    /// Starts an environment for `os` whose user profile (`~`) is `home`.
    /// On Windows `home` is `FOLDERID_Profile`, never `$HOME`.
    pub fn builder(os: Os, home: impl Into<PathBuf>) -> EnvBuilder {
        EnvBuilder {
            env: Env {
                os,
                home: home.into(),
                local_app_data: None,
                roaming_app_data: None,
                vars: BTreeMap::new(),
                hostname: None,
                clock: Arc::new(SystemClock),
            },
        }
    }

    pub fn os(&self) -> Os {
        self.os
    }

    /// The user profile directory (`~`).
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// A Windows Known Folder, if known.
    pub fn known_folder(&self, folder: KnownFolder) -> Option<&Path> {
        match folder {
            KnownFolder::LocalAppData => self.local_app_data.as_deref(),
            KnownFolder::RoamingAppData => self.roaming_app_data.as_deref(),
        }
    }

    /// An environment variable. Names are case-insensitive on Windows. Empty
    /// values read as unset, matching how Windows and the XDG spec treat them.
    pub fn var(&self, name: &str) -> Option<&OsStr> {
        self.vars
            .get(&var_key(self.os, name))
            .map(OsString::as_os_str)
            .filter(|value| !value.is_empty())
    }

    /// The current time from the injected clock.
    pub fn now(&self) -> SystemTime {
        self.clock.now()
    }

    /// The computer's host name, if known. The default machine label.
    pub fn hostname(&self) -> Option<&str> {
        self.hostname
            .as_deref()
            .filter(|name| !name.trim().is_empty())
    }
}

/// Builder for [`Env`]. The clock defaults to [`SystemClock`].
#[derive(Debug, Clone)]
pub struct EnvBuilder {
    env: Env,
}

impl EnvBuilder {
    pub fn known_folder(mut self, folder: KnownFolder, path: impl Into<PathBuf>) -> Self {
        let path = Some(path.into());
        match folder {
            KnownFolder::LocalAppData => self.env.local_app_data = path,
            KnownFolder::RoamingAppData => self.env.roaming_app_data = path,
        }
        self
    }

    pub fn var(mut self, name: &str, value: impl Into<OsString>) -> Self {
        self.env
            .vars
            .insert(var_key(self.env.os, name), value.into());
        self
    }

    pub fn hostname(mut self, name: impl Into<String>) -> Self {
        self.env.hostname = Some(name.into());
        self
    }

    pub fn clock(mut self, clock: impl Clock + 'static) -> Self {
        self.env.clock = Arc::new(clock);
        self
    }

    pub fn build(self) -> Env {
        self.env
    }
}

fn var_key(os: Os, name: &str) -> String {
    match os {
        Os::Windows => name.to_ascii_uppercase(),
        Os::Linux | Os::MacOs => name.to_owned(),
    }
}

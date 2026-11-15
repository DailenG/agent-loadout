use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use agent_loadout_core::{Env, EnvBuilder, FixedClock, KnownFolder, Os};
use tempfile::TempDir;

use crate::RootedFs;

/// 2026-01-01T00:00:00Z, the instant every `TempHome` clock reports.
const FIXED_NOW: Duration = Duration::from_secs(1_767_225_600);

const XDG_DIRS: [(&str, &str); 4] = [
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_STATE_HOME", "state"),
];

/// A temp root shaped like a user profile, deleted on drop:
///
/// ```text
/// <root>/home/           profile (~, HOME, USERPROFILE)
/// <root>/appdata/        APPDATA (FOLDERID_RoamingAppData)
/// <root>/localappdata/   LOCALAPPDATA (FOLDERID_LocalAppData)
/// <root>/xdg/{config,data,cache,state}/
/// <root>/projects/       project folders
/// <root>/bin/            stub binaries; the only PATH entry of `command`
/// ```
#[derive(Debug)]
pub struct TempHome {
    dir: TempDir,
}

impl TempHome {
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("loadout-test-")
            .tempdir()
            .expect("create temp root");
        let home = Self { dir };
        let xdg = home.xdg();
        let dirs = [
            home.home(),
            home.appdata(),
            home.local_appdata(),
            home.projects(),
            home.bin(),
        ]
        .into_iter()
        .chain(XDG_DIRS.iter().map(|(_, sub)| xdg.join(sub)));
        for dir in dirs {
            std::fs::create_dir_all(&dir)
                .unwrap_or_else(|err| panic!("create {}: {err}", dir.display()));
        }
        home
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn home(&self) -> PathBuf {
        self.root().join("home")
    }

    pub fn appdata(&self) -> PathBuf {
        self.root().join("appdata")
    }

    pub fn local_appdata(&self) -> PathBuf {
        self.root().join("localappdata")
    }

    pub fn xdg(&self) -> PathBuf {
        self.root().join("xdg")
    }

    pub fn projects(&self) -> PathBuf {
        self.root().join("projects")
    }

    pub fn bin(&self) -> PathBuf {
        self.root().join("bin")
    }

    /// Baseline `Env` for `os`: profile `home/`, Windows Known Folders
    /// `localappdata/` and `appdata/`, a fixed clock and no variables.
    pub fn env_builder(&self, os: Os) -> EnvBuilder {
        let builder =
            Env::builder(os, self.home()).clock(FixedClock(SystemTime::UNIX_EPOCH + FIXED_NOW));
        match os {
            Os::Windows => builder
                .known_folder(KnownFolder::LocalAppData, self.local_appdata())
                .known_folder(KnownFolder::RoamingAppData, self.appdata()),
            Os::Linux | Os::MacOs => builder,
        }
    }

    pub fn env(&self, os: Os) -> Env {
        self.env_builder(os).build()
    }

    /// A file system that panics on any access outside the temp root.
    pub fn fs(&self) -> RootedFs {
        RootedFs::new(self.root())
    }

    /// A command with a cleared environment that sees only this temp root:
    /// `HOME`, `USERPROFILE`, `APPDATA`, `LOCALAPPDATA`, `XDG_*` and a `PATH`
    /// holding only `bin/`. The working directory is the temp root.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .current_dir(self.root())
            .env("HOME", self.home())
            .env("USERPROFILE", self.home())
            .env("APPDATA", self.appdata())
            .env("LOCALAPPDATA", self.local_appdata())
            .env("PATH", self.bin());
        for (var, sub) in XDG_DIRS {
            command.env(var, self.xdg().join(sub));
        }
        command
    }
}

impl Default for TempHome {
    fn default() -> Self {
        Self::new()
    }
}

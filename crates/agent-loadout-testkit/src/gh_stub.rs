//! The `gh-stub` binary as a fake GitHub CLI (SPEC 9.6 `auth = gh`, 9.7).
//!
//! [`GhStub::install`] copies the stub into a directory as `gh` (`gh.exe`)
//! next to a `gh-stub.json` that says what it answers. The stub appends
//! every call's arguments to `gh-stub.log` (never what it reads on stdin)
//! and supports:
//!
//! - `auth status`: exit 0 when logged in, else 1.
//! - `auth git-credential get`: git's credential protocol; answers
//!   `username=<user>` and `password=<token>` when logged in.
//! - `repo create <name> --private`: prints
//!   `https://github.com/<user>/<name>`; a name created before fails with
//!   "already exists". Anything without `--private` is refused.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::op_stub::{built_binary, copy_executable};

/// Config file next to the installed stub.
pub const CONFIG_FILE: &str = "gh-stub.json";
/// Call log next to the installed stub.
pub const LOG_FILE: &str = "gh-stub.log";
/// Repositories the stub created, one name per line.
pub const REPOS_FILE: &str = "gh-stub-repos.txt";

/// What the stub answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhStubConfig {
    pub logged_in: bool,
    pub user: String,
    /// What `auth git-credential get` returns as the password.
    pub token: String,
}

impl Default for GhStubConfig {
    fn default() -> Self {
        Self {
            logged_in: true,
            user: "stub-user".to_owned(),
            token: String::new(),
        }
    }
}

/// An installed stub.
#[derive(Debug)]
pub struct GhStub {
    dir: PathBuf,
    config: GhStubConfig,
}

impl GhStub {
    /// Installs the stub in `dir` as `gh`, logged in as `stub-user`.
    pub fn install(dir: &Path) -> Self {
        let target = dir.join(format!("gh{}", std::env::consts::EXE_SUFFIX));
        copy_executable(&binary(), &target);
        let mut stub = Self {
            dir: dir.to_owned(),
            config: GhStubConfig::default(),
        };
        stub.save();
        stub
    }

    pub fn set_logged_in(&mut self, logged_in: bool) -> &mut Self {
        self.config.logged_in = logged_in;
        self.save()
    }

    pub fn set_token(&mut self, token: &str) -> &mut Self {
        self.config.token = token.to_owned();
        self.save()
    }

    /// Every call so far, one line of arguments each.
    pub fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join(LOG_FILE))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn save(&mut self) -> &mut Self {
        let path = self.dir.join(CONFIG_FILE);
        let text = serde_json::to_string_pretty(&self.config).expect("config serializes");
        std::fs::write(&path, text).unwrap_or_else(|err| panic!("write {}: {err}", path.display()));
        self
    }
}

/// The built `gh-stub` binary.
pub fn binary() -> PathBuf {
    built_binary("gh-stub")
}

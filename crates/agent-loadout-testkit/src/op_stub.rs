//! The `op-stub` binary as a fake 1Password CLI (SPEC 19 item 7).
//!
//! [`OpStub::install`] copies the stub into a directory (a `TempHome`'s
//! `bin/`) as `op` (`op.exe`), next to an `op-stub.json` that says what it
//! answers: version, signed in or locked, items and Environments. The stub
//! appends every call (its arguments, never a value) to `op-stub.log`.
//!
//! `op environment read` is forbidden: the stub refuses it, and dropping
//! the [`OpStub`] fails the test when it was ever called.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Config file next to the installed stub.
pub const CONFIG_FILE: &str = "op-stub.json";
/// Call log next to the installed stub.
pub const LOG_FILE: &str = "op-stub.log";
/// Log prefix of a forbidden call.
pub const FORBIDDEN: &str = "FORBIDDEN";

/// Signed in, or every call needs sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpStubMode {
    #[default]
    Ok,
    Locked,
}

/// What the stub answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpStubConfig {
    /// `op --version` output.
    pub version: String,
    pub mode: OpStubMode,
    /// `op://` reference -> value, for `op read`.
    pub items: BTreeMap<String, String>,
    /// Environment id -> variable name -> value, for `op environment secret
    /// list` (values are included, as 1Password does for "show value by
    /// default" secrets; the parser must drop them).
    pub environments: BTreeMap<String, BTreeMap<String, String>>,
}

impl Default for OpStubConfig {
    fn default() -> Self {
        Self {
            version: "2.42.0".to_owned(),
            mode: OpStubMode::Ok,
            items: BTreeMap::new(),
            environments: BTreeMap::new(),
        }
    }
}

/// An installed stub.
#[derive(Debug)]
pub struct OpStub {
    dir: PathBuf,
    config: OpStubConfig,
}

impl OpStub {
    /// Installs the stub in `dir` as `op` with the default config (2.42.0,
    /// signed in, no items).
    pub fn install(dir: &Path) -> Self {
        let target = dir.join(format!("op{}", std::env::consts::EXE_SUFFIX));
        std::fs::copy(binary(), &target)
            .unwrap_or_else(|err| panic!("install op-stub at {}: {err}", target.display()));
        let mut stub = Self {
            dir: dir.to_owned(),
            config: OpStubConfig::default(),
        };
        stub.save();
        stub
    }

    pub fn set_version(&mut self, version: &str) -> &mut Self {
        self.config.version = version.to_owned();
        self.save()
    }

    pub fn set_mode(&mut self, mode: OpStubMode) -> &mut Self {
        self.config.mode = mode;
        self.save()
    }

    pub fn add_item(&mut self, reference: &str, value: &str) -> &mut Self {
        self.config
            .items
            .insert(reference.to_owned(), value.to_owned());
        self.save()
    }

    pub fn add_environment_variable(
        &mut self,
        environment: &str,
        name: &str,
        value: &str,
    ) -> &mut Self {
        self.config
            .environments
            .entry(environment.to_owned())
            .or_default()
            .insert(name.to_owned(), value.to_owned());
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

impl Drop for OpStub {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let forbidden: Vec<String> = self
            .calls()
            .into_iter()
            .filter(|line| line.starts_with(FORBIDDEN))
            .collect();
        assert!(
            forbidden.is_empty(),
            "the 1Password CLI was asked for `op environment read`, which Loadout must never call:\n{}",
            forbidden.join("\n")
        );
    }
}

/// The built `op-stub` binary, uplifted into the profile directory
/// (`target/debug`) that holds the running test executable (in `deps/` or
/// `build/<package>/<hash>/out/`). `cargo test --workspace` builds it; a
/// single-package run of another crate may need `cargo build -p
/// agent-loadout-testkit --bin op-stub` first.
pub fn binary() -> PathBuf {
    let name = format!("op-stub{}", std::env::consts::EXE_SUFFIX);
    let exe = std::env::current_exe().expect("current test executable");
    exe.ancestors()
        .skip(1)
        .take(5)
        .map(|dir| dir.join(&name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| {
            panic!(
                "{name} not found near {}; run `cargo build -p agent-loadout-testkit --bin op-stub` \
                 (`cargo test --workspace` builds it)",
                exe.display()
            )
        })
}

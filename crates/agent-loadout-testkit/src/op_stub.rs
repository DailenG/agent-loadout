//! The `op-stub` binary as a fake 1Password CLI (SPEC 19 item 7).
//!
//! [`OpStub::install`] copies the stub into a directory (a `TempHome`'s
//! `bin/`) as `op` (`op.exe`), next to an `op-stub.json` that says what it
//! answers: version, signed in or locked, items, Environments and an
//! accepted service account token. The stub appends every call (its
//! arguments, never a value) to `op-stub.log`, plus the line
//! [`SERVICE_ACCOUNT_SEEN`] when the call had `OP_SERVICE_ACCOUNT_TOKEN`.
//!
//! `op run --environment <id> [--no-masking] -- <cmd>` starts `<cmd>` with
//! the Environment's variables added; without `--no-masking` it replaces
//! their values in the command's stdout, as 1Password does.
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
/// Log line of a call that had `OP_SERVICE_ACCOUNT_TOKEN` set.
pub const SERVICE_ACCOUNT_SEEN: &str = "env OP_SERVICE_ACCOUNT_TOKEN";
/// Log prefix of a call that had `OP_ACCOUNT` set (followed by its value,
/// which is an account name, not a secret).
pub const ACCOUNT_SEEN: &str = "env OP_ACCOUNT";
/// What `op run` writes instead of a value without `--no-masking`.
pub const CONCEALED: &str = "<concealed by 1Password>";

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
    /// The service account token `op` accepts, if any.
    #[serde(default)]
    pub service_account_token: Option<String>,
    /// Accounts signed in to the desktop app. With more than one, a call
    /// without `OP_ACCOUNT` (and without a service account) fails, as real
    /// `op` does; with any, an `OP_ACCOUNT` not among them fails.
    #[serde(default)]
    pub accounts: Vec<String>,
}

impl Default for OpStubConfig {
    fn default() -> Self {
        Self {
            version: "2.42.0".to_owned(),
            mode: OpStubMode::Ok,
            items: BTreeMap::new(),
            environments: BTreeMap::new(),
            service_account_token: None,
            accounts: Vec::new(),
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
        copy_executable(&binary(), &target);
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

    /// Accept `token` as `OP_SERVICE_ACCOUNT_TOKEN`: with it, calls succeed
    /// even when the desktop app is locked; another token fails.
    pub fn set_service_account_token(&mut self, token: &str) -> &mut Self {
        self.config.service_account_token = Some(token.to_owned());
        self.save()
    }

    /// Accounts signed in to the desktop app (see [`OpStubConfig::accounts`]).
    pub fn set_accounts(&mut self, accounts: &[&str]) -> &mut Self {
        self.config.accounts = accounts.iter().map(|a| (*a).to_owned()).collect();
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

/// Copies an executable so that it can be run at once from any thread.
///
/// On Unix the copy runs in a child process (`cp`): a write descriptor
/// opened in this process would be inherited by a child that another test
/// thread forks at the same moment, and while that child holds it, running
/// the copy fails with ETXTBSY ("Text file busy").
pub(crate) fn copy_executable(from: &Path, to: &Path) {
    #[cfg(unix)]
    {
        let status = std::process::Command::new("cp")
            .arg("--")
            .arg(from)
            .arg(to)
            .status()
            .unwrap_or_else(|err| panic!("run cp for {}: {err}", to.display()));
        assert!(status.success(), "cp {} failed: {status}", to.display());
    }
    #[cfg(not(unix))]
    std::fs::copy(from, to)
        .map(drop)
        .unwrap_or_else(|err| panic!("copy {} to {}: {err}", from.display(), to.display()));
}

/// The built `op-stub` binary. See [`built_binary`].
pub fn binary() -> PathBuf {
    built_binary("op-stub")
}

/// A binary of this crate (`op-stub`, `fake-mcp-server`), uplifted into the
/// profile directory (`target/debug`) that holds the running test
/// executable (in `deps/` or `build/<package>/<hash>/out/`). `cargo test
/// --workspace` builds them; a single-package run of another crate may need
/// `cargo build -p agent-loadout-testkit --bins` first.
pub fn built_binary(stem: &str) -> PathBuf {
    let name = format!("{stem}{}", std::env::consts::EXE_SUFFIX);
    let exe = std::env::current_exe().expect("current test executable");
    exe.ancestors()
        .skip(1)
        .take(5)
        .map(|dir| dir.join(&name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| {
            panic!(
                "{name} not found near {}; run `cargo build -p agent-loadout-testkit --bins` \
                 (`cargo test --workspace` builds it)",
                exe.display()
            )
        })
}

//! The `skillspector-stub` binary as a fake SkillSpector, uv and uvx
//! (#30).
//!
//! [`SkillSpectorStub::new`] makes a `bin` folder; [`SkillSpectorStub::add`]
//! copies the stub into it as `skillspector`, `uvx` or `uv` (plus the
//! platform's executable suffix), so a test puts that folder alone on
//! `PATH`. A `skillspector-stub.json` beside the copies says what they do
//! ([`StubConfig`]). Every call appends one JSON line to
//! `skillspector-stub.log`: the program it ran as, its arguments, its
//! whole environment, the bytes it read on stdin and the files below its
//! last argument. Tests use it to assert what Loadout passes: never a
//! forbidden flag, never a provider key, stdin closed.
//!
//! As `uvx` (or `uv tool run`) it acts like uv's tool runner: with
//! `--offline` it answers only once "prepared" (the `prepared` file, made
//! by a run without `--offline` or by [`SkillSpectorStub::set_prepared`]),
//! else it fails like uv without its cache; then it runs the arguments
//! after `skillspector` as SkillSpector would. With [`StubConfig::hang`]
//! a scan starts a child of its own (a heartbeat every 50 ms) and never
//! exits, so a test can check that a timeout kills the whole tree.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::op_stub::{built_binary, copy_executable};

/// Config file beside the copies.
pub const CONFIG_FILE: &str = "skillspector-stub.json";
/// Call log beside the copies (one JSON object per line).
pub const LOG_FILE: &str = "skillspector-stub.log";
/// Made once uv "downloaded" SkillSpector.
pub const PREPARED_FILE: &str = "prepared";
/// First argument of the stub's own heartbeat child.
pub const HEARTBEAT_ARG: &str = "__heartbeat";
/// What uv prints when `--offline` finds nothing in its cache.
pub const UV_OFFLINE_ERROR: &str = "  x No solution found when resolving tool dependencies:\n  \
    `-> Because skillspector==2.13.7 was not found in the cache and you require \
    skillspector==2.13.7, we can conclude that your requirements are unsatisfiable.\n\n      \
    hint: Packages were unavailable because the network was disabled. When the network is \
    disabled, registry packages may only be read from the cache.\n";
/// In [`StubConfig::stderr`], replaced by the scanned folder (the last
/// argument), as SkillSpector names it in its errors.
pub const SKILL_PLACEHOLDER: &str = "{skill}";
/// In [`StubConfig::stderr`], replaced by the scanned folder with every
/// backslash doubled, as Python's error reprs and JSON-escaped text print
/// a Windows path.
pub const SKILL_ESCAPED_PLACEHOLDER: &str = "{skill_escaped}";

/// What the stub does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StubConfig {
    /// A scan prints this on stdout.
    #[serde(default)]
    pub stdout: String,
    /// A scan's exit code.
    #[serde(default)]
    pub exit: i32,
    /// A scan prints this on stderr instead of the default line
    /// ([`SKILL_PLACEHOLDER`] and [`SKILL_ESCAPED_PLACEHOLDER`] become the
    /// scanned folder).
    #[serde(default)]
    pub stderr: Option<String>,
    /// Spaces a scan prints on stdout after `stdout`.
    #[serde(default)]
    pub pad: usize,
    /// A scan starts a heartbeat child writing to `heartbeat`, then never
    /// exits.
    #[serde(default)]
    pub hang: bool,
    #[serde(default)]
    pub heartbeat: Option<PathBuf>,
    /// Every call starts a heartbeat child writing here first thing, before
    /// anything else (as `uvx` starts `uv`), so a test can check it never
    /// escapes the kill.
    #[serde(default)]
    pub early_heartbeat: Option<PathBuf>,
    /// `--version` prints this (as SkillSpector).
    pub version: String,
    /// `--version`'s exit code.
    #[serde(default)]
    pub version_exit: i32,
    /// uv's progress lines while preparing (stderr).
    #[serde(default)]
    pub prepare_lines: Vec<String>,
    /// uv's exit code while preparing; not 0: nothing is prepared.
    #[serde(default)]
    pub prepare_exit: i32,
    /// Preparing never ends.
    #[serde(default)]
    pub prepare_hang: bool,
}

impl Default for StubConfig {
    fn default() -> Self {
        Self {
            stdout: String::new(),
            exit: 0,
            stderr: None,
            pad: 0,
            hang: false,
            heartbeat: None,
            early_heartbeat: None,
            version: "skillspector 2.13.7".to_owned(),
            version_exit: 0,
            prepare_lines: Vec::new(),
            prepare_exit: 0,
            prepare_hang: false,
        }
    }
}

/// One call, as the stub saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StubCall {
    /// `skillspector`, `uvx` or `uv`.
    pub program: String,
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// Bytes read from stdin before EOF.
    pub stdin_bytes: usize,
    /// Files below the last argument, relative, `/`-separated.
    pub files: Vec<String>,
}

impl StubCall {
    /// A scan (not a `--version` probe or Prepare).
    pub fn is_scan(&self) -> bool {
        self.argv.iter().any(|arg| arg == "scan")
    }
}

/// A `bin` folder of stub copies.
#[derive(Debug)]
pub struct SkillSpectorStub {
    bin: PathBuf,
}

impl SkillSpectorStub {
    /// An empty `<root>/bin` (no program yet), with the default config.
    pub fn new(root: &Path) -> Self {
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin)
            .unwrap_or_else(|err| panic!("create {}: {err}", bin.display()));
        let stub = Self { bin };
        stub.set(&StubConfig::default());
        stub
    }

    /// The folder to put on `PATH`.
    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// Copies the stub in as `program` (`skillspector`, `uvx`, `uv`).
    pub fn add(&self, program: &str) -> PathBuf {
        let path = self
            .bin
            .join(format!("{program}{}", std::env::consts::EXE_SUFFIX));
        copy_executable(&binary(), &path);
        path
    }

    pub fn set(&self, config: &StubConfig) {
        let path = self.bin.join(CONFIG_FILE);
        let text = serde_json::to_string_pretty(config).expect("config serializes");
        std::fs::write(&path, text).unwrap_or_else(|err| panic!("write {}: {err}", path.display()));
    }

    /// A scan prints `stdout` and exits with `exit`.
    pub fn answer(&self, stdout: &str, exit: i32) {
        self.set(&StubConfig {
            stdout: stdout.to_owned(),
            exit,
            ..StubConfig::default()
        });
    }

    /// Whether uv "has" SkillSpector in its cache.
    pub fn set_prepared(&self, prepared: bool) {
        let path = self.bin.join(PREPARED_FILE);
        if prepared {
            std::fs::write(&path, b"").expect("write the prepared marker");
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }

    pub fn prepared(&self) -> bool {
        self.bin.join(PREPARED_FILE).is_file()
    }

    /// Every call so far.
    pub fn calls(&self) -> Vec<StubCall> {
        std::fs::read_to_string(self.bin.join(LOG_FILE))
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("a stub log line"))
            .collect()
    }

    /// Every scan so far.
    pub fn scans(&self) -> Vec<StubCall> {
        self.calls().into_iter().filter(StubCall::is_scan).collect()
    }
}

/// The built `skillspector-stub` binary.
pub fn binary() -> PathBuf {
    built_binary("skillspector-stub")
}

/// A hand-made report of `fixtures/skillspector/output`.
pub fn output(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/skillspector/output")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

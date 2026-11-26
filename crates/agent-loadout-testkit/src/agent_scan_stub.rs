//! The `agent-scan-stub` binary as a fake Snyk Agent Scan (#27).
//!
//! [`AgentScanStub::install`] installs the stub as the scanner of a
//! `LOADOUT_HOME` through the real install path ([`Tool::install`], with a
//! [`StubDownload`] serving the stub's bytes and an [`Asset`] of their
//! hash), next to an `agent-scan-stub.json` that says what it prints. Every
//! call appends one JSON line to `agent-scan-stub.log`: its arguments, its
//! whole environment, whether stdin was closed (EOF at once) and the files
//! below the scanned path. Tests use it to assert what Loadout passes:
//! never a dangerous flag, the token only in the environment.
//!
//! With [`StubConfig::hang`] the stub starts a child of its own (a second
//! stub writing a heartbeat file every 50 ms) and never exits, so a test
//! can check that a timeout kills the whole tree: the heartbeat stops.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use agent_loadout_core::CoreError;
use agent_loadout_core::paths::LoadoutHome;
use agent_loadout_core::scan::external::snyk::{Asset, Download, Tool, VERSION, copy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::op_stub::built_binary;

/// Config file next to the installed stub.
pub const CONFIG_FILE: &str = "agent-scan-stub.json";
/// Call log next to the installed stub (one JSON object per line).
pub const LOG_FILE: &str = "agent-scan-stub.log";
/// First argument of the stub's own heartbeat child.
pub const HEARTBEAT_ARG: &str = "__heartbeat";

/// What the stub does.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StubConfig {
    /// Printed on stdout.
    pub stdout: String,
    /// Exit code.
    pub exit: i32,
    /// Start a heartbeat child writing to `heartbeat`, then never exit.
    #[serde(default)]
    pub hang: bool,
    #[serde(default)]
    pub heartbeat: Option<PathBuf>,
    /// Printed on stderr instead of the default line.
    #[serde(default)]
    pub stderr: Option<String>,
    /// Spaces printed on stdout after `stdout`.
    #[serde(default)]
    pub pad: usize,
}

/// One call, as the stub saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StubCall {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// Bytes read from stdin before EOF.
    pub stdin_bytes: usize,
    /// Files below the last argument, relative, `/`-separated.
    pub files: Vec<String>,
}

/// An installed stub.
#[derive(Debug)]
pub struct AgentScanStub {
    tool: Tool,
}

impl AgentScanStub {
    /// Installs the stub as `home`'s scanner (version [`VERSION`]) through
    /// [`Tool::install`], printing `{}` until [`AgentScanStub::set`].
    pub fn install(home: &LoadoutHome) -> Self {
        let bytes = std::fs::read(binary()).expect("read agent-scan-stub");
        let tool = Tool::new(home, VERSION, asset(&bytes));
        tool.install(&StubDownload(bytes), &mut |_, _| {}, None)
            .expect("install the stub");
        let stub = Self { tool };
        stub.set(&StubConfig::default());
        stub
    }

    pub fn tool(&self) -> &Tool {
        &self.tool
    }

    pub fn set(&self, config: &StubConfig) {
        let path = self.tool.dir().join(CONFIG_FILE);
        let text = serde_json::to_string_pretty(config).expect("config serializes");
        std::fs::write(&path, text).unwrap_or_else(|err| panic!("write {}: {err}", path.display()));
    }

    /// Prints `stdout` and exits 0.
    pub fn answer(&self, stdout: &str) {
        self.set(&StubConfig {
            stdout: stdout.to_owned(),
            ..StubConfig::default()
        });
    }

    /// Every call so far.
    pub fn calls(&self) -> Vec<StubCall> {
        std::fs::read_to_string(self.tool.dir().join(LOG_FILE))
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("a stub log line"))
            .collect()
    }
}

/// The built `agent-scan-stub` binary.
pub fn binary() -> PathBuf {
    built_binary("agent-scan-stub")
}

/// An asset whose hash and size are `bytes`'.
pub fn asset(bytes: &[u8]) -> Asset {
    Asset {
        name: format!("agent-scan-stub{}", std::env::consts::EXE_SUFFIX),
        sha256: Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        size: bytes.len() as u64,
    }
}

/// A recorded output of `fixtures/agent-scan/output`.
pub fn output(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/agent-scan/output")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

/// A [`Download`] serving fixed bytes (no network).
#[derive(Debug, Clone)]
pub struct StubDownload(pub Vec<u8>);

impl Download for StubDownload {
    fn fetch(
        &self,
        _url: &str,
        out: &mut dyn Write,
        max: u64,
        progress: &mut dyn FnMut(u64, u64),
        cancel: Option<&AtomicBool>,
    ) -> Result<u64, CoreError> {
        copy(
            &mut self.0.as_slice(),
            out,
            max,
            self.0.len() as u64,
            progress,
            cancel,
        )
        .map_err(|err| {
            CoreError::new(
                agent_loadout_core::ErrorCode::ScannerDownloadFailed,
                err.to_string(),
            )
        })
    }
}

//! Installing the pinned scanner (#27): a download from the GitHub release
//! into `<LOADOUT_HOME>/tools/snyk-agent-scan/<version>/` (machine-local,
//! never synced, never on `PATH`), verified against the compiled size and
//! SHA-256 before it is renamed into place; `installed.json` is written
//! last, so a scanner counts as installed only after verification.
//!
//! These are real files on purpose (`std::fs`, not [`crate::fs::Fs`]): a
//! process runs them.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ureq::tls::{RootCerts, TlsConfig};

use super::SCANNER;
use super::pin::{self, Asset, MAX_DOWNLOAD};
use crate::catalog::http::proxy;
use crate::env::Env;
use crate::error::{CoreError, ErrorCode};
use crate::paths::LoadoutHome;

/// `<LOADOUT_HOME>/<TOOLS_DIR>/snyk-agent-scan/<version>/`.
pub const TOOLS_DIR: &str = "tools";
/// Written after verification; its absence means "not installed".
const MARKER: &str = "installed.json";
/// Hosts a download may be served from: the release page and the two
/// storage hosts GitHub redirects release assets to.
pub const DOWNLOAD_HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
/// Redirects followed (GitHub uses one).
const MAX_REDIRECTS: usize = 5;

/// What `installed.json` records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Marker {
    version: String,
    asset: String,
    sha256: String,
    size: u64,
}

/// The scanner's place on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    root: PathBuf,
    version: String,
    asset: Asset,
}

impl Tool {
    /// The pinned release for this platform; `not_supported_on_os` when Snyk
    /// publishes no build for it.
    pub fn pinned(home: &LoadoutHome) -> Result<Self, CoreError> {
        let asset = pin::pinned_asset().ok_or_else(|| {
            CoreError::new(
                ErrorCode::NotSupportedOnOs,
                format!(
                    "Snyk Agent Scan has no build for {} on {}",
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ),
            )
        })?;
        Ok(Self::new(home, pin::VERSION, asset))
    }

    /// Another version or asset (tests install a stub with its own hash).
    pub fn new(home: &LoadoutHome, version: &str, asset: Asset) -> Self {
        Self {
            root: tool_root(home),
            version: version.to_owned(),
            asset,
        }
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn asset(&self) -> &Asset {
        &self.asset
    }

    /// `tools/snyk-agent-scan/<version>`.
    pub fn dir(&self) -> PathBuf {
        self.root.join(&self.version)
    }

    /// The executable inside [`Tool::dir`].
    pub fn binary(&self) -> PathBuf {
        self.dir()
            .join(format!("agent-scan{}", std::env::consts::EXE_SUFFIX))
    }

    /// Verified and in place: `installed.json` names this version and asset,
    /// and the binary has the pinned size.
    pub fn installed(&self) -> bool {
        let Ok(text) = std::fs::read_to_string(self.dir().join(MARKER)) else {
            return false;
        };
        let Ok(marker) = serde_json::from_str::<Marker>(&text) else {
            return false;
        };
        marker == self.marker()
            && std::fs::metadata(self.binary())
                .is_ok_and(|meta| meta.is_file() && meta.len() == self.asset.size)
    }

    fn marker(&self) -> Marker {
        Marker {
            version: self.version.clone(),
            asset: self.asset.name.clone(),
            sha256: self.asset.sha256.clone(),
            size: self.asset.size,
        }
    }

    /// Hashes the installed binary (25-33 MB, well under a second) against
    /// the pin. On a mismatch, or a binary that cannot be read, deletes
    /// `installed.json` (so [`Tool::install`] repairs it) and says why.
    /// Between this check and the run the file could still change; that
    /// needs the same user, who could also change Loadout itself.
    pub fn verify(&self) -> Result<(), String> {
        let binary = self.binary();
        let hashed = std::fs::File::open(&binary).and_then(|mut file| {
            let mut hasher = Sha256::new();
            let mut size = 0u64;
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                size += n as u64;
            }
            Ok((size, hex(&hasher.finalize())))
        });
        let why = match hashed {
            Ok((size, digest)) if size == self.asset.size && digest == self.asset.sha256 => {
                return Ok(());
            }
            Ok(_) => "the installed scanner is not the pinned build (its SHA-256 differs); \
                      reinstall it (`loadout scanner install`, or Settings > Security)"
                .to_owned(),
            Err(err) => format!(
                "the installed scanner cannot be read to check its SHA-256 ({}); reinstall it",
                err.kind()
            ),
        };
        let _ = std::fs::remove_file(self.dir().join(MARKER));
        Err(why)
    }

    /// Deletes partial downloads (`.agent-scan*.loadout-*.tmp`) a killed
    /// install left in [`Tool::dir`].
    fn remove_partial_downloads(&self) {
        let Ok(entries) = std::fs::read_dir(self.dir()) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(".agent-scan")
                && name.contains(".loadout-")
                && name.ends_with(".tmp")
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// Downloads the asset into a temporary file next to the binary,
    /// checks its size and SHA-256 against the pin, then renames it into
    /// place and writes `installed.json`. A mismatch deletes the download
    /// (`scanner_verification_failed`); nothing else is touched. Other
    /// versions in `tools/snyk-agent-scan` are removed afterwards. Already
    /// installed and its SHA-256 still matches: nothing is downloaded.
    pub fn install(
        &self,
        download: &dyn Download,
        progress: &mut dyn FnMut(u64, u64),
        cancel: Option<&AtomicBool>,
    ) -> Result<(), CoreError> {
        if self.installed() && self.verify().is_ok() {
            return Ok(());
        }
        let dir = self.dir();
        std::fs::create_dir_all(&dir).map_err(|err| io("cannot create folder", &dir, &err))?;
        self.remove_partial_downloads();
        let binary = self.binary();
        let temp = crate::apply::temp_path(&binary)?;
        let mut result = self.download_verified(download, &temp, progress, cancel);
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            result = Err(canceled());
        }
        if let Err(err) = result {
            let _ = std::fs::remove_file(&temp);
            return Err(err);
        }
        make_executable(&temp)?;
        let _ = std::fs::remove_file(dir.join(MARKER));
        std::fs::rename(&temp, &binary).map_err(|err| {
            let _ = std::fs::remove_file(&temp);
            io("cannot move the download into place", &binary, &err)
        })?;
        let marker = serde_json::to_vec_pretty(&self.marker()).map_err(|err| {
            CoreError::new(ErrorCode::IoError, format!("cannot write {MARKER}: {err}"))
        })?;
        crate::apply::write_atomic(&crate::fs::RealFs, &dir.join(MARKER), &marker)?;
        self.remove_other_versions();
        Ok(())
    }

    fn download_verified(
        &self,
        download: &dyn Download,
        temp: &Path,
        progress: &mut dyn FnMut(u64, u64),
        cancel: Option<&AtomicBool>,
    ) -> Result<(), CoreError> {
        let file =
            std::fs::File::create(temp).map_err(|err| io("cannot create file", temp, &err))?;
        let mut sink = Hashing {
            inner: std::io::BufWriter::new(file),
            hasher: Sha256::new(),
            written: 0,
        };
        let url = pin::download_url(&self.version, &self.asset.name);
        let size = self.asset.size;
        download.fetch(
            &url,
            &mut sink,
            MAX_DOWNLOAD.max(size),
            &mut |received, total| {
                progress(received, if total == 0 { size } else { total });
            },
            cancel,
        )?;
        sink.inner
            .flush()
            .map_err(|err| io("cannot write file", temp, &err))?;
        let file = sink
            .inner
            .into_inner()
            .map_err(|err| io("cannot write file", temp, err.error()))?;
        file.sync_all()
            .map_err(|err| io("cannot write file", temp, &err))?;
        drop(file);
        let digest = hex(&sink.hasher.finalize());
        if sink.written != self.asset.size || digest != self.asset.sha256 {
            return Err(CoreError::new(
                ErrorCode::ScannerVerificationFailed,
                format!(
                    "the download of {} is not the pinned build (expected {} bytes with SHA-256 {}, \
                     got {} bytes with SHA-256 {digest}); nothing was installed",
                    self.asset.name, self.asset.size, self.asset.sha256, sink.written
                ),
            ));
        }
        Ok(())
    }

    /// Deletes `tools/snyk-agent-scan` (every version); `false` when there
    /// was nothing. Settings are kept.
    pub fn remove(&self) -> Result<bool, CoreError> {
        if !self.root.exists() {
            return Ok(false);
        }
        std::fs::remove_dir_all(&self.root)
            .map_err(|err| io("cannot remove folder", &self.root, &err))?;
        Ok(true)
    }

    /// Older pins left from an earlier Loadout: removed after an install.
    fn remove_other_versions(&self) {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy() != self.version {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

/// `<LOADOUT_HOME>/tools/snyk-agent-scan`.
pub(super) fn tool_root(home: &LoadoutHome) -> PathBuf {
    home.path.join(TOOLS_DIR).join(SCANNER)
}

fn io(what: &str, path: &Path, err: &std::io::Error) -> CoreError {
    CoreError::new(ErrorCode::IoError, format!("{what}: {err}")).with_path(path)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|err| io("cannot make the scanner executable", path, &err))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), CoreError> {
    Ok(())
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Counts and hashes what is written.
struct Hashing<W: Write> {
    inner: W,
    hasher: Sha256,
    written: u64,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Fetches one URL into `out`, at most `max` bytes, reporting
/// `(received, total)` (`total` 0 when unknown) and stopping with
/// `canceled` once `cancel` is set. Other errors are
/// `scanner_download_failed`.
pub trait Download: Send + Sync {
    fn fetch(
        &self,
        url: &str,
        out: &mut dyn Write,
        max: u64,
        progress: &mut dyn FnMut(u64, u64),
        cancel: Option<&AtomicBool>,
    ) -> Result<u64, CoreError>;
}

/// [`Download`] over `ureq` (rustls, the platform's certificate verifier),
/// with the proxy from the injected [`Env`] (`HTTPS_PROXY`, then
/// `ALL_PROXY`; a SOCKS or unusable proxy is an error, never a direct
/// connection). https only; redirects are followed here, one at a time,
/// and only to [`DOWNLOAD_HOSTS`].
pub struct UreqDownload {
    agent: ureq::Agent,
    problem: Option<String>,
}

impl std::fmt::Debug for UreqDownload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UreqDownload").finish_non_exhaustive()
    }
}

impl UreqDownload {
    pub fn new(env: &Env) -> Self {
        let (proxy, problem) = match proxy(
            env,
            &["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"],
        ) {
            Ok(proxy) => (proxy, None),
            Err(why) => (None, Some(why)),
        };
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .https_only(true)
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_global(Some(Duration::from_secs(15 * 60)))
            .user_agent(format!("Loadout/{}", env!("CARGO_PKG_VERSION")))
            .proxy(proxy)
            .tls_config(
                TlsConfig::builder()
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .build()
            .new_agent();
        Self { agent, problem }
    }
}

fn failed(url: &str, why: impl std::fmt::Display) -> CoreError {
    CoreError::new(
        ErrorCode::ScannerDownloadFailed,
        format!(
            "the scanner could not be downloaded from {}: {why}",
            crate::catalog::http::host_of(url)
        ),
    )
}

/// Refuses an address that is not https on one of [`DOWNLOAD_HOSTS`]
/// (default port).
pub fn check_download_url(url: &str) -> Result<url::Url, CoreError> {
    let parsed = url::Url::parse(url).map_err(|_| failed(url, "the address does not parse"))?;
    let allowed = parsed.scheme() == "https"
        && parsed.port().is_none()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed
            .host_str()
            .is_some_and(|host| DOWNLOAD_HOSTS.contains(&host));
    if !allowed {
        return Err(failed(
            url,
            "it redirected to an address Loadout does not download from",
        ));
    }
    Ok(parsed)
}

impl Download for UreqDownload {
    fn fetch(
        &self,
        url: &str,
        out: &mut dyn Write,
        max: u64,
        progress: &mut dyn FnMut(u64, u64),
        cancel: Option<&AtomicBool>,
    ) -> Result<u64, CoreError> {
        if let Some(why) = &self.problem {
            return Err(failed(url, why));
        }
        let mut current = check_download_url(url)?;
        for _ in 0..=MAX_REDIRECTS {
            let response = self
                .agent
                .get(current.as_str())
                .call()
                .map_err(|err| match err {
                    ureq::Error::Timeout(_) => failed(url, "it did not answer in time"),
                    ureq::Error::HostNotFound => failed(url, "the host was not found"),
                    ureq::Error::ConnectionFailed => failed(url, "the connection failed"),
                    other => failed(url, other),
                })?;
            let status = response.status().as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| failed(url, "a redirect without a location"))?;
                let next = current
                    .join(location)
                    .map_err(|_| failed(url, "a redirect that does not parse"))?;
                current = check_download_url(next.as_str())?;
                continue;
            }
            if status != 200 {
                return Err(failed(url, format!("HTTP status {status}")));
            }
            let total = response
                .headers()
                .get("content-length")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
                .unwrap_or(0);
            if total > max {
                return Err(failed(url, "the file is larger than expected"));
            }
            let mut reader = response.into_body().into_reader();
            return copy(&mut reader, out, max, total, progress, cancel).map_err(|err| {
                if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                    canceled()
                } else if err.kind() == std::io::ErrorKind::FileTooLarge {
                    failed(url, "the file is larger than expected")
                } else {
                    failed(url, err)
                }
            });
        }
        Err(failed(url, "too many redirects"))
    }
}

/// `canceled`: the user stopped the install.
pub(super) fn canceled() -> CoreError {
    CoreError::new(
        ErrorCode::Canceled,
        "the scanner install was canceled; nothing was installed",
    )
}

/// Copies `reader` into `out`, at most `max` bytes (`FileTooLarge` past
/// it), reporting progress about every 256 KiB; `Interrupted` once
/// `cancel` is set.
pub fn copy(
    reader: &mut dyn Read,
    out: &mut dyn Write,
    max: u64,
    total: u64,
    progress: &mut dyn FnMut(u64, u64),
    cancel: Option<&AtomicBool>,
) -> std::io::Result<u64> {
    const STEP: u64 = 256 * 1024;
    let mut buf = vec![0u8; 64 * 1024];
    let mut received = 0u64;
    let mut reported = 0u64;
    progress(0, total);
    loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        received += n as u64;
        if received > max {
            return Err(std::io::Error::from(std::io::ErrorKind::FileTooLarge));
        }
        out.write_all(&buf[..n])?;
        if received - reported >= STEP {
            reported = received;
            progress(received, total);
        }
    }
    progress(received, total);
    Ok(received)
}

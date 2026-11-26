//! The pinned Snyk Agent Scan release (#27): one single-file executable per
//! platform, with the SHA-256 of each taken from the release's
//! cleartext-signed `sha256sums.txt.asc` (signing key fingerprint
//! [`SIGNING_FINGERPRINT`]). Loadout verifies a download against these
//! compiled hashes, not against a file fetched at install time: there is no
//! OpenPGP implementation in the tree (sequoia is LGPL, rpgp pulls `rsa`
//! with RUSTSEC-2023-0071, and a hand-written verifier is crypto code we do
//! not want to own). `scripts/verify-agent-scan-pins.*` checks the signature
//! with gpg when the pin is bumped, and a unit test checks that these hashes
//! equal the ones in the signed body of the recorded `.asc`
//! (`fixtures/agent-scan/v0.7.1/sha256sums.txt.asc`).

/// The pinned release (tag `v0.7.1`, published 2026-11-24).
pub const VERSION: &str = "0.7.1";

/// Fingerprint of the key that signs `sha256sums.txt.asc` ("Snyk Limited
/// <code-signing@snyk.io>", Ed25519, expires 2028-07): what the dev script
/// accepts.
pub const SIGNING_FINGERPRINT: &str = "EF4FF9D18D82D5629B82491D2A20F86E4EF131B1";

/// Where the release assets are downloaded from.
const RELEASES: &str = "https://github.com/snyk/agent-scan/releases/download";

/// The largest download accepted (the biggest pinned asset is ~32 MiB).
pub const MAX_DOWNLOAD: u64 = 64 * 1024 * 1024;

/// One platform's asset of the pinned release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinnedAsset {
    /// `std::env::consts::OS` value.
    pub os: &'static str,
    /// `std::env::consts::ARCH` values this asset runs on.
    pub arch: &'static [&'static str],
    pub name: &'static str,
    /// Lowercase hex.
    pub sha256: &'static str,
    pub size: u64,
}

/// Every asset of the pinned release. Windows on Arm runs the x64 build
/// (emulated); Snyk publishes no Arm build for Windows.
pub const PINNED: &[PinnedAsset] = &[
    PinnedAsset {
        os: "windows",
        arch: &["x86_64", "aarch64"],
        name: "agent-scan-0.7.1-windows-x86_64.exe",
        sha256: "b71765b3201931c66e8286fd50f0051b59b6b76828deacfe0d80314363148ef0",
        size: 25_480_568,
    },
    PinnedAsset {
        os: "linux",
        arch: &["x86_64"],
        name: "agent-scan-0.7.1-linux-x86_64",
        sha256: "f9a0a35122aa68b767d53889b517eeececc92efb1c9a709e5b551cd3c2509d0e",
        size: 33_527_680,
    },
    PinnedAsset {
        os: "linux",
        arch: &["aarch64"],
        name: "agent-scan-0.7.1-linux-arm64",
        sha256: "836d521c2f5ac59adac66ec6c8ddece82b6ca2b0d1dbbdceeefb8b107d45ac22",
        size: 29_625_304,
    },
    PinnedAsset {
        os: "macos",
        arch: &["aarch64"],
        name: "agent-scan-0.7.1-macos-arm64",
        sha256: "9f468bd6ae53b01672371e1f9de2cfb871d1198d034ff8f1b6be0bf508a8bde7",
        size: 23_808_160,
    },
    PinnedAsset {
        os: "macos",
        arch: &["x86_64"],
        name: "agent-scan-0.7.1-macos-x86_64",
        sha256: "d64313a4c839223c3c4531f8d05b0a5e680b8dc7eee42fa73c2acde8962bd889",
        size: 25_591_344,
    },
];

/// The asset to install and verify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    /// Lowercase hex SHA-256.
    pub sha256: String,
    pub size: u64,
}

impl From<&PinnedAsset> for Asset {
    fn from(pinned: &PinnedAsset) -> Self {
        Self {
            name: pinned.name.to_owned(),
            sha256: pinned.sha256.to_owned(),
            size: pinned.size,
        }
    }
}

impl Asset {
    /// The asset `bytes` would be (a test stub's own pin).
    pub fn of(name: &str, bytes: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        Self {
            name: name.to_owned(),
            sha256: super::install::hex(&Sha256::digest(bytes)),
            size: bytes.len() as u64,
        }
    }
}

/// The pinned asset for `os` and `arch` (`std::env::consts` values).
pub fn asset_for(os: &str, arch: &str) -> Option<Asset> {
    PINNED
        .iter()
        .find(|pinned| pinned.os == os && pinned.arch.contains(&arch))
        .map(Asset::from)
}

/// The pinned asset this build runs; `None` on a platform Snyk publishes
/// no build for.
pub fn pinned_asset() -> Option<Asset> {
    asset_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// The release download address of `asset`.
pub fn download_url(version: &str, asset: &str) -> String {
    format!("{RELEASES}/v{version}/{asset}")
}

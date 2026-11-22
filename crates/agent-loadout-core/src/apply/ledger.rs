//! The ownership ledger `state/ledger.json` (SPEC 8.1): every native entry
//! Loadout wrote or adopted on this machine, with a keyed hash of what was
//! written. It is the only definition of ownership, machine-local and never
//! versioned.

use std::fmt;
use std::path::Path;

use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::adapters::mapper::Canonical;
use crate::apply::write_atomic;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::applied::Scope;

/// Ledger file in `state/`.
pub const LEDGER_FILE: &str = "ledger.json";
/// HMAC key file in `state/` (32 random bytes, user-only permissions).
pub const LEDGER_KEY_FILE: &str = "ledger.key";

const LEDGER_SCHEMA: u32 = 1;
const HASH_PREFIX: &str = "hmac-sha256:";
const KEY_LEN: usize = 32;

/// What a ledger entry owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A server entry in a container (`mcpServers.github`).
    McpEntry,
    /// A string member of a list (`disabledMcpServers` holding `slack`).
    ListMember,
}

/// How an entry came to be owned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Loadout wrote a new entry.
    Created,
    /// A pre-existing entry the user handed to Loadout (or an identical one
    /// adopted silently).
    Adopted,
}

/// One owned item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub kind: EntryKind,
    pub client: String,
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The native file, absolute, as resolved on this machine.
    pub file: String,
    /// JSON pointer of the container or list (`/mcpServers`), see
    /// [`container_pointer`].
    pub container: String,
    /// The entry key, or the list member.
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
    /// `hmac-sha256:<hex>` over the mapper's canonical form of what was
    /// written ([`LedgerKey::hash`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_hash: Option<String>,
    /// RFC 3339, UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_at: Option<String>,
    /// Abbreviated store commit the entry was written from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_commit: Option<String>,
    /// Delivery mode of the references in the entry (`env`), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
    /// Keys a newer build wrote, kept on rewrite.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl LedgerEntry {
    /// The container this entry is owned in.
    pub fn slot(&self) -> LedgerSlot<'_> {
        LedgerSlot {
            scope: self.scope,
            project_id: self.project_id.as_deref(),
            file: &self.file,
            container: &self.container,
        }
    }

    fn is(&self, slot: &LedgerSlot<'_>, key: &str) -> bool {
        self.slot() == *slot && self.key == key
    }
}

/// Where owned entries live: one container (or list) of one file, in one
/// scope and project (SPEC 8.1, decision 10 of M9). Two projects, or a
/// project and the user scope, never own each other's entries, even in
/// the same file and container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerSlot<'a> {
    pub scope: Scope,
    pub project_id: Option<&'a str>,
    pub file: &'a str,
    /// [`container_pointer`] of the container.
    pub container: &'a str,
}

/// `state/ledger.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    pub schema: u32,
    #[serde(default)]
    pub entries: Vec<LedgerEntry>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            schema: LEDGER_SCHEMA,
            entries: Vec::new(),
            extra: serde_json::Map::new(),
        }
    }
}

impl Ledger {
    /// Reads the ledger in `state_dir`; an absent file is an empty ledger.
    /// A ledger that does not parse is an error, never an empty ledger:
    /// losing it would turn owned entries into foreign ones.
    pub fn load(fs: &dyn Fs, state_dir: &Path) -> Result<Self, CoreError> {
        let path = state_dir.join(LEDGER_FILE);
        if !fs.exists(&path) {
            return Ok(Self::default());
        }
        let text = fs.read_to_string(&path).map_err(|err| {
            CoreError::new(ErrorCode::IoError, format!("cannot read the ledger: {err}"))
                .with_path(&path)
        })?;
        let ledger: Self = serde_json::from_str(&text).map_err(|err| {
            CoreError::new(
                ErrorCode::ParseError,
                format!("the ledger does not parse: {err}"),
            )
            .with_path(&path)
        })?;
        if ledger.schema != LEDGER_SCHEMA {
            return Err(CoreError::new(
                ErrorCode::UnsupportedSchema,
                format!(
                    "ledger schema {} is not supported; this build reads schema {LEDGER_SCHEMA}",
                    ledger.schema
                ),
            )
            .with_path(&path));
        }
        Ok(ledger)
    }

    /// Writes the ledger atomically (temp file, then replace).
    pub fn save(&self, fs: &dyn Fs, state_dir: &Path) -> Result<(), CoreError> {
        let mut text = serde_json::to_string_pretty(self).map_err(|err| {
            CoreError::new(
                ErrorCode::InvalidValue,
                format!("cannot serialize the ledger: {err}"),
            )
        })?;
        text.push('\n');
        write_atomic(fs, &state_dir.join(LEDGER_FILE), text.as_bytes())
    }

    pub fn get(&self, slot: &LedgerSlot<'_>, key: &str) -> Option<&LedgerEntry> {
        self.entries.iter().find(|entry| entry.is(slot, key))
    }

    /// Entries of one slot.
    pub fn in_container<'a>(
        &'a self,
        slot: LedgerSlot<'a>,
    ) -> impl Iterator<Item = &'a LedgerEntry> + 'a {
        self.entries
            .iter()
            .filter(move |entry| entry.slot() == slot)
    }

    /// Replaces the entry with the same slot and key, or appends.
    pub fn upsert(&mut self, entry: LedgerEntry) {
        match self
            .entries
            .iter_mut()
            .find(|known| known.slot() == entry.slot() && known.key == entry.key)
        {
            Some(slot) => *slot = entry,
            None => self.entries.push(entry),
        }
    }

    /// Drops the entry; returns whether it was there.
    pub fn remove(&mut self, slot: &LedgerSlot<'_>, key: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| !entry.is(slot, key));
        self.entries.len() != before
    }
}

/// The HMAC-SHA256 key of ledger hashes (`state/ledger.key`). Keyed because
/// an inline-delivered entry holds secret values: a plain hash would let
/// anyone with the ledger test guesses offline.
#[derive(Clone)]
pub struct LedgerKey([u8; KEY_LEN]);

impl fmt::Debug for LedgerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LedgerKey(..)")
    }
}

impl LedgerKey {
    /// Reads the key in `state_dir`; `None` when there is none yet. A key
    /// of the wrong length is `invalid_value`; it is never replaced
    /// silently, since every hash in the ledger depends on it.
    pub fn load(fs: &dyn Fs, state_dir: &Path) -> Result<Option<Self>, CoreError> {
        let path = state_dir.join(LEDGER_KEY_FILE);
        if !fs.exists(&path) {
            return Ok(None);
        }
        let bytes = fs.read(&path).map_err(|err| {
            CoreError::new(
                ErrorCode::IoError,
                format!("cannot read the ledger key: {err}"),
            )
            .with_path(&path)
        })?;
        let key: [u8; KEY_LEN] = bytes.try_into().map_err(|_| {
            CoreError::new(
                ErrorCode::InvalidValue,
                format!("the ledger key must be {KEY_LEN} bytes"),
            )
            .with_path(&path)
        })?;
        Ok(Some(Self(key)))
    }

    /// [`LedgerKey::load`], creating 32 random bytes with user-only
    /// permissions on first use.
    pub fn load_or_create(fs: &dyn Fs, state_dir: &Path) -> Result<Self, CoreError> {
        if let Some(key) = Self::load(fs, state_dir)? {
            return Ok(key);
        }
        let path = state_dir.join(LEDGER_KEY_FILE);
        let mut key = [0u8; KEY_LEN];
        getrandom::fill(&mut key).map_err(|err| {
            CoreError::new(
                ErrorCode::IoError,
                format!("cannot read random bytes for the ledger key: {err}"),
            )
        })?;
        fs.create_dir_all(state_dir).map_err(|err| {
            CoreError::new(
                ErrorCode::IoError,
                format!("cannot create directory: {err}"),
            )
            .with_path(state_dir)
        })?;
        fs.write_private(&path, &key).map_err(|err| {
            CoreError::new(
                ErrorCode::IoError,
                format!("cannot write the ledger key: {err}"),
            )
            .with_path(&path)
        })?;
        Ok(Self(key))
    }

    /// A key for tests and tools that never persist hashes.
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    /// `hmac-sha256:<hex>` of a canonical form.
    pub fn hash(&self, canonical: &Canonical) -> String {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.0)
            .expect("HMAC accepts any key length");
        mac.update(canonical.as_str().as_bytes());
        let digest = mac.finalize().into_bytes();
        let mut text = String::with_capacity(HASH_PREFIX.len() + digest.len() * 2);
        text.push_str(HASH_PREFIX);
        for byte in digest {
            text.push_str(&format!("{byte:02x}"));
        }
        text
    }
}

/// RFC 6901 JSON pointer of a key path: `/mcpServers`,
/// `/projects/C:~1code~1app/mcpServers`.
pub fn container_pointer<S: AsRef<str>>(keys: &[S]) -> String {
    keys.iter()
        .map(|key| format!("/{}", key.as_ref().replace('~', "~0").replace('/', "~1")))
        .collect()
}

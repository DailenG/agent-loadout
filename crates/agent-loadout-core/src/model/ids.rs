//! Identifier rules (SPEC 5.1). Store files keep ids as plain strings so a
//! bad id survives a rewrite and is reported by validation with
//! [`ErrorCode::InvalidId`] instead of failing to load.

use crate::error::{CoreError, ErrorCode};

/// Longest server, skill, project id or secret alias, in bytes.
pub const MAX_ID_LEN: usize = 64;

/// JSON Schema pattern for server, skill and project ids.
pub const SLUG_PATTERN: &str = "^[a-z0-9][a-z0-9._-]{0,63}$";
/// JSON Schema pattern for machine and apply ids.
pub const ULID_PATTERN: &str = "^[0-7][0-9A-HJKMNP-TV-Z]{25}$";

/// Which identifier rule applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdKind {
    /// Server id: `^[a-z0-9][a-z0-9._-]{0,63}$`.
    Server,
    /// Skill id, same rule as servers.
    Skill,
    /// Project id, same rule as servers.
    Project,
    /// Secret alias: `^[A-Za-z_][A-Za-z0-9_]{0,63}$`, so it doubles as an
    /// environment variable name.
    SecretAlias,
    /// Environment variable name: `^[A-Za-z_][A-Za-z0-9_]*$`.
    EnvVar,
    /// Machine id: a ULID (26 Crockford base32 characters, upper case).
    Machine,
    /// Apply id: a ULID, as for machines.
    Apply,
}

impl IdKind {
    /// Short English noun for messages ("server id", "secret alias").
    pub const fn noun(self) -> &'static str {
        match self {
            IdKind::Server => "server id",
            IdKind::Skill => "skill id",
            IdKind::Project => "project id",
            IdKind::SecretAlias => "secret alias",
            IdKind::EnvVar => "environment variable name",
            IdKind::Machine => "machine id",
            IdKind::Apply => "apply id",
        }
    }

    const fn rule(self) -> &'static str {
        match self {
            IdKind::Server | IdKind::Skill | IdKind::Project => {
                "1 to 64 characters from a-z, 0-9, '.', '_', '-', starting with a letter or digit"
            }
            IdKind::SecretAlias => {
                "1 to 64 characters from A-Z, a-z, 0-9, '_', not starting with a digit"
            }
            IdKind::EnvVar => "characters from A-Z, a-z, 0-9, '_', not starting with a digit",
            IdKind::Machine | IdKind::Apply => "a ULID: 26 upper-case Crockford base32 characters",
        }
    }
}

/// Whether `value` satisfies the rule for `kind`.
pub fn is_valid(kind: IdKind, value: &str) -> bool {
    match kind {
        IdKind::Server | IdKind::Skill | IdKind::Project => is_slug(value),
        IdKind::SecretAlias => value.len() <= MAX_ID_LEN && is_env_name(value),
        IdKind::EnvVar => is_env_name(value),
        IdKind::Machine | IdKind::Apply => is_ulid(value),
    }
}

/// Checks `value` against the rule for `kind`.
pub fn check(kind: IdKind, value: &str) -> Result<(), CoreError> {
    if is_valid(kind, value) {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorCode::InvalidId,
            invalid_message(kind, value),
        ))
    }
}

/// English message for an id that breaks its rule.
pub(crate) fn invalid_message(kind: IdKind, value: &str) -> String {
    format!(
        "invalid {} {value:?}: expected {}",
        kind.noun(),
        kind.rule()
    )
}

fn is_slug(value: &str) -> bool {
    let bytes = value.as_bytes();
    let Some(first) = bytes.first() else {
        return false;
    };
    bytes.len() <= MAX_ID_LEN
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && bytes.iter().all(|&b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

fn is_env_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    let Some(first) = bytes.first() else {
        return false;
    };
    (first.is_ascii_alphabetic() || *first == b'_')
        && bytes
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}

fn is_ulid(value: &str) -> bool {
    const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let bytes = value.as_bytes();
    // The first character carries 3 bits of a 128-bit value, so it is 0-7.
    bytes.len() == 26 && bytes[0] <= b'7' && bytes.iter().all(|b| CROCKFORD.contains(b))
}

/// Abbreviated or full lower-case hex git object id (SHA-1 or SHA-256).
pub fn is_commit_id(value: &str) -> bool {
    (7..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

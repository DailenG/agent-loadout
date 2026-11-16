//! Typed core errors. The CLI and the GUI localize from [`ErrorCode`]; the
//! message is English diagnostic detail, never state.

use std::fmt;
use std::path::{Path, PathBuf};

/// Stable error code, serialized as `snake_case` (see [`ErrorCode::as_str`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorCode {
    /// A path argument, override or path field is unusable (for example
    /// relative, or personal where a placeholder is required).
    InvalidPath,
    /// The user profile directory could not be determined.
    ProfileNotFound,
    /// A file could not be read or written.
    IoError,
    /// A file is not valid UTF-8, or not valid in its format (TOML, JSON, JSONC).
    ParseError,
    /// A store file declares a `schema` version this build does not read.
    UnsupportedSchema,
    /// A required field is absent.
    MissingField,
    /// A field has the wrong type or a value outside its allowed set.
    InvalidValue,
    /// An identifier breaks the rules of SPEC 5.1.
    InvalidId,
    /// An id disagrees with the file or directory name that holds it.
    IdMismatch,
    /// A field is present where the rest of the definition forbids it
    /// (for example `url` on a stdio server).
    FieldNotAllowed,
    /// A `client.<id>` key is not in that client's passthrough allowlist.
    PassthroughNotAllowed,
    /// A template or secret reference does not parse (SPEC 11.2).
    InvalidRef,
    /// A client id is not in the adapter table. Warning: kept as written.
    UnknownClient,
    /// An OS name is not `windows`, `linux` or `macos`. Warning: kept as written.
    UnknownOs,
    /// Another process holds `state/loadout.lock` and the wait timed out.
    Locked,
    /// The store repository could not be read or written.
    GitError,
    /// An entity with this id already exists.
    AlreadyExists,
    /// No entity with this id exists.
    NotFound,
    /// A native client file parses, but a value on the path being read or
    /// edited has another kind (for example `mcpServers` is an array).
    UnexpectedShape,
    /// A client has no target on this operating system (Claude Desktop on
    /// Linux).
    NotSupportedOnOs,
    /// A client has no target in the requested scope (Claude Desktop at
    /// project scope).
    ScopeNotSupported,
    /// A client cannot run a server over this transport (HTTP on Claude
    /// Desktop). The target is blocked; nothing is dropped silently.
    UnsupportedTransport,
    /// A client has no native field for a set server field (`cwd` on Claude
    /// Desktop). The target is blocked; nothing is dropped silently.
    UnsupportedField,
    /// Warning: an MSIX install keeps its own copy of a client file, so both
    /// copies are written (Claude Desktop on Windows).
    MsixDualWrite,
}

impl ErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidPath => "invalid_path",
            ErrorCode::ProfileNotFound => "profile_not_found",
            ErrorCode::IoError => "io_error",
            ErrorCode::ParseError => "parse_error",
            ErrorCode::UnsupportedSchema => "unsupported_schema",
            ErrorCode::MissingField => "missing_field",
            ErrorCode::InvalidValue => "invalid_value",
            ErrorCode::InvalidId => "invalid_id",
            ErrorCode::IdMismatch => "id_mismatch",
            ErrorCode::FieldNotAllowed => "field_not_allowed",
            ErrorCode::PassthroughNotAllowed => "passthrough_not_allowed",
            ErrorCode::InvalidRef => "invalid_ref",
            ErrorCode::UnknownClient => "unknown_client",
            ErrorCode::UnknownOs => "unknown_os",
            ErrorCode::Locked => "locked",
            ErrorCode::GitError => "git_error",
            ErrorCode::AlreadyExists => "already_exists",
            ErrorCode::NotFound => "not_found",
            ErrorCode::UnexpectedShape => "unexpected_shape",
            ErrorCode::NotSupportedOnOs => "not_supported_on_os",
            ErrorCode::ScopeNotSupported => "scope_not_supported",
            ErrorCode::UnsupportedTransport => "unsupported_transport",
            ErrorCode::UnsupportedField => "unsupported_field",
            ErrorCode::MsixDualWrite => "msix_dual_write",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreError {
    code: ErrorCode,
    message: String,
    path: Option<PathBuf>,
}

impl CoreError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            path: None,
        }
    }

    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if let Some(path) = &self.path {
            write!(f, ": {}", path.display())?;
        }
        Ok(())
    }
}

impl std::error::Error for CoreError {}

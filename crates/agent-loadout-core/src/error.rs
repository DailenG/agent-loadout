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
    /// Warning: a client treats another entry name as the same server (Pi
    /// counts names that differ only in `-` and `_` as one), so writing the
    /// entry would clash with it.
    Collision,
    /// Warning: an installed extension takes over the client's MCP support
    /// (Pi with `pi-mcp-adapter`), so the client's own MCP config may not be
    /// what loads.
    ClientMcpReplacedByExtension,
    /// A client file kept changing between planning and writing (SPEC 8.4
    /// step 1, after three re-plans). The file is left as it is.
    FileChangedDuringApply,
    /// The edited file did not read back as planned: an owned entry differs
    /// from the desired one, or something outside the owned entries changed
    /// (SPEC 8.4 step 3). The file is left as it is.
    SelfCheckFailed,
    /// Another process kept a client file open for longer than the retry
    /// window (Windows sharing violation, SPEC 8.4 step 4).
    FileLocked,
    /// No delivery mode of this build can bring a reference (`${secret:..}`,
    /// or `${env:..}` in a field the client does not expand) to this client.
    /// The entry is blocked; nothing is written in plain text.
    DeliveryUnavailable,
    /// Warning: a known client has no adapter in this build yet, so nothing
    /// is written for it.
    NoAdapter,
    /// A native entry holds literal values that may be secrets; importing it
    /// needs secret extraction (SPEC 11.7) instead of copying the values.
    NeedsSecretExtraction,
    /// The operation changes state that later operations changed too and
    /// needs an explicit confirmation (rolling back an older apply, SPEC 8.7).
    ConfirmationRequired,
    /// Warning: an apply run did not finish; its journal lists what each
    /// file got (SPEC 8.4 step 6). Nothing is replayed.
    UnfinishedApply,
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
            ErrorCode::Collision => "collision",
            ErrorCode::ClientMcpReplacedByExtension => "client_mcp_replaced_by_extension",
            ErrorCode::FileChangedDuringApply => "file_changed_during_apply",
            ErrorCode::SelfCheckFailed => "self_check_failed",
            ErrorCode::FileLocked => "file_locked",
            ErrorCode::DeliveryUnavailable => "delivery_unavailable",
            ErrorCode::NoAdapter => "no_adapter",
            ErrorCode::NeedsSecretExtraction => "needs_secret_extraction",
            ErrorCode::ConfirmationRequired => "confirmation_required",
            ErrorCode::UnfinishedApply => "unfinished_apply",
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

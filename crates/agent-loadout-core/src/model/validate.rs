//! Validation results and per-file dispatch for store files (SPEC 5).
//!
//! Every model has a `validate` method returning [`Diagnostics`]. Errors make
//! a file invalid; warnings (unknown client ids, unknown OS names) are
//! reported but the data is kept, so a store written by a newer build
//! survives being opened by an older one.

use std::fmt;
use std::path::{Component, Path};

use crate::error::{CoreError, ErrorCode};
use crate::model::TomlDocument;
use crate::model::applied::AppliedRecord;
use crate::model::clients::{ClientPrefsFile, ClientSelection, known_client};
use crate::model::ids::{self, IdKind};
use crate::model::machine::MachineOverlay;
use crate::model::project::{ManifestLayer, ProjectManifest};
use crate::model::secrets::SecretCatalog;
use crate::model::server::Server;
use crate::model::settings::Settings;
use crate::model::skill_meta::SkillMeta;
use crate::secrets::grammar::{SecretRef, parse_template};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// One finding about one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: ErrorCode,
    /// Dotted TOML key path (`enable.clients[1]`, `client.codex.foo`);
    /// empty for the whole file.
    pub path: String,
    /// English detail for logs and the CLI; frontends localize from `code`.
    pub message: String,
}

impl Diagnostic {
    pub fn error(code: ErrorCode, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code,
            path: path.into(),
            message: message.into(),
        }
    }

    pub fn warning(code: ErrorCode, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(code, path, message)
        }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// The diagnostic as a [`CoreError`] about `file`.
    pub fn to_core_error(&self, file: Option<&Path>) -> CoreError {
        let error = CoreError::new(self.code, self.to_string());
        match file {
            Some(file) => error.with_path(file),
            None => error,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

/// All findings for one file, in discovery order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnostics(Vec<Diagnostic>);

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.0.push(diagnostic);
    }

    pub fn error(&mut self, code: ErrorCode, path: impl Into<String>, message: impl Into<String>) {
        self.push(Diagnostic::error(code, path, message));
    }

    pub fn warning(
        &mut self,
        code: ErrorCode,
        path: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.push(Diagnostic::warning(code, path, message));
    }

    /// No errors (warnings allowed).
    pub fn is_valid(&self) -> bool {
        !self.0.iter().any(Diagnostic::is_error)
    }

    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter().filter(|d| d.is_error())
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter().filter(|d| !d.is_error())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.0
    }

    /// The first error as a [`CoreError`] about `file`, if any.
    pub fn first_error(&self, file: Option<&Path>) -> Option<CoreError> {
        self.errors().next().map(|d| d.to_core_error(file))
    }

    pub(crate) fn check_id(&mut self, kind: IdKind, value: &str, path: &str) -> bool {
        let valid = ids::is_valid(kind, value);
        if !valid {
            self.error(
                ErrorCode::InvalidId,
                path,
                ids::invalid_message(kind, value),
            );
        }
        valid
    }

    /// `id` must be valid for `kind` and, when the file or directory name is
    /// known, equal to it (SPEC 5.1).
    pub(crate) fn check_named_id(
        &mut self,
        kind: IdKind,
        id: &str,
        path: &str,
        expected: Option<&str>,
    ) {
        if self.check_id(kind, id, path)
            && let Some(expected) = expected.filter(|expected| *expected != id)
        {
            self.error(
                ErrorCode::IdMismatch,
                path,
                format!(
                    "{} {id:?} does not match its file name {expected:?}",
                    kind.noun()
                ),
            );
        }
    }

    /// Template grammar (SPEC 11.2) for one string field.
    pub(crate) fn check_template(&mut self, value: &str, path: &str) {
        if let Err(err) = parse_template(value) {
            self.error(ErrorCode::InvalidRef, path, err.message());
        }
    }

    /// Whole-value provider reference (`secrets.toml` `ref`).
    pub(crate) fn check_secret_ref(&mut self, value: &str, path: &str) {
        if let Err(err) = SecretRef::parse(value) {
            self.error(ErrorCode::InvalidRef, path, err.message());
        }
    }

    /// Unknown client ids are warnings and are kept as written.
    pub(crate) fn check_client_ids<'a>(
        &mut self,
        ids: impl IntoIterator<Item = &'a String>,
        path: &str,
    ) {
        for (index, id) in ids.into_iter().enumerate() {
            if known_client(id).is_none() {
                self.warning(
                    ErrorCode::UnknownClient,
                    at(path, index),
                    format!("unknown client {id:?}; kept as written"),
                );
            }
        }
    }

    pub(crate) fn check_client_selection(&mut self, selection: &ClientSelection, path: &str) {
        if let ClientSelection::List(ids) = selection {
            self.check_client_ids(ids, path);
        }
    }
}

impl IntoIterator for Diagnostics {
    type Item = Diagnostic;
    type IntoIter = std::vec::IntoIter<Diagnostic>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// `prefix.key`, quoting `key` when it is not a bare TOML key.
pub(crate) fn join(prefix: &str, key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'));
    let key = if bare {
        key.to_owned()
    } else {
        format!("{key:?}")
    };
    if prefix.is_empty() {
        key
    } else {
        format!("{prefix}.{key}")
    }
}

/// `prefix[index]`.
pub(crate) fn at(prefix: &str, index: usize) -> String {
    format!("{prefix}[{index}]")
}

/// A store file, identified by its path relative to the store root
/// (SPEC 4.5). Carries the id implied by the file or directory name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreFile {
    /// `servers/<id>.toml`
    Server { id: String },
    /// `secrets.toml`
    Secrets,
    /// `clients.toml`
    Clients,
    /// `settings.toml`
    Settings,
    /// `skill-meta/<id>.toml`
    SkillMeta { id: String },
    /// `projects/<id>/project.toml` or `projects/<id>/project.local.toml`
    Project { id: String, local: bool },
    /// `machines/<machine-id>.toml`
    Machine { machine_id: String },
    /// `machines/<machine-id>.applied.toml`
    Applied { machine_id: String },
}

impl StoreFile {
    /// Classifies a store-relative path; `None` for files that are not
    /// schema files (skill content, `loadout.toml`, unknown files).
    pub fn classify(relative: &Path) -> Option<Self> {
        let parts: Vec<&str> = relative
            .components()
            .map(|component| match component {
                Component::Normal(part) => part.to_str(),
                _ => None,
            })
            .collect::<Option<_>>()?;
        let stem = |name: &str, suffix: &str| name.strip_suffix(suffix).map(str::to_owned);
        match parts.as_slice() {
            ["secrets.toml"] => Some(Self::Secrets),
            ["clients.toml"] => Some(Self::Clients),
            ["settings.toml"] => Some(Self::Settings),
            ["servers", name] => stem(name, ".toml").map(|id| Self::Server { id }),
            ["skill-meta", name] => stem(name, ".toml").map(|id| Self::SkillMeta { id }),
            ["projects", id, "project.toml"] => Some(Self::Project {
                id: (*id).to_owned(),
                local: false,
            }),
            ["projects", id, "project.local.toml"] => Some(Self::Project {
                id: (*id).to_owned(),
                local: true,
            }),
            ["machines", name] => match stem(name, ".applied.toml") {
                Some(machine_id) => Some(Self::Applied { machine_id }),
                None => stem(name, ".toml").map(|machine_id| Self::Machine { machine_id }),
            },
            _ => None,
        }
    }
}

/// A parsed store file of any kind.
#[derive(Debug, Clone)]
pub enum StoreDocument {
    /// Boxed: a server is several times larger than the other kinds.
    Server(Box<TomlDocument<Server>>),
    Secrets(TomlDocument<SecretCatalog>),
    Clients(TomlDocument<ClientPrefsFile>),
    Settings(TomlDocument<Settings>),
    SkillMeta(TomlDocument<SkillMeta>),
    Project(TomlDocument<ProjectManifest>),
    Machine(TomlDocument<MachineOverlay>),
    Applied(TomlDocument<AppliedRecord>),
}

impl StoreDocument {
    /// Parses `text` as the kind of `file`.
    pub fn parse(file: &StoreFile, text: &str) -> Result<Self, Diagnostic> {
        Ok(match file {
            StoreFile::Server { .. } => Self::Server(Box::new(TomlDocument::parse(text)?)),
            StoreFile::Secrets => Self::Secrets(TomlDocument::parse(text)?),
            StoreFile::Clients => Self::Clients(TomlDocument::parse(text)?),
            StoreFile::Settings => Self::Settings(TomlDocument::parse(text)?),
            StoreFile::SkillMeta { .. } => Self::SkillMeta(TomlDocument::parse(text)?),
            StoreFile::Project { .. } => Self::Project(TomlDocument::parse(text)?),
            StoreFile::Machine { .. } => Self::Machine(TomlDocument::parse(text)?),
            StoreFile::Applied { .. } => Self::Applied(TomlDocument::parse(text)?),
        })
    }

    /// Validates against the rules for `file`. `shared_project_id` is the id
    /// of the sibling `project.toml`, when there is one; it is only read for
    /// `project.local.toml`.
    pub fn validate(&self, file: &StoreFile, shared_project_id: Option<&str>) -> Diagnostics {
        let name = |file: &StoreFile| match file {
            StoreFile::Server { id }
            | StoreFile::SkillMeta { id }
            | StoreFile::Project { id, .. } => Some(id.clone()),
            StoreFile::Machine { machine_id } | StoreFile::Applied { machine_id } => {
                Some(machine_id.clone())
            }
            StoreFile::Secrets | StoreFile::Clients | StoreFile::Settings => None,
        };
        let name = name(file);
        let name = name.as_deref();
        match self {
            Self::Server(doc) => doc.value().validate(name),
            Self::Secrets(doc) => doc.value().validate(),
            Self::Clients(doc) => doc.value().validate(),
            Self::Settings(doc) => doc.value().validate(),
            Self::SkillMeta(doc) => doc.value().validate(name),
            Self::Project(doc) => {
                let local = matches!(file, StoreFile::Project { local: true, .. });
                let layer = if local {
                    ManifestLayer::Local {
                        shared_id: shared_project_id,
                    }
                } else {
                    ManifestLayer::Shared
                };
                doc.value().validate(layer, name)
            }
            Self::Machine(doc) => doc.value().validate(name),
            Self::Applied(doc) => doc.value().validate(name),
        }
    }

    /// Serializes back to text, keeping comments, formatting and unknown keys.
    pub fn to_text(&self) -> Result<String, CoreError> {
        match self {
            Self::Server(doc) => doc.to_text(),
            Self::Secrets(doc) => doc.to_text(),
            Self::Clients(doc) => doc.to_text(),
            Self::Settings(doc) => doc.to_text(),
            Self::SkillMeta(doc) => doc.to_text(),
            Self::Project(doc) => doc.to_text(),
            Self::Machine(doc) => doc.to_text(),
            Self::Applied(doc) => doc.to_text(),
        }
    }
}

/// Parses and validates one store file. A parse failure is the only
/// diagnostic when the text does not load.
pub fn validate_store_file(
    file: &StoreFile,
    text: &str,
    shared_project_id: Option<&str>,
) -> Diagnostics {
    match StoreDocument::parse(file, text) {
        Ok(doc) => doc.validate(file, shared_project_id),
        Err(diagnostic) => {
            let mut diagnostics = Diagnostics::new();
            diagnostics.push(diagnostic);
            diagnostics
        }
    }
}

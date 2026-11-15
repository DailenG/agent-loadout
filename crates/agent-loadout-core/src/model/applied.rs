//! `store/machines/<machine-id>.applied.toml`: the last apply of a machine
//! (SPEC 8.7). No file content, values or hashes; those stay machine-local.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;
use crate::model::StoreModel;
use crate::model::clients::known_client;
use crate::model::ids::{self, IdKind};
use crate::model::validate::{Diagnostics, at, join};

/// What started an apply run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ApplyTrigger {
    Instant,
    Review,
    Restore,
    Rollback,
    Pull,
}

/// Result of an apply run or of one target (also the `Loadout-Apply` trailer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ApplyOutcome {
    Ok,
    Partial,
    Blocked,
    Failed,
}

/// Layer a target belongs to (SPEC 6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    User,
    Project,
    ProjectLocal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "Loadout apply record (store/machines/<machine-id>.applied.toml)")]
pub struct AppliedRecord {
    /// Schema version; always 1.
    #[schemars(extend("const" = 1))]
    pub schema: i64,
    /// ULID; equals the file name without `.applied.toml`.
    #[schemars(extend("pattern" = ids::ULID_PATTERN))]
    pub machine_id: String,
    /// ULID of the apply run.
    #[schemars(extend("pattern" = ids::ULID_PATTERN))]
    pub apply_id: String,
    /// When the run finished (TOML offset date-time).
    #[schemars(with = "String", extend("format" = "date-time"))]
    pub time: toml::value::Datetime,
    /// Store commit that was applied.
    pub store_revision: String,
    pub trigger: ApplyTrigger,
    pub outcome: ApplyOutcome,
    /// One record per target file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<AppliedTarget>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

impl StoreModel for AppliedRecord {
    const SCHEMA_NAME: &'static str = "applied";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AppliedTarget {
    pub client: String,
    pub scope: Scope,
    /// Required for project scopes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Target file written with `~` or `<P>` placeholders, never a personal path.
    pub file: String,
    pub create: u32,
    pub update: u32,
    pub remove: u32,
    pub disable: u32,
    pub outcome: ApplyOutcome,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

impl AppliedRecord {
    /// Validates per SPEC 8.7. `file_id` is the file name without
    /// `.applied.toml`.
    pub fn validate(&self, file_id: Option<&str>) -> Diagnostics {
        let mut d = Diagnostics::new();
        d.check_named_id(IdKind::Machine, &self.machine_id, "machine_id", file_id);
        d.check_id(IdKind::Apply, &self.apply_id, "apply_id");
        let complete = self.time.date.is_some() && self.time.time.is_some();
        if !complete || self.time.offset.is_none() {
            d.error(
                ErrorCode::InvalidValue,
                "time",
                "`time` must be an offset date-time such as 2026-01-01T00:00:00Z",
            );
        }
        if !ids::is_commit_id(&self.store_revision) {
            d.error(
                ErrorCode::InvalidValue,
                "store_revision",
                format!("{:?} is not a git commit id", self.store_revision),
            );
        }
        for (index, target) in self.targets.iter().enumerate() {
            let path = at("targets", index);
            if known_client(&target.client).is_none() {
                d.warning(
                    ErrorCode::UnknownClient,
                    join(&path, "client"),
                    format!("unknown client {:?}; kept as written", target.client),
                );
            }
            match (target.scope, &target.project_id) {
                (Scope::Project | Scope::ProjectLocal, None) => d.error(
                    ErrorCode::MissingField,
                    join(&path, "project_id"),
                    "project targets need `project_id`",
                ),
                (_, Some(id)) => {
                    d.check_id(IdKind::Project, id, &join(&path, "project_id"));
                }
                (Scope::User, None) => {}
            }
            if !has_placeholder_root(&target.file) {
                d.error(
                    ErrorCode::InvalidPath,
                    join(&path, "file"),
                    format!("{:?} must start with `~/` or `<P>/`", target.file),
                );
            }
        }
        d
    }
}

fn has_placeholder_root(file: &str) -> bool {
    ["~/", "<P>/"].iter().any(|root| file.starts_with(root))
}

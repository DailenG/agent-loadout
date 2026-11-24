//! `store/skill-meta/<id>.toml`: skill metadata kept outside the skill
//! directory (SPEC 12.1).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;
use crate::model::clients::ClientSelection;
use crate::model::ids::{self, IdKind};
use crate::model::validate::Diagnostics;
use crate::model::{SCHEMA_VERSION, StoreModel};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SkillSourceKind {
    Git,
    Local,
    AgentsCli,
    Catalog,
    Authored,
}

/// How a skill reaches client skill directories (SPEC 12.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DistributionMethod {
    Link,
    Copy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "Loadout skill metadata (store/skill-meta/<id>.toml)")]
pub struct SkillMeta {
    /// Schema version; always 1.
    #[schemars(extend("const" = 1))]
    pub schema: i64,
    /// Skill id; equals the file name. Identity, not the name.
    #[schemars(extend("pattern" = ids::SLUG_PATTERN))]
    pub id: String,
    /// Name from the SKILL.md frontmatter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SkillSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable: Option<SkillEnable>,
    /// The bundled copy Loadout wrote (the Loadout skill for agents, #8).
    /// Marks the skill as bundled on its own, without `[source]`: 0.5.x
    /// reads an unknown `[source] kind` as an error but keeps this table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundled: Option<BundledCopy>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

impl StoreModel for SkillMeta {
    const SCHEMA_NAME: &'static str = "skill-meta";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SkillSource {
    pub kind: SkillSourceKind,
    /// Repository URL (git).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Skill directory inside the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Branch, tag or commit followed for updates (git); a commit pins it.
    #[serde(rename = "ref", default, skip_serializing_if = "Option::is_none")]
    pub git_ref: Option<String>,
    /// Commit the store copy came from (git).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The catalog entry it was installed from, `<catalog>/<entry id>`
    /// (#26).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SkillEnable {
    /// Client ids, or "all-detected". Unknown ids are a warning and are kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clients: Option<ClientSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<DistributionMethod>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

/// The copy of a bundled skill Loadout last wrote into the store: content
/// with another hash was edited by the user and is never updated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BundledCopy {
    /// The Loadout version the content shipped with.
    pub version: String,
    /// `sha256:<hex>` of the content as written (paths and bytes, sorted).
    pub hash: String,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

impl SkillMeta {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            schema: SCHEMA_VERSION,
            id: id.into(),
            name: None,
            tags: None,
            source: None,
            enable: None,
            bundled: None,
            extra: toml::Table::new(),
        }
    }

    /// Validates per SPEC 12.1. `file_id` is the file name without `.toml`.
    pub fn validate(&self, file_id: Option<&str>) -> Diagnostics {
        let mut d = Diagnostics::new();
        d.check_named_id(IdKind::Skill, &self.id, "id", file_id);
        if let Some(source) = &self.source {
            if source.kind == SkillSourceKind::Git && source.url.is_none() {
                d.error(
                    ErrorCode::MissingField,
                    "source.url",
                    "git sources need `url`",
                );
            }
            if let Some(commit) = &source.commit
                && !ids::is_commit_id(commit)
            {
                d.error(
                    ErrorCode::InvalidValue,
                    "source.commit",
                    format!("{commit:?} is not a git commit id"),
                );
            }
        }
        if let Some(clients) = self.enable.as_ref().and_then(|e| e.clients.as_ref()) {
            d.check_client_selection(clients, "enable.clients");
        }
        d
    }
}

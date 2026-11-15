//! Project manifests: `<P>/.loadout/project.toml` (shared) and
//! `<P>/.loadout/project.local.toml` (private), mirrored in the store under
//! `projects/<project-id>/` (SPEC 5.5).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;
use crate::model::clients::ClientSelection;
use crate::model::ids::IdKind;
use crate::model::server::ServerBody;
use crate::model::validate::{Diagnostics, at, join};
use crate::model::{SCHEMA_VERSION, StoreModel};

/// Both manifest files share this schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(title = "Loadout project manifest (.loadout/project.toml, .loadout/project.local.toml)")]
pub struct ProjectManifest {
    /// Schema version; always 1.
    #[schemars(extend("const" = 1))]
    pub schema: i64,
    /// Project id. Required in `project.toml`, and in `project.local.toml`
    /// when there is no shared file (a private project).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("pattern" = crate::model::ids::SLUG_PATTERN))]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defaults: Option<ProjectDefaults>,
    /// Servers by name: overrides (`from`), suppressions (`enabled = false`)
    /// or complete project-only definitions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub servers: Option<BTreeMap<String, ProjectServer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<ProjectSkills>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

impl StoreModel for ProjectManifest {
    const SCHEMA_NAME: &'static str = "project";
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectDefaults {
    /// Clients that receive project-scope files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clients: Option<ClientSelection>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectServer {
    /// Store server used as the base of this entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// `false` alone suppresses the same-named system server in this folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Clients that get this entry; defaults to `defaults.clients`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clients: Option<ClientSelection>,
    /// Inherited keys to remove, as dotted paths (`env.FOO`, `headers.X-Debug`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unset: Option<Vec<String>>,
    #[serde(flatten)]
    pub body: ServerBody,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectSkills {
    /// Skill ids enabled in this project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable: Option<Vec<String>>,
    /// `false`: link and hide via `.git/info/exclude`; `true`: copy files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<bool>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: toml::Table,
}

/// Which manifest file is being validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestLayer<'a> {
    /// `project.toml`.
    Shared,
    /// `project.local.toml`; `shared_id` is the id of the sibling
    /// `project.toml`, `None` when the project is private-only.
    Local { shared_id: Option<&'a str> },
}

impl ProjectManifest {
    pub fn new(id: Option<String>) -> Self {
        Self {
            schema: SCHEMA_VERSION,
            id,
            name: None,
            defaults: None,
            servers: None,
            skills: None,
            extra: toml::Table::new(),
        }
    }

    /// Validates per SPEC 5.5 and 6.2. `dir_id` is the store mirror
    /// directory name (`projects/<dir_id>/`), when known.
    pub fn validate(&self, layer: ManifestLayer<'_>, dir_id: Option<&str>) -> Diagnostics {
        let mut d = Diagnostics::new();
        match (layer, self.id.as_deref()) {
            (ManifestLayer::Shared, None) => {
                d.error(ErrorCode::MissingField, "id", "project.toml needs `id`");
            }
            (ManifestLayer::Local { shared_id: None }, None) => d.error(
                ErrorCode::MissingField,
                "id",
                "project.local.toml without a project.toml is a private project and needs `id`",
            ),
            (ManifestLayer::Local { shared_id: Some(_) }, None) => {}
            (ManifestLayer::Shared | ManifestLayer::Local { shared_id: None }, Some(id)) => {
                d.check_named_id(IdKind::Project, id, "id", dir_id);
            }
            (
                ManifestLayer::Local {
                    shared_id: Some(shared),
                },
                Some(id),
            ) => {
                if d.check_id(IdKind::Project, id, "id") && id != shared {
                    d.warning(
                        ErrorCode::IdMismatch,
                        "id",
                        format!("id {id:?} differs from project.toml's {shared:?}, which wins"),
                    );
                }
            }
        }
        if let Some(clients) = self.defaults.as_ref().and_then(|x| x.clients.as_ref()) {
            d.check_client_selection(clients, "defaults.clients");
        }
        // A local file next to a shared one merges over it by server name
        // (SPEC 6.2), so its tables may be partial.
        let standalone = !matches!(layer, ManifestLayer::Local { shared_id: Some(_) });
        for (name, server) in self.servers.iter().flatten() {
            let path = join("servers", name);
            d.check_id(IdKind::Server, name, &path);
            server.validate_into(&path, standalone, &mut d);
        }
        if let Some(skills) = self
            .skills
            .as_ref()
            .and_then(|skills| skills.enable.as_ref())
        {
            for (index, id) in skills.iter().enumerate() {
                d.check_id(IdKind::Skill, id, &at("skills.enable", index));
            }
        }
        d
    }
}

impl ProjectServer {
    /// `enabled = false` with no definition: hides a system server here.
    pub fn is_suppression(&self) -> bool {
        self.enabled == Some(false) && self.from.is_none() && self.body == ServerBody::default()
    }

    fn validate_into(&self, path: &str, standalone: bool, d: &mut Diagnostics) {
        if let Some(from) = &self.from {
            d.check_id(IdKind::Server, from, &join(path, "from"));
        }
        if let Some(clients) = &self.clients {
            d.check_client_selection(clients, &join(path, "clients"));
        }
        for (index, entry) in self.unset.iter().flatten().enumerate() {
            if entry.split('.').any(str::is_empty) {
                d.error(
                    ErrorCode::InvalidValue,
                    at(&join(path, "unset"), index),
                    format!("invalid unset path {entry:?}; expected a dotted key such as env.FOO"),
                );
            }
        }
        // Without `from` the table is a complete definition (SPEC 6.2),
        // unless it may layer over a same-named shared entry.
        let complete = standalone && self.from.is_none() && !self.is_suppression();
        self.body.validate_into(path, complete, d);
    }
}

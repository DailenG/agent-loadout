//! The declarative adapter table entry (SPEC 7.1 `AdapterSpec`, `McpSpec`)
//! and the target files it resolves to.

use std::path::{Path, PathBuf};

use crate::adapters::caps::{Caps, ProjectDisable};
use crate::adapters::detect::Probe;
use crate::adapters::location::{Location, project_key};
use crate::adapters::mapper::EntryMapper;
use crate::env::{Env, Os};
use crate::error::{CoreError, ErrorCode};
use crate::formats::{Document, Format};
use crate::fs::Fs;
use crate::model::applied::Scope;

/// Support tier (SPEC 20.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    /// Daily clients, first wave.
    A,
    /// Daily clients, second wave.
    B,
    /// Supported.
    C,
    /// Best effort or ported.
    D,
}

/// One client: identity, detection probes and MCP targets.
pub struct AdapterSpec {
    /// Client id (`claude-code`), as used in store files.
    pub id: &'static str,
    pub display_name: &'static str,
    pub tier: Tier,
    /// Detection probes; any hit detects the client (SPEC 7.7).
    pub detect: &'static [Probe],
    pub mcp: Option<McpSpec>,
}

/// Where and how a client keeps MCP servers.
pub struct McpSpec {
    /// Operating systems the client has MCP targets on.
    pub oses: &'static [Os],
    /// User-scope candidates: the first existing one wins, else `[0]` is
    /// created.
    pub user: &'static [Location],
    /// Extra copies of the user file, each written too when it exists (the
    /// Claude Desktop MSIX copy). Usually [`Location::Glob`].
    pub user_mirrors: &'static [Location],
    /// Shared project candidates, relative to `<P>`.
    pub project: &'static [Location],
    /// Private project candidates; may point into a user file whose
    /// container holds a [`ContainerPath::PROJECT_KEY`] segment.
    pub project_local: &'static [Location],
    pub format: Format,
    /// Server container in user and project files.
    pub container: ContainerPath,
    /// Server container in the project-local file.
    pub project_local_container: ContainerPath,
    pub mapper: &'static dyn EntryMapper,
    pub caps: Caps,
}

/// A path of keys from the document root to a container or list. A
/// [`ContainerPath::PROJECT_KEY`] segment stands for the key of the project
/// in a map keyed by project path (see [`project_key`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerPath(pub &'static [&'static str]);

impl ContainerPath {
    pub const PROJECT_KEY: &'static str = "{project_key}";

    pub const fn segments(self) -> &'static [&'static str] {
        self.0
    }

    /// The keys of this path in `doc`. A project key segment is resolved
    /// against the keys already in the map that holds it, so an existing
    /// key for `project` is reused as written.
    pub fn keys(
        self,
        os: Os,
        doc: &Document,
        project: Option<&Path>,
    ) -> Result<Vec<String>, CoreError> {
        let mut keys: Vec<String> = Vec::with_capacity(self.0.len());
        for segment in self.0 {
            if *segment != Self::PROJECT_KEY {
                keys.push((*segment).to_owned());
                continue;
            }
            let project = project.ok_or_else(|| {
                CoreError::new(
                    ErrorCode::InvalidPath,
                    "this container is keyed by project and needs a project folder",
                )
            })?;
            let map: Vec<&str> = keys.iter().map(String::as_str).collect();
            let existing = doc.list(&map)?;
            keys.push(project_key(os, project, &existing));
        }
        Ok(keys)
    }
}

/// One native file to write for a client in a scope.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub client: &'static str,
    pub scope: Scope,
    pub os: Os,
    pub file: PathBuf,
    pub format: Format,
    pub container: ContainerPath,
    /// The project folder, for project scopes.
    pub project: Option<PathBuf>,
    /// Warnings attached to this target (for example `msix_dual_write`).
    pub warnings: Vec<CoreError>,
}

impl Target {
    /// The container keys in `doc`, the parsed text of [`Target::file`].
    pub fn container_keys(&self, doc: &Document) -> Result<Vec<String>, CoreError> {
        self.container.keys(self.os, doc, self.project.as_deref())
    }
}

impl AdapterSpec {
    /// The files this client's MCP servers go to in `scope`. Only checks
    /// which files exist; never reads or writes them.
    ///
    /// Errors: `not_supported_on_os` when the client has no MCP target on
    /// this OS, `scope_not_supported` when it has none in `scope`,
    /// `invalid_path` for a project scope without an absolute `project`.
    pub fn mcp_targets(
        &self,
        env: &Env,
        fs: &dyn Fs,
        scope: Scope,
        project: Option<&Path>,
    ) -> Result<Vec<Target>, CoreError> {
        let mcp = self.mcp_on(env.os())?;
        let (candidates, container) = match scope {
            Scope::User => (mcp.user, mcp.container),
            Scope::Project => (mcp.project, mcp.container),
            Scope::ProjectLocal => (mcp.project_local, mcp.project_local_container),
        };
        let project = match scope {
            Scope::User => None,
            Scope::Project | Scope::ProjectLocal => Some(project_folder(project)?),
        };
        if candidates.is_empty() {
            return Err(CoreError::new(
                ErrorCode::ScopeNotSupported,
                format!(
                    "{} has no {} MCP config",
                    self.display_name,
                    scope_name(scope)
                ),
            ));
        }
        let target = |file: PathBuf, warnings: Vec<CoreError>| Target {
            client: self.id,
            scope,
            os: env.os(),
            file,
            format: mcp.format,
            container,
            project: project.map(Path::to_path_buf),
            warnings,
        };
        let file = first_existing(self, candidates, env, fs, project)?;
        let mut targets = vec![target(file.clone(), Vec::new())];
        if scope == Scope::User {
            for mirror in mcp.user_mirrors {
                for copy in mirror.expand(env, fs, None) {
                    let warning = CoreError::new(
                        ErrorCode::MsixDualWrite,
                        format!(
                            "{} keeps a packaged (MSIX) copy of its config; both {} and this copy are written",
                            self.display_name,
                            file.display()
                        ),
                    )
                    .with_path(&copy);
                    targets.push(target(copy, vec![warning]));
                }
            }
        }
        Ok(targets)
    }

    /// The list a project-local file uses to hide same-named user servers
    /// in `project` (Claude Code `projects["<P>"].disabledMcpServers`), as a
    /// target whose container is the list. `None` when the client has no
    /// such mechanism.
    pub fn suppression_target(
        &self,
        env: &Env,
        fs: &dyn Fs,
        project: &Path,
    ) -> Result<Option<Target>, CoreError> {
        let mcp = self.mcp_on(env.os())?;
        let Some(ProjectDisable::ListMember(list)) = mcp.caps.project_disable else {
            return Ok(None);
        };
        let mut targets = self.mcp_targets(env, fs, Scope::ProjectLocal, Some(project))?;
        let mut target = targets.swap_remove(0);
        target.container = list;
        Ok(Some(target))
    }

    fn mcp_on(&self, os: Os) -> Result<&McpSpec, CoreError> {
        let mcp = self.mcp.as_ref().ok_or_else(|| {
            CoreError::new(
                ErrorCode::ScopeNotSupported,
                format!("{} has no MCP config", self.display_name),
            )
        })?;
        if !mcp.oses.contains(&os) {
            return Err(CoreError::new(
                ErrorCode::NotSupportedOnOs,
                format!("{} has no MCP config on {}", self.display_name, os.as_str()),
            ));
        }
        Ok(mcp)
    }
}

fn project_folder(project: Option<&Path>) -> Result<&Path, CoreError> {
    match project {
        Some(path) if path.is_absolute() => Ok(path),
        Some(path) => Err(CoreError::new(
            ErrorCode::InvalidPath,
            "the project folder must be an absolute path",
        )
        .with_path(path)),
        None => Err(CoreError::new(
            ErrorCode::InvalidPath,
            "a project scope needs a project folder",
        )),
    }
}

/// The first candidate that exists, else the first one that resolves.
fn first_existing(
    spec: &AdapterSpec,
    candidates: &[Location],
    env: &Env,
    fs: &dyn Fs,
    project: Option<&Path>,
) -> Result<PathBuf, CoreError> {
    let resolved: Vec<PathBuf> = candidates
        .iter()
        .filter_map(|location| location.resolve(env, project))
        .collect();
    resolved
        .iter()
        .find(|path| fs.exists(path))
        .or_else(|| resolved.first())
        .cloned()
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::NotSupportedOnOs,
                format!(
                    "{} has no MCP config path on {}",
                    spec.display_name,
                    env.os().as_str()
                ),
            )
        })
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::User => "user",
        Scope::Project => "project",
        Scope::ProjectLocal => "project-local",
    }
}

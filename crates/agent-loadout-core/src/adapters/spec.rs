//! The declarative adapter table entry (SPEC 7.1 `AdapterSpec`, `McpSpec`)
//! and the target files it resolves to.

use std::path::{Path, PathBuf};

use crate::adapters::caps::{Caps, ProjectDisable};
use crate::adapters::detect::{McpExtension, Probe};
use crate::adapters::location::{Location, project_key};
use crate::adapters::mapper::{EntryMapper, MapError, RenderedServer, WriteCx};
use crate::env::{Env, Os};
use crate::error::{CoreError, ErrorCode};
use crate::formats::{Document, Format, NativeMap, NativeValue};
use crate::fs::Fs;
use crate::model::applied::Scope;
use crate::model::clients::ClientPrefs;

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
    /// created. With [`McpSpec::profiles`], this is the default profile.
    pub user: &'static [Location],
    /// Extra copies of the user file, each written too when it exists (the
    /// Claude Desktop MSIX copy). Usually [`Location::Glob`].
    pub user_mirrors: &'static [Location],
    /// Named profiles, each with its own user file (omp), selected by
    /// `profiles` in `clients.toml`.
    pub profiles: Option<Profiles>,
    /// Shared project candidates, relative to `<P>`.
    pub project: &'static [Location],
    /// Private project candidates; may point into a user file whose
    /// container holds a [`ContainerPath::PROJECT_KEY`] segment.
    pub project_local: &'static [Location],
    pub format: Format,
    /// The text of a new file (omp's `$schema` line); empty for most
    /// clients.
    pub skeleton: &'static str,
    /// Server container in user and project files.
    pub container: ContainerPath,
    /// Server container in the project-local file.
    pub project_local_container: ContainerPath,
    /// Containers in the same files that the client still loads but Loadout
    /// only reads (import, effective view) and never edits (Codex legacy
    /// `[mcp.servers]`).
    pub legacy_containers: &'static [ContainerPath],
    /// Whether the client treats two entry names as the same server (Pi:
    /// `-` and `_` are equal). Most clients compare exactly
    /// ([`exact_names`]).
    pub same_name: fn(&str, &str) -> bool,
    /// Extensions that replace the client's MCP support when installed.
    pub replaced_by: &'static [McpExtension],
    pub mapper: &'static dyn EntryMapper,
    pub caps: Caps,
}

/// Named profiles of a client (omp `~/.omp/profiles/<name>/agent/mcp.json`).
#[derive(Debug, Clone, Copy)]
pub struct Profiles {
    /// The name that selects [`McpSpec::user`] (`default`). It is also the
    /// selection when `clients.toml` has no `profiles` key.
    pub default: &'static str,
    /// The directory holding one directory per named profile.
    pub dir: Location,
    /// The user file inside a profile's directory, `/`-separated.
    pub rel: &'static str,
    /// Whether a profile name is valid for the client.
    pub valid_name: fn(&str) -> bool,
}

/// [`McpSpec::same_name`] for clients that compare names exactly.
pub fn exact_names(a: &str, b: &str) -> bool {
    a == b
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
    /// The named profile this user file belongs to (omp); `None` for the
    /// default profile and for clients without profiles.
    pub profile: Option<String>,
    /// The text of the file when it does not exist yet.
    pub skeleton: &'static str,
    /// Warnings attached to this target (for example `msix_dual_write`).
    pub warnings: Vec<CoreError>,
}

impl Target {
    /// The container keys in `doc`, the parsed text of [`Target::file`].
    pub fn container_keys(&self, doc: &Document) -> Result<Vec<String>, CoreError> {
        self.container.keys(self.os, doc, self.project.as_deref())
    }

    /// The document to edit when [`Target::file`] does not exist: the
    /// client's skeleton (empty for most clients).
    pub fn new_document(&self) -> Document {
        Document::parse(self.skeleton.as_bytes(), self.format)
            .expect("an adapter skeleton parses in its format")
    }
}

impl McpSpec {
    /// The project entry that hides a same-named user server `server` when
    /// the client suppresses through `enabled: false`
    /// ([`ProjectDisable::EnabledFalse`]): `{ "enabled": false }` alone for
    /// a partial override, else `server` itself with `enabled: false`.
    /// `None` when the client suppresses some other way, or not at all.
    pub fn suppression_entry(
        &self,
        server: &RenderedServer,
        os: Os,
    ) -> Result<Option<NativeValue>, MapError> {
        match self.caps.project_disable {
            Some(ProjectDisable::EnabledFalse { partial: true }) => {
                let mut entry = NativeMap::new();
                entry.insert("enabled", NativeValue::Bool(false));
                Ok(Some(NativeValue::Object(entry)))
            }
            Some(ProjectDisable::EnabledFalse { partial: false }) => {
                let disabled = RenderedServer {
                    enabled: false,
                    ..server.clone()
                };
                self.mapper
                    .to_native(&disabled, &WriteCx { os, existing: None })
                    .map(Some)
            }
            Some(ProjectDisable::ListMember(_)) | None => Ok(None),
        }
    }
}

impl AdapterSpec {
    /// The files this client's MCP servers go to in `scope`. Only checks
    /// which files exist; never reads or writes them. `prefs` is the
    /// client's table in `clients.toml`; it selects the profiles of a
    /// client with [`McpSpec::profiles`] (an empty list selects none) and is
    /// ignored by the others.
    ///
    /// Errors: `not_supported_on_os` when the client has no MCP target on
    /// this OS, `scope_not_supported` when it has none in `scope`,
    /// `invalid_path` for a project scope without an absolute `project`,
    /// `invalid_value` for a profile name the client rejects.
    pub fn mcp_targets(
        &self,
        env: &Env,
        fs: &dyn Fs,
        scope: Scope,
        project: Option<&Path>,
        prefs: &ClientPrefs,
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
        let target = |file: PathBuf, profile: Option<String>, warnings: Vec<CoreError>| Target {
            client: self.id,
            scope,
            os: env.os(),
            file,
            format: mcp.format,
            container,
            project: project.map(Path::to_path_buf),
            profile,
            skeleton: mcp.skeleton,
            warnings,
        };
        let files = match (scope, &mcp.profiles) {
            (Scope::User, Some(profiles)) => self.profile_files(profiles, env, fs, prefs)?,
            _ => vec![(first_existing(self, candidates, env, fs, project)?, None)],
        };
        let mut targets = Vec::with_capacity(files.len());
        for (file, profile) in files {
            targets.push(target(file, profile, Vec::new()));
        }
        if scope == Scope::User
            && let Some(file) = targets.first().map(|target| target.file.clone())
        {
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
                    targets.push(target(copy, None, vec![warning]));
                }
            }
        }
        Ok(targets)
    }

    /// Where a project hides a same-named user server, as a target:
    ///
    /// - [`ProjectDisable::ListMember`]: the project-local file, with the
    ///   list as its container (Claude Code
    ///   `projects["<P>"].disabledMcpServers`), for
    ///   [`Document::set_list_member`].
    /// - [`ProjectDisable::EnabledFalse`]: the project file and its server
    ///   container, for an upsert of [`McpSpec::suppression_entry`].
    ///
    /// `None` when the client cannot suppress a user server in a project.
    pub fn suppression_target(
        &self,
        env: &Env,
        fs: &dyn Fs,
        project: &Path,
        prefs: &ClientPrefs,
    ) -> Result<Option<Target>, CoreError> {
        let mcp = self.mcp_on(env.os())?;
        let (scope, list) = match mcp.caps.project_disable {
            Some(ProjectDisable::ListMember(list)) => (Scope::ProjectLocal, Some(list)),
            Some(ProjectDisable::EnabledFalse { .. }) => (Scope::Project, None),
            None => return Ok(None),
        };
        let mut targets = self.mcp_targets(env, fs, scope, Some(project), prefs)?;
        let mut target = targets.swap_remove(0);
        if let Some(list) = list {
            target.container = list;
        }
        Ok(Some(target))
    }

    /// `collision` for each key in `keys` that the client treats as the
    /// same server as `name` without being equal to it (Pi `my-tool` and
    /// `my_tool`). `keys` are the names already in the container, or other
    /// names desired for the same target.
    pub fn name_collisions(&self, name: &str, keys: &[String]) -> Vec<CoreError> {
        let Some(mcp) = &self.mcp else {
            return Vec::new();
        };
        keys.iter()
            .filter(|key| key.as_str() != name && (mcp.same_name)(key, name))
            .map(|key| {
                CoreError::new(
                    ErrorCode::Collision,
                    format!(
                        "{} treats {key:?} and {name:?} as the same server; set `native_name` or remove one",
                        self.display_name
                    ),
                )
            })
            .collect()
    }

    /// The user files of the profiles `prefs` selects, in order, without
    /// duplicates.
    fn profile_files(
        &self,
        profiles: &Profiles,
        env: &Env,
        fs: &dyn Fs,
        prefs: &ClientPrefs,
    ) -> Result<Vec<(PathBuf, Option<String>)>, CoreError> {
        let mcp = self.mcp_on(env.os())?;
        let default = [profiles.default.to_owned()];
        let names = prefs.profiles.as_deref().unwrap_or(&default);
        let mut files: Vec<(PathBuf, Option<String>)> = Vec::with_capacity(names.len());
        for name in names {
            let entry = if name == profiles.default {
                (first_existing(self, mcp.user, env, fs, None)?, None)
            } else {
                if !(profiles.valid_name)(name) {
                    return Err(CoreError::new(
                        ErrorCode::InvalidValue,
                        format!("{name:?} is not a valid {} profile name", self.display_name),
                    ));
                }
                let dir = profiles.dir.resolve(env, None).ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::NotSupportedOnOs,
                        format!(
                            "{} has no profile directory on {}",
                            self.display_name,
                            env.os().as_str()
                        ),
                    )
                })?;
                let mut file = dir.join(name);
                for part in profiles.rel.split('/').filter(|part| !part.is_empty()) {
                    file.push(part);
                }
                (file, Some(name.clone()))
            };
            if !files.iter().any(|(file, _)| *file == entry.0) {
                files.push(entry);
            }
        }
        Ok(files)
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

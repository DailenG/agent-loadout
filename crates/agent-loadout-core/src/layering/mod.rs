//! Project scope (SPEC 6): the project layer of a folder over the system
//! layer of the store.
//!
//! - [`manifest`]: discovery of `<P>`, loading and validation of
//!   `.loadout/project.toml` and `.loadout/project.local.toml`, the content
//!   hash.
//! - [`merge`]: local over shared, `from`, `unset`, suppressions, client
//!   sets and the layer each entry comes from.
//! - [`trust`]: `state/trust.json` (SPEC 17).
//! - [`git`]: the project's repository, for the private-file rules of 6.5.
//! - [`effective`]: what a client loads in a folder, and from where (6.4).
//! - [`projects`]: init, add, remove, the trust review and trust, shared by
//!   `loadout project` and the app.
//!
//! The planner ([`crate::apply::plan`]) turns known projects into project
//! and project-local targets.

pub mod effective;
pub mod git;
pub mod manifest;
pub mod merge;
pub mod projects;
pub mod trust;

pub use effective::{
    EffectiveEntry, EffectiveView, Layer, ProjectRef, Shadowed, Winner, effective,
};

use std::path::{Path, PathBuf};

pub use git::ProjectRepo;
pub use manifest::{
    LOCAL_FILE, ManifestFile, Project, SHARED_FILE, content_hash, discover, path_key, same_path,
};
pub use merge::{
    EntryBody, EntryLayer, ProjectEntry, deep_merge, default_clients, project_entries, remove_path,
    suppression_mechanism,
};
pub use trust::{TRUST_FILE, TrustRecord, TrustState, TrustStore};

use crate::env::Env;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::machine::MachineOverlay;

/// A project this machine knows (overlay `[projects.<id>] path`).
#[derive(Debug, Clone)]
pub struct KnownProject {
    /// The overlay key.
    pub id: String,
    pub root: PathBuf,
    pub status: ProjectStatus,
}

/// What a known project's folder holds now.
#[derive(Debug, Clone)]
pub enum ProjectStatus {
    /// The folder does not exist on this machine.
    Missing,
    /// The manifests cannot be used (absent, unreadable, invalid); the
    /// project's entries are left alone.
    Failed(CoreError),
    Loaded {
        project: Box<Project>,
        trust: TrustState,
    },
}

impl KnownProject {
    /// The loaded project, if any.
    pub fn project(&self) -> Option<&Project> {
        match &self.status {
            ProjectStatus::Loaded { project, .. } => Some(project),
            _ => None,
        }
    }

    /// `trust_required` for a project that is not trusted as it is now.
    pub fn trust_error(&self) -> Option<CoreError> {
        let ProjectStatus::Loaded { project, trust } = &self.status else {
            return None;
        };
        trust_required(project, &self.root, *trust)
    }
}

/// `trust_required` for `project` in `root` when `trust` is not
/// [`TrustState::Trusted`].
pub fn trust_required(project: &Project, root: &Path, trust: TrustState) -> Option<CoreError> {
    let why = match trust {
        TrustState::Trusted => return None,
        TrustState::Untrusted => "is not trusted on this machine",
        TrustState::Changed => "changed since it was trusted",
    };
    Some(
        CoreError::new(
            ErrorCode::TrustRequired,
            format!(
                "project {:?} {why}; review its manifests, then run `{} project trust {}`",
                project.name(),
                crate::brand::BIN_NAME,
                root.display()
            ),
        )
        .with_path(manifest::manifest_dir(root)),
    )
}

/// The projects of this machine's overlay, in id order, with their
/// manifests and trust state.
pub fn known_projects(
    fs: &dyn Fs,
    env: &Env,
    overlay: Option<&MachineOverlay>,
    trust: &TrustStore,
) -> Vec<KnownProject> {
    let Some(projects) = overlay.and_then(|overlay| overlay.projects.as_ref()) else {
        return Vec::new();
    };
    projects
        .iter()
        .map(|(id, known)| {
            let root = PathBuf::from(&known.path);
            let status = load_status(fs, env, id, &root, trust);
            KnownProject {
                id: id.clone(),
                root,
                status,
            }
        })
        .collect()
}

fn load_status(fs: &dyn Fs, env: &Env, id: &str, root: &Path, trust: &TrustStore) -> ProjectStatus {
    if !fs.is_dir(root) {
        return ProjectStatus::Missing;
    }
    // A manifest directly in the home directory is ignored (SPEC 6.1):
    // there, project files are the user files of several clients.
    if same_path(root, env.home()) {
        return ProjectStatus::Failed(
            CoreError::new(
                ErrorCode::InvalidPath,
                format!(
                    "project {id:?} is the home directory, which cannot be a project; its \
                     entries are left alone"
                ),
            )
            .with_path(root),
        );
    }
    match Project::load(fs, root) {
        Ok(Some(project)) if project.id != id => ProjectStatus::Failed(
            CoreError::new(
                ErrorCode::IdMismatch,
                format!(
                    "the manifests in this folder are project {:?}, but this machine knows the \
                     folder as project {id:?}",
                    project.id
                ),
            )
            .with_path(manifest::manifest_dir(root)),
        ),
        Ok(Some(project)) => {
            let state = trust.state(&project);
            ProjectStatus::Loaded {
                project: Box::new(project),
                trust: state,
            }
        }
        Ok(None) => ProjectStatus::Failed(
            CoreError::new(
                ErrorCode::NotFound,
                format!("project {id:?} has no manifest in its folder; its entries are left alone"),
            )
            .with_path(manifest::manifest_dir(root)),
        ),
        Err(err) => ProjectStatus::Failed(err),
    }
}

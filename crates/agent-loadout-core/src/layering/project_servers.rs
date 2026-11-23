//! Server entries of a project manifest (SPEC 5.5, 13.2 project scope):
//! add, edit and remove `[servers.<name>]` and its `clients`, in
//! `project.toml` (shared) or `project.local.toml` (private).
//!
//! The folder is the source of truth, so an edit writes the manifest file
//! first. Loadout trusts what it writes itself (M9 decision 2): the edited
//! content is mirrored into the store, trusted and applied in one commit
//! (`Loadout-Scope: project/<id>`). An edit starts only from trusted content
//! the user was shown: the manifests must still have the content hash the
//! caller saw, and that content must be trusted on this machine; otherwise
//! `trust_required` (review and trust first, as `loadout project trust`).
//! The write is a compare-and-swap against the checked bytes, the other
//! manifest must still be as checked, and what is trusted is exactly that
//! checked content plus the new text (never a reload). When the commit
//! fails, what this call wrote is put back (the manifest, and a
//! `.gitignore` it created or appended to).

use std::cell::Cell;
use std::path::Path;

use crate::apply::exec::swap_file;
use crate::apply::plan::read_target;
use crate::apply::{ApplyEnv, ApplyReport};
use crate::error::{CoreError, ErrorCode};
use crate::layering::manifest::manifest_dir;
use crate::layering::projects::{
    GITIGNORE, commit_project, ensure_line, load_project, mirror, save_trust, write_file,
};
use crate::layering::{
    EntryLayer, LOCAL_FILE, ManifestFile, SHARED_FILE, TrustStore, trust_required,
};
use crate::model::TomlDocument;
use crate::model::clients::ClientSelection;
use crate::model::ids::{self, IdKind};
use crate::model::project::{ManifestLayer, ProjectManifest, ProjectServer};
use crate::store::{CommitId, Store};

/// What to do with `[servers.<name>]` of one manifest file.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerEdit {
    /// Writes the table as given. With `create`, a name the file already
    /// holds is `already_exists`.
    Set {
        server: Box<ProjectServer>,
        create: bool,
    },
    /// Removes the table; `not_found` when the file does not hold it.
    Remove,
    /// Sets the table's `clients` (`None`: the project's defaults).
    Clients(Option<ClientSelection>),
}

impl ServerEdit {
    fn verb(&self) -> &'static str {
        match self {
            Self::Set { create: true, .. } => "add",
            Self::Set { create: false, .. } => "update",
            Self::Remove => "remove",
            Self::Clients(_) => "clients",
        }
    }
}

fn not_in_file(name: &str, file: &str) -> CoreError {
    CoreError::new(
        ErrorCode::NotFound,
        format!("{file} has no server {name:?}"),
    )
}

/// Applies `edit` to server `name` in the `layer` manifest of the project
/// in `root`, then mirrors, trusts and applies the result in one commit.
/// `expected` is the manifests' content hash as the user saw it. A file
/// that would not change makes no commit (`(None, None)`).
///
/// Writing to the shared layer needs an existing `project.toml` (sharing a
/// private project is `loadout project share`). Writing to the local layer
/// of a shared project creates `project.local.toml` when needed, and keeps
/// it out of the project's repository through `.loadout/.gitignore`.
pub fn edit_server(
    store: &Store<'_>,
    ax: &ApplyEnv<'_>,
    root: &Path,
    expected: &str,
    layer: EntryLayer,
    name: &str,
    edit: ServerEdit,
) -> Result<(Option<ApplyReport>, Option<CommitId>), CoreError> {
    ids::check(IdKind::Server, name)?;
    let fs = ax.fs;
    let project = load_project(fs, root)?;
    let trust = TrustStore::load(fs, &ax.home.state_dir())?;
    if let Some(err) = trust_required(&project, root, trust.state(&project)) {
        return Err(err);
    }
    if project.hash() != expected {
        return Err(CoreError::new(
            ErrorCode::TrustRequired,
            format!(
                "the manifests of project {:?} changed since they were shown; reload and try \
                 again",
                project.name()
            ),
        )
        .with_path(manifest_dir(root)));
    }
    let (file, manifest_layer) = match layer {
        EntryLayer::Shared => {
            let Some(shared) = project.shared.as_ref() else {
                return Err(CoreError::new(
                    ErrorCode::InvalidValue,
                    format!(
                        "project {:?} has no shared project.toml; share it first with `{} \
                         project share`",
                        project.id,
                        crate::brand::BIN_NAME
                    ),
                )
                .with_path(manifest_dir(root)));
            };
            (Some(shared), ManifestLayer::Shared)
        }
        EntryLayer::Local => (
            project.local.as_ref(),
            ManifestLayer::Local {
                shared_id: project
                    .shared
                    .as_ref()
                    .map(|shared| shared.manifest.id.as_deref().unwrap_or("")),
            },
        ),
    };
    let path = match file {
        Some(file) => file.path.clone(),
        None => manifest_dir(root).join(LOCAL_FILE),
    };
    let file_name = if layer == EntryLayer::Shared {
        crate::layering::SHARED_FILE
    } else {
        LOCAL_FILE
    };
    let before = file.map(|file| file.bytes.clone());
    let mut doc = match &before {
        Some(bytes) => {
            let text = std::str::from_utf8(bytes).map_err(|_| {
                CoreError::new(ErrorCode::ParseError, "the manifest is not valid UTF-8")
                    .with_path(&path)
            })?;
            TomlDocument::<ProjectManifest>::parse(text)
                .map_err(|diagnostic| diagnostic.to_core_error(Some(&path)))?
        }
        // A local file next to the shared one: the id comes from there.
        None => TomlDocument::new(ProjectManifest::new(None)),
    };
    let verb = edit.verb();
    {
        let manifest = doc.value_mut();
        let servers = manifest.servers.get_or_insert_with(Default::default);
        match edit {
            ServerEdit::Set { server, create } => {
                if create && servers.contains_key(name) {
                    return Err(CoreError::new(
                        ErrorCode::AlreadyExists,
                        format!("{file_name} already has a server {name:?}"),
                    )
                    .with_path(&path));
                }
                servers.insert(name.to_owned(), *server);
            }
            ServerEdit::Remove => {
                if servers.remove(name).is_none() {
                    return Err(not_in_file(name, file_name).with_path(&path));
                }
            }
            ServerEdit::Clients(selection) => {
                let Some(server) = servers.get_mut(name) else {
                    return Err(not_in_file(name, file_name).with_path(&path));
                };
                server.clients = selection;
            }
        }
        if servers.is_empty() {
            manifest.servers = None;
        }
    }
    if let Some(err) = doc
        .value()
        .validate(manifest_layer, None)
        .first_error(Some(&path))
    {
        return Err(err);
    }
    let text = doc.to_text()?;
    if before.as_deref() == Some(text.as_bytes()) {
        return Ok((None, None));
    }

    // What is trusted, mirrored and applied: the other file exactly as it
    // was checked, and this file's new text. Never a reload, which could
    // hold a change made by someone else meanwhile.
    let mut edited = project.clone();
    let written = ManifestFile {
        path: path.clone(),
        bytes: text.clone().into_bytes(),
        manifest: TomlDocument::<ProjectManifest>::parse(&text)
            .map(TomlDocument::into_value)
            .map_err(|diagnostic| diagnostic.to_core_error(Some(&path)))?,
    };
    let (other_path, other_bytes) = match layer {
        EntryLayer::Shared => {
            edited.shared = Some(written);
            (
                manifest_dir(root).join(LOCAL_FILE),
                project.local.as_ref().map(|file| file.bytes.as_slice()),
            )
        }
        EntryLayer::Local => {
            edited.local = Some(written);
            (
                manifest_dir(root).join(SHARED_FILE),
                project.shared.as_ref().map(|file| file.bytes.as_slice()),
            )
        }
    };
    let changed = || {
        CoreError::new(
            ErrorCode::TrustRequired,
            format!(
                "the manifests of project {:?} changed while being edited; reload and try again",
                project.name()
            ),
        )
        .with_path(manifest_dir(root))
    };

    let ignore = manifest_dir(root).join(GITIGNORE);
    let ignore_before = read_target(fs, &ignore)?;
    let swapped = Cell::new(false);
    let committed = (|| {
        if before.is_none() {
            // The shared project's repository must never see the private
            // file (SPEC 5.5).
            ensure_line(fs, &ignore, LOCAL_FILE)?;
        }
        // Compare and swap: this file must still hold the bytes checked
        // above (the swap compares them), and so must the other file,
        // before and after the swap.
        if read_target(fs, &other_path)?.as_deref() != other_bytes {
            return Err(changed());
        }
        if !swap_file(fs, &path, before.as_deref(), text.as_bytes())? {
            return Err(changed());
        }
        swapped.set(true);
        if load_project(fs, root)?.hash() != edited.hash() {
            return Err(changed());
        }
        let summary = format!("project {}: {verb} {name}", project.id);
        commit_project(
            store,
            ax,
            &format!("project.server.{verb}"),
            summary,
            &project.id,
            true,
            |tx| {
                mirror(tx, &edited)?;
                save_trust(ax, &edited)
            },
        )
    })();
    committed.inspect_err(|_| {
        // Nothing was committed: put back what this call wrote, and only
        // where the file still holds it.
        if swapped.get() {
            let _ = match &before {
                Some(bytes) => swap_file(fs, &path, Some(text.as_bytes()), bytes).map(drop),
                None if read_target(fs, &path).ok().flatten().as_deref()
                    == Some(text.as_bytes()) =>
                {
                    fs.remove_file(&path)
                        .map_err(|err| CoreError::new(ErrorCode::IoError, err.to_string()))
                }
                None => Ok(()),
            };
        }
        if before.is_none() && read_target(fs, &ignore).ok().flatten() != ignore_before {
            let _ = match &ignore_before {
                Some(bytes) => write_file(fs, &ignore, bytes),
                None => fs
                    .remove_file(&ignore)
                    .map_err(|err| CoreError::new(ErrorCode::IoError, err.to_string())),
            };
        }
    })
}

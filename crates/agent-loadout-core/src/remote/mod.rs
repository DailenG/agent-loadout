//! Remotes (SPEC 9.6 to 9.9): optional backup and sync targets for the
//! store. None exists by default and none is privileged.
//!
//! - [`config`]: `state/remotes.toml`, machine-local.
//! - [`folder`]: folder remotes, one subfolder per machine
//!   (`repo-per-machine`, `mirror`, `archive`); pull merges every sibling
//!   machine's head through [`crate::store::merge`].
//! - [`git`]: git remotes through the system `git` CLI; [`credential`] is
//!   the `auth = token` helper; [`gh`] the optional GitHub CLI
//!   conveniences.
//! - [`manifest`]: `machine.json` and `EXPORT.json`.
//! - [`sync`]: push, pull and status of every remote alike, recorded in
//!   `state/remote-status.toml`; [`restore`] is `restore --from`.
//! - [`sync_artifacts`]: sync tool conflict and temp names, reported and
//!   left alone.
//! - [`unc`]: mapped drive letters resolved to UNC paths on Windows.

pub mod config;
pub mod credential;
pub mod folder;
pub mod gh;
pub mod git;
pub mod manifest;
pub mod restore;
pub mod sync;
pub mod sync_artifacts;
pub mod unc;

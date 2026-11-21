//! Remotes (SPEC 9.6 to 9.9): optional backup and sync targets for the
//! store. None exists by default and none is privileged.
//!
//! - [`config`]: `state/remotes.toml`, machine-local.
//! - [`folder`]: folder remotes, one subfolder per machine
//!   (`repo-per-machine`, `mirror`, `archive`); pull merges every sibling
//!   machine's head through [`crate::store::merge`].
//! - [`manifest`]: `machine.json` and `EXPORT.json`.
//! - [`sync_artifacts`]: sync tool conflict and temp names, reported and
//!   left alone.
//! - [`unc`]: mapped drive letters resolved to UNC paths on Windows.
//!
//! Git remotes arrive in M13.

pub mod config;
pub mod folder;
pub mod manifest;
pub mod sync_artifacts;
pub mod unc;

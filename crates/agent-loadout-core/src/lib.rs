//! Core library of Loadout.
//!
//! This crate never reads process-global state (`std::env`, home directory
//! lookups, the system clock) outside [`env`]. Callers inject an [`Env`] and an
//! [`Fs`]; the CLI and the app build them from the process, tests build them
//! from a temp root.

pub mod access;
pub mod adapters;
pub mod agent;
pub mod apply;
pub mod brand;
pub mod catalog;
pub mod env;
pub mod error;
pub mod formats;
pub mod fs;
pub mod layering;
pub mod local_settings;
pub mod mcp_source;
pub mod migrate;
pub mod model;
pub mod paths;
pub mod remote;
pub mod scan;
pub mod secrets;
pub mod skills;
pub mod sources;
pub mod store;

pub use env::{Clock, Env, EnvBuilder, FixedClock, KnownFolder, Os, SystemClock};
pub use error::{CoreError, ErrorCode};
pub use fs::{Fs, RealFs};

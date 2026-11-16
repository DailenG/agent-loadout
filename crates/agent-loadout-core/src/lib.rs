//! Core library of Loadout.
//!
//! This crate never reads process-global state (`std::env`, home directory
//! lookups, the system clock) outside [`env`]. Callers inject an [`Env`] and an
//! [`Fs`]; the CLI and the app build them from the process, tests build them
//! from a temp root.

pub mod adapters;
pub mod brand;
pub mod env;
pub mod error;
pub mod formats;
pub mod fs;
pub mod model;
pub mod paths;
pub mod store;

pub use env::{Clock, Env, EnvBuilder, FixedClock, KnownFolder, Os, SystemClock};
pub use error::{CoreError, ErrorCode};
pub use fs::{Fs, RealFs};

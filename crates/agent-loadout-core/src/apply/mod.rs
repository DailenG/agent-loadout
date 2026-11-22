//! Apply engine (SPEC 8): reconcile client files with the store's desired
//! state through a plan, own exactly what was written (the ledger), commit
//! every run to the store, and roll a run back.
//!
//! - [`render`]: the system layer, rendered per client.
//! - [`plan`]: the SPEC 8.3 planning table, per file.
//! - [`exec`]: SPEC 8.4 execution (re-plan, backup, self check, atomic
//!   compare-and-swap write, ledger, journal) and the apply commit (8.7).
//! - [`ledger`], [`backup`], [`journal`]: machine-local records.
//! - [`import`]: native entries into the store (8.6).
//! - [`rollback`]: one-click rollback of an apply run (8.7).
//!
//! Project scopes come from project manifests through [`crate::layering`];
//! the planner turns them into project and project-local targets.

pub mod backup;
pub mod exec;
pub mod import;
pub mod journal;
pub mod ledger;
pub mod plan;
pub mod render;
pub mod rollback;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::adapters::ADAPTERS;
use crate::adapters::detect::RegistryProbe;
use crate::adapters::location::Location;
use crate::env::{Env, KnownFolder, Os};
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::paths::LoadoutHome;
use crate::secrets::delivery::DeliveryEnv;
use crate::secrets::scanner::SecretMemory;

pub use exec::{
    ApplyReport, ApplyRequest, ApplyStatus, TargetResult, apply_in, instant_apply, preview, run,
};
pub use plan::{
    Action, Decision, EntryPlan, EntryState, OnConflict, Plan, PlanOptions, ProjectSelection,
    ScopeFilter, TargetFilter, TargetPlan,
};

/// What an apply runs against.
#[derive(Clone, Copy)]
pub struct ApplyEnv<'a> {
    pub env: &'a Env,
    pub fs: &'a dyn Fs,
    pub home: &'a LoadoutHome,
    /// For `all-detected` enablement; the CLI passes a registry-free probe.
    pub registry: &'a dyn RegistryProbe,
    /// Secret providers and the `loadout` launcher (SPEC 11.4).
    pub delivery: &'a DeliveryEnv,
}

impl<'a> ApplyEnv<'a> {
    /// The render context; values read while rendering go to `memory` (the
    /// store's [`SecretMemory`]).
    pub fn render_cx(&self, memory: Arc<SecretMemory>) -> render::RenderCx<'a> {
        render::RenderCx {
            env: self.env,
            fs: self.fs,
            loadout_home: &self.home.path,
            registry: self.registry,
            delivery: self.delivery,
            memory,
        }
    }
}

/// `.<name>.loadout-<random>.tmp` next to `file` (SPEC 8.4 step 4).
pub(crate) fn temp_path(file: &Path) -> Result<PathBuf, CoreError> {
    let mut random = [0u8; 4];
    getrandom::fill(&mut random).map_err(|err| {
        CoreError::new(
            ErrorCode::IoError,
            format!("cannot read random bytes: {err}"),
        )
    })?;
    let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(file.with_file_name(format!(".{name}.loadout-{suffix}.tmp")))
}

/// Writes a machine-local state file atomically: a temp file in the same
/// directory, then a replace.
pub(crate) fn write_atomic(fs: &dyn Fs, path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    let io = |what: &str, at: &Path, err: std::io::Error| {
        CoreError::new(ErrorCode::IoError, format!("{what}: {err}")).with_path(at)
    };
    if let Some(dir) = path.parent() {
        fs.create_dir_all(dir)
            .map_err(|err| io("cannot create directory", dir, err))?;
    }
    let temp = temp_path(path)?;
    let like = fs.exists(path).then_some(path);
    fs.write_new(&temp, bytes, like)
        .map_err(|err| io("cannot write file", &temp, err))?;
    fs.replace(&temp, path).map_err(|err| {
        let _ = fs.remove_file(&temp);
        io("cannot replace file", path, err)
    })
}

/// `file` with a placeholder root for the store (`machines/*.applied.toml`
/// must not hold personal paths): the longest of the project folder
/// (`<P>/.mcp.json`), the client override variables in force
/// (`$CODEX_HOME/config.toml`), the Windows app data folders
/// (`%APPDATA%/Claude/...`), `$XDG_CONFIG_HOME` and `~`. A file under none
/// of them is `$EXTERNAL/<file name>`.
pub fn placeholder_path(env: &Env, file: &Path, project: Option<&Path>) -> String {
    let windows = env.os() == Os::Windows;
    let spell = |name: &str| {
        if windows {
            format!("%{name}%")
        } else {
            format!("${name}")
        }
    };
    let mut bases: Vec<(String, PathBuf)> = vec![("~".to_owned(), env.home().to_path_buf())];
    if let Some(project) = project {
        bases.push(("<P>".to_owned(), project.to_path_buf()));
    }
    if windows {
        for (name, folder) in [
            ("APPDATA", KnownFolder::RoamingAppData),
            ("LOCALAPPDATA", KnownFolder::LocalAppData),
        ] {
            if let Some(path) = env.known_folder(folder) {
                bases.push((spell(name), path.to_path_buf()));
            }
        }
    }
    let mut vars = vec!["XDG_CONFIG_HOME"];
    for spec in ADAPTERS {
        if let Some(mcp) = &spec.mcp {
            for location in mcp
                .user
                .iter()
                .chain(mcp.project_local)
                .chain(mcp.user_mirrors)
            {
                location_vars(location, &mut vars);
            }
        }
    }
    for var in vars {
        if let Some(path) = env
            .var(var)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
        {
            bases.push((spell(var), path));
        }
    }
    let best = bases
        .iter()
        .filter_map(|(name, base)| file.strip_prefix(base).ok().map(|rest| (name, base, rest)))
        .max_by_key(|(_, base, _)| base.components().count());
    match best {
        Some((name, _, rest)) => {
            let rest: Vec<String> = rest
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            format!("{name}/{}", rest.join("/"))
        }
        None => format!(
            "{}/{}",
            spell("EXTERNAL"),
            file.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        ),
    }
}

fn location_vars(location: &Location, out: &mut Vec<&'static str>) {
    match location {
        Location::EnvDir { var, fallback, .. } => {
            if !out.contains(var) {
                out.push(var);
            }
            location_vars(fallback, out);
        }
        Location::Glob { base, .. } => location_vars(base, out),
        _ => {}
    }
}

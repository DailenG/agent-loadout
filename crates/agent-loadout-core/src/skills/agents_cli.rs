//! Compatibility with the agents `skills` CLI (SPEC 12.4): its folder
//! `~/.agents/skills` and lock file `~/.agents/.skill-lock.json` are
//! scanned read-only when `agents_cli_compat` is on (the default). Update
//! and remove run that CLI only when it is installed on `PATH` (never
//! `npx --yes`). "Vendor into store" copies one of its skills into the
//! store as a store skill.
//!
//! The lock file is read leniently [verify: format of the current `skills`
//! CLI]: `{"skills": {"<name>": {"source", "sourceType", "sourceUrl", ...}}}`;
//! unknown fields are ignored and a lock file that does not parse is a
//! warning.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::env::Os;
use crate::error::{CoreError, ErrorCode};
use crate::model::skill_meta::{SkillSource, SkillSourceKind};
use crate::remote::git::Invocation;
use crate::skills::install::{Installed, Naming, install_files};
use crate::skills::{Entry, SKILL_FILE, SkillsEnv, entry, frontmatter, io_error, read_folder};
use crate::store::{Store, StoreRead};

/// `~/.agents/skills`.
pub fn skills_dir(sx: &SkillsEnv<'_>) -> PathBuf {
    sx.env.home().join(".agents").join("skills")
}

/// `~/.agents/.skill-lock.json`.
pub fn lock_file(sx: &SkillsEnv<'_>) -> PathBuf {
    sx.env.home().join(".agents").join(".skill-lock.json")
}

/// Whether the scan is on (`agents_cli_compat` in `settings.toml`,
/// default on).
pub fn enabled(read: &dyn StoreRead) -> bool {
    crate::skills::store::settings(read)
        .agents_cli_compat
        .unwrap_or(true)
}

/// One skill of the agents CLI.
#[derive(Debug, Clone)]
pub struct AgentsSkill {
    pub name: String,
    pub path: PathBuf,
    /// A link (the CLI's `--symlink` installs), not a folder.
    pub is_link: bool,
    /// Lock file `source` (`owner/repo`).
    pub source: Option<String>,
    /// Lock file `sourceType` (`github`, `local`, ...).
    pub source_type: Option<String>,
    /// Lock file `sourceUrl`.
    pub source_url: Option<String>,
    pub description: Option<String>,
}

/// What [`scan`] found.
#[derive(Debug, Clone, Default)]
pub struct AgentsScan {
    pub skills: Vec<AgentsSkill>,
    /// A lock file that does not parse.
    pub warnings: Vec<CoreError>,
}

/// The agents CLI's skills, sorted by name. Reads only.
pub fn scan(sx: &SkillsEnv<'_>) -> Result<AgentsScan, CoreError> {
    let dir = skills_dir(sx);
    let mut scan = AgentsScan::default();
    if !sx.fs.is_dir(&dir) {
        return Ok(scan);
    }
    let lock = read_lock(sx, &mut scan.warnings);
    for path in sx
        .fs
        .read_dir(&dir)
        .map_err(|err| io_error("cannot list directory", &dir, &err))?
    {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with('.') || !sx.fs.is_dir(&path) {
            continue;
        }
        let is_link = matches!(entry(sx.fs, &path)?, Entry::Link(_));
        let locked = lock
            .as_ref()
            .and_then(|lock| lock.get("skills"))
            .and_then(|skills| skills.get(name));
        let field = |key: &str| {
            locked
                .and_then(|entry| entry.get(key))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let description = sx
            .fs
            .read_to_string(&path.join(SKILL_FILE))
            .ok()
            .and_then(|text| frontmatter::parse(&text).ok().flatten())
            .and_then(|fm| fm.description().map(str::to_owned));
        scan.skills.push(AgentsSkill {
            name: name.to_owned(),
            path: path.clone(),
            is_link,
            source: field("source"),
            source_type: field("sourceType"),
            source_url: field("sourceUrl"),
            description,
        });
    }
    Ok(scan)
}

fn read_lock(sx: &SkillsEnv<'_>, warnings: &mut Vec<CoreError>) -> Option<Value> {
    let path = lock_file(sx);
    if !sx.fs.exists(&path) {
        return None;
    }
    let parsed = sx
        .fs
        .read_to_string(&path)
        .map_err(|err| io_error("cannot read file", &path, &err))
        .and_then(|text| {
            serde_json::from_str::<Value>(&text).map_err(|err| {
                CoreError::new(
                    ErrorCode::ParseError,
                    format!("the agents CLI lock file does not parse: {err}"),
                )
                .with_path(&path)
            })
        });
    match parsed {
        Ok(value) => Some(value),
        Err(err) => {
            warnings.push(err);
            None
        }
    }
}

fn find(sx: &SkillsEnv<'_>, name: &str) -> Result<AgentsSkill, CoreError> {
    check_name(name)?;
    scan(sx)?
        .skills
        .into_iter()
        .find(|skill| skill.name == name)
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::NotFound,
                format!("the agents CLI has no skill {name:?}"),
            )
        })
}

/// Names that are safe as one argument of the `skills` CLI.
fn check_name(name: &str) -> Result<(), CoreError> {
    let valid = !name.is_empty()
        && !name.starts_with(['-', '.'])
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if valid {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorCode::InvalidValue,
            format!("{name:?} is not an agents CLI skill name"),
        ))
    }
}

/// Copies one agents CLI skill into the store (one commit) as a store
/// skill with source kind `agents-cli`. The agents CLI's copy stays.
pub fn vendor(
    store: &Store<'_>,
    sx: &SkillsEnv<'_>,
    name: &str,
    naming: &Naming,
) -> Result<Installed, CoreError> {
    let skill = find(sx, name)?;
    let files = read_folder(sx.fs, &skill.path)?;
    let source = SkillSource {
        kind: SkillSourceKind::AgentsCli,
        url: skill.source_url.clone(),
        path: None,
        git_ref: None,
        commit: None,
        extra: toml::Table::new(),
    };
    install_files(store, sx, naming, files, skill.name, source)
}

/// The `skills` CLI on `PATH` (`skills.exe` or `skills.cmd` on Windows).
/// Relative `PATH` entries are skipped. Like git's lookup, this reads the
/// real file system, where programs live.
pub fn locate_cli(sx: &SkillsEnv<'_>) -> Option<PathBuf> {
    let (separator, names): (char, &[&str]) = match sx.env.os() {
        Os::Windows => (';', &["skills.exe", "skills.cmd"]),
        Os::Linux | Os::MacOs => (':', &["skills"]),
    };
    let path_var = sx.env.var("PATH")?.to_string_lossy().into_owned();
    path_var
        .split(separator)
        .map(|dir| PathBuf::from(dir.trim().trim_matches('"')))
        .filter(|dir| dir.is_absolute())
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

/// What to ask the agents CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliAction {
    Update,
    Remove,
}

/// Runs `skills update <name>` or `skills remove <name> --global --yes`
/// [verify: flags of the current `skills` CLI] with stdin closed;
/// `tool_not_found` when the CLI is not installed. Returns its output.
pub fn run_cli(sx: &SkillsEnv<'_>, action: CliAction, name: &str) -> Result<String, CoreError> {
    check_name(name)?;
    find(sx, name)?;
    let program = locate_cli(sx).ok_or_else(|| {
        CoreError::new(
            ErrorCode::ToolNotFound,
            "the agents `skills` CLI is not installed or not on PATH; Loadout never runs it \
             through npx",
        )
    })?;
    let args: Vec<OsString> = match action {
        CliAction::Update => vec!["update".into(), name.into()],
        CliAction::Remove => vec![
            "remove".into(),
            name.into(),
            "--global".into(),
            "--yes".into(),
        ],
    };
    let invocation = Invocation {
        program,
        args,
        env: sx
            .env
            .vars()
            .map(|(name, value)| (OsString::from(name), value.to_owned()))
            .collect(),
        cwd: sx.env.home().to_path_buf(),
    };
    let output = sx.runner.run(&invocation, &mut |_| {}).map_err(|err| {
        CoreError::new(ErrorCode::ToolNotFound, format!("cannot run skills: {err}"))
    })?;
    if output.success {
        Ok(output.stdout)
    } else {
        let message = output
            .stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("the skills CLI failed")
            .trim()
            .to_owned();
        Err(CoreError::new(ErrorCode::IoError, message).with_path(Path::new(&skills_dir(sx))))
    }
}

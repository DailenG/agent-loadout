// Portions derived from SkillDock (https://github.com/DailenG/skilldock), MIT, Copyright (c) 2026 wanghuan

//! SkillDock's data folder (`~/.skilldock`), read only: its MCP registry,
//! settings, skill tags and skill library, and the mapping of one registry
//! record to a store server (SPEC 15.1).
//!
//! Shapes follow SkillDock's `mcp_manager.rs` (`McpServerRecord`,
//! `McpPersistence`), `state.rs` (`SettingsPersistence`,
//! `SkillTagPersistence`), `models.rs` (`WorkspacePersistence`) and
//! `workspace.rs` (the `config/`, `data/`, `credentials/` layout and the
//! older top-level files). Every field is read leniently: SkillDock wrote
//! some of them only in later versions.
//!
//! Records SkillDock synced with disabled tools were written to clients
//! through a wrapper: `npx -y mcp-filter --include <tool>... -- <command>
//! <args>...` for stdio servers and `npx -y mcp-remote@latest <url>
//! [--transport http-only|sse-only] [--allow-http] [--header N:V]...
//! [--ignore-tool <tool>]...` for remote ones. [`unwrap`] turns both back
//! into the native server plus a tool filter.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::adapters::adapter;
use crate::adapters::caps::{Caps, FieldSet};
use crate::apply::render::native_to_template;
use crate::error::{CoreError, ErrorCode};
use crate::fs::Fs;
use crate::model::clients::{ClientSelection, NATIVE_NAME_KEY};
use crate::model::ids::{self, IdKind};
use crate::model::server::{Enable, Meta, Server, ToolFilter, Transport};
use crate::secrets::classify::{classify_import, find_in_text};
use crate::secrets::extract::{Aliases, Extraction, Extractor};

/// SkillDock's data folder under the home folder.
pub const DIR: &str = ".skilldock";
/// The folder of SkillDock's first releases (`skillm`), read for its MCP
/// registry only.
pub const LEGACY_DIR: &str = ".skillm";
/// `meta.origin` of a migrated server.
pub const ORIGIN: &str = "migrated:skilldock";

/// Where SkillDock kept its MCP registry, newest layout first.
pub const MCP_FILES: [&str; 3] = [
    "~/.skilldock/config/mcp-servers.json",
    "~/.skilldock/mcp-servers.json",
    "~/.skillm/mcp-servers.json",
];
/// Settings, newest layout first.
pub const SETTINGS_FILES: [&str; 2] = [
    "~/.skilldock/config/settings.json",
    "~/.skilldock/settings.json",
];
/// Workspace state (installed skills and, in older versions, tags).
pub const STATE_FILES: [&str; 2] = ["~/.skilldock/data/state.json", "~/.skilldock/state.json"];
/// Skill tags, newest layout first.
pub const TAG_FILES: [&str; 2] = [
    "~/.skilldock/data/skill-tags.json",
    "~/.skilldock/skill-tags.json",
];
/// The skill library.
pub const SKILLS_DIR: &str = "~/.skilldock/skills";
/// SkillDock's GitHub credential.
pub const GITHUB_CREDENTIAL: &str = "~/.skilldock/credentials/github.json";
/// SkillDock's backup repository (a working copy).
pub const BACKUP_REPO: &str = "~/.skilldock/backup/repo";

/// `~/...` display path as an absolute path under `home`.
pub fn resolve(home: &Path, display: &str) -> PathBuf {
    let rest = display.strip_prefix("~/").unwrap_or(display);
    rest.split('/')
        .fold(home.to_path_buf(), |path, part| path.join(part))
}

/// The first of `candidates` that exists, as `(display, path)`.
pub fn first_existing(fs: &dyn Fs, home: &Path, candidates: &[&str]) -> Option<(String, PathBuf)> {
    candidates.iter().find_map(|display| {
        let path = resolve(home, display);
        fs.exists(&path).then(|| ((*display).to_owned(), path))
    })
}

/// Reads a JSON file; `parse_error` names the display path, never the
/// content.
pub fn read_json(fs: &dyn Fs, path: &Path, display: &str) -> Result<Value, CoreError> {
    let text = fs.read_to_string(path).map_err(|err| {
        CoreError::new(ErrorCode::IoError, format!("cannot read {display}: {err}"))
    })?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    serde_json::from_str(text).map_err(|err| {
        CoreError::new(
            ErrorCode::ParseError,
            format!("{display} is not valid JSON (line {})", err.line()),
        )
    })
}

/// One `McpServerRecord`.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// The key SkillDock wrote into client files.
    pub id: String,
    pub name: String,
    /// The server definition as SkillDock kept it (client JSON shape).
    pub server: Value,
    pub description: String,
    pub source_url: String,
    pub enabled_app_ids: Vec<String>,
    pub imported_from_app_ids: Vec<String>,
    pub managed_app_ids: Vec<String>,
    /// `(tool, is_enabled)`.
    pub tools: Vec<(String, bool)>,
    /// `plugin` for records a SkillDock plugin installed.
    pub lifecycle_source: String,
    pub owner_plugin_name: String,
    /// SkillDock app ids that were renamed to Loadout client ids
    /// ([`APP_RENAMES`]), as `(skilldock, loadout)`.
    pub renamed: Vec<(String, String)>,
}

impl Record {
    /// Whether a SkillDock plugin owns the record (plugins are not
    /// migrated).
    pub fn plugin_owned(&self) -> bool {
        self.lifecycle_source == "plugin" || !self.owner_plugin_name.is_empty()
    }
}

fn text(map: &Map<String, Value>, key: &str) -> String {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_owned()
}

fn texts(map: &Map<String, Value>, key: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in map.get(key).and_then(Value::as_array).into_iter().flatten() {
        if let Some(item) = item.as_str().map(str::trim).filter(|s| !s.is_empty())
            && !out.iter().any(|known| known == item)
        {
            out.push(item.to_owned());
        }
    }
    out
}

/// SkillDock app ids that name a Loadout client differently: SkillDock's
/// `github-copilot` writes `~/.copilot/mcp-config.json`, Loadout's
/// `copilot` user file (`mcp_manager.rs` `APP_GITHUB_COPILOT`).
pub const APP_RENAMES: [(&str, &str); 1] = [("github-copilot", "copilot")];

/// An app id list with [`APP_RENAMES`] applied (deduplicated, in order).
fn app_ids(
    map: &Map<String, Value>,
    key: &str,
    renamed: &mut Vec<(String, String)>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in texts(map, key) {
        let id = match APP_RENAMES.iter().find(|(from, _)| *from == id) {
            Some((from, to)) => {
                if !renamed.iter().any(|(known, _)| known == from) {
                    renamed.push(((*from).to_owned(), (*to).to_owned()));
                }
                (*to).to_owned()
            }
            None => id,
        };
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// The records of an MCP registry file (`{ "servers": [...] }`, or a bare
/// array in the oldest files). A record that is not an object or has no
/// id is an error of its own, so the others still migrate.
pub fn parse_records(doc: &Value) -> Result<Vec<Result<Record, CoreError>>, CoreError> {
    let items = match doc {
        Value::Array(items) => items,
        Value::Object(map) => match map.get("servers") {
            Some(Value::Array(items)) => items,
            None => return Ok(Vec::new()),
            Some(_) => {
                return Err(CoreError::new(
                    ErrorCode::UnexpectedShape,
                    "`servers` is not a list",
                ));
            }
        },
        _ => {
            return Err(CoreError::new(
                ErrorCode::UnexpectedShape,
                "the MCP registry is not an object",
            ));
        }
    };
    Ok(items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let map = item.as_object().ok_or_else(|| {
                CoreError::new(
                    ErrorCode::UnexpectedShape,
                    format!("record {} is not an object", index + 1),
                )
            })?;
            let id = text(map, "id");
            let name = text(map, "name");
            let id = if id.is_empty() { name.clone() } else { id };
            if id.is_empty() {
                return Err(CoreError::new(
                    ErrorCode::MissingField,
                    format!("record {} has no id or name", index + 1),
                ));
            }
            let tools = map
                .get("tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|tool| {
                    let tool = tool.as_object()?;
                    let name = text(tool, "name");
                    let enabled = tool
                        .get("isEnabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    (!name.is_empty()).then_some((name, enabled))
                })
                .collect();
            let mut renamed = Vec::new();
            Ok(Record {
                name: if name.is_empty() { id.clone() } else { name },
                id,
                server: map.get("server").cloned().unwrap_or(Value::Null),
                description: text(map, "description"),
                source_url: text(map, "sourceUrl"),
                enabled_app_ids: app_ids(map, "enabledAppIds", &mut renamed),
                imported_from_app_ids: app_ids(map, "importedFromAppIds", &mut renamed),
                managed_app_ids: app_ids(map, "managedAppIds", &mut renamed),
                tools,
                lifecycle_source: text(map, "lifecycleSource"),
                owner_plugin_name: text(map, "ownerPluginName"),
                renamed,
            })
        })
        .collect())
}

/// The last component of a path SkillDock recorded (`/` or `\`
/// separators): the skill folder it names.
fn folder_of(path: &str) -> Option<String> {
    path.trim()
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// Whether a SkillDock skill reference (lower-cased `name`, optional skill
/// `folder`) names the skill in `folder_name` whose names are `names`:
/// by folder when SkillDock recorded a path, else by lower-cased name.
fn names_skill(name: &str, folder: Option<&str>, folder_name: &str, names: &[&str]) -> bool {
    match folder {
        Some(folder) => folder.eq_ignore_ascii_case(folder_name),
        None => names.iter().any(|n| n.trim().to_lowercase() == name),
    }
}

/// One SkillDock skill tag. SkillDock allowed one tag per skill; its keys
/// are `"<lower-cased name>\n<normalized skill path>"` (`state.rs`
/// `skill_tag_key`), older ones the lower-cased name alone, and
/// `installedSkills[].tag` repeats it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagEntry {
    /// The lower-cased skill name.
    pub name: String,
    /// The skill folder from the key's path, if the key has one.
    pub folder: Option<String>,
    pub tag: String,
}

impl TagEntry {
    /// Whether the tag belongs to the skill in `folder_name` named `names`.
    pub fn matches(&self, folder_name: &str, names: &[&str]) -> bool {
        names_skill(&self.name, self.folder.as_deref(), folder_name, names)
    }
}

/// The `skillTags` map of `skill-tags.json` or `state.json`.
pub fn parse_tags(doc: &Value, into: &mut Vec<TagEntry>) {
    let Some(tags) = doc.get("skillTags").and_then(Value::as_object) else {
        return;
    };
    for (key, tag) in tags {
        let Some(tag) = tag.as_str().map(str::trim).filter(|tag| !tag.is_empty()) else {
            continue;
        };
        let (name, folder) = match key.split_once('\n') {
            Some((name, path)) => (name, folder_of(path)),
            None => (key.as_str(), None),
        };
        push_tag(
            into,
            TagEntry {
                name: name.trim().to_lowercase(),
                folder,
                tag: tag.to_owned(),
            },
        );
    }
}

fn push_tag(into: &mut Vec<TagEntry>, entry: TagEntry) {
    if !into.contains(&entry) {
        into.push(entry);
    }
}

/// What `state.json` recorded about an installed skill.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstalledSkill {
    /// The lower-cased skill name.
    pub name: String,
    /// The skill folder (`canonicalPath`, else `localPath`).
    pub folder: Option<String>,
    pub source_type: String,
    pub source_url: String,
    pub branch: String,
    /// `tag`, flattened from SkillDock's `SkillInstanceMetadata`.
    pub tag: String,
}

impl InstalledSkill {
    /// Whether this is the skill in `folder_name` named `names`.
    pub fn matches(&self, folder_name: &str, names: &[&str]) -> bool {
        names_skill(&self.name, self.folder.as_deref(), folder_name, names)
    }
}

/// `installedSkills` of `state.json`; their embedded tags are added to
/// `tags`.
pub fn parse_installed(doc: &Value, tags: &mut Vec<TagEntry>) -> Vec<InstalledSkill> {
    let mut out = Vec::new();
    for item in doc
        .get("installedSkills")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(map) = item.as_object() else {
            continue;
        };
        let name = text(map, "name").to_lowercase();
        if name.is_empty() {
            continue;
        }
        let canonical = text(map, "canonicalPath");
        let path = if canonical.is_empty() {
            text(map, "localPath")
        } else {
            canonical
        };
        let skill = InstalledSkill {
            name,
            folder: folder_of(&path),
            source_type: text(map, "sourceType"),
            source_url: text(map, "sourceUrl"),
            branch: text(map, "branch"),
            tag: text(map, "tag"),
        };
        if !skill.tag.is_empty() {
            push_tag(
                tags,
                TagEntry {
                    name: skill.name.clone(),
                    folder: skill.folder.clone(),
                    tag: skill.tag.clone(),
                },
            );
        }
        out.push(skill);
    }
    out
}

/// A record as a store server.
#[derive(Debug, Clone)]
pub struct Mapped {
    pub server: Server,
    /// The wrapper the definition was unwrapped from (`mcp-filter`,
    /// `mcp-remote`).
    pub unwrapped: Option<&'static str>,
    pub extractions: Vec<Extraction>,
    /// The command is an absolute path on this machine.
    pub machine_path: bool,
    /// Definition keys with no store field (dropped).
    pub dropped: Vec<String>,
    /// Client ids Loadout has no adapter for (kept in the enable list).
    pub unknown_clients: Vec<String>,
    /// Informational notes (renamed client ids).
    pub notes: Vec<String>,
    /// What needs a look (dropped wrapper env keys, a homepage left out).
    pub review: Vec<String>,
}

/// The parts of a SkillDock server definition.
#[derive(Debug, Clone, Default, PartialEq)]
struct Raw {
    transport: Option<Transport>,
    command: Option<String>,
    args: Vec<String>,
    cwd: Option<String>,
    url: Option<String>,
    env: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    /// From an `mcp-filter --include` list.
    allow: Vec<String>,
    /// From `mcp-remote --ignore-tool`.
    deny: Vec<String>,
    /// `env` keys of an `mcp-remote` wrapper its headers and URL did not
    /// use (dropped: a remote server has no environment).
    unused_env: Vec<String>,
}

/// `text` with each `${NAME}` that `env` defines replaced by its value;
/// the names used go to `used`. The wrapper's shell expanded them inside
/// its own process, so they are not variables of this machine.
fn substitute(text: &str, env: &BTreeMap<String, String>, used: &mut BTreeSet<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) if env.contains_key(&after[..end]) => {
                let name = &after[..end];
                out.push_str(&rest[..start]);
                out.push_str(&env[name]);
                used.insert(name.to_owned());
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str(&rest[..start + 2]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_filter(arg: &str) -> bool {
    arg == "mcp-filter" || arg.ends_with("/mcp-filter") || arg.starts_with("mcp-filter@")
}

fn is_remote(arg: &str) -> bool {
    arg == "mcp-remote" || arg.starts_with("mcp-remote@") || arg.ends_with("/mcp-remote")
}

/// The wrapper an entry's `command` and `args` use, if any: SkillDock's
/// `mcp-filter` or `mcp-remote` in the first arguments of `npx`.
pub fn wrapper_kind(command: Option<&str>, args: &[String]) -> Option<&'static str> {
    let command = command?;
    let program = command
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(command)
        .trim_end_matches(".cmd")
        .trim_end_matches(".exe");
    if program != "npx" {
        return None;
    }
    let first = args.iter().take(3).find(|arg| !arg.starts_with('-'))?;
    if is_filter(first) {
        Some("mcp-filter")
    } else if is_remote(first) {
        Some("mcp-remote")
    } else {
        None
    }
}

/// The definition inside a SkillDock wrapper, or `None` when `raw` is not
/// one or cannot be read back.
fn unwrap(raw: &Raw) -> Option<(Raw, &'static str)> {
    let kind = wrapper_kind(raw.command.as_deref(), &raw.args)?;
    let args = &raw.args;
    let start = args.iter().position(|arg| !arg.starts_with('-'))? + 1;
    match kind {
        "mcp-filter" => {
            let delimiter = args.iter().position(|arg| arg == "--")?;
            let command = args.get(delimiter + 1)?.trim();
            if command.is_empty() || delimiter < start {
                return None;
            }
            let mut allow = Vec::new();
            let mut index = start;
            while index < delimiter {
                if args[index] == "--include"
                    && let Some(tool) = args.get(index + 1).map(|t| t.trim())
                {
                    if !tool.is_empty() && !allow.iter().any(|known| known == tool) {
                        allow.push(tool.to_owned());
                    }
                    index += 2;
                } else {
                    index += 1;
                }
            }
            Some((
                Raw {
                    transport: Some(Transport::Stdio),
                    command: Some(command.to_owned()),
                    args: args[delimiter + 2..].to_vec(),
                    cwd: raw.cwd.clone(),
                    env: raw.env.clone(),
                    allow,
                    ..Raw::default()
                },
                kind,
            ))
        }
        _ => {
            let url = args.get(start)?.trim();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return None;
            }
            let mut inner = Raw {
                transport: Some(Transport::Http),
                url: Some(url.to_owned()),
                ..Raw::default()
            };
            let mut index = start + 1;
            while index < args.len() {
                let value = args.get(index + 1);
                match (args[index].as_str(), value) {
                    ("--transport", Some(mode)) => {
                        inner.transport = Some(match mode.as_str() {
                            "sse-only" | "sse-first" => Transport::Sse,
                            _ => Transport::Http,
                        });
                        index += 2;
                    }
                    ("--header", Some(header)) => {
                        let (name, value) = header.split_once(':')?;
                        inner
                            .headers
                            .insert(name.trim().to_owned(), value.trim().to_owned());
                        index += 2;
                    }
                    ("--ignore-tool", Some(tool)) => {
                        if !inner.deny.iter().any(|known| known == tool) {
                            inner.deny.push(tool.clone());
                        }
                        index += 2;
                    }
                    ("--allow-http", _) => index += 1,
                    _ => return None,
                }
            }
            // The wrapper's `env` fed its own `${NAME}` expansion; a remote
            // server has no environment.
            let mut used = BTreeSet::new();
            inner.url = inner.url.map(|url| substitute(&url, &raw.env, &mut used));
            for value in inner.headers.values_mut() {
                *value = substitute(value, &raw.env, &mut used);
            }
            inner.unused_env = raw
                .env
                .keys()
                .filter(|k| !used.contains(*k))
                .cloned()
                .collect();
            Some((inner, kind))
        }
    }
}

const KNOWN_KEYS: [&str; 7] = ["type", "command", "args", "env", "url", "headers", "cwd"];

fn read_raw(server: &Value, dropped: &mut Vec<String>) -> Result<Raw, CoreError> {
    let map = server.as_object().ok_or_else(|| {
        CoreError::new(
            ErrorCode::UnexpectedShape,
            "the server definition is not an object",
        )
    })?;
    let shape = |field: &str, expected: &str| {
        CoreError::new(
            ErrorCode::UnexpectedShape,
            format!("`{field}` is not {expected}"),
        )
    };
    let string = |key: &str| -> Result<Option<String>, CoreError> {
        match map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) if text.trim().is_empty() => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(shape(key, "a string")),
        }
    };
    let string_map = |key: &str| -> Result<BTreeMap<String, String>, CoreError> {
        match map.get(key) {
            None | Some(Value::Null) => Ok(BTreeMap::new()),
            Some(Value::Object(items)) => items
                .iter()
                .map(|(name, value)| match value {
                    Value::String(text) => Ok((name.clone(), text.clone())),
                    Value::Number(_) | Value::Bool(_) => Ok((name.clone(), value.to_string())),
                    _ => Err(shape(&format!("{key}.{name}"), "a string")),
                })
                .collect(),
            Some(_) => Err(shape(key, "an object")),
        }
    };
    let args = match map.get("args") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(text) => Ok(text.clone()),
                Value::Number(_) | Value::Bool(_) => Ok(item.to_string()),
                _ => Err(shape("args", "a list of strings")),
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err(shape("args", "a list")),
    };
    let command = string("command")?;
    let url = string("url")?;
    let transport = match string("type")?.as_deref() {
        Some("stdio") => Some(Transport::Stdio),
        Some("http" | "streamable-http" | "streamableHttp" | "streamable_http") => {
            Some(Transport::Http)
        }
        Some("sse") => Some(Transport::Sse),
        Some(other) => {
            return Err(CoreError::new(
                ErrorCode::UnsupportedTransport,
                format!("transport {other:?} has no store equivalent"),
            ));
        }
        None if command.is_some() => Some(Transport::Stdio),
        None if url.is_some() => Some(Transport::Http),
        None => None,
    };
    for key in map.keys() {
        if !KNOWN_KEYS.contains(&key.as_str()) {
            dropped.push(key.clone());
        }
    }
    Ok(Raw {
        transport,
        command,
        args,
        cwd: string("cwd")?,
        url,
        env: string_map("env")?,
        headers: string_map("headers")?,
        allow: Vec::new(),
        deny: Vec::new(),
        unused_env: Vec::new(),
    })
}

/// Whether `command` names a program by an absolute path (or `~`), which
/// only holds on this machine (SkillDock rewrote `npx` and friends to
/// `~/.local/bin/...` shims).
pub fn is_machine_path(command: &str) -> bool {
    let bytes = command.as_bytes();
    command.starts_with('/')
        || command.starts_with('~')
        || command.starts_with("\\\\")
        || (bytes.len() > 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'))
}

/// Maps a record to store server `id`. Sensitive literals are replaced by
/// `${secret:<alias>}` references and listed in [`Mapped::extractions`];
/// the values are dropped.
pub fn map_record(record: &Record, id: &str, aliases: &mut Aliases) -> Result<Mapped, CoreError> {
    ids::check(IdKind::Server, id)?;
    let mut dropped = Vec::new();
    let raw = read_raw(&record.server, &mut dropped)?;
    let (raw, unwrapped) = match unwrap(&raw) {
        Some((inner, kind)) => (inner, Some(kind)),
        None if wrapper_kind(raw.command.as_deref(), &raw.args).is_some() => {
            return Err(CoreError::new(
                ErrorCode::UnexpectedShape,
                "a SkillDock wrapper whose arguments cannot be read back",
            ));
        }
        None => (raw, None),
    };
    let Some(transport) = raw.transport else {
        return Err(CoreError::new(
            ErrorCode::MissingField,
            "the definition has neither `command` nor `url`",
        ));
    };
    let caps = &adapter(crate::adapters::claude_code::ID)
        .and_then(|spec| spec.mcp.as_ref())
        .expect("Claude Code has MCP")
        .caps;
    let mut extractor = Extractor::new(id, aliases);
    let mut server = Server::new(id);
    let body = &mut server.body;
    body.transport = Some(transport);
    if let Some(command) = &raw.command {
        body.command = Some(convert(caps, command, FieldSet::COMMAND)?);
    }
    if !raw.args.is_empty() {
        let args = raw
            .args
            .iter()
            .map(|arg| convert(caps, arg, FieldSet::ARGS))
            .collect::<Result<Vec<_>, _>>()?;
        body.args = Some(extractor.args(args));
    }
    if let Some(cwd) = &raw.cwd {
        body.cwd = Some(convert(caps, cwd, FieldSet::CWD)?);
    }
    if let Some(url) = &raw.url {
        let url = convert(caps, url, FieldSet::URL)?;
        body.url = Some(extractor.url(url)?);
    }
    for (field, source, label) in [
        (FieldSet::ENV, &raw.env, "env"),
        (FieldSet::HEADERS, &raw.headers, "headers"),
    ] {
        let mut map = BTreeMap::new();
        for (name, value) in source {
            map.insert(name.clone(), convert(caps, value, field)?);
        }
        let map = extractor.map(label, map);
        let map = (!map.is_empty()).then_some(map);
        match label {
            "env" => body.env = map,
            _ => body.headers = map,
        }
    }
    // The migration keeps no value: the wizard asks for each one.
    let extractions: Vec<Extraction> = extractor
        .finish()
        .into_iter()
        .map(|extracted| extracted.extraction)
        .collect();
    let disabled: Vec<String> = record
        .tools
        .iter()
        .filter(|(_, enabled)| !enabled)
        .map(|(name, _)| name.clone())
        .collect();
    let mut deny = raw.deny.clone();
    for tool in disabled {
        if !deny.contains(&tool) {
            deny.push(tool);
        }
    }
    deny.sort();
    let mut allow = raw.allow.clone();
    allow.sort();
    if !deny.is_empty() || !allow.is_empty() {
        body.tools = Some(ToolFilter {
            // A deny list says the same as the wrapper's include list.
            allow: (deny.is_empty() && !allow.is_empty()).then_some(allow),
            deny: (!deny.is_empty()).then_some(deny),
            extra: toml::Table::new(),
        });
    }
    if record.name != id {
        body.display_name = Some(record.name.clone());
    }
    if !record.description.is_empty() {
        body.description = Some(record.description.clone());
    }
    let mut notes = Vec::new();
    let mut review = Vec::new();
    for (from, to) in &record.renamed {
        notes.push(format!("SkillDock's {from} is {to}"));
    }
    if !raw.unused_env.is_empty() {
        review.push(format!(
            "the wrapper's env keys its headers did not use were dropped: {}",
            raw.unused_env.join(", ")
        ));
    }
    // SkillDock kept `sourceUrl` as given; one with credentials would fail
    // the store's leak scan and with it the whole migration.
    let homepage = (!record.source_url.is_empty()).then(|| record.source_url.clone());
    let homepage = match homepage {
        Some(url) if classify_import(None, &url).is_some() || !find_in_text(&url).is_empty() => {
            review.push(
                "the source URL may hold a credential and was left out of meta.homepage".to_owned(),
            );
            None
        }
        other => other,
    };
    body.meta = Some(Meta {
        homepage,
        origin: Some(ORIGIN.to_owned()),
        extra: toml::Table::new(),
    });
    let mut unknown_clients = Vec::new();
    for client in &record.enabled_app_ids {
        if adapter(client).is_none() {
            unknown_clients.push(client.clone());
        }
    }
    if record.id != id {
        // The native key stays SkillDock's, so existing entries match.
        let mut clients = BTreeMap::new();
        for client in record
            .enabled_app_ids
            .iter()
            .chain(&record.managed_app_ids)
            .chain(&record.imported_from_app_ids)
        {
            if adapter(client).is_some() {
                let mut table = toml::Table::new();
                table.insert(
                    NATIVE_NAME_KEY.to_owned(),
                    toml::Value::String(record.id.clone()),
                );
                clients.insert(client.clone(), table);
            }
        }
        body.client = (!clients.is_empty()).then_some(clients);
    }
    if !record.enabled_app_ids.is_empty() {
        server.enable = Some(Enable {
            clients: Some(ClientSelection::List(record.enabled_app_ids.clone())),
            extra: toml::Table::new(),
        });
    }
    if let Some(err) = server.validate(Some(id)).first_error(None) {
        return Err(err);
    }
    let machine_path = server.body.command.as_deref().is_some_and(is_machine_path);
    Ok(Mapped {
        server,
        unwrapped,
        extractions,
        machine_path,
        dropped,
        unknown_clients,
        notes,
        review,
    })
}

fn convert(caps: &Caps, text: &str, field: FieldSet) -> Result<String, CoreError> {
    native_to_template(text, caps, field).ok_or_else(|| {
        CoreError::new(
            ErrorCode::UnsupportedField,
            "a value uses a variable form the store cannot express (`${NAME:-default}`)",
        )
    })
}

/// `settings.json` fields and how they map (SPEC 15.1): theme, agents CLI
/// compatibility, install activation. The rest is listed as skipped.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkillDockSettings {
    pub fields: Map<String, Value>,
}

impl SkillDockSettings {
    pub fn parse(doc: &Value) -> Result<Self, CoreError> {
        match doc {
            Value::Object(fields) => Ok(Self {
                fields: fields.clone(),
            }),
            _ => Err(CoreError::new(
                ErrorCode::UnexpectedShape,
                "settings are not an object",
            )),
        }
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.fields
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    pub fn bool(&self, key: &str) -> Option<bool> {
        self.fields.get(key).and_then(Value::as_bool)
    }
}

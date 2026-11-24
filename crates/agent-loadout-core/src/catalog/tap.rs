//! Git taps (#26): a repository of skill folders and/or a Claude Code
//! plugin marketplace (`.claude-plugin/marketplace.json`). Indexed from a
//! shallow clone that checks out only what the index reads (`SKILL.md`,
//! `.mcp.json`, `.claude-plugin/`, `LICENSE*`; blobs fetched lazily from
//! remote URLs), through the system git and the clone helpers `skills add`
//! uses. Nothing in the tap runs; symbolic links are never read.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Catalog, CatalogEntry, EntryKind, PluginSource, PluginSourceKind, Publisher};
use crate::error::{CoreError, ErrorCode};
use crate::model::ids;
use crate::remote::git::RunOutput;
use crate::skills::frontmatter;
use crate::skills::install::{
    check_ref, check_url, fetch_url, git_path, is_commit_ref, is_remote_url, remove_dir, rev_parse,
    run_git,
};
use crate::skills::{SKILL_FILE, SkillsEnv, confine, io_error};
use crate::sources::SourceRef;

/// Marketplace names Claude Code reserves for `github.com/anthropics/`.
const RESERVED: [&str; 19] = [
    "claude-code-marketplace",
    "claude-code-plugins",
    "claude-plugins-official",
    "anthropic-marketplace",
    "anthropic-plugins",
    "agent-skills",
    "anthropic-agent-skills",
    "life-sciences",
    "knowledge-work-plugins",
    "claude-for-legal",
    "claude-for-financial-services",
    "financial-services-plugins",
    "first-party-plugins",
    "claude-tag-plugins",
    "claude-community",
    "claude-plugins-community",
    "healthcare",
    "anthropic-plugin-directory",
    "claude-plugin-directory",
];

/// Plugin parts Loadout never imports (code that runs on its own, or
/// client-only features).
const NOT_IMPORTED: [(&str, &str); 7] = [
    ("hooks", "hooks"),
    ("commands", "commands"),
    ("agents", "agents"),
    ("bin", "bin"),
    ("monitors", "monitors"),
    ("workflows", "workflows"),
    ("output-styles", "outputStyles"),
];

/// The marketplace file, from the tap's root.
const MARKETPLACE: &str = ".claude-plugin/marketplace.json";
/// Largest file the index reads.
const MAX_FILE: u64 = 1024 * 1024;

/// A tap's index, cached for its TTL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TapIndex {
    pub commit: String,
    /// The branch or tag followed (`None` when pinned to a commit).
    pub git_ref: Option<String>,
    pub marketplace: Option<String>,
    pub warning: Option<String>,
    pub entries: Vec<CatalogEntry>,
}

/// Whether `url` is a repository of `github.com/anthropics`.
pub(crate) fn is_anthropic(url: &str) -> bool {
    SourceRef::from_git_url(url)
        .is_ok_and(|source| source.key().starts_with("github.com/anthropics/"))
}

/// A plugin of a remote tap that names a local repository (an absolute
/// path, `file://`): a web catalog never points into this machine.
pub(crate) fn local_source_problem(tap_url: &str, source: &SourceRef) -> Option<String> {
    let remote_tap = SourceRef::from_git_url(tap_url).is_ok_and(|tap| !tap.is_local());
    (remote_tap && source.is_local()).then(|| {
        "a plugin of a remote catalog cannot name a repository on this computer".to_owned()
    })
}

/// The tap's Official mark on a plugin: only for code that lives in an
/// Anthropic repository (the tap's own folders, or a source there), never
/// for a third-party plugin an Anthropic marketplace lists.
pub(crate) fn plugin_official(tap_official: bool, source: &PluginSource) -> bool {
    tap_official
        && (source.kind == PluginSourceKind::Relative
            || source.url.as_deref().is_some_and(is_anthropic))
}

/// Whether a marketplace name is reserved for Anthropic, or looks like one
/// (non-ASCII or control characters, `.`/`_`/space for `-`, case).
pub(crate) fn reserved_like(name: &str) -> bool {
    if !name.is_ascii() || name.chars().any(char::is_control) {
        return true;
    }
    let normal = name.to_ascii_lowercase().replace(['.', '_', ' '], "-");
    RESERVED.contains(&normal.as_str())
}

/// The files of the tap's tree: path to whether it is a regular file (a
/// symbolic link or submodule is not).
type Tree = BTreeMap<String, bool>;

fn git_error(what: &str, output: &RunOutput) -> CoreError {
    let detail = output
        .stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("git failed")
        .trim()
        .to_owned();
    CoreError::new(
        ErrorCode::CatalogUnreachable,
        SourceRef::redact_userinfo(&format!("{what}: {detail}")),
    )
}

/// Clones the tap shallow and sparse into `dir` at its ref, and lists its
/// tree. Returns the commit, the branch followed and the tree.
fn clone_tap(
    sx: &SkillsEnv<'_>,
    url: &str,
    git_ref: Option<&str>,
    dir: &Path,
) -> Result<(String, Option<String>, Tree), CoreError> {
    check_url(url)?;
    if let Some(git_ref) = git_ref {
        check_ref(git_ref)?;
    }
    let git = git_path(sx)?;
    let fetch = fetch_url(url)?;
    let parent = dir.parent().expect("a clone folder has a parent");
    confine(sx.fs, dir)?;
    sx.fs
        .create_dir_all(parent)
        .map_err(|err| io_error("cannot create directory", parent, &err))?;
    let remote = is_remote_url(url);
    let commit_ref = git_ref.filter(|r| is_commit_ref(r));
    let mut args: Vec<&str> = vec!["clone", "--no-progress", "--no-checkout"];
    if remote {
        args.extend(["--depth", "1", "--filter=blob:none"]);
    }
    if let Some(branch) = git_ref.filter(|r| !is_commit_ref(r)) {
        args.extend(["--branch", branch]);
    }
    args.push("--");
    args.push(&fetch);
    let dir_text = dir.to_string_lossy().into_owned();
    args.push(&dir_text);
    let run = |cwd: &Path, args: &[&str]| {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        run_git(sx, &git, cwd, &args)
    };
    let output = run(parent, &args)?;
    if !output.success {
        let _ = remove_dir(sx.fs, dir);
        return Err(git_error(
            &format!(
                "the tap {} cannot be reached",
                SourceRef::redact_userinfo(url)
            ),
            &output,
        ));
    }
    let steps: [&[&str]; 1] = [&[
        "sparse-checkout",
        "set",
        "--no-cone",
        SKILL_FILE,
        ".mcp.json",
        ".claude-plugin/",
        "LICENSE*",
    ]];
    for step in steps {
        let output = run(dir, step)?;
        if !output.success {
            return Err(git_error("cannot read the tap", &output));
        }
    }
    let target = match commit_ref {
        Some(wanted) => {
            let mut output = run(dir, &["checkout", "--quiet", "--detach", wanted])?;
            if !output.success && remote {
                let fetched = run(
                    dir,
                    &[
                        "fetch",
                        "--quiet",
                        "--depth",
                        "1",
                        "--filter=blob:none",
                        &fetch,
                        wanted,
                    ],
                )?;
                if fetched.success {
                    output = run(dir, &["checkout", "--quiet", "--detach", "FETCH_HEAD"])?;
                }
            }
            if !output.success {
                return Err(CoreError::new(
                    ErrorCode::NotFound,
                    format!("the tap has no commit {wanted}"),
                ));
            }
            "HEAD"
        }
        None => {
            let output = run(dir, &["checkout", "--quiet"])?;
            if !output.success {
                return Err(git_error("cannot read the tap", &output));
            }
            "HEAD"
        }
    };
    let commit = rev_parse(sx, &git, dir, target)?;
    let branch = match git_ref {
        Some(_) if commit_ref.is_some() => None,
        Some(git_ref) => Some(git_ref.to_owned()),
        None => {
            let output = run(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
            output
                .success
                .then(|| output.stdout.trim().to_owned())
                .filter(|branch| !branch.is_empty())
        }
    };
    let output = run(dir, &["ls-tree", "-r", "-z", "--full-tree", "HEAD"])?;
    if !output.success {
        return Err(git_error("cannot list the tap", &output));
    }
    let mut tree = Tree::new();
    for record in output.stdout.split('\0').filter(|r| !r.is_empty()) {
        let Some((meta, path)) = record.split_once('\t') else {
            continue;
        };
        let mut parts = meta.split(' ');
        let (mode, kind) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        tree.insert(path.to_owned(), kind == "blob" && mode != "120000");
    }
    // Config files a plugin names in `mcpServers`: checked out too, so the
    // index reads their server names.
    let patterns = server_file_patterns(sx, dir, &tree);
    if !patterns.is_empty() {
        let mut args = vec!["sparse-checkout", "add"];
        args.extend(patterns.iter().map(String::as_str));
        let output = run(dir, &args)?;
        if !output.success {
            return Err(git_error("cannot read the tap", &output));
        }
    }
    Ok((commit, branch, tree))
}

/// Sparse patterns for the files named by a string `mcpServers` in the
/// marketplace or a plugin's `plugin.json`: plain relative paths only
/// (`a-z 0-9 . _ - /`, no `..`), at most 64.
fn server_file_patterns(sx: &SkillsEnv<'_>, dir: &Path, tree: &Tree) -> Vec<String> {
    let manifests = tree.keys().filter(|path| {
        *path == MARKETPLACE
            || *path == ".claude-plugin/plugin.json"
            || path.ends_with("/.claude-plugin/plugin.json")
    });
    let mut patterns: Vec<String> = Vec::new();
    for path in manifests {
        let Some(json) =
            read(sx, dir, tree, path).and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            continue;
        };
        let mut values = vec![json.get("mcpServers")];
        if let Some(plugins) = json.get("plugins").and_then(Value::as_array) {
            values.extend(plugins.iter().map(|plugin| plugin.get("mcpServers")));
        }
        for rel in values.into_iter().flatten().filter_map(Value::as_str) {
            let rel = rel.trim().trim_start_matches("./");
            let plain = (1..=200).contains(&rel.len())
                && rel
                    .split('/')
                    .all(|part| !part.is_empty() && part != "." && part != "..")
                && rel
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b));
            let pattern = format!("**/{rel}");
            if plain && patterns.len() < 64 && !patterns.contains(&pattern) {
                patterns.push(pattern);
            }
        }
    }
    patterns
}

/// The text of a regular file of the tap that the checkout holds.
fn read(sx: &SkillsEnv<'_>, dir: &Path, tree: &Tree, path: &str) -> Option<String> {
    if tree.get(path) != Some(&true) {
        return None;
    }
    let file = path
        .split('/')
        .fold(dir.to_path_buf(), |file, part| file.join(part));
    // The size first: a huge file is never read into memory.
    if std::fs::symlink_metadata(&file).ok()?.len() > MAX_FILE {
        return None;
    }
    let bytes = sx.fs.read(&file).ok()?;
    (bytes.len() as u64 <= MAX_FILE).then(|| String::from_utf8_lossy(&bytes).into_owned())
}

/// A relative path inside the tap: `/`-separated, no `..`, not absolute.
fn clean_rel(base: &str, rel: &str) -> Option<String> {
    if rel.contains('\\') || rel.starts_with('/') || rel.contains(':') {
        return None;
    }
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// Whether the tree holds anything below `dir`.
fn has_dir(tree: &Tree, dir: &str) -> bool {
    let prefix = format!("{dir}/");
    tree.range(prefix.clone()..)
        .next()
        .is_some_and(|(path, _)| path.starts_with(&prefix))
}

/// Skill folders directly below `dir` (`<dir>/<name>/SKILL.md`).
fn skills_below(tree: &Tree, dir: &str) -> Vec<String> {
    let prefix = format!("{}/", dir.trim_end_matches('/'));
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        prefix
    };
    tree.iter()
        .filter(|(_, regular)| **regular)
        .filter_map(|(path, _)| {
            let rest = path.strip_prefix(&prefix)?;
            let (name, file) = rest.split_once('/')?;
            (file == SKILL_FILE).then(|| join(dir, name))
        })
        .collect()
}

/// The license of a skill folder: its frontmatter `license`, else the kind
/// of its `LICENSE*` file.
fn license(
    sx: &SkillsEnv<'_>,
    dir: &Path,
    tree: &Tree,
    folder: &str,
    fm: Option<&frontmatter::Frontmatter>,
) -> Option<String> {
    if let Some(text) = fm
        .and_then(|fm| fm.get("license"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return Some(text.to_owned());
    }
    let prefix = if folder.is_empty() {
        String::new()
    } else {
        format!("{folder}/")
    };
    let (path, _) = tree.iter().find(|(path, regular)| {
        **regular
            && path
                .strip_prefix(&prefix)
                .is_some_and(|name| !name.contains('/') && name.starts_with("LICENSE"))
    })?;
    let text = read(sx, dir, tree, path).unwrap_or_default();
    let name = path.rsplit('/').next().unwrap_or(path);
    Some(
        if text.contains("Apache License") && text.contains("Version 2.0") {
            "Apache-2.0".to_owned()
        } else if text.contains("MIT License")
            || text.contains("Permission is hereby granted, free of charge")
        {
            "MIT".to_owned()
        } else {
            format!("See {name}")
        },
    )
}

/// What a tap's skill or plugin shares.
struct TapCtx<'a> {
    catalog: &'a Catalog,
    commit: String,
    git_ref: Option<String>,
    pinned: bool,
    official: bool,
    warning: Option<String>,
    repo_name: String,
}

impl TapCtx<'_> {
    fn entry(&self, id: String, kind: EntryKind, name: String) -> CatalogEntry {
        CatalogEntry {
            catalog: self.catalog.id.clone(),
            id,
            kind,
            name,
            description: None,
            version: None,
            updated_at: None,
            deprecated: false,
            status_message: None,
            publisher: None,
            official: self.official,
            warning: self.warning.clone(),
            repository: Some(SourceRef::redact_userinfo(&self.catalog.url)),
            website: None,
            license: None,
            transports: Vec::new(),
            packages: Vec::new(),
            remotes: Vec::new(),
            required_secrets: Vec::new(),
            pinned: self.pinned,
            git_ref: self.git_ref.clone(),
            commit: Some(self.commit.clone()),
            path: None,
            skills: Vec::new(),
            servers: Vec::new(),
            not_imported: Vec::new(),
            source: None,
            install_mcp: false,
            install_skill: false,
            problem: None,
            server_json: None,
            mcp_servers: None,
        }
    }
}

/// A string or the `url` of an object (`repository`, `author`).
fn text_of(value: Option<&Value>, key: &str) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.trim().to_owned()),
        Value::Object(map) => map.get(key)?.as_str().map(|t| t.trim().to_owned()),
        _ => None,
    }
    .filter(|text| !text.is_empty())
}

/// `owner/repo` as a GitHub URL, or a git URL as it is.
fn git_url_of(text: &str) -> String {
    let is_slug = !text.contains("://")
        && !text.contains('@')
        && text.split('/').count() == 2
        && text.split('/').all(|part| !part.is_empty());
    if is_slug {
        format!("https://github.com/{}", text.trim_end_matches(".git"))
    } else {
        text.to_owned()
    }
}

/// A plugin's `source`: where its files are.
fn plugin_source(
    ctx: &TapCtx<'_>,
    tree: &Tree,
    plugin_root: Option<&str>,
    source: Option<&Value>,
) -> (PluginSource, Option<String>) {
    let unsupported = |kind: PluginSourceKind, why: &str| {
        (
            PluginSource {
                kind,
                url: None,
                path: None,
                git_ref: None,
                sha: None,
            },
            Some(why.to_owned()),
        )
    };
    match source {
        Some(Value::String(rel)) => {
            let rel = rel.trim();
            let path = if rel == "." || rel.starts_with("./") {
                clean_rel("", rel)
            } else if let Some(root) = plugin_root
                && !rel.contains('/')
                && !rel.is_empty()
            {
                clean_rel("", root).and_then(|root| clean_rel(&root, rel))
            } else {
                None
            };
            let Some(path) = path else {
                return unsupported(
                    PluginSourceKind::Relative,
                    "its folder is not a relative path inside the tap",
                );
            };
            if !path.is_empty() && !has_dir(tree, &path) {
                return unsupported(PluginSourceKind::Relative, "its folder is not in the tap");
            }
            (
                PluginSource {
                    kind: PluginSourceKind::Relative,
                    url: Some(SourceRef::redact_userinfo(&ctx.catalog.url)),
                    path: (!path.is_empty()).then_some(path),
                    git_ref: ctx.git_ref.clone(),
                    sha: ctx.pinned.then(|| ctx.commit.clone()),
                },
                None,
            )
        }
        Some(Value::Object(map)) => {
            let get = |key: &str| {
                map.get(key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
            };
            let (kind, url, path) = match get("source") {
                Some("github") => (PluginSourceKind::Github, get("repo").map(git_url_of), None),
                Some("url") => (PluginSourceKind::Url, get("url").map(git_url_of), None),
                Some("git-subdir") => (
                    PluginSourceKind::GitSubdir,
                    get("url").map(git_url_of),
                    get("path").and_then(|path| clean_rel("", path)),
                ),
                Some("npm") => {
                    return unsupported(
                        PluginSourceKind::Npm,
                        "plugins from npm sources are not supported",
                    );
                }
                Some("archive") => {
                    return unsupported(
                        PluginSourceKind::Archive,
                        "plugins from archive sources are not supported",
                    );
                }
                Some("command") => {
                    return unsupported(
                        PluginSourceKind::Command,
                        "plugins from command sources are never installed: they run a command",
                    );
                }
                _ => {
                    return unsupported(
                        PluginSourceKind::Unknown,
                        "this plugin's source kind is not known",
                    );
                }
            };
            let sha = get("sha").map(str::to_owned);
            let git_ref = get("ref").map(str::to_owned);
            let problem = match &url {
                None => Some("its source names no repository".to_owned()),
                Some(url) => match SourceRef::from_git_url(url) {
                    Err(err) => Some(err.message().to_owned()),
                    Ok(source) => local_source_problem(&ctx.catalog.url, &source),
                },
            }
            .or_else(|| {
                sha.as_deref()
                    .filter(|sha| !ids::is_commit_id(sha))
                    .map(|_| "its sha is not a full commit id".to_owned())
            })
            .or_else(|| {
                git_ref
                    .as_deref()
                    .filter(|r| check_ref(r).is_err())
                    .map(|_| "its ref is not a branch or tag".to_owned())
            })
            .or_else(|| {
                (kind == PluginSourceKind::GitSubdir && path.is_none())
                    .then(|| "its folder is not a relative path".to_owned())
            });
            (
                PluginSource {
                    kind,
                    url: url.map(|url| SourceRef::redact_userinfo(&url)),
                    path,
                    git_ref,
                    sha,
                },
                problem,
            )
        }
        _ => unsupported(PluginSourceKind::Unknown, "this plugin names no source"),
    }
}

/// Strings of a `skills`-like field: one string or a list.
fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(text)) => vec![text.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// Server names of an `.mcp.json` (with or without `mcpServers`).
fn server_names(json: &Value) -> Vec<String> {
    let servers = json.get("mcpServers").unwrap_or(json);
    servers
        .as_object()
        .map(|map| {
            map.iter()
                .filter(|(_, value)| value.is_object())
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The skill folders, MCP server names and parts never imported of a
/// plugin inside the tap at `folder`.
fn plugin_parts(
    sx: &SkillsEnv<'_>,
    dir: &Path,
    tree: &Tree,
    folder: &str,
    entry: &Value,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let manifest: Option<Value> = read(sx, dir, tree, &join(folder, ".claude-plugin/plugin.json"))
        .and_then(|text| serde_json::from_str(&text).ok());
    let field = |key: &str| {
        entry
            .get(key)
            .or_else(|| manifest.as_ref().and_then(|m| m.get(key)))
    };
    let mut skills: Vec<String> = Vec::new();
    let mut add_skill = |path: String| {
        if !skills.contains(&path) {
            skills.push(path);
        }
    };
    let listed = strings(field("skills"));
    if listed.is_empty() {
        let below = skills_below(tree, &join(folder, "skills"));
        if below.is_empty() {
            if tree.get(&join(folder, SKILL_FILE)) == Some(&true) {
                add_skill(folder.to_owned());
            }
        } else {
            below.into_iter().for_each(&mut add_skill);
        }
    } else {
        for rel in listed {
            let Some(path) = clean_rel(folder, &rel) else {
                continue;
            };
            if tree.get(&join(&path, SKILL_FILE)) == Some(&true) {
                add_skill(path);
            } else {
                skills_below(tree, &path)
                    .into_iter()
                    .for_each(&mut add_skill);
            }
        }
    }
    let mut servers: Vec<String> = Vec::new();
    let mut add_servers = |names: Vec<String>| {
        for name in names {
            if !servers.contains(&name) {
                servers.push(name);
            }
        }
    };
    if let Some(json) = read(sx, dir, tree, &join(folder, ".mcp.json"))
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    {
        add_servers(server_names(&json));
    }
    match field("mcpServers") {
        Some(Value::Object(_)) => add_servers(server_names(&serde_json::json!({
            "mcpServers": field("mcpServers").cloned().unwrap_or_default()
        }))),
        Some(Value::String(rel)) => {
            if let Some(json) = clean_rel(folder, rel)
                .and_then(|path| read(sx, dir, tree, &path))
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            {
                add_servers(server_names(&json));
            }
        }
        _ => {}
    }
    let not_imported = NOT_IMPORTED
        .iter()
        .filter(|(folder_name, key)| {
            has_dir(tree, &join(folder, folder_name)) || field(key).is_some()
        })
        .map(|(folder_name, _)| (*folder_name).to_owned())
        .collect();
    (skills, servers, not_imported)
}

/// Indexes the tap `catalog` (a fresh clone, removed afterward). Folders
/// whose first part is in `exclude` are left out.
pub(crate) fn index(
    sx: &SkillsEnv<'_>,
    catalog: &Catalog,
    exclude: &[&str],
) -> Result<TapIndex, CoreError> {
    let dir: PathBuf = super::cache::root(sx).join(format!(
        ".tmp-{}",
        ids::new_ulid(sx.env.now())?.to_ascii_lowercase()
    ));
    let result = clone_tap(sx, &catalog.url, catalog.git_ref.as_deref(), &dir).and_then(
        |(commit, git_ref, tree)| build(sx, catalog, exclude, &dir, commit, git_ref, &tree),
    );
    let _ = remove_dir(sx.fs, &dir);
    result
}

fn build(
    sx: &SkillsEnv<'_>,
    catalog: &Catalog,
    exclude: &[&str],
    dir: &Path,
    commit: String,
    git_ref: Option<String>,
    tree: &Tree,
) -> Result<TapIndex, CoreError> {
    let marketplace: Option<Value> =
        read(sx, dir, tree, MARKETPLACE).and_then(|text| serde_json::from_str(&text).ok());
    let market_name = marketplace
        .as_ref()
        .and_then(|m| m.get("name"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let official = is_anthropic(&catalog.url);
    let warning = market_name
        .as_deref()
        .filter(|name| !official && reserved_like(name))
        .map(|name| {
            format!(
                "this marketplace uses the name {name:?}, which is reserved for Anthropic's \
                 marketplaces, but it is not from github.com/anthropics"
            )
        });
    let repo_name = catalog
        .url
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\', ':'])
        .next()
        .unwrap_or("skill")
        .trim_end_matches(".git")
        .to_owned();
    let ctx = TapCtx {
        catalog,
        pinned: catalog.git_ref.as_deref().is_some_and(is_commit_ref),
        commit,
        git_ref,
        official,
        warning: warning.clone(),
        repo_name,
    };
    let excluded = |path: &str| {
        let first = path.split('/').next().unwrap_or_default();
        exclude.contains(&first)
    };
    // Skill folders: the root SKILL.md, skills/*/SKILL.md, then each
    // relative plugin's skills.
    let mut skill_dirs: Vec<String> = Vec::new();
    if tree.get(SKILL_FILE) == Some(&true) {
        skill_dirs.push(String::new());
    }
    skill_dirs.extend(skills_below(tree, "skills"));
    let mut plugins = Vec::new();
    let owner = marketplace
        .as_ref()
        .and_then(|m| text_of(m.get("owner"), "name"));
    let plugin_root = marketplace
        .as_ref()
        .and_then(|m| m.get("metadata"))
        .and_then(|meta| meta.get("pluginRoot"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    for plugin in marketplace
        .as_ref()
        .and_then(|m| m.get("plugins"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        let Some(name) = plugin
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| super::is_plugin_name(name))
        else {
            continue;
        };
        let (source, mut problem) =
            plugin_source(&ctx, tree, plugin_root.as_deref(), plugin.get("source"));
        let (skills, servers, not_imported) = match (&source.kind, &problem) {
            (PluginSourceKind::Relative, None) => {
                let folder = source.path.clone().unwrap_or_default();
                plugin_parts(sx, dir, tree, &folder, plugin)
            }
            _ => (Vec::new(), Vec::new(), Vec::new()),
        };
        for skill in &skills {
            if !skill_dirs.contains(skill) && !excluded(skill) {
                skill_dirs.push(skill.clone());
            }
        }
        let relative = source.kind == PluginSourceKind::Relative;
        if problem.is_none() && relative && skills.is_empty() && servers.is_empty() {
            problem = Some("this plugin holds no skill or MCP server Loadout imports".to_owned());
        }
        let mut entry = ctx.entry(
            format!("plugin/{name}"),
            EntryKind::Plugin,
            plugin
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or(name)
                .to_owned(),
        );
        entry.description = text_of(plugin.get("description"), "");
        entry.version = text_of(plugin.get("version"), "");
        entry.license = text_of(plugin.get("license"), "");
        entry.website = text_of(plugin.get("homepage"), "");
        if let Some(repo) = text_of(plugin.get("repository"), "url") {
            entry.repository = Some(SourceRef::redact_userinfo(&repo));
        } else if !relative {
            entry.repository.clone_from(&source.url);
        }
        entry.publisher = text_of(plugin.get("author"), "name")
            .or_else(|| owner.clone())
            .map(|name| Publisher {
                name,
                checked: None,
            });
        if !relative {
            entry.pinned = source.sha.is_some();
            entry.git_ref.clone_from(&source.git_ref);
            entry.commit.clone_from(&source.sha);
        }
        entry.path = if relative { source.path.clone() } else { None };
        entry.mcp_servers = plugin
            .get("mcpServers")
            .filter(|value| value.is_object() || value.is_string())
            .map(Value::to_string);
        entry.skills = skills;
        entry.servers = servers;
        entry.not_imported = not_imported;
        entry.install_mcp = problem.is_none() && (!relative || !entry.servers.is_empty());
        entry.install_skill = problem.is_none() && (!relative || !entry.skills.is_empty());
        entry.problem = problem;
        entry.official = plugin_official(ctx.official, &source);
        entry.source = Some(source);
        plugins.push(entry);
    }
    // Skill entries, one per folder; ids from the folder name, made unique.
    let mut taken: BTreeSet<String> = BTreeSet::new();
    let mut ids_of: BTreeMap<String, String> = BTreeMap::new();
    let mut entries = Vec::new();
    for folder in skill_dirs.into_iter().filter(|folder| !excluded(folder)) {
        let base = if folder.is_empty() {
            ctx.repo_name.to_lowercase()
        } else {
            folder.rsplit('/').next().unwrap_or(&folder).to_owned()
        };
        let mut id = base.clone();
        let mut n = 2;
        while taken.contains(&id) || id.starts_with("plugin/") {
            id = format!("{base}-{n}");
            n += 1;
        }
        taken.insert(id.clone());
        let text = read(sx, dir, tree, &join(&folder, SKILL_FILE)).unwrap_or_default();
        let fm = frontmatter::parse(&text).ok().flatten();
        let mut entry = ctx.entry(
            id.clone(),
            EntryKind::Skill,
            fm.as_ref()
                .and_then(|fm| fm.name())
                .unwrap_or(&base)
                .to_owned(),
        );
        entry.description = fm
            .as_ref()
            .and_then(|fm| fm.description())
            .map(str::to_owned);
        entry.license = license(sx, dir, tree, &folder, fm.as_ref());
        entry.path = (!folder.is_empty()).then(|| folder.clone());
        entry.install_skill = true;
        ids_of.insert(folder, id);
        entries.push(entry);
    }
    // A plugin lists its skills by their entry ids.
    for plugin in &mut plugins {
        plugin.skills = plugin
            .skills
            .iter()
            .filter_map(|folder| ids_of.get(folder).cloned())
            .collect();
    }
    entries.extend(plugins);
    Ok(TapIndex {
        commit: ctx.commit.clone(),
        git_ref: ctx.git_ref.clone(),
        marketplace: market_name,
        warning,
        entries,
    })
}

//! The converters (#5): what a folder's files say about the MCP servers it
//! offers, as raw drafts. Nothing here runs or fetches anything; values
//! are read as written. A declared secret becomes the provisional
//! reference `${secret:<KEY>}` (the resolver renames it to a free alias),
//! a literal that may be a secret stays for the resolver's extraction.
//! Parse errors never quote the file: a skip says where, not what.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::adapters::adapter;
use crate::apply::import::read_native;
use crate::env::Os;
use crate::error::{CoreError, ErrorCode};
use crate::formats::{Document, Format};
use crate::model::server::{ServerFields, ServerOverride, ToolFilter, Transport};
use crate::remote::git::is_github_url;
use crate::secrets::classify::classify_import;
use crate::skills::SkillFiles;
use crate::sources::SourceRef;

use super::snippet;
use super::{DraftInput, DraftKind, Skipped};

/// Stands for the folder of a git source's checkout until the draft has
/// its id (`${loadout_home}/servers/<id>[/<path>]`). Never leaves memory.
pub(crate) const CHECKOUT_TOKEN: &str = "\u{0}checkout\u{0}";
/// Stands for the draft's id (local image tags) until it has one.
pub(crate) const ID_TOKEN: &str = "\u{0}id\u{0}";
/// The namespace of images a draft builds locally: `docker run` never
/// resolves it to a registry (and `--pull=never` refuses to try).
pub(crate) const IMAGE_PREFIX: &str = "loadout-local/";

/// The client config files read, with the client whose shape each has
/// (`None`: detected like a pasted snippet).
const CLIENT_FILES: [(&str, Option<&str>); 7] = [
    (".mcp.json", Some("claude-code")),
    ("mcp.json", None),
    (".vscode/mcp.json", Some("vscode-legacy")),
    (".cursor/mcp.json", Some("cursor")),
    (".codex/config.toml", Some("codex")),
    (".omp/mcp.json", Some("omp")),
    ("claude_desktop_config.json", Some("claude-desktop")),
];

/// Registries a package runs from without naming one.
const DEFAULT_REGISTRIES: [&str; 6] = [
    "https://registry.npmjs.org",
    "https://pypi.org",
    "https://pypi.org/simple",
    "https://api.nuget.org/v3/index.json",
    "https://docker.io",
    "https://registry-1.docker.io",
];

/// A git source pinned to the resolved commit.
#[derive(Debug, Clone)]
pub(crate) struct GitPin {
    pub url: String,
    pub commit: String,
    /// The folder inside the repository.
    pub path: Option<String>,
}

/// What the converters read.
pub(crate) struct Ctx<'a> {
    pub files: &'a SkillFiles,
    /// Dependency and build folders left out of `files` (relative paths).
    pub skipped: &'a [String],
    /// How a command names the folder: a local folder's template path, or
    /// [`CHECKOUT_TOKEN`] for a git source.
    pub root: String,
    pub git: Option<&'a GitPin>,
    pub os: Os,
    /// The repository or folder name (fallback names).
    pub name: String,
    /// `skills::content_hash` of `files` (local image tags).
    pub content_hash: String,
}

/// A draft before ids, aliases and secret extraction.
#[derive(Debug, Clone)]
pub(crate) struct RawDraft {
    pub kind: DraftKind,
    pub file: String,
    pub converter: String,
    pub entry: Option<String>,
    /// Where the id comes from.
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub fields: ServerFields,
    pub os: Option<BTreeMap<String, ServerOverride>>,
    pub tools: Option<ToolFilter>,
    /// The client's allowlisted native fields (`[client.<id>]`).
    pub passthrough: Option<(String, toml::Table)>,
    /// Declared inputs; a secret's `alias` is its provisional key.
    pub inputs: Vec<DraftInput>,
    pub runs_repo_code: bool,
    /// The command names a package version (or commit); `false` for a
    /// registry package without a concrete version.
    pub pinned: bool,
    pub setup: Vec<String>,
}

impl RawDraft {
    fn new(kind: DraftKind, file: &str, converter: &str, name: String) -> Self {
        Self {
            kind,
            file: file.to_owned(),
            converter: converter.to_owned(),
            entry: None,
            name,
            display_name: None,
            description: None,
            homepage: None,
            fields: ServerFields::default(),
            os: None,
            tools: None,
            passthrough: None,
            inputs: Vec::new(),
            runs_repo_code: false,
            pinned: true,
            setup: Vec::new(),
        }
    }
}

/// Every draft the folder offers, in priority order: `server.json`, an
/// MCPB manifest, committed client configs, and only when those gave
/// nothing, the folder as a package (`package.json` bin, `pyproject.toml`
/// scripts, `Dockerfile`).
pub(crate) fn all(ctx: &Ctx<'_>) -> (Vec<RawDraft>, Vec<Skipped>) {
    let mut drafts = Vec::new();
    let mut skipped = Vec::new();
    let run = |result: Result<(Vec<RawDraft>, Vec<Skipped>), Skipped>,
               drafts: &mut Vec<RawDraft>,
               skipped: &mut Vec<Skipped>| match result {
        Ok((mut found, mut skip)) => {
            drafts.append(&mut found);
            skipped.append(&mut skip);
        }
        Err(skip) => skipped.push(skip),
    };
    if let Some(bytes) = ctx.files.get("server.json") {
        run(registry(ctx, bytes), &mut drafts, &mut skipped);
    }
    if let Some(bytes) = ctx.files.get("manifest.json")
        && is_bundle(bytes)
    {
        let found = bundle(ctx, bytes).map(|draft| (vec![draft], Vec::new()));
        run(found, &mut drafts, &mut skipped);
    }
    for (file, hint) in CLIENT_FILES {
        if let Some(bytes) = ctx.files.get(file) {
            run(
                client_config(ctx, file, hint, bytes),
                &mut drafts,
                &mut skipped,
            );
        }
    }
    if drafts.is_empty() {
        if let Some(bytes) = ctx.files.get("package.json") {
            run(npm(ctx, bytes), &mut drafts, &mut skipped);
        }
        if let Some(bytes) = ctx.files.get("pyproject.toml") {
            run(python(ctx, bytes), &mut drafts, &mut skipped);
        }
        if ctx.files.contains_key("Dockerfile") {
            drafts.push(docker(ctx));
        }
    }
    (drafts, skipped)
}

fn skip(file: &str, entry: Option<String>, error: CoreError) -> Skipped {
    Skipped {
        file: file.to_owned(),
        entry,
        error,
    }
}

/// A file that does not parse: where, never what (a parser's message can
/// quote the file, and the file may hold a secret).
fn parse_error(file: &str, line: Option<usize>) -> Skipped {
    let message = match line {
        Some(line) => format!("could not parse {file} (line {line})"),
        None => format!("could not parse {file}"),
    };
    skip(file, None, CoreError::new(ErrorCode::ParseError, message))
}

fn json_error(file: &str, err: &serde_json::Error) -> Skipped {
    parse_error(file, Some(err.line()).filter(|line| *line > 0))
}

fn toml_error(file: &str, text: &str, err: &toml::de::Error) -> Skipped {
    let line = err
        .span()
        .map(|span| text[..span.start.min(text.len())].matches('\n').count() + 1);
    parse_error(file, line)
}

/// A literal for a store field: `${` written as `$${`.
pub(crate) fn escape(text: &str) -> String {
    text.replace("${", "$${")
}

fn secret_ref(key: &str) -> String {
    format!("${{secret:{key}}}")
}

/// An environment variable name from any text: upper case, `_` for other
/// characters.
fn env_name(text: &str) -> String {
    let mut name: String = text
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    name = name.trim_matches('_').to_owned();
    if name.is_empty() || name.as_bytes()[0].is_ascii_digit() {
        name.insert(0, '_');
    }
    name
}

fn env_ref(name: &str) -> String {
    format!("${{env:{}}}", env_name(name))
}

// ---------------------------------------------------------------- registry

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RegistryServer {
    name: Option<String>,
    title: Option<String>,
    description: Option<String>,
    #[serde(alias = "website_url")]
    website_url: Option<String>,
    packages: Vec<Package>,
    remotes: Vec<Remote>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Package {
    #[serde(alias = "registry_type", alias = "registry_name")]
    registry_type: Option<String>,
    #[serde(alias = "registry_base_url")]
    registry_base_url: Option<String>,
    #[serde(alias = "name")]
    identifier: Option<String>,
    version: Option<String>,
    #[serde(alias = "runtime_hint")]
    runtime_hint: Option<String>,
    transport: Option<PackageTransport>,
    #[serde(alias = "runtime_arguments")]
    runtime_arguments: Vec<Argument>,
    #[serde(alias = "package_arguments")]
    package_arguments: Vec<Argument>,
    #[serde(alias = "environment_variables")]
    environment_variables: Vec<Input>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct PackageTransport {
    #[serde(rename = "type")]
    kind: Option<String>,
    url: Option<String>,
    headers: Vec<Input>,
    variables: BTreeMap<String, Input>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Remote {
    #[serde(rename = "type", alias = "transport_type")]
    kind: Option<String>,
    url: Option<String>,
    headers: Vec<Input>,
    variables: BTreeMap<String, Input>,
}

/// An environment variable, header, argument or variable of the registry
/// schema: the shared input fields.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Input {
    name: Option<String>,
    description: Option<String>,
    #[serde(alias = "is_required")]
    is_required: bool,
    #[serde(alias = "is_secret")]
    is_secret: bool,
    value: Option<String>,
    default: Option<serde_json::Value>,
    variables: BTreeMap<String, Input>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Argument {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(alias = "value_hint")]
    value_hint: Option<String>,
    #[serde(flatten)]
    input: Input,
}

fn json_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// `text` (a literal) with each `{name}` of `variables` filled in.
fn fill(
    text: &str,
    variables: &BTreeMap<String, Input>,
    field: &str,
    inputs: &mut Vec<DraftInput>,
) -> String {
    let mut out = escape(text);
    for (name, variable) in variables {
        let pattern = format!("{{{name}}}");
        if out.contains(&pattern) {
            let filled = input_value(variable, name, field, inputs);
            out = out.replace(&pattern, &filled);
        }
    }
    out
}

/// The store value of one registry input at `field` (`env.NAME`,
/// `headers.NAME`, `args[N]`, `url`): a secret is a provisional reference,
/// a value has its `{var}`s filled in, else the default, else an
/// environment reference.
fn input_value(input: &Input, key: &str, field: &str, inputs: &mut Vec<DraftInput>) -> String {
    if input.is_secret {
        inputs.push(DraftInput {
            field: field.to_owned(),
            description: input.description.clone(),
            secret: true,
            required: input.is_required,
            alias: Some(env_name(key)),
            value_dropped: false,
        });
        return secret_ref(&env_name(key));
    }
    if let Some(value) = &input.value {
        return fill(value, &input.variables, field, inputs);
    }
    if let Some(default) = input.default.as_ref().and_then(json_text) {
        return escape(&default);
    }
    inputs.push(DraftInput {
        field: field.to_owned(),
        description: input.description.clone(),
        secret: false,
        required: input.is_required,
        alias: None,
        value_dropped: false,
    });
    env_ref(key)
}

fn arguments(arguments: &[Argument], args: &mut Vec<String>, inputs: &mut Vec<DraftInput>) {
    for argument in arguments {
        let named = argument.kind.as_deref() == Some("named");
        let name = argument.input.name.clone().unwrap_or_default();
        if named && !name.is_empty() {
            args.push(escape(&name));
        }
        // A named argument without a value is a flag, unless it is
        // required (its value comes from the user then).
        let has_value = argument.input.value.is_some()
            || argument.input.default.is_some()
            || argument.input.is_secret
            || argument.input.is_required
            || !named;
        if has_value {
            let key = argument
                .value_hint
                .clone()
                .filter(|hint| !hint.is_empty())
                .unwrap_or_else(|| name.trim_start_matches('-').to_owned());
            let key = if key.is_empty() {
                "arg".to_owned()
            } else {
                key
            };
            let field = format!("args[{}]", args.len() + 1);
            let value = input_value(&argument.input, &key, &field, inputs);
            args.push(value);
        }
    }
}

fn header_map(headers: &[Input], inputs: &mut Vec<DraftInput>) -> Option<BTreeMap<String, String>> {
    let map: BTreeMap<String, String> = headers
        .iter()
        .filter_map(|header| {
            let name = header.name.clone()?;
            let field = format!("headers.{name}");
            Some((name.clone(), input_value(header, &name, &field, inputs)))
        })
        .collect();
    (!map.is_empty()).then_some(map)
}

fn remote_transport(kind: Option<&str>) -> Option<Transport> {
    match kind.unwrap_or("streamable-http") {
        "streamable-http" | "http" | "streamable_http" => Some(Transport::Http),
        "sse" => Some(Transport::Sse),
        _ => None,
    }
}

/// The last segment of a registry name (`io.github.acme/weather`).
fn short_name(name: &str) -> String {
    name.rsplit('/').next().unwrap_or(name).to_owned()
}

fn registry(ctx: &Ctx<'_>, bytes: &[u8]) -> Result<(Vec<RawDraft>, Vec<Skipped>), Skipped> {
    const FILE: &str = "server.json";
    let server: RegistryServer =
        serde_json::from_slice(bytes).map_err(|err| json_error(FILE, &err))?;
    let base = server
        .name
        .as_deref()
        .map(short_name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| ctx.name.clone());
    let several = server.packages.len() > 1;
    let mut drafts = Vec::new();
    let mut skipped = Vec::new();
    let template = |name: String, entry: String| {
        let mut draft = RawDraft::new(DraftKind::Registry, FILE, "registry", name);
        draft.entry = Some(entry);
        draft.display_name.clone_from(&server.title);
        draft.description.clone_from(&server.description);
        draft.homepage.clone_from(&server.website_url);
        draft
    };
    for (index, package) in server.packages.iter().enumerate() {
        let entry = format!("packages[{index}]");
        let kind = package.registry_type.as_deref().unwrap_or("npm");
        let name = if several {
            format!("{base}-{kind}")
        } else {
            base.clone()
        };
        let mut draft = template(name, entry.clone());
        match package_fields(package, kind, &mut draft) {
            Ok(()) => drafts.push(draft),
            Err(err) => skipped.push(skip(FILE, Some(entry), err)),
        }
    }
    for (index, remote) in server.remotes.iter().enumerate() {
        let entry = format!("remotes[{index}]");
        let name = if server.packages.is_empty() && server.remotes.len() == 1 {
            base.clone()
        } else if index == 0 {
            format!("{base}-remote")
        } else {
            format!("{base}-remote-{}", index + 1)
        };
        let mut draft = template(name, entry.clone());
        let (Some(transport), Some(url)) = (remote_transport(remote.kind.as_deref()), &remote.url)
        else {
            skipped.push(skip(
                FILE,
                Some(entry),
                CoreError::new(
                    ErrorCode::UnsupportedField,
                    "a remote without a transport and URL Loadout can use",
                ),
            ));
            continue;
        };
        draft.fields.transport = Some(transport);
        draft.fields.url = Some(fill(url, &remote.variables, "url", &mut draft.inputs));
        draft.fields.headers = header_map(&remote.headers, &mut draft.inputs);
        drafts.push(draft);
    }
    if drafts.is_empty() && skipped.is_empty() {
        return Err(skip(
            FILE,
            None,
            CoreError::new(
                ErrorCode::UnexpectedShape,
                "server.json has no packages or remotes",
            ),
        ));
    }
    Ok((drafts, skipped))
}

/// A registry other than the package type's default, as written.
fn custom_registry(package: &Package) -> Option<&str> {
    let url = package.registry_base_url.as_deref()?.trim();
    let bare = url.trim_end_matches('/');
    (!bare.is_empty() && !DEFAULT_REGISTRIES.contains(&bare)).then_some(url)
}

fn package_fields(package: &Package, kind: &str, draft: &mut RawDraft) -> Result<(), CoreError> {
    let identifier = package
        .identifier
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| CoreError::new(ErrorCode::MissingField, "the package has no identifier"))?;
    let version = package
        .version
        .as_deref()
        .filter(|v| !v.is_empty() && *v != "latest");
    let registry = custom_registry(package);
    let inputs = &mut draft.inputs;
    let mut env = BTreeMap::new();
    for variable in &package.environment_variables {
        if let Some(name) = &variable.name {
            let value = input_value(variable, name, &format!("env.{name}"), inputs);
            env.insert(name.clone(), value);
        }
    }
    let mut args = Vec::new();
    let mut pinned = version.is_some();
    let command = match kind {
        "npm" => {
            if package.runtime_arguments.is_empty() {
                args.push("-y".to_owned());
            }
            if let Some(registry) = registry {
                args.push(format!("--registry={}", escape(registry)));
            }
            arguments(&package.runtime_arguments, &mut args, inputs);
            args.push(escape(&match version {
                Some(version) => format!("{identifier}@{version}"),
                None => identifier.to_owned(),
            }));
            package
                .runtime_hint
                .clone()
                .unwrap_or_else(|| "npx".to_owned())
        }
        "pypi" => {
            if let Some(registry) = registry {
                args.push(format!("--index-url={}", escape(registry)));
            }
            arguments(&package.runtime_arguments, &mut args, inputs);
            args.push(escape(&match version {
                Some(version) => format!("{identifier}=={version}"),
                None => identifier.to_owned(),
            }));
            package
                .runtime_hint
                .clone()
                .unwrap_or_else(|| "uvx".to_owned())
        }
        "nuget" => {
            arguments(&package.runtime_arguments, &mut args, inputs);
            args.push(escape(&match version {
                Some(version) => format!("{identifier}@{version}"),
                None => identifier.to_owned(),
            }));
            args.push("--yes".to_owned());
            if let Some(registry) = registry {
                args.extend(["--source".to_owned(), escape(registry)]);
            }
            package
                .runtime_hint
                .clone()
                .unwrap_or_else(|| "dnx".to_owned())
        }
        "oci" | "docker" => {
            args.extend(["run", "-i", "--rm"].map(str::to_owned));
            arguments(&package.runtime_arguments, &mut args, inputs);
            for name in env.keys() {
                let passed = args.windows(2).any(|pair| {
                    pair[0] == "-e"
                        && (pair[1] == *name || pair[1].starts_with(&format!("{name}=")))
                });
                if !passed {
                    args.extend(["-e".to_owned(), name.clone()]);
                }
            }
            let last = identifier.rsplit('/').next().unwrap_or(identifier);
            let digest = identifier.contains('@');
            let tagged = last.contains(':') || digest;
            pinned = (tagged && !last.ends_with(":latest")) || version.is_some();
            // A registry host named apart from the identifier goes in
            // front of it, as `docker` reads image names.
            let image = match registry {
                Some(registry)
                    if !identifier
                        .split('/')
                        .next()
                        .is_some_and(|h| h.contains('.')) =>
                {
                    let host = registry
                        .trim_start_matches("https://")
                        .trim_start_matches("http://")
                        .trim_end_matches('/');
                    format!("{host}/{identifier}")
                }
                _ => identifier.to_owned(),
            };
            args.push(escape(&match version {
                Some(version) if !tagged => format!("{image}:{version}"),
                _ => image,
            }));
            package
                .runtime_hint
                .clone()
                .unwrap_or_else(|| "docker".to_owned())
        }
        other => {
            return Err(CoreError::new(
                ErrorCode::UnsupportedField,
                format!("{other} packages are not supported; add the server by hand"),
            ));
        }
    };
    draft.pinned = pinned;
    arguments(&package.package_arguments, &mut args, inputs);
    let transport = package.transport.as_ref();
    match transport.and_then(|t| t.kind.as_deref()).unwrap_or("stdio") {
        "stdio" => {
            draft.fields.transport = Some(Transport::Stdio);
            draft.fields.command = Some(escape(&command));
            draft.fields.args = (!args.is_empty()).then_some(args);
            draft.fields.env = (!env.is_empty()).then_some(env);
        }
        other => {
            // The package serves HTTP itself: the draft connects, and
            // starting it is a setup step.
            let transport = transport.expect("a transport kind comes from a transport");
            let url = transport.url.clone().ok_or_else(|| {
                CoreError::new(
                    ErrorCode::MissingField,
                    format!("the {other} package transport has no URL"),
                )
            })?;
            draft.fields.transport = remote_transport(Some(other));
            draft.fields.url = Some(fill(&url, &transport.variables, "url", inputs));
            draft.fields.headers = header_map(&transport.headers, inputs);
            let mut line = vec![command];
            line.extend(args);
            draft.setup.push(line.join(" "));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ bundle

fn is_bundle(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes).is_ok_and(|value| {
        (value.get("manifest_version").is_some() || value.get("dxt_version").is_some())
            && value
                .get("server")
                .is_some_and(serde_json::Value::is_object)
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Manifest {
    name: Option<String>,
    display_name: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
    server: BundleServer,
    user_config: BTreeMap<String, UserConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BundleServer {
    #[serde(rename = "type")]
    kind: Option<String>,
    entry_point: Option<String>,
    mcp_config: Option<McpConfig>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
struct McpConfig {
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
    platform_overrides: BTreeMap<String, McpConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct UserConfig {
    title: Option<String>,
    description: Option<String>,
    sensitive: bool,
    required: bool,
    default: Option<serde_json::Value>,
}

/// One manifest string with its substitutions: `${__dirname}` is the
/// folder, `${user_config.KEY}` the user's answer (a secret, the default
/// or an environment reference), `${HOME}` and the known folders the home
/// folder; anything else with `${` is kept literal.
fn bundle_text(
    text: &str,
    root: &str,
    config: &BTreeMap<String, UserConfig>,
    field: &str,
    inputs: &mut Vec<DraftInput>,
) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&escape(&rest[..start]));
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&escape(&rest[start..]));
            rest = "";
            break;
        };
        let name = &after[..end];
        let replaced = match name {
            "__dirname" => Some(root.to_owned()),
            "HOME" => Some("${home}".to_owned()),
            "DESKTOP" => Some("${home}/Desktop".to_owned()),
            "DOCUMENTS" => Some("${home}/Documents".to_owned()),
            "DOWNLOADS" => Some("${home}/Downloads".to_owned()),
            "pathSeparator" | "/" => Some("/".to_owned()),
            _ => name.strip_prefix("user_config.").map(|key| {
                let entry = config.get(key);
                let sensitive = entry.is_some_and(|e| e.sensitive);
                let description = entry.and_then(|e| e.description.clone().or(e.title.clone()));
                let required = entry.is_some_and(|e| e.required);
                if sensitive {
                    inputs.push(DraftInput {
                        field: field.to_owned(),
                        description,
                        secret: true,
                        required,
                        alias: Some(env_name(key)),
                        value_dropped: false,
                    });
                    return secret_ref(&env_name(key));
                }
                match entry.and_then(|e| e.default.as_ref()).and_then(json_text) {
                    // A default may name known folders too (no user_config
                    // inside a default).
                    Some(default) => bundle_text(&default, root, &BTreeMap::new(), field, inputs),
                    None => {
                        inputs.push(DraftInput {
                            field: field.to_owned(),
                            description,
                            secret: false,
                            required,
                            alias: None,
                            value_dropped: false,
                        });
                        env_ref(key)
                    }
                }
            }),
        };
        match replaced {
            Some(text) => out.push_str(&text),
            None => out.push_str(&escape(&rest[start..start + 3 + end])),
        }
        rest = &after[end + 1..];
    }
    out.push_str(&escape(rest));
    out
}

/// The fields of one `mcp_config` (or override); input fields are named
/// `<prefix>command`, `<prefix>args[N]`, `<prefix>env.NAME`.
fn bundle_fields(
    mcp: &McpConfig,
    root: &str,
    config: &BTreeMap<String, UserConfig>,
    prefix: &str,
    inputs: &mut Vec<DraftInput>,
) -> ServerFields {
    let command = mcp
        .command
        .as_deref()
        .map(|command| bundle_text(command, root, config, &format!("{prefix}command"), inputs));
    let args = mcp.args.as_ref().map(|args| {
        args.iter()
            .enumerate()
            .map(|(index, arg)| {
                let field = format!("{prefix}args[{}]", index + 1);
                bundle_text(arg, root, config, &field, inputs)
            })
            .collect::<Vec<_>>()
    });
    let env = mcp.env.as_ref().map(|env| {
        env.iter()
            .map(|(name, value)| {
                let field = format!("{prefix}env.{name}");
                (
                    name.clone(),
                    bundle_text(value, root, config, &field, inputs),
                )
            })
            .collect::<BTreeMap<_, _>>()
    });
    ServerFields {
        transport: command.is_some().then_some(Transport::Stdio),
        command,
        args: args.filter(|args| !args.is_empty()),
        env: env.filter(|env| !env.is_empty()),
        ..ServerFields::default()
    }
}

fn bundle(ctx: &Ctx<'_>, bytes: &[u8]) -> Result<RawDraft, Skipped> {
    const FILE: &str = "manifest.json";
    let manifest: Manifest = serde_json::from_slice(bytes).map_err(|err| json_error(FILE, &err))?;
    let root = "${__dirname}";
    let entry = manifest.server.entry_point.clone();
    let mcp = match &manifest.server.mcp_config {
        Some(mcp) => mcp.clone(),
        None => {
            let entry = entry.clone().ok_or_else(|| {
                skip(
                    FILE,
                    None,
                    CoreError::new(
                        ErrorCode::MissingField,
                        "the manifest has neither server.mcp_config nor server.entry_point",
                    ),
                )
            })?;
            let (command, args) = match manifest.server.kind.as_deref().unwrap_or("node") {
                "node" => ("node".to_owned(), vec![format!("{root}/{entry}")]),
                "python" => ("python".to_owned(), vec![format!("{root}/{entry}")]),
                "uv" => (
                    "uv".to_owned(),
                    vec![
                        "run".to_owned(),
                        "--directory".to_owned(),
                        root.to_owned(),
                        entry,
                    ],
                ),
                _ => (format!("{root}/{entry}"), Vec::new()),
            };
            McpConfig {
                command: Some(command),
                args: Some(args),
                ..McpConfig::default()
            }
        }
    };
    let name = manifest.name.clone().unwrap_or_else(|| ctx.name.clone());
    let mut draft = RawDraft::new(DraftKind::Bundle, FILE, "bundle", name);
    draft.display_name = manifest.display_name.clone();
    draft.description = manifest.description.clone();
    draft.homepage = manifest.homepage.clone();
    draft.fields = bundle_fields(
        &mcp,
        &ctx.root,
        &manifest.user_config,
        "",
        &mut draft.inputs,
    );
    if draft.fields.command.is_none() {
        return Err(skip(
            FILE,
            None,
            CoreError::new(
                ErrorCode::MissingField,
                "the manifest's mcp_config has no command",
            ),
        ));
    }
    let mut os = BTreeMap::new();
    for (platform, over) in &mcp.platform_overrides {
        let name = match platform.as_str() {
            "win32" => Os::Windows.as_str(),
            "darwin" => Os::MacOs.as_str(),
            "linux" => Os::Linux.as_str(),
            _ => continue,
        };
        let prefix = format!("os.{name}.");
        let mut fields = bundle_fields(
            over,
            &ctx.root,
            &manifest.user_config,
            &prefix,
            &mut draft.inputs,
        );
        fields.transport = None;
        os.insert(
            name.to_owned(),
            ServerOverride {
                fields,
                extra: toml::Table::new(),
            },
        );
    }
    draft.os = (!os.is_empty()).then_some(os);
    draft.runs_repo_code = uses_root(&draft, &ctx.root);
    Ok(draft)
}

/// Whether any launch field of `draft` (or of an `[os.*]` override) names
/// the folder.
pub(crate) fn uses_root(draft: &RawDraft, root: &str) -> bool {
    let fields = std::iter::once(&draft.fields)
        .chain(draft.os.iter().flatten().map(|(_, over)| &over.fields));
    fields
        .into_iter()
        .any(|fields| fields_mention(fields, root))
}

pub(crate) fn fields_mention(fields: &ServerFields, text: &str) -> bool {
    fields
        .command
        .iter()
        .chain(fields.cwd.iter())
        .any(|value| value.contains(text))
        || fields
            .args
            .iter()
            .flatten()
            .any(|value| value.contains(text))
        || fields
            .env
            .iter()
            .flatten()
            .any(|(_, value)| value.contains(text))
}

// ----------------------------------------------------------- client config

/// Whether a passthrough value holds a literal the import classifier
/// flags (by its key, or by its shape).
fn flagged(key: &str, value: &toml::Value) -> bool {
    match value {
        toml::Value::String(text) => classify_import(Some(key), text).is_some(),
        toml::Value::Array(items) => items.iter().any(|item| flagged(key, item)),
        toml::Value::Table(table) => table.iter().any(|(key, value)| flagged(key, value)),
        _ => false,
    }
}

/// Drops the passthrough values that hold a flagged literal (never
/// stored), each listed as an input whose value was not kept (`field`:
/// `client.<id>.<key>...`). A table emptied this way is removed.
fn scrub_passthrough(table: &mut toml::Table, path: &str, inputs: &mut Vec<DraftInput>) {
    let keys: Vec<String> = table.keys().cloned().collect();
    for key in keys {
        let field = format!("{path}.{key}");
        match table.get_mut(&key) {
            Some(toml::Value::Table(inner)) => {
                let before = inputs.len();
                scrub_passthrough(inner, &field, inputs);
                if inner.is_empty() && inputs.len() > before {
                    table.remove(&key);
                }
            }
            Some(value) if flagged(&key, value) => {
                table.remove(&key);
                inputs.push(DraftInput {
                    field,
                    description: None,
                    secret: true,
                    required: false,
                    alias: None,
                    value_dropped: true,
                });
            }
            _ => {}
        }
    }
}

fn client_config(
    ctx: &Ctx<'_>,
    file: &str,
    hint: Option<&str>,
    bytes: &[u8],
) -> Result<(Vec<RawDraft>, Vec<Skipped>), Skipped> {
    let text = String::from_utf8_lossy(bytes);
    let format = if file.ends_with(".toml") {
        Format::Toml
    } else {
        Format::Jsonc
    };
    let doc = Document::parse(text.as_bytes(), format).map_err(|_| parse_error(file, None))?;
    let hint = hint.and_then(adapter);
    let found = snippet::entries(&doc, ctx.os, hint).map_err(|err| skip(file, None, err))?;
    let spec = found.spec;
    let converter = format!("client:{}", spec.id);
    let mut drafts = Vec::new();
    let mut skipped = Vec::new();
    for (name, value) in &found.entries {
        let def = match read_native(spec, name, value) {
            Ok(def) => def,
            Err(err) => {
                skipped.push(skip(file, Some(name.clone()), err));
                continue;
            }
        };
        let mut draft = RawDraft::new(DraftKind::ClientConfig, file, &converter, name.clone());
        draft.entry = Some(name.clone());
        draft.fields = def.fields;
        draft.tools = def.tools;
        let mut passthrough = def.passthrough;
        scrub_passthrough(
            &mut passthrough,
            &format!("client.{}", spec.id),
            &mut draft.inputs,
        );
        draft.passthrough = (!passthrough.is_empty()).then(|| (spec.id.to_owned(), passthrough));
        in_folder(ctx, &mut draft.fields);
        draft.runs_repo_code = fields_mention(&draft.fields, &ctx.root);
        drafts.push(draft);
    }
    Ok((drafts, skipped))
}

/// A committed config runs in its project: `${project}` is the folder, and
/// a relative path that exists in the folder (or below a dependency or
/// build folder left out of the read) makes the folder the working
/// directory (as the client would run it).
fn in_folder(ctx: &Ctx<'_>, fields: &mut ServerFields) {
    let root = ctx.root.as_str();
    let replace = |value: &mut String| {
        if value.contains("${project}") {
            *value = value.replace("${project}", root);
        }
    };
    fields.command.iter_mut().for_each(replace);
    fields.cwd.iter_mut().for_each(replace);
    fields.args.iter_mut().flatten().for_each(replace);
    fields
        .env
        .iter_mut()
        .flatten()
        .for_each(|(_, value)| replace(value));
    let relative_in_folder = |value: &str| {
        let value = value.trim_start_matches("./");
        !value.is_empty()
            && !value.starts_with(['-', '/', '\\', '$'])
            && !value.contains(':')
            && (value.contains('/') || value.contains('.'))
            && (ctx.files.contains_key(value)
                || ctx
                    .files
                    .keys()
                    .any(|path| path.starts_with(&format!("{value}/")))
                || ctx
                    .skipped
                    .iter()
                    .any(|dir| value == dir || value.starts_with(&format!("{dir}/"))))
    };
    let uses_relative = fields.command.as_deref().is_some_and(relative_in_folder)
        || fields
            .args
            .iter()
            .flatten()
            .any(|arg| relative_in_folder(arg));
    match &mut fields.cwd {
        Some(cwd) if !cwd.contains(root) && (cwd == "." || relative_in_folder(cwd)) => {
            let rest = cwd.trim_start_matches("./").trim_start_matches('.');
            *cwd = if rest.is_empty() {
                root.to_owned()
            } else {
                format!("{root}/{}", rest.trim_start_matches('/'))
            };
        }
        None if uses_relative => fields.cwd = Some(root.to_owned()),
        _ => {}
    }
}

// ----------------------------------------------------------------- package

/// `owner/repo` of a GitHub URL.
fn github_slug(url: &str) -> Option<String> {
    if !is_github_url(url) {
        return None;
    }
    let rest = url
        .split_once("github.com/")
        .or_else(|| url.split_once("github.com:"))?
        .1;
    let mut parts = rest.trim_end_matches('/').split('/');
    let owner = parts.next().filter(|p| !p.is_empty())?;
    let repo = parts
        .next()
        .filter(|p| !p.is_empty())?
        .trim_end_matches(".git");
    Some(format!("{owner}/{repo}"))
}

/// `git+<url>` as npm and uv read it: scp-style becomes `ssh://`, a local
/// repository `file:///...`.
fn git_plus(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if ["https://", "http://", "ssh://", "git://", "file://"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
    {
        return format!("git+{url}");
    }
    if let Some((user_host, path)) = url.split_once(':')
        && user_host.contains('@')
        && !user_host.contains(['/', '\\'])
    {
        return format!("git+ssh://{user_host}/{}", path.trim_start_matches('/'));
    }
    match SourceRef::from_git_url(url) {
        Ok(source) if source.is_local() => format!("git+{}", source.key()),
        _ => format!("git+{url}"),
    }
}

fn short(text: &str) -> &str {
    &text[..text.len().min(12)]
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PackageJson {
    name: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
    bin: Option<serde_json::Value>,
    dependencies: BTreeMap<String, serde_json::Value>,
}

fn npm(ctx: &Ctx<'_>, bytes: &[u8]) -> Result<(Vec<RawDraft>, Vec<Skipped>), Skipped> {
    const FILE: &str = "package.json";
    let package: PackageJson =
        serde_json::from_slice(bytes).map_err(|err| json_error(FILE, &err))?;
    let unscoped = package
        .name
        .as_deref()
        .map(|name| name.rsplit('/').next().unwrap_or(name).to_owned())
        .unwrap_or_else(|| ctx.name.clone());
    let bins: Vec<(String, String)> = match &package.bin {
        Some(serde_json::Value::String(path)) => vec![(unscoped.clone(), path.clone())],
        Some(serde_json::Value::Object(map)) => map
            .iter()
            .filter_map(|(name, path)| Some((name.clone(), path.as_str()?.to_owned())))
            .collect(),
        _ => Vec::new(),
    };
    let Some((bin, bin_path)) = bins
        .iter()
        .find(|(name, _)| *name == unscoped)
        .or_else(|| bins.first())
        .cloned()
    else {
        return Ok((Vec::new(), Vec::new()));
    };
    let bin_path = bin_path.trim_start_matches("./").to_owned();
    let mut draft = RawDraft::new(DraftKind::Package, FILE, "npm", bin.clone());
    draft.entry = Some(format!("bin.{bin}"));
    draft.description = package.description.clone();
    draft.homepage = package.homepage.clone();
    draft.runs_repo_code = true;
    let mut fields = ServerFields {
        transport: Some(Transport::Stdio),
        ..ServerFields::default()
    };
    let remote_spec =
        ctx.git
            .filter(|pin| pin.path.is_none())
            .map(|pin| match github_slug(&pin.url) {
                Some(slug) => format!("github:{slug}#{}", pin.commit),
                None => format!("{}#{}", git_plus(&pin.url), pin.commit),
            });
    match remote_spec {
        Some(spec) => {
            fields.command = Some("npx".to_owned());
            let mut args = vec!["-y".to_owned()];
            if bins.len() > 1 {
                args.extend(["-p".to_owned(), escape(&spec), escape(&bin)]);
            } else {
                args.push(escape(&spec));
            }
            fields.args = Some(args);
        }
        None => {
            fields.command = Some("node".to_owned());
            fields.args = Some(vec![format!("{}/{}", ctx.root, escape(&bin_path))]);
            if !package.dependencies.is_empty() {
                draft
                    .setup
                    .push(format!("npm install --prefix {}", ctx.root));
            }
            let built = ctx
                .skipped
                .iter()
                .any(|dir| bin_path.starts_with(&format!("{dir}/")));
            if !ctx.files.contains_key(&bin_path) && !built {
                draft.setup.push(format!(
                    "npm run build --prefix {} ({bin_path} is built)",
                    ctx.root
                ));
            }
        }
    }
    draft.fields = fields;
    Ok((vec![draft], Vec::new()))
}

fn python(ctx: &Ctx<'_>, bytes: &[u8]) -> Result<(Vec<RawDraft>, Vec<Skipped>), Skipped> {
    const FILE: &str = "pyproject.toml";
    let text = String::from_utf8_lossy(bytes);
    let doc: toml::Table = text
        .parse()
        .map_err(|err: toml::de::Error| toml_error(FILE, &text, &err))?;
    let project = doc.get("project").and_then(toml::Value::as_table);
    let description = project
        .and_then(|p| p.get("description"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned);
    let scripts: Vec<String> = project
        .and_then(|p| p.get("scripts"))
        .and_then(toml::Value::as_table)
        .map(|scripts| scripts.keys().cloned().collect())
        .unwrap_or_default();
    let drafts = scripts
        .into_iter()
        .map(|script| {
            let mut draft = RawDraft::new(DraftKind::Package, FILE, "python", script.clone());
            draft.entry = Some(format!("project.scripts.{script}"));
            draft.description.clone_from(&description);
            draft.runs_repo_code = true;
            let args = match ctx.git {
                Some(pin) => {
                    let mut from = format!("{}@{}", git_plus(&pin.url), pin.commit);
                    if let Some(path) = &pin.path {
                        from.push_str(&format!("#subdirectory={path}"));
                    }
                    vec!["--from".to_owned(), escape(&from), escape(&script)]
                }
                None => vec![
                    "run".to_owned(),
                    "--directory".to_owned(),
                    ctx.root.clone(),
                    escape(&script),
                ],
            };
            draft.fields = ServerFields {
                transport: Some(Transport::Stdio),
                command: Some(if ctx.git.is_some() { "uvx" } else { "uv" }.to_owned()),
                args: Some(args),
                ..ServerFields::default()
            };
            draft
        })
        .collect();
    Ok((drafts, Vec::new()))
}

/// The repository as an image built on this machine: tagged
/// `loadout-local/<id>:<commit or content>`, a name no registry serves,
/// and run with `--pull=never`, so a missing image fails instead of being
/// pulled from someone else's namespace.
fn docker(ctx: &Ctx<'_>) -> RawDraft {
    const FILE: &str = "Dockerfile";
    let version = match ctx.git {
        Some(pin) => short(&pin.commit).to_owned(),
        None => short(ctx.content_hash.trim_start_matches("sha256:")).to_owned(),
    };
    let tag = format!("{IMAGE_PREFIX}{ID_TOKEN}:{version}");
    let context = match ctx.git {
        Some(pin) => {
            let mut context = format!("{}#{}", pin.url, pin.commit);
            if let Some(path) = &pin.path {
                context.push_str(&format!(":{path}"));
            }
            context
        }
        None => ctx.root.clone(),
    };
    let mut draft = RawDraft::new(DraftKind::Package, FILE, "docker", ctx.name.clone());
    draft.runs_repo_code = true;
    draft.setup.push(format!("docker build -t {tag} {context}"));
    draft.fields = ServerFields {
        transport: Some(Transport::Stdio),
        command: Some("docker".to_owned()),
        args: Some(vec![
            "run".to_owned(),
            "-i".to_owned(),
            "--rm".to_owned(),
            "--pull=never".to_owned(),
            tag,
        ]),
        ..ServerFields::default()
    };
    draft
}

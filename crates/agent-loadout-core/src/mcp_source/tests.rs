//! Converter units (#5): each file kind as raw drafts, without git.

use super::convert::{self, CHECKOUT_TOKEN, Ctx, GitPin, ID_TOKEN, IMAGE_PREFIX};
use super::*;
use crate::env::Os;
use crate::model::server::Transport;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn files(entries: &[(&str, &str)]) -> SkillFiles {
    entries
        .iter()
        .map(|(path, text)| ((*path).to_owned(), text.as_bytes().to_vec()))
        .collect()
}

fn local<'a>(files: &'a SkillFiles) -> Ctx<'a> {
    Ctx {
        files,
        skipped: &[],
        root: "${home}/code/weather".to_owned(),
        git: None,
        os: Os::Linux,
        name: "weather".to_owned(),
        content_hash: crate::skills::content_hash(files),
        plugin_servers: None,
    }
}

fn git<'a>(files: &'a SkillFiles, pin: &'a GitPin) -> Ctx<'a> {
    Ctx {
        files,
        skipped: &[],
        root: CHECKOUT_TOKEN.to_owned(),
        git: Some(pin),
        os: Os::Linux,
        name: "weather".to_owned(),
        content_hash: crate::skills::content_hash(files),
        plugin_servers: None,
    }
}

fn pin(url: &str, path: Option<&str>) -> GitPin {
    GitPin {
        url: url.to_owned(),
        commit: SHA.to_owned(),
        path: path.map(str::to_owned),
    }
}

const SERVER_JSON: &str = r#"{
  "$schema": "https://static.modelcontextprotocol.io/schemas/2025-09-29/server.schema.json",
  "name": "io.github.acme/weather",
  "title": "Weather",
  "description": "Forecasts",
  "version": "1.2.0",
  "packages": [
    {
      "registryType": "npm",
      "identifier": "@acme/weather-mcp",
      "version": "1.2.0",
      "transport": { "type": "stdio" },
      "packageArguments": [
        { "type": "named", "name": "--units", "default": "metric" }
      ],
      "environmentVariables": [
        { "name": "WEATHER_API_KEY", "description": "API key", "isRequired": true, "isSecret": true },
        { "name": "LOG_LEVEL", "default": "info" },
        { "name": "REGION", "isRequired": true }
      ]
    },
    {
      "registry_type": "pypi",
      "identifier": "acme-weather",
      "version": "1.2.0",
      "environment_variables": [{ "name": "WEATHER_API_KEY", "is_secret": true }]
    },
    { "registryType": "oci", "identifier": "ghcr.io/acme/weather", "version": "1.2.0",
      "environmentVariables": [{ "name": "WEATHER_API_KEY", "isSecret": true }] },
    { "registryType": "mcpb", "identifier": "https://example.com/weather.mcpb" }
  ],
  "remotes": [
    {
      "type": "streamable-http",
      "url": "https://mcp.acme.dev/weather",
      "headers": [
        { "name": "Authorization", "value": "Bearer {token}", "isRequired": true,
          "variables": { "token": { "isSecret": true, "description": "Token" } } }
      ]
    }
  ]
}"#;

#[test]
fn server_json_packages_and_remotes_become_drafts() {
    let files = files(&[("server.json", SERVER_JSON)]);
    let (drafts, skipped) = convert::all(&local(&files));
    let names: Vec<&str> = drafts.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "weather-npm",
            "weather-pypi",
            "weather-oci",
            "weather-remote"
        ]
    );
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert_eq!(skipped[0].entry.as_deref(), Some("packages[3]"));
    assert_eq!(skipped[0].error.code(), ErrorCode::UnsupportedField);

    let npm = &drafts[0];
    assert_eq!(npm.kind, DraftKind::Registry);
    assert_eq!(npm.fields.command.as_deref(), Some("npx"));
    assert_eq!(
        npm.fields.args.as_deref().unwrap(),
        ["-y", "@acme/weather-mcp@1.2.0", "--units", "metric"]
    );
    let env = npm.fields.env.as_ref().unwrap();
    assert_eq!(env["WEATHER_API_KEY"], "${secret:WEATHER_API_KEY}");
    assert_eq!(env["LOG_LEVEL"], "info");
    assert_eq!(env["REGION"], "${env:REGION}");
    assert!(
        npm.inputs
            .iter()
            .any(|i| i.secret && i.field == "env.WEATHER_API_KEY" && i.required)
    );
    assert!(
        npm.inputs
            .iter()
            .any(|i| !i.secret && i.field == "env.REGION")
    );
    assert_eq!(npm.display_name.as_deref(), Some("Weather"));

    assert_eq!(drafts[1].fields.command.as_deref(), Some("uvx"));
    assert_eq!(
        drafts[1].fields.args.as_deref().unwrap(),
        ["acme-weather==1.2.0"]
    );
    assert_eq!(
        drafts[2].fields.args.as_deref().unwrap(),
        [
            "run",
            "-i",
            "--rm",
            "-e",
            "WEATHER_API_KEY",
            "ghcr.io/acme/weather:1.2.0"
        ]
    );

    let remote = &drafts[3];
    assert_eq!(remote.fields.transport, Some(Transport::Http));
    assert_eq!(
        remote.fields.url.as_deref(),
        Some("https://mcp.acme.dev/weather")
    );
    assert_eq!(
        remote.fields.headers.as_ref().unwrap()["Authorization"],
        "Bearer ${secret:TOKEN}"
    );
}

const MANIFEST: &str = r#"{
  "manifest_version": "0.2",
  "name": "weather",
  "display_name": "Weather",
  "server": {
    "type": "node",
    "entry_point": "server/index.js",
    "mcp_config": {
      "command": "node",
      "args": ["${__dirname}/server/index.js", "--dir", "${user_config.folder}", "${HOME}/x"],
      "env": { "API_KEY": "${user_config.api_key}", "PLAIN": "a ${weird} value" },
      "platform_overrides": { "win32": { "command": "node.exe" } }
    }
  },
  "user_config": {
    "api_key": { "type": "string", "title": "API key", "sensitive": true, "required": true },
    "folder": { "type": "directory", "default": "/tmp" }
  }
}"#;

#[test]
fn a_bundle_manifest_runs_from_its_folder() {
    let files = files(&[("manifest.json", MANIFEST), ("server/index.js", "x")]);
    let pin = pin("https://github.com/acme/weather", None);
    let (drafts, skipped) = convert::all(&git(&files, &pin));
    assert!(skipped.is_empty(), "{skipped:?}");
    let draft = &drafts[0];
    assert_eq!(draft.kind, DraftKind::Bundle);
    assert!(draft.runs_repo_code);
    assert!(convert::uses_root(draft, CHECKOUT_TOKEN));
    let args = draft.fields.args.as_deref().unwrap();
    assert_eq!(args[0], format!("{CHECKOUT_TOKEN}/server/index.js"));
    assert_eq!(&args[1..], ["--dir", "/tmp", "${home}/x"]);
    let env = draft.fields.env.as_ref().unwrap();
    assert_eq!(env["API_KEY"], "${secret:API_KEY}");
    assert_eq!(env["PLAIN"], "a $${weird} value");
    let os = draft.os.as_ref().unwrap();
    assert_eq!(os["windows"].fields.command.as_deref(), Some("node.exe"));
}

#[test]
fn committed_configs_run_in_their_folder() {
    let files = files(&[
        (
            ".mcp.json",
            r#"{ "mcpServers": { "Weather": { "command": "node", "args": ["dist/index.js"],
                 "env": { "API_KEY": "sk-live-1234567890abcdef1234567890" } } } }"#,
        ),
        (
            ".cursor/mcp.json",
            r#"{ "mcpServers": { "fetch": { "command": "uvx", "args": ["mcp-server-fetch"] } } }"#,
        ),
        (
            ".vscode/mcp.json",
            r#"{ "servers": { "fetch": { "command": "uvx", "args": ["mcp-server-fetch"] },
                 "prompted": { "command": "x", "env": { "K": "${input:key}" } } } }"#,
        ),
        ("dist/index.js", "console.log(1)"),
    ]);
    let (drafts, skipped) = convert::all(&local(&files));
    let names: Vec<(&str, &str)> = drafts
        .iter()
        .map(|d| (d.name.as_str(), d.converter.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("Weather", "client:claude-code"),
            ("fetch", "client:vscode-legacy"),
            ("fetch", "client:cursor")
        ]
    );
    let weather = &drafts[0];
    assert_eq!(weather.fields.cwd.as_deref(), Some("${home}/code/weather"));
    assert!(weather.runs_repo_code);
    // The literal stays for the resolver's extraction.
    assert!(weather.fields.env.as_ref().unwrap()["API_KEY"].starts_with("sk-"));
    assert!(!drafts[1].runs_repo_code);
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert_eq!(skipped[0].error.code(), ErrorCode::NeedsManualSecret);
}

#[test]
fn a_repository_is_a_package_only_without_configs() {
    let package = r#"{ "name": "@acme/weather-mcp", "bin": { "weather-mcp": "dist/index.js" },
                       "dependencies": { "x": "1" } }"#;
    let files_only = files(&[("package.json", package)]);
    let github = pin("https://github.com/Acme/Weather.git", None);
    let (drafts, _) = convert::all(&git(&files_only, &github));
    let npm = &drafts[0];
    assert_eq!(npm.kind, DraftKind::Package);
    assert!(npm.runs_repo_code);
    assert_eq!(npm.fields.command.as_deref(), Some("npx"));
    assert_eq!(
        npm.fields.args.as_deref().unwrap(),
        ["-y".to_owned(), format!("github:Acme/Weather#{SHA}")]
    );

    let other = pin("https://git.acme.corp/team/weather.git", None);
    let (drafts, _) = convert::all(&git(&files_only, &other));
    assert_eq!(
        drafts[0].fields.args.as_deref().unwrap()[1],
        format!("git+https://git.acme.corp/team/weather.git#{SHA}")
    );

    let (drafts, _) = convert::all(&local(&files_only));
    assert_eq!(drafts[0].fields.command.as_deref(), Some("node"));
    assert_eq!(
        drafts[0].fields.args.as_deref().unwrap(),
        ["${home}/code/weather/dist/index.js"]
    );
    assert_eq!(drafts[0].setup.len(), 2, "{:?}", drafts[0].setup);

    let with_config = files(&[
        ("package.json", package),
        (
            ".mcp.json",
            r#"{ "mcpServers": { "w": { "command": "npx", "args": ["-y", "w"] } } }"#,
        ),
    ]);
    let (drafts, _) = convert::all(&local(&with_config));
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].kind, DraftKind::ClientConfig);
}

#[test]
fn python_and_docker_packages_are_pinned() {
    let files = files(&[
        (
            "pyproject.toml",
            "[project]\nname = \"weather\"\n[project.scripts]\nweather-mcp = \"weather:main\"\n",
        ),
        ("Dockerfile", "FROM python:3.12\n"),
    ]);
    let pin = pin("git@github.com:acme/weather.git", Some("servers/weather"));
    let (drafts, _) = convert::all(&git(&files, &pin));
    assert_eq!(drafts[0].fields.command.as_deref(), Some("uvx"));
    assert_eq!(
        drafts[0].fields.args.as_deref().unwrap(),
        [
            "--from".to_owned(),
            format!("git+ssh://git@github.com/acme/weather.git@{SHA}#subdirectory=servers/weather"),
            "weather-mcp".to_owned()
        ]
    );
    let docker = &drafts[1];
    assert_eq!(docker.converter, "docker");
    assert_eq!(
        docker.fields.args.as_deref().unwrap(),
        [
            "run".to_owned(),
            "-i".to_owned(),
            "--rm".to_owned(),
            "--pull=never".to_owned(),
            format!("{IMAGE_PREFIX}{ID_TOKEN}:0123456789ab")
        ]
    );
    assert_eq!(
        docker.setup,
        [format!(
            "docker build -t {IMAGE_PREFIX}{ID_TOKEN}:0123456789ab git@github.com:acme/weather.git#{SHA}:servers/weather"
        )]
    );

    let (drafts, _) = convert::all(&local(&files));
    assert_eq!(
        drafts[0].fields.args.as_deref().unwrap(),
        ["run", "--directory", "${home}/code/weather", "weather-mcp"]
    );
}

#[test]
fn ids_in_paths_and_image_tags_follow_a_renamed_id_by_whole_name() {
    let mut server = Server::new("weather");
    server.body.command = Some("node".to_owned());
    server.body.args = Some(vec![
        "${loadout_home}/servers/weather/index.js".to_owned(),
        "${loadout_home}/servers/weatherly/x".to_owned(),
        "loadout-local/weather:0123".to_owned(),
        "loadout-local/weatherly:0123".to_owned(),
    ]);
    server.body.cwd = Some("${loadout_home}/servers/weather".to_owned());
    rename_id(&mut server, "weather", "wx");
    assert_eq!(
        server.body.args.unwrap(),
        [
            "${loadout_home}/servers/wx/index.js",
            "${loadout_home}/servers/weatherly/x",
            "loadout-local/wx:0123",
            "loadout-local/weatherly:0123"
        ]
    );
    assert_eq!(
        server.body.cwd.as_deref(),
        Some("${loadout_home}/servers/wx")
    );
}

#[test]
fn parse_errors_say_where_never_what() {
    // A file holding a secret where the schema wants an object: the skip
    // names the file and line, never the value (#10).
    let files = files(&[
        (
            "server.json",
            "{\n  \"name\": \"x\",\n  \"remotes\": [\n    { \"type\": \"sse\", \"url\": \"https://a.example\",\n      \"headers\": [\"Authorization: Bearer sk-live-abcdefghijklmnopqrstuvwxyz0123\"] }\n  ]\n}\n",
        ),
        (
            "pyproject.toml",
            "[project]\nname = \"x\"\ntoken = \"sk-live-abcdefghijklmnopqrstuvwxyz0123\" broken\n",
        ),
    ]);
    let (_, skipped) = convert::all(&local(&files));
    let messages: Vec<String> = skipped
        .iter()
        .map(|s| s.error.message().to_owned())
        .collect();
    assert_eq!(
        messages,
        [
            "could not parse server.json (line 5)",
            "could not parse pyproject.toml (line 3)"
        ],
        "{skipped:?}"
    );
}

#[test]
fn registry_packages_without_a_version_are_flagged_and_registries_honored() {
    let files = files(&[(
        "server.json",
        r#"{ "name": "acme/tools", "packages": [
            { "registryType": "npm", "identifier": "@acme/tools",
              "registryBaseUrl": "https://npm.pkg.github.com" },
            { "registryType": "pypi", "identifier": "acme-tools", "version": "2.0.0",
              "registryBaseUrl": "https://pypi.acme.example/simple" },
            { "registryType": "npm", "identifier": "@acme/pinned", "version": "1.0.0",
              "registryBaseUrl": "https://registry.npmjs.org/" },
            { "registryType": "oci", "identifier": "acme/tools", "version": "3.1.0",
              "registryBaseUrl": "https://ghcr.io" }
          ],
          "remotes": [
            { "type": "streamable-http", "url": "https://{tenant}.acme.example/mcp",
              "variables": { "tenant": { "description": "Tenant", "isRequired": true } } }
          ] }"#,
    )]);
    let (drafts, skipped) = convert::all(&local(&files));
    assert!(skipped.is_empty(), "{skipped:?}");
    assert!(!drafts[0].pinned);
    assert_eq!(
        drafts[0].fields.args.as_deref().unwrap(),
        ["-y", "--registry=https://npm.pkg.github.com", "@acme/tools"]
    );
    assert!(drafts[1].pinned);
    assert_eq!(
        drafts[1].fields.args.as_deref().unwrap(),
        [
            "--index-url=https://pypi.acme.example/simple",
            "acme-tools==2.0.0"
        ]
    );
    // The default registry adds nothing.
    assert_eq!(
        drafts[2].fields.args.as_deref().unwrap(),
        ["-y", "@acme/pinned@1.0.0"]
    );
    assert_eq!(
        drafts[3].fields.args.as_deref().unwrap().last().unwrap(),
        "ghcr.io/acme/tools:3.1.0"
    );
    // A URL template's variables are inputs (P3 #14).
    assert_eq!(
        drafts[4].fields.url.as_deref(),
        Some("https://${env:TENANT}.acme.example/mcp")
    );
    assert!(
        drafts[4]
            .inputs
            .iter()
            .any(|i| i.field == "url" && i.required)
    );
    // The scan flags the unpinned command too (M40 rule).
    let report = crate::scan::Scanner::builtin().scan_command(
        drafts[0].fields.command.as_deref().unwrap(),
        drafts[0].fields.args.as_deref().unwrap(),
    );
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.rule == "mcp-unpinned-package"),
        "{report:?}"
    );
}

#[test]
fn a_required_named_argument_keeps_its_value() {
    let files = files(&[(
        "server.json",
        r#"{ "name": "x", "packages": [ { "registryType": "npm", "identifier": "x", "version": "1.0.0",
             "packageArguments": [ { "type": "named", "name": "--workspace", "isRequired": true } ] } ] }"#,
    )]);
    let (drafts, _) = convert::all(&local(&files));
    assert_eq!(
        drafts[0].fields.args.as_deref().unwrap(),
        ["-y", "x@1.0.0", "--workspace", "${env:WORKSPACE}"]
    );
}

#[test]
fn passthrough_values_that_look_secret_are_dropped() {
    let files = files(&[(
        ".omp/mcp.json",
        r#"{ "mcpServers": { "notes": { "url": "https://notes.example/mcp",
             "auth": { "type": "bearer", "token": "sk-live-abcdefghijklmnopqrstuvwxyz0123" } } } }"#,
    )]);
    let (drafts, skipped) = convert::all(&local(&files));
    assert!(skipped.is_empty(), "{skipped:?}");
    let draft = &drafts[0];
    let text = format!("{:?}", draft.passthrough);
    assert!(!text.contains("sk-live"), "{text}");
    let input = draft
        .inputs
        .iter()
        .find(|input| input.field == "client.omp.auth.token")
        .unwrap_or_else(|| panic!("{:?}", draft.inputs));
    assert!(input.value_dropped);
}

#[test]
fn setup_steps_are_written_for_a_person() {
    assert_eq!(
        setup_text(
            "npm install --prefix ${loadout_home}/servers/w and ${home}/x $${y}",
            "~",
            "C:/lh"
        ),
        "npm install --prefix C:/lh/servers/w and ~/x ${y}"
    );
}

#[test]
fn a_swap_that_is_undone_leaves_the_old_checkout() {
    let root = std::env::temp_dir().join(format!("loadout-mcp-swap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("home")).unwrap();
    let env = crate::env::Env::builder(Os::Linux, root.join("home")).build();
    let fs = crate::fs::RealFs;
    let home = crate::paths::loadout_home(&env, &fs, Some(&root.join("lh"))).unwrap();
    let runner = crate::remote::git::SystemRunner::default();
    let sx = SkillsEnv {
        env: &env,
        fs: &fs,
        home: &home,
        registry: &crate::adapters::detect::NoRegistry,
        runner: &runner,
        link_kind: crate::skills::LinkKind::Junction,
    };
    let old = checkout_dir(&sx, "w");
    std::fs::create_dir_all(old.join(".git")).unwrap();
    std::fs::write(old.join("index.js"), "old").unwrap();
    let clone = root.join("clone");
    std::fs::create_dir_all(clone.join(".git")).unwrap();
    std::fs::write(clone.join("index.js"), "new").unwrap();
    let swap = place_checkout(&sx, &clone, "w", false, SHA, "u").unwrap();
    assert_eq!(
        std::fs::read_to_string(old.join("index.js")).unwrap(),
        "new"
    );
    swap.finish(&fs, false);
    assert_eq!(
        std::fs::read_to_string(old.join("index.js")).unwrap(),
        "old"
    );
    assert_eq!(
        std::fs::read_to_string(clone.join("index.js")).unwrap(),
        "new"
    );
    let swap = place_checkout(&sx, &clone, "w", false, SHA, "u").unwrap();
    swap.finish(&fs, true);
    assert_eq!(
        std::fs::read_to_string(old.join("index.js")).unwrap(),
        "new"
    );
    assert_eq!(
        std::fs::read_to_string(old.join(".git").join(MARKER_FILE)).unwrap(),
        format!("{SHA}\nu\n")
    );
    let leftovers: Vec<_> = std::fs::read_dir(old.parent().unwrap())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(leftovers, ["w"]);
}

#[test]
fn ids_and_urls() {
    assert_eq!(id_from_name("Weather MCP"), "weather-mcp");
    assert_eq!(id_from_name("@@"), "server");
    assert_eq!(
        provenance_url("https://user:tok@github.com/acme/w.git"),
        "https://github.com/acme/w.git"
    );
    assert_eq!(
        provenance_url("git@github.com:acme/w.git"),
        "git@github.com:acme/w.git"
    );
    assert!(is_commit_ref("0123456789abcdef0123456789abcdef01234567"));
    assert!(!is_commit_ref("0123abc"));
    assert!(!is_commit_ref("main"));
    let short = crate::skills::install::check_ref("0123abc").unwrap_err();
    assert!(
        short.message().contains("full 40-character commit"),
        "{short}"
    );
    let taken = BTreeSet::from(["w".to_owned(), "w-2".to_owned()]);
    assert_eq!(free_id("w", &taken), "w-3");
}

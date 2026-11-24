//! `loadout agent install|remove|status|skill-path` (#8, M46) against a temp
//! `LOADOUT_HOME` and a temp profile: install, status and remove; the
//! update of an older store copy by the next store change (never by a read
//! command, which writes nothing and waits for no lock); a customized copy
//! left alone (reported once) until `install --reset`; the store copy's
//! metadata as 0.5.0 reads it; and the JSON fields the skill text cites,
//! against real output.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};

use agent_loadout_core::agent::{self, SKILL_ID};
use agent_loadout_core::paths::loadout_home;
use agent_loadout_core::skills::store;
use agent_loadout_core::store::{Operation, Store, StoreLock, Transaction};
use agent_loadout_core::{Os, RealFs};
use agent_loadout_testkit::TempHome;
use serde_json::Value;

struct Cli {
    th: TempHome,
    home: PathBuf,
}

impl Cli {
    fn new() -> Self {
        let th = TempHome::new();
        let home = th.root().join("lh");
        Self { th, home }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = self.th.command(env!("CARGO_BIN_EXE_loadout"));
        // Windows sockets (the catalog stub) need SYSTEMROOT.
        if let Some(root) = std::env::var_os("SYSTEMROOT").filter(|_| cfg!(windows)) {
            command.env("SYSTEMROOT", root);
        }
        command
            .arg("--home")
            .arg(&self.home)
            .args(args)
            .output()
            .expect("run loadout")
    }

    fn json(&self, args: &[&str]) -> (i32, Value, String) {
        let output = self.run(&[&["--json"], args].concat());
        let doc = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
            panic!(
                "{args:?}: stdout is not JSON ({err}):\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        (output.status.code().unwrap(), doc, stderr)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (code, doc, _) = self.json(args);
        assert_eq!(code, 0, "{args:?}: {doc:#}");
        doc
    }

    /// `~/<rel>/loadout`, a client's link to the store copy.
    fn link(&self, rel: &str) -> PathBuf {
        rel.split('/')
            .fold(self.th.home(), |path, part| path.join(part))
            .join(SKILL_ID)
    }

    /// Edits the store directly (one commit), as another machine or an
    /// older Loadout would.
    fn edit_store(&self, f: impl FnOnce(&mut Transaction<'_>)) {
        let env = self.th.env(host_os());
        let home = loadout_home(&env, &RealFs, Some(&self.home)).unwrap();
        let store = Store::open(&env, &RealFs, &home, "loadout-test/0.0.0")
            .unwrap()
            .unwrap();
        store
            .transaction(Operation::new("test.edit", "test: edit"), |tx| {
                f(tx);
                Ok(())
            })
            .unwrap();
    }

    /// Makes the store copy one an older Loadout wrote: `text` as its
    /// `SKILL.md`, recorded with its hash (`edited`: the hash of the bundled
    /// text instead, as if the user changed it afterwards).
    fn age_store_copy(&self, text: &str, edited: bool) {
        self.edit_store(|tx| {
            let mut files = agent::bundled_files();
            if !edited {
                files.insert("SKILL.md".to_owned(), text.as_bytes().to_vec());
            }
            let mut meta = store::read_meta(tx, SKILL_ID).unwrap().unwrap();
            let bundled = meta.bundled.as_mut().unwrap();
            bundled.version = "0.0.1".to_owned();
            bundled.hash = agent::normalized_hash(&files);
            store::stage_meta(tx, &meta).unwrap();
            tx.write("skills/loadout/SKILL.md", text).unwrap();
        });
    }

    fn store_file(&self, rel: &str) -> String {
        std::fs::read_to_string(self.home.join("store").join(rel)).unwrap()
    }

    /// A local folder holding one skill named `name`.
    fn skill_source(&self, name: &str) -> String {
        let src = self.th.root().join("src").join(name);
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name}.\n---\n{name}\n"),
        )
        .unwrap();
        src.to_string_lossy().into_owned()
    }
}

fn host_os() -> Os {
    if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    }
}

fn bundled_skill_md() -> String {
    agent::FILES[0].1.to_owned()
}

/// Every file below `root` (links recorded by target, never followed).
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&path).unwrap_or_default();
                out.insert(path, target.to_string_lossy().as_bytes().to_vec());
            } else if meta.is_dir() {
                walk(&path, out);
            } else {
                out.insert(path.clone(), std::fs::read(&path).unwrap_or_default());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, &mut out);
    out
}

/// The read and plan commands the skill lists (its commands.md tables
/// before the hand-off one), placeholders filled with harmless values.
fn cited_read_commands() -> Vec<Vec<String>> {
    let commands = agent::FILES
        .iter()
        .find(|(path, _)| *path == "references/commands.md")
        .unwrap()
        .1;
    let reads = commands
        .split("## Commands to hand to the user")
        .next()
        .unwrap();
    let mut found = Vec::new();
    for line in reads.lines().filter(|line| line.starts_with("| `loadout ")) {
        let cell = line.split('`').nth(1).unwrap();
        let words: Vec<String> = cell
            .split_whitespace()
            .skip(1)
            .map(|word| {
                if word.contains("..") {
                    "HEAD..HEAD".to_owned()
                } else if word.starts_with('<') {
                    "x".to_owned()
                } else {
                    word.to_owned()
                }
            })
            .collect();
        found.push(words);
    }
    assert!(found.len() >= 20, "{found:?}");
    found
}

#[test]
fn install_status_remove() {
    let cli = Cli::new();
    // No store yet: status reads only and creates nothing.
    let doc = cli.ok(&["agent", "status"]);
    assert_eq!(doc["bundled_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(doc["skill"], "loadout");
    assert!(doc["copy"].is_null());
    assert_eq!(doc["taken"], false);
    assert_eq!(doc["clients"][0]["client"], "omp");
    assert!(!cli.home.join("store").exists());

    let doc = cli.ok(&["agent", "install", "--client", "claude-code"]);
    assert!(doc["commit"].is_string());
    assert_eq!(doc["removed"], false);
    assert_eq!(doc["links"]["results"][0]["status"], "created");
    let link = cli.link(".claude/skills");
    assert_eq!(
        std::fs::read_to_string(link.join("SKILL.md")).unwrap(),
        bundled_skill_md()
    );
    assert!(link.join("references").join("commands.md").is_file());
    // Marked by `[bundled]` alone: no `[source]` 0.5.x could not read.
    let meta = cli.store_file("skill-meta/loadout.toml");
    assert!(meta.contains("[bundled]"), "{meta}");
    assert!(!meta.contains("[source]"), "{meta}");
    assert!(meta.contains(&format!("version = \"{}\"", env!("CARGO_PKG_VERSION"))));

    let doc = cli.ok(&["agent", "status"]);
    assert_eq!(doc["copy"]["state"], "current");
    assert!(doc["copy"]["action"].is_null());
    assert_eq!(doc["copy"]["version"], env!("CARGO_PKG_VERSION"));
    let claude = doc["clients"]
        .as_array()
        .unwrap()
        .iter()
        .find(|client| client["client"] == "claude-code")
        .unwrap();
    assert_eq!(claude["enabled"], true);

    // A normal store skill: listed with its origin.
    let doc = cli.ok(&["skills", "ls"]);
    assert_eq!(doc["skills"][0]["id"], "loadout");
    assert_eq!(doc["skills"][0]["source"]["kind"], "bundled");

    let doc = cli.ok(&["agent", "remove", "--client", "claude-code"]);
    assert!(doc["commit"].is_string());
    assert!(std::fs::symlink_metadata(&link).is_err());
    let doc = cli.ok(&["agent", "remove"]);
    assert_eq!(doc["removed"], true);
    assert!(cli.ok(&["agent", "status"])["copy"].is_null());
    // Nothing left to remove.
    let doc = cli.ok(&["agent", "remove"]);
    assert!(doc["commit"].is_null());
    assert_eq!(doc["removed"], false);
}

#[test]
fn install_defaults_to_detected_clients() {
    let cli = Cli::new();
    // Nothing detected: the user names a client.
    let (code, doc, _) = cli.json(&["agent", "install"]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "invalid_value");

    std::fs::create_dir_all(cli.th.home().join(".omp")).unwrap();
    std::fs::create_dir_all(cli.th.home().join(".claude")).unwrap();
    cli.ok(&["agent", "install"]);
    assert!(cli.link(".omp/agent/skills").join("SKILL.md").is_file());
    assert!(cli.link(".claude/skills").join("SKILL.md").is_file());
    let meta = cli.store_file("skill-meta/loadout.toml");
    assert!(
        meta.contains("clients = [\"claude-code\", \"omp\"]"),
        "{meta}"
    );

    let (code, doc, _) = cli.json(&["agent", "install", "--client", "nope"]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "unknown_client");
}

#[test]
fn a_skill_named_loadout_that_is_not_bundled_is_never_touched() {
    let cli = Cli::new();
    let src = cli.skill_source("mine");
    cli.ok(&["skills", "add", &src, "--id", "loadout"]);
    let mine = cli.store_file("skills/loadout/SKILL.md");

    let (code, doc, _) = cli.json(&["agent", "install", "--client", "claude-code"]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "already_exists");
    // The way out uses commands that exist (Loadout has no rename).
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(message.contains("loadout skills rm loadout"), "{message}");
    assert!(message.contains("--id"), "{message}");
    assert!(!message.contains("rename"), "{message}");
    let (code, doc, _) = cli.json(&["agent", "remove"]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "invalid_value");
    let doc = cli.ok(&["agent", "status"]);
    assert_eq!(doc["taken"], true);
    assert!(doc["copy"].is_null());
    assert_eq!(cli.store_file("skills/loadout/SKILL.md"), mine);
}

/// After a Loadout update, every read command the skill cites leaves the
/// data directory and the profile byte-identical (no commit, no link
/// sync) and adds no wait while another process holds the store lock; the
/// next store change updates the copy and says so.
#[test]
fn read_commands_never_write_and_a_store_change_updates_an_older_copy() {
    let cli = Cli::new();
    cli.ok(&["agent", "install", "--client", "claude-code"]);
    cli.age_store_copy("old text\n", false);

    let doc = cli.ok(&["agent", "status"]);
    assert_eq!(doc["copy"]["state"], "outdated");
    assert_eq!(doc["copy"]["action"], "loadout agent install");

    let held = StoreLock::acquire(
        &cli.th.fs(),
        &cli.home.join("state"),
        Duration::from_secs(1),
    )
    .unwrap();
    let before = snapshot(cli.th.root());
    for words in cited_read_commands() {
        let args: Vec<&str> = words.iter().map(String::as_str).collect();
        let started = Instant::now();
        let output = cli.run(&args);
        let took = started.elapsed();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            took < Duration::from_secs(5),
            "{args:?} waited {took:?}: {stderr}"
        );
        assert!(!stderr.contains("Loadout skill"), "{args:?}: {stderr}");
        assert_eq!(snapshot(cli.th.root()), before, "{args:?} wrote something");
    }
    drop(held);
    assert_eq!(cli.store_file("skills/loadout/SKILL.md"), "old text\n");

    // A command that changes the store updates it first, and says so.
    let src = cli.skill_source("notes");
    let (code, _, stderr) = cli.json(&["skills", "add", &src]);
    assert_eq!(code, 0);
    assert!(
        stderr.contains("Updated the Loadout skill in the store from 0.0.1"),
        "{stderr}"
    );
    assert_eq!(
        cli.store_file("skills/loadout/SKILL.md"),
        bundled_skill_md()
    );
    assert_eq!(cli.ok(&["agent", "status"])["copy"]["state"], "current");
    let doc = cli.ok(&["history", "--entity", "skill/loadout", "--limit", "1"]);
    let message = doc["commits"][0].to_string();
    assert!(message.contains("skill: update loadout"), "{message}");
}

/// The update's skill sync shows client folders that need a decision
/// (here a copy the user changed in place).
#[test]
fn the_update_reports_client_folders_that_need_a_decision() {
    let cli = Cli::new();
    cli.ok(&["agent", "install", "--client", "claude-code"]);
    cli.ok(&[
        "skills",
        "enable",
        "loadout",
        "--client",
        "claude-code",
        "--copy",
    ]);
    let copy = cli.link(".claude/skills");
    std::fs::write(copy.join("SKILL.md"), "changed in place\n").unwrap();
    cli.age_store_copy("old text\n", false);

    let src = cli.skill_source("notes");
    let (_, _, stderr) = cli.json(&["skills", "add", &src]);
    assert!(stderr.contains("Updated the Loadout skill"), "{stderr}");
    assert!(stderr.contains("changed"), "{stderr}");
}

#[test]
fn a_customized_copy_is_reported_once_and_kept_until_reset() {
    let cli = Cli::new();
    std::fs::create_dir_all(cli.th.home().join(".omp")).unwrap();
    cli.ok(&[
        "agent",
        "install",
        "--client",
        "claude-code",
        "--client",
        "omp",
    ]);
    // An older copy the user edited afterwards.
    cli.age_store_copy("my own notes\n", true);
    let doc = cli.ok(&["agent", "status"]);
    assert_eq!(doc["copy"]["state"], "customized");
    assert_eq!(doc["copy"]["action"], "loadout agent install --reset");

    let (_, _, stderr) = cli.json(&["skills", "add", &cli.skill_source("one")]);
    assert!(stderr.contains("customized, not updated"), "{stderr}");
    let (_, _, stderr) = cli.json(&["skills", "add", &cli.skill_source("two")]);
    assert!(!stderr.contains("customized"), "reported once: {stderr}");
    assert_eq!(cli.store_file("skills/loadout/SKILL.md"), "my own notes\n");

    // Installing for a client keeps the edit too.
    let doc = cli.ok(&["agent", "install", "--client", "omp"]);
    assert_eq!(doc["refreshed"]["kind"], "customized");
    assert_eq!(cli.store_file("skills/loadout/SKILL.md"), "my own notes\n");

    // --reset replaces it and keeps the clients it is enabled for.
    let doc = cli.ok(&["agent", "install", "--reset"]);
    assert_eq!(doc["reset"], true);
    assert_eq!(
        cli.store_file("skills/loadout/SKILL.md"),
        bundled_skill_md()
    );
    let meta = cli.store_file("skill-meta/loadout.toml");
    assert!(
        meta.contains("clients = [\"claude-code\", \"omp\"]"),
        "{meta}"
    );
    assert_eq!(cli.ok(&["agent", "status"])["copy"]["state"], "current");
}

#[test]
fn skill_path_prints_a_folder_with_the_bundled_copy() {
    let cli = Cli::new();
    let doc = cli.ok(&["agent", "skill-path"]);
    let dir = PathBuf::from(doc["path"].as_str().unwrap());
    assert!(dir.starts_with(cli.home.join("cache")), "{}", dir.display());
    assert_eq!(
        std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
        bundled_skill_md()
    );
    // A stale copy is rewritten.
    std::fs::write(dir.join("SKILL.md"), "stale").unwrap();
    std::fs::write(dir.join("extra.md"), "extra").unwrap();
    cli.ok(&["agent", "skill-path"]);
    assert_eq!(
        std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
        bundled_skill_md()
    );
    assert!(!dir.join("extra.md").exists());
    // No store is created for it.
    assert!(!cli.home.join("store").exists());
}

/// `instance` against `schema`: `$ref`, `type`, `enum`, `const`,
/// `properties`, `required`, `items`, `anyOf`/`oneOf`. Enough of JSON
/// Schema for the store schemas; extra keys are allowed, as serde allows
/// them.
fn validate(root: &Value, schema: &Value, instance: &Value, at: &str) -> Result<(), String> {
    if let Some(reference) = schema["$ref"].as_str() {
        let name = reference.trim_start_matches("#/$defs/");
        return validate(root, &root["$defs"][name], instance, at);
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(options) = schema[key].as_array() {
            return if options
                .iter()
                .any(|option| validate(root, option, instance, at).is_ok())
            {
                Ok(())
            } else {
                Err(format!("{at}: matches none of {key}"))
            };
        }
    }
    if let Some(values) = schema["enum"].as_array()
        && !values.contains(instance)
    {
        return Err(format!("{at}: {instance} is not one of {values:?}"));
    }
    if let Some(value) = schema.get("const")
        && value != instance
    {
        return Err(format!("{at}: {instance} is not {value}"));
    }
    let kind_ok = match schema["type"].as_str() {
        Some("string") => instance.is_string(),
        Some("integer") => instance.is_i64() || instance.is_u64(),
        Some("boolean") => instance.is_boolean(),
        Some("array") => instance.is_array(),
        Some("object") => instance.is_object(),
        _ => true,
    };
    if !kind_ok {
        return Err(format!("{at}: {instance} is not a {}", schema["type"]));
    }
    if let (Some(properties), Some(object)) =
        (schema["properties"].as_object(), instance.as_object())
    {
        for required in schema["required"].as_array().into_iter().flatten() {
            let name = required.as_str().unwrap();
            if !object.contains_key(name) {
                return Err(format!("{at}: missing {name}"));
            }
        }
        for (name, value) in object {
            if let Some(property) = properties.get(name) {
                validate(root, property, value, &format!("{at}.{name}"))?;
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), instance.as_array()) {
        for (index, value) in array.iter().enumerate() {
            validate(root, items, value, &format!("{at}[{index}]"))?;
        }
    }
    Ok(())
}

/// A store holding the Loadout skill stays readable by 0.5.0 machines
/// syncing it: its metadata validates under the v0.5.0 schema (a new
/// `[source] kind` would not, and 0.5.0 refuses merges holding it).
#[test]
fn the_store_copy_metadata_validates_under_the_0_5_0_schema() {
    let cli = Cli::new();
    cli.ok(&["agent", "install", "--client", "claude-code"]);
    let schema: Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/schema-v0.5.0/skill-meta.schema.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let meta: toml::Value = toml::from_str(&cli.store_file("skill-meta/loadout.toml")).unwrap();
    let meta = serde_json::to_value(meta).unwrap();
    validate(&schema, &schema, &meta, "skill-meta").unwrap();
    // The checker refuses what 0.5.0 refused.
    let mut bad = meta.clone();
    bad["source"] = serde_json::json!({ "kind": "bundled" });
    assert!(validate(&schema, &schema, &bad, "skill-meta").is_err());
}

fn registry_fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/catalogs/registry")
            .join(name),
    )
    .unwrap()
}

/// A registry on 127.0.0.1 answering every request with the recorded
/// search page (or entry, for `/versions/`).
fn registry_stub() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let search = registry_fixture("search-weather.json");
    let entry = registry_fixture("entry-weather-1.2.0.json");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let target = String::from_utf8_lossy(&request)
                .split(' ')
                .nth(1)
                .unwrap_or("/")
                .to_owned();
            let body = if target.contains("/versions/") {
                &entry
            } else {
                &search
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    url
}

/// Whether `word` is a key or a string value anywhere in `doc`.
fn mentions(doc: &Value, word: &str) -> bool {
    match doc {
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| key == word || mentions(value, word)),
        Value::Array(items) => items.iter().any(|item| mentions(item, word)),
        Value::String(text) => text == word,
        _ => false,
    }
}

/// Values the rows list as examples of a field, which other states would
/// show (catalog statuses, this skill's copy states); their spelling is
/// checked by the catalog tests and `CopyState::as_str`.
const LISTED_VALUES: [&str; 7] = [
    "cached",
    "stale",
    "off",
    "error",
    "outdated",
    "customized",
    "newer",
];

/// Every JSON field commands.md cites for `command` is in its real
/// output `doc`.
fn check_fields(command: &str, doc: &Value) {
    let commands = agent::FILES
        .iter()
        .find(|(path, _)| *path == "references/commands.md")
        .unwrap()
        .1;
    let row = commands
        .lines()
        .find(|line| line.starts_with(&format!("| `{command}")))
        .unwrap_or_else(|| panic!("commands.md has no row for {command}"));
    let shows = row.split('|').nth(2).unwrap();
    let mut checked = 0;
    for (n, token) in shows.split('`').enumerate() {
        let is_field = n % 2 == 1
            && !token.is_empty()
            && token
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || "_.[]".contains(ch));
        if !is_field || LISTED_VALUES.contains(&token) {
            continue;
        }
        for segment in token.split('.') {
            let segment = segment.trim_end_matches("[]");
            assert!(
                mentions(doc, segment),
                "{command}: commands.md cites `{token}`, but `{segment}` is not in\n{doc:#}"
            );
            checked += 1;
        }
    }
    assert!(checked > 3, "{command}: only {checked} fields cited");
}

#[test]
fn the_fields_the_skill_cites_are_in_real_output() {
    let cli = Cli::new();
    // A store server enabled for Claude Code (status entries, server ls),
    // an entry Loadout does not manage with a literal secret (import
    // --scan), the Loadout skill (skills ls) and a catalog (search).
    cli.ok(&[
        "server",
        "add",
        "demo",
        "--client",
        "claude-code",
        "--",
        "npx",
        "demo-mcp",
    ]);
    let claude = cli.th.home().join(".claude.json");
    let mut config: Value =
        serde_json::from_str(&std::fs::read_to_string(&claude).unwrap()).unwrap();
    config["mcpServers"]["weather"] = serde_json::json!({
        "command": "npx",
        "args": ["weather-mcp"],
        "env": { "WEATHER_API_KEY": "sk-live-4f9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c" }
    });
    std::fs::write(&claude, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    cli.ok(&["agent", "install", "--client", "claude-code"]);
    let url = registry_stub();
    for id in ["mcp-registry", "anthropic-skills", "skills-sh"] {
        cli.ok(&["catalog", "disable", id]);
    }
    cli.ok(&["catalog", "add", &url, "--kind", "registry", "--id", "acme"]);
    cli.ok(&["catalog", "online", "on"]);

    check_fields(
        "loadout import --scan --json",
        &cli.ok(&["import", "--scan"]),
    );
    check_fields("loadout status --json", &cli.ok(&["status"]));
    check_fields("loadout server ls --json", &cli.ok(&["server", "ls"]));
    check_fields("loadout skills ls --json", &cli.ok(&["skills", "ls"]));
    check_fields(
        "loadout catalog search <query> --json",
        &cli.ok(&["catalog", "search", "weather"]),
    );
    check_fields("loadout agent status --json", &cli.ok(&["agent", "status"]));
}

//! `loadout catalog ...` and catalog installs (#26) against a temp
//! `LOADOUT_HOME` and a local HTTP stub serving recorded MCP Registry
//! answers: the online switch (off: zero requests), catalogs.toml
//! changes, search and show, `server add --from catalog:...` with its
//! provenance, and skills.sh addresses. Taps are covered by the core tests.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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
        // Windows sockets need SYSTEMROOT, which the hermetic environment
        // leaves out.
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

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let output = self.run(&[&["--json"], args].concat());
        let doc = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
            panic!(
                "{args:?}: stdout is not JSON ({err}):\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.code().unwrap(), doc)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (code, doc) = self.json(args);
        assert_eq!(code, 0, "{args:?}: {doc:#}");
        doc
    }
}

fn registry_fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/catalogs/registry")
        .join(name);
    std::fs::read(path).unwrap()
}

/// A registry on 127.0.0.1: entries for `/versions/`, the search page
/// otherwise; counts requests.
fn stub() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
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
            counter.fetch_add(1, Ordering::SeqCst);
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
    (url, hits)
}

/// The built-ins off (they would reach the network) and the stub added
/// as `acme`.
fn with_stub(cli: &Cli, url: &str) {
    for id in ["mcp-registry", "anthropic-skills", "skills-sh"] {
        cli.ok(&["catalog", "disable", id]);
    }
    let added = cli.ok(&["catalog", "add", url, "--kind", "registry", "--id", "acme"]);
    assert_eq!(added["catalog"], "acme");
}

#[test]
fn catalogs_are_listed_and_changed_and_nothing_is_contacted_while_off() {
    let cli = Cli::new();
    let (url, hits) = stub();
    let listed = cli.ok(&["catalog", "ls"]);
    assert_eq!(listed["online"], false);
    assert_eq!(listed["decided"], false);
    let ids: Vec<&str> = listed["catalogs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["mcp-registry", "anthropic-skills", "skills-sh"]);
    assert_eq!(
        listed["catalogs"][0]["attribution"],
        "Data from the Official MCP Registry"
    );

    with_stub(&cli, &url);
    let (code, doc) = cli.json(&["catalog", "search", "weather"]);
    assert_eq!(code, 1, "no catalog answered: {doc:#}");
    assert_eq!(doc["results"][0]["status"], "off");
    assert_eq!(doc["results"][0]["error"]["code"], "catalogs_off");
    let (code, doc) = cli.json(&["catalog", "refresh"]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "catalogs_off");
    let (code, doc) = cli.json(&[
        "server",
        "add",
        "--from",
        "catalog:acme/io.github.acme/weather",
        "--dry-run",
    ]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "catalogs_off");
    assert_eq!(hits.load(Ordering::SeqCst), 0, "off: zero requests");

    let (code, doc) = cli.json(&["catalog", "rm", "mcp-registry"]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "invalid_value");
    cli.ok(&["catalog", "add", "https://github.com/acme/skills"]);
    let listed = cli.ok(&["catalog", "ls"]);
    let tap = listed["catalogs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "skills")
        .unwrap()
        .clone();
    assert_eq!(tap["kind"], "git_tap", "a GitHub URL is a tap");
    // Off: a skill from a tap is not cloned.
    let (code, doc) = cli.json(&["skills", "add", "catalog:skills/pdf", "--yes"]);
    assert_eq!(code, 1, "{doc:#}");
    assert_eq!(doc["error"]["code"], "catalogs_off");
    // A token in a tap URL is refused, never stored or printed.
    let (code, doc) = cli.json(&[
        "catalog",
        "add",
        "https://oauth2:glpat-SECRET123@gitlab.com/acme/skills.git",
    ]);
    assert_eq!(code, 1, "{doc:#}");
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(message.contains("git credential helper"), "{message}");
    assert!(!doc.to_string().contains("SECRET123"), "{doc:#}");
    cli.ok(&["catalog", "rm", "skills"]);
    let text = std::fs::read_to_string(cli.home.join("store").join("catalogs.toml")).unwrap();
    assert!(text.contains("[catalogs.acme]"), "{text}");
    assert!(
        !text.contains("skills\"") && !text.contains("[catalogs.skills]"),
        "{text}"
    );
}

#[test]
fn a_registry_entry_is_searched_shown_and_added_with_its_provenance() {
    let cli = Cli::new();
    let (url, hits) = stub();
    with_stub(&cli, &url);
    let on = cli.ok(&["catalog", "online", "on"]);
    assert_eq!(on["online"], true);
    assert_eq!(
        on["contacts"],
        serde_json::json!(["127.0.0.1:".to_owned() + url.rsplit(':').next().unwrap()])
    );

    let found = cli.ok(&["catalog", "search", "weather"]);
    let results = found["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "only enabled catalogs: {found:#}");
    assert_eq!(results[0]["status"], "fresh");
    let entries = results[0]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["ref"], "catalog:acme/io.github.acme/weather");
    assert_eq!(
        entries[0]["required_secrets"],
        serde_json::json!(["WEATHER_API_KEY", "Authorization", "tenant"])
    );
    assert!(entries[0].get("server_json").is_none());
    let short = cli.json(&["catalog", "search", "we"]).1;
    assert_eq!(short["results"][0]["status"], "query_too_short");

    let shown = cli.ok(&["catalog", "show", "acme/io.github.acme/weather"]);
    assert_eq!(shown["entry"]["version"], "1.2.0");
    let text = cli.run(&["catalog", "show", "catalog:acme/io.github.acme/weather"]);
    let text = String::from_utf8_lossy(&text.stdout).into_owned();
    assert!(
        text.contains("loadout server add --from catalog:acme/io.github.acme/weather"),
        "{text}"
    );

    let dry = cli.ok(&[
        "server",
        "add",
        "--from",
        "catalog:acme/io.github.acme/weather",
        "--dry-run",
    ]);
    assert_eq!(dry["source"]["kind"], "registry");
    assert_eq!(dry["source"]["catalog"], "acme/io.github.acme/weather");
    assert_eq!(dry["source"]["version"], "1.2.0");
    let saved = cli.ok(&[
        "server",
        "add",
        "--from",
        "catalog:acme/io.github.acme/weather",
        "--trust-source",
    ]);
    let id = saved["saved"][0].as_str().unwrap().to_owned();
    let file = std::fs::read_to_string(
        cli.home
            .join("store")
            .join("servers")
            .join(format!("{id}.toml")),
    )
    .unwrap();
    assert!(file.contains("origin = \"catalog:acme\""), "{file}");
    assert!(file.contains("kind = \"registry\""), "{file}");
    assert!(
        file.contains("catalog = \"acme/io.github.acme/weather\""),
        "{file}"
    );
    assert!(file.contains("version = \"1.2.0\""), "{file}");
    let (code, doc) = cli.json(&[
        "server",
        "add",
        "--from",
        "catalog:acme/io.github.acme/weather",
        "--path",
        "x",
    ]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "invalid_value");
    assert!(hits.load(Ordering::SeqCst) >= 3);
}

#[test]
fn skills_sh_addresses_map_to_github_and_refuse_other_sites() {
    let cli = Cli::new();
    let (code, doc) = cli.json(&["skills", "add", "https://skills.sh/site/mintlify.com/docs"]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "invalid_value");
    assert!(
        doc["error"]["message"].as_str().unwrap().contains("GitHub"),
        "{doc:#}"
    );
    let (code, doc) = cli.json(&["skills", "add", "catalog:nope/x"]);
    assert_eq!(code, 1);
    assert_eq!(doc["error"]["code"], "not_found");
}

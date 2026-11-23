//! M31: MCP import commands against a temp home (SPEC 8.6, 11.7; #3, #9):
//! user files and known projects are scanned, identical definitions
//! collapse, differing ones import side by side, literal secrets go to the
//! keyring or a reference without crossing IPC or reaching the store, and
//! identical entries are adopted without a write in one commit.

use std::path::{Path, PathBuf};

use agent_loadout_core::ErrorCode;
use agent_loadout_core::apply::ledger::{Ledger, Origin};
use agent_loadout_core::apply::unmanaged::SEEN_FILE;
use agent_loadout_core::layering::{Project, TrustStore, projects as core_projects};

use super::import::{
    self, ImportChoices, ImportPick, ImportScope, ImportSecretChoice, ImportSecretTarget,
    SecretDestinationKind,
};
use super::tests::Fx;

/// A plaintext value planted in a client file; it may reach the keyring
/// only.
const PLANTED: &str = "planted-chatty-value-4f2a9c";

fn omp(fx: &Fx) -> PathBuf {
    fx.th.home().join(".omp").join("agent").join("mcp.json")
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn servers(entries: &[(&str, &str)]) -> String {
    let body = entries
        .iter()
        .map(|(name, value)| format!("    \"{name}\": {value}"))
        .collect::<Vec<_>>()
        .join(",\n");
    format!("{{\n  \"mcpServers\": {{\n{body}\n  }}\n}}\n")
}

fn stdio(command: &str, args: &[&str]) -> String {
    let args = args
        .iter()
        .map(|a| format!("\"{a}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{\"type\": \"stdio\", \"command\": \"{command}\", \"args\": [{args}]}}")
}

/// A project folder with a manifest, known on this machine; trusted when
/// asked.
fn known_project(fx: &Fx, name: &str, id: &str, trusted: bool) -> PathBuf {
    let root = fx.th.projects().join(name);
    write(
        &root.join(".loadout").join("project.local.toml"),
        &format!("schema = 1\nid = \"{id}\"\nname = \"{name}\"\n"),
    );
    let fs = fx.core.fs.as_ref();
    let project = Project::load(fs, &root).unwrap().unwrap();
    let store = fx.store();
    core_projects::add(&store, &fx.core.apply_env(), &project).unwrap();
    if trusted {
        let state = fx.core.home.state_dir();
        let mut trust = TrustStore::load(fs, &state).unwrap();
        trust.trust(&project);
        trust.save(fs, &state).unwrap();
    }
    root
}

/// Every working-tree file of the store holds no planted value.
fn assert_store_clean(fx: &Fx) {
    fn walk(dir: &Path) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            if path.is_dir() {
                walk(&path);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                assert!(
                    !String::from_utf8_lossy(&bytes).contains(PLANTED),
                    "{path:?} holds the value"
                );
            }
        }
    }
    walk(&fx.core.home.path.join("store"));
}

fn pick(name: &str, source: &str, id: &str) -> ImportPick {
    ImportPick {
        name: name.to_owned(),
        source: source.to_owned(),
        id: id.to_owned(),
    }
}

fn choices(picks: Vec<ImportPick>, secrets: Vec<ImportSecretChoice>) -> ImportChoices {
    ImportChoices {
        picks,
        secrets,
        only: false,
    }
}

/// Import commits in the store's history.
fn imports(fx: &Fx) -> usize {
    fx.summaries()
        .iter()
        .filter(|summary| summary.starts_with("import:"))
        .count()
}

/// Identical entries of two clients collapse into one definition; the
/// import enables it for both and adopts both entries without a write, in
/// one commit.
#[test]
fn identical_entries_collapse_and_are_adopted_without_writes() {
    let fx = Fx::new();
    let docs = stdio("npx", &["-y", "@example/docs-mcp"]);
    write(&fx.claude(), &servers(&[("docs", &docs)]));
    write(&omp(&fx), &servers(&[("docs", &docs)]));
    let claude_before = fx.read(&fx.claude());
    let omp_before = fx.read(&omp(&fx));

    let scan = import::scan(&fx.core, None).unwrap();
    assert!(
        !fx.core.home.path.join("store").exists(),
        "a scan creates nothing"
    );
    let docs = scan.candidates.iter().find(|c| c.name == "docs").unwrap();
    assert!(!docs.conflict);
    assert_eq!(docs.variants.len(), 1);
    let variant = &docs.variants[0];
    let keys: Vec<&str> = variant.sources.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(keys, ["claude-code", "omp"]);
    assert_eq!(variant.sources[0].file, "~/.claude.json");
    assert_eq!(variant.sources[0].id, "claude-code/docs");
    assert!(variant.enables);
    assert_eq!(variant.suggested_id, "docs");
    fx.assert_no_root(&serde_json::to_string(&scan).unwrap());

    let applied = import::apply_choices(
        &fx.core,
        choices(vec![pick("docs", &variant.key, "docs")], Vec::new()),
    )
    .unwrap();
    assert_eq!(applied.imported, ["docs"]);
    assert!(applied.skipped.is_empty(), "{:?}", applied.skipped);
    let run = applied.change.apply.as_ref().expect("instant mode applies");
    assert_eq!(run.files_written, 0, "{run:#?}");
    assert_eq!(fx.read(&fx.claude()), claude_before);
    assert_eq!(fx.read(&omp(&fx)), omp_before);
    assert_eq!(fx.summaries()[0], "import: 1 server (docs)");
    assert_eq!(imports(&fx), 1, "one commit");
    let ledger = Ledger::load(fx.core.fs.as_ref(), &fx.core.home.state_dir()).unwrap();
    assert_eq!(ledger.entries.len(), 2);
    assert!(
        ledger
            .entries
            .iter()
            .all(|e| e.origin == Some(Origin::Adopted))
    );
    let server = fx.store_file("servers/docs.toml");
    assert!(server.contains("\"claude-code\""), "{server}");
    assert!(server.contains("\"omp\""), "{server}");
}

/// Differing definitions are a conflict; importing both gives `name` and
/// `name-<client>`, each enabled where it was found and adopted there
/// without a write.
#[test]
fn conflicting_definitions_import_side_by_side() {
    let fx = Fx::new();
    write(
        &fx.claude(),
        &servers(&[("docs", &stdio("npx", &["-y", "@example/docs-mcp"]))]),
    );
    write(
        &omp(&fx),
        &servers(&[("docs", &stdio("npx", &["-y", "@example/docs-mcp", "--v2"]))]),
    );
    let claude_before = fx.read(&fx.claude());
    let omp_before = fx.read(&omp(&fx));

    let scan = import::scan(&fx.core, None).unwrap();
    let docs = scan.candidates.iter().find(|c| c.name == "docs").unwrap();
    assert!(docs.conflict);
    let ids: Vec<&str> = docs
        .variants
        .iter()
        .map(|v| v.suggested_id.as_str())
        .collect();
    assert_eq!(ids, ["docs", "docs-omp"]);

    let applied = import::apply_choices(
        &fx.core,
        choices(
            vec![
                pick("docs", "claude-code", "docs"),
                pick("docs", "omp", "docs-omp"),
            ],
            Vec::new(),
        ),
    )
    .unwrap();
    assert_eq!(applied.imported, ["docs", "docs-omp"]);
    assert_eq!(applied.change.apply.as_ref().unwrap().files_written, 0);
    assert_eq!(fx.read(&fx.claude()), claude_before);
    assert_eq!(fx.read(&omp(&fx)), omp_before);
    assert_eq!(fx.summaries()[0], "import: 2 servers (docs, docs-omp)");
    let renamed = fx.store_file("servers/docs-omp.toml");
    assert!(renamed.contains("native_name = \"docs\""), "{renamed}");
    assert!(renamed.contains("--v2"), "{renamed}");
    let ledger = Ledger::load(fx.core.fs.as_ref(), &fx.core.home.state_dir()).unwrap();
    let owners: Vec<(&str, Option<&str>)> = ledger
        .entries
        .iter()
        .map(|e| (e.client.as_str(), e.server_id.as_deref()))
        .collect();
    assert!(
        owners.contains(&("claude-code", Some("docs"))),
        "{owners:?}"
    );
    assert!(owners.contains(&("omp", Some("docs-omp"))), "{owners:?}");
    assert!(
        ledger
            .entries
            .iter()
            .all(|e| e.origin == Some(Origin::Adopted))
    );

    // The same definition chosen twice, or an id taken, is skipped.
    write(
        &fx.claude(),
        &servers(&[
            ("docs", &stdio("npx", &["-y", "@example/docs-mcp"])),
            ("notes", &stdio("notes-mcp", &[])),
        ]),
    );
    let applied = import::apply_choices(
        &fx.core,
        choices(
            vec![
                pick("notes", "claude-code", "docs"),
                pick("notes", "claude-code", "notes"),
                pick("notes", "claude-code", "notes-2"),
            ],
            Vec::new(),
        ),
    )
    .unwrap();
    assert_eq!(applied.imported, ["notes"]);
    let codes: Vec<ErrorCode> = applied.skipped.iter().map(|s| s.error.code).collect();
    assert_eq!(codes, [ErrorCode::AlreadyExists, ErrorCode::InvalidValue]);
}

/// A literal secret goes to the keyring: the value is taken from the
/// client file in core and never reaches the store or IPC; without a
/// destination the definition is skipped.
#[test]
fn literal_secrets_go_to_the_keyring_without_crossing_ipc() {
    let fx = Fx::new();
    let chatty = format!(
        "{{\"type\": \"stdio\", \"command\": \"chatty\", \"env\": {{\"LOG_LEVEL\": \"info\", \
         \"CHATTY_API_KEY\": \"{PLANTED}\"}}}}"
    );
    write(&fx.claude(), &servers(&[("chatty", &chatty)]));

    let scan = import::scan(&fx.core, Some(ImportScope::User)).unwrap();
    let json = serde_json::to_string(&scan).unwrap();
    assert!(!json.contains(PLANTED), "{json}");
    let variant = &scan.candidates[0].variants[0];
    assert_eq!(variant.secrets.len(), 1);
    assert_eq!(variant.secrets[0].field, "env.CHATTY_API_KEY");
    assert_eq!(variant.secrets[0].suggested_alias, "CHATTY_CHATTY_API_KEY");
    let draft = variant.draft.as_ref().unwrap();
    let env: Vec<(&str, &str)> = draft
        .env
        .iter()
        .map(|kv| (kv.key.as_str(), kv.value.as_str()))
        .collect();
    assert!(
        env.contains(&("CHATTY_API_KEY", "${secret:CHATTY_CHATTY_API_KEY}")),
        "{env:?}"
    );
    assert!(env.contains(&("LOG_LEVEL", "info")), "{env:?}");

    // No destination: skipped, nothing committed.
    let applied = import::apply_choices(
        &fx.core,
        choices(vec![pick("chatty", "claude-code", "chatty")], Vec::new()),
    )
    .unwrap();
    assert!(applied.imported.is_empty());
    assert_eq!(
        applied.skipped[0].error.code,
        ErrorCode::NeedsSecretExtraction
    );
    assert!(!applied.skipped[0].error.message.contains(PLANTED));

    let applied = import::apply_choices(
        &fx.core,
        choices(
            vec![pick("chatty", "claude-code", "chatty")],
            vec![ImportSecretChoice {
                name: "chatty".to_owned(),
                field: None,
                target: ImportSecretTarget::Keyring,
            }],
        ),
    )
    .unwrap();
    let json = serde_json::to_string(&applied).unwrap();
    assert!(!json.contains(PLANTED), "{json}");
    fx.assert_no_root(&json);
    assert_eq!(applied.imported, ["chatty"]);
    assert_eq!(applied.secrets.len(), 1);
    assert_eq!(applied.secrets[0].alias, "CHATTY_CHATTY_API_KEY");
    assert_eq!(
        applied.secrets[0].destination,
        SecretDestinationKind::Keyring
    );
    assert_eq!(
        fx.keyring
            .peek("loadout", "CHATTY_CHATTY_API_KEY")
            .as_deref(),
        Some(PLANTED)
    );
    let server = fx.store_file("servers/chatty.toml");
    assert!(
        server.contains("CHATTY_API_KEY = \"${secret:CHATTY_CHATTY_API_KEY}\""),
        "{server}"
    );
    let secrets = fx.store_file("secrets.toml");
    assert!(
        secrets.contains("keyring://loadout/CHATTY_CHATTY_API_KEY"),
        "{secrets}"
    );
    assert_store_clean(&fx);
    assert_eq!(imports(&fx), 1, "one commit holds the server and its alias");
}

/// In review mode the import commits the store change only; the entry
/// whose literal became a reference waits in Pending changes. In instant
/// mode the same import rewrites it to the reference and takes it over.
#[test]
fn a_rewritten_entry_waits_in_review_mode_and_is_written_in_instant_mode() {
    let chatty = format!(
        "{{\"type\": \"stdio\", \"command\": \"chatty\", \"env\": {{\"CHATTY_API_KEY\": \
         \"{PLANTED}\"}}}}"
    );
    let keyring = || {
        choices(
            vec![pick("chatty", "claude-code", "chatty")],
            vec![ImportSecretChoice {
                name: "chatty".to_owned(),
                field: Some("env.CHATTY_API_KEY".to_owned()),
                target: ImportSecretTarget::Keyring,
            }],
        )
    };

    let review = Fx::new();
    review.stage(&[("settings.toml", "schema = 1\napply_mode = \"review\"\n")]);
    write(&review.claude(), &servers(&[("chatty", &chatty)]));
    let before = review.read(&review.claude());
    let applied = import::apply_choices(&review.core, keyring()).unwrap();
    assert_eq!(applied.imported, ["chatty"]);
    assert!(applied.change.apply.is_none(), "review mode does not apply");
    assert_eq!(review.read(&review.claude()), before);
    let pending = super::apply::pending(&review.core).unwrap();
    let json = serde_json::to_string(&pending).unwrap();
    assert!(!json.contains(PLANTED), "{json}");
    assert!(
        pending
            .files
            .iter()
            .any(|file| file.file == "~/.claude.json"),
        "{json}"
    );

    let instant = Fx::new();
    write(&instant.claude(), &servers(&[("chatty", &chatty)]));
    let applied = import::apply_choices(&instant.core, keyring()).unwrap();
    let run = applied.change.apply.expect("instant mode applies");
    assert_eq!(run.files_written, 1, "{run:#?}");
    let claude = instant.read(&instant.claude());
    assert!(!claude.contains(PLANTED), "{claude}");
    assert!(claude.contains("CHATTY_CHATTY_API_KEY"), "{claude}");
    let ledger = Ledger::load(instant.core.fs.as_ref(), &instant.core.home.state_dir()).unwrap();
    assert_eq!(ledger.entries.len(), 1);
    assert_eq!(ledger.entries[0].server_id.as_deref(), Some("chatty"));
}

/// A 1Password reference or an environment variable is recorded as given;
/// a malformed reference is refused before anything is written.
#[test]
fn literal_secrets_map_to_references() {
    let fx = Fx::new();
    let remote = format!(
        "{{\"type\": \"http\", \"url\": \"https://mcp.example.test/mcp\", \"headers\": \
         {{\"Authorization\": \"Bearer {PLANTED}\", \"X-Api-Key\": \"{PLANTED}\"}}}}"
    );
    write(&fx.claude(), &servers(&[("remote", &remote)]));

    let bad = import::apply_choices(
        &fx.core,
        choices(
            vec![pick("remote", "claude-code", "remote")],
            vec![ImportSecretChoice {
                name: "remote".to_owned(),
                field: None,
                target: ImportSecretTarget::Onepassword {
                    reference: "op://only-two/parts".to_owned(),
                },
            }],
        ),
    )
    .unwrap_err();
    assert_eq!(bad.code, ErrorCode::InvalidValue);
    assert!(!fx.core.home.path.join("store").exists());

    let applied = import::apply_choices(
        &fx.core,
        choices(
            vec![pick("remote", "claude-code", "remote")],
            vec![
                ImportSecretChoice {
                    name: "remote".to_owned(),
                    field: Some("headers.Authorization".to_owned()),
                    target: ImportSecretTarget::Onepassword {
                        reference: "op://Private/Remote MCP/credential".to_owned(),
                    },
                },
                ImportSecretChoice {
                    name: "remote".to_owned(),
                    field: None,
                    target: ImportSecretTarget::Env {
                        name: "REMOTE_API_KEY".to_owned(),
                    },
                },
            ],
        ),
    )
    .unwrap();
    assert!(!serde_json::to_string(&applied).unwrap().contains(PLANTED));
    let kinds: Vec<(&str, SecretDestinationKind)> = applied
        .secrets
        .iter()
        .map(|s| (s.field.as_str(), s.destination))
        .collect();
    assert_eq!(
        kinds,
        [
            ("headers.Authorization", SecretDestinationKind::Onepassword),
            ("headers.X-Api-Key", SecretDestinationKind::Env),
        ]
    );
    let server = fx.store_file("servers/remote.toml");
    assert!(
        server.contains("Authorization = \"Bearer ${secret:REMOTE_AUTHORIZATION}\""),
        "{server}"
    );
    let secrets = fx.store_file("secrets.toml");
    assert!(
        secrets.contains("op://Private/Remote MCP/credential"),
        "{secrets}"
    );
    assert!(secrets.contains("env:REMOTE_API_KEY"), "{secrets}");
    assert!(fx.keyring.peek("loadout", "REMOTE_AUTHORIZATION").is_none());
    assert_store_clean(&fx);
}

/// Every known project is scanned, trusted or not, and marked; project
/// files are never written. A definition found only in project files is
/// imported as a definition without enablement.
#[test]
fn known_projects_are_scanned_read_only() {
    let fx = Fx::new();
    let alpha = known_project(&fx, "alpha", "alpha-a1b2", true);
    let beta = known_project(&fx, "beta", "beta-c3d4", false);
    let pg = stdio("uvx", &["postgres-mcp"]);
    write(&alpha.join(".mcp.json"), &servers(&[("pg", &pg)]));
    write(
        &beta.join(".mcp.json"),
        &servers(&[("pg", &pg), ("beta-only", &stdio("beta-mcp", &[]))]),
    );
    let alpha_before = fx.read(&alpha.join(".mcp.json"));
    let beta_before = fx.read(&beta.join(".mcp.json"));
    let commits = fx.commits();

    let scan = import::scan(&fx.core, None).unwrap();
    fx.assert_no_root(&serde_json::to_string(&scan).unwrap());
    let pg_candidate = scan.candidates.iter().find(|c| c.name == "pg").unwrap();
    assert_eq!(pg_candidate.variants.len(), 1, "{pg_candidate:#?}");
    let variant = &pg_candidate.variants[0];
    assert!(!variant.enables, "project files only");
    let claude: Vec<(&str, Option<bool>, &str)> = variant
        .sources
        .iter()
        .filter(|s| s.client == "claude-code")
        .map(|s| (s.key.as_str(), s.trusted, s.file.as_str()))
        .collect();
    assert_eq!(
        claude,
        [
            ("claude-code@alpha-a1b2", Some(true), "<P>/.mcp.json"),
            ("claude-code@beta-c3d4", Some(false), "<P>/.mcp.json"),
        ]
    );

    let user = import::scan(&fx.core, Some(ImportScope::User)).unwrap();
    assert!(user.candidates.is_empty(), "{:#?}", user.candidates);
    let only_beta = import::scan(
        &fx.core,
        Some(ImportScope::Project {
            id: "beta-c3d4".to_owned(),
        }),
    )
    .unwrap();
    let names: Vec<&str> = only_beta
        .candidates
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["pg", "beta-only"]);
    let unknown = import::scan(
        &fx.core,
        Some(ImportScope::Project {
            id: "nope-0000".to_owned(),
        }),
    )
    .unwrap_err();
    assert_eq!(unknown.code, ErrorCode::NotFound);

    let applied = import::apply_choices(
        &fx.core,
        choices(vec![pick("pg", "claude-code@beta-c3d4", "pg")], Vec::new()),
    )
    .unwrap();
    assert_eq!(applied.imported, ["pg"]);
    assert_eq!(applied.definition_only, ["pg"]);
    assert_eq!(fx.commits(), commits + 1);
    let server = fx.store_file("servers/pg.toml");
    assert!(!server.contains("[enable]"), "{server}");
    assert_eq!(fx.read(&alpha.join(".mcp.json")), alpha_before);
    assert_eq!(fx.read(&beta.join(".mcp.json")), beta_before);
}

/// Review follow-up 1: each user file of a client is its own source
/// (`omp`, `omp#work`). Importing the default profile's definition leaves
/// the work profile's different entry untouched and still unmanaged.
#[test]
fn another_profile_holding_another_definition_is_left_alone() {
    let fx = Fx::new();
    fx.stage(&[(
        "clients.toml",
        "schema = 1\n\n[omp]\nprofiles = [\"default\", \"work\"]\n",
    )]);
    let work = fx
        .th
        .home()
        .join(".omp")
        .join("profiles")
        .join("work")
        .join("agent")
        .join("mcp.json");
    write(
        &omp(&fx),
        &servers(&[("docs", &stdio("npx", &["-y", "@example/docs-mcp"]))]),
    );
    write(
        &work,
        &servers(&[(
            "docs",
            &stdio("npx", &["-y", "@example/docs-mcp", "--work"]),
        )]),
    );
    let work_before = fx.read(&work);

    let scan = import::scan(&fx.core, Some(ImportScope::User)).unwrap();
    let docs = scan.candidates.iter().find(|c| c.name == "docs").unwrap();
    assert!(docs.conflict, "{docs:#?}");
    let keys: Vec<&str> = docs
        .variants
        .iter()
        .flat_map(|v| v.sources.iter().map(|s| s.key.as_str()))
        .collect();
    assert_eq!(keys, ["omp", "omp#work"]);
    let ids = |summary: &import::UnmanagedSummary| -> Vec<String> {
        summary
            .groups
            .iter()
            .flat_map(|g| g.entries.iter().map(|e| e.id.clone()))
            .collect()
    };
    let before = import::summary(&fx.core, None, false).unwrap();
    assert_eq!(ids(&before), ["omp/docs", "omp#work/docs"]);

    let applied = import::apply_choices(
        &fx.core,
        choices(vec![pick("docs", "omp", "docs")], Vec::new()),
    )
    .unwrap();
    assert_eq!(applied.imported, ["docs"]);
    assert_eq!(
        fx.read(&work),
        work_before,
        "the other profile is not taken over"
    );
    let after = import::summary(&fx.core, None, false).unwrap();
    assert_eq!(ids(&after), ["omp#work/docs"]);
}

/// Review follow-up 2: the import's apply plans user files only, so a
/// trusted project whose manifest names the imported server keeps its own
/// different foreign entry.
#[test]
fn a_project_entry_of_the_same_name_is_left_alone() {
    let fx = Fx::new();
    let root = fx.th.projects().join("gamma");
    write(
        &root.join(".loadout").join("project.toml"),
        "schema = 1\nid = \"gamma-e5f6\"\nname = \"gamma\"\n\n[servers.docs]\nfrom = \"docs\"\n\
         args = [\"--project\"]\nclients = [\"claude-code\"]\n",
    );
    let fs = fx.core.fs.as_ref();
    let project = Project::load(fs, &root).unwrap().unwrap();
    core_projects::add(&fx.store(), &fx.core.apply_env(), &project).unwrap();
    let state = fx.core.home.state_dir();
    let mut trust = TrustStore::load(fs, &state).unwrap();
    trust.trust(&project);
    trust.save(fs, &state).unwrap();
    write(
        &root.join(".mcp.json"),
        &servers(&[("docs", &stdio("other-docs-mcp", &[]))]),
    );
    write(
        &fx.claude(),
        &servers(&[("docs", &stdio("npx", &["-y", "@example/docs-mcp"]))]),
    );
    let project_before = fx.read(&root.join(".mcp.json"));
    let claude_before = fx.read(&fx.claude());

    let applied = import::apply_choices(
        &fx.core,
        choices(vec![pick("docs", "claude-code", "docs")], Vec::new()),
    )
    .unwrap();
    assert_eq!(applied.imported, ["docs"]);
    assert_eq!(fx.read(&root.join(".mcp.json")), project_before);
    assert_eq!(
        fx.read(&fx.claude()),
        claude_before,
        "adopted without a write"
    );
}

/// The unmanaged summary groups findings per client and scope with
/// placeholder paths; acknowledged findings stop being new, in machine
/// state only.
#[test]
fn unmanaged_summary_and_acknowledgments() {
    let fx = Fx::new();
    write(
        &fx.claude(),
        &servers(&[
            ("docs", &stdio("docs-mcp", &[])),
            ("notes", &stdio("notes-mcp", &[])),
        ]),
    );
    write(&omp(&fx), &servers(&[("docs", &stdio("docs-mcp", &[]))]));

    let summary = import::summary(&fx.core, None, false).unwrap();
    fx.assert_no_root(&serde_json::to_string(&summary).unwrap());
    assert_eq!((summary.total, summary.new), (3, 3));
    let claude = summary
        .groups
        .iter()
        .find(|g| g.client == "claude-code")
        .unwrap();
    assert_eq!(claude.files, ["~/.claude.json"]);
    let ids: Vec<&str> = claude.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["claude-code/docs", "claude-code/notes"]);

    let added = import::acknowledge(
        &fx.core,
        &["claude-code/docs".to_owned(), "omp/docs".to_owned()],
    )
    .unwrap();
    assert_eq!(added, 2);
    assert_eq!(
        import::acknowledge(&fx.core, &["omp/docs".to_owned()]).unwrap(),
        0
    );
    assert!(fx.core.home.state_dir().join(SEEN_FILE).exists());
    assert!(
        !fx.core.home.path.join("store").exists(),
        "acknowledgments are machine state, not store content"
    );

    let fresh = import::summary(&fx.core, None, true).unwrap();
    assert_eq!((fresh.total, fresh.new), (1, 1));
    assert_eq!(fresh.groups.len(), 1);
    assert_eq!(fresh.groups[0].entries[0].id, "claude-code/notes");
    let all = import::summary(&fx.core, Some(ImportScope::User), false).unwrap();
    assert_eq!((all.total, all.new), (3, 1));
}

//! Store schemas and validation (SPEC 5, milestone M1).
//!
//! Fixtures live in `fixtures/store/`: `valid/` is a complete store; every
//! file under `invalid/<code>/` is laid out like a store file and must fail
//! validation with exactly that error code.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use agent_loadout_core::ErrorCode;
use agent_loadout_core::model::applied::AppliedRecord;
use agent_loadout_core::model::clients::{ClientPrefsFile, ClientSelection};
use agent_loadout_core::model::ids::{self, IdKind};
use agent_loadout_core::model::machine::MachineOverlay;
use agent_loadout_core::model::project::{ManifestLayer, ProjectManifest};
use agent_loadout_core::model::secrets::SecretCatalog;
use agent_loadout_core::model::server::Server;
use agent_loadout_core::model::settings::Settings;
use agent_loadout_core::model::skill_meta::SkillMeta;
use agent_loadout_core::model::{
    Severity, StoreDocument, StoreFile, StoreModel, TomlDocument, json_schema, validate_store_file,
};
use agent_loadout_testkit::TempHome;

/// Every error code store-file validation emits; each has a directory of
/// invalid fixtures.
const VALIDATION_ERROR_CODES: [ErrorCode; 10] = [
    ErrorCode::ParseError,
    ErrorCode::UnsupportedSchema,
    ErrorCode::MissingField,
    ErrorCode::InvalidValue,
    ErrorCode::InvalidId,
    ErrorCode::IdMismatch,
    ErrorCode::FieldNotAllowed,
    ErrorCode::PassthroughNotAllowed,
    ErrorCode::InvalidRef,
    ErrorCode::InvalidPath,
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn fixtures() -> PathBuf {
    repo_root().join("fixtures").join("store")
}

/// Files below `dir`, as paths relative to `dir`, sorted.
fn files_below(dir: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn classify(relative: &Path) -> StoreFile {
    StoreFile::classify(relative)
        .unwrap_or_else(|| panic!("not a store schema file: {}", relative.display()))
}

/// Id of the `project.toml` next to a `project.local.toml`, if there is one.
fn sibling_shared_id(store: &Path, relative: &Path) -> Option<String> {
    let shared = store.join(relative).with_file_name("project.toml");
    let text = std::fs::read_to_string(shared).ok()?;
    TomlDocument::<ProjectManifest>::parse(&text)
        .ok()?
        .into_value()
        .id
}

fn validate_fixture(store: &Path, relative: &Path) -> Vec<(Severity, ErrorCode, String)> {
    let file = classify(relative);
    let shared = sibling_shared_id(store, relative);
    validate_store_file(&file, &read(&store.join(relative)), shared.as_deref())
        .into_iter()
        .map(|d| (d.severity, d.code, d.to_string()))
        .collect()
}

#[test]
fn each_invalid_fixture_yields_its_expected_code() {
    let invalid = fixtures().join("invalid");
    let mut seen = BTreeSet::new();
    let mut checked = 0;
    for code_dir in std::fs::read_dir(&invalid).unwrap() {
        let code_dir = code_dir.unwrap().path();
        let name = code_dir.file_name().unwrap().to_str().unwrap().to_owned();
        let expected = VALIDATION_ERROR_CODES
            .into_iter()
            .find(|code| code.as_str() == name)
            .unwrap_or_else(|| panic!("fixtures/store/invalid/{name} is not a validation code"));
        seen.insert(name.clone());
        for relative in files_below(&code_dir) {
            let found = validate_fixture(&code_dir, &relative);
            let errors: Vec<_> = found
                .iter()
                .filter(|(s, ..)| *s == Severity::Error)
                .collect();
            assert!(
                !errors.is_empty(),
                "{name}/{}: expected {expected}, got no errors ({found:?})",
                relative.display()
            );
            for (_, code, message) in &errors {
                assert_eq!(
                    *code,
                    expected,
                    "{name}/{}: unexpected error {message}",
                    relative.display()
                );
            }
            checked += 1;
        }
    }
    let wanted: BTreeSet<String> = VALIDATION_ERROR_CODES
        .iter()
        .map(|code| code.as_str().to_owned())
        .collect();
    assert_eq!(
        seen, wanted,
        "every validation code needs a fixture directory"
    );
    assert!(checked >= 40, "only {checked} invalid fixtures");
}

#[test]
fn valid_fixtures_have_no_errors() {
    let store = fixtures().join("valid");
    let files = files_below(&store);
    assert!(files.len() >= 13, "valid store is incomplete: {files:?}");
    for relative in files {
        let found = validate_fixture(&store, &relative);
        let errors: Vec<_> = found
            .iter()
            .filter(|(s, ..)| *s == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{}: {errors:?}", relative.display());
        // Only the fixture about unknown clients carries warnings.
        if !relative.ends_with("beta-tools.toml") {
            assert!(found.is_empty(), "{}: {found:?}", relative.display());
        }
    }
}

#[test]
fn load_then_save_of_every_valid_fixture_is_byte_identical() {
    let store = fixtures().join("valid");
    for relative in files_below(&store) {
        let file = classify(&relative);
        // Normalize whatever the checkout did to line endings, then test both.
        let lf = read(&store.join(&relative)).replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        let bom = format!("\u{feff}{crlf}");
        for (variant, text) in [("lf", &lf), ("crlf", &crlf), ("bom+crlf", &bom)] {
            let doc = StoreDocument::parse(&file, text)
                .unwrap_or_else(|d| panic!("{} ({variant}): {d}", relative.display()));
            let saved = doc.to_text().unwrap();
            assert!(
                saved == *text,
                "{} ({variant}) changed on save:\n--- original\n{text}\n--- saved\n{saved}",
                relative.display()
            );
        }
    }
}

#[test]
fn typed_rewrite_without_the_original_document_keeps_every_value() {
    fn check<T: StoreModel + Clone + PartialEq + std::fmt::Debug>(text: &str, name: &Path) {
        let value = TomlDocument::<T>::parse(text).unwrap().into_value();
        let rewritten = TomlDocument::new(value.clone()).to_text().unwrap();
        let reparsed = TomlDocument::<T>::parse(&rewritten)
            .unwrap_or_else(|d| panic!("{}: {d}\n{rewritten}", name.display()))
            .into_value();
        assert_eq!(reparsed, value, "{}:\n{rewritten}", name.display());
    }
    let store = fixtures().join("valid");
    for relative in files_below(&store) {
        let text = read(&store.join(&relative));
        match classify(&relative) {
            StoreFile::Server { .. } => check::<Server>(&text, &relative),
            StoreFile::Secrets => check::<SecretCatalog>(&text, &relative),
            StoreFile::Clients => check::<ClientPrefsFile>(&text, &relative),
            StoreFile::Settings => check::<Settings>(&text, &relative),
            StoreFile::SkillMeta { .. } => check::<SkillMeta>(&text, &relative),
            StoreFile::Project { .. } => check::<ProjectManifest>(&text, &relative),
            StoreFile::Machine { .. } => check::<MachineOverlay>(&text, &relative),
            StoreFile::Applied { .. } => check::<AppliedRecord>(&text, &relative),
        }
    }
}

#[test]
fn private_only_project_local_manifest_with_id_validates() {
    let text = read(
        &fixtures()
            .join("valid")
            .join("projects")
            .join("private-only-q3x9")
            .join("project.local.toml"),
    );
    let manifest = TomlDocument::<ProjectManifest>::parse(&text)
        .unwrap()
        .into_value();
    let private = ManifestLayer::Local { shared_id: None };
    let found = manifest.validate(private, Some("private-only-q3x9"));
    assert!(found.is_empty(), "{found:?}");

    // The same file without an id is only valid next to a project.toml.
    let mut anonymous = manifest.clone();
    anonymous.id = None;
    let found = anonymous.validate(private, None);
    assert_eq!(
        found
            .errors()
            .map(|d| (d.code, d.path.as_str()))
            .collect::<Vec<_>>(),
        [(ErrorCode::MissingField, "id")]
    );
    let layered = ManifestLayer::Local {
        shared_id: Some("private-only-q3x9"),
    };
    assert!(anonymous.validate(layered, None).is_empty());

    // A local id that disagrees with the shared one is a warning; shared wins.
    let found = manifest.validate(
        ManifestLayer::Local {
            shared_id: Some("other-project-0000"),
        },
        None,
    );
    assert!(found.is_valid());
    assert_eq!(
        found.warnings().map(|d| d.code).collect::<Vec<_>>(),
        [ErrorCode::IdMismatch]
    );
}

#[test]
fn unknown_enable_client_is_a_warning_and_survives_a_rewrite() {
    let text = read(
        &fixtures()
            .join("valid")
            .join("servers")
            .join("beta-tools.toml"),
    );
    let mut doc = TomlDocument::<Server>::parse(&text).unwrap();

    let found = doc.value().validate(Some("beta-tools"));
    assert!(found.is_valid(), "{found:?}");
    let warnings: Vec<_> = found
        .warnings()
        .map(|d| (d.code, d.path.as_str()))
        .collect();
    assert_eq!(
        warnings,
        [
            (ErrorCode::UnknownClient, "client.future-client"),
            (ErrorCode::UnknownClient, "enable.clients[1]"),
        ]
    );

    // An unrelated edit keeps the unknown id, its comment and its table.
    doc.value_mut().body.description = Some("Edited".to_owned());
    let saved = doc.to_text().unwrap();
    let expected = text.replace(
        "description = \"Enabled for a client this build does not know yet\"",
        "description = \"Edited\"",
    );
    assert_eq!(saved, expected);

    // An edit of the list itself keeps the unknown id too.
    let Some(ClientSelection::List(clients)) = doc
        .value_mut()
        .enable
        .as_mut()
        .and_then(|enable| enable.clients.as_mut())
    else {
        panic!("expected a client list");
    };
    clients.push("codex".to_owned());
    let saved = doc.to_text().unwrap();
    assert!(
        saved.contains("clients = [\"claude-code\", \"future-client\", \"codex\"]"),
        "{saved}"
    );
    assert!(saved.contains("# \"future-client\" comes from a newer Loadout"));
    let reparsed = TomlDocument::<Server>::parse(&saved).unwrap();
    assert_eq!(reparsed.value(), doc.value());
}

#[test]
fn edits_keep_comments_formatting_and_unknown_keys() {
    let text =
        read(&fixtures().join("valid").join("servers").join("github.toml")).replace("\r\n", "\n");
    let mut doc = TomlDocument::<Server>::parse(&text).unwrap();
    let server = doc.value_mut();
    server
        .body
        .env
        .as_mut()
        .unwrap()
        .insert("GITHUB_TOOLSETS".to_owned(), "repos".to_owned());
    server.body.tags = None;
    server
        .body
        .env
        .as_mut()
        .unwrap()
        .insert("GITHUB_HOST".to_owned(), "${env:GH_HOST}".to_owned());
    // A removed table takes its header comment and the comment lines directly
    // above it (toml_edit attaches them to the header).
    server.body.tools = None;

    let saved = doc.to_text().unwrap();
    let expected = text
        .replace("tags = [\"dev\"]\n", "")
        // The changed value keeps its spacing and trailing comment.
        .replace(
            "GITHUB_TOOLSETS = \"repos,issues\"",
            "GITHUB_TOOLSETS = \"repos\"",
        )
        .replace(
            "# literal non-secret values are fine\n",
            "# literal non-secret values are fine\nGITHUB_HOST = \"${env:GH_HOST}\"\n",
        )
        .replace(
            "\n[tools]                             # written only via native fields\n\
             deny = [\"delete_repository\"]\n",
            "",
        );
    assert_eq!(saved, expected);
    assert!(doc.value().validate(Some("github")).is_empty());
}

#[test]
fn documents_load_and_save_through_the_injected_fs() {
    let th = TempHome::new();
    let fs = th.fs();
    let servers = th.root().join("store").join("servers");
    agent_loadout_core::Fs::create_dir_all(&fs, &servers).unwrap();
    let path = servers.join("linear.toml");
    let text = read(&fixtures().join("valid").join("servers").join("linear.toml"));
    agent_loadout_core::Fs::write(&fs, &path, text.as_bytes()).unwrap();

    let mut doc = TomlDocument::<Server>::load(&fs, &path).unwrap();
    doc.value_mut().body.url = Some("https://mcp.example.com/v2/mcp".to_owned());
    doc.save(&fs, &path).unwrap();
    let saved = agent_loadout_core::Fs::read_to_string(&fs, &path).unwrap();
    assert_eq!(
        saved,
        text.replace(
            "https://mcp.example.com/mcp",
            "https://mcp.example.com/v2/mcp"
        )
    );

    let missing = TomlDocument::<Server>::load(&fs, &servers.join("absent.toml")).unwrap_err();
    assert_eq!(missing.code(), ErrorCode::IoError);

    agent_loadout_core::Fs::write(&fs, &path, b"schema = 1\nid = ").unwrap();
    let broken = TomlDocument::<Server>::load(&fs, &path).unwrap_err();
    assert_eq!(broken.code(), ErrorCode::ParseError);
    assert_eq!(broken.path(), Some(path.as_path()));
}

#[test]
fn parse_errors_point_at_the_field() {
    let cases: [(&str, ErrorCode, &str); 7] = [
        (
            "schema = 1\nid = \"x\"\ntransport = \"ws\"\n",
            ErrorCode::InvalidValue,
            "transport",
        ),
        (
            "schema = 1\nid = \"x\"\ncommand = \"npx\"\n[env]\nPORT = 8080\n",
            ErrorCode::InvalidValue,
            "env.PORT",
        ),
        (
            "schema = 1\nid = \"x\"\n[os.linux]\ncommand = false\n",
            ErrorCode::InvalidValue,
            "os.linux.command",
        ),
        (
            "schema = 1\ntransport = \"stdio\"\n",
            ErrorCode::MissingField,
            "id",
        ),
        (
            "schema = \"1\"\nid = \"x\"\n",
            ErrorCode::InvalidValue,
            "schema",
        ),
        (
            "schema = 1\nid = \"x\"\n[enable]\nclients = 3\n",
            ErrorCode::InvalidValue,
            "enable.clients",
        ),
        ("schema = 3\n", ErrorCode::UnsupportedSchema, "schema"),
    ];
    for (text, code, path) in cases {
        let err = TomlDocument::<Server>::parse(text).unwrap_err();
        assert_eq!(
            (err.code, err.path.as_str()),
            (code, path),
            "{text:?}: {err}"
        );
    }
}

#[test]
fn identifier_rules() {
    let long = "a".repeat(64);
    let too_long = "a".repeat(65);
    let cases: [(IdKind, &str, bool); 18] = [
        (IdKind::Server, "github", true),
        (IdKind::Server, "0x.mcp_server-2", true),
        (IdKind::Server, &long, true),
        (IdKind::Server, &too_long, false),
        (IdKind::Server, "", false),
        (IdKind::Server, "GitHub", false),
        (IdKind::Server, ".hidden", false),
        (IdKind::Project, "sample-app-7k2f", true),
        (IdKind::Skill, "repo conventions", false),
        (IdKind::SecretAlias, "GITHUB_PAT", true),
        (IdKind::SecretAlias, "_private2", true),
        (IdKind::SecretAlias, "2FA", false),
        (IdKind::SecretAlias, "has-dash", false),
        (IdKind::SecretAlias, &format!("A{too_long}"), false),
        (IdKind::Machine, "01JCX5B7K3M9N1P5Q7R3S9T1V5", true),
        (IdKind::Machine, "01jcx5b7k3m9n1p5q7r3s9t1v5", false),
        (IdKind::Machine, "81JCX5B7K3M9N1P5Q7R3S9T1V5", false),
        (IdKind::Apply, "01JCX5B7K3M9N1P5Q7R3S9T1VU", false),
    ];
    for (kind, value, valid) in cases {
        assert_eq!(ids::is_valid(kind, value), valid, "{kind:?} {value:?}");
        assert_eq!(ids::check(kind, value).is_ok(), valid);
    }
    assert_eq!(
        ids::check(IdKind::Server, "Bad").unwrap_err().code(),
        ErrorCode::InvalidId
    );
}

/// `docs/schema/*.json` must match the Rust types. Regenerate with
/// `LOADOUT_BLESS=1 cargo test -p agent-loadout-core --test model`.
#[test]
fn json_schemas_match_docs() {
    let schemas = [
        (Server::SCHEMA_NAME, json_schema::<Server>()),
        (SecretCatalog::SCHEMA_NAME, json_schema::<SecretCatalog>()),
        (
            ClientPrefsFile::SCHEMA_NAME,
            json_schema::<ClientPrefsFile>(),
        ),
        (Settings::SCHEMA_NAME, json_schema::<Settings>()),
        (SkillMeta::SCHEMA_NAME, json_schema::<SkillMeta>()),
        (
            ProjectManifest::SCHEMA_NAME,
            json_schema::<ProjectManifest>(),
        ),
        (MachineOverlay::SCHEMA_NAME, json_schema::<MachineOverlay>()),
        (AppliedRecord::SCHEMA_NAME, json_schema::<AppliedRecord>()),
    ];
    let dir = repo_root().join("docs").join("schema");
    let bless = std::env::var_os("LOADOUT_BLESS").is_some();
    if bless {
        std::fs::create_dir_all(&dir).unwrap();
    }
    let mut stale = Vec::new();
    for (name, generated) in schemas {
        let path = dir.join(format!("{name}.schema.json"));
        if bless {
            std::fs::write(&path, &generated).unwrap();
            continue;
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        if committed.replace("\r\n", "\n") != generated {
            stale.push(path.display().to_string());
        }
    }
    let on_disk: BTreeSet<String> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let expected: BTreeSet<String> = [
        Server::SCHEMA_NAME,
        SecretCatalog::SCHEMA_NAME,
        ClientPrefsFile::SCHEMA_NAME,
        Settings::SCHEMA_NAME,
        SkillMeta::SCHEMA_NAME,
        ProjectManifest::SCHEMA_NAME,
        MachineOverlay::SCHEMA_NAME,
        AppliedRecord::SCHEMA_NAME,
    ]
    .iter()
    .map(|name| format!("{name}.schema.json"))
    .collect();
    assert_eq!(on_disk, expected, "unexpected files in docs/schema");
    assert!(
        stale.is_empty(),
        "stale JSON Schemas (rerun with LOADOUT_BLESS=1): {stale:?}"
    );
}

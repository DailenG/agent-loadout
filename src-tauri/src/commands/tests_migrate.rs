//! M20 GUI: migration wizard commands against a temp home holding the
//! synthetic fixture `fixtures/skilldock-home` (SPEC 15.1, 19 item 1).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use agent_loadout_core::ErrorCode;
use agent_loadout_core::store::LogFilter;

use super::migrate::{self, MigrationItemStatus, MigrationOffer, MigrationSecretChoice};
use super::tests::Fx;

/// The fixture's plaintext values (SPEC 11.7): none may reach an IPC reply.
const PLANTED: [&str; 4] = [
    "fixture-header-value",
    "fixture-env-value",
    "fixture-arg-value",
    "fixture-remote-env-value",
];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/skilldock-home")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Every file below `dir`, by relative path, with its bytes.
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                out.insert(format!("{relative}/"), Vec::new());
                walk(root, &path, out);
            } else {
                out.insert(relative, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn setup() -> (Fx, PathBuf) {
    let fx = Fx::new();
    copy_dir(&fixtures(), &fx.th.home());
    let source = fx.th.home().join(".skilldock");
    (fx, source)
}

fn assert_clean(fx: &Fx, json: &str) {
    for value in PLANTED {
        assert!(!json.contains(value), "a plaintext value in {json}");
    }
    let root = fx.th.root().to_string_lossy().into_owned();
    for form in [
        root.clone(),
        root.replace('\\', "/"),
        root.replace('\\', "\\\\"),
    ] {
        assert!(!json.contains(&form), "the temp root in {json}");
    }
}

fn commits(fx: &Fx) -> Vec<String> {
    match fx.core.open_store().unwrap() {
        Some(store) => store
            .log(&LogFilter::default())
            .unwrap()
            .into_iter()
            .map(|entry| entry.summary)
            .collect(),
        None => Vec::new(),
    }
}

/// The dry run reads `~/.skilldock` only and creates nothing; the report
/// lists every item with placeholders, routes plaintext values to
/// extractions by alias, and holds no value.
#[test]
fn the_dry_run_reports_and_creates_nothing() {
    let (fx, source) = setup();
    let before = tree(&fx.th.home());
    let report = migrate::plan(&fx.core).unwrap();
    assert_eq!(tree(&fx.th.home()), before, "the home is byte-identical");
    assert!(!fx.core.home.path.join("store").exists(), "no store");
    assert!(!fx.core.home.state_dir().exists(), "no state");

    assert_eq!(report.source, "~/.skilldock");
    assert!(
        report.counts.migrated > 0 && report.counts.needs_review > 0 && report.counts.skipped > 0
    );
    assert_eq!(
        report.items.len() as u32,
        report.counts.migrated + report.counts.needs_review + report.counts.skipped
    );
    assert!(!report.files.is_empty());
    assert!(report.extractions.len() >= 3);
    for extraction in &report.extractions {
        assert!(
            extraction.suggested_ref.starts_with("keyring://"),
            "{extraction:?}"
        );
    }
    let wrappers: Vec<_> = report
        .items
        .iter()
        .flat_map(|item| &item.offers)
        .filter(|offer| matches!(offer, MigrationOffer::ReplaceWrapper { .. }))
        .collect();
    assert!(
        wrappers.contains(&&MigrationOffer::ReplaceWrapper {
            client: "codex".to_owned(),
            name: "context7".to_owned(),
        }),
        "{wrappers:?}"
    );
    assert!(
        report
            .items
            .iter()
            .any(|item| item.status == MigrationItemStatus::NeedsReview)
    );
    assert_clean(&fx, &serde_json::to_string(&report).unwrap());
    drop(source);
}

/// The migration and the extraction choices are one commit after the
/// pre-migrate tag; the whole home (client files and `~/.skilldock`) stays
/// byte-identical; the keyring value goes one way; "Replace SkillDock
/// wrapper" rewrites only Codex's file, and refuses anything else.
#[test]
fn the_migration_is_one_commit_and_leaves_skilldock_untouched() {
    let (fx, source) = setup();
    let home = fx.th.home();
    let before = tree(&home);
    let skilldock = tree(&source);
    let plan = migrate::plan(&fx.core).unwrap();
    let first = plan.extractions[0].alias.clone();
    let second = plan.extractions[1].alias.clone();
    let value = ["fixture-", "typed-", "value"].concat();
    let choices = vec![
        MigrationSecretChoice::Keyring {
            alias: first.clone(),
            value: value.clone(),
        },
        MigrationSecretChoice::Env {
            alias: second.clone(),
            name: "NOTES_TOKEN".to_owned(),
        },
    ];
    let applied = migrate::apply_migration(&fx.core, choices).unwrap();
    assert_eq!(
        tree(&home),
        before,
        "every client file and ~/.skilldock byte-identical"
    );
    let summaries = commits(&fx);
    let migrations: Vec<_> = summaries
        .iter()
        .filter(|s| s.starts_with("migrate: skilldock"))
        .collect();
    assert_eq!(migrations.len(), 1, "{summaries:?}");
    assert_eq!(
        summaries[0], *migrations[0],
        "the migration is the last commit"
    );
    assert!(
        !summaries.iter().any(|s| s.starts_with("secrets:")),
        "the choices are in the migration commit: {summaries:?}"
    );
    assert!(applied.commit.is_some());
    assert!(applied.tag.starts_with("auto/pre-migrate/"));
    assert!(
        applied
            .post_tag
            .as_deref()
            .is_some_and(|tag| tag.starts_with("auto/post-migrate/"))
    );
    assert!(!applied.report.servers.is_empty() && !applied.report.skills.is_empty());
    let json = serde_json::to_string(&applied).unwrap();
    assert!(!json.contains(&value));
    assert_clean(&fx, &json);
    assert_eq!(
        fx.keyring.peek("loadout", &first).as_deref(),
        Some(value.as_str())
    );
    let store = fx.core.open_store().unwrap().unwrap();
    let catalog = store.read_to_string("secrets.toml").unwrap().unwrap();
    assert!(
        catalog.contains(&format!("keyring://loadout/{first}")),
        "{catalog}"
    );
    assert!(catalog.contains("env:NOTES_TOKEN"), "{catalog}");
    assert!(!catalog.contains(&value));
    drop(store);

    // Bad references and aliases that are not extractions are refused.
    let bad = |choice| {
        migrate::apply_migration(&fx.core, vec![choice])
            .unwrap_err()
            .code
    };
    for reference in [
        "op://vault/item",
        "op://vault//field",
        "op://a/b/c/d/e",
        "vault/item/field",
    ] {
        let choice = MigrationSecretChoice::Onepassword {
            alias: second.clone(),
            reference: reference.to_owned(),
        };
        assert_eq!(bad(choice), ErrorCode::InvalidValue, "{reference}");
    }
    let choice = MigrationSecretChoice::Env {
        alias: second.clone(),
        name: "NOT A NAME".to_owned(),
    };
    assert_ne!(
        migrate::apply_migration(&fx.core, vec![choice])
            .map(drop)
            .err()
            .map(|e| e.code),
        None
    );

    // A second run changes nothing.
    let again = migrate::apply_migration(&fx.core, Vec::new()).unwrap();
    assert!(again.commit.is_none());

    // Replace SkillDock wrapper: a blocked wrapper and an unknown name are
    // refused with nothing written; Codex's collision is replaced and only
    // Codex's file changes.
    let err = migrate::replace_wrapper(&fx.core, "claude-code", "context7").unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidValue);
    let err = migrate::replace_wrapper(&fx.core, "codex", "no-such-entry").unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(tree(&home), before);
    let codex = home.join(".codex").join("config.toml");
    let change = migrate::replace_wrapper(&fx.core, "codex", "context7").unwrap();
    assert!(change.commit.is_some());
    let native = std::fs::read_to_string(&codex).unwrap();
    assert!(
        native.contains("context7") && !native.contains("mcp-filter"),
        "{native}"
    );
    let after = tree(&home);
    let changed: Vec<&String> = after
        .iter()
        .filter(|(path, bytes)| before.get(*path) != Some(*bytes))
        .map(|(path, _)| path)
        .collect();
    let codex_file = Path::new(".codex")
        .join("config.toml")
        .to_string_lossy()
        .into_owned();
    assert_eq!(changed, [&codex_file], "only Codex's file is written");
    assert_eq!(tree(&source), skilldock);
    assert_clean(&fx, &serde_json::to_string(&change).unwrap());
}

#[test]
fn no_skilldock_folder_is_not_found() {
    let fx = Fx::new();
    let err = migrate::plan(&fx.core).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = migrate::replace_wrapper(&fx.core, "", "x").unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidValue);
}

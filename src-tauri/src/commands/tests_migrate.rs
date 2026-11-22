//! M20 GUI: migration wizard commands against a temp home holding the
//! synthetic fixture `fixtures/skilldock-home` (SPEC 15.1, 19 item 1).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use agent_loadout_core::ErrorCode;
use agent_loadout_core::store::LogFilter;

use super::migrate::{self, MigrationItemStatus, MigrationOffer};
use super::secrets::{self, SecretScope};
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

/// The migration is one commit after the pre-migrate tag; the fixture tree
/// stays byte-identical; secrets set from the extraction step go one way;
/// "Replace SkillDock wrapper" rewrites the wrapped Codex entry natively.
#[test]
fn the_migration_is_one_commit_and_leaves_skilldock_untouched() {
    let (fx, source) = setup();
    let before = tree(&source);
    let applied = migrate::apply_migration(&fx.core).unwrap();
    assert_eq!(tree(&source), before, "~/.skilldock is byte-identical");
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
    assert!(applied.commit.is_some());
    assert!(applied.tag.starts_with("auto/pre-migrate/"));
    assert!(
        applied
            .post_tag
            .as_deref()
            .is_some_and(|tag| tag.starts_with("auto/post-migrate/"))
    );
    assert!(!applied.report.servers.is_empty() && !applied.report.skills.is_empty());
    assert_clean(&fx, &serde_json::to_string(&applied).unwrap());

    // Extraction step: a keyring value (one way) and an env reference.
    let first = &applied.report.extractions[0];
    let value = ["fixture-", "typed-", "value"].concat();
    let change =
        secrets::set_keyring_value(&fx.core, &first.alias, &value, SecretScope::Store).unwrap();
    let json = serde_json::to_string(&change).unwrap();
    assert!(!json.contains(&value));
    assert_eq!(
        fx.keyring.peek("loadout", &first.alias).as_deref(),
        Some(value.as_str())
    );
    let second = &applied.report.extractions[1];
    secrets::set_reference(
        &fx.core,
        &second.alias,
        "env:NOTES_TOKEN",
        SecretScope::Store,
        None,
    )
    .unwrap();
    assert_eq!(tree(&source), before);

    // A second run changes nothing.
    let again = migrate::apply_migration(&fx.core).unwrap();
    assert!(again.commit.is_none());

    // Replace SkillDock wrapper: Codex's wrapped entry becomes native.
    let codex = fx.th.home().join(".codex").join("config.toml");
    let wrapped = std::fs::read_to_string(&codex).unwrap();
    assert!(wrapped.contains("mcp-filter"), "{wrapped}");
    let change = migrate::replace_wrapper(&fx.core, "codex", "context7").unwrap();
    assert!(change.commit.is_some());
    let native = std::fs::read_to_string(&codex).unwrap();
    assert!(
        native.contains("context7") && !native.contains("mcp-filter"),
        "{native}"
    );
    assert_eq!(tree(&source), before);
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

//! Migration from SkillDock (SPEC 15.1): the synthetic
//! `fixtures/skilldock-home` (a home folder with `~/.skilldock` and the
//! client files SkillDock wrote) migrates to a golden store; `~/.skilldock`
//! is never changed; SkillDock's wrappers are offered for replacement;
//! plaintext values go to the extraction wizard and never reach the store;
//! the report lists every item. Everything lives in a `TempHome`.

mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_loadout_core::adapters::detect::NoRegistry;
use agent_loadout_core::apply::{Action, Decision, EntryState, OnConflict, PlanOptions};
use agent_loadout_core::migrate::{self, ItemKind, ItemStatus, Offer, Report};
use agent_loadout_core::remote::git::SystemRunner;
use agent_loadout_core::skills::link::{self, SkillLedger};
use agent_loadout_core::skills::{LinkKind, LinkStatus, SkillsEnv};
use agent_loadout_core::store::tags::TagKind;
use sha2::{Digest, Sha256};
use support::{Fx, copy_dir, fixtures};

static RUNNER: SystemRunner = SystemRunner {
    idle_timeout: Duration::from_secs(60),
};
static NO_REGISTRY: NoRegistry = NoRegistry;

/// Values written in plain text in the fixture; none may reach the store.
const PLAINTEXT: [&str; 4] = [
    "fixture-env-value",
    "fixture-arg-value",
    "fixture-header-value",
    "fixture-placeholder",
];

struct Setup {
    fx: Fx,
    /// HEAD of `~/.skilldock/skills/team-review` (a git repository).
    team_review_head: String,
    /// HEAD of `~/.skilldock/backup/repo`.
    backup_head: String,
}

fn skilldock(fx: &Fx) -> PathBuf {
    fx.th.home().join(".skilldock")
}

/// A git repository at `dir` on `main` with every file committed once,
/// with a fixed signature so the commit id does not depend on the clock.
fn git_repo(dir: &Path, origin: Option<&str>) -> String {
    let mut options = git2::RepositoryInitOptions::new();
    options.initial_head("main").external_template(false);
    let repo = git2::Repository::init_opts(dir, &options).unwrap();
    if let Some(url) = origin {
        repo.remote("origin", url).unwrap();
    }
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let signature =
        git2::Signature::new("test", "test@example.invalid", &git2::Time::new(0, 0)).unwrap();
    repo.commit(Some("HEAD"), &signature, &signature, "fixture", &tree, &[])
        .unwrap()
        .to_string()
}

/// A directory link at `link` to `target`: a junction on Windows, a
/// symbolic link elsewhere.
fn dir_link(target: &Path, link: &Path) {
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    #[cfg(windows)]
    junction::create(target, link).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
}

/// The fixture home, plus what a repository cannot hold: the git
/// repositories, a link from Claude Code's skill folder into SkillDock's
/// library, and a skill holding a token-shaped value (assembled here).
fn setup() -> Setup {
    let fx = Fx::new();
    copy_dir(&fixtures().join("skilldock-home"), &fx.th.home());
    let sd = skilldock(&fx);
    let team_review_head = git_repo(
        &sd.join("skills").join("team-review"),
        Some("https://example.invalid/team/team-review.git"),
    );
    let backup = sd.join("backup").join("repo");
    std::fs::create_dir_all(backup.join("mcp")).unwrap();
    std::fs::write(
        backup.join("mcp").join("servers.json"),
        "{\"servers\": []}\n",
    )
    .unwrap();
    let backup_head = git_repo(&backup, None);
    let leaky = sd.join("skills").join("leaky");
    std::fs::create_dir_all(&leaky).unwrap();
    let token = ["gh", "p_", "FAKE0000000000000000000000000000000000"].concat();
    std::fs::write(
        leaky.join("SKILL.md"),
        format!("---\nname: leaky\n---\n\nexport GH={token}\n"),
    )
    .unwrap();
    dir_link(
        &sd.join("skills").join("repo-notes"),
        &fx.th
            .home()
            .join(".claude")
            .join("skills")
            .join("repo-notes"),
    );
    Setup {
        fx,
        team_review_head,
        backup_head,
    }
}

/// A hash of every path, file and link target below `dir` (links are not
/// followed).
fn tree_hash(dir: &Path) -> String {
    fn walk(root: &Path, dir: &Path, hasher: &mut Sha256) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            let relative = path.strip_prefix(root).unwrap().to_string_lossy();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            hasher.update(relative.as_bytes());
            if meta.file_type().is_symlink() {
                hasher.update(b"link");
                hasher.update(
                    std::fs::read_link(&path)
                        .unwrap()
                        .to_string_lossy()
                        .as_bytes(),
                );
            } else if meta.is_dir() {
                hasher.update(b"dir");
                walk(root, &path, hasher);
            } else {
                hasher.update(b"file");
                hasher.update(std::fs::read(&path).unwrap());
                hasher.update(meta.len().to_le_bytes());
            }
        }
    }
    let mut hasher = Sha256::new();
    walk(dir, dir, &mut hasher);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Every store file except `.git` and the store identity (a fresh ULID),
/// as `== path ==` blocks; line endings as LF (the fixture checks out with
/// CRLF where git converts them).
fn store_tree(fx: &Fx, redact: &[(&str, &str)]) -> String {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if relative == ".git" || relative == "loadout.toml" {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let text = std::fs::read_to_string(&path)
                    .unwrap()
                    .replace("\r\n", "\n");
                out.push((relative, text));
            }
        }
    }
    let root = fx.home.store_dir();
    let mut files = Vec::new();
    walk(&root, &root, &mut files);
    files.sort();
    let mut text = String::new();
    for (path, content) in files {
        text.push_str(&format!("== {path} ==\n{content}"));
        if !content.ends_with('\n') {
            text.push('\n');
        }
    }
    for (from, to) in redact {
        text = text.replace(from, to);
    }
    text
}

/// The report as one line per item, then the extractions.
fn report_text(report: &Report) -> String {
    let mut text = String::new();
    for item in &report.items {
        text.push_str(&format!(
            "[{}] {} {}{}: {}{}\n",
            item.status.as_str(),
            item.kind.as_str(),
            item.name,
            item.id
                .as_ref()
                .map_or_else(String::new, |id| format!(" -> {id}")),
            item.detail,
            item.code
                .map_or_else(String::new, |code| format!(" ({code})")),
        ));
        for offer in &item.offers {
            text.push_str(&format!(
                "    offer: {}\n",
                serde_json::to_string(offer).unwrap()
            ));
        }
    }
    for extraction in &report.extractions {
        text.push_str(&format!(
            "extract {} {} -> ${{secret:{}}} ({}, default {})\n",
            extraction.server,
            extraction.field,
            extraction.alias,
            extraction.reason,
            extraction.suggested_ref
        ));
    }
    text
}

fn item<'a>(report: &'a Report, kind: ItemKind, name: &str) -> &'a migrate::Item {
    report
        .items
        .iter()
        .find(|item| item.kind == kind && item.name == name)
        .unwrap_or_else(|| panic!("no {kind:?} item {name:?} in\n{}", report_text(report)))
}

#[test]
fn the_dry_run_creates_nothing_and_lists_every_item() {
    let s = setup();
    let fx = &s.fx;
    let before = tree_hash(&skilldock(fx));
    let claude = fx.read(&fx.claude());
    let migration = migrate::plan(None, &fx.ax()).unwrap();
    assert_eq!(tree_hash(&skilldock(fx)), before, "~/.skilldock changed");
    assert_eq!(fx.read(&fx.claude()), claude);
    assert!(
        !fx.home.store_dir().exists() && !fx.home.state_dir().exists(),
        "the dry run created Loadout data"
    );
    let report = &migration.report;
    assert_eq!(report.source, "~/.skilldock");

    // Every top-level entry of ~/.skilldock, and every record and skill
    // folder, is in the report.
    let names: Vec<&str> = report.items.iter().map(|i| i.name.as_str()).collect();
    for record in [
        "context7",
        "fetch",
        "Remote-Docs",
        "notes",
        "time",
        "plugin-helper",
        "broken",
    ] {
        assert!(
            names.contains(&record),
            "no item for record {record:?}: {names:?}"
        );
    }
    for skill in ["repo-notes", "team-review", "leaky", "retired-skill"] {
        assert!(
            names.contains(&skill),
            "no item for skill {skill:?}: {names:?}"
        );
    }
    for path in [
        "~/.skilldock/credentials/github.json",
        "~/.skilldock/credentials/skillhub.json",
        "~/.skilldock/data/publishing",
        "~/.skilldock/plugins",
        "~/.skilldock/logs",
        "~/.skilldock/backup/repo",
        "~/.skilldock/backup/last-sync.json",
        "~/.skilldock/skills/README.txt",
    ] {
        assert!(names.contains(&path), "no item for {path}: {names:?}");
    }
    for key in [
        "theme",
        "agentSkillsCompatibilityEnabled",
        "skillInstallActivation",
        "mcpInstallActivation",
        "defaultOpenToolId",
        "language",
        "githubBackup",
    ] {
        item(report, ItemKind::Setting, key);
    }
    assert_eq!(
        item(report, ItemKind::Server, "plugin-helper").status,
        ItemStatus::Skipped
    );
    assert_eq!(
        item(report, ItemKind::Server, "broken").status,
        ItemStatus::NeedsReview
    );
    let leaky = item(report, ItemKind::Skill, "leaky");
    assert_eq!(leaky.status, ItemStatus::NeedsReview);
    assert_eq!(leaky.code, Some("leak_detected"));
    let credential = item(
        report,
        ItemKind::Credential,
        "~/.skilldock/credentials/github.json",
    );
    assert_eq!(credential.status, ItemStatus::NeedsReview);
    assert!(matches!(
        credential.offers[..],
        [Offer::KeyringToken { .. }]
    ));
    let notes = item(report, ItemKind::Server, "notes");
    assert!(notes.offers.contains(&Offer::MachinePath {
        server: "notes".to_owned()
    }));
    assert!(notes.detail.contains("legacy-ide"), "{}", notes.detail);
    // No report text holds a plaintext value or an absolute temp path.
    let text = report_text(report);
    for value in PLAINTEXT {
        assert!(!text.contains(value), "{value} in the report");
    }
    let root = fx.th.root().to_string_lossy().into_owned();
    assert!(!text.contains(&root), "an absolute path in the report");
    insta::assert_snapshot!("dry_run_report", text);
    assert!(migration.files().any(|f| f == "servers/context7.toml"));
}

#[test]
fn the_fixture_migrates_to_a_golden_store() {
    let s = setup();
    let fx = &s.fx;
    let store = fx.store();
    let head_before = store.head().unwrap();
    let commits_before = fx.commit_count();
    let before = tree_hash(&skilldock(fx));
    let claude = fx.read(&fx.claude());

    let applied = migrate::apply(&store, &fx.ax()).unwrap();

    assert_eq!(tree_hash(&skilldock(fx)), before, "~/.skilldock changed");
    assert_eq!(fx.read(&fx.claude()), claude, "a client file was written");
    // One commit, after the pre-migrate tag on the store as it was.
    assert_eq!(fx.commit_count(), commits_before + 1);
    let commit = applied.commit.clone().unwrap();
    let log = fx.log();
    assert_eq!(log[0].id, commit);
    assert_eq!(log[0].summary, "migrate: skilldock (5 servers, 2 skills)");
    let tag = store.tag(&applied.tag).unwrap().unwrap();
    assert!(
        applied.tag.starts_with("auto/pre-migrate/"),
        "{}",
        applied.tag
    );
    assert_eq!(tag.kind, TagKind::Auto);
    assert_eq!(tag.target, head_before);
    let post = store
        .tag(applied.post_tag.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert!(post.name.starts_with("auto/post-migrate/"), "{}", post.name);
    assert_eq!(post.target, commit);
    assert_eq!(
        applied.report.servers,
        ["context7", "fetch", "remote-docs", "notes", "time"]
    );
    assert_eq!(applied.report.skills, ["repo-notes", "team-review"]);

    // The golden store: servers, skills without `.git`, metadata, settings.
    let tree = store_tree(fx, &[(&s.team_review_head, "<team-review-head>")]);
    assert!(
        !tree.contains("== skills/team-review/.git"),
        "nested .git copied"
    );
    for value in PLAINTEXT {
        assert!(!tree.contains(value), "{value} reached the store");
    }
    assert!(
        !tree.contains("FAKE0000"),
        "the leaky skill reached the store"
    );
    assert!(
        tree.contains("commit = \"<team-review-head>\""),
        "no source commit"
    );
    insta::assert_snapshot!("golden_store", tree);

    // The backup repository, bare-cloned with its history.
    assert_eq!(
        applied.backup_import.as_deref(),
        Some("imports/skilldock-backup.git")
    );
    let bare =
        git2::Repository::open_bare(fx.home.imports_dir().join("skilldock-backup.git")).unwrap();
    assert!(bare.is_bare());
    assert_eq!(
        bare.head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string(),
        s.backup_head
    );

    // The link into SkillDock's library is owned now, and the next skills
    // sync re-points it to the store copy (an update).
    let link_path = fx
        .th
        .home()
        .join(".claude")
        .join("skills")
        .join("repo-notes");
    let ledger = SkillLedger::load(&fx.fs, &fx.home.state_dir()).unwrap();
    let owned = ledger.get(&link_path).expect("the link is owned");
    assert_eq!(owned.skill_id, "repo-notes");
    let sx = SkillsEnv {
        env: &fx.env,
        fs: &fx.fs,
        home: &fx.home,
        registry: &NO_REGISTRY,
        runner: &RUNNER,
        link_kind: LinkKind::Junction,
    };
    let report = link::sync(&store, &sx).unwrap();
    let result = report
        .results
        .iter()
        .find(|r| r.path == link_path)
        .unwrap_or_else(|| panic!("{report:#?}"));
    assert_eq!(result.status, LinkStatus::Updated);
    let target = std::fs::canonicalize(&link_path).unwrap();
    assert_eq!(
        target,
        std::fs::canonicalize(fx.home.store_dir().join("skills").join("repo-notes")).unwrap()
    );
    assert_eq!(
        tree_hash(&skilldock(fx)),
        before,
        "the sync changed ~/.skilldock"
    );
}

#[test]
fn skilldock_wrappers_are_offered_for_replacement() {
    let s = setup();
    let fx = &s.fx;
    let store = fx.store();
    let applied = migrate::apply(&store, &fx.ax()).unwrap();
    let report = &applied.report;

    let context7 = item(report, ItemKind::Server, "context7");
    assert!(context7.detail.contains("tools denied: get-library-docs"));
    let remote = item(report, ItemKind::Server, "Remote-Docs");
    assert!(
        remote
            .detail
            .contains("unwrapped from SkillDock's mcp-remote wrapper")
    );
    // Codex has native tool filters: its wrapped entry collides with the
    // native definition and is offered for replacement.
    let wrapped = item(report, ItemKind::ClientEntry, "codex/context7");
    assert_eq!(wrapped.status, ItemStatus::NeedsReview);
    assert_eq!(wrapped.code, Some("collision"));
    assert!(
        wrapped.detail.contains("mcp-filter wrapper"),
        "{}",
        wrapped.detail
    );
    assert_eq!(
        wrapped.offers,
        [Offer::ReplaceWrapper {
            client: "codex".to_owned(),
            name: "context7".to_owned(),
            command: "loadout adopt codex/context7".to_owned(),
        }]
    );
    // Claude Code has none: the wrapper is offered too, with why the
    // native definition cannot be written there yet.
    let blocked = item(report, ItemKind::ClientEntry, "claude-code/context7");
    assert_eq!(blocked.status, ItemStatus::NeedsReview);
    assert_eq!(
        blocked.code,
        Some("unsupported_field"),
        "{}",
        blocked.detail
    );
    assert!(
        blocked.detail.contains("mcp-filter wrapper"),
        "{}",
        blocked.detail
    );
    assert!(matches!(blocked.offers[..], [Offer::ReplaceWrapper { .. }]));
    // SkillDock managed `fetch` and it renders identically: adopted on the
    // first apply without a write. `time` was imported from Codex and is
    // identical there: the scan enabled it.
    let fetch = item(report, ItemKind::ClientEntry, "claude-code/fetch");
    assert_eq!(fetch.status, ItemStatus::Migrated, "{}", fetch.detail);
    let time = item(report, ItemKind::ClientEntry, "codex/time");
    assert_eq!(time.status, ItemStatus::Migrated, "{}", time.detail);
    assert!(
        fx.read(&fx.home.store_dir().join("servers").join("time.toml"))
            .contains("clients = [\"codex\"]")
    );

    // The first apply leaves the wrapper as a collision; replacing it
    // (`loadout adopt codex/context7`) writes the native entry with
    // Codex's own tool filter.
    let codex = fx.th.home().join(".codex").join("config.toml");
    let plan = fx.plan();
    let entry = support::entry(&plan, &codex, "context7");
    assert_eq!(entry.state, EntryState::Collision);
    assert!(entry.unresolved);
    assert_eq!(
        support::entry(&plan, &fx.claude(), "fetch").action,
        Action::Adopt
    );
    let (report, _) = fx.apply_with(PlanOptions {
        decisions: vec![Decision {
            client: "codex".to_owned(),
            name: "context7".to_owned(),
            resolution: OnConflict::Overwrite,
        }],
        ..PlanOptions::default()
    });
    let entry = support::result_entry(&report, &codex, "context7");
    assert_eq!(entry.action, Action::AdoptOverwrite, "{entry:#?}");
    let text = fx.read(&codex);
    assert!(!text.contains("mcp-filter"), "{text}");
    assert!(
        text.contains("args = [\"-y\", \"@upstash/context7-mcp\"]"),
        "{text}"
    );
    assert!(text.contains("\"get-library-docs\""), "{text}");
}

#[test]
fn plaintext_values_go_to_the_extraction_wizard() {
    let s = setup();
    let fx = &s.fx;
    let store = fx.store();
    let applied = migrate::apply(&store, &fx.ax()).unwrap();
    let report = &applied.report;
    let fields: Vec<(&str, &str, &str)> = report
        .extractions
        .iter()
        .map(|e| (e.server.as_str(), e.field.as_str(), e.alias.as_str()))
        .collect();
    assert_eq!(
        fields,
        [
            (
                "remote-docs",
                "headers.Authorization",
                "REMOTE_DOCS_AUTHORIZATION"
            ),
            ("notes", "args[2]", "NOTES_TOKEN"),
            ("notes", "env.NOTES_API_KEY", "NOTES_NOTES_API_KEY"),
        ]
    );
    for extraction in &report.extractions {
        assert_eq!(
            extraction.suggested_ref,
            format!("keyring://loadout/{}", extraction.alias)
        );
    }
    let notes = item(report, ItemKind::Server, "notes");
    assert_eq!(notes.status, ItemStatus::NeedsReview);
    assert!(notes.offers.contains(&Offer::SetSecret {
        alias: "NOTES_NOTES_API_KEY".to_owned(),
        command: "loadout secrets set NOTES_NOTES_API_KEY --keyring".to_owned(),
    }));
    // The store holds references, never the values; non-secrets stay.
    let text = fx.read(&fx.home.store_dir().join("servers").join("notes.toml"));
    assert!(
        text.contains("NOTES_API_KEY = \"${secret:NOTES_NOTES_API_KEY}\""),
        "{text}"
    );
    assert!(text.contains("\"${secret:NOTES_TOKEN}\""), "{text}");
    assert!(text.contains("LOG_LEVEL = \"info\""), "{text}");
    let remote = fx.read(&fx.home.store_dir().join("servers").join("remote-docs.toml"));
    assert!(
        remote.contains("Authorization = \"Bearer ${secret:REMOTE_DOCS_AUTHORIZATION}\""),
        "{remote}"
    );
    // No secret was stored anywhere (catalog or keyring): the alias is
    // missing until `secrets set`, so the entry is blocked, not written.
    assert!(fx.read_opt_catalog().is_none());
    assert!(fx.keyring.peek("loadout", "NOTES_NOTES_API_KEY").is_none());
    let blocked = item(report, ItemKind::ClientEntry, "claude-code/notes");
    assert_eq!(blocked.status, ItemStatus::NeedsReview);
    assert_eq!(blocked.code, Some("missing_secret"), "{}", blocked.detail);
    // The history holds none of the values either.
    let all = store_tree(fx, &[]);
    for value in PLAINTEXT {
        assert!(!all.contains(value));
    }
}

#[test]
fn a_second_migration_changes_nothing() {
    let s = setup();
    let fx = &s.fx;
    let store = fx.store();
    migrate::apply(&store, &fx.ax()).unwrap();
    let commits = fx.commit_count();
    let again = migrate::apply(&store, &fx.ax()).unwrap();
    assert_eq!(again.commit, None);
    assert_eq!(fx.commit_count(), commits);
    assert!(again.report.servers.is_empty() && again.report.skills.is_empty());
    let context7 = item(&again.report, ItemKind::Server, "context7");
    assert_eq!(context7.status, ItemStatus::Skipped);
    assert_eq!(context7.code, Some("already_exists"));
    let backup = item(&again.report, ItemKind::Backup, "~/.skilldock/backup/repo");
    assert_eq!(backup.status, ItemStatus::Skipped);
    assert_eq!(again.backup_import, None);
}

#[test]
fn no_skilldock_data_is_not_found() {
    let fx = Fx::new();
    let err = migrate::plan(None, &fx.ax()).unwrap_err();
    assert_eq!(err.code().as_str(), "not_found");
}

trait CatalogRead {
    fn read_opt_catalog(&self) -> Option<String>;
}

impl CatalogRead for Fx {
    fn read_opt_catalog(&self) -> Option<String> {
        std::fs::read_to_string(self.home.store_dir().join("secrets.toml")).ok()
    }
}

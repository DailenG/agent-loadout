//! Skills (SPEC 12, 17): store and frontmatter, installs from folders,
//! archives and local bare git repositories, links (junctions on Windows)
//! and copies in client skill folders, project links hidden through
//! `.git/info/exclude`, tool folder classification and the agents CLI
//! scan. Every path lives in a `TempHome`; git sources are local bare
//! repositories made with the system git (tests that need it skip with a
//! notice when it is absent).

mod support;

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_loadout_core::adapters::detect::NoRegistry;
use agent_loadout_core::formats::NativeValue;
use agent_loadout_core::layering::projects::exclude_manifest_dir;
use agent_loadout_core::model::applied::Scope;
use agent_loadout_core::model::skill_meta::{DistributionMethod, SkillSourceKind};
use agent_loadout_core::remote::git::SystemRunner;
use agent_loadout_core::skills::agents_cli::{self, CliAction};
use agent_loadout_core::skills::classify::{self, EntryClass};
use agent_loadout_core::skills::install::{self, InstallRequest, Naming, Source, UpdateStatus};
use agent_loadout_core::skills::link::{self, LinkMethod, SkillLedger};
use agent_loadout_core::skills::{
    self, LinkKind, LinkStatus, SkillsEnv, SyncReport, frontmatter, store,
};
use agent_loadout_core::{Env, ErrorCode};
use agent_loadout_testkit::git_env::{git_vars, init_bare, system_git_or_skip, with_git_vars};
use support::{Fx, copy_dir, fixtures, git_commit, git_init, host_os};

static RUNNER: SystemRunner = SystemRunner {
    idle_timeout: Duration::from_secs(60),
};
static NO_REGISTRY: NoRegistry = NoRegistry;

fn sx<'a>(fx: &'a Fx, env: &'a Env) -> SkillsEnv<'a> {
    SkillsEnv {
        env,
        fs: &fx.fs,
        home: &fx.home,
        registry: &NO_REGISTRY,
        runner: &RUNNER,
        link_kind: LinkKind::Junction,
    }
}

/// A copy of `fixtures/skills/<name>` at `<root>/src/<name>`.
fn source(fx: &Fx, name: &str) -> PathBuf {
    let to = fx.th.root().join("src").join(name);
    copy_dir(&fixtures().join("skills").join(name), &to);
    to
}

fn install_folder(fx: &Fx, env: &Env, name: &str) -> skills::Installed {
    let request = InstallRequest::new(Source::Local(source(fx, name)));
    install::install(&fx.store(), &sx(fx, env), &request).unwrap()
}

fn status_of(report: &SyncReport, path: &Path) -> LinkStatus {
    report
        .results
        .iter()
        .find(|result| result.path == path)
        .unwrap_or_else(|| panic!("no result for {} in {report:#?}", path.display()))
        .status
}

fn claude_skills(fx: &Fx) -> PathBuf {
    fx.th.home().join(".claude").join("skills")
}

/// Whether `path` is a link (junction or symbolic link) and not a folder.
fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// A directory link at `link` to `target`, as a test fixture: a junction
/// on Windows, a symbolic link elsewhere.
fn dir_link(target: &Path, link: &Path) {
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    #[cfg(windows)]
    junction::create(target, link).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[test]
fn frontmatter_keeps_every_field() {
    let text = std::fs::read_to_string(
        fixtures()
            .join("skills")
            .join("repo-conventions")
            .join("SKILL.md"),
    )
    .unwrap();
    let fm = frontmatter::parse(&text).unwrap().unwrap();
    let keys: Vec<&str> = fm.fields.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "name",
            "description",
            "license",
            "allowed-tools",
            "metadata"
        ]
    );
    assert_eq!(fm.name(), Some("repo-conventions"));
    // A folded multi-line value, which SkillDock's line reader lost.
    assert_eq!(
        fm.description(),
        Some("House rules for this repository: commit style, branch names and review etiquette.")
    );
    let json = fm.to_json();
    assert_eq!(json["allowed-tools"], serde_json::json!(["Read", "Grep"]));
    assert_eq!(json["metadata"]["version"], 2);
    assert_eq!(json["metadata"]["reviewed"], true);
    assert_eq!(
        fm.get("license"),
        Some(&NativeValue::String("MIT".to_owned()))
    );

    // CRLF and a BOM; no frontmatter; an unclosed block; invalid YAML.
    let crlf = "\u{feff}---\r\nname: x\r\n---\r\nbody\r\n";
    assert_eq!(frontmatter::parse(crlf).unwrap().unwrap().name(), Some("x"));
    assert!(frontmatter::parse("# no frontmatter\n").unwrap().is_none());
    assert!(frontmatter::parse("---\nname: x\n").unwrap().is_none());
    let err = frontmatter::parse("---\nname: [x\n---\n").unwrap_err();
    assert_eq!(err.code(), ErrorCode::ParseError);
}

#[test]
fn local_install_is_one_commit_without_nested_git() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let src = source(&fx, "repo-conventions");
    // A nested repository: never copied into the store.
    fx.write(&src.join(".git").join("HEAD"), "ref: refs/heads/main\n");
    fx.write(&src.join("references").join(".git"), "gitdir: elsewhere\n");
    let store = fx.store();
    let before = fx.commit_count();
    let request = InstallRequest::new(Source::Local(src));
    let installed = install::install(&store, &sx(&fx, &env), &request).unwrap();
    assert_eq!(installed.id, "repo-conventions");
    assert_eq!(installed.name, "repo-conventions");
    assert_eq!(fx.commit_count(), before + 1);
    assert_eq!(
        fx.log()[0].summary,
        "skill: add repo-conventions from repo-conventions"
    );

    let files = store::read_content(&store, "repo-conventions").unwrap();
    let paths: Vec<&str> = files.keys().map(String::as_str).collect();
    assert_eq!(paths, ["SKILL.md", "references/style.md"]);
    let skill = store::get(&store, "repo-conventions").unwrap().unwrap();
    assert!(skill.problems.is_empty(), "{:?}", skill.problems);
    let meta = skill.meta.unwrap();
    assert_eq!(meta.name.as_deref(), Some("repo-conventions"));
    assert_eq!(meta.source.unwrap().kind, SkillSourceKind::Local);
    assert!(meta.enable.is_none());
    // Metadata lives outside the skill folder.
    assert!(
        store
            .root()
            .join("skill-meta")
            .join("repo-conventions.toml")
            .is_file()
    );
    assert!(store.worktree_changes().unwrap().is_empty());
}

#[test]
fn a_taken_name_needs_confirmation_then_gets_a_suffix() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let first = install_folder(&fx, &env, "plain-notes");
    // No frontmatter: the id comes from the folder name.
    assert_eq!(first.id, "plain-notes");

    let src = fx.th.root().join("src").join("plain-notes");
    let mut request = InstallRequest::new(Source::Local(src));
    let err = install::install(&fx.store(), &sx(&fx, &env), &request).unwrap_err();
    assert_eq!(err.code(), ErrorCode::AlreadyExists);
    assert!(err.message().contains("plain-notes-2"), "{err}");

    request.naming.rename_if_taken = true;
    let second = install::install(&fx.store(), &sx(&fx, &env), &request).unwrap();
    assert_eq!(second.id, "plain-notes-2");
    assert_eq!(second.renamed_from.as_deref(), Some("plain-notes"));
    let ids = store::ids(&fx.store()).unwrap();
    assert_eq!(ids, ["plain-notes", "plain-notes-2"]);
}

#[test]
fn enable_links_without_admin_and_disable_removes_only_the_link() {
    let fx = Fx::new();
    let env = fx.env.clone();
    install_folder(&fx, &env, "repo-conventions");
    let store = fx.store();
    let sx = sx(&fx, &env);
    let before = fx.commit_count();

    let (commit, report) = link::set_enabled(
        &store,
        &sx,
        "repo-conventions",
        &["claude-code".to_owned()],
        true,
        None,
    )
    .unwrap();
    assert!(commit.is_some());
    assert_eq!(fx.commit_count(), before + 1);
    assert_eq!(
        fx.log()[0].summary,
        "skill: enable repo-conventions for claude-code"
    );
    let path = claude_skills(&fx).join("repo-conventions");
    assert_eq!(status_of(&report, &path), LinkStatus::Created);
    assert!(is_link(&path));
    // A junction on Windows (made as a normal user: no Developer Mode or
    // admin rights), not a directory symbolic link.
    #[cfg(windows)]
    {
        assert!(junction::exists(&path).unwrap());
        let target = junction::get_target(&path).unwrap();
        let store_dir = store::content_dir(&store, "repo-conventions").unwrap();
        assert!(agent_loadout_core::layering::same_path(&target, &store_dir));
    }
    // The client reads the store content through it.
    let text = std::fs::read_to_string(path.join("SKILL.md")).unwrap();
    assert!(text.contains("name: repo-conventions"));

    let ledger = SkillLedger::load(&fx.fs, &fx.home.state_dir()).unwrap();
    let owned = ledger.get(&path).unwrap();
    let expected = if cfg!(windows) {
        LinkMethod::Junction
    } else {
        LinkMethod::Symlink
    };
    assert_eq!(owned.method, expected);
    assert_eq!(owned.skill_id, "repo-conventions");
    assert_eq!(owned.scope, Scope::User);
    // MCP entries and skill links share the ledger file.
    let raw: serde_json::Value =
        serde_json::from_str(&fx.read(&fx.home.state_dir().join("ledger.json"))).unwrap();
    assert_eq!(raw["skill_links"][0]["kind"], "skill_link");
    assert!(raw["entries"].is_array());

    // A second sync changes nothing.
    let again = link::sync(&store, &sx).unwrap();
    assert_eq!(status_of(&again, &path), LinkStatus::Unchanged);

    let (_, report) = link::set_enabled(
        &store,
        &sx,
        "repo-conventions",
        &["claude-code".to_owned()],
        false,
        None,
    )
    .unwrap();
    assert_eq!(status_of(&report, &path), LinkStatus::Removed);
    assert!(std::fs::symlink_metadata(&path).is_err());
    let content = store::content_dir(&store, "repo-conventions").unwrap();
    assert!(content.join("SKILL.md").is_file());
    assert!(content.join("references").join("style.md").is_file());
    let meta = store::read_meta(&store, "repo-conventions")
        .unwrap()
        .unwrap();
    assert!(meta.enable.is_none());
}

#[test]
fn a_real_folder_or_a_foreign_link_is_never_overwritten() {
    let fx = Fx::new();
    let env = fx.env.clone();
    install_folder(&fx, &env, "repo-conventions");
    let store = fx.store();
    let sx = sx(&fx, &env);

    // A real folder of the same name in Claude Code's skills.
    let real = claude_skills(&fx).join("repo-conventions");
    fx.write(&real.join("SKILL.md"), "mine\n");
    // A link to another folder in Codex's skills.
    let other = fx.th.root().join("other-skill");
    fx.write(&other.join("SKILL.md"), "other\n");
    let codex = fx
        .th
        .home()
        .join(".codex")
        .join("skills")
        .join("repo-conventions");
    dir_link(&other, &codex);

    let clients = ["claude-code".to_owned(), "codex".to_owned()];
    let (_, report) =
        link::set_enabled(&store, &sx, "repo-conventions", &clients, true, None).unwrap();
    assert_eq!(status_of(&report, &real), LinkStatus::Foreign);
    assert_eq!(status_of(&report, &codex), LinkStatus::Collision);
    assert!(report.needs_attention());
    let collision = report
        .results
        .iter()
        .find(|result| result.path == codex)
        .unwrap();
    assert_eq!(collision.found_target.as_deref(), Some(other.as_path()));
    assert_eq!(fx.read(&real.join("SKILL.md")), "mine\n");
    assert!(!is_link(&real));
    assert_eq!(fx.read(&codex.join("SKILL.md")), "other\n");
    let ledger = SkillLedger::load(&fx.fs, &fx.home.state_dir()).unwrap();
    assert!(ledger.links.is_empty());

    // Disabling never removes what Loadout does not own.
    link::set_enabled(&store, &sx, "repo-conventions", &clients, false, None).unwrap();
    assert_eq!(fx.read(&real.join("SKILL.md")), "mine\n");
    assert!(is_link(&codex));
    assert!(other.join("SKILL.md").is_file());
}

#[test]
fn copies_are_owned_by_hash_and_edits_are_kept() {
    let fx = Fx::new();
    let env = fx.env.clone();
    install_folder(&fx, &env, "repo-conventions");
    let store = fx.store();
    let sx = sx(&fx, &env);
    let cursor = fx
        .th
        .home()
        .join(".cursor")
        .join("skills")
        .join("repo-conventions");
    let clients = ["cursor".to_owned()];
    let (_, report) = link::set_enabled(
        &store,
        &sx,
        "repo-conventions",
        &clients,
        true,
        Some(DistributionMethod::Copy),
    )
    .unwrap();
    assert_eq!(status_of(&report, &cursor), LinkStatus::Created);
    assert!(!is_link(&cursor));
    assert!(cursor.join(skills::MARKER_FILE).is_file());
    assert!(cursor.join("references").join("style.md").is_file());
    let ledger = SkillLedger::load(&fx.fs, &fx.home.state_dir()).unwrap();
    let owned = ledger.get(&cursor).unwrap();
    assert_eq!(owned.method, LinkMethod::Copy);
    assert!(
        owned
            .written_hash
            .as_deref()
            .unwrap()
            .starts_with("sha256:")
    );

    // Edited in place: drift, never overwritten or removed.
    fx.write(&cursor.join("SKILL.md"), "edited by hand\n");
    let (_, report) =
        link::set_enabled(&store, &sx, "repo-conventions", &clients, false, None).unwrap();
    assert_eq!(status_of(&report, &cursor), LinkStatus::Changed);
    assert_eq!(fx.read(&cursor.join("SKILL.md")), "edited by hand\n");
    // No longer owned: enabling again finds a foreign folder.
    let (_, report) =
        link::set_enabled(&store, &sx, "repo-conventions", &clients, true, None).unwrap();
    assert_eq!(status_of(&report, &cursor), LinkStatus::Foreign);
    assert_eq!(fx.read(&cursor.join("SKILL.md")), "edited by hand\n");
}

#[test]
fn project_links_are_excluded_and_the_project_stays_clean() {
    let fx = Fx::new();
    let env = fx.env.clone();
    install_folder(&fx, &env, "repo-conventions");
    let store = fx.store();
    let sx = sx(&fx, &env);

    let root = fx.th.projects().join("web-app");
    let repo = git_init(&root);
    fx.write(&root.join("README.md"), "# web app\n");
    git_commit(&repo, "README.md");
    fx.manifest(
        &root,
        "project.local.toml",
        "schema = 1\nid = \"web-app-7k2f\"\n\n[defaults]\nclients = [\"claude-code\", \"omp\"]\n",
    );
    exclude_manifest_dir(&fx.fs, &root).unwrap();
    fx.know_projects(&[("web-app-7k2f", &root)]);
    fx.trust(&root);

    let before = fx.commit_count();
    let (commit, report) =
        link::set_project_enabled(&store, &sx, &root, "repo-conventions", true).unwrap();
    assert!(commit.is_some());
    assert_eq!(fx.commit_count(), before + 1);
    let claude = root.join(".claude").join("skills").join("repo-conventions");
    let omp = root.join(".omp").join("skills").join("repo-conventions");
    assert_eq!(status_of(&report, &claude), LinkStatus::Created);
    assert_eq!(status_of(&report, &omp), LinkStatus::Created);
    assert!(is_link(&claude) && is_link(&omp));
    assert!(!report.needs_attention(), "{report:#?}");

    // The manifest lists the skill, the mirror holds it, and the edited
    // manifest stays trusted (Loadout wrote it).
    let manifest = fx.read(&root.join(".loadout").join("project.local.toml"));
    assert!(
        manifest.contains("enable = [\"repo-conventions\"]"),
        "{manifest}"
    );
    let mirror = store
        .read_to_string("projects/web-app-7k2f/project.local.toml")
        .unwrap()
        .unwrap();
    assert_eq!(mirror, manifest);

    // Each link is in .git/info/exclude; nothing is staged; status is clean.
    let exclude = fx.read(&root.join(".git").join("info").join("exclude"));
    assert!(
        exclude
            .lines()
            .any(|line| line == "/.claude/skills/repo-conventions")
    );
    assert!(
        exclude
            .lines()
            .any(|line| line == "/.omp/skills/repo-conventions")
    );
    let repo = git2::Repository::open(&root).unwrap();
    let mut options = git2::StatusOptions::new();
    options.include_untracked(true).recurse_untracked_dirs(true);
    let statuses = repo.statuses(Some(&mut options)).unwrap();
    let dirty: Vec<String> = statuses
        .iter()
        .filter_map(|entry| entry.path().ok().map(str::to_owned))
        .collect();
    assert!(dirty.is_empty(), "git status is not clean: {dirty:?}");
    assert_eq!(repo.index().unwrap().len(), 1, "only README.md is tracked");
    if let Some(git) = system_git_or_skip("project_links_are_excluded (git status)") {
        let output = fx
            .th
            .command(&git)
            .envs(git_vars(&fx.th, &git))
            .current_dir(&root)
            .args(["status", "--porcelain", "--untracked-files=all"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    }

    // Disable removes the links and keeps the exclude lines harmless.
    let (_, report) =
        link::set_project_enabled(&store, &sx, &root, "repo-conventions", false).unwrap();
    assert_eq!(status_of(&report, &claude), LinkStatus::Removed);
    assert!(std::fs::symlink_metadata(&claude).is_err());
    assert!(
        store::content_dir(&store, "repo-conventions")
            .unwrap()
            .join("SKILL.md")
            .is_file()
    );
}

#[test]
fn an_untrusted_project_is_left_alone() {
    let fx = Fx::new();
    let env = fx.env.clone();
    install_folder(&fx, &env, "repo-conventions");
    let store = fx.store();
    let sx = sx(&fx, &env);
    let root = fx.th.projects().join("tools");
    fx.manifest(
        &root,
        "project.local.toml",
        "schema = 1\nid = \"tools-1a2b\"\n\n[defaults]\nclients = [\"claude-code\"]\n\n\
         [skills]\nenable = [\"repo-conventions\"]\n",
    );
    fx.know_projects(&[("tools-1a2b", &root)]);
    let report = link::sync(&store, &sx).unwrap();
    assert!(report.results.is_empty(), "{report:#?}");
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.code() == ErrorCode::TrustRequired)
    );
    let err = link::set_project_enabled(&store, &sx, &root, "repo-conventions", false).unwrap_err();
    assert_eq!(err.code(), ErrorCode::TrustRequired);

    // Shared projects (`share = true`) get copies once trusted.
    fx.manifest(
        &root,
        "project.local.toml",
        "schema = 1\nid = \"tools-1a2b\"\n\n[defaults]\nclients = [\"claude-code\"]\n\n\
         [skills]\nenable = [\"repo-conventions\"]\nshare = true\n",
    );
    fx.trust(&root);
    let report = link::sync(&store, &sx).unwrap();
    let copy = root.join(".claude").join("skills").join("repo-conventions");
    assert_eq!(status_of(&report, &copy), LinkStatus::Created);
    assert!(!is_link(&copy));
    assert!(copy.join(skills::MARKER_FILE).is_file());
}

/// Runs the system git with the test kit's isolated environment.
fn git_run(fx: &Fx, git: &Path, cwd: &Path, args: &[&str]) {
    let status = fx
        .th
        .command(git)
        .envs(git_vars(&fx.th, git))
        .current_dir(cwd)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// A bare repository `<root>/up.git` holding `fixtures/skills/monorepo`
/// on `main`; returns it and the work repository that pushes to it.
fn upstream(fx: &Fx, git: &Path) -> (PathBuf, PathBuf) {
    let bare = fx.th.root().join("up.git");
    init_bare(&fx.th, git, &bare);
    let work = fx.th.root().join("work");
    let root = fx.th.root();
    git_run(
        fx,
        git,
        root,
        &["init", "--quiet", "--initial-branch=main", "work"],
    );
    git_run(fx, git, &work, &["remote", "add", "origin", "../up.git"]);
    copy_dir(&fixtures().join("skills").join("monorepo"), &work);
    push(fx, git, &work, "initial");
    (bare, work)
}

fn push(fx: &Fx, git: &Path, work: &Path, message: &str) {
    git_run(fx, git, work, &["add", "-A"]);
    git_run(fx, git, work, &["commit", "--quiet", "-m", message]);
    git_run(fx, git, work, &["push", "--quiet", "origin", "HEAD:main"]);
}

#[test]
fn git_install_update_and_an_unreachable_upstream() {
    let Some(git) = system_git_or_skip("git_install_update_and_an_unreachable_upstream") else {
        return;
    };
    let fx = Fx::new();
    let env = with_git_vars(&fx.th, &git, fx.th.env_builder(host_os())).build();
    let sx = sx(&fx, &env);
    let store = fx.store();
    let (bare, work) = upstream(&fx, &git);
    let url = bare.to_string_lossy().into_owned();

    // The repository holds two skills: without a path the install asks.
    let source = Source::parse(&url, &fx.fs);
    assert!(matches!(source, Source::Git { .. }), "{source:?}");
    let err = install::install(&store, &sx, &InstallRequest::new(source.clone())).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);
    assert!(err.message().contains("skills/alpha"), "{err}");

    let mut request = InstallRequest::new(source);
    request.path = Some("skills/alpha".to_owned());
    let installed = install::install(&store, &sx, &request).unwrap();
    assert_eq!(installed.id, "alpha");
    let commit = installed.source_commit.clone().unwrap();
    assert_eq!(
        fx.log()[0].summary,
        format!("skill: add alpha from {url}@{}", &commit[..7])
    );
    let meta = store::read_meta(&store, "alpha").unwrap().unwrap();
    let source = meta.source.unwrap();
    assert_eq!(source.kind, SkillSourceKind::Git);
    assert_eq!(source.url.as_deref(), Some(url.as_str()));
    assert_eq!(source.path.as_deref(), Some("skills/alpha"));
    assert_eq!(source.git_ref.as_deref(), Some("main"));
    assert_eq!(source.commit.as_deref(), Some(commit.as_str()));
    // Sparse: the cache clone holds the skill folder only.
    let cache = fx.home.cache_dir().join("skill-sources").join("alpha");
    assert!(
        cache
            .join("skills")
            .join("alpha")
            .join("SKILL.md")
            .is_file()
    );
    assert!(!cache.join("skills").join("beta").exists());
    assert!(!cache.join("README.md").exists());

    // A copy for Claude Code and a link for omp.
    link::set_enabled(
        &store,
        &sx,
        "alpha",
        &["claude-code".to_owned()],
        true,
        Some(DistributionMethod::Copy),
    )
    .unwrap();
    let copy = claude_skills(&fx).join("alpha");
    assert!(copy.join(skills::MARKER_FILE).is_file());

    let check = install::check_update(&store, &sx, "alpha").unwrap();
    assert_eq!(check.status, UpdateStatus::UpToDate);

    // Upstream changes the skill.
    let changed = "---\nname: alpha\ndescription: Second version.\n---\n\n# Alpha v2\n";
    fx.write(&work.join("skills").join("alpha").join("SKILL.md"), changed);
    fx.write(&work.join("skills").join("alpha").join("notes.md"), "new\n");
    push(&fx, &git, &work, "alpha v2");
    let check = install::check_update(&store, &sx, "alpha").unwrap();
    assert_eq!(check.status, UpdateStatus::Available);
    let kinds: Vec<(&str, &str)> = check
        .changes
        .iter()
        .map(|change| (change.path.as_str(), change.kind.as_str()))
        .collect();
    assert_eq!(kinds, [("SKILL.md", "modified"), ("notes.md", "added")]);
    // Checking changes nothing in the store.
    assert!(
        !fx.read(&store.root().join("skills").join("alpha").join("SKILL.md"))
            .contains("v2")
    );

    let updated = install::update(&store, &sx, "alpha").unwrap();
    assert!(updated.commit.is_some());
    assert_eq!(
        fx.log()[0].summary,
        format!("skill: update alpha to {}", &updated.to[..7])
    );
    assert_eq!(
        fx.read(&store.root().join("skills").join("alpha").join("SKILL.md")),
        changed
    );
    // The owned, unchanged copy follows the store.
    assert_eq!(status_of(&updated.report, &copy), LinkStatus::Updated);
    assert_eq!(fx.read(&copy.join("SKILL.md")), changed);
    assert!(copy.join("notes.md").is_file());

    // Upstream removed: source_unreachable, content intact.
    std::fs::remove_dir_all(&bare).unwrap();
    let check = install::check_update(&store, &sx, "alpha").unwrap();
    assert_eq!(check.status, UpdateStatus::Unreachable);
    assert_eq!(
        check.error.as_ref().unwrap().code(),
        ErrorCode::SourceUnreachable
    );
    let commits = fx.commit_count();
    let err = install::update(&store, &sx, "alpha").unwrap_err();
    assert_eq!(err.code(), ErrorCode::SourceUnreachable);
    assert_eq!(fx.commit_count(), commits);
    assert_eq!(
        fx.read(&store.root().join("skills").join("alpha").join("SKILL.md")),
        changed
    );
    assert_eq!(fx.read(&copy.join("SKILL.md")), changed);
    let skill = store::get(&store, "alpha").unwrap().unwrap();
    assert!(skill.problems.is_empty(), "{:?}", skill.problems);

    // A clone that cannot be reached at install is source_unreachable too.
    let gone = InstallRequest::new(Source::Git {
        url: fx
            .th
            .root()
            .join("missing.git")
            .to_string_lossy()
            .into_owned(),
        git_ref: None,
        path: None,
    });
    let err = install::install(&store, &sx, &gone).unwrap_err();
    assert_eq!(err.code(), ErrorCode::SourceUnreachable);

    // Removing the skill removes its owned copy, its cache and its files.
    let (_, report) = skills::remove(&store, &sx, "alpha").unwrap();
    assert_eq!(status_of(&report, &copy), LinkStatus::Removed);
    assert!(!copy.exists());
    assert!(!cache.exists());
    assert!(!store.root().join("skills").join("alpha").exists());
    assert!(store::read_meta(&store, "alpha").unwrap().is_none());
    assert_eq!(fx.log()[0].summary, "skill: remove alpha");
}

#[test]
fn source_urls() {
    let fx = Fx::new();
    let tree = Source::parse(
        "https://github.com/example-org/agent-skills/tree/main/skills/pdf-tools",
        &fx.fs,
    );
    assert_eq!(
        tree,
        Source::Git {
            url: "https://github.com/example-org/agent-skills".to_owned(),
            git_ref: Some("main".to_owned()),
            path: Some("skills/pdf-tools".to_owned()),
        }
    );
    let blob = Source::parse(
        "https://github.com/example-org/agent-skills/blob/v2/skills/pdf-tools/SKILL.md",
        &fx.fs,
    );
    assert!(matches!(
        &blob,
        Source::Git { git_ref: Some(r), path: Some(p), .. } if r == "v2" && p == "skills/pdf-tools"
    ));
    assert!(matches!(
        Source::parse("git@example.com:team/skills.git", &fx.fs),
        Source::Git { .. }
    ));
    let folder = fx.th.root().join("src");
    assert_eq!(
        Source::parse(&folder.to_string_lossy(), &fx.fs),
        Source::Local(folder)
    );
}

/// A zip of `entries` (name, bytes) at `<root>/<name>`.
fn zip_file(fx: &Fx, name: &str, entries: &[(&str, &[u8])]) -> PathBuf {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (entry, bytes) in entries {
        writer.start_file(*entry, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    let bytes = writer.finish().unwrap().into_inner();
    let path = fx.th.root().join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn install_zip(
    fx: &Fx,
    env: &Env,
    path: PathBuf,
) -> Result<skills::Installed, agent_loadout_core::CoreError> {
    install::install(
        &fx.store(),
        &sx(fx, env),
        &InstallRequest::new(Source::Local(path)),
    )
}

#[test]
fn archives_are_checked_for_zip_slip_links_and_size() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let skill_md: &[u8] = b"---\nname: zipped\n---\n# Zipped\n";

    // A valid .skill archive with one top folder installs.
    let good = zip_file(
        &fx,
        "zipped.skill",
        &[
            ("zipped/SKILL.md", skill_md),
            ("zipped/scripts/run.sh", b"echo hi\n"),
            ("__MACOSX/zipped/._SKILL.md", b"x"),
        ],
    );
    let installed = install_zip(&fx, &env, good).unwrap();
    assert_eq!(installed.id, "zipped");
    let files = store::read_content(&fx.store(), "zipped").unwrap();
    let paths: Vec<&str> = files.keys().map(String::as_str).collect();
    assert_eq!(paths, ["SKILL.md", "scripts/run.sh"]);

    let commits = fx.commit_count();
    let refused = |name: &str, entries: &[(&str, &[u8])], code: ErrorCode| {
        let path = zip_file(&fx, name, entries);
        let err = install_zip(&fx, &env, path).unwrap_err();
        assert_eq!(err.code(), code, "{name}: {err}");
        err
    };
    // Zip-slip: parent segments and absolute names.
    let err = refused(
        "slip.zip",
        &[("SKILL.md", skill_md), ("../escaped.md", b"x")],
        ErrorCode::InvalidPath,
    );
    assert!(err.message().contains("zip-slip"), "{err}");
    refused(
        "slip2.zip",
        &[("SKILL.md", skill_md), ("a/../../escaped.md", b"x")],
        ErrorCode::InvalidPath,
    );
    refused(
        "abs.zip",
        &[("SKILL.md", skill_md), ("/abs.md", b"x")],
        ErrorCode::InvalidPath,
    );
    refused(
        "drive.zip",
        &[("SKILL.md", skill_md), ("C:/abs.md", b"x")],
        ErrorCode::InvalidPath,
    );
    // A symbolic link entry.
    {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("SKILL.md", options).unwrap();
        writer.write_all(skill_md).unwrap();
        writer.add_symlink("secrets", "../../..", options).unwrap();
        let path = fx.th.root().join("link.zip");
        std::fs::write(&path, writer.finish().unwrap().into_inner()).unwrap();
        let err = install_zip(&fx, &env, path).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidPath);
        assert!(err.message().contains("symbolic link"), "{err}");
    }
    // Size limits: 5 MB per file, 20 MB in all, 500 files.
    let big = vec![b'a'; (skills::MAX_ARCHIVE_FILE_BYTES + 1) as usize];
    refused(
        "big.zip",
        &[("SKILL.md", skill_md), ("big.bin", &big)],
        ErrorCode::InvalidValue,
    );
    let part = vec![b'b'; (skills::MAX_ARCHIVE_FILE_BYTES - 1024) as usize];
    let parts: Vec<(String, &[u8])> = (0..5)
        .map(|n| (format!("part{n}.bin"), part.as_slice()))
        .collect();
    let mut entries: Vec<(&str, &[u8])> = vec![("SKILL.md", skill_md)];
    entries.extend(parts.iter().map(|(name, bytes)| (name.as_str(), *bytes)));
    refused("total.zip", &entries, ErrorCode::InvalidValue);
    let names: Vec<String> = (0..skills::MAX_ARCHIVE_FILES)
        .map(|n| format!("f{n}.txt"))
        .collect();
    let mut entries: Vec<(&str, &[u8])> = vec![("SKILL.md", skill_md)];
    entries.extend(names.iter().map(|name| (name.as_str(), b"x" as &[u8])));
    refused("many.zip", &entries, ErrorCode::InvalidValue);
    // No SKILL.md.
    refused("empty.zip", &[("README.md", b"x")], ErrorCode::NotFound);
    // Nothing refused reached the store.
    assert_eq!(fx.commit_count(), commits);
    assert_eq!(store::ids(&fx.store()).unwrap(), ["zipped"]);
}

#[test]
fn folder_links_that_escape_are_refused() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let src = source(&fx, "repo-conventions");
    let outside = fx.th.root().join("private");
    fx.write(&outside.join("token.txt"), "not part of the skill\n");
    dir_link(&outside, &src.join("linked"));
    let request = InstallRequest::new(Source::Local(src.clone()));
    let err = install::install(&fx.store(), &sx(&fx, &env), &request).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidPath);
    assert!(err.message().contains("outside the skill folder"), "{err}");

    // A link that stays inside the skill folder is followed.
    std::fs::remove_dir(src.join("linked")).unwrap_or_else(|_| {
        std::fs::remove_file(src.join("linked")).unwrap();
    });
    dir_link(&src.join("references"), &src.join("linked"));
    let installed = install::install(&fx.store(), &sx(&fx, &env), &request).unwrap();
    let files = store::read_content(&fx.store(), &installed.id).unwrap();
    assert!(files.contains_key("linked/style.md"));
}

#[test]
fn tool_folders_are_classified_imported_and_trashed() {
    let fx = Fx::new();
    let env = fx.env.clone();
    install_folder(&fx, &env, "repo-conventions");
    let store = fx.store();
    let sx = sx(&fx, &env);
    link::set_enabled(
        &store,
        &sx,
        "repo-conventions",
        &["claude-code".to_owned()],
        true,
        None,
    )
    .unwrap();
    let dir = claude_skills(&fx);
    fx.write(
        &dir.join("hand-made").join("SKILL.md"),
        "---\nname: hand-made\ndescription: Written in place.\n---\n",
    );
    let elsewhere = fx.th.root().join("elsewhere");
    fx.write(&elsewhere.join("SKILL.md"), "---\nname: elsewhere\n---\n");
    dir_link(&elsewhere, &dir.join("elsewhere"));

    let dirs = classify::scan(&sx, Scope::User, None).unwrap();
    let claude = dirs.iter().find(|dir| dir.client == "claude-code").unwrap();
    assert!(claude.exists);
    let classes: Vec<(&str, EntryClass)> = claude
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.class))
        .collect();
    assert_eq!(
        classes,
        [
            ("elsewhere", EntryClass::Mismatch),
            ("hand-made", EntryClass::Unmanaged),
            ("repo-conventions", EntryClass::Managed),
        ]
    );
    let hand = &claude.entries[1];
    assert_eq!(hand.description.as_deref(), Some("Written in place."));
    assert!(dirs.iter().any(|dir| dir.client == "codex" && !dir.exists));

    // Import copies the unmanaged folder into the store; it stays in place.
    let naming = Naming::default();
    let imported = classify::import(
        &store,
        &sx,
        "claude-code",
        Scope::User,
        None,
        "hand-made",
        &naming,
    )
    .unwrap();
    assert_eq!(imported.id, "hand-made");
    assert!(dir.join("hand-made").join("SKILL.md").is_file());
    let err = classify::import(
        &store,
        &sx,
        "claude-code",
        Scope::User,
        None,
        "elsewhere",
        &naming,
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);

    // Trash moves an entry (a link moves as a link); managed ones refuse.
    let trashed = classify::trash(&sx, "claude-code", Scope::User, None, "elsewhere").unwrap();
    assert!(trashed.starts_with(fx.home.backups_dir().join("trash")));
    assert!(is_link(&trashed));
    assert!(std::fs::symlink_metadata(dir.join("elsewhere")).is_err());
    assert!(elsewhere.join("SKILL.md").is_file());
    let err =
        classify::trash(&sx, "claude-code", Scope::User, None, "repo-conventions").unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);
    let err = classify::trash(&sx, "claude-code", Scope::User, None, "../x").unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);
}

#[test]
fn agents_cli_skills_are_scanned_read_only_and_vendored() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let sx = sx(&fx, &env);
    let store = fx.store();
    assert!(agents_cli::enabled(&store));
    copy_dir(
        &fixtures().join("skills").join("agents-cli"),
        &fx.th.home().join(".agents"),
    );
    let scan = agents_cli::scan(&sx).unwrap();
    assert!(scan.warnings.is_empty());
    assert_eq!(scan.skills.len(), 1);
    let skill = &scan.skills[0];
    assert_eq!(skill.name, "pdf-tools");
    assert_eq!(skill.source.as_deref(), Some("example-org/agent-skills"));
    assert_eq!(skill.source_type.as_deref(), Some("github"));
    assert_eq!(
        skill.description.as_deref(),
        Some("Extract text and tables from PDF files.")
    );

    let vendored = agents_cli::vendor(&store, &sx, "pdf-tools", &Naming::default()).unwrap();
    assert_eq!(vendored.id, "pdf-tools");
    let meta = store::read_meta(&store, "pdf-tools").unwrap().unwrap();
    let source = meta.source.unwrap();
    assert_eq!(source.kind, SkillSourceKind::AgentsCli);
    assert_eq!(
        source.url.as_deref(),
        Some("https://github.com/example-org/agent-skills.git")
    );
    // The agents CLI's own copy is untouched.
    assert!(
        fx.th
            .home()
            .join(".agents")
            .join("skills")
            .join("pdf-tools")
            .join("SKILL.md")
            .is_file()
    );
    // Without the `skills` CLI on PATH nothing runs (never npx).
    let err = agents_cli::run_cli(&sx, CliAction::Update, "pdf-tools").unwrap_err();
    assert_eq!(err.code(), ErrorCode::ToolNotFound);
    let err = agents_cli::run_cli(&sx, CliAction::Remove, "--all").unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);

    // A lock file that does not parse is a warning.
    fx.write(&agents_cli::lock_file(&sx), "{ not json");
    let scan = agents_cli::scan(&sx).unwrap();
    assert_eq!(scan.warnings.len(), 1);
    assert!(scan.skills[0].source.is_none());
}

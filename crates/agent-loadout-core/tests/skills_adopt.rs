//! Adopt and bulk import of unmanaged skill folders (SPEC 8.6, 12.5, #3):
//! a folder is replaced by the managed link (a junction on Windows) only
//! when its content equals the store copy, the original goes to
//! `backups/trash/`, identical folders in several clients become one store
//! skill, differing ones are a conflict (pick one, or import both as
//! `name-<client>`), and each action is one store commit. Every path lives
//! in a `TempHome`.

mod support;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use agent_loadout_core::ErrorCode;
use agent_loadout_core::adapters::detect::NoRegistry;
use agent_loadout_core::layering::projects::exclude_manifest_dir;
use agent_loadout_core::layering::{Project, TrustState, TrustStore};
use agent_loadout_core::model::applied::Scope;
use agent_loadout_core::model::clients::ClientSelection;
use agent_loadout_core::model::skill_meta::DistributionMethod;
use agent_loadout_core::remote::git::SystemRunner;
use agent_loadout_core::skills::adopt::{self, AdoptAction, AdoptChoice, FolderStatus};
use agent_loadout_core::skills::classify;
use agent_loadout_core::skills::hash_cache::FolderHashCache;
use agent_loadout_core::skills::install::Naming;
use agent_loadout_core::skills::link::{self, LinkMethod, ORIGIN_ADOPTED, SkillLedger};
use agent_loadout_core::skills::{LinkKind, LinkStatus, SkillsEnv, store};
use agent_loadout_core::{Env, Fs};
use support::{Fx, git_commit, git_init};

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

fn claude_skills(fx: &Fx) -> PathBuf {
    fx.th.home().join(".claude").join("skills")
}

fn codex_skills(fx: &Fx) -> PathBuf {
    fx.th.home().join(".codex").join("skills")
}

fn omp_skills(fx: &Fx) -> PathBuf {
    fx.th.home().join(".omp").join("agent").join("skills")
}

/// A skill folder `<dir>/<name>` with a `SKILL.md` saying `body`.
fn skill(fx: &Fx, dir: &Path, name: &str, body: &str) -> PathBuf {
    let folder = dir.join(name);
    fx.write(
        &folder.join("SKILL.md"),
        &format!("---\nname: {name}\ndescription: {body}\n---\n\n{body}\n"),
    );
    fx.write(&folder.join("scripts").join("run.sh"), "echo ok\n");
    folder
}

fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

fn link_method() -> LinkMethod {
    if cfg!(windows) {
        LinkMethod::Junction
    } else {
        LinkMethod::Symlink
    }
}

fn clients_of(fx: &Fx, id: &str) -> Option<ClientSelection> {
    store::read_meta(&fx.store(), id)
        .unwrap()
        .and_then(|meta| meta.enable)
        .and_then(|enable| enable.clients)
}

fn list(ids: &[&str]) -> Option<ClientSelection> {
    Some(ClientSelection::List(
        ids.iter().map(|id| (*id).to_owned()).collect(),
    ))
}

/// Every file in `backups/trash/`, as `<entry>/<file>` relative paths.
fn trash_entries(fx: &Fx) -> Vec<String> {
    let dir = fx.home.backups_dir().join("trash");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

#[test]
fn a_folder_becomes_a_link_to_the_store_and_the_original_goes_to_the_trash() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let sx = sx(&fx, &env);
    let folder = skill(&fx, &claude_skills(&fx), "notes", "Take notes.");
    let store = fx.store();

    let scan = adopt::scan(Some(&store), &sx, Scope::User, None, None).unwrap();
    assert_eq!(scan.candidates.len(), 1);
    let candidate = &scan.candidates[0];
    assert_eq!(candidate.name, "notes");
    assert!(!candidate.is_conflict());
    assert_eq!(
        candidate.variants[0].clients().collect::<Vec<_>>(),
        ["claude-code"]
    );
    assert_eq!(
        candidate.variants[0].description.as_deref(),
        Some("Take notes.")
    );

    let before = fx.commit_count();
    let choices = adopt::default_choices(&scan);
    let report = adopt::adopt(&store, &sx, Scope::User, None, None, &choices).unwrap();
    assert_eq!(fx.commit_count(), before + 1, "one commit for the action");
    assert_eq!(fx.log()[0].summary, "skill: adopt notes");
    assert_eq!(report.added, ["notes"]);
    assert!(!report.needs_attention(), "{report:#?}");
    let result = &report.folders[0];
    assert_eq!(result.status, FolderStatus::Adopted);
    assert_eq!(clients_of(&fx, "notes"), list(&["claude-code"]));

    // The folder is now the managed link (a junction on Windows).
    assert!(is_link(&folder), "the folder is replaced by a link");
    let content = store::content_dir(&store, "notes").unwrap();
    assert!(
        folder.join("SKILL.md").is_file(),
        "the link reaches the store"
    );
    assert!(content.join("scripts").join("run.sh").is_file());
    let ledger = SkillLedger::load(&fx.fs, &fx.home.state_dir()).unwrap();
    let owned = ledger.get(&folder).expect("owned");
    assert_eq!(owned.method, link_method());
    assert_eq!(owned.origin(), ORIGIN_ADOPTED);
    assert_eq!(owned.skill_id, "notes");

    // The original is in the trash, never deleted.
    let trash = result.trash.as_ref().unwrap();
    assert!(trash.starts_with(fx.home.backups_dir().join("trash")));
    assert!(trash.join("SKILL.md").is_file() && !is_link(trash));
    assert_eq!(trash_entries(&fx).len(), 1);

    // Nothing left to adopt; the next sync keeps it owned and adopted.
    let scan = adopt::scan(Some(&store), &sx, Scope::User, None, None).unwrap();
    assert!(scan.candidates.is_empty());
    let report = link::sync(&store, &sx).unwrap();
    assert!(
        report
            .results
            .iter()
            .all(|result| result.status == LinkStatus::Unchanged)
    );
    let ledger = SkillLedger::load(&fx.fs, &fx.home.state_dir()).unwrap();
    assert_eq!(ledger.get(&folder).unwrap().origin(), ORIGIN_ADOPTED);
}

/// The 0.2 import copied a folder into the store and left it in place, so
/// enabling the skill for the same client found "a real folder in the
/// way". Adopt hands that folder over.
#[test]
fn an_imported_folder_in_the_way_is_adopted() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let sx = sx(&fx, &env);
    let folder = skill(&fx, &claude_skills(&fx), "notes", "Take notes.");
    let store = fx.store();
    classify::import(
        &store,
        &sx,
        "claude-code",
        Scope::User,
        None,
        "notes",
        &Naming::default(),
    )
    .unwrap();
    let (_, report) = link::set_enabled(
        &store,
        &sx,
        "notes",
        &["claude-code".to_owned()],
        true,
        None,
    )
    .unwrap();
    assert!(
        report
            .results
            .iter()
            .any(|result| result.status == LinkStatus::Foreign)
    );

    let scan = adopt::scan(Some(&store), &sx, Scope::User, None, None).unwrap();
    assert!(scan.candidates[0].variants[0].in_store);
    let before = fx.commit_count();
    let report = adopt::adopt(
        &store,
        &sx,
        Scope::User,
        None,
        None,
        &adopt::default_choices(&scan),
    )
    .unwrap();
    assert_eq!(report.reused, ["notes"]);
    assert!(report.added.is_empty());
    assert_eq!(report.folders[0].status, FolderStatus::Adopted);
    assert!(is_link(&folder));
    // Nothing changed in the store: the skill was enabled there already.
    assert_eq!(fx.commit_count(), before);
    assert!(!report.needs_attention(), "{report:#?}");
}

#[test]
fn identical_folders_become_one_skill_and_differing_ones_a_conflict() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let sx = sx(&fx, &env);
    let store = fx.store();
    let claude_shared = skill(&fx, &claude_skills(&fx), "shared", "Same everywhere.");
    let codex_shared = skill(&fx, &codex_skills(&fx), "shared", "Same everywhere.");
    let claude_diff = skill(&fx, &claude_skills(&fx), "diff", "Claude's version.");
    let omp_diff = skill(&fx, &omp_skills(&fx), "diff", "omp's version.");

    let scan = adopt::scan(Some(&store), &sx, Scope::User, None, None).unwrap();
    let shared = scan.candidate("shared").unwrap();
    assert!(!shared.is_conflict());
    assert_eq!(
        shared.variants[0].clients().collect::<Vec<_>>(),
        ["claude-code", "codex"]
    );
    let diff = scan.candidate("diff").unwrap();
    assert!(diff.is_conflict());
    assert_eq!(diff.variants.len(), 2);

    // Bulk: the identical pair is adopted, the conflict is left as it is.
    let before = fx.commit_count();
    let report = adopt::adopt(
        &store,
        &sx,
        Scope::User,
        None,
        None,
        &adopt::default_choices(&scan),
    )
    .unwrap();
    assert_eq!(fx.commit_count(), before + 1);
    assert_eq!(report.added, ["shared"]);
    assert_eq!(clients_of(&fx, "shared"), list(&["claude-code", "codex"]));
    assert!(is_link(&claude_shared) && is_link(&codex_shared));
    let statuses: Vec<(&str, &str, FolderStatus)> = report
        .folders
        .iter()
        .map(|folder| (folder.name.as_str(), folder.client, folder.status))
        .collect();
    assert_eq!(
        statuses,
        [
            ("diff", "claude-code", FolderStatus::Conflict),
            ("diff", "omp", FolderStatus::Conflict),
            ("shared", "claude-code", FolderStatus::Adopted),
            ("shared", "codex", FolderStatus::Adopted),
        ]
    );
    assert!(report.needs_attention(), "a conflict waits for a choice");
    assert!(!is_link(&claude_diff) && !is_link(&omp_diff));

    // Import both: Claude Code's as `diff`, omp's as `diff-omp`.
    let scan = adopt::scan(Some(&store), &sx, Scope::User, None, None).unwrap();
    let diff = scan.candidate("diff").unwrap();
    let claude_variant = diff
        .variants
        .iter()
        .find(|variant| variant.clients().any(|client| client == "claude-code"))
        .unwrap()
        .hash
        .clone();
    let before = fx.commit_count();
    let report = adopt::adopt(
        &store,
        &sx,
        Scope::User,
        None,
        None,
        &[AdoptChoice {
            name: "diff".to_owned(),
            action: AdoptAction::Both {
                primary: claude_variant,
            },
        }],
    )
    .unwrap();
    assert_eq!(fx.commit_count(), before + 1, "both in one commit");
    assert_eq!(fx.log()[0].summary, "skill: adopt diff, diff-omp");
    assert_eq!(report.added, ["diff", "diff-omp"]);
    assert_eq!(clients_of(&fx, "diff"), list(&["claude-code"]));
    assert_eq!(clients_of(&fx, "diff-omp"), list(&["omp"]));
    assert!(is_link(&claude_diff));
    // omp's folder went to the trash; its content is linked as `diff-omp`.
    assert!(std::fs::symlink_metadata(&omp_diff).is_err());
    let renamed = omp_skills(&fx).join("diff-omp");
    assert!(is_link(&renamed));
    assert!(
        fx.read(&renamed.join("SKILL.md"))
            .contains("omp's version."),
        "the renamed skill keeps its own content"
    );
    assert!(
        report
            .folders
            .iter()
            .all(|f| f.status == FolderStatus::Adopted)
    );
    assert!(!report.needs_attention(), "{report:#?}");
}

#[test]
fn a_folder_that_differs_from_the_store_copy_is_kept() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let sx = sx(&fx, &env);
    let store = fx.store();
    // The store holds `notes`; Claude Code's folder has other content.
    let elsewhere = skill(&fx, &fx.th.root().join("src"), "notes", "The store's.");
    agent_loadout_core::skills::install::install(
        &store,
        &sx,
        &agent_loadout_core::skills::InstallRequest::new(
            agent_loadout_core::skills::Source::Local(elsewhere),
        ),
    )
    .unwrap();
    let folder = skill(&fx, &claude_skills(&fx), "notes", "Edited by hand.");

    let scan = adopt::scan(Some(&store), &sx, Scope::User, None, None).unwrap();
    let candidate = scan.candidate("notes").unwrap();
    assert!(candidate.is_conflict(), "the store copy is a variant too");
    let stored = candidate
        .variants
        .iter()
        .find(|variant| variant.in_store)
        .unwrap();
    assert!(stored.folders.is_empty());
    let local = candidate
        .variants
        .iter()
        .find(|variant| !variant.in_store)
        .unwrap();

    // Taking the folder's content as `notes` would overwrite the store's.
    let err = adopt::adopt(
        &store,
        &sx,
        Scope::User,
        None,
        None,
        &[AdoptChoice {
            name: "notes".to_owned(),
            action: AdoptAction::Pick {
                variant: local.hash.clone(),
            },
        }],
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);

    // Keeping the store's copy: the edited folder stays as it is.
    let before = fx.commit_count();
    let report = adopt::adopt(
        &store,
        &sx,
        Scope::User,
        None,
        None,
        &[AdoptChoice {
            name: "notes".to_owned(),
            action: AdoptAction::Pick {
                variant: stored.hash.clone(),
            },
        }],
    )
    .unwrap();
    assert_eq!(fx.commit_count(), before);
    assert_eq!(report.folders[0].status, FolderStatus::Differs);
    assert!(!is_link(&folder) && folder.join("SKILL.md").is_file());
    assert!(trash_entries(&fx).is_empty());

    // A content no folder has (the scan was stale) is refused.
    let err = adopt::adopt(
        &store,
        &sx,
        Scope::User,
        None,
        None,
        &[AdoptChoice {
            name: "notes".to_owned(),
            action: AdoptAction::Pick {
                variant: "sha256:gone".to_owned(),
            },
        }],
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::EntryChanged);
}

/// The store copy leaves `.git` out: a folder that is a repository is
/// imported but never replaced.
#[test]
fn a_folder_holding_a_repository_is_imported_but_kept() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let sx = sx(&fx, &env);
    let store = fx.store();
    let folder = skill(&fx, &codex_skills(&fx), "tools", "Tooling.");
    fx.write(&folder.join(".git").join("HEAD"), "ref: refs/heads/main\n");

    let scan = adopt::scan(Some(&store), &sx, Scope::User, None, Some("codex")).unwrap();
    let report = adopt::adopt(
        &store,
        &sx,
        Scope::User,
        None,
        Some("codex"),
        &adopt::default_choices(&scan),
    )
    .unwrap();
    assert_eq!(report.added, ["tools"]);
    let result = &report.folders[0];
    assert_eq!(result.status, FolderStatus::Kept);
    assert_eq!(
        result.error.as_ref().unwrap().code(),
        ErrorCode::InvalidValue
    );
    assert!(folder.join(".git").join("HEAD").is_file());
    assert!(!is_link(&folder));
    assert!(report.needs_attention());
    assert!(trash_entries(&fx).is_empty());
}

#[test]
fn one_tool_only_and_names_that_are_not_ids() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let sx = sx(&fx, &env);
    let store = fx.store();
    skill(&fx, &claude_skills(&fx), "notes", "Claude.");
    let codex = skill(&fx, &codex_skills(&fx), "notes", "Codex.");
    skill(&fx, &codex_skills(&fx), "Odd Name", "Not an id.");
    fx.write(
        &codex_skills(&fx).join("empty").join("README.md"),
        "no skill\n",
    );

    // Codex alone: no conflict with Claude Code's folder.
    let scan = adopt::scan(Some(&store), &sx, Scope::User, None, Some("codex")).unwrap();
    assert_eq!(scan.candidates.len(), 1);
    assert!(!scan.candidates[0].is_conflict());
    let mut problems: Vec<(&str, ErrorCode)> = scan
        .problems
        .iter()
        .map(|problem| (problem.name.as_str(), problem.error.code()))
        .collect();
    problems.sort_by_key(|(name, _)| *name);
    assert_eq!(
        problems,
        [
            ("Odd Name", ErrorCode::InvalidId),
            ("empty", ErrorCode::NotFound)
        ]
    );
    let report = adopt::adopt(
        &store,
        &sx,
        Scope::User,
        None,
        Some("codex"),
        &adopt::default_choices(&scan),
    )
    .unwrap();
    assert_eq!(report.folders.len(), 1);
    assert!(is_link(&codex));
    assert_eq!(clients_of(&fx, "notes"), list(&["codex"]));

    let err = adopt::scan(Some(&store), &sx, Scope::User, None, Some("nope")).unwrap_err();
    assert_eq!(err.code(), ErrorCode::UnknownClient);
}

#[test]
fn project_folders_are_adopted_into_the_manifest() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let sx = sx(&fx, &env);
    let root = fx.th.projects().join("web-app");
    let repo = git_init(&root);
    fx.write(&root.join("README.md"), "# web app\n");
    git_commit(&repo, "README.md");
    fx.manifest(
        &root,
        "project.local.toml",
        "schema = 1\nid = \"web-app-7k2f\"\n\n[defaults]\nclients = [\"claude-code\"]\n",
    );
    exclude_manifest_dir(&fx.fs, &root).unwrap();
    fx.know_projects(&[("web-app-7k2f", &root)]);
    let store = fx.store();
    let folder = skill(
        &fx,
        &root.join(".claude").join("skills"),
        "lint",
        "Lint it.",
    );
    let other = skill(&fx, &root.join(".omp").join("skills"), "fmt", "Format it.");

    // Untrusted: nothing is written.
    let scan = adopt::scan(Some(&store), &sx, Scope::Project, Some(&root), None).unwrap();
    let choices = adopt::default_choices(&scan);
    let before = fx.commit_count();
    let err = adopt::adopt(&store, &sx, Scope::Project, Some(&root), None, &choices).unwrap_err();
    assert_eq!(err.code(), ErrorCode::TrustRequired);
    assert_eq!(fx.commit_count(), before);

    fx.trust(&root);
    let report = adopt::adopt(&store, &sx, Scope::Project, Some(&root), None, &choices).unwrap();
    assert_eq!(fx.commit_count(), before + 1, "one commit");
    assert_eq!(
        fx.log()[0].summary,
        "skill: adopt fmt, lint in project web-app-7k2f"
    );
    let manifest = fx.read(&root.join(".loadout").join("project.local.toml"));
    assert!(
        manifest.contains("enable = [\"fmt\", \"lint\"]"),
        "{manifest}"
    );
    // No user-scope enable for project skills.
    assert_eq!(clients_of(&fx, "lint"), None);
    // Claude Code is the project's client: its folder is a link now,
    // hidden from the repository.
    assert!(is_link(&folder));
    let exclude = fx.read(&root.join(".git").join("info").join("exclude"));
    assert!(exclude.lines().any(|line| line == "/.claude/skills/lint"));
    // omp is not: its folder is kept, with the reason.
    let fmt = report
        .folders
        .iter()
        .find(|folder| folder.client == "omp")
        .unwrap();
    assert_eq!(fmt.status, FolderStatus::Kept);
    assert!(!is_link(&other) && other.join("SKILL.md").is_file());
    // The edited manifest stays trusted (Loadout wrote it).
    let project = Project::load(&fx.fs, &root).unwrap().unwrap();
    let trust = TrustStore::load(&fx.fs, &fx.home.state_dir()).unwrap();
    assert_eq!(trust.state(&project), TrustState::Trusted);
    assert!(fx.fs.exists(&store::content_dir(&store, "fmt").unwrap()));
}

// Review follow-up.

/// omp and Pi share one folder when `PI_CODING_AGENT_DIR` is set: it is
/// replaced once and reported adopted for both.
#[test]
fn a_folder_omp_and_pi_share_is_adopted_once() {
    let fx = Fx::new();
    let agent = fx.th.home().join(".shared-agent");
    let env = fx
        .env
        .with_var("PI_CODING_AGENT_DIR", agent.clone().into_os_string());
    let sx = sx(&fx, &env);
    let store = fx.store();
    let folder = skill(&fx, &agent.join("skills"), "notes", "Shared.");

    let scan = adopt::scan(Some(&store), &sx, Scope::User, None, None).unwrap();
    let candidate = scan.candidate("notes").unwrap();
    assert!(!candidate.is_conflict());
    assert_eq!(
        candidate.variants[0].clients().collect::<Vec<_>>(),
        ["omp", "pi"]
    );
    let report = adopt::adopt(
        &store,
        &sx,
        Scope::User,
        None,
        None,
        &adopt::default_choices(&scan),
    )
    .unwrap();
    let statuses: Vec<(&str, FolderStatus)> = report
        .folders
        .iter()
        .map(|folder| (folder.client, folder.status))
        .collect();
    assert_eq!(
        statuses,
        [
            ("omp", FolderStatus::Adopted),
            ("pi", FolderStatus::Adopted)
        ]
    );
    assert!(!report.needs_attention(), "{report:#?}");
    assert!(is_link(&folder));
    assert_eq!(trash_entries(&fx).len(), 1);
    assert_eq!(clients_of(&fx, "notes"), list(&["omp", "pi"]));
}

/// `origin = adopted` stays when the entry is remade: link to copy (the
/// link branch) and back (the copy branch).
#[test]
fn the_adopted_origin_survives_a_change_of_method() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let sx = sx(&fx, &env);
    let store = fx.store();
    let folder = skill(&fx, &claude_skills(&fx), "notes", "Take notes.");
    let scan = adopt::scan(Some(&store), &sx, Scope::User, None, None).unwrap();
    adopt::adopt(
        &store,
        &sx,
        Scope::User,
        None,
        None,
        &adopt::default_choices(&scan),
    )
    .unwrap();
    let owned = || {
        SkillLedger::load(&fx.fs, &fx.home.state_dir())
            .unwrap()
            .get(&folder)
            .cloned()
            .unwrap()
    };
    for (method, made) in [
        (DistributionMethod::Copy, LinkMethod::Copy),
        (DistributionMethod::Link, link_method()),
    ] {
        let (_, report) = link::set_method(&store, &sx, "notes", method).unwrap();
        assert!(
            report
                .results
                .iter()
                .any(|result| result.status == LinkStatus::Updated),
            "{report:#?}"
        );
        let owned = owned();
        assert_eq!(owned.method, made);
        assert_eq!(owned.origin(), ORIGIN_ADOPTED, "after {made:?}");
    }
}

/// A `name-<client>` id that a folder next to the variant already uses is
/// skipped: its link would be in the way.
#[test]
fn a_renamed_skill_skips_an_id_a_folder_uses() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let sx = sx(&fx, &env);
    let store = fx.store();
    skill(&fx, &claude_skills(&fx), "diff", "Claude's version.");
    let omp_diff = skill(&fx, &omp_skills(&fx), "diff", "omp's version.");
    let unrelated = skill(&fx, &omp_skills(&fx), "diff-omp", "Something else.");

    let scan = adopt::scan(Some(&store), &sx, Scope::User, None, None).unwrap();
    let claude_variant = scan
        .candidate("diff")
        .unwrap()
        .variants
        .iter()
        .find(|variant| variant.clients().any(|client| client == "claude-code"))
        .unwrap()
        .hash
        .clone();
    let report = adopt::adopt(
        &store,
        &sx,
        Scope::User,
        None,
        None,
        &[AdoptChoice {
            name: "diff".to_owned(),
            action: AdoptAction::Both {
                primary: claude_variant,
            },
        }],
    )
    .unwrap();
    assert_eq!(report.added, ["diff", "diff-omp-2"]);
    let adopted: Vec<(&str, FolderStatus)> = report
        .folders
        .iter()
        .filter(|folder| folder.name == "diff")
        .map(|folder| (folder.client, folder.status))
        .collect();
    assert_eq!(
        adopted,
        [
            ("claude-code", FolderStatus::Adopted),
            ("omp", FolderStatus::Adopted)
        ]
    );
    assert!(is_link(&omp_skills(&fx).join("diff-omp-2")));
    assert!(std::fs::symlink_metadata(&omp_diff).is_err());
    assert!(!is_link(&unrelated) && fx.read(&unrelated.join("SKILL.md")).contains("Something"));
}

/// Denies this account reading `file`'s data (Windows: a deny ACE for
/// Everyone on read data through `icacls`; Unix: mode 000). `false`, after
/// undoing it, where the file still reads (root, or no `icacls`): the
/// caller skips.
fn deny_read(file: &Path) -> bool {
    if cfg!(windows) {
        let denied = std::process::Command::new("icacls")
            .arg(file)
            .args(["/deny", "*S-1-1-0:(RD)"])
            .stdout(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !denied {
            return false;
        }
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
    }
    if std::fs::read(file).is_ok() {
        allow_read(file);
        return false;
    }
    true
}

fn allow_read(file: &Path) {
    if cfg!(windows) {
        let _ = std::process::Command::new("icacls")
            .arg(file)
            .args(["/remove:d", "*S-1-1-0"])
            .stdout(std::process::Stdio::null())
            .status();
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644));
        }
    }
}

/// A store skill with a file this account cannot read (#16 review): the
/// listing reports it and never distributes it, and adopting a folder that
/// would be renamed onto its id takes the next free id instead of failing.
#[test]
fn an_unreadable_store_file_is_listed_and_adopt_skips_its_id() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let sx = sx(&fx, &env);
    let store = fx.store();
    let source = skill(&fx, &fx.th.root().join("src"), "diff-omp", "In the store.");
    agent_loadout_core::skills::install::install(
        &store,
        &sx,
        &agent_loadout_core::skills::InstallRequest::new(
            agent_loadout_core::skills::Source::Local(source),
        ),
    )
    .unwrap();
    let locked = store::content_dir(&store, "diff-omp")
        .unwrap()
        .join("scripts")
        .join("run.sh");
    if !deny_read(&locked) {
        eprintln!("skipped: this account reads the file anyway");
        return;
    }
    let listed = store::list(&store);
    skill(&fx, &claude_skills(&fx), "diff", "Claude's version.");
    skill(&fx, &omp_skills(&fx), "diff", "omp's version.");
    let scan = adopt::scan(Some(&store), &sx, Scope::User, None, None);
    let report = scan.and_then(|scan| {
        let claude = scan
            .candidate("diff")
            .unwrap()
            .variants
            .iter()
            .find(|variant| variant.clients().any(|client| client == "claude-code"))
            .unwrap()
            .hash
            .clone();
        adopt::adopt(
            &store,
            &sx,
            Scope::User,
            None,
            None,
            &[AdoptChoice {
                name: "diff".to_owned(),
                action: AdoptAction::Both { primary: claude },
            }],
        )
    });
    allow_read(&locked);

    let listed = listed.unwrap();
    let locked_skill = listed.iter().find(|skill| skill.id == "diff-omp").unwrap();
    assert!(!locked_skill.has_content, "{locked_skill:#?}");
    assert!(
        locked_skill
            .problems
            .iter()
            .any(|err| err.message().contains("cannot read file")),
        "{locked_skill:#?}"
    );
    assert_eq!(report.unwrap().added, ["diff", "diff-omp-2"]);
}

// The scan cache of the unmanaged-entry notice (#3, #9).

fn cursor_skills(fx: &Fx) -> PathBuf {
    fx.th.home().join(".cursor").join("skills")
}

/// A scan as lines (names, contents, clients, folders, problems); the
/// variants' files are left out (a cached scan does not hold them).
fn shown(scan: &adopt::UnmanagedScan) -> Vec<String> {
    let mut out = Vec::new();
    for candidate in &scan.candidates {
        for variant in &candidate.variants {
            let folders: Vec<String> = variant
                .folders
                .iter()
                .map(|folder| format!("{}={}", folder.client, folder.path.display()))
                .collect();
            out.push(format!(
                "{} {} {} {:?} {:?} {}",
                candidate.name,
                variant.hash,
                variant.in_store,
                variant.description,
                folders,
                variant.bytes
            ));
        }
    }
    for problem in &scan.problems {
        out.push(format!(
            "problem {} {} {}",
            problem.client,
            problem.name,
            problem.error.code()
        ));
    }
    out
}

fn mtime(path: &Path) -> SystemTime {
    std::fs::metadata(path).unwrap().modified().unwrap()
}

fn set_mtime(path: &Path, time: SystemTime) {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

fn hash_of(scan: &adopt::UnmanagedScan, name: &str) -> String {
    scan.candidate(name).unwrap().variants[0].hash.clone()
}

#[test]
fn a_cached_scan_equals_a_full_one_and_edits_invalidate_it() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let sx = sx(&fx, &env);
    let store = fx.store();
    let notes = skill(&fx, &claude_skills(&fx), "notes", "Take notes.");
    skill(&fx, &claude_skills(&fx), "shared", "Same.");
    skill(&fx, &codex_skills(&fx), "shared", "Same.");
    skill(&fx, &claude_skills(&fx), "diff", "Claude's.");
    skill(&fx, &omp_skills(&fx), "diff", "omp's.");
    fx.write(&codex_skills(&fx).join("empty").join("README.md"), "x\n");
    // A store skill one folder equals.
    let source = skill(&fx, &fx.th.root().join("src"), "stored", "In the store.");
    agent_loadout_core::skills::install::install(
        &store,
        &sx,
        &agent_loadout_core::skills::InstallRequest::new(
            agent_loadout_core::skills::Source::Local(source),
        ),
    )
    .unwrap();
    skill(&fx, &cursor_skills(&fx), "stored", "In the store.");

    let cache = FolderHashCache::new();
    let full = || adopt::scan(Some(&store), &sx, Scope::User, None, None).unwrap();
    let cached = || adopt::scan_cached(Some(&store), &sx, Scope::User, None, None, &cache).unwrap();
    let first = cached();
    assert_eq!(shown(&first), shown(&full()));
    assert!(first.candidate("stored").unwrap().variants[0].in_store);
    assert_eq!(
        cache.len(),
        8,
        "the seven folders scanned (one without SKILL.md) and the store copy"
    );
    assert_eq!(shown(&cached()), shown(&full()), "answered from the cache");

    let file = notes.join("scripts").join("run.sh");
    let before = hash_of(&full(), "notes");
    let old = mtime(&file);

    // Same size, new modification time.
    fx.write(&file, "echo no\n");
    set_mtime(&file, old + Duration::from_secs(5));
    let after = hash_of(&full(), "notes");
    assert_ne!(after, before);
    assert_eq!(hash_of(&cached(), "notes"), after);

    // Same modification time, new size.
    let old = mtime(&file);
    fx.write(&file, "echo longer\n");
    set_mtime(&file, old);
    let longer = hash_of(&full(), "notes");
    assert_ne!(longer, after);
    assert_eq!(hash_of(&cached(), "notes"), longer);
    assert_eq!(shown(&cached()), shown(&full()));
}

/// An edit that keeps a file's size and modification time is not seen by
/// the cache; adopt never uses it: a choice made from that stale scan is
/// refused, and the fresh content is what gets stored.
#[test]
fn adopt_reads_again_what_the_cache_holds() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let sx = sx(&fx, &env);
    let store = fx.store();
    let notes = skill(&fx, &claude_skills(&fx), "notes", "Take notes.");
    let cache = FolderHashCache::new();
    let stale = adopt::scan_cached(Some(&store), &sx, Scope::User, None, None, &cache).unwrap();

    let file = notes.join("scripts").join("run.sh");
    let old = mtime(&file);
    fx.write(&file, "echo no\n");
    set_mtime(&file, old);
    let cached = adopt::scan_cached(Some(&store), &sx, Scope::User, None, None, &cache).unwrap();
    assert_eq!(hash_of(&cached, "notes"), hash_of(&stale, "notes"));

    let choice = |variant: String| AdoptChoice {
        name: "notes".to_owned(),
        action: AdoptAction::Pick { variant },
    };
    let err = adopt::adopt(
        &store,
        &sx,
        Scope::User,
        None,
        None,
        &[choice(hash_of(&stale, "notes"))],
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::EntryChanged);
    assert!(!is_link(&notes));

    let fresh = adopt::scan(Some(&store), &sx, Scope::User, None, None).unwrap();
    let report = adopt::adopt(
        &store,
        &sx,
        Scope::User,
        None,
        None,
        &[choice(hash_of(&fresh, "notes"))],
    )
    .unwrap();
    assert_eq!(report.folders[0].status, FolderStatus::Adopted);
    let stored = store::content_dir(&store, "notes").unwrap();
    assert_eq!(fx.read(&stored.join("scripts").join("run.sh")), "echo no\n");
}

/// Timing of the notice's scan over 300 unmanaged folders (100 each in
/// Claude Code, Codex and Cursor, about 100 KB each): full, cached cold,
/// cached warm. `cargo test -p agent-loadout-core --release --test
/// skills_adopt -- --ignored --nocapture perf_`.
#[test]
#[ignore = "timing; run by hand"]
fn perf_scan_300_unmanaged_folders() {
    let fx = Fx::new();
    let env = fx.env.clone();
    let sx = sx(&fx, &env);
    let store = fx.store();
    let chunk = "lorem ipsum dolor sit amet\n".repeat(900);
    for (n, dir) in [claude_skills(&fx), codex_skills(&fx), cursor_skills(&fx)]
        .iter()
        .enumerate()
    {
        for i in 0..100 {
            let folder = skill(&fx, dir, &format!("skill-{n}-{i:03}"), "Fixture.");
            for f in 0..4 {
                fx.write(&folder.join("docs").join(format!("part-{f}.md")), &chunk);
            }
        }
    }
    let time = |what: &str, run: &dyn Fn() -> adopt::UnmanagedScan| {
        let start = Instant::now();
        let scan = run();
        let took = start.elapsed();
        println!(
            "{what:<14} {:>8.1} ms ({} candidates)",
            took.as_secs_f64() * 1000.0,
            scan.candidates.len()
        );
        scan
    };
    let cache = FolderHashCache::new();
    let full = time("full", &|| {
        adopt::scan(Some(&store), &sx, Scope::User, None, None).unwrap()
    });
    let cold = time("cached, cold", &|| {
        adopt::scan_cached(Some(&store), &sx, Scope::User, None, None, &cache).unwrap()
    });
    let warm = time("cached, warm", &|| {
        adopt::scan_cached(Some(&store), &sx, Scope::User, None, None, &cache).unwrap()
    });
    time("full, again", &|| {
        adopt::scan(Some(&store), &sx, Scope::User, None, None).unwrap()
    });
    // What a warm scan still costs: listing and classifying the folders.
    let start = Instant::now();
    classify::scan(&sx, Scope::User, None).unwrap();
    println!(
        "{:<14} {:>8.1} ms",
        "classify only",
        start.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(full.candidates.len(), 300);
    assert_eq!(shown(&cold), shown(&full));
    assert_eq!(shown(&warm), shown(&full));
}

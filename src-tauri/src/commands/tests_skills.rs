//! M19: Skills and Install page commands against a temp home (SPEC 12,
//! 13.7, 17). `RootedFs` panics on any path outside the temp root.

use std::path::{Path, PathBuf};

use agent_loadout_core::ErrorCode;
use agent_loadout_testkit::git_env::{git_vars, init_bare, system_git_or_skip};

use super::skills::{
    self, SkillEntryClass, SkillFileChangeKind, SkillLinkMethod, SkillLinkStatus, SkillScope,
    SkillUpdateStatus,
};
use super::tests::Fx;
use super::{FileScope, projects};

const NOTES: &str = "---\nname: repo-notes\ndescription: Notes about this repository.\n\
                     license: MIT\nmetadata:\n  team: core\n---\n\n# Repo notes\n";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A skill folder outside the store, as the folder picker would return it.
fn skill_folder(fx: &Fx, name: &str, text: &str) -> PathBuf {
    let dir = fx.th.projects().join("downloads").join(name);
    write(&dir.join("SKILL.md"), text);
    write(&dir.join("scripts").join("check.sh"), "echo ok\n");
    dir
}

fn claude_skills(fx: &Fx) -> PathBuf {
    fx.th.home().join(".claude").join("skills")
}

/// A junction (Windows) or a symbolic link, not a folder.
fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

fn assert_no_root(fx: &Fx, json: &str) {
    let root = fx.th.root().to_string_lossy().into_owned();
    for form in [
        root.clone(),
        root.replace('\\', "/"),
        root.replace('\\', "\\\\"),
    ] {
        assert!(!json.contains(&form), "{json}");
    }
}

/// Commits after the store's initial one.
fn commits(fx: &Fx) -> usize {
    match fx.core.open_store().unwrap() {
        Some(store) => store
            .log(&agent_loadout_core::store::LogFilter::default())
            .unwrap()
            .len()
            .saturating_sub(1),
        None => 0,
    }
}

fn summaries(fx: &Fx) -> Vec<String> {
    fx.core
        .open_store()
        .unwrap()
        .map(|store| {
            store
                .log(&agent_loadout_core::store::LogFilter::default())
                .unwrap()
                .into_iter()
                .map(|entry| entry.summary)
                .collect()
        })
        .unwrap_or_default()
}

/// The brief's acceptance: install a local skill, enable it for Claude
/// Code, and the link exists (a junction on Windows, made without admin
/// rights); one commit per change; nothing crosses IPC with the temp root.
#[test]
fn install_a_local_skill_enable_for_claude_code_and_the_link_exists() {
    let fx = Fx::new();
    let folder = skill_folder(&fx, "repo-notes", NOTES);

    let (path, shown, name) = skills::inspect_source(&fx.core, &folder).unwrap();
    assert_eq!(path, folder);
    assert_eq!(name, "repo-notes");
    assert!(shown.starts_with('~') || shown.starts_with('%'), "{shown}");

    let installed = skills::install_local(&fx.core, &folder, None, false).unwrap();
    assert_eq!(installed.id, "repo-notes");
    assert!(installed.commit.is_some());
    assert_eq!(commits(&fx), 1, "the install is one commit");
    assert_eq!(summaries(&fx)[0], "skill: add repo-notes from repo-notes");

    let list = skills::list(&fx.core, &SkillScope::User).unwrap();
    assert_eq!(list.skills.len(), 1);
    let skill = &list.skills[0];
    assert_eq!(skill.name, "repo-notes");
    assert_eq!(
        skill.description.as_deref(),
        Some("Notes about this repository.")
    );
    let keys: Vec<&str> = skill.fields.iter().map(|f| f.key.as_str()).collect();
    assert_eq!(keys, ["name", "description", "license", "metadata"]);
    assert_eq!(skill.fields[3].value, "{\"team\":\"core\"}");
    assert!(skill.clients.is_empty() && skill.links.is_empty());
    assert!(skill.has_content);
    assert!(
        list.clients.iter().any(|client| client.id == "claude-code"),
        "Claude Code has a skill folder"
    );

    // Enable for Claude Code: one commit, and the link follows at once.
    let change = skills::set_clients(
        &fx.core,
        "repo-notes",
        &["claude-code".to_owned()],
        true,
        None,
    )
    .unwrap();
    assert!(change.commit.is_some());
    assert_eq!(commits(&fx), 2);
    assert_eq!(
        summaries(&fx)[0],
        "skill: enable repo-notes for claude-code"
    );
    assert!(!change.links.needs_attention, "{:?}", change.links);
    let note = &change.links.notes[0];
    assert_eq!(note.client, "claude-code");
    assert_eq!(note.status, SkillLinkStatus::Created);
    assert_eq!(note.path, "~/.claude/skills/repo-notes");

    let link = claude_skills(&fx).join("repo-notes");
    assert!(is_link(&link), "a link, not a copy");
    let through = std::fs::read_to_string(link.join("SKILL.md")).unwrap();
    assert!(through.contains("Notes about this repository."));
    assert!(link.join("scripts").join("check.sh").is_file());

    let list = skills::list(&fx.core, &SkillScope::User).unwrap();
    let skill = &list.skills[0];
    assert_eq!(skill.clients, ["claude-code"]);
    assert!(!skill.all_detected);
    assert_eq!(skill.links.len(), 1);
    assert_eq!(skill.links[0].scope, FileScope::User);
    let method = if cfg!(windows) {
        SkillLinkMethod::Junction
    } else {
        SkillLinkMethod::Symlink
    };
    assert_eq!(skill.links[0].method, method);
    assert_no_root(&fx, &serde_json::to_string(&list).unwrap());

    // The Claude Code folder lists it as managed.
    let dirs = skills::tool_dirs(&fx.core, &SkillScope::User).unwrap();
    let claude = dirs.iter().find(|dir| dir.client == "claude-code").unwrap();
    assert!(claude.exists);
    assert_eq!(claude.dir, "~/.claude/skills");
    assert_eq!(claude.entries.len(), 1);
    assert_eq!(claude.entries[0].class, SkillEntryClass::Managed);
    assert_eq!(claude.entries[0].skill_id.as_deref(), Some("repo-notes"));
    assert_no_root(&fx, &serde_json::to_string(&dirs).unwrap());

    // Disable: the link goes, the store copy stays.
    let change = skills::set_clients(
        &fx.core,
        "repo-notes",
        &["claude-code".to_owned()],
        false,
        None,
    )
    .unwrap();
    assert_eq!(change.links.notes[0].status, SkillLinkStatus::Removed);
    assert!(!link.exists() && !is_link(&link));
    assert_eq!(commits(&fx), 3);
    let files = skills::files(&fx.core, "repo-notes").unwrap();
    let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["SKILL.md", "scripts/check.sh"]);
}

#[test]
fn picked_sources_are_validated() {
    let fx = Fx::new();
    let code = |path: &Path| skills::inspect_source(&fx.core, path).unwrap_err().code;
    assert_eq!(code(Path::new("relative/skill")), ErrorCode::InvalidPath);
    let folder = skill_folder(&fx, "a", NOTES);
    assert_eq!(
        code(&folder.join("..").join("a")),
        ErrorCode::InvalidPath,
        "`..` parts"
    );
    assert_eq!(code(&fx.th.projects().join("gone")), ErrorCode::NotFound);
    let text = fx.th.projects().join("notes.txt");
    write(&text, "hello");
    assert_eq!(code(&text), ErrorCode::InvalidValue, "not an archive");
    let data = fx.core.home.path.join("store").join("skills").join("x");
    std::fs::create_dir_all(&data).unwrap();
    assert_eq!(code(&data), ErrorCode::InvalidPath, "the data directory");
    let archive = fx.th.projects().join("bundle.SKILL");
    write(&archive, "not checked here");
    assert_eq!(
        skills::inspect_source(&fx.core, &archive).unwrap().2,
        "bundle.SKILL"
    );
    let err = skills::inspect_source(&fx.core, &text).unwrap_err();
    assert_no_root(&fx, &err.message);

    // The install checks again: nothing reaches the store.
    let err = skills::install_local(&fx.core, &data, None, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidPath);
    assert_eq!(commits(&fx), 0);
}

/// Git installs take URLs only; a local path is picked in the dialog.
#[test]
fn git_installs_take_urls_not_paths() {
    let fx = Fx::new();
    let folder = skill_folder(&fx, "repo-notes", NOTES);
    for url in [
        folder.to_string_lossy().into_owned(),
        "repo-notes".to_owned(),
    ] {
        let err = skills::install_git(&fx.core, &url, None, None, None, false).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidValue, "{url}");
    }
    assert_eq!(commits(&fx), 0);
}

#[test]
fn a_taken_name_needs_confirmation_then_gets_a_suffix() {
    let fx = Fx::new();
    let folder = skill_folder(&fx, "repo-notes", NOTES);
    skills::install_local(&fx.core, &folder, None, false).unwrap();
    let err = skills::install_local(&fx.core, &folder, None, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::AlreadyExists);
    assert_eq!(commits(&fx), 1);
    let again = skills::install_local(&fx.core, &folder, None, true).unwrap();
    assert_eq!(again.id, "repo-notes-2");
    assert_eq!(again.renamed_from.as_deref(), Some("repo-notes"));
    // An id the user typed.
    let named =
        skills::install_local(&fx.core, &folder, Some(" notes ".to_owned()), false).unwrap();
    assert_eq!(named.id, "notes");
}

#[test]
fn tags_files_and_remove() {
    let fx = Fx::new();
    let folder = skill_folder(&fx, "repo-notes", NOTES);
    skills::install_local(&fx.core, &folder, None, false).unwrap();

    let tags = ["team".to_owned(), " Team ".to_owned(), String::new()];
    assert!(
        skills::set_tags(&fx.core, "repo-notes", &tags)
            .unwrap()
            .is_some()
    );
    assert_eq!(summaries(&fx)[0], "skill: tag repo-notes");
    let list = skills::list(&fx.core, &SkillScope::User).unwrap();
    assert_eq!(list.skills[0].tags, ["team"]);
    assert!(
        skills::set_tags(&fx.core, "repo-notes", &["team".to_owned()])
            .unwrap()
            .is_none(),
        "unchanged: no commit"
    );
    let err = skills::set_tags(&fx.core, "repo-notes", &["x".repeat(21)]).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidValue);

    let file = skills::read_file(&fx.core, "repo-notes", "SKILL.md").unwrap();
    assert!(file.text.unwrap().contains("# Repo notes"));
    for bad in ["../SKILL.md", "missing.md", ""] {
        let err = skills::read_file(&fx.core, "repo-notes", bad).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound, "{bad}");
    }
    let err = skills::files(&fx.core, "../store").unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidId);

    skills::set_clients(
        &fx.core,
        "repo-notes",
        &["claude-code".to_owned()],
        true,
        None,
    )
    .unwrap();
    let link = claude_skills(&fx).join("repo-notes");
    assert!(is_link(&link));
    let removed = skills::remove(&fx.core, "repo-notes").unwrap();
    assert!(removed.commit.is_some());
    assert!(!is_link(&link), "the link goes with the skill");
    assert!(folder.join("SKILL.md").is_file(), "the source is untouched");
    assert!(
        skills::list(&fx.core, &SkillScope::User)
            .unwrap()
            .skills
            .is_empty()
    );
}

/// A known, trusted project: the page lists project-enabled skills, and
/// an enable needs the manifest hash the page showed.
#[test]
fn project_scope_lists_and_enables_with_the_shown_hash() {
    let fx = Fx::new();
    let folder = skill_folder(&fx, "repo-notes", NOTES);
    skills::install_local(&fx.core, &folder, None, false).unwrap();
    let root = fx.th.projects().join("web");
    std::fs::create_dir_all(&root).unwrap();
    let done = projects::init(&fx.core, &root, false, None).unwrap();
    let scope = SkillScope::Project {
        id: done.id.clone(),
    };

    let list = skills::list(&fx.core, &scope).unwrap();
    let project = list.project.clone().unwrap();
    assert_eq!(project.id, done.id);
    assert_eq!(project.trust, projects::ProjectTrust::Trusted);
    assert!(!project.share);
    assert!(!list.skills[0].project_enabled);

    // Content other than what was shown: nothing written.
    let before = commits(&fx);
    let err =
        skills::set_project(&fx.core, &done.id, "repo-notes", true, "sha256:other").unwrap_err();
    assert_eq!(err.code, ErrorCode::TrustRequired);
    assert_eq!(commits(&fx), before);
    assert_no_root(&fx, &err.message);

    let change =
        skills::set_project(&fx.core, &done.id, "repo-notes", true, &project.hash).unwrap();
    assert!(change.commit.is_some());
    assert_eq!(commits(&fx), before + 1);
    let manifest =
        std::fs::read_to_string(root.join(".loadout").join("project.local.toml")).unwrap();
    assert!(manifest.contains("\"repo-notes\""), "{manifest}");
    let list = skills::list(&fx.core, &scope).unwrap();
    assert!(list.skills[0].project_enabled);
    assert!(!list.skills[0].project_shared);
    assert_no_root(&fx, &serde_json::to_string(&list).unwrap());

    let dirs = skills::tool_dirs(&fx.core, &scope).unwrap();
    let claude = dirs.iter().find(|dir| dir.client == "claude-code").unwrap();
    assert_eq!(claude.dir, "<P>/.claude/skills");

    let err = skills::list(
        &fx.core,
        &SkillScope::Project {
            id: "nope-0000".to_owned(),
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

/// A real folder in a client's skill folder is unmanaged: it can be
/// imported (and stays) or moved to the trash (never deleted).
#[test]
fn unmanaged_entries_are_imported_or_trashed() {
    let fx = Fx::new();
    write(
        &claude_skills(&fx).join("hand-made").join("SKILL.md"),
        "---\nname: hand-made\ndescription: Made by hand.\n---\n",
    );
    write(
        &claude_skills(&fx).join("old-one").join("SKILL.md"),
        "---\nname: old-one\n---\n",
    );
    let dirs = skills::tool_dirs(&fx.core, &SkillScope::User).unwrap();
    let claude = dirs.iter().find(|dir| dir.client == "claude-code").unwrap();
    assert!(
        claude
            .entries
            .iter()
            .all(|entry| entry.class == SkillEntryClass::Unmanaged)
    );
    assert_eq!(
        claude.entries[0].description.as_deref(),
        Some("Made by hand.")
    );

    let imported = skills::import_entry(
        &fx.core,
        "claude-code",
        &SkillScope::User,
        "hand-made",
        None,
        false,
    )
    .unwrap();
    assert_eq!(imported.id, "hand-made");
    assert!(
        claude_skills(&fx)
            .join("hand-made")
            .join("SKILL.md")
            .is_file()
    );

    let trashed =
        skills::trash_entry(&fx.core, "claude-code", &SkillScope::User, "old-one").unwrap();
    assert!(!claude_skills(&fx).join("old-one").exists());
    assert!(trashed.ends_with("-claude-code-old-one"), "{trashed}");
    assert_no_root(&fx, &trashed);
    let trash = fx.core.home.backups_dir().join("trash");
    assert_eq!(std::fs::read_dir(trash).unwrap().count(), 1);

    let err = skills::trash_entry(&fx.core, "claude-code", &SkillScope::User, "../x").unwrap_err();
    assert_ne!(err.code, ErrorCode::IoError);
}

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

/// A git skill: the update check lists changed files, the diff shows the
/// store copy against the fetched source, and the update commits.
#[test]
fn git_skill_update_check_diff_and_update() {
    let Some(git) = system_git_or_skip("git_skill_update_check_diff_and_update") else {
        return;
    };
    let fx = Fx::with_git("test-desk", &git);
    let home = fx.th.home();
    let bare = home.join("up.git");
    init_bare(&fx.th, &git, &bare);
    git_run(
        &fx,
        &git,
        &home,
        &["init", "--quiet", "--initial-branch=main", "work"],
    );
    let work = home.join("work");
    git_run(&fx, &git, &work, &["remote", "add", "origin", "../up.git"]);
    write(&work.join("skills").join("alpha").join("SKILL.md"), NOTES);
    let push = |message: &str| {
        git_run(&fx, &git, &work, &["add", "-A"]);
        git_run(&fx, &git, &work, &["commit", "--quiet", "-m", message]);
        git_run(
            &fx,
            &git,
            &work,
            &["push", "--quiet", "origin", "HEAD:main"],
        );
    };
    push("initial");

    let url = format!(
        "file:///{}",
        bare.to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches('/')
    );
    let installed = skills::install_git(
        &fx.core,
        &url,
        Some("skills/alpha".to_owned()),
        None,
        Some("alpha".to_owned()),
        false,
    )
    .unwrap();
    assert_eq!(installed.id, "alpha");
    let list = skills::list(&fx.core, &SkillScope::User).unwrap();
    let source = list.skills[0].source.clone().unwrap();
    assert_eq!(source.path.as_deref(), Some("skills/alpha"));
    assert_eq!(source.url.as_deref(), Some("file:///~/up.git"));
    assert_no_root(&fx, &serde_json::to_string(&list).unwrap());

    let checks = skills::check_updates(&fx.core, &[]).unwrap();
    assert_eq!(checks[0].status, SkillUpdateStatus::UpToDate);

    let v2 = NOTES.replace("# Repo notes", "# Repo notes v2");
    write(&work.join("skills").join("alpha").join("SKILL.md"), &v2);
    push("v2");
    let checks = skills::check_updates(&fx.core, &["alpha".to_owned()]).unwrap();
    assert_eq!(checks[0].status, SkillUpdateStatus::Available);
    assert_eq!(checks[0].changes[0].path, "SKILL.md");
    assert_eq!(checks[0].changes[0].kind, SkillFileChangeKind::Modified);

    let diff = skills::update_diff(&fx.core, "alpha", "SKILL.md").unwrap();
    assert!(!diff.binary);
    assert!(diff.before.unwrap().contains("# Repo notes\n"));
    assert!(diff.after.unwrap().contains("# Repo notes v2"));
    assert_eq!(
        skills::update_diff(&fx.core, "alpha", "nope.md")
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );

    // Upstream moves on after the check: the update installs what was
    // reviewed (v2) without fetching.
    let reviewed = checks[0].latest.clone().unwrap();
    let v3 = NOTES.replace("# Repo notes", "# Repo notes v3");
    write(&work.join("skills").join("alpha").join("SKILL.md"), &v3);
    push("v3");
    let before = commits(&fx);
    let updated = skills::update(&fx.core, "alpha", &reviewed).unwrap();
    assert!(updated.commit.is_some());
    assert_eq!(updated.to, reviewed);
    assert_eq!(commits(&fx), before + 1);
    let file = skills::read_file(&fx.core, "alpha", "SKILL.md").unwrap();
    let text = file.text.unwrap();
    assert!(text.contains("v2") && !text.contains("v3"), "{text}");

    // A later check sees v3: the old review no longer applies.
    let checks = skills::check_updates(&fx.core, &["alpha".to_owned()]).unwrap();
    assert_eq!(checks[0].status, SkillUpdateStatus::Available);
    let newest = checks[0].latest.clone().unwrap();
    assert_ne!(newest, reviewed);
    let before = commits(&fx);
    let err = skills::update(&fx.core, "alpha", &reviewed).unwrap_err();
    assert_eq!(err.code, ErrorCode::EntryChanged);
    assert_no_root(&fx, &err.message);
    assert_eq!(commits(&fx), before, "nothing written");
    skills::update(&fx.core, "alpha", &newest).unwrap();
    let text = skills::read_file(&fx.core, "alpha", "SKILL.md")
        .unwrap()
        .text
        .unwrap();
    assert!(text.contains("v3"));
}

/// One skill's failed check is that skill's entry; the others are checked.
#[test]
fn a_failed_check_does_not_fail_the_others() {
    let fx = Fx::new();
    let folder = skill_folder(&fx, "repo-notes", NOTES);
    skills::install_local(&fx.core, &folder, None, false).unwrap();
    let checks = skills::check_updates(
        &fx.core,
        &["gone-skill".to_owned(), "repo-notes".to_owned()],
    )
    .unwrap();
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0].id, "gone-skill");
    assert_eq!(checks[0].status, SkillUpdateStatus::Failed);
    assert_eq!(checks[0].error.as_ref().unwrap().code, ErrorCode::NotFound);
    assert_eq!(checks[1].status, SkillUpdateStatus::NotTracked);
    // A reviewed update of a skill without a git source is refused.
    let err = skills::update(&fx.core, "repo-notes", "0123abc").unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidValue);
}

/// Link or copy changes only the method: the clients stay as they are.
#[test]
fn switching_to_copies_keeps_the_clients() {
    let fx = Fx::new();
    let folder = skill_folder(&fx, "repo-notes", NOTES);
    skills::install_local(&fx.core, &folder, None, false).unwrap();
    let off = skill_folder(&fx, "off-skill", &NOTES.replace("repo-notes", "off-skill"));
    skills::install_local(&fx.core, &off, None, false).unwrap();
    skills::set_clients(
        &fx.core,
        "repo-notes",
        &["claude-code".to_owned()],
        true,
        None,
    )
    .unwrap();
    assert!(is_link(&claude_skills(&fx).join("repo-notes")));

    let before = commits(&fx);
    let change = skills::set_method(&fx.core, "repo-notes", skills::SkillMethod::Copy).unwrap();
    assert!(change.commit.is_some());
    assert_eq!(commits(&fx), before + 1);
    assert_eq!(summaries(&fx)[0], "skill: distribute repo-notes as copy");
    let copy = claude_skills(&fx).join("repo-notes");
    assert!(
        !is_link(&copy) && copy.join("SKILL.md").is_file(),
        "now a copy"
    );
    let list = skills::list(&fx.core, &SkillScope::User).unwrap();
    let skill = list
        .skills
        .iter()
        .find(|skill| skill.id == "repo-notes")
        .unwrap();
    assert_eq!(skill.clients, ["claude-code"]);
    assert!(!skill.all_detected);
    assert_eq!(skill.method, skills::SkillMethod::Copy);

    // An off skill stays off.
    skills::set_method(&fx.core, "off-skill", skills::SkillMethod::Copy).unwrap();
    let list = skills::list(&fx.core, &SkillScope::User).unwrap();
    let skill = list
        .skills
        .iter()
        .find(|skill| skill.id == "off-skill")
        .unwrap();
    assert!(skill.clients.is_empty() && !skill.all_detected);
    assert!(!claude_skills(&fx).join("off-skill").exists());
}

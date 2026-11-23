//! Bring your setup in (#3): Discover projects and skill adopt commands
//! against a temp home. Client files are written into the temp root;
//! `RootedFs` panics on any path outside it. Nothing that crosses IPC names
//! the temp root.

use std::path::{Path, PathBuf};

use agent_loadout_core::ErrorCode;

use super::discover;
use super::projects::{self, ProjectTrust};
use super::skills::{self, SkillAdoptAction, SkillAdoptChoice, SkillFolderStatus, SkillScope};
use super::tests::Fx;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
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

fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

fn skill(dir: &Path, name: &str, body: &str) -> PathBuf {
    let folder = dir.join(name);
    write(
        &folder.join("SKILL.md"),
        &format!("---\nname: {name}\ndescription: {body}\n---\n"),
    );
    folder
}

/// Claude Code knows `app` (no manifest) and `lib` (a shared manifest);
/// Codex trusts `lib` too.
fn known_folders(fx: &Fx) -> (PathBuf, PathBuf) {
    let app = fx.th.home().join("code").join("app");
    let lib = fx.th.home().join("code").join("lib");
    std::fs::create_dir_all(&app).unwrap();
    write(
        &lib.join(".loadout").join("project.toml"),
        "schema = 1\nid = \"lib-0001\"\nname = \"Lib\"\n",
    );
    let mut map = serde_json::Map::new();
    for root in [&app, &lib] {
        map.insert(root.to_string_lossy().into_owned(), serde_json::json!({}));
    }
    write(
        &fx.claude(),
        &serde_json::json!({ "projects": map }).to_string(),
    );
    write(
        &fx.th.home().join(".codex").join("config.toml"),
        &format!(
            "[projects.'{}']\ntrust_level = \"trusted\"\n",
            lib.to_string_lossy()
        ),
    );
    (app, lib)
}

#[test]
fn discover_lists_known_folders_and_adds_them_untrusted() {
    let fx = Fx::new();
    let (app, lib) = known_folders(&fx);

    let found = discover::discover_list(&fx.core).unwrap();
    assert!(found.warnings.is_empty());
    let json = serde_json::to_string(&found).unwrap();
    assert_no_root(&fx, &json);
    let rows: Vec<(&str, &str, Vec<&str>)> = found
        .candidates
        .iter()
        .map(|row| {
            (
                row.name.as_str(),
                row.path.as_str(),
                row.clients.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("app", "~/code/app", vec!["claude-code"]),
            ("lib", "~/code/lib", vec!["claude-code", "codex"]),
        ]
    );
    assert!(found.candidates[0].manifest.is_none());
    let manifest = found.candidates[1].manifest.as_ref().unwrap();
    assert_eq!(manifest.id, "lib-0001");
    assert_eq!(manifest.layout, projects::ManifestLayout::Shared);

    // The store exists before the count (its first commit is its init).
    fx.store();
    let before = fx.commits();
    let ids: Vec<String> = found.candidates.iter().map(|row| row.id.clone()).collect();
    let added = discover::add_discovered(&fx.core, &ids).unwrap();
    assert_no_root(&fx, &serde_json::to_string(&added).unwrap());
    assert!(added.commit.is_some());
    assert_eq!(fx.commits(), before + 1, "one commit");
    assert!(added.failed.is_empty());
    assert!(
        added
            .added
            .iter()
            .all(|row| row.trust == ProjectTrust::Untrusted),
        "adding never trusts"
    );
    assert!(app.join(".loadout").join("project.local.toml").is_file());
    assert!(!lib.join(".loadout").join("project.local.toml").exists());

    // The Projects page lists both, waiting for their review.
    let rows = projects::list(&fx.core).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.trust == ProjectTrust::Untrusted));
    assert!(
        discover::discover_list(&fx.core)
            .unwrap()
            .candidates
            .is_empty()
    );

    let err = discover::add_discovered(&fx.core, &ids).unwrap_err();
    assert_eq!(err.code, ErrorCode::EntryChanged);
}

#[test]
fn adopt_groups_folders_and_replaces_them_with_links() {
    let fx = Fx::new();
    let claude = fx.th.home().join(".claude").join("skills");
    let cursor = fx.th.home().join(".cursor").join("skills");
    let notes_claude = skill(&claude, "notes", "Same.");
    let notes_cursor = skill(&cursor, "notes", "Same.");
    skill(&claude, "lint", "Claude's lint.");
    let lint_cursor = skill(&cursor, "lint", "Cursor's lint.");

    let scan = skills::scan_unmanaged(&fx.core, &SkillScope::User, None).unwrap();
    assert_no_root(&fx, &serde_json::to_string(&scan).unwrap());
    let names: Vec<(&str, bool)> = scan
        .candidates
        .iter()
        .map(|candidate| (candidate.name.as_str(), candidate.conflict))
        .collect();
    assert_eq!(names, [("lint", true), ("notes", false)]);
    let notes = &scan.candidates[1].variants[0];
    let folders: Vec<(&str, &str)> = notes
        .folders
        .iter()
        .map(|folder| (folder.client.as_str(), folder.path.as_str()))
        .collect();
    assert_eq!(
        folders,
        [
            ("claude-code", "~/.claude/skills/notes"),
            ("cursor", "~/.cursor/skills/notes"),
        ]
    );
    let lint = &scan.candidates[0];
    let cursor_lint = lint
        .variants
        .iter()
        .find(|variant| variant.folders.iter().any(|f| f.client == "cursor"))
        .unwrap();

    fx.store();
    let before = fx.commits();
    let choices = [
        SkillAdoptChoice {
            name: "notes".to_owned(),
            action: SkillAdoptAction::Pick {
                variant: notes.hash.clone(),
            },
        },
        SkillAdoptChoice {
            name: "lint".to_owned(),
            action: SkillAdoptAction::Pick {
                variant: cursor_lint.hash.clone(),
            },
        },
    ];
    let result = skills::adopt_folders(&fx.core, &SkillScope::User, None, &choices).unwrap();
    assert_no_root(&fx, &serde_json::to_string(&result).unwrap());
    assert_eq!(fx.commits(), before + 1, "one commit for the action");
    assert_eq!(result.added, ["lint", "notes"]);
    let statuses: Vec<(&str, &str, SkillFolderStatus)> = result
        .folders
        .iter()
        .map(|folder| (folder.name.as_str(), folder.client.as_str(), folder.status))
        .collect();
    assert_eq!(
        statuses,
        [
            ("lint", "claude-code", SkillFolderStatus::Differs),
            ("lint", "cursor", SkillFolderStatus::Adopted),
            ("notes", "claude-code", SkillFolderStatus::Adopted),
            ("notes", "cursor", SkillFolderStatus::Adopted),
        ]
    );
    // A placeholder path (the test's data directory is outside every
    // placeholder root, so only the entry's name shows).
    let trash = result.folders[1].trash.as_deref().unwrap();
    assert!(trash.ends_with("-cursor-lint"), "{trash}");
    assert!(is_link(&notes_claude) && is_link(&notes_cursor) && is_link(&lint_cursor));
    assert!(!is_link(&claude.join("lint")));
    assert!(!result.needs_attention, "{result:#?}");

    let list = skills::list(&fx.core, &SkillScope::User).unwrap();
    let notes = list
        .skills
        .iter()
        .find(|skill| skill.id == "notes")
        .unwrap();
    assert_eq!(notes.clients, ["claude-code", "cursor"]);
    assert_eq!(notes.links.len(), 2);

    // A stale choice is refused.
    let err = skills::adopt_folders(
        &fx.core,
        &SkillScope::User,
        None,
        &[SkillAdoptChoice {
            name: "lint".to_owned(),
            action: SkillAdoptAction::Both {
                primary: "sha256:stale".to_owned(),
            },
        }],
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::EntryChanged);
}

//! M41: MCP servers from a Git repository or folder through the app's
//! commands (#5): the review (drafts, inputs, trust), the save with the
//! dialog's answer and the editor's edits, the update check, and that no
//! absolute path or secret value crosses IPC. Git sources are local bare
//! repositories in the temp home.

use std::path::{Path, PathBuf};

use agent_loadout_core::ErrorCode;
use agent_loadout_testkit::git_env::{git_vars, init_bare, system_git_or_skip};

use super::mcp_source::{self, McpDraftKind, McpDraftPick, McpSourceKind};
use super::skills::SkillUpdateStatus;
use super::sources::TrustGrantArg;
use super::tests::Fx;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/mcp-sources")
        .join(name)
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

#[test]
fn a_registry_repository_is_reviewed_and_saved_with_the_dialogs_answer() {
    let Some(git) =
        system_git_or_skip("a_registry_repository_is_reviewed_and_saved_with_the_dialogs_answer")
    else {
        return;
    };
    let fx = Fx::with_git("test-desk", &git);
    let home = fx.th.home();
    let bare = home.join("weather.git");
    init_bare(&fx.th, &git, &bare);
    git_run(
        &fx,
        &git,
        &home,
        &["init", "--quiet", "--initial-branch=main", "work"],
    );
    let work = home.join("work");
    git_run(
        &fx,
        &git,
        &work,
        &["remote", "add", "origin", "../weather.git"],
    );
    copy_dir(&fixture("registry"), &work);
    git_run(&fx, &git, &work, &["add", "-A"]);
    git_run(&fx, &git, &work, &["commit", "--quiet", "-m", "initial"]);
    git_run(
        &fx,
        &git,
        &work,
        &["push", "--quiet", "origin", "HEAD:main"],
    );
    let url = format!(
        "file:///{}",
        bare.to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches('/')
    );

    let err = mcp_source::review_git(&fx.core, "not a url", None, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidValue);

    let (mut resolved, review) = mcp_source::review_git(&fx.core, &url, None, None).unwrap();
    assert_eq!(review.source.kind, McpSourceKind::Git);
    assert_eq!(review.source.git_ref.as_deref(), Some("main"));
    let keys: Vec<&str> = review.drafts.iter().map(|d| d.key.as_str()).collect();
    assert_eq!(keys, ["server.json#packages[0]", "server.json#remotes[0]"]);
    let npm = &review.drafts[0];
    assert_eq!(npm.kind, McpDraftKind::Registry);
    assert_eq!(npm.launch, "npx -y @acme/weather-mcp@1.2.0");
    assert!(!npm.runs_repo_code);
    let secret = npm.inputs.iter().find(|input| input.secret).unwrap();
    assert_eq!(secret.alias.as_deref(), Some("WEATHER_WEATHER_API_KEY"));
    assert!(review.trust.is_some());
    assert!(review.moved_trust.is_none());
    fx.assert_no_root(&serde_json::to_string(&review).unwrap());

    // The dialog was not answered: nothing is saved, the review stays usable.
    let mut draft = npm.draft.clone();
    draft.id = "forecast".to_owned();
    draft.clients.clients = vec!["claude-code".to_owned()];
    let picks = vec![McpDraftPick {
        key: npm.key.clone(),
        draft,
        trust: None,
    }];
    let err = mcp_source::save(&fx.core, &mut resolved, &picks, None, None, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::SourceTrustRequired);
    let saved = mcp_source::save(
        &fx.core,
        &mut resolved,
        &picks,
        Some(TrustGrantArg::Once),
        None,
        false,
    )
    .unwrap();
    assert_eq!(saved.ids, ["forecast"]);
    assert!(saved.change.commit.is_some());
    let text = fx.store_file("servers/forecast.toml");
    assert!(text.contains("origin = \"source:git\""), "{text}");
    assert!(text.contains("clients = [\"claude-code\"]"), "{text}");

    let check = mcp_source::check_update(&fx.core, "forecast").unwrap();
    assert_eq!(check.status, SkillUpdateStatus::UpToDate);
    fx.assert_no_root(&serde_json::to_string(&check).unwrap());
}

#[test]
fn a_local_folder_is_shown_under_its_placeholder_and_gated() {
    let fx = Fx::new();
    let folder = fx.th.home().join("code").join("notes");
    copy_dir(&fixture("configs"), &folder);
    let (mut resolved, review) = mcp_source::review_local(&fx.core, &folder).unwrap();
    assert_eq!(review.source.kind, McpSourceKind::Local);
    assert_eq!(review.source.folder.as_deref(), Some("~/code/notes"));
    let draft = &review.drafts[0];
    assert_eq!(draft.also_in, [".cursor/mcp.json"]);
    assert!(draft.runs_repo_code);
    assert!(!draft.checkout);
    assert_eq!(draft.draft.cwd.as_deref(), Some("${home}/code/notes"));
    let input = draft.inputs.iter().find(|input| input.secret).unwrap();
    assert!(input.value_dropped);
    let json = serde_json::to_string(&review).unwrap();
    assert!(!json.contains("not-a-real-key-123"), "{json}");
    fx.assert_no_root(&json);

    let picks = vec![McpDraftPick {
        key: draft.key.clone(),
        draft: draft.draft.clone(),
        trust: None,
    }];
    let err = mcp_source::save(&fx.core, &mut resolved, &picks, None, None, false).unwrap_err();
    assert_eq!(err.code, ErrorCode::SourceTrustRequired);
    mcp_source::save(
        &fx.core,
        &mut resolved,
        &picks,
        Some(TrustGrantArg::Once),
        None,
        false,
    )
    .unwrap();
    let text = fx.store_file("servers/notes.toml");
    assert!(!text.contains("not-a-real-key-123"), "{text}");
    assert!(text.contains("${secret:NOTES_NOTES_API_KEY}"), "{text}");
}

/// A bare repository `~/<name>.git` holding `fixtures/mcp-sources/<fixture>`
/// and its `file://` URL.
fn upstream(fx: &Fx, git: &Path, fixture: &str, name: &str) -> String {
    let home = fx.th.home();
    let bare = home.join(format!("{name}.git"));
    init_bare(&fx.th, git, &bare);
    let work = format!("{name}-work");
    git_run(
        fx,
        git,
        &home,
        &["init", "--quiet", "--initial-branch=main", &work],
    );
    let work = home.join(work);
    git_run(
        fx,
        git,
        &work,
        &["remote", "add", "origin", &format!("../{name}.git")],
    );
    copy_dir(&fixture_dir(fixture), &work);
    git_run(fx, git, &work, &["add", "-A"]);
    git_run(fx, git, &work, &["commit", "--quiet", "-m", "initial"]);
    git_run(fx, git, &work, &["push", "--quiet", "origin", "HEAD:main"]);
    format!(
        "file:///{}",
        bare.to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches('/')
    )
}

fn fixture_dir(name: &str) -> PathBuf {
    fixture(name)
}

#[test]
fn launch_lines_cross_ipc_under_placeholders_and_save_the_real_paths() {
    let Some(git) =
        system_git_or_skip("launch_lines_cross_ipc_under_placeholders_and_save_the_real_paths")
    else {
        return;
    };
    let fx = Fx::with_git("test-desk", &git);
    let url = upstream(&fx, &git, "node", "clock");
    let (mut resolved, review) = mcp_source::review_git(&fx.core, &url, None, None).unwrap();
    let draft = &review.drafts[0];
    // `npx -y git+file:///<home>/clock.git#<sha>`: the home folder is `~`
    // in the launch line and in the Form (#18). The slash before the home
    // folder belongs to the home path on Unix and to the URL on Windows.
    assert!(draft.launch.contains("git+file://"), "{}", draft.launch);
    assert!(draft.launch.contains("~/clock.git#"), "{}", draft.launch);
    assert_eq!(draft.launch_lines.len(), 1);
    fx.assert_no_root(&serde_json::to_string(&review).unwrap());
    let picks = vec![McpDraftPick {
        key: draft.key.clone(),
        draft: draft.draft.clone(),
        trust: None,
    }];
    mcp_source::save(
        &fx.core,
        &mut resolved,
        &picks,
        Some(TrustGrantArg::Once),
        None,
        false,
    )
    .unwrap();
    // The store holds the real path, not the placeholder.
    let text = fx.store_file("servers/clock-mcp.toml");
    assert!(!text.contains("git+file:///~"), "{text}");
    assert!(text.contains("git+file:///"), "{text}");
}

#[test]
fn the_review_map_keeps_its_cap_and_never_brings_back_a_discarded_review() {
    use super::mcp_source::test_hooks;
    let fx = Fx::new();
    let folder = fx.th.home().join("code").join("notes");
    copy_dir(&fixture("configs"), &folder);
    let resolve = || mcp_source::review_local(&fx.core, &folder).unwrap().0;
    // A discard while a save holds the review: the failed save does not
    // put it back (#7, P3 #19).
    let token = test_hooks::remember_source(resolve());
    let held = test_hooks::take_source(&token).unwrap();
    test_hooks::discard_review(&token);
    test_hooks::keep_source(token.clone(), held);
    assert!(!test_hooks::known(&token));
    // Putting reviews back keeps the cap.
    let tokens: Vec<String> = (0..test_hooks::MAX)
        .map(|_| test_hooks::remember_source(resolve()))
        .collect();
    let held = test_hooks::take_source(&tokens[0]).unwrap();
    test_hooks::remember_source(resolve());
    test_hooks::keep_source(tokens[0].clone(), held);
    assert!(test_hooks::count() <= test_hooks::MAX);
}

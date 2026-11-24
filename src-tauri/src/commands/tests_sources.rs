//! M39: source trust commands against a temp home (#24, SPEC 17): the
//! dialog's check, the dialog's answer passed with the install and the
//! update, Settings > Sources (list, revoke, block). No path crosses IPC.

use std::path::{Path, PathBuf};

use agent_loadout_core::ErrorCode;
use agent_loadout_testkit::git_env::{git_vars, init_bare, system_git_or_skip};

use super::skills::{self, SkillUpdateStatus};
use super::sources::{
    self, SourceAskReason, SourceDecisionKind, SourceScope, SourceTrustState, TrustGrantArg,
};
use super::tests::Fx;

const NOTES: &str = "---\nname: alpha\ndescription: Alpha.\n---\n\n# Alpha\n";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
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

/// A bare repository in the home folder holding one skill, and its
/// `file://` URL.
fn upstream(fx: &Fx, git: &Path) -> (PathBuf, String) {
    let home = fx.th.home();
    let bare = home.join("up.git");
    init_bare(&fx.th, git, &bare);
    git_run(
        fx,
        git,
        &home,
        &["init", "--quiet", "--initial-branch=main", "work"],
    );
    let work = home.join("work");
    git_run(fx, git, &work, &["remote", "add", "origin", "../up.git"]);
    write(&work.join("skills").join("alpha").join("SKILL.md"), NOTES);
    git_run(fx, git, &work, &["add", "-A"]);
    git_run(fx, git, &work, &["commit", "--quiet", "-m", "initial"]);
    git_run(fx, git, &work, &["push", "--quiet", "origin", "HEAD:main"]);
    let url = format!(
        "file:///{}",
        bare.to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches('/')
    );
    (work, url)
}

#[test]
fn the_dialog_check_offers_narrow_scopes_first() {
    let fx = Fx::new();
    let info = sources::check(&fx.core, "https://github.com/Acme/Skills.git").unwrap();
    assert_eq!(info.display, "github.com/acme/skills");
    assert!(!info.official);
    assert_eq!(
        info.state,
        SourceTrustState::Ask {
            reason: SourceAskReason::FirstTime,
            lapse: None
        }
    );
    let scopes: Vec<(SourceScope, &str)> = info
        .scopes
        .iter()
        .map(|option| (option.scope, option.display.as_str()))
        .collect();
    assert_eq!(
        scopes,
        [
            (SourceScope::Repo, "github.com/acme/skills"),
            (SourceScope::Owner, "github.com/acme/*")
        ]
    );
    let json = serde_json::to_value(&info).unwrap();
    assert_eq!(json["state"]["kind"], "ask");
    assert_eq!(json["state"]["reason"], "first_time");
    assert_eq!(json["installedSkills"], serde_json::json!([]));
    assert!(
        sources::check(&fx.core, "https://github.com/anthropics/skills")
            .unwrap()
            .official
    );
    let err = sources::check(&fx.core, "not a url").unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidValue);
    // Checking writes nothing, not even a store.
    assert!(fx.core.open_store().unwrap().is_none());
}

#[test]
fn installs_and_updates_take_the_dialogs_answer() {
    let Some(git) = system_git_or_skip("installs_and_updates_take_the_dialogs_answer") else {
        return;
    };
    let fx = Fx::with_git("test-desk", &git);
    let (work, url) = upstream(&fx, &git);
    let install = |trust| {
        skills::install_git(
            &fx.core,
            &url,
            Some("skills/alpha".to_owned()),
            None,
            Some("alpha".to_owned()),
            false,
            trust,
        )
    };
    let err = install(None).unwrap_err();
    assert_eq!(err.code, ErrorCode::SourceTrustRequired);
    assert!(
        !err.message
            .contains(&fx.th.root().to_string_lossy().into_owned()),
        "{}",
        err.message
    );

    let info = sources::check(&fx.core, &url).unwrap();
    assert_eq!(info.display, "~/up.git");
    assert_eq!(info.scopes.len(), 1);
    assert_no_root(&fx, &serde_json::to_string(&info).unwrap());

    install(Some(TrustGrantArg::Always {
        scope: SourceScope::Repo,
    }))
    .unwrap();
    let listed = sources::list(&fx.core).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].decision, SourceDecisionKind::Trust);
    assert_eq!(listed[0].scope, SourceScope::Repo);
    assert_eq!(listed[0].display, "~/up.git");
    assert_eq!(listed[0].machine.as_deref(), Some("test-desk"));
    assert_no_root(&fx, &serde_json::to_string(&listed).unwrap());

    // A trusted source: the update check carries the trust and the diff.
    write(
        &work.join("skills").join("alpha").join("run.sh"),
        "#!/bin/sh\necho hi\n",
    );
    git_run(&fx, &git, &work, &["add", "-A"]);
    git_run(&fx, &git, &work, &["commit", "--quiet", "-m", "v2"]);
    git_run(
        &fx,
        &git,
        &work,
        &["push", "--quiet", "origin", "HEAD:main"],
    );
    let checks = skills::check_updates(&fx.core, &[]).unwrap();
    assert_eq!(checks[0].status, SkillUpdateStatus::Available);
    assert!(checks[0].changes[0].script);
    let trust = checks[0].trust.as_ref().unwrap();
    assert!(matches!(trust.state, SourceTrustState::Trusted { .. }));
    assert_no_root(&fx, &serde_json::to_string(&checks).unwrap());

    // Revoked: the update asks again; once is enough for this one.
    sources::revoke(&fx.core, &listed[0].id).unwrap();
    assert!(sources::list(&fx.core).unwrap().is_empty());
    let latest = checks[0].latest.clone().unwrap();
    let err = skills::update(&fx.core, "alpha", &latest, false, None, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::SourceTrustRequired);
    skills::update(
        &fx.core,
        "alpha",
        &latest,
        false,
        Some(TrustGrantArg::Once),
        None,
    )
    .unwrap();
    assert!(sources::list(&fx.core).unwrap().is_empty());

    // Blocked from Settings: no answer gets through.
    sources::block_url(&fx.core, &url, SourceScope::Repo).unwrap();
    let listed = sources::list(&fx.core).unwrap();
    assert_eq!(listed[0].decision, SourceDecisionKind::Block);
    let err = install(Some(TrustGrantArg::Once)).unwrap_err();
    assert_eq!(err.code, ErrorCode::SourceBlocked);
    sources::revoke(&fx.core, &listed[0].id).unwrap();
    assert_eq!(
        sources::revoke(&fx.core, &listed[0].id).unwrap_err().code,
        ErrorCode::NotFound
    );
}

#[test]
fn a_listed_trust_can_be_turned_into_a_block() {
    let fx = Fx::new();
    let url = "https://github.com/acme/skills";
    sources::block_url(&fx.core, url, SourceScope::Owner).unwrap();
    let listed = sources::list(&fx.core).unwrap();
    assert_eq!(listed[0].display, "github.com/acme/*");
    assert_eq!(listed[0].scope, SourceScope::Owner);
    let info = sources::check(&fx.core, url).unwrap();
    assert!(matches!(info.state, SourceTrustState::Blocked { .. }));
    // Blocking a block changes nothing.
    assert_eq!(sources::block(&fx.core, &listed[0].id).unwrap(), None);
    let err = sources::block_url(&fx.core, url, SourceScope::Host);
    assert!(err.is_ok(), "a host can be blocked: {err:?}");
}

#[test]
fn a_blocked_source_is_refused_before_the_review_fetches_it() {
    let Some(git) = system_git_or_skip("a_blocked_source_is_refused_before_the_review") else {
        return;
    };
    let fx = Fx::with_git("test-desk", &git);
    // No repository there: any fetch would fail with source_unreachable.
    let missing = fx.th.home().join("gone.git");
    let url = format!(
        "file:///{}",
        missing
            .to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches('/')
    );
    sources::block_url(&fx.core, &url, SourceScope::Repo).unwrap();
    let err = skills::review_git(&fx.core, &url, None, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::SourceBlocked, "{}", err.message);
    assert_no_root(&fx, &err.message);
}

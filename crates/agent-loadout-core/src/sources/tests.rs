use std::path::Path;

use super::detect::{SourceChangeKind, moved, redirect_target};
use super::*;
use crate::remote::git::RunOutput;

fn key(url: &str) -> String {
    SourceRef::from_git_url(url)
        .unwrap_or_else(|err| panic!("{url}: {err}"))
        .key()
        .to_owned()
}

#[test]
fn git_urls_of_one_repository_give_one_key() {
    for url in [
        "https://github.com/Acme/Skills.git",
        "https://github.com/acme/skills/",
        "git@github.com:acme/skills.git",
        "ssh://git@github.com/acme/skills",
        "ssh://git@github.com:22/acme/skills.git",
        "https://user:token@github.com/acme/skills",
        "https://github.com/acme/skills/tree/main/skills/x",
        "https://github.com/%61cme/skills",
    ] {
        assert_eq!(key(url), "github.com/acme/skills", "{url}");
    }
}

#[test]
fn hosts_keep_ports_and_case_sensitive_paths() {
    assert_eq!(
        key("https://Git.Acme.Corp:8443/Team/Sub/Tools.git"),
        "git.acme.corp:8443/Team/Sub/Tools"
    );
    assert_eq!(
        key("https://git.acme.corp:443/team/tools"),
        "git.acme.corp/team/tools"
    );
    assert_eq!(
        key("https://gitlab.com/Group/Sub/Repo/-/tree/main/x"),
        "gitlab.com/group/sub/repo"
    );
    assert_eq!(
        key("https://git.acme.corp/tools.git"),
        "git.acme.corp/tools"
    );
}

#[test]
fn userinfo_never_reaches_the_shown_url() {
    let source = SourceRef::from_git_url("https://user:s3cret@github.com/acme/skills").unwrap();
    assert!(!source.url().contains("s3cret"), "{}", source.url());
    assert!(!source.url().contains("user"), "{}", source.url());
}

/// Git contacts the URL rebuilt from the parsed parts: the key's host and
/// path, with the credentials given (P0-1).
#[test]
fn the_fetch_url_is_the_canonical_url() {
    let fetch = |url: &str| SourceRef::from_git_url(url).unwrap().fetch_url(url);
    let given = "https://user:s3cret@GitHub.com.:443/Acme/Skills.git/";
    let source = SourceRef::from_git_url(given).unwrap();
    assert_eq!(source.url(), "https://github.com/acme/skills.git");
    assert_eq!(
        source.fetch_url(given),
        "https://user:s3cret@github.com/acme/skills.git"
    );
    // Credentials only come from a URL of this same source.
    assert_eq!(
        source.fetch_url("https://user:s3cret@github.com/other/skills"),
        "https://github.com/acme/skills.git"
    );
    assert_eq!(
        fetch("https://github.com/acme/skills/tree/main/x"),
        "https://github.com/acme/skills"
    );
    assert_eq!(
        fetch("https://gitlab.com/Group/Sub/Repo/-/tree/main"),
        "https://gitlab.com/group/sub/repo"
    );
    assert_eq!(
        fetch("https://git.acme.corp:8443/My%20Team/%74ools"),
        "https://git.acme.corp:8443/My%20Team/tools"
    );
    assert_eq!(
        fetch("git@GitHub.com:acme/skills.git"),
        "git@github.com:acme/skills.git"
    );
    assert_eq!(
        fetch("me@git.acme.corp:/srv/git/tools.git"),
        "me@git.acme.corp:/srv/git/tools.git"
    );
    assert_eq!(
        fetch("ssh://git:pw@git.acme.corp:2222/team/tools"),
        "ssh://git:pw@git.acme.corp:2222/team/tools"
    );
    assert_eq!(
        SourceRef::from_git_url("ssh://git:pw@git.acme.corp:2222/team/tools")
            .unwrap()
            .url(),
        "ssh://git@git.acme.corp:2222/team/tools"
    );
    assert_eq!(
        fetch("http://git.acme.corp:80/team/tools"),
        "http://git.acme.corp/team/tools"
    );
    let path = if cfg!(windows) {
        r"C:\repos\x.git"
    } else {
        "/repos/x.git"
    };
    assert_eq!(fetch(path), path);
    for url in [
        "https://github.com/acme/skills",
        "git@github.com:acme/skills.git",
        "http://git.acme.corp/team/tools",
    ] {
        let source = SourceRef::from_git_url(url).unwrap();
        assert_eq!(
            SourceRef::from_git_url(source.url()).unwrap(),
            source,
            "{url}"
        );
    }
}

#[test]
fn userinfo_is_redacted_from_text() {
    assert_eq!(
        SourceRef::redact_userinfo(
            "fatal: unable to access 'https://user:s3cret@host/x/': 403; see ssh://git@h/y"
        ),
        "fatal: unable to access 'https://***@host/x/': 403; see ssh://***@h/y"
    );
    assert_eq!(
        SourceRef::redact_userinfo("https://tok@evil?@github.com/x"),
        "https://***@github.com/x"
    );
    assert_eq!(SourceRef::redact_userinfo("no url here"), "no url here");
}

#[test]
fn local_sources_are_file_keys() {
    let path = if cfg!(windows) {
        r"c:\Repos\x.git\"
    } else {
        "/home/me/x.git/"
    };
    let source = SourceRef::from_git_url(path).unwrap();
    let expected = if cfg!(windows) {
        "file:///C:/Repos/x.git"
    } else {
        "file:///home/me/x.git"
    };
    assert_eq!(source.key(), expected);
    assert!(source.is_local());
    assert_eq!(source.scopes(), vec![SourceScope::Repo]);
    assert_eq!(source.scope_key(SourceScope::Owner), None);
    assert_eq!(key(expected), expected);
    let back = source.local_path().unwrap();
    assert_eq!(SourceRef::from_local_path(&back).unwrap().key(), expected);
    assert!(SourceRef::from_local_path(Path::new("relative/x")).is_err());
}

#[test]
fn not_sources_are_refused() {
    for text in [
        "",
        "relative/path",
        "ftp://host/x",
        "https://github.com/",
        "https:///x",
    ] {
        let err = SourceRef::from_git_url(text).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidValue, "{text}");
    }
}

#[test]
fn scopes_skip_aggregator_hosts_and_missing_owners() {
    let github = SourceRef::from_git_url("https://github.com/acme/skills").unwrap();
    assert_eq!(github.scopes(), vec![SourceScope::Repo, SourceScope::Owner]);
    assert_eq!(
        github.matching_keys(),
        vec![
            "github.com/acme/skills",
            "github.com/acme/*",
            "github.com/*"
        ]
    );
    let corp = SourceRef::from_git_url("https://git.acme.corp/team/tools").unwrap();
    assert_eq!(
        corp.scopes(),
        vec![SourceScope::Repo, SourceScope::Owner, SourceScope::Host]
    );
    let root = SourceRef::from_git_url("https://git.acme.corp/tools").unwrap();
    assert_eq!(root.scopes(), vec![SourceScope::Repo, SourceScope::Host]);
    for scope in [SourceScope::Repo, SourceScope::Owner, SourceScope::Host] {
        if let Some(key) = corp.scope_key(scope) {
            assert_eq!(key_shape(&key), Some(scope), "{key}");
        }
    }
}

#[test]
fn official_owners_are_marked() {
    assert!(
        SourceRef::from_git_url("https://github.com/anthropics/skills")
            .unwrap()
            .is_official()
    );
    assert!(
        !SourceRef::from_git_url("https://github.com/anthropic-s/skills")
            .unwrap()
            .is_official()
    );
}

fn record(decision: SourceDecision, scope: SourceScope) -> SourceRecord {
    SourceRecord {
        decision,
        scope,
        decided_at: "2026-11-23T10:00:00Z".parse().unwrap(),
        via: Some(TrustVia::CliCommand),
        machine: None,
        url: None,
        lapsed: None,
        extra: toml::Table::new(),
    }
}

#[test]
fn block_wins_and_the_most_specific_trust_decides() {
    let source = SourceRef::from_git_url("https://git.acme.corp/team/tools").unwrap();
    let mut file = SourcesFile::new();
    assert!(matches!(
        state_of(&file, &source),
        TrustState::Ask {
            reason: AskReason::FirstTime,
            lapse: None
        }
    ));
    file.sources.insert(
        "git.acme.corp/team/*".into(),
        record(SourceDecision::Trust, SourceScope::Owner),
    );
    assert!(
        matches!(state_of(&file, &source), TrustState::Trusted { ref key, .. } if key == "git.acme.corp/team/*")
    );
    // A lapsed repository record is more specific than the owner's trust.
    let mut lapsed = record(SourceDecision::Trust, SourceScope::Repo);
    lapsed.lapsed = Some(Lapse {
        reason: LapseReason::HistoryRewritten,
        at: "2026-11-24T10:00:00Z".parse().unwrap(),
        detail: None,
        extra: toml::Table::new(),
    });
    file.sources
        .insert("git.acme.corp/team/tools".into(), lapsed);
    assert!(matches!(
        state_of(&file, &source),
        TrustState::Ask {
            reason: AskReason::Lapsed,
            lapse: Some(_)
        }
    ));
    // A host block beats every trust below it.
    file.sources.insert(
        "git.acme.corp/*".into(),
        record(SourceDecision::Block, SourceScope::Host),
    );
    assert!(
        matches!(state_of(&file, &source), TrustState::Blocked { ref key, .. } if key == "git.acme.corp/*")
    );
    assert!(file.validate().is_valid());
}

#[test]
fn key_shapes() {
    assert_eq!(key_shape("github.com/acme/skills"), Some(SourceScope::Repo));
    assert_eq!(key_shape("git.acme.corp/tools"), Some(SourceScope::Repo));
    assert_eq!(key_shape("github.com/acme/*"), Some(SourceScope::Owner));
    assert_eq!(key_shape("github.com/*"), Some(SourceScope::Host));
    assert_eq!(
        key_shape("file:///C:/repos/my skills"),
        Some(SourceScope::Repo)
    );
    for bad in [
        "",
        "github.com",
        "GitHub.com/acme/x",
        "github.com/a/*/b",
        "github.com//x",
        "github.com/../x",
        "https://github.com/acme/x",
        "user@github.com/acme/x",
        "github.com/acme/x y",
        "file:///",
        "file:///C:/x/*",
    ] {
        assert_eq!(key_shape(bad), None, "{bad}");
    }
}

#[test]
fn redirects_of_another_repository_are_moves() {
    let output = |stderr: &str| RunOutput {
        success: true,
        stdout: String::new(),
        stderr: stderr.to_owned(),
        timed_out: false,
    };
    assert_eq!(
        redirect_target(
            "https://github.com/acme/skills",
            "warning: redirecting to https://github.com/new/skills.git/\n"
        ),
        Some("https://github.com/new/skills.git".to_owned())
    );
    // The remote's own messages never count, and only HTTP redirects.
    for (url, stderr) in [
        (
            "https://evil.example/x/skills",
            "remote: warning: redirecting to https://github.com/anthropics/skills/\n",
        ),
        (
            "https://evil.example/x/skills",
            "hint: warning: redirecting to https://github.com/anthropics/skills/\n",
        ),
        (
            "ssh://git@evil.example/x/skills",
            "warning: redirecting to https://github.com/anthropics/skills/\n",
        ),
        (
            "git@evil.example:x/skills",
            "warning: redirecting to https://github.com/anthropics/skills/\n",
        ),
    ] {
        assert_eq!(redirect_target(url, stderr), None, "{url} {stderr}");
    }
    let ssh = SourceRef::from_git_url("ssh://git@evil.example/x/skills").unwrap();
    assert_eq!(
        moved(
            &ssh,
            &output("warning: redirecting to https://github.com/anthropics/skills/\n")
        ),
        None
    );
    let source = SourceRef::from_git_url("https://github.com/acme/skills").unwrap();
    let change = moved(
        &source,
        &output("warning: redirecting to https://github.com/new-owner/skills.git/\n"),
    )
    .unwrap();
    assert_eq!(change.kind, SourceChangeKind::Moved);
    assert_eq!(
        change.detail.as_deref(),
        Some("github.com/new-owner/skills")
    );
    // Case-only or `.git` redirects are the same repository.
    assert_eq!(
        moved(
            &source,
            &output("warning: redirecting to https://github.com/Acme/Skills.git/\n")
        ),
        None
    );
    assert_eq!(moved(&source, &output("")), None);
}

/// Text git would contact elsewhere than the key names (#24 review P0-1,
/// P1-1): refused, never cut down to a trusted key.
#[test]
fn urls_whose_key_and_fetched_location_could_differ_are_refused() {
    for text in [
        "https://evil.example?@github.com/acme/skills",
        "https://evil.example#@github.com/anthropics/skills",
        "https://evil.example\\@github.com/acme/skills",
        "https://github.com/acme/skills/../../evil/repo",
        "https://gitlab.com/acme/skills/-/../../../evil/repo",
        "https://git.acme.corp/team/%2e%2e/evil/repo",
        "https://git.acme.corp/team/%2E/repo",
        "https://git.acme.corp/team%2fevil/repo",
        "https://git.acme.corp/team%5Cevil/repo",
        "https://github.com/acme/./skills",
        "https://github.com/acme//skills",
        "https://github.com/acme/sk\tills",
        "https://a@b@github.com/acme/skills",
        "https://github.com/acme/skills?tab=readme#top",
        "ssh://git@github.com/acme/skills/../../evil/repo",
        "git@github.com:acme/../evil/repo",
        "git@github.com:acme/skills?x",
    ] {
        let err = SourceRef::from_git_url(text)
            .map(|source| source.key().to_owned())
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidValue, "{text}");
    }
}

#[test]
fn hosts_are_normalized_before_keying() {
    for url in [
        "https://github.com./acme/skills",
        "https://GitHub.COM:443/Acme/Skills",
        "ssh://git@GITHUB.com.:22/acme/skills.git",
        "git@GitHub.com.:Acme/Skills.git",
    ] {
        assert_eq!(key(url), "github.com/acme/skills", "{url}");
    }
    // A trailing-dot host is still an aggregator: no host scope offered.
    let dotted = SourceRef::from_git_url("https://github.com./evil/repo").unwrap();
    assert_eq!(dotted.scopes(), vec![SourceScope::Repo, SourceScope::Owner]);
    assert_eq!(
        dotted.scope_key(SourceScope::Owner).as_deref(),
        Some("github.com/evil/*")
    );
    assert!(SourceRef::from_git_url("https://github.com../acme/skills").is_err());
}

/// Plain `http://` and `git://` can be tampered with on the way: trust
/// given over HTTPS or SSH does not carry over (P3-2).
#[test]
fn insecure_transports_get_their_own_keys() {
    assert_eq!(
        key("http://github.com/Acme/Skills.git"),
        "http://github.com/acme/skills"
    );
    assert_eq!(
        key("git://github.com/acme/skills"),
        "git://github.com/acme/skills"
    );
    assert_eq!(
        key("http://git.acme.corp:80/team/tools"),
        "http://git.acme.corp/team/tools"
    );
    let http = SourceRef::from_git_url("http://git.acme.corp/team/tools").unwrap();
    assert_eq!(
        http.matching_keys(),
        vec![
            "http://git.acme.corp/team/tools",
            "http://git.acme.corp/team/*",
            "http://git.acme.corp/*"
        ]
    );
    for scope in [SourceScope::Repo, SourceScope::Owner, SourceScope::Host] {
        let key = http.scope_key(scope).unwrap();
        assert_eq!(key_shape(&key), Some(scope), "{key}");
    }
    let mut file = SourcesFile::new();
    file.sources.insert(
        "git.acme.corp/team/tools".into(),
        record(SourceDecision::Trust, SourceScope::Repo),
    );
    assert!(matches!(
        state_of(&file, &http),
        TrustState::Ask {
            reason: AskReason::FirstTime,
            ..
        }
    ));
    // A block covers every transport.
    file.sources.insert(
        "git.acme.corp/team/*".into(),
        record(SourceDecision::Block, SourceScope::Owner),
    );
    assert!(
        matches!(state_of(&file, &http), TrustState::Blocked { ref key, .. } if key == "git.acme.corp/team/*")
    );
    assert!(file.validate().is_valid());
}

/// Userinfo (a token) never reaches an error message (P3-1).
#[test]
fn credentials_never_reach_messages() {
    for text in [
        "https://user:s3cret@github.com/",
        "ftp://user:s3cret@host/x",
        "https://user:s3cret@evil.example?@github.com/acme/skills",
        "https://s3cret@github.com/acme/../x",
    ] {
        let err = SourceRef::from_git_url(text).unwrap_err();
        assert!(!err.to_string().contains("s3cret"), "{err}");
    }
}

/// `sources.toml` keys must be canonical: a non-canonical key would never
/// match the key of the URL it means (P2-4).
#[test]
fn non_canonical_keys_are_refused() {
    for (bad, scope) in [
        ("github.com/Acme/*", SourceScope::Owner),
        ("github.com/acme/Skills", SourceScope::Repo),
        ("github.com/acme/skills.git", SourceScope::Repo),
        ("github.com./acme/*", SourceScope::Owner),
        ("github.com:443/acme/*", SourceScope::Owner),
        ("git.acme.corp:443/*", SourceScope::Host),
        ("git.acme.corp/team/%2e%2e/x", SourceScope::Repo),
    ] {
        let mut file = SourcesFile::new();
        file.sources
            .insert(bad.into(), record(SourceDecision::Block, scope));
        assert!(!file.validate().is_valid(), "{bad}");
    }
    let mut file = SourcesFile::new();
    for (good, scope) in [
        ("github.com/acme/*", SourceScope::Owner),
        ("git.acme.corp:8443/Team/Tools", SourceScope::Repo),
        ("http://git.acme.corp/*", SourceScope::Host),
        ("git://github.com/acme/*", SourceScope::Owner),
    ] {
        file.sources
            .insert(good.into(), record(SourceDecision::Block, scope));
    }
    assert!(file.validate().is_valid(), "{:?}", file.validate());
}

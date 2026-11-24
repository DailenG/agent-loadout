//! Source trust (#24, SPEC 17): the first install from a source asks,
//! "always" persists and skips the prompt, a block wins over any trust and
//! is never contacted, a force-pushed (amended or rolled back) or moved
//! repository asks again, updates from a trusted source still show their
//! diff, and a fast-forward never asks again.
//!
//! Hermetic: git sources are local bare repositories in a `TempHome`;
//! `https://github.com/acme/...` URLs reach them through an `insteadOf`
//! rule in the isolated git config, so they are fetched shallow like a
//! real remote. A moved repository is a real HTTP 301 from a local server
//! thread in front of `git http-backend`; a runner that adds a `remote:`
//! line plays a server that only claims a redirect.

mod support;

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use agent_loadout_core::adapters::detect::NoRegistry;
use agent_loadout_core::remote::git::{Invocation, RunOutput, Runner, SystemRunner};
use agent_loadout_core::skills::install::{self, Decisions, InstallRequest, Source, UpdateStatus};
use agent_loadout_core::skills::{LinkKind, SkillsEnv};
use agent_loadout_core::sources::{
    self, AskReason, LapseReason, SourceChangeKind, SourceDecision, SourceRef, SourceScope,
    TrustGrant, TrustState, TrustVia,
};
use agent_loadout_core::{Env, ErrorCode};
use agent_loadout_testkit::git_env::{git_vars, init_bare, system_git_or_skip, with_git_vars};
use support::{Fx, copy_dir, fixtures, host_os};

static RUNNER: SystemRunner = SystemRunner {
    idle_timeout: Duration::from_secs(60),
};
static NO_REGISTRY: NoRegistry = NoRegistry;

fn sx<'a>(fx: &'a Fx, env: &'a Env, runner: &'a dyn Runner) -> SkillsEnv<'a> {
    SkillsEnv {
        env,
        fs: &fx.fs,
        home: &fx.home,
        registry: &NO_REGISTRY,
        runner,
        link_kind: LinkKind::Junction,
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

/// `<root>/repos/acme/skills.git` holding `fixtures/skills/monorepo`, the
/// work repository that pushes to it, and `https://github.com/acme/` mapped
/// to `<root>/repos/acme/` in the isolated git config.
struct Upstream {
    bare: PathBuf,
    work: PathBuf,
}

fn upstream(fx: &Fx, git: &Path) -> Upstream {
    let repos = fx.th.root().join("repos");
    let bare = repos.join("acme").join("skills.git");
    std::fs::create_dir_all(bare.parent().unwrap()).unwrap();
    init_bare(&fx.th, git, &bare);
    // git_vars wrote the empty global config; map the fake host onto the
    // folder of bare repositories (file:// so clones are shallow).
    let base = format!(
        "file:///{}/",
        repos
            .to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches('/')
    );
    std::fs::write(
        fx.th.root().join("gitconfig"),
        format!("[url \"{base}\"]\n\tinsteadOf = https://github.com/\n[protocol \"file\"]\n\tallow = always\n"),
    )
    .unwrap();
    let work = fx.th.root().join("work");
    git_run(
        fx,
        git,
        fx.th.root(),
        &["init", "--quiet", "--initial-branch=main", "work"],
    );
    git_run(
        fx,
        git,
        &work,
        &["remote", "add", "origin", &bare.to_string_lossy()],
    );
    copy_dir(&fixtures().join("skills").join("monorepo"), &work);
    push(fx, git, &work, "initial");
    Upstream { bare, work }
}

fn push(fx: &Fx, git: &Path, work: &Path, message: &str) {
    git_run(fx, git, work, &["add", "-A"]);
    git_run(fx, git, work, &["commit", "--quiet", "-m", message]);
    git_run(fx, git, work, &["push", "--quiet", "origin", "HEAD:main"]);
}

const URL: &str = "https://github.com/acme/skills";

fn request(path: &str, trust: Option<TrustGrant>) -> InstallRequest {
    let mut request = InstallRequest::new(Source::Git {
        url: URL.to_owned(),
        git_ref: None,
        path: None,
    });
    request.path = Some(path.to_owned());
    request.decisions = Decisions {
        trust,
        via: TrustVia::CliPrompt,
        ..Decisions::default()
    };
    request
}

fn source() -> SourceRef {
    SourceRef::from_git_url(URL).unwrap()
}

fn trust_with(grant: TrustGrant) -> Decisions {
    Decisions {
        trust: Some(grant),
        via: TrustVia::App,
        ..Decisions::default()
    }
}

#[test]
fn first_install_asks_once_and_always_persists() {
    let Some(git) = system_git_or_skip("first_install_asks_once_and_always_persists") else {
        return;
    };
    let fx = Fx::new();
    let env = with_git_vars(&fx.th, &git, fx.th.env_builder(host_os())).build();
    let sx = sx(&fx, &env, &RUNNER);
    let store = fx.store();
    upstream(&fx, &git);

    // First time: the gate refuses before anything is fetched.
    let check = sources::check(&store, &source()).unwrap();
    assert_eq!(
        check.state,
        TrustState::Ask {
            reason: AskReason::FirstTime,
            lapse: None
        }
    );
    assert_eq!(check.scopes, vec![SourceScope::Repo, SourceScope::Owner]);
    let commits = fx.commit_count();
    let err = install::install(&store, &sx, &request("skills/alpha", None)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::SourceTrustRequired, "{err}");
    assert_eq!(fx.commit_count(), commits);
    let cache = fx.home.cache_dir().join("skill-sources");
    assert!(
        !cache.exists() || std::fs::read_dir(&cache).unwrap().next().is_none(),
        "nothing is cloned before the user decides"
    );

    // Trust this time only: installs, records nothing.
    let installed = install::install(
        &store,
        &sx,
        &request("skills/alpha", Some(TrustGrant::Once)),
    )
    .unwrap();
    assert_eq!(installed.id, "alpha");
    assert!(sources::list(&store).unwrap().is_empty());

    // The next install from the same source asks again, and says what is
    // already installed from it.
    let check = sources::check(&store, &source()).unwrap();
    assert_eq!(
        check.state,
        TrustState::Ask {
            reason: AskReason::NotTrusted,
            lapse: None
        }
    );
    assert_eq!(check.installed_skills, vec!["alpha".to_owned()]);
    let err = install::install(&store, &sx, &request("skills/beta", None)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::SourceTrustRequired);

    // Always (repository): one commit records it, then the install.
    let installed = install::install(
        &store,
        &sx,
        &request("skills/beta", Some(TrustGrant::Always(SourceScope::Repo))),
    )
    .unwrap();
    assert_eq!(installed.id, "beta");
    let log = fx.log();
    assert_eq!(
        log[1].summary,
        "source: trust github.com/acme/skills (repo)"
    );
    let records = sources::list(&store).unwrap();
    assert_eq!(records.len(), 1);
    let (key, record) = &records[0];
    assert_eq!(key, "github.com/acme/skills");
    assert_eq!(record.decision, SourceDecision::Trust);
    assert_eq!(record.via, Some(TrustVia::CliPrompt));
    assert_eq!(record.url.as_deref(), Some(URL));
    assert_eq!(record.machine.as_deref(), Some("test-desk"));

    // No prompt any more: an update check and an update need no answer.
    assert!(matches!(
        sources::check(&store, &source()).unwrap().state,
        TrustState::Trusted { .. }
    ));
    let checked = install::check_update(&store, &sx, "alpha").unwrap();
    assert_eq!(checked.status, UpdateStatus::UpToDate);
    assert!(matches!(
        checked.trust.unwrap().state,
        TrustState::Trusted { .. }
    ));

    // Revoking asks again; nothing is uninstalled.
    sources::untrust(&store, "github.com/acme/skills").unwrap();
    assert_eq!(
        fx.log()[0].summary,
        "source: untrust github.com/acme/skills"
    );
    assert!(store.root().join("skills").join("alpha").is_dir());
    let err = install::update(&store, &sx, "alpha", &Decisions::default()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::SourceTrustRequired);
}

#[test]
fn block_overrides_trust() {
    let Some(git) = system_git_or_skip("block_overrides_trust") else {
        return;
    };
    let fx = Fx::new();
    let env = with_git_vars(&fx.th, &git, fx.th.env_builder(host_os())).build();
    let sx = sx(&fx, &env, &RUNNER);
    let store = fx.store();
    upstream(&fx, &git);

    sources::trust(&store, &source(), SourceScope::Owner, TrustVia::CliCommand).unwrap();
    assert_eq!(
        fx.log()[0].summary,
        "source: trust github.com/acme/* (owner)"
    );
    install::install(&store, &sx, &request("skills/alpha", None)).unwrap();

    sources::block(&store, &source(), SourceScope::Repo, TrustVia::CliCommand).unwrap();
    let check = sources::check(&store, &source()).unwrap();
    assert!(
        matches!(check.state, TrustState::Blocked { ref key, .. } if key == "github.com/acme/skills")
    );
    for grant in [
        None,
        Some(TrustGrant::Once),
        Some(TrustGrant::Always(SourceScope::Repo)),
    ] {
        let err = install::install(&store, &sx, &request("skills/beta", grant)).unwrap_err();
        assert_eq!(err.code(), ErrorCode::SourceBlocked, "{grant:?}");
        let err = install::update(
            &store,
            &sx,
            "alpha",
            &Decisions {
                trust: grant,
                ..Decisions::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::SourceBlocked, "{grant:?}");
    }
    // The whole host can be blocked, never trusted.
    let err =
        sources::trust(&store, &source(), SourceScope::Host, TrustVia::CliCommand).unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);
    let err = sources::record_key(
        &store,
        "github.com/*",
        SourceDecision::Trust,
        TrustVia::CliCommand,
        None,
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidValue);
    sources::record_key(
        &store,
        "github.com/*",
        SourceDecision::Block,
        TrustVia::CliCommand,
        None,
    )
    .unwrap();
    let other = SourceRef::from_git_url("https://github.com/other/repo").unwrap();
    assert!(matches!(
        sources::check(&store, &other).unwrap().state,
        TrustState::Blocked { .. }
    ));
    // Removing the blocks lets the owner's trust work again.
    sources::untrust(&store, "github.com/*").unwrap();
    sources::untrust(&store, "github.com/acme/skills").unwrap();
    install::install(&store, &sx, &request("skills/beta", None)).unwrap();
}

#[test]
fn a_trusted_source_still_shows_its_diff_and_a_fast_forward_keeps_trust() {
    let Some(git) = system_git_or_skip("a_trusted_source_still_shows_its_diff") else {
        return;
    };
    let fx = Fx::new();
    let env = with_git_vars(&fx.th, &git, fx.th.env_builder(host_os())).build();
    let sx = sx(&fx, &env, &RUNNER);
    let store = fx.store();
    let up = upstream(&fx, &git);
    install::install(
        &store,
        &sx,
        &request("skills/alpha", Some(TrustGrant::Always(SourceScope::Repo))),
    )
    .unwrap();
    let clone = fx.home.cache_dir().join("skill-sources").join("alpha");
    assert!(
        clone.join(".git").join("shallow").is_file(),
        "the cache clone is shallow"
    );

    // Two pushes on top: a fast-forward of a shallow cache clone.
    let alpha = up.work.join("skills").join("alpha");
    fx.write(&alpha.join("notes.md"), "one\n");
    push(&fx, &git, &up.work, "one");
    fx.write(&alpha.join("run.sh"), "#!/bin/sh\necho hi\n");
    push(&fx, &git, &up.work, "two");
    let commits = fx.commit_count();
    let checked = install::check_update(&store, &sx, "alpha").unwrap();
    assert_eq!(checked.status, UpdateStatus::Available);
    assert_eq!(checked.source_change, None);
    assert!(matches!(
        checked.trust.as_ref().unwrap().state,
        TrustState::Trusted { .. }
    ));
    let files: Vec<(&str, &str, bool)> = checked
        .changes
        .iter()
        .map(|change| (change.path.as_str(), change.kind.as_str(), change.script))
        .collect();
    assert_eq!(
        files,
        [("notes.md", "added", false), ("run.sh", "added", true)]
    );
    assert_eq!(fx.commit_count(), commits, "a check writes nothing");
    let updated = install::update_reviewed(
        &store,
        &sx,
        "alpha",
        checked.latest.as_deref().unwrap(),
        &Decisions::default(),
    )
    .unwrap();
    assert!(updated.commit.is_some());
    let _ = up.bare;
}

#[test]
fn a_force_push_makes_trust_lapse_until_the_user_decides_again() {
    let Some(git) = system_git_or_skip("a_force_push_makes_trust_lapse") else {
        return;
    };
    let fx = Fx::new();
    let env = with_git_vars(&fx.th, &git, fx.th.env_builder(host_os())).build();
    let sx = sx(&fx, &env, &RUNNER);
    let store = fx.store();
    let up = upstream(&fx, &git);
    sources::trust(&store, &source(), SourceScope::Owner, TrustVia::App).unwrap();
    install::install(&store, &sx, &request("skills/alpha", None)).unwrap();

    // Rewrite the branch: the installed commit is gone from it.
    fx.write(
        &up.work.join("skills").join("alpha").join("notes.md"),
        "rewritten\n",
    );
    git_run(&fx, &git, &up.work, &["add", "-A"]);
    git_run(
        &fx,
        &git,
        &up.work,
        &["commit", "--quiet", "--amend", "-m", "rewritten"],
    );
    git_run(
        &fx,
        &git,
        &up.work,
        &["push", "--quiet", "--force", "origin", "HEAD:main"],
    );

    let checked = install::check_update(&store, &sx, "alpha").unwrap();
    assert_eq!(checked.status, UpdateStatus::Available);
    let change = checked.source_change.clone().unwrap();
    assert_eq!(change.kind, SourceChangeKind::HistoryRewritten);
    assert_eq!(
        fx.log()[0].summary,
        "source: trust lapsed for github.com/acme/skills (history_rewritten)"
    );
    let TrustState::Ask {
        reason: AskReason::Lapsed,
        lapse: Some(lapse),
    } = &checked.trust.as_ref().unwrap().state
    else {
        panic!("{:?}", checked.trust);
    };
    assert_eq!(lapse.reason, LapseReason::HistoryRewritten);
    // The owner's trust stays; the repository asks again.
    let latest = checked.latest.clone().unwrap();
    let err =
        install::update_reviewed(&store, &sx, "alpha", &latest, &Decisions::default()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::SourceTrustRequired);
    assert!(err.message().contains("force-push"), "{err}");
    // Once: updates, the lapse stays.
    install::update_reviewed(&store, &sx, "alpha", &latest, &trust_with(TrustGrant::Once)).unwrap();
    assert!(matches!(
        sources::check(&store, &source()).unwrap().state,
        TrustState::Ask {
            reason: AskReason::Lapsed,
            ..
        }
    ));
    // Always: the lapse is cleared; the next check asks nothing.
    sources::admit(
        &store,
        &source(),
        Some(TrustGrant::Always(SourceScope::Repo)),
        TrustVia::App,
    )
    .unwrap();
    let checked = install::check_update(&store, &sx, "alpha").unwrap();
    assert_eq!(checked.status, UpdateStatus::UpToDate);
    assert_eq!(checked.source_change, None);
    assert!(matches!(
        checked.trust.unwrap().state,
        TrustState::Trusted { .. }
    ));
}

/// The system git, plus a line that only claims a redirect: what a
/// hostile server (or an SSH server's stderr) can print.
struct Spoofing;

impl Runner for Spoofing {
    fn run(
        &self,
        invocation: &Invocation,
        progress: &mut dyn FnMut(&str),
    ) -> io::Result<RunOutput> {
        let mut output = RUNNER.run(invocation, progress)?;
        let first = invocation.args.first().and_then(|arg| arg.to_str());
        if matches!(first, Some("fetch" | "clone")) && output.success {
            output.stderr.push_str(
                "remote: warning: redirecting to https://github.com/new-owner/skills.git/\n",
            );
        }
        Ok(output)
    }
}

#[test]
fn a_remote_message_is_never_a_move() {
    let Some(git) = system_git_or_skip("a_remote_message_is_never_a_move") else {
        return;
    };
    let fx = Fx::new();
    let env = with_git_vars(&fx.th, &git, fx.th.env_builder(host_os())).build();
    let store = fx.store();
    upstream(&fx, &git);
    install::install(
        &store,
        &sx(&fx, &env, &RUNNER),
        &request("skills/alpha", Some(TrustGrant::Always(SourceScope::Repo))),
    )
    .unwrap();

    let spoofed = sx(&fx, &env, &Spoofing);
    let commits = fx.commit_count();
    let checked = install::check_update(&store, &spoofed, "alpha").unwrap();
    assert_eq!(checked.source_change, None);
    assert!(checked.moved_trust.is_none());
    assert_eq!(fx.commit_count(), commits, "no lapse");
    assert!(matches!(
        checked.trust.unwrap().state,
        TrustState::Trusted { .. }
    ));
    let prepared = install::prepare(&spoofed, &request("skills/beta", None)).unwrap();
    assert_eq!(
        prepared.trust_source().unwrap().key(),
        "github.com/acme/skills"
    );
    assert!(prepared.moved_to().is_none());
    assert!(prepared.source_change().is_none());
}

/// A smart-HTTP git server (`git http-backend`) for the bare repositories
/// below a folder, answering `301 Moved Permanently` from `/old/<path>` to
/// `/new/<path>`, as a host answers for a renamed or transferred
/// repository. Counts the connections it accepts.
struct HttpGit {
    port: u16,
    requests: Arc<AtomicUsize>,
}

impl HttpGit {
    fn start(fx: &Fx, git: &Path, root: &Path) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&requests);
        let vars: Vec<(String, OsString)> = git_vars(&fx.th, git)
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect();
        let (git, root) = (git.to_path_buf(), root.to_path_buf());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else {
                    break;
                };
                counted.fetch_add(1, Ordering::SeqCst);
                // A failed exchange is git's error to report.
                let _ = serve_git(stream, &git, &root, &vars);
            }
        });
        Self { port, requests }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}/{path}", self.port)
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

fn serve_git(
    mut stream: TcpStream,
    git: &Path,
    root: &Path,
    vars: &[(String, OsString)],
) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    let header = |name: &str| {
        headers
            .iter()
            .find(|(found, _)| found == name)
            .map(|(_, value)| value.clone())
    };
    let mut body = Vec::new();
    if let Some(length) = header("content-length") {
        body.resize(length.parse().unwrap_or(0), 0);
        reader.read_exact(&mut body)?;
    } else if header("transfer-encoding").is_some_and(|value| value == "chunked") {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size)?;
            let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
            let mut chunk = vec![0; size + 2];
            reader.read_exact(&mut chunk)?;
            if size == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..size]);
        }
    }
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    if let Some(rest) = path.strip_prefix("/old/") {
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{query}")
        };
        return write!(
            stream,
            "HTTP/1.1 301 Moved Permanently\r\nLocation: /new/{rest}{query}\r\n\
             Content-Length: 0\r\nConnection: close\r\n\r\n"
        );
    }
    let Some(rest) = path.strip_prefix("/new") else {
        return write!(
            stream,
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
    };
    let mut command = std::process::Command::new(git);
    command
        .arg("http-backend")
        .env_clear()
        .envs(vars.iter().map(|(name, value)| (name, value)))
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("REQUEST_METHOD", &method)
        .env("PATH_INFO", rest)
        .env("QUERY_STRING", query)
        .env("CONTENT_TYPE", header("content-type").unwrap_or_default())
        .env("CONTENT_LENGTH", body.len().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(encoding) = header("content-encoding") {
        command.env("HTTP_CONTENT_ENCODING", encoding);
    }
    if let Some(protocol) = header("git-protocol") {
        command.env("GIT_PROTOCOL", protocol);
    }
    let mut child = command.spawn()?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let writer = std::thread::spawn(move || stdin.write_all(&body));
    let output = child.wait_with_output()?;
    writer.join().expect("stdin writer")?;
    // A CGI response: headers (`Status:` among them), a blank line, the body.
    let out = output.stdout;
    let (head, start) = match out.windows(4).position(|window| window == b"\r\n\r\n") {
        Some(at) => (&out[..at], at + 4),
        None => match out.windows(2).position(|window| window == b"\n\n") {
            Some(at) => (&out[..at], at + 2),
            None => (&out[..0], 0),
        },
    };
    let mut status = "200 OK".to_owned();
    let mut response = Vec::new();
    for line in String::from_utf8_lossy(head).lines() {
        match line.strip_prefix("Status:") {
            Some(value) => status = value.trim().to_owned(),
            None => response.extend_from_slice(format!("{line}\r\n").as_bytes()),
        }
    }
    let body = &out[start..];
    write!(stream, "HTTP/1.1 {status}\r\n")?;
    stream.write_all(&response)?;
    write!(
        stream,
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

fn meta_url(store: &agent_loadout_core::store::Store<'_>, id: &str) -> Option<String> {
    agent_loadout_core::skills::store::read_meta(store, id)
        .unwrap()
        .and_then(|meta| meta.source)
        .and_then(|source| source.url)
}

#[test]
fn a_moved_repository_asks_for_both_locations_and_follows_the_move() {
    let Some(git) = system_git_or_skip("a_moved_repository_asks_for_both_locations") else {
        return;
    };
    let fx = Fx::new();
    let env = with_git_vars(&fx.th, &git, fx.th.env_builder(host_os())).build();
    let sx = sx(&fx, &env, &RUNNER);
    let store = fx.store();
    let up = upstream(&fx, &git);
    let server = HttpGit::start(&fx, &git, &fx.th.root().join("repos"));
    let old = server.url("old/acme/skills.git");
    let new = server.url("new/acme/skills.git");
    let (old_ref, new_ref) = (
        SourceRef::from_git_url(&old).unwrap(),
        SourceRef::from_git_url(&new).unwrap(),
    );
    assert_ne!(old_ref.key(), new_ref.key());

    // An install through a real 301: the review names both locations.
    let mut request = InstallRequest::new(Source::parse(&old, &fx.fs));
    request.path = Some("skills/alpha".to_owned());
    let mut prepared = install::prepare(&sx, &request).unwrap();
    assert_eq!(prepared.trust_source().unwrap().key(), old_ref.key());
    assert_eq!(prepared.moved_to().unwrap().key(), new_ref.key());
    assert_eq!(
        prepared.source_change().unwrap().kind,
        SourceChangeKind::Moved
    );
    let mut install = |decisions: Decisions| {
        install::install_prepared(&store, &sx, &mut prepared, &Default::default(), &decisions)
    };
    let err = install(Decisions::default()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::SourceTrustRequired);
    assert!(err.message().contains(old_ref.key()), "{err}");
    // The answer for the URL the user gave never carries over to where it
    // moved, and nothing is recorded while the target still asks.
    let commits = fx.commit_count();
    let err = install(trust_with(TrustGrant::Always(SourceScope::Repo))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::SourceTrustRequired);
    assert!(err.message().contains(new_ref.key()), "{err}");
    assert_eq!(fx.commit_count(), commits);
    // Both answered: "always" is recorded for the source it was given for.
    install(Decisions {
        moved_trust: Some(TrustGrant::Once),
        ..trust_with(TrustGrant::Always(SourceScope::Repo))
    })
    .unwrap();
    let keys: Vec<String> = sources::list(&store)
        .unwrap()
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(keys, [old_ref.key().to_owned()]);

    // A push, then a check through the redirect again: the old location's
    // trust lapses and the new location asks.
    fx.write(
        &up.work.join("skills").join("alpha").join("notes.md"),
        "two\n",
    );
    push(&fx, &git, &up.work, "two");
    let checked = install::check_update(&store, &sx, "alpha").unwrap();
    assert_eq!(checked.status, UpdateStatus::Available);
    let change = checked.source_change.clone().unwrap();
    assert_eq!(change.kind, SourceChangeKind::Moved);
    assert_eq!(change.detail.as_deref(), Some(new_ref.key()));
    assert_eq!(
        fx.log()[0].summary,
        format!("source: trust lapsed for {} (moved)", old_ref.key())
    );
    let moved = checked.moved_trust.clone().unwrap();
    assert_eq!(moved.source.key(), new_ref.key());
    assert!(matches!(moved.state, TrustState::Ask { .. }));
    let latest = checked.latest.clone().unwrap();
    let err =
        install::update_reviewed(&store, &sx, "alpha", &latest, &trust_with(TrustGrant::Once))
            .unwrap_err();
    assert_eq!(err.code(), ErrorCode::SourceTrustRequired);
    assert!(err.message().contains(new_ref.key()), "{err}");
    // Trusting the new location: the update moves the skill's URL there.
    install::update_reviewed(
        &store,
        &sx,
        "alpha",
        &latest,
        &Decisions {
            moved_trust: Some(TrustGrant::Always(SourceScope::Repo)),
            ..trust_with(TrustGrant::Once)
        },
    )
    .unwrap();
    assert_eq!(meta_url(&store, "alpha").as_deref(), Some(new_ref.url()));

    // The next check fetches from there: no redirect, no lapse again.
    let commits = fx.commit_count();
    let checked = install::check_update(&store, &sx, "alpha").unwrap();
    assert_eq!(checked.status, UpdateStatus::UpToDate);
    assert_eq!(checked.source_change, None);
    assert!(checked.moved_trust.is_none());
    assert!(matches!(
        checked.trust.unwrap().state,
        TrustState::Trusted { .. }
    ));
    assert_eq!(fx.commit_count(), commits);
    assert!(server.requests() > 0);
}

#[test]
fn a_rollback_force_push_makes_trust_lapse() {
    let Some(git) = system_git_or_skip("a_rollback_force_push_makes_trust_lapse") else {
        return;
    };
    let fx = Fx::new();
    let env = with_git_vars(&fx.th, &git, fx.th.env_builder(host_os())).build();
    let sx = sx(&fx, &env, &RUNNER);
    let store = fx.store();
    let up = upstream(&fx, &git);
    // The first commit is old; a newer one goes on top and is installed.
    let old = "2020-01-01T00:00:00Z";
    let status = fx
        .th
        .command(&git)
        .envs(git_vars(&fx.th, &git))
        .env("GIT_COMMITTER_DATE", old)
        .current_dir(&up.work)
        .args(["commit", "--quiet", "--amend", "--no-edit", "--date", old])
        .status()
        .unwrap();
    assert!(status.success());
    git_run(
        &fx,
        &git,
        &up.work,
        &["push", "--quiet", "--force", "origin", "HEAD:main"],
    );
    fx.write(
        &up.work.join("skills").join("alpha").join("notes.md"),
        "new\n",
    );
    push(&fx, &git, &up.work, "new");
    sources::trust(&store, &source(), SourceScope::Repo, TrustVia::App).unwrap();
    install::install(&store, &sx, &request("skills/alpha", None)).unwrap();

    // Roll the branch back to the old commit.
    git_run(
        &fx,
        &git,
        &up.work,
        &["push", "--quiet", "--force", "origin", "HEAD~1:main"],
    );
    let checked = install::check_update(&store, &sx, "alpha").unwrap();
    assert_eq!(
        checked.status,
        UpdateStatus::Available,
        "{:?}",
        checked.error
    );
    assert_eq!(
        checked.source_change.map(|change| change.kind),
        Some(SourceChangeKind::HistoryRewritten)
    );
    assert_eq!(
        fx.log()[0].summary,
        "source: trust lapsed for github.com/acme/skills (history_rewritten)"
    );
}

#[test]
fn a_blocked_source_is_never_contacted() {
    let Some(git) = system_git_or_skip("a_blocked_source_is_never_contacted") else {
        return;
    };
    let fx = Fx::new();
    let env = with_git_vars(&fx.th, &git, fx.th.env_builder(host_os())).build();
    let sx = sx(&fx, &env, &RUNNER);
    let store = fx.store();
    let up = upstream(&fx, &git);
    install::install(
        &store,
        &sx,
        &request("skills/alpha", Some(TrustGrant::Always(SourceScope::Repo))),
    )
    .unwrap();
    sources::block(&store, &source(), SourceScope::Owner, TrustVia::CliCommand).unwrap();
    // Any contact fails from now on: the repository is gone.
    std::fs::remove_dir_all(&up.bare).unwrap();

    let checked = install::check_update(&store, &sx, "alpha").unwrap();
    assert_eq!(checked.status, UpdateStatus::Blocked);
    assert_eq!(checked.status.as_str(), "source_blocked");
    assert_eq!(checked.error.unwrap().code(), ErrorCode::SourceBlocked);
    assert!(matches!(
        checked.trust.unwrap().state,
        TrustState::Blocked { .. }
    ));
    let err = install::update(&store, &sx, "alpha", &trust_with(TrustGrant::Once)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::SourceBlocked, "{err}");
    let err =
        install::install(&store, &sx, &request("skills/beta", Some(TrustGrant::Once))).unwrap_err();
    assert_eq!(err.code(), ErrorCode::SourceBlocked, "{err}");
    let err = sources::refuse_blocked(&store, &source()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::SourceBlocked);
}

#[test]
fn local_repositories_are_gated_by_path() {
    let Some(git) = system_git_or_skip("local_repositories_are_gated_by_path") else {
        return;
    };
    let fx = Fx::new();
    let env = with_git_vars(&fx.th, &git, fx.th.env_builder(host_os())).build();
    let sx = sx(&fx, &env, &RUNNER);
    let store = fx.store();
    let up = upstream(&fx, &git);
    let url = up.bare.to_string_lossy().into_owned();
    let mut request = InstallRequest::new(Source::parse(&url, &fx.fs));
    request.path = Some("skills/alpha".to_owned());
    let err = install::install(&store, &sx, &request).unwrap_err();
    assert_eq!(err.code(), ErrorCode::SourceTrustRequired);
    request.decisions.trust = Some(TrustGrant::Always(SourceScope::Repo));
    install::install(&store, &sx, &request).unwrap();
    let records = sources::list(&store).unwrap();
    assert!(records[0].0.starts_with("file:///"), "{records:?}");
    assert_eq!(
        records[0].0,
        SourceRef::from_local_path(&up.bare).unwrap().key()
    );
}

//! Git remotes (SPEC 9.6, 9.7, 9.8) against local bare repositories through
//! the system git, isolated from the user's git configuration
//! (`agent_loadout_testkit::git_env`). Tests that need git skip with a
//! notice when it is absent. `gh` is `gh-stub`.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;

use agent_loadout_core::brand;
use agent_loadout_core::paths::{LoadoutHome, loadout_home};
use agent_loadout_core::remote::config::RemoteConfig;
use agent_loadout_core::remote::credential::{self, CredentialRequest, token_account};
use agent_loadout_core::remote::gh;
use agent_loadout_core::remote::git::{
    self, GitAuth, GitCli, Invocation, RunOutput, Runner, SystemRunner,
};
use agent_loadout_core::store::merge::MergeOutcome;
use agent_loadout_core::store::{Operation, Store, bare_refs};
use agent_loadout_core::{Env, ErrorCode, Os};
use agent_loadout_testkit::gh_stub::GhStub;
use agent_loadout_testkit::git_env::{git_vars, init_bare, system_git_or_skip, with_git_vars};
use agent_loadout_testkit::{MemKeyring, RootedFs, TempHome};

const CLIENT: &str = "loadout-test/0.0.0";

fn host_os() -> Os {
    if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    }
}

fn server(id: &str, command: &str) -> String {
    format!("schema = 1\nid = \"{id}\"\ntransport = \"stdio\"\ncommand = \"{command}\"\n")
}

/// A token-shaped value that is no real secret.
fn fake_token() -> String {
    ["stub", "-token-", "0123456789abcdef"].concat()
}

/// Every process a git remote ran, then the real thing.
struct Recording {
    inner: SystemRunner,
    calls: Mutex<Vec<Invocation>>,
}

impl Recording {
    fn new() -> Self {
        Self {
            inner: SystemRunner::default(),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<Invocation> {
        self.calls.lock().unwrap().clone()
    }
}

impl Runner for Recording {
    fn run(
        &self,
        invocation: &Invocation,
        progress: &mut dyn FnMut(&str),
    ) -> io::Result<RunOutput> {
        self.calls.lock().unwrap().push(invocation.clone());
        self.inner.run(invocation, progress)
    }
}

/// Records and answers every call with `output`, running nothing.
struct Scripted {
    output: RunOutput,
    calls: Mutex<Vec<Invocation>>,
}

impl Runner for Scripted {
    fn run(&self, invocation: &Invocation, _: &mut dyn FnMut(&str)) -> io::Result<RunOutput> {
        self.calls.lock().unwrap().push(invocation.clone());
        Ok(self.output.clone())
    }
}

struct World {
    th: TempHome,
    fs: RootedFs,
    git: PathBuf,
    bare: PathBuf,
}

struct Machine {
    env: Env,
    home: LoadoutHome,
}

impl World {
    fn new(test: &str) -> Option<Self> {
        let git = system_git_or_skip(test)?;
        let th = TempHome::new();
        let fs = th.fs();
        let bare = th.root().join("remote.git");
        init_bare(&th, &git, &bare);
        Some(Self { th, fs, git, bare })
    }

    fn machine(&self, label: &str) -> Machine {
        let env = with_git_vars(&self.th, &self.git, self.th.env_builder(host_os()))
            .hostname(label)
            .build();
        let home = loadout_home(&env, &self.fs, Some(&self.th.root().join(label))).unwrap();
        Machine { env, home }
    }

    fn store<'a>(&'a self, machine: &Machine) -> Store<'a> {
        Store::open_or_init(&machine.env, &self.fs, &machine.home, CLIENT).unwrap()
    }

    fn remote(&self, auth: GitAuth) -> RemoteConfig {
        RemoteConfig::git("origin", &self.bare.to_string_lossy(), auth)
    }
}

fn edit(store: &Store<'_>, summary: &str, path: &str, text: String) {
    store
        .transaction(Operation::new("test.edit", summary), |tx| {
            tx.write(path, text)
        })
        .unwrap();
}

fn args(invocation: &Invocation) -> Vec<String> {
    invocation
        .args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

/// No argument and no environment value of any call holds `needle`.
fn assert_nowhere(calls: &[Invocation], needle: &str) {
    for call in calls {
        for arg in &call.args {
            assert!(!arg.to_string_lossy().contains(needle), "argv: {call:?}");
        }
        for (name, value) in &call.env {
            assert!(
                !value.to_string_lossy().contains(needle),
                "env {}: {call:?}",
                name.to_string_lossy()
            );
        }
    }
}

#[test]
fn push_and_pull_through_a_local_bare_repository() {
    let Some(world) = World::new("push_and_pull_through_a_local_bare_repository") else {
        return;
    };
    let runner = Recording::new();
    let remote = world.remote(GitAuth::System);
    let a = world.machine("desk-a");
    let store_a = world.store(&a);
    let cli = GitCli {
        git: world.git.clone(),
        runner: &runner,
        helper: None,
    };
    edit(&store_a, "x", "servers/x.toml", server("x", "x-mcp"));
    store_a
        .create_user_tag("pre-reset", &store_a.head().unwrap(), None)
        .unwrap();
    let pushed = git::push(&store_a, &cli, &remote, &mut |_| {}).unwrap();
    assert_eq!(pushed.head, store_a.head().unwrap());
    assert!(pushed.pulled.is_none());

    // B starts from the bare repository: history and tags.
    let b = world.machine("desk-b");
    let (head, tags) = bare_refs(&world.bare).expect("main in the bare repository");
    let store_b = Store::init_from_bare(
        &b.env,
        &world.fs,
        &b.home,
        CLIENT,
        &world.bare,
        &head,
        &tags,
    )
    .unwrap();
    assert_eq!(store_b.head().unwrap(), store_a.head().unwrap());
    assert!(store_b.tag_exists("pre-reset"));
    edit(&store_b, "y", "servers/y.toml", server("y", "y-mcp"));
    git::push(&store_b, &cli, &remote, &mut |_| {}).unwrap();

    // A is behind: its push is rejected, pulls (merges) and retries once.
    edit(&store_a, "z", "servers/z.toml", server("z", "z-mcp"));
    let pushed = git::push(&store_a, &cli, &remote, &mut |_| {}).unwrap();
    let pulled = pushed.pulled.expect("a rejected push pulls first");
    assert!(
        pulled
            .tag
            .as_deref()
            .is_some_and(|t| t.starts_with("auto/pre-pull/"))
    );
    assert!(
        matches!(pulled.siblings[0].outcome, Ok(MergeOutcome::Merged(_))),
        "{:?}",
        pulled.siblings[0].outcome
    );
    assert_eq!(pulled.siblings[0].machine_id, None);
    for id in ["x", "y", "z"] {
        assert!(
            a.home
                .store_dir()
                .join(format!("servers/{id}.toml"))
                .exists(),
            "{id}"
        );
    }
    let (remote_head, _) = bare_refs(&world.bare).unwrap();
    assert_eq!(remote_head, store_a.head().unwrap().to_string());

    // B pulls: a fast-forward to the same head; again: up to date.
    let pulled = git::pull(&store_b, &cli, &remote, &mut |_| {}).unwrap();
    assert!(matches!(
        pulled.siblings[0].outcome,
        Ok(MergeOutcome::FastForward(_))
    ));
    assert_eq!(store_b.head().unwrap(), store_a.head().unwrap());
    assert!(b.home.store_dir().join("servers/z.toml").exists());
    let again = git::pull(&store_a, &cli, &remote, &mut |_| {}).unwrap();
    assert!(matches!(
        again.siblings[0].outcome,
        Ok(MergeOutcome::UpToDate)
    ));
    assert!(again.tag.is_none());

    // Every git ran in the store folder, with prompts off, with the URL as
    // given, and with no credential flags for `auth = system`.
    let calls = runner.calls();
    assert!(!calls.is_empty());
    for call in &calls {
        assert_eq!(call.program, world.git);
        assert!(call.cwd.ends_with("store"), "{call:?}");
        assert!(
            call.env
                .iter()
                .any(|(name, value)| name == "GIT_TERMINAL_PROMPT" && value == "0")
        );
        let args = args(call);
        assert!(!args.contains(&"-c".to_owned()), "{args:?}");
        assert!(
            args.contains(&world.bare.to_string_lossy().into_owned()),
            "{args:?}"
        );
    }
    assert!(calls.iter().any(|c| args(c)[0] == "push"));
    assert!(calls.iter().any(|c| args(c)[0] == "fetch"));
}

#[test]
fn an_empty_or_missing_remote() {
    let Some(world) = World::new("an_empty_or_missing_remote") else {
        return;
    };
    let runner = SystemRunner::default();
    let cli = GitCli {
        git: world.git.clone(),
        runner: &runner,
        helper: None,
    };
    let a = world.machine("desk-a");
    let store = world.store(&a);
    let head = store.head().unwrap();
    // A new, empty repository: nothing to pull.
    let pulled = git::pull(&store, &cli, &world.remote(GitAuth::System), &mut |_| {}).unwrap();
    assert!(pulled.siblings.is_empty());
    assert!(pulled.tag.is_none());
    assert_eq!(store.head().unwrap(), head);
    // A repository that is not there: unreachable, and local work goes on.
    let missing = RemoteConfig::git(
        "gone",
        &world.th.root().join("missing.git").to_string_lossy(),
        GitAuth::System,
    );
    let err = git::push(&store, &cli, &missing, &mut |_| {}).unwrap_err();
    assert_eq!(err.code(), ErrorCode::RemoteUnreachable, "{err}");
    edit(&store, "x", "servers/x.toml", server("x", "x-mcp"));
}

#[test]
fn scp_style_urls_reach_git_unchanged() {
    let th = TempHome::new();
    let fs = th.fs();
    let env = th.env(host_os());
    let home = loadout_home(&env, &fs, Some(&th.root().join("lh"))).unwrap();
    let store = Store::open_or_init(&env, &fs, &home, CLIENT).unwrap();
    let scripted = Scripted {
        output: RunOutput {
            success: true,
            ..RunOutput::default()
        },
        calls: Mutex::new(Vec::new()),
    };
    let cli = GitCli {
        git: PathBuf::from("git"),
        runner: &scripted,
        helper: None,
    };
    let url = "git@git.example.com:me/loadout-store.git";
    let remote = RemoteConfig::git("origin", url, GitAuth::System);
    git::push(&store, &cli, &remote, &mut |_| {}).unwrap();
    let calls = scripted.calls.lock().unwrap().clone();
    assert_eq!(
        args(&calls[0]),
        [
            "push",
            "--progress",
            "--",
            url,
            "refs/heads/main:refs/heads/main",
            "refs/tags/*:refs/tags/*"
        ]
    );
    assert_eq!(
        git::url_scheme_host(url),
        Some(("ssh".into(), "git.example.com".into()))
    );
    assert!(!git::is_github_url(url));
    assert!(git::is_github_url("https://github.com/me/store.git"));
    assert!(git::is_github_url("git@github.com:me/store.git"));

    // Failures by message; only a non-fast-forward rejection pulls and
    // retries (a protected branch or a hook decline does not).
    for (stderr, code) in [
        (
            "fatal: Could not read from remote repository.",
            ErrorCode::RemoteUnreachable,
        ),
        (
            "fatal: Authentication failed for 'https://git.example.com/me/s.git/'",
            ErrorCode::RemoteAuthFailed,
        ),
        (
            "fatal: could not read Username for 'https://git.example.com': terminal prompts disabled",
            ErrorCode::RemoteAuthFailed,
        ),
        (
            " ! [remote rejected] main -> main (protected branch hook declined)\nerror: failed to push some refs to 'x'",
            ErrorCode::RemoteRejected,
        ),
        (
            "fatal: '/srv/403/store.git' does not appear to be a git repository",
            ErrorCode::RemoteUnreachable,
        ),
    ] {
        let failing = Scripted {
            output: RunOutput {
                success: false,
                stderr: format!("{stderr}\n"),
                ..RunOutput::default()
            },
            calls: Mutex::new(Vec::new()),
        };
        let cli = GitCli {
            git: PathBuf::from("git"),
            runner: &failing,
            helper: None,
        };
        let err = git::push(&store, &cli, &remote, &mut |_| {}).unwrap_err();
        assert_eq!(err.code(), code, "{stderr}");
        assert_eq!(failing.calls.lock().unwrap().len(), 1, "no retry: {stderr}");
    }
}

#[test]
fn hand_edited_names_and_urls_never_reach_git() {
    let th = TempHome::new();
    let fs = th.fs();
    let env = th.env(host_os());
    let home = loadout_home(&env, &fs, Some(&th.root().join("lh"))).unwrap();
    let store = Store::open_or_init(&env, &fs, &home, CLIENT).unwrap();
    let scripted = Scripted {
        output: RunOutput {
            success: true,
            ..RunOutput::default()
        },
        calls: Mutex::new(Vec::new()),
    };
    let cli = GitCli {
        git: PathBuf::from("git"),
        runner: &scripted,
        helper: Some(th.root().join("loadout")),
    };
    // As `remotes.toml` could hold them after a hand edit.
    let mut bad_name = RemoteConfig::git("o", "https://git.example.com/s.git", GitAuth::Token);
    bad_name.name = "o;touch pwned".to_owned();
    let mut bad_url = RemoteConfig::git("o", "x", GitAuth::System);
    bad_url.url = Some("--upload-pack=touch pwned".to_owned());
    for remote in [&bad_name, &bad_url] {
        let err = git::push(&store, &cli, remote, &mut |_| {}).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidValue, "{remote:?}");
        let err = git::pull(&store, &cli, remote, &mut |_| {}).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidValue, "{remote:?}");
    }
    assert!(scripted.calls.lock().unwrap().is_empty());
}

#[test]
fn the_idle_timeout_stops_git_and_its_helpers() {
    let th = TempHome::new();
    let mut env = Vec::new();
    if let Some(root) = std::env::var_os("SYSTEMROOT") {
        env.push((OsString::from("SYSTEMROOT"), root));
    }
    let invocation = Invocation {
        program: agent_loadout_testkit::gh_stub::binary(),
        args: ["--silent-child", "60"].map(OsString::from).to_vec(),
        env,
        cwd: th.root().to_path_buf(),
    };
    let runner = SystemRunner {
        idle_timeout: std::time::Duration::from_secs(1),
    };
    let started = std::time::Instant::now();
    let output = runner.run(&invocation, &mut |_| {}).unwrap();
    assert!(output.timed_out);
    assert!(!output.success);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "returned after {:?}",
        started.elapsed()
    );
}

#[test]
fn an_inherited_git_dir_never_redirects_a_push() {
    let Some(world) = World::new("an_inherited_git_dir_never_redirects_a_push") else {
        return;
    };
    // Loadout run from a git hook of another repository.
    let decoy = world.th.root().join("decoy.git");
    init_bare(&world.th, &world.git, &decoy);
    let env = with_git_vars(&world.th, &world.git, world.th.env_builder(host_os()))
        .hostname("desk-a")
        .var("GIT_DIR", &decoy)
        .var("GIT_WORK_TREE", world.th.root())
        .var("GIT_INDEX_FILE", world.th.root().join("index"))
        .build();
    let home = loadout_home(&env, &world.fs, Some(&world.th.root().join("desk-a"))).unwrap();
    let store = Store::open_or_init(&env, &world.fs, &home, CLIENT).unwrap();
    edit(&store, "x", "servers/x.toml", server("x", "x-mcp"));
    let runner = Recording::new();
    let cli = GitCli {
        git: world.git.clone(),
        runner: &runner,
        helper: None,
    };
    git::push(&store, &cli, &world.remote(GitAuth::System), &mut |_| {}).unwrap();
    let (remote_head, _) = bare_refs(&world.bare).unwrap();
    assert_eq!(remote_head, store.head().unwrap().to_string());
    for call in runner.calls() {
        assert!(
            !call.env.iter().any(|(name, _)| name == "GIT_DIR"
                || name == "GIT_WORK_TREE"
                || name == "GIT_INDEX_FILE"),
            "{call:?}"
        );
    }
}

#[test]
fn token_auth_uses_the_helper_and_never_passes_the_token() {
    let Some(world) = World::new("token_auth_uses_the_helper_and_never_passes_the_token") else {
        return;
    };
    let runner = Recording::new();
    let helper = world.th.root().join("bin").join("loadout.exe");
    let cli = GitCli {
        git: world.git.clone(),
        runner: &runner,
        helper: Some(helper.clone()),
    };
    let a = world.machine("desk-a");
    let store = world.store(&a);
    let remote = world.remote(GitAuth::Token);
    let keyring = MemKeyring::new();
    keyring.insert(
        brand::KEYRING_SERVICE,
        &token_account("origin"),
        &fake_token(),
    );
    edit(&store, "x", "servers/x.toml", server("x", "x-mcp"));
    git::push(&store, &cli, &remote, &mut |_| {}).unwrap();
    git::pull(&store, &cli, &remote, &mut |_| {}).unwrap();

    let calls = runner.calls();
    let helper_text = helper.to_string_lossy().replace('\\', "/");
    for call in &calls {
        let args = args(call);
        assert_eq!(
            args[..4],
            [
                "-c".to_owned(),
                "credential.helper=".to_owned(),
                "-c".to_owned(),
                format!("credential.helper=!\"{helper_text}\" git-credential --remote origin"),
            ]
        );
    }
    assert_nowhere(&calls, &fake_token());
    // The helper git starts reads this data directory's remotes, also when
    // it was chosen with `--home` rather than `LOADOUT_HOME`.
    for call in &calls {
        assert!(
            call.env.iter().any(|(name, value)| name == brand::HOME_ENV_VAR
                && Path::new(value) == a.home.path),
            "{call:?}"
        );
    }

    // The helper's answer: the remote's username (default x-access-token)
    // and the token, for the remote's own protocol and host only.
    let mut https = RemoteConfig::git(
        "origin",
        "https://git.example.com/me/store.git",
        GitAuth::Token,
    );
    let request =
        CredentialRequest::parse("protocol=https\nhost=git.example.com\npath=me/store.git\n\n");
    let answer = credential::get(&https, &request, &keyring)
        .unwrap()
        .unwrap();
    assert_eq!(answer.username, credential::DEFAULT_USERNAME);
    assert_eq!(answer.password.expose(), fake_token());
    assert_eq!(
        answer.to_protocol(),
        format!("username=x-access-token\npassword={}\n", fake_token())
    );
    https.username = Some("me".to_owned());
    let answer = credential::get(&https, &request, &keyring)
        .unwrap()
        .unwrap();
    assert_eq!(answer.username, "me");
    let elsewhere = CredentialRequest::parse("protocol=https\nhost=evil.example.com\n");
    assert_eq!(credential::get(&https, &elsewhere, &keyring).unwrap(), None);
    let plain_http = CredentialRequest::parse("protocol=http\nhost=git.example.com\n");
    assert_eq!(
        credential::get(&https, &plain_http, &keyring).unwrap(),
        None
    );
    let empty = MemKeyring::new();
    assert_eq!(
        credential::get(&https, &request, &empty)
            .unwrap_err()
            .code(),
        ErrorCode::MissingSecret
    );
    let system = RemoteConfig::git("other", "https://git.example.com/x.git", GitAuth::System);
    assert_eq!(
        credential::get(&system, &request, &keyring)
            .unwrap_err()
            .code(),
        ErrorCode::InvalidValue
    );
}

#[test]
fn gh_auth_passes_the_helper_flags_to_git() {
    let Some(world) = World::new("gh_auth_passes_the_helper_flags_to_git") else {
        return;
    };
    let mut stub = GhStub::install(&world.th.bin());
    stub.set_token(&fake_token());
    let remote = RemoteConfig::git("gh", "https://github.com/stub-user/store.git", GitAuth::Gh);
    let flags = git::auth_args(&remote, None, host_os()).unwrap();
    assert_eq!(
        flags,
        [
            "-c",
            "credential.helper=",
            "-c",
            "credential.helper=!gh auth git-credential"
        ]
        .map(OsString::from)
    );
    // git itself runs the helper the flags name: `gh auth git-credential get`.
    let mut child = world
        .th
        .command(&world.git)
        .envs(git_vars(&world.th, &world.git))
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(&flags)
        .args(["credential", "fill"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"protocol=https\nhost=github.com\npath=stub-user/store.git\n\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("username=stub-user"), "{stdout}");
    assert!(
        stdout.contains(&format!("password={}", fake_token())),
        "{stdout}"
    );
    assert!(
        stub.calls()
            .iter()
            .any(|call| call == "auth git-credential get"),
        "{:?}",
        stub.calls()
    );
    // Only for github.com URLs.
    assert!(git::is_github_url(remote.url.as_deref().unwrap()));
}

#[test]
fn create_private_repository_calls_gh_on_the_stub() {
    let th = TempHome::new();
    let mut stub = GhStub::install(&th.bin());
    let env = th.env_builder(host_os()).var("PATH", th.bin()).build();
    let fs = th.fs();
    let gh_path = gh::locate(&env, &fs).expect("the stub on PATH");
    let runner = SystemRunner::default();
    let url = gh::create_private_repo(&env, &gh_path, &runner, "loadout-store").unwrap();
    assert_eq!(url, "https://github.com/stub-user/loadout-store.git");
    assert!(
        stub.calls()
            .contains(&"repo create loadout-store --private".to_owned())
    );
    let err = gh::create_private_repo(&env, &gh_path, &runner, "loadout-store").unwrap_err();
    assert_eq!(err.code(), ErrorCode::AlreadyExists);
    // Names that could be flags never reach gh.
    let calls = stub.calls().len();
    for bad in ["-x", "--private", "a/b/c", "", "a b"] {
        assert_eq!(
            gh::create_private_repo(&env, &gh_path, &runner, bad)
                .unwrap_err()
                .code(),
            ErrorCode::InvalidValue,
            "{bad:?}"
        );
    }
    assert_eq!(stub.calls().len(), calls);
    stub.set_logged_in(false);
    assert_eq!(
        gh::create_private_repo(&env, &gh_path, &runner, "other")
            .unwrap_err()
            .code(),
        ErrorCode::RemoteAuthFailed
    );
    assert!(
        !stub
            .calls()
            .iter()
            .any(|c| c.starts_with("repo create other"))
    );
}

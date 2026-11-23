//! The pending-plan cache and the scoped instant apply (#15): a cached plan
//! is served only while nothing it was planned from changed, and a plan
//! scoped to some servers equals the whole plan on the targets it covers
//! and leaves no work outside them.

mod support;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use agent_loadout_core::Fs;
use agent_loadout_core::apply::cache::{CachedPlan, PlanCache};
use agent_loadout_core::apply::{
    self, ApplyEnv, ApplyRequest, Plan, PlanOptions, ScopeFilter, Touched,
};
use agent_loadout_core::model::applied::ApplyTrigger;
use agent_loadout_core::store::Store;
use support::{Fx, stdio};

fn cached(fx: &Fx, cache: &PlanCache) -> Arc<CachedPlan> {
    cache
        .plan(&fx.ax(), &|fs| {
            Store::open(&fx.env, fs, &fx.home, "loadout-test/0")
        })
        .unwrap()
}

/// Debug text of each target, by file and container.
fn targets(plan: &Plan) -> Vec<(String, String)> {
    plan.targets
        .iter()
        .map(|target| {
            (
                format!("{}#{}", target.file.display(), target.pointer),
                format!("{target:?}"),
            )
        })
        .collect()
}

#[test]
fn a_cached_plan_is_served_until_something_it_read_changes() {
    let fx = Fx::new();
    fx.put_server(stdio("alpha", &["claude-code"], &["a"]));
    let cache = PlanCache::new();

    let first = cached(&fx, &cache);
    let again = cached(&fx, &cache);
    assert!(Arc::ptr_eq(&first, &again), "nothing changed");
    assert_eq!(targets(&first.plan), targets(&fx.plan()));

    // A client edits its file: the next plan sees the foreign entry.
    fx.write(
        &fx.claude(),
        "{\n  \"mcpServers\": {\n    \"theirs\": { \"command\": \"x\" }\n  }\n}\n",
    );
    let edited = cached(&fx, &cache);
    assert!(!Arc::ptr_eq(&first, &edited));
    assert!(
        edited.plan.targets[0]
            .entries
            .iter()
            .any(|entry| entry.name == "theirs")
    );
    assert_eq!(targets(&edited.plan), targets(&fx.plan()));

    // A store commit.
    fx.put_server(stdio("beta", &["claude-code"], &["b"]));
    let committed = cached(&fx, &cache);
    assert!(!Arc::ptr_eq(&edited, &committed));
    assert!(committed.desired().servers.contains_key("beta"));
    assert_eq!(targets(&committed.plan), targets(&fx.plan()));

    // An apply changes the client file and the ledger.
    fx.apply();
    let applied = cached(&fx, &cache);
    assert!(!Arc::ptr_eq(&committed, &applied));
    assert_eq!(targets(&applied.plan), targets(&fx.plan()));
    assert!(!applied.plan.has_work());

    // The drift watcher's invalidation.
    cache.invalidate();
    let invalidated = cached(&fx, &cache);
    assert!(!Arc::ptr_eq(&applied, &invalidated));
    assert!(Arc::ptr_eq(&invalidated, &cached(&fx, &cache)));
}

#[test]
fn a_trust_change_makes_the_cached_plan_stale() {
    let fx = Fx::new();
    fx.put_server(stdio("alpha", &["claude-code"], &["a"]));
    let root = fx.th.projects().join("app");
    fx.manifest(
        &root,
        "project.toml",
        "schema = 1\nid = \"app-1234\"\nname = \"app\"\n\n[defaults]\nclients = [\"claude-code\"]\n\n[servers.local]\ntransport = \"stdio\"\ncommand = \"local-mcp\"\n",
    );
    fx.know_projects(&[("app-1234", &root)]);
    let cache = PlanCache::new();
    let blocked = cached(&fx, &cache);
    assert!(
        blocked
            .plan
            .targets
            .iter()
            .any(|target| target.project_id.is_some() && target.problem.is_some()),
        "the project waits for trust"
    );

    fx.trust(&root);
    let trusted = cached(&fx, &cache);
    assert!(!Arc::ptr_eq(&blocked, &trusted));
    assert!(
        trusted
            .plan
            .targets
            .iter()
            .filter(|target| target.project_id.is_some())
            .all(|target| target.problem.is_none())
    );
    assert_eq!(targets(&trusted.plan), targets(&fx.plan()));
}

/// Client preferences that force Claude Code, omp and Codex on (so
/// `all-detected` selects them) and disable natively in omp.
const PREFS: &str = "schema = 1\n\n[claude-code]\nenabled = true\n\n[codex]\nenabled = true\n\n\
                     [omp]\nenabled = true\nnative_disable = true\n";

/// A known, trusted project with entries derived from store servers under
/// other names (`gh` from `server-0`, `docs` from `server-1`), a
/// suppression of `server-2` and a project-only server; its clients are
/// `all-detected`, so a transport a client cannot run drops it.
fn project(fx: &Fx) {
    let root = fx.th.projects().join("proj");
    fx.manifest(
        &root,
        "project.toml",
        "schema = 1\nid = \"proj-5678\"\nname = \"proj\"\n\n\
         [servers.gh]\nfrom = \"server-0\"\n\n\
         [servers.docs]\nfrom = \"server-1\"\nargs = [\"--docs\"]\n\n\
         [servers.server-2]\nenabled = false\n\n\
         [servers.local]\ntransport = \"stdio\"\ncommand = \"local-mcp\"\n",
    );
    fx.know_projects(&[("proj-5678", &root)]);
    fx.trust(&root);
}

/// `server` as an SSE server: Codex cannot run it.
fn sse(id: &str, clients: &[&str]) -> agent_loadout_core::model::server::Server {
    let mut server = stdio(id, clients, &[]);
    server.body.transport = Some(agent_loadout_core::model::server::Transport::Sse);
    server.body.command = None;
    server.body.args = None;
    server.body.url = Some(format!("https://{id}.example.test/sse"));
    server
}

#[test]
fn scope_parts_equal_scoped_plans() {
    let fx = Fx::new();
    fx.edit("prefs", |tx| tx.write("clients.toml", PREFS.to_owned()));
    for n in 0..3 {
        fx.put_server(stdio(
            &format!("server-{n}"),
            &["claude-code", "omp", "codex"],
            &["a"],
        ));
    }
    project(&fx);
    fx.apply();
    fx.put_server(sse("server-0", &["claude-code", "omp"]));
    let full = fx.plan();
    assert!(
        full.targets
            .iter()
            .any(|target| target.project_id.is_some())
    );
    for scope in [ScopeFilter::System, ScopeFilter::Project, ScopeFilter::All] {
        let alone = fx.plan_with(&PlanOptions {
            scope,
            ..PlanOptions::default()
        });
        let part = full.scope_part(scope);
        assert_eq!(targets(&part), targets(&alone), "{scope:?}");
        assert_eq!(
            format!("{:?}", part.warnings),
            format!("{:?}", alone.warnings)
        );
    }
}

/// A small deterministic generator, so the cases differ but repeat.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, below: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % below
    }
}

#[test]
fn a_server_scoped_plan_is_the_whole_plan_on_the_targets_it_covers() {
    const CLIENTS: [&str; 3] = ["claude-code", "omp", "codex"];
    let mut rng = Lcg(14);
    for case in 0..8 {
        let fx = Fx::new();
        fx.edit("prefs", |tx| tx.write("clients.toml", PREFS.to_owned()));
        let ids: Vec<String> = (0..5).map(|n| format!("server-{n}")).collect();
        let pick = |rng: &mut Lcg| -> Vec<&'static str> {
            CLIENTS
                .iter()
                .copied()
                .filter(|_| rng.next(2) == 0)
                .collect()
        };
        for id in &ids {
            let clients = pick(&mut rng);
            fx.put_server(stdio(id, &clients, &["--v1"]));
        }
        project(&fx);
        fx.apply();
        // Change one server: its clients, its arguments, its transport (SSE,
        // which Codex cannot run), or remove it. The first case turns the
        // server `gh` starts from into SSE.
        let (changed, how) = if case == 0 {
            ("server-0".to_owned(), 2)
        } else {
            (ids[rng.next(ids.len())].clone(), rng.next(4))
        };
        match how {
            0 => fx.put_server(stdio(&changed, &pick(&mut rng), &["--v1"])),
            1 => fx.put_server(stdio(&changed, &pick(&mut rng), &["--v2"])),
            2 => fx.put_server(sse(&changed, &pick(&mut rng))),
            _ => fx.remove_server(&changed),
        };
        let full = fx.plan();
        let scoped = fx.plan_with(&Touched::Servers(vec![changed.clone()]).options());
        let full_targets = targets(&full);
        for (key, debug) in targets(&scoped) {
            assert!(
                full_targets.contains(&(key.clone(), debug)),
                "case {case}: {key} differs from the whole plan"
            );
        }
        let keys: Vec<String> = targets(&scoped).into_iter().map(|(key, _)| key).collect();
        for (target, (key, _)) in full.targets.iter().zip(&full_targets) {
            assert!(
                keys.contains(key) || !target.has_work(),
                "case {case}: {key} has work outside the scope of {changed}"
            );
        }
    }
}

/// What runs before the first read of a file.
type Hook<'a> = Box<dyn FnOnce() + Send + 'a>;

/// The file system with hooks: one run before the first read of a file,
/// and failing replaces of files whose path contains a text.
struct Hooked<'a> {
    inner: &'a dyn Fs,
    before_read: Mutex<Option<(PathBuf, Hook<'a>)>>,
    /// (path text, replaces of such paths that still succeed)
    fail_replace: Mutex<Option<(String, usize)>>,
}

impl<'a> Hooked<'a> {
    fn new(inner: &'a dyn Fs) -> Self {
        Self {
            inner,
            before_read: Mutex::new(None),
            fail_replace: Mutex::new(None),
        }
    }

    fn hook(&self, path: &Path) {
        let mut slot = self.before_read.lock().unwrap();
        if slot.as_ref().is_some_and(|(at, _)| at == path) {
            let (_, hook) = slot.take().unwrap();
            drop(slot);
            hook();
        }
    }
}

impl Fs for Hooked<'_> {
    fn root(&self) -> Option<&Path> {
        self.inner.root()
    }
    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }
    fn is_dir(&self, path: &Path) -> bool {
        self.inner.is_dir(path)
    }
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.hook(path);
        self.inner.read_to_string(path)
    }
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.hook(path);
        self.inner.read(path)
    }
    fn write(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.inner.write(path, contents)
    }
    fn write_new(&self, path: &Path, contents: &[u8], like: Option<&Path>) -> io::Result<()> {
        self.inner.write_new(path, contents, like)
    }
    fn replace(&self, from: &Path, to: &Path) -> io::Result<()> {
        if let Some((text, left)) = self.fail_replace.lock().unwrap().as_mut()
            && to.to_string_lossy().contains(text.as_str())
        {
            if *left == 0 {
                return Err(io::Error::other("injected failure"));
            }
            *left -= 1;
        }
        self.inner.replace(from, to)
    }
    fn write_private(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        self.inner.write_private(path, contents)
    }
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.create_dir_all(path)
    }
    fn create_private_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.create_private_dir_all(path)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_file(path)
    }
    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_dir_all(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        self.inner.read_dir(path)
    }
    fn open_lock_file(&self, path: &Path) -> io::Result<std::fs::File> {
        self.inner.open_lock_file(path)
    }
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.canonicalize(path)
    }
}

/// Review M30-1: a commit lands while a plan is computed (after it read the
/// store); a request that arrives after the commit waits for that plan, and
/// must not get it.
#[test]
fn a_request_after_a_commit_never_gets_the_plan_started_before_it() {
    let fx = Fx::new();
    fx.put_server(stdio("alpha", &["claude-code"], &["a"]));
    fx.write(&fx.claude(), "{}\n");
    let cache = PlanCache::new();
    let hooked = Hooked::new(&fx.fs);
    let (committed, arrived) = mpsc::channel::<()>();
    let fx_ref = &fx;
    *hooked.before_read.lock().unwrap() = Some((
        fx.claude(),
        Box::new(move || {
            fx_ref.put_server(stdio("beta", &["claude-code"], &["b"]));
            committed.send(()).unwrap();
            // Long enough for the later request to arrive and wait.
            std::thread::sleep(Duration::from_millis(500));
        }),
    ));
    std::thread::scope(|scope| {
        let early = scope.spawn(|| {
            let ax = ApplyEnv {
                fs: &hooked,
                ..fx.ax()
            };
            cache
                .plan(&ax, &|fs| {
                    Store::open(&fx.env, fs, &fx.home, "loadout-test/0")
                })
                .unwrap()
        });
        arrived.recv().unwrap();
        let late = cached(&fx, &cache);
        let early = early.join().unwrap();
        assert!(!early.desired().servers.contains_key("beta"));
        assert!(
            late.desired().servers.contains_key("beta"),
            "the request after the commit got the plan started before it"
        );
    });
}

/// Review M30-2: the journal cannot be written after the first file was:
/// what that file got stays owned, so the next apply cleans it up.
#[test]
fn a_run_stopped_by_the_journal_keeps_what_it_wrote_owned() {
    let fx = Fx::new();
    fx.put_server(stdio("alpha", &["claude-code"], &["a"]));
    let failing = Hooked::new(&fx.fs);
    // The journal's start is written; its first record is not.
    *failing.fail_replace.lock().unwrap() = Some(("journal".to_owned(), 1));
    let ax = ApplyEnv {
        fs: &failing,
        ..fx.ax()
    };
    let request = ApplyRequest {
        options: PlanOptions::default(),
        trigger: ApplyTrigger::Review,
    };
    assert!(apply::run(&fx.store(), &ax, &request).is_err());
    assert!(fx.read(&fx.claude()).contains("\"alpha\""));
    assert!(
        fx.ledger().entries.iter().any(|row| row.key == "alpha"),
        "the written entry is owned"
    );

    fx.remove_server("alpha");
    fx.apply();
    assert!(!fx.read(&fx.claude()).contains("\"alpha\""));
}

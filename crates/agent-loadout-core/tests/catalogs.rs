//! Catalogs (#26): `catalogs.toml`, the "Online catalogs" switch (off:
//! zero requests), registry search through a local HTTP stub (cache, TTL,
//! stale results offline, one slow catalog not holding the others), a git
//! tap laid out like anthropics/skills with a marketplace of every plugin
//! source kind, and the install handoff with provenance into the M41
//! review and the git skill install. Nothing reaches the network: the
//! registry is a stub thread on 127.0.0.1 and taps are local bare
//! repositories made with the system git (those tests skip without it).

mod support;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use agent_loadout_core::adapters::detect::NoRegistry;
use agent_loadout_core::catalog::{
    self, CatalogEnv, CatalogSourceKind, EntryKind, HttpLimits, McpRequest, NewCatalog,
    PluginSourceKind, ResultStatus, UreqHttp,
};
use agent_loadout_core::mcp_source::{self, Pick};
use agent_loadout_core::model::server::ServerSourceKind;
use agent_loadout_core::remote::git::{Invocation, RunOutput, Runner, SystemRunner};
use agent_loadout_core::skills::install::{self, Decisions, UpdateStatus};
use agent_loadout_core::skills::{LinkKind, SkillsEnv, store as skill_store};
use agent_loadout_core::sources::{TrustGrant, TrustVia};
use agent_loadout_core::store::{Store, StoreRead};
use agent_loadout_core::{Env, ErrorCode, FixedClock};
use agent_loadout_testkit::git_env::{git_vars, init_bare, system_git_or_skip, with_git_vars};
use support::{Fx, copy_dir, fixtures, host_os};

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

fn once() -> Decisions {
    Decisions {
        trust: Some(TrustGrant::Once),
        via: TrustVia::CliFlag,
        ..Decisions::default()
    }
}

fn registry_fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixtures().join("catalogs").join("registry").join(name)).unwrap()
}

// ------------------------------------------------------------- HTTP stub

type Routes = Arc<Mutex<Vec<(String, u16, Vec<u8>)>>>;

/// A local HTTP server: answers `GET`s by the first route whose text the
/// request target contains (404 otherwise), after `delay`; counts them.
struct Stub {
    url: String,
    hits: Arc<AtomicUsize>,
    routes: Routes,
}

impl Stub {
    fn start(delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let routes: Routes = Arc::default();
        let (thread_hits, thread_routes) = (hits.clone(), routes.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let (hits, routes) = (thread_hits.clone(), thread_routes.clone());
                std::thread::spawn(move || {
                    let mut request = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut buf) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => request.extend_from_slice(&buf[..n]),
                        }
                    }
                    let text = String::from_utf8_lossy(&request);
                    let target = text.split(' ').nth(1).unwrap_or("/").to_owned();
                    hits.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(delay);
                    let (status, body) = routes
                        .lock()
                        .unwrap()
                        .iter()
                        .find(|(part, ..)| target.contains(part.as_str()))
                        .map(|(_, status, body)| (*status, body.clone()))
                        .unwrap_or((404, b"{}".to_vec()));
                    let head = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                });
            }
        });
        Self { url, hits, routes }
    }

    fn route(&self, part: &str, status: u16, body: Vec<u8>) {
        self.routes
            .lock()
            .unwrap()
            .insert(0, (part.to_owned(), status, body));
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

/// A store with the built-in catalogs off (they would reach the network)
/// and `catalogs` added.
fn store_with<'a>(fx: &'a Fx, catalogs: &[(&str, CatalogSourceKind, &str)]) -> Store<'a> {
    let store = fx.store();
    for id in ["mcp-registry", "anthropic-skills", "skills-sh"] {
        catalog::set_enabled(&store, id, false).unwrap();
    }
    for (id, kind, url) in catalogs {
        store_add(&store, id, *kind, url);
    }
    store
}

fn store_add(store: &Store<'_>, id: &str, kind: CatalogSourceKind, url: &str) {
    let new = NewCatalog {
        kind,
        url: url.to_owned(),
        id: Some(id.to_owned()),
        git_ref: None,
        title: None,
    };
    catalog::add(store, &new).unwrap();
}

fn online(fx: &Fx) {
    catalog::set_online(&fx.fs, &fx.home, true).unwrap();
}

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000 + seconds)
}

fn env_at(fx: &Fx, seconds: u64) -> Env {
    fx.th
        .env_builder(host_os())
        .clock(FixedClock(at(seconds)))
        .build()
}

fn http(env: &Env) -> UreqHttp {
    UreqHttp::new(env, HttpLimits::default())
}

// ------------------------------------------------------------------ store

#[test]
fn catalogs_toml_adds_removes_and_turns_catalogs_on_and_off() {
    let fx = Fx::new();
    let store = fx.store();
    let ids: Vec<String> = catalog::list(Some(&store))
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(ids, ["mcp-registry", "anthropic-skills", "skills-sh"]);

    let registry = NewCatalog {
        kind: CatalogSourceKind::McpRegistry,
        url: "https://registry.acme.example/".to_owned(),
        id: None,
        git_ref: None,
        title: Some("Acme".to_owned()),
    };
    let (id, commit) = catalog::add(&store, &registry).unwrap();
    assert_eq!(id, "registry.acme.example");
    assert!(commit.is_some());
    let again = catalog::add(&store, &registry).unwrap_err();
    assert_eq!(again.code(), ErrorCode::AlreadyExists);
    let tap = NewCatalog {
        kind: CatalogSourceKind::GitTap,
        url: "https://github.com/acme/skills.git".to_owned(),
        id: None,
        git_ref: Some("main".to_owned()),
        title: None,
    };
    assert_eq!(catalog::add(&store, &tap).unwrap().0, "skills");
    let insecure = NewCatalog {
        url: "http://registry.other.example".to_owned(),
        ..registry.clone()
    };
    assert_eq!(
        catalog::add(&store, &insecure).unwrap_err().code(),
        ErrorCode::InvalidValue
    );

    let env = env_at(&fx, 0);
    let sx = sx(&fx, &env);
    assert_eq!(
        catalog::remove(&store, &sx, "mcp-registry")
            .unwrap_err()
            .code(),
        ErrorCode::InvalidValue
    );
    catalog::set_enabled(&store, "mcp-registry", false).unwrap();
    catalog::set_enabled(&store, "skills", false).unwrap();
    assert!(
        catalog::set_enabled(&store, "skills", false)
            .unwrap()
            .is_none()
    );
    let text = store.read_file("catalogs.toml").unwrap().unwrap();
    assert!(
        text.contains("[catalogs.mcp-registry]\nenabled = false"),
        "{text}"
    );
    let catalogs = catalog::list(Some(&store)).unwrap();
    let skills = catalogs.iter().find(|c| c.id == "skills").unwrap();
    assert!(!skills.enabled);
    assert_eq!(skills.git_ref.as_deref(), Some("main"));
    assert_eq!(skills.url, "https://github.com/acme/skills.git");
    assert_eq!(
        catalog::contacts(&catalogs),
        ["github.com", "registry.acme.example"],
        "only enabled catalogs are contacted"
    );

    catalog::set_enabled(&store, "mcp-registry", true).unwrap();
    catalog::remove(&store, &sx, "skills").unwrap();
    let text = store.read_file("catalogs.toml").unwrap().unwrap();
    assert!(!text.contains("mcp-registry"), "{text}");
    assert!(!text.contains("skills"), "{text}");
}

// ---------------------------------------------------------------- setting

#[test]
fn with_online_catalogs_off_nothing_is_contacted() {
    let fx = Fx::new();
    let stub = Stub::start(Duration::ZERO);
    stub.route(
        "/v0.1/servers",
        200,
        registry_fixture("search-weather.json"),
    );
    let store = store_with(&fx, &[("acme", CatalogSourceKind::McpRegistry, &stub.url)]);
    let setting = catalog::online(&fx.fs, &fx.home).unwrap();
    assert!(!setting.on && !setting.decided, "off until turned on");
    let env = env_at(&fx, 0);
    let sx = sx(&fx, &env);
    let http = http(&env);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };

    let results = catalog::search(&cx, Some(&store), "weather", None).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, ResultStatus::Off);
    assert_eq!(
        results[0].error.as_ref().unwrap().code(),
        ErrorCode::CatalogsOff
    );
    for err in [
        catalog::show(&cx, Some(&store), "acme", "io.github.acme/weather").unwrap_err(),
        catalog::refresh(&cx, Some(&store), None).unwrap_err(),
        catalog::mcp_handoff(
            &cx,
            Some(&store),
            "catalog:acme/io.github.acme/weather",
            None,
        )
        .unwrap_err(),
    ] {
        assert_eq!(err.code(), ErrorCode::CatalogsOff, "{err}");
    }
    assert_eq!(stub.hits(), 0, "online catalogs off: zero requests");

    // Turned off again after use: the cache answers, still no request.
    online(&fx);
    catalog::search(&cx, Some(&store), "weather", None).unwrap();
    assert_eq!(stub.hits(), 1);
    catalog::set_online(&fx.fs, &fx.home, false).unwrap();
    let results = catalog::search(&cx, Some(&store), "weather", None).unwrap();
    assert_eq!(results[0].status, ResultStatus::Off);
    assert_eq!(results[0].entries.len(), 3);
    assert_eq!(stub.hits(), 1);
    assert!(catalog::online(&fx.fs, &fx.home).unwrap().decided);
}

// ----------------------------------------------------------------- search

#[test]
fn registry_results_are_cached_for_a_day_and_stale_ones_labeled_offline() {
    let fx = Fx::new();
    let stub = Stub::start(Duration::ZERO);
    stub.route(
        "/v0.1/servers",
        200,
        registry_fixture("search-weather.json"),
    );
    let store = store_with(&fx, &[("acme", CatalogSourceKind::McpRegistry, &stub.url)]);
    online(&fx);
    let env = env_at(&fx, 0);
    let sx0 = sx(&fx, &env);
    let http = http(&env);
    let cx = CatalogEnv {
        sx: &sx0,
        http: &http,
    };

    let short = catalog::search_one(&cx, Some(&store), "acme", "we", None).unwrap();
    assert_eq!(short.status, ResultStatus::QueryTooShort);
    assert!(short.error.unwrap().message().contains("3 characters"));
    assert_eq!(stub.hits(), 0);

    let fresh = catalog::search_one(&cx, Some(&store), "acme", "weather", None).unwrap();
    assert_eq!(fresh.status, ResultStatus::Fresh);
    assert_eq!(fresh.entries.len(), 3);
    assert_eq!(
        fresh.attribution, None,
        "attribution is the official registry's"
    );
    assert_eq!(stub.hits(), 1);

    let later = env_at(&fx, 60 * 60);
    let sx1 = sx(&fx, &later);
    let cx1 = CatalogEnv {
        sx: &sx1,
        http: &http,
    };
    let cached = catalog::search_one(&cx1, Some(&store), "acme", "weather", None).unwrap();
    assert_eq!(cached.status, ResultStatus::Cached);
    assert_eq!(cached.age, Some(Duration::from_secs(3600)));
    assert_eq!(stub.hits(), 1, "a fresh cache answers without a request");

    // A day later the registry fails: the old results, labeled.
    stub.route("/v0.1/servers", 503, b"{}".to_vec());
    let next_day = env_at(&fx, 25 * 60 * 60);
    let sx2 = sx(&fx, &next_day);
    let cx2 = CatalogEnv {
        sx: &sx2,
        http: &http,
    };
    let stale = catalog::search_one(&cx2, Some(&store), "acme", "weather", None).unwrap();
    assert_eq!(stale.status, ResultStatus::Stale);
    assert_eq!(stale.age, Some(Duration::from_secs(25 * 60 * 60)));
    assert_eq!(stale.entries.len(), 3);
    assert_eq!(
        stale.error.as_ref().unwrap().code(),
        ErrorCode::CatalogUnreachable
    );
    assert_eq!(stub.hits(), 2);

    // A refresh that fails keeps the stale results ("Try again" while
    // still offline loses nothing), and the links of those entries.
    let refreshed = catalog::refresh(&cx2, Some(&store), Some("acme")).unwrap();
    assert_eq!(refreshed[0].status, ResultStatus::Error);
    let kept = catalog::search_one(&cx2, Some(&store), "acme", "weather", None).unwrap();
    assert_eq!(kept.status, ResultStatus::Stale);
    assert_eq!(kept.entries.len(), 3);
    let listed = kept.entries[0].id.clone();
    assert!(catalog::cached_entry(&sx2, Some(&store), "acme", &listed).is_ok());

    // A refresh that works drops the old pages: the next search fetches.
    stub.route(
        "/v0.1/servers",
        200,
        registry_fixture("search-weather.json"),
    );
    let refreshed = catalog::refresh(&cx2, Some(&store), Some("acme")).unwrap();
    assert_eq!(refreshed[0].status, ResultStatus::Fresh);
    let before = stub.hits();
    let fresh = catalog::search_one(&cx2, Some(&store), "acme", "weather", None).unwrap();
    assert_eq!(fresh.status, ResultStatus::Fresh);
    assert_eq!(stub.hits(), before + 1);
}

#[test]
fn one_slow_catalog_does_not_hold_the_others() {
    let fx = Fx::new();
    let fast = Stub::start(Duration::ZERO);
    fast.route(
        "/v0.1/servers",
        200,
        registry_fixture("search-weather.json"),
    );
    let slow = Stub::start(Duration::from_secs(4));
    slow.route(
        "/v0.1/servers",
        200,
        registry_fixture("search-weather.json"),
    );
    let store = store_with(
        &fx,
        &[
            ("fast", CatalogSourceKind::McpRegistry, &fast.url),
            ("slow", CatalogSourceKind::McpRegistry, &slow.url),
        ],
    );
    online(&fx);
    let env = env_at(&fx, 0);
    let sx = sx(&fx, &env);
    let limits = HttpLimits {
        connect: Duration::from_secs(1),
        total: Duration::from_secs(1),
        max_bytes: 1024 * 1024,
    };
    let http = UreqHttp::new(&env, limits);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };
    let started = Instant::now();
    let results = catalog::search(&cx, Some(&store), "weather", None).unwrap();
    let elapsed = started.elapsed();
    let status = |id: &str| results.iter().find(|r| r.catalog == id).unwrap();
    assert_eq!(status("fast").status, ResultStatus::Fresh);
    assert_eq!(status("fast").entries.len(), 3);
    assert_eq!(status("slow").status, ResultStatus::Error);
    assert_eq!(
        status("slow").error.as_ref().unwrap().code(),
        ErrorCode::CatalogUnreachable
    );
    assert!(elapsed < Duration::from_secs(3), "took {elapsed:?}");
}

// ---------------------------------------------------------------- registry

#[test]
fn a_registry_entry_installs_through_the_m41_review_and_updates() {
    let fx = Fx::new();
    let stub = Stub::start(Duration::ZERO);
    stub.route(
        "/versions/latest",
        200,
        registry_fixture("entry-weather-1.2.0.json"),
    );
    stub.route("weather-gone", 200, registry_fixture("entry-deleted.json"));
    let store = store_with(&fx, &[("acme", CatalogSourceKind::McpRegistry, &stub.url)]);
    online(&fx);
    let env = env_at(&fx, 0);
    let sx = sx(&fx, &env);
    let http = http(&env);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };

    let err = catalog::mcp_handoff(
        &cx,
        Some(&store),
        "catalog:acme/io.github.acme/weather-gone",
        None,
    )
    .unwrap_err();
    assert_eq!(
        err.code(),
        ErrorCode::NotFound,
        "deleted entries are refused"
    );

    let handoff = catalog::mcp_handoff(
        &cx,
        Some(&store),
        "catalog:acme/io.github.acme/weather",
        None,
    )
    .unwrap();
    assert!(matches!(handoff.request, McpRequest::Registry(_)));
    // Nothing is asked before the review: each draft is gated on what it
    // runs (its npm package), never on the claimed repository.
    assert!(handoff.trust_source().unwrap().is_none());
    let mut resolved = handoff.resolve(Some(&store), &sx).unwrap();
    let draft = &resolved.drafts()[0];
    assert_eq!(
        draft.trust_source.as_ref().unwrap().key(),
        "registry.npmjs.org/%40acme/weather-mcp"
    );
    let source = draft.provenance();
    assert_eq!(source.kind, ServerSourceKind::Registry);
    assert_eq!(
        source.catalog.as_deref(),
        Some("acme/io.github.acme/weather")
    );
    assert_eq!(source.version.as_deref(), Some("1.2.0"));
    assert_eq!(source.git_ref, None);
    assert_eq!(source.url.as_deref(), Some(stub.url.as_str()));
    assert!(
        draft
            .server
            .body
            .args
            .as_ref()
            .unwrap()
            .iter()
            .any(|a| a.contains("@acme/weather-mcp@1.2.0"))
    );
    let pick = Pick {
        key: draft.key.clone(),
        server: draft.server.clone(),
        trust: None,
    };
    let id = draft.server.id.clone();
    // The trust gate applies as for any source.
    let err = mcp_source::save(
        &store,
        &sx,
        &fx.ax(),
        &mut resolved,
        std::slice::from_ref(&pick),
        &Decisions::default(),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::SourceTrustRequired);
    let saved = mcp_source::save(&store, &sx, &fx.ax(), &mut resolved, &[pick], &once()).unwrap();
    assert_eq!(saved.ids, std::slice::from_ref(&id));
    let server = store.server(&id).unwrap().unwrap().into_value();
    let meta = server.body.meta.unwrap();
    assert_eq!(meta.origin.as_deref(), Some("catalog:acme"));
    assert_eq!(
        meta.source.unwrap().catalog.as_deref(),
        Some("acme/io.github.acme/weather")
    );
    let text = store
        .read_file(&format!("servers/{id}.toml"))
        .unwrap()
        .unwrap();
    assert!(text.contains("kind = \"registry\""), "{text}");

    // A newer version appears: the update check reads the registry again.
    stub.route(
        "/versions/latest",
        200,
        registry_fixture("entry-weather-1.3.0.json"),
    );
    let check = mcp_source::check_update(&store, &sx, &id).unwrap();
    assert_eq!(check.status, UpdateStatus::Available, "{:?}", check.error);
    assert_eq!(check.current.as_deref(), Some("1.2.0"));
    assert_eq!(check.version.as_deref(), Some("1.3.0"));
    assert_eq!(
        check.trust.as_ref().unwrap().source.key(),
        "registry.npmjs.org/%40acme/weather-mcp"
    );
    // The update token is the content's hash, not the version string.
    let err = mcp_source::update(&store, &sx, &fx.ax(), &id, Some("1.3.0"), &once()).unwrap_err();
    assert_eq!(err.code(), ErrorCode::EntryChanged);
    let token = check.latest.clone().unwrap();
    let updated = mcp_source::update(&store, &sx, &fx.ax(), &id, Some(&token), &once()).unwrap();
    assert_eq!(updated.to.as_deref(), Some("1.3.0"));
    assert!(updated.commit.is_some());
    let server = store.server(&id).unwrap().unwrap().into_value();
    let source = server.body.meta.unwrap().source.unwrap();
    assert_eq!(source.version.as_deref(), Some("1.3.0"));

    // Online catalogs off: the check says so instead of contacting.
    let hits = stub.hits();
    catalog::set_online(&fx.fs, &fx.home, false).unwrap();
    let check = mcp_source::check_update(&store, &sx, &id).unwrap();
    assert_eq!(check.status, UpdateStatus::Unreachable);
    assert_eq!(check.error.unwrap().code(), ErrorCode::CatalogsOff);
    assert_eq!(stub.hits(), hits);

    // Online again, but the catalog turned off, or removed and added again
    // at another address: nothing is contacted.
    online(&fx);
    catalog::set_enabled(&store, "acme", false).unwrap();
    let check = mcp_source::check_update(&store, &sx, &id).unwrap();
    assert_eq!(check.status, UpdateStatus::Unreachable);
    assert!(
        check.error.unwrap().message().contains("turned off"),
        "a turned-off catalog says so"
    );
    assert_eq!(stub.hits(), hits);
    catalog::remove(&store, &sx, "acme").unwrap();
    let other = Stub::start(Duration::ZERO);
    store_add(&store, "acme", CatalogSourceKind::McpRegistry, &other.url);
    let check = mcp_source::check_update(&store, &sx, &id).unwrap();
    assert_eq!(check.status, UpdateStatus::Unreachable);
    assert_eq!(check.error.unwrap().code(), ErrorCode::NotFound);
    assert_eq!((stub.hits(), other.hits()), (hits, 0));
}

#[test]
fn a_pinned_registry_version_is_not_followed() {
    let fx = Fx::new();
    let stub = Stub::start(Duration::ZERO);
    stub.route(
        "/versions/1.2.0",
        200,
        registry_fixture("entry-weather-1.2.0.json"),
    );
    let store = store_with(&fx, &[("acme", CatalogSourceKind::McpRegistry, &stub.url)]);
    online(&fx);
    let env = env_at(&fx, 0);
    let sx = sx(&fx, &env);
    let http = http(&env);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };
    let handoff = catalog::mcp_handoff(
        &cx,
        Some(&store),
        "catalog:acme/io.github.acme/weather",
        Some("1.2.0"),
    )
    .unwrap();
    let mut resolved = handoff.resolve(Some(&store), &sx).unwrap();
    let draft = resolved.drafts()[0].clone();
    assert_eq!(draft.provenance().git_ref.as_deref(), Some("1.2.0"));
    let pick = Pick {
        key: draft.key.clone(),
        server: draft.server.clone(),
        trust: None,
    };
    mcp_source::save(&store, &sx, &fx.ax(), &mut resolved, &[pick], &once()).unwrap();
    let hits = stub.hits();
    let check = mcp_source::check_update(&store, &sx, &draft.server.id).unwrap();
    assert_eq!(check.status, UpdateStatus::UpToDate);
    assert_eq!(stub.hits(), hits, "a pinned version is not looked up");
}

// --------------------------------------------------------------------- taps

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

/// A bare repository holding `fixtures/catalogs/tap`.
fn tap_repo(fx: &Fx, git: &Path) -> PathBuf {
    let root = fx.th.root();
    let bare = root.join("skills.git");
    init_bare(&fx.th, git, &bare);
    let work = root.join("skills-work");
    git_run(
        fx,
        git,
        root,
        &["init", "--quiet", "--initial-branch=main", "skills-work"],
    );
    git_run(
        fx,
        git,
        &work,
        &["remote", "add", "origin", "../skills.git"],
    );
    copy_dir(&fixtures().join("catalogs").join("tap"), &work);
    git_run(fx, git, &work, &["add", "-A"]);
    git_run(fx, git, &work, &["commit", "--quiet", "-m", "tap"]);
    git_run(fx, git, &work, &["push", "--quiet", "origin", "HEAD:main"]);
    bare
}

#[test]
fn a_tap_is_indexed_like_anthropics_skills_with_every_plugin_source_kind() {
    let Some(git) = system_git_or_skip("a_tap_is_indexed_like_anthropics_skills") else {
        return;
    };
    let fx = Fx::new();
    let env = with_git_vars(&fx.th, &git, fx.th.env_builder(host_os())).build();
    let sx = sx(&fx, &env);
    let bare = tap_repo(&fx, &git);
    let url = bare.to_string_lossy().into_owned();
    let store = store_with(&fx, &[("tap", CatalogSourceKind::GitTap, &url)]);
    online(&fx);
    let stub = Stub::start(Duration::ZERO);
    let http = http(&env);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };

    let results = catalog::search_one(&cx, Some(&store), "tap", "", None).unwrap();
    assert_eq!(results.status, ResultStatus::Fresh, "{:?}", results.error);
    let entry = |id: &str| {
        results
            .entries
            .iter()
            .find(|e| e.id == id)
            .unwrap_or_else(|| {
                panic!(
                    "no {id}: {:?}",
                    results.entries.iter().map(|e| &e.id).collect::<Vec<_>>()
                )
            })
    };
    let skills: Vec<&str> = results
        .entries
        .iter()
        .filter(|e| e.kind == EntryKind::Skill)
        .map(|e| e.id.as_str())
        .collect();
    assert_eq!(
        skills,
        ["pdf", "xlsx", "forecast"],
        "template/ is not a skill"
    );
    let pdf = entry("pdf");
    assert_eq!(pdf.path.as_deref(), Some("skills/pdf"));
    assert_eq!(
        pdf.description.as_deref(),
        Some("Read, fill and merge PDF files.")
    );
    assert_eq!(
        pdf.license.as_deref(),
        Some("Complete terms in LICENSE.txt")
    );
    assert_eq!(entry("xlsx").license.as_deref(), Some("MIT"));
    assert!(pdf.install_skill && !pdf.install_mcp);
    assert!(!pdf.official, "a tap outside github.com/anthropics");
    assert!(
        pdf.warning.as_deref().unwrap().contains("reserved"),
        "a reserved marketplace name outside github.com/anthropics"
    );
    assert_eq!(pdf.git_ref.as_deref(), Some("main"));
    assert!(!pdf.pinned);

    let docs = entry("plugin/document-skills");
    assert_eq!(docs.skills, ["pdf", "xlsx"]);
    assert!(docs.install_skill);
    assert!(!docs.install_mcp);

    let weather = entry("plugin/weather-tools");
    assert_eq!(weather.skills, ["forecast"]);
    assert_eq!(weather.servers, ["broken", "forecast", "notion"]);
    assert_eq!(weather.not_imported, ["hooks", "commands"]);
    assert_eq!(weather.license.as_deref(), Some("MIT"));
    assert_eq!(weather.version.as_deref(), Some("2.0.0"));
    assert_eq!(
        weather.publisher.as_ref().unwrap().name,
        "Acme Weather Team"
    );
    assert_eq!(weather.path.as_deref(), Some("plugins/weather-tools"));
    assert!(weather.install_mcp && weather.install_skill);

    let gh = entry("plugin/gh-plugin");
    let source = gh.source.as_ref().unwrap();
    assert_eq!(source.kind, PluginSourceKind::Github);
    assert_eq!(
        source.url.as_deref(),
        Some("https://github.com/acme/gh-plugin")
    );
    assert!(gh.pinned);
    assert_eq!(
        gh.commit.as_deref(),
        Some("0123456789abcdef0123456789abcdef01234567")
    );
    assert!(gh.install_mcp && gh.problem.is_none());

    let url_plugin = entry("plugin/url-plugin");
    assert_eq!(
        url_plugin.source.as_ref().unwrap().kind,
        PluginSourceKind::Url
    );
    assert_eq!(url_plugin.git_ref.as_deref(), Some("v1"));
    assert!(!url_plugin.pinned);

    let subdir = entry("plugin/subdir-plugin").source.clone().unwrap();
    assert_eq!(subdir.kind, PluginSourceKind::GitSubdir);
    assert_eq!(
        subdir.url.as_deref(),
        Some("https://github.com/acme/monorepo")
    );
    assert_eq!(subdir.path.as_deref(), Some("tools/x"));

    for (id, kind) in [
        ("plugin/npm-plugin", PluginSourceKind::Npm),
        ("plugin/archive-plugin", PluginSourceKind::Archive),
        ("plugin/command-plugin", PluginSourceKind::Command),
        ("plugin/escape-plugin", PluginSourceKind::Relative),
        ("plugin/dotdot-plugin", PluginSourceKind::Relative),
        ("plugin/subdir-escape", PluginSourceKind::GitSubdir),
    ] {
        let refused = entry(id);
        assert_eq!(refused.source.as_ref().unwrap().kind, kind, "{id}");
        assert!(refused.problem.is_some(), "{id}");
        assert!(!refused.install_mcp && !refused.install_skill, "{id}");
    }
    assert!(results.entries.iter().all(|e| e.id != "plugin/bad name!"));

    let found = catalog::search_one(&cx, Some(&store), "tap", "PDF", None).unwrap();
    let ids: Vec<&str> = found.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["pdf", "plugin/document-skills"]);
    assert_eq!(found.status, ResultStatus::Cached);
    let shown = catalog::show(&cx, Some(&store), "tap", "plugin/weather-tools").unwrap();
    assert_eq!(shown.entry.servers.len(), 3);
    assert_eq!(stub.hits(), 0, "taps use git, not HTTP");
    let tmp: Vec<_> = std::fs::read_dir(fx.home.cache_dir().join("catalogs"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
        .collect();
    assert!(tmp.is_empty(), "the clone is removed after indexing");
}

#[test]
fn tap_entries_install_through_the_existing_reviews_with_provenance() {
    let Some(git) = system_git_or_skip("tap_entries_install_through_the_existing_reviews") else {
        return;
    };
    let fx = Fx::new();
    let env = with_git_vars(&fx.th, &git, fx.th.env_builder(host_os())).build();
    let sx = sx(&fx, &env);
    let bare = tap_repo(&fx, &git);
    let url = bare.to_string_lossy().into_owned();
    let store = store_with(&fx, &[("tap", CatalogSourceKind::GitTap, &url)]);
    online(&fx);
    let http = http(&env);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };

    // A skill: the git skill install at its folder.
    let (mut request, entry) =
        catalog::skill_request(&cx, Some(&store), "catalog:tap/pdf", None, None).unwrap();
    assert_eq!(entry.id, "pdf");
    assert_eq!(request.path.as_deref(), Some("skills/pdf"));
    assert_eq!(request.catalog.as_deref(), Some("tap/pdf"));
    request.decisions = once();
    let installed = install::install(&store, &sx, &request).unwrap();
    let meta = skill_store::read_meta(&store, &installed.id)
        .unwrap()
        .unwrap();
    let source = meta.source.unwrap();
    assert_eq!(source.catalog.as_deref(), Some("tap/pdf"));
    assert_eq!(source.path.as_deref(), Some("skills/pdf"));

    // A plugin with several skills names them; one with one installs it.
    let err = catalog::skill_request(
        &cx,
        Some(&store),
        "catalog:tap/plugin/document-skills",
        None,
        None,
    )
    .unwrap_err();
    assert!(err.message().contains("catalog:tap/xlsx"), "{err}");
    let (request, _) = catalog::skill_request(
        &cx,
        Some(&store),
        "catalog:tap/plugin/weather-tools",
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        request.path.as_deref(),
        Some("plugins/weather-tools/skills/forecast")
    );
    let err = catalog::skill_request(
        &cx,
        Some(&store),
        "catalog:tap/plugin/npm-plugin",
        None,
        None,
    )
    .unwrap_err();
    assert!(err.message().contains("npm"), "{err}");

    // An MCP plugin: the M41 git review of its folder; its
    // ${CLAUDE_PLUGIN_ROOT} server runs from a checkout.
    let err = catalog::mcp_handoff(&cx, Some(&store), "catalog:tap/pdf", None).unwrap_err();
    assert!(err.message().contains("skills add"), "{err}");
    let handoff =
        catalog::mcp_handoff(&cx, Some(&store), "catalog:tap/plugin/weather-tools", None).unwrap();
    let McpRequest::Git(request) = &handoff.request else {
        panic!("a git review");
    };
    assert_eq!(request.path.as_deref(), Some("plugins/weather-tools"));
    assert_eq!(request.git_ref.as_deref(), Some("main"));
    let mut resolved = handoff.resolve(Some(&store), &sx).unwrap();
    let keys: Vec<&str> = resolved.drafts().iter().map(|d| d.key.as_str()).collect();
    assert!(keys.contains(&".mcp.json#forecast"), "{keys:?}");
    assert!(keys.contains(&".mcp.json#notion"), "{keys:?}");
    assert!(
        !keys.contains(&".mcp.json#broken")
            && resolved
                .skipped()
                .iter()
                .any(|s| s.entry.as_deref() == Some("broken")),
        "an empty url is skipped, not drafted: {keys:?}"
    );
    let forecast = resolved
        .drafts()
        .iter()
        .find(|d| d.key == ".mcp.json#forecast")
        .unwrap()
        .clone();
    assert!(forecast.checkout && forecast.runs_repo_code);
    let args = forecast.server.body.args.clone().unwrap();
    assert!(
        args[0].starts_with("${loadout_home}/servers/") && args[0].ends_with("/server.js"),
        "{args:?}"
    );
    let pick = Pick {
        key: forecast.key.clone(),
        server: forecast.server.clone(),
        trust: None,
    };
    mcp_source::save(&store, &sx, &fx.ax(), &mut resolved, &[pick], &once()).unwrap();
    let server = store
        .server(&forecast.server.id)
        .unwrap()
        .unwrap()
        .into_value();
    let meta = server.body.meta.unwrap();
    assert_eq!(meta.origin.as_deref(), Some("catalog:tap"));
    let source = meta.source.unwrap();
    assert_eq!(source.kind, ServerSourceKind::Git);
    assert_eq!(source.catalog.as_deref(), Some("tap/plugin/weather-tools"));
    assert_eq!(source.path.as_deref(), Some("plugins/weather-tools"));

    // Servers declared inline in the marketplace, or in a file it names,
    // are what the review drafts: the entry offers nothing it cannot draft.
    for (id, server) in [("inline-only", "tides"), ("path-servers", "waves")] {
        let handoff =
            catalog::mcp_handoff(&cx, Some(&store), &format!("catalog:tap/plugin/{id}"), None)
                .unwrap();
        assert_eq!(handoff.entry.servers, [server], "{id}");
        let resolved = handoff.resolve(Some(&store), &sx).unwrap();
        let names: Vec<&str> = resolved
            .drafts()
            .iter()
            .map(|d| d.server.id.as_str())
            .collect();
        assert_eq!(names, [server], "{id}: {:?}", resolved.skipped());
    }
}

#[test]
fn a_pinned_tap_reads_its_commit() {
    let Some(git) = system_git_or_skip("a_pinned_tap_reads_its_commit") else {
        return;
    };
    let fx = Fx::new();
    let env = with_git_vars(&fx.th, &git, fx.th.env_builder(host_os())).build();
    let sx = sx(&fx, &env);
    let bare = tap_repo(&fx, &git);
    let output = fx
        .th
        .command(&git)
        .envs(git_vars(&fx.th, &git))
        .current_dir(&bare)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let commit = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    let store = store_with(&fx, &[]);
    let new = NewCatalog {
        kind: CatalogSourceKind::GitTap,
        url: bare.to_string_lossy().into_owned(),
        id: Some("pinned".to_owned()),
        git_ref: Some(commit.clone()),
        title: None,
    };
    catalog::add(&store, &new).unwrap();
    online(&fx);
    let http = http(&env);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };
    let results = catalog::search_one(&cx, Some(&store), "pinned", "", None).unwrap();
    assert_eq!(results.status, ResultStatus::Fresh, "{:?}", results.error);
    let pdf = results.entries.iter().find(|e| e.id == "pdf").unwrap();
    assert!(pdf.pinned);
    assert_eq!(pdf.commit.as_deref(), Some(commit.as_str()));
    let (request, _) =
        catalog::skill_request(&cx, Some(&store), "catalog:pinned/pdf", None, None).unwrap();
    assert_eq!(request.git_ref.as_deref(), Some(commit.as_str()));
    let mut request = request;
    request.decisions = once();
    let installed = install::install(&store, &sx, &request).unwrap();
    assert_eq!(installed.source_commit.as_deref(), Some(commit.as_str()));
    let meta = skill_store::read_meta(&store, &installed.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        meta.source.unwrap().git_ref.as_deref(),
        Some(commit.as_str()),
        "a commit ref pins the skill"
    );
}

#[test]
fn a_catalog_added_again_at_another_address_never_serves_the_old_cache() {
    let fx = Fx::new();
    let old = Stub::start(Duration::ZERO);
    old.route(
        "/v0.1/servers",
        200,
        registry_fixture("search-weather.json"),
    );
    let store = store_with(&fx, &[("acme", CatalogSourceKind::McpRegistry, &old.url)]);
    online(&fx);
    let env = env_at(&fx, 0);
    let sx = sx(&fx, &env);
    let http = http(&env);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };
    let found = catalog::search_one(&cx, Some(&store), "acme", "weather", None).unwrap();
    let id = found.entries[0].id.clone();
    assert!(catalog::cached_entry(&sx, Some(&store), "acme", &id).is_ok());

    let other = Stub::start(Duration::ZERO);
    catalog::remove(&store, &sx, "acme").unwrap();
    catalog::add(
        &store,
        &NewCatalog {
            kind: CatalogSourceKind::McpRegistry,
            url: other.url.clone(),
            id: Some("acme".to_owned()),
            git_ref: None,
            title: None,
        },
    )
    .unwrap();
    assert!(
        catalog::cached_entry(&sx, Some(&store), "acme", &id).is_err(),
        "the old registry's pages are not this catalog's"
    );
}

#[test]
fn old_cache_files_and_left_over_clones_are_pruned() {
    let fx = Fx::new();
    let stub = Stub::start(Duration::ZERO);
    stub.route(
        "/v0.1/servers",
        200,
        registry_fixture("search-weather.json"),
    );
    let store = store_with(&fx, &[("acme", CatalogSourceKind::McpRegistry, &stub.url)]);
    online(&fx);
    let root = fx.home.cache_dir().join("catalogs");
    // An interrupted index, and a page nobody asked for in weeks.
    let clone = root.join(".tmp-00000000000000000000000000");
    std::fs::create_dir_all(clone.join("skills")).unwrap();
    std::fs::create_dir_all(root.join("acme")).unwrap();
    let old_page = root.join("acme").join("search-0000000000000000.json");
    std::fs::write(&old_page, r#"{"fetched_at":0,"key":"x","data":[[],null]}"#).unwrap();

    let env = env_at(&fx, 0);
    let sx = sx(&fx, &env);
    let http = http(&env);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };
    catalog::search_one(&cx, Some(&store), "acme", "weather", None).unwrap();
    assert!(!clone.exists(), "a left-over clone is removed");
    assert!(!old_page.exists(), "a page weeks old is removed");
    let kept: Vec<_> = std::fs::read_dir(root.join("acme")).unwrap().collect();
    assert_eq!(kept.len(), 1, "the new page stays");
}

// ------------------------------------------------------ git while off

/// A runner that records each process it is asked to start and starts
/// none.
#[derive(Default)]
struct SpyRunner(Mutex<Vec<String>>);

impl Runner for SpyRunner {
    fn run(
        &self,
        invocation: &Invocation,
        _progress: &mut dyn FnMut(&str),
    ) -> std::io::Result<RunOutput> {
        self.0
            .lock()
            .unwrap()
            .push(format!("{:?} {:?}", invocation.program, invocation.args));
        Err(std::io::Error::other("no process may run"))
    }
}

#[test]
fn with_online_catalogs_off_git_never_runs_for_taps() {
    let fx = Fx::new();
    // The built-in taps stay on, with a user-added remote tap.
    let store = fx.store();
    store_add(
        &store,
        "tap",
        CatalogSourceKind::GitTap,
        "https://git.example.com/acme/skills.git",
    );
    let env = env_at(&fx, 0);
    let spy = SpyRunner::default();
    let sx = SkillsEnv {
        runner: &spy,
        ..sx(&fx, &env)
    };
    let http = http(&env);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };

    for results in catalog::search(&cx, Some(&store), "pdf", None).unwrap() {
        assert!(
            matches!(results.status, ResultStatus::Off | ResultStatus::Link),
            "{}: {:?}",
            results.catalog,
            results.status
        );
    }
    let off = |err: agent_loadout_core::CoreError| {
        assert_eq!(err.code(), ErrorCode::CatalogsOff, "{err}");
    };
    off(catalog::show(&cx, Some(&store), "tap", "pdf").unwrap_err());
    off(catalog::refresh(&cx, Some(&store), None).unwrap_err());
    for text in ["catalog:tap/pdf", "catalog:anthropic-skills/pdf"] {
        off(catalog::skill_request(&cx, Some(&store), text, None, None).unwrap_err());
    }
    off(catalog::mcp_handoff(&cx, Some(&store), "catalog:tap/plugin/x", None).unwrap_err());
    assert_eq!(
        *spy.0.lock().unwrap(),
        Vec::<String>::new(),
        "online catalogs off: git never runs for a tap"
    );
}

// ------------------------------------------------------- tap URL tokens

#[test]
fn a_tap_url_with_a_token_is_refused_and_never_stored() {
    let fx = Fx::new();
    let store = store_with(&fx, &[]);
    for url in [
        "https://oauth2:glpat-SECRET123@gitlab.com/acme/skills.git",
        "https://ghp_SECRET123@github.com/acme/skills",
        "ssh://git:SECRET123@git.example.com/acme/skills.git",
    ] {
        let new = NewCatalog {
            kind: CatalogSourceKind::GitTap,
            url: url.to_owned(),
            id: Some("tokened".to_owned()),
            git_ref: None,
            title: None,
        };
        let err = catalog::add(&store, &new).unwrap_err();
        assert!(
            err.message().contains("git credential helper"),
            "{url}: {err}"
        );
        assert!(!err.message().contains("SECRET123"), "{err}");
    }
    let file = store
        .read_file("catalogs.toml")
        .unwrap()
        .unwrap_or_default();
    assert!(!file.contains("SECRET123"), "{file}");
    // An ssh user is not a secret.
    store_add(
        &store,
        "ssh",
        CatalogSourceKind::GitTap,
        "git@github.com:acme/skills.git",
    );
}

// ------------------------------------------------- remote commit pins

/// Smart HTTP on 127.0.0.1 through `git http-backend` (CGI): a remote
/// for the shallow clone and fetch paths. Counts requests.
struct GitHttp {
    url: String,
    hits: Arc<AtomicUsize>,
}

impl GitHttp {
    fn start(fx: &Fx, git: &Path, root: &Path) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let base = fx.th.command(git);
        let mut env: Vec<(std::ffi::OsString, std::ffi::OsString)> = base
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_owned(), v?.to_owned())))
            .collect();
        env.extend(
            git_vars(&fx.th, git)
                .into_iter()
                .map(|(k, v)| (k.into(), v)),
        );
        if let Some(root) = std::env::var_os("SYSTEMROOT") {
            env.push(("SYSTEMROOT".into(), root));
        }
        env.push(("GIT_PROJECT_ROOT".into(), root.into()));
        env.push(("GIT_HTTP_EXPORT_ALL".into(), "1".into()));
        let (git, cwd, thread_hits) = (git.to_path_buf(), root.to_path_buf(), hits.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                thread_hits.fetch_add(1, Ordering::SeqCst);
                let (git, cwd, env) = (git.clone(), cwd.clone(), env.clone());
                std::thread::spawn(move || {
                    let _ = serve_cgi(&mut stream, &git, &cwd, &env);
                });
            }
        });
        Self { url, hits }
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

fn serve_cgi(
    stream: &mut std::net::TcpStream,
    git: &Path,
    cwd: &Path,
    env: &[(std::ffi::OsString, std::ffi::OsString)],
) -> std::io::Result<()> {
    let mut request = Vec::new();
    let mut buf = [0u8; 8192];
    let head_end = loop {
        if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        match stream.read(&mut buf)? {
            0 => return Ok(()),
            n => request.extend_from_slice(&buf[..n]),
        }
    };
    let head = String::from_utf8_lossy(&request[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or_default().split(' ');
    let method = first.next().unwrap_or("GET").to_owned();
    let target = first.next().unwrap_or("/").to_owned();
    let header = |name: &str| {
        head.lines().skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case(name)
                .then(|| value.trim().to_owned())
        })
    };
    let length: usize = header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = request[head_end..].to_vec();
    while body.len() < length {
        match stream.read(&mut buf)? {
            0 => break,
            n => body.extend_from_slice(&buf[..n]),
        }
    }
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let mut command = std::process::Command::new(git);
    command
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(cwd)
        .arg("http-backend")
        .env("REQUEST_METHOD", &method)
        .env("PATH_INFO", path)
        .env("QUERY_STRING", query)
        .env("CONTENT_LENGTH", body.len().to_string())
        .env("REMOTE_ADDR", "127.0.0.1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    for (cgi, name) in [
        ("CONTENT_TYPE", "Content-Type"),
        ("GIT_PROTOCOL", "Git-Protocol"),
        ("HTTP_CONTENT_ENCODING", "Content-Encoding"),
    ] {
        if let Some(value) = header(name) {
            command.env(cgi, value);
        }
    }
    let mut child = command.spawn()?;
    child.stdin.take().unwrap().write_all(&body)?;
    let output = child.wait_with_output()?;
    let out = output.stdout;
    let split = out
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|at| (at, 4))
        .or_else(|| out.windows(2).position(|w| w == b"\n\n").map(|at| (at, 2)))
        .unwrap_or((out.len(), 0));
    let cgi_head = String::from_utf8_lossy(&out[..split.0]).into_owned();
    let cgi_body = &out[(split.0 + split.1).min(out.len())..];
    let mut status = "200 OK".to_owned();
    let mut response = String::new();
    for line in cgi_head.lines().filter(|l| !l.is_empty()) {
        match line.split_once(':') {
            Some((key, value)) if key.eq_ignore_ascii_case("Status") => {
                status = value.trim().to_owned();
            }
            _ => {
                response.push_str(line);
                response.push_str("\r\n");
            }
        }
    }
    let head = format!(
        "HTTP/1.1 {status}\r\n{response}Content-Length: {}\r\nConnection: close\r\n\r\n",
        cgi_body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(cgi_body)
}

#[test]
fn a_remote_tap_pinned_behind_its_tip_fetches_the_full_commit() {
    let Some(git) = system_git_or_skip("a_remote_tap_pinned_behind_its_tip") else {
        return;
    };
    let fx = Fx::new();
    let env = with_git_vars(&fx.th, &git, fx.th.env_builder(host_os())).build();
    let sx = sx(&fx, &env);
    let bare = tap_repo(&fx, &git);
    let rev = |dir: &Path| {
        let output = fx
            .th
            .command(&git)
            .envs(git_vars(&fx.th, &git))
            .current_dir(dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    let pinned = rev(&bare);
    // A newer tip: a depth-1 clone does not hold the pin.
    let work = fx.th.root().join("skills-work");
    git_run(
        &fx,
        &git,
        &work,
        &["commit", "--quiet", "--allow-empty", "-m", "next"],
    );
    git_run(
        &fx,
        &git,
        &work,
        &["push", "--quiet", "origin", "HEAD:main"],
    );
    assert_ne!(rev(&bare), pinned);
    git_run(
        &fx,
        &git,
        &bare,
        &["config", "uploadpack.allowReachableSHA1InWant", "true"],
    );
    let server = GitHttp::start(&fx, &git, fx.th.root());
    let url = format!("{}/skills.git", server.url);
    let store = store_with(&fx, &[]);

    // A short commit is refused with the way to the full one.
    let mut new = NewCatalog {
        kind: CatalogSourceKind::GitTap,
        url: url.clone(),
        id: Some("remote".to_owned()),
        git_ref: Some(pinned[..7].to_owned()),
        title: None,
    };
    let err = catalog::add(&store, &new).unwrap_err();
    assert!(err.message().contains("full 40-character commit"), "{err}");
    new.git_ref = Some(pinned.clone());
    catalog::add(&store, &new).unwrap();

    online(&fx);
    let http = http(&env);
    let cx = CatalogEnv {
        sx: &sx,
        http: &http,
    };
    let results = catalog::search_one(&cx, Some(&store), "remote", "", None).unwrap();
    assert_eq!(results.status, ResultStatus::Fresh, "{:?}", results.error);
    let pdf = results.entries.iter().find(|e| e.id == "pdf").unwrap();
    assert!(pdf.pinned);
    assert_eq!(pdf.commit.as_deref(), Some(pinned.as_str()));
    assert!(server.hits() > 0, "the tap was read over HTTP");
}

//! The SPEC 11.7 classifier and the SPEC 11.8 leak scanner in
//! `Store::transaction`.
//!
//! Token-shaped values are assembled at runtime from fragments (SPEC 20.1
//! rule 2), so this file never holds a string in a real token format.

use std::collections::BTreeMap;
use std::path::Path;

use agent_loadout_core::model::server::{Server, Transport};
use agent_loadout_core::paths::{LoadoutHome, loadout_home};
use agent_loadout_core::secrets::classify::{
    Reason, TokenKind, classify_args_import, classify_args_stored, classify_import,
    classify_stored, entropy, is_sensitive_argument_name,
};
use agent_loadout_core::secrets::grammar::SecretRef;
use agent_loadout_core::secrets::providers::Resolver;
use agent_loadout_core::secrets::providers::onepassword::OpCli;
use agent_loadout_core::secrets::scanner::{
    ALLOWLIST_FILE, Allowlist, LeakKind, SecretMemory, allow_hash,
};
use agent_loadout_core::store::{Operation, Store, Transaction};
use agent_loadout_core::{CoreError, Env, ErrorCode, Os};
use agent_loadout_testkit::{MemKeyring, RootedFs, TempHome};
use git2::{ObjectType, Repository, Signature, Time};

const CLIENT: &str = "loadout-test/0.0.0";
/// Filler for fake tokens: token characters, obviously not a real token.
const FILLER: &str = "FAKEfake0123456789FAKEfake0123456789FAKEfake";

/// `prefix` (given in fragments) followed by `len` filler characters.
fn fake(prefix: &[&str], len: usize) -> String {
    format!("{}{}", prefix.concat(), &FILLER[..len])
}

fn fake_github() -> String {
    fake(&["gh", "p_"], 36)
}

fn fake_jwt() -> String {
    // `{"alg":"none"}` and `{"sub":"fake"}`, base64url, plus a fake signature.
    [
        "ey",
        "JhbGciOiJub25lIn0",
        ".",
        "ey",
        "JzdWIiOiJmYWtlIn0",
        ".",
        "FAKEsignature0123",
    ]
    .concat()
}

fn fake_private_key() -> String {
    [
        "-----BEGIN ",
        "PRIVATE KEY-----\nFAKEFAKEFAKE\n-----END ",
        "PRIVATE KEY-----",
    ]
    .concat()
}

/// High entropy, not a token format.
const RANDOM: &str = "Xk9fQ2mZ7pL4vB8nR1tY6wC3hJ5dG0sA";

/// `(key, value, import result, stored result)`.
type Row<'a> = (Option<&'a str>, String, Option<Reason>, Option<Reason>);
/// A staged change that should be refused.
type Attempt<'a> = (
    &'a str,
    Box<dyn FnOnce(&mut Transaction<'_>) -> Result<(), CoreError> + 'a>,
);

#[test]
fn classifier_table() {
    use Reason::{HighEntropy, SensitiveKey, TokenFormat, UrlTokenParameter, UrlUserinfo};
    let github = fake_github();
    let rows: Vec<Row<'_>> = vec![
        (Some("LOG_LEVEL"), "info".into(), None, None),
        (Some("GITHUB_TOOLSETS"), "repos,issues".into(), None, None),
        (
            Some("GITHUB_TOKEN"),
            "short".into(),
            Some(SensitiveKey),
            None,
        ),
        (Some("AUTH_MODE"), "oauth".into(), Some(SensitiveKey), None),
        (
            Some("db_passwd"),
            "hunter2".into(),
            Some(SensitiveKey),
            None,
        ),
        (Some("X-Api-Key"), "abc".into(), Some(SensitiveKey), None),
        (
            Some("PRIVATE_KEY_ID"),
            "abc".into(),
            Some(SensitiveKey),
            None,
        ),
        (
            Some("CLIENT_CREDENTIALS"),
            "abc".into(),
            Some(SensitiveKey),
            None,
        ),
        (
            Some("API_KEY"),
            RANDOM.into(),
            Some(SensitiveKey),
            Some(HighEntropy),
        ),
        (
            Some("MY_TOKEN"),
            RANDOM[..19].into(),
            Some(SensitiveKey),
            None,
        ),
        (Some("DESCRIPTION"), RANDOM.into(), None, None),
        (Some("TOKEN"), "".into(), None, None),
        (Some("PASSWORD"), "${secret:DB_PASSWORD}".into(), None, None),
        (
            Some("Authorization"),
            "Bearer ${secret:LINEAR_TOKEN}".into(),
            None,
            None,
        ),
        (
            Some("Authorization"),
            format!("Bearer {github}"),
            Some(TokenFormat(TokenKind::GithubClassic)),
            Some(TokenFormat(TokenKind::GithubClassic)),
        ),
        (
            Some("AUTH_URL"),
            "http://localhost:8080/oauth/callback".into(),
            Some(SensitiveKey),
            None,
        ),
        (
            Some("SECRET_FILE"),
            r"C:\keys\fake\secret-file-0123.pem".into(),
            Some(SensitiveKey),
            None,
        ),
        // Token formats count under any key, or none.
        (
            None,
            github.clone(),
            Some(TokenFormat(TokenKind::GithubClassic)),
            Some(TokenFormat(TokenKind::GithubClassic)),
        ),
        (
            None,
            fake(&["github", "_pat_"], 40),
            Some(TokenFormat(TokenKind::GithubFineGrained)),
            Some(TokenFormat(TokenKind::GithubFineGrained)),
        ),
        (
            None,
            fake(&["gh", "o_"], 36),
            Some(TokenFormat(TokenKind::GithubOauth)),
            Some(TokenFormat(TokenKind::GithubOauth)),
        ),
        (
            Some("OPENAI"),
            fake(&["s", "k-"], 40),
            Some(TokenFormat(TokenKind::OpenAi)),
            Some(TokenFormat(TokenKind::OpenAi)),
        ),
        (
            None,
            fake(&["s", "k-ant-"], 40),
            Some(TokenFormat(TokenKind::Anthropic)),
            Some(TokenFormat(TokenKind::Anthropic)),
        ),
        (
            None,
            fake(&["xo", "xb-"], 24),
            Some(TokenFormat(TokenKind::Slack)),
            Some(TokenFormat(TokenKind::Slack)),
        ),
        (
            None,
            fake(&["xo", "xp-"], 24),
            Some(TokenFormat(TokenKind::Slack)),
            Some(TokenFormat(TokenKind::Slack)),
        ),
        (
            None,
            ["AK", "IA", "FAKE000000000000"].concat(),
            Some(TokenFormat(TokenKind::AwsAccessKey)),
            Some(TokenFormat(TokenKind::AwsAccessKey)),
        ),
        (
            None,
            fake(&["AI", "za"], 35),
            Some(TokenFormat(TokenKind::GoogleApiKey)),
            Some(TokenFormat(TokenKind::GoogleApiKey)),
        ),
        (
            None,
            fake(&["gl", "pat-"], 20),
            Some(TokenFormat(TokenKind::GitlabToken)),
            Some(TokenFormat(TokenKind::GitlabToken)),
        ),
        (
            None,
            fake_private_key(),
            Some(TokenFormat(TokenKind::PrivateKey)),
            Some(TokenFormat(TokenKind::PrivateKey)),
        ),
        (
            None,
            fake_jwt(),
            Some(TokenFormat(TokenKind::Jwt)),
            Some(TokenFormat(TokenKind::Jwt)),
        ),
        // Look-alikes that are not tokens.
        (None, "sk-learn".into(), None, None),
        (
            None,
            "task-runner-FAKEfake0123456789FAKE".into(),
            None,
            None,
        ),
        (None, ["AK", "IA", "FAKE00000000000"].concat(), None, None),
        (
            None,
            "@modelcontextprotocol/server-github".into(),
            None,
            None,
        ),
        // URLs.
        (
            None,
            "https://user:fake-password@example.com/mcp".into(),
            Some(UrlUserinfo),
            Some(UrlUserinfo),
        ),
        (
            None,
            "https://fake-token-as-user@example.com/mcp".into(),
            Some(UrlUserinfo),
            Some(UrlUserinfo),
        ),
        (
            None,
            "https://${env:MCP_USER}:${secret:MCP_PASSWORD}@example.com/mcp".into(),
            None,
            None,
        ),
        (
            None,
            "https://example.com/mcp?access_token=fake123".into(),
            Some(UrlTokenParameter),
            Some(UrlTokenParameter),
        ),
        (
            None,
            "https://example.com/mcp?key=fake123&page=2".into(),
            Some(UrlTokenParameter),
            Some(UrlTokenParameter),
        ),
        (
            None,
            "https://example.com/mcp?token=${secret:MCP_TOKEN}".into(),
            None,
            None,
        ),
        (None, "https://example.com/mcp?page=2".into(), None, None),
    ];
    for (key, value, import, stored) in rows {
        assert_eq!(
            classify_import(key, &value),
            import,
            "import: {key:?} = {value:?}"
        );
        assert_eq!(
            classify_stored(key, &value),
            stored,
            "stored: {key:?} = {value:?}"
        );
    }
}

#[test]
fn arguments_are_classified_with_their_flag() {
    let args = |list: &[&str]| list.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    let flagged =
        |results: Vec<Option<Reason>>| results.iter().map(Option::is_some).collect::<Vec<bool>>();

    let plain = args(&[
        "-y",
        "@example/server",
        "--api-key",
        "fake-value",
        "--port",
        "8080",
    ]);
    assert_eq!(
        flagged(classify_args_import(&plain)),
        [false, false, false, true, false, false]
    );
    assert_eq!(flagged(classify_args_stored(&plain)), [false; 6]);

    let inline = args(&["--token=fake-value", "--verbose"]);
    assert_eq!(
        classify_args_import(&inline),
        [Some(Reason::SensitiveKey), None]
    );

    let random = args(&["--access-token", RANDOM]);
    assert_eq!(
        classify_args_stored(&random),
        [None, Some(Reason::HighEntropy)]
    );

    let referenced = args(&["--api-key", "${secret:EXAMPLE_KEY}", "--no-auth", "--port"]);
    assert_eq!(flagged(classify_args_import(&referenced)), [false; 4]);

    let github = fake_github();
    let positional = args(&["serve", &github]);
    assert_eq!(
        classify_args_stored(&positional)[1],
        Some(Reason::TokenFormat(TokenKind::GithubClassic))
    );

    // SkillDock's SENSITIVE_ARGUMENT_NAMES rule is included.
    for name in [
        "--api-key",
        "--github-token",
        "--auth_token",
        "--authorization=x",
    ] {
        assert!(is_sensitive_argument_name(name), "{name}");
    }
    for name in ["--port", "--tokenizer-path", "-y"] {
        assert!(!is_sensitive_argument_name(name), "{name}");
    }
}

#[test]
fn entropy_is_bits_per_character() {
    assert_eq!(entropy(""), 0.0);
    assert_eq!(entropy("aaaa"), 0.0);
    assert_eq!(entropy("abab"), 1.0);
    assert_eq!(entropy("abcd"), 2.0);
    assert!(entropy(RANDOM) > 3.5);
}

#[test]
fn memory_matches_recorded_values_exactly() {
    let memory = SecretMemory::new().unwrap();
    assert!(memory.is_empty());
    memory.record("short");
    assert!(memory.is_empty(), "values under 8 bytes are not tracked");
    memory.record("  planted-FAKE-value-0042\n");
    assert_eq!(memory.len(), 2, "the value and its trimmed form");
    let text = b"env = { X = \"planted-FAKE-value-0042\" }";
    let found = memory.find(text).unwrap();
    assert_eq!(&text[found], b"planted-FAKE-value-0042");
    assert_eq!(memory.find(b"planted-FAKE-value-0043"), None);
    assert!(!format!("{memory:?}").contains("planted"));
}

// The scanner inside Store::transaction.

fn host_os() -> Os {
    if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    }
}

struct Fx {
    th: TempHome,
    env: Env,
    fs: RootedFs,
    home: LoadoutHome,
}

impl Fx {
    fn new() -> Self {
        let th = TempHome::new();
        let env = th.env_builder(host_os()).hostname("test-desk").build();
        let fs = th.fs();
        let home = loadout_home(&env, &fs, Some(&th.root().join("lh"))).unwrap();
        Self { th, env, fs, home }
    }

    fn store(&self) -> Store<'_> {
        Store::open_or_init(&self.env, &self.fs, &self.home, CLIENT).unwrap()
    }

    /// Every object in the store's object database, decompressed.
    fn objects(&self) -> Vec<Vec<u8>> {
        let repo = Repository::open(self.home.store_dir()).unwrap();
        let odb = repo.odb().unwrap();
        let mut ids = Vec::new();
        odb.foreach(|id| {
            ids.push(*id);
            true
        })
        .unwrap();
        assert!(!ids.is_empty());
        ids.iter()
            .map(|id| {
                let object = odb.read(*id).unwrap();
                assert!(matches!(
                    object.kind(),
                    ObjectType::Blob | ObjectType::Tree | ObjectType::Commit | ObjectType::Tag
                ));
                object.data().to_vec()
            })
            .collect()
    }

    /// Every file under `dir`, recursively.
    fn files(dir: &Path) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                out.extend(Self::files(&path));
            } else {
                out.push(std::fs::read(&path).unwrap());
            }
        }
        out
    }
}

fn server(id: &str, env: &[(&str, &str)]) -> Server {
    let mut server = Server::new(id);
    server.body.transport = Some(Transport::Stdio);
    server.body.command = Some("npx".to_owned());
    server.body.args = Some(vec!["-y".to_owned(), "@example/fake-mcp".to_owned()]);
    if !env.is_empty() {
        server.body.env = Some(
            env.iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect::<BTreeMap<_, _>>(),
        );
    }
    server
}

fn try_commit(
    store: &Store<'_>,
    summary: &str,
    f: impl FnOnce(&mut Transaction<'_>) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    store
        .transaction(Operation::new("test.edit", summary), f)
        .map(|_| ())
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn a_planted_secret_never_reaches_a_git_object() {
    let fx = Fx::new();
    let store = fx.store();
    let planted = "planted-FAKE-value-0042";
    let keyring = MemKeyring::with("loadout", "PLANTED", planted);
    let op = OpCli::NotFound;
    let resolver = Resolver::new(&fx.env, &keyring, &op, store.secret_memory().clone());
    let reference = SecretRef::parse("keyring://loadout/PLANTED").unwrap();
    assert_eq!(resolver.resolve(&reference).unwrap().expose(), planted);
    let head = store.head().unwrap();

    let attempts: Vec<Attempt<'_>> = vec![
        (
            "server: add leaky",
            Box::new(|tx| tx.add_server(server("leaky", &[("SETTING", planted)]))),
        ),
        (
            "server: add described",
            Box::new(|tx| {
                let mut described = server("described", &[]);
                described.body.description = Some(format!("uses {planted} for login"));
                tx.add_server(described)
            }),
        ),
        (
            "server: add with an argument",
            Box::new(|tx| {
                let mut argued = server("argued", &[]);
                argued.body.args = Some(vec!["--flag".to_owned(), planted.to_owned()]);
                tx.add_server(argued)
            }),
        ),
        (
            "skill: add notes",
            Box::new(|tx| {
                tx.write(
                    "skills/notes/SKILL.md",
                    format!("# Notes\n\nkey: {planted}\n"),
                )
            }),
        ),
        (
            // Resolved values are never allowlisted.
            "server: add allowlisted",
            Box::new(|tx| {
                tx.write(
                    ALLOWLIST_FILE,
                    Allowlist::append(None, &allow_hash(planted)),
                )?;
                tx.add_server(server("allowlisted", &[("SETTING", planted)]))
            }),
        ),
    ];
    for (summary, attempt) in attempts {
        let err = try_commit(&store, summary, attempt).unwrap_err();
        assert_eq!(err.code(), ErrorCode::LeakDetected, "{summary}: {err}");
        assert!(!err.message().contains(planted), "{summary}: {err}");
        assert!(
            err.message()
                .contains("holds a secret value resolved by Loadout"),
            "{err}"
        );
    }
    let err = try_commit(&store, &format!("server: rotate {planted}"), |tx| {
        tx.add_server(server("rotated", &[]))
    })
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::LeakDetected);
    assert!(err.message().contains("commit message"), "{err}");

    // A reference is what the store holds instead.
    try_commit(&store, "server: add referenced", |tx| {
        tx.add_server(server("referenced", &[("SETTING", "${secret:PLANTED}")]))
    })
    .unwrap();
    assert_ne!(store.head().unwrap(), head);

    for object in fx.objects() {
        assert!(
            !contains(&object, planted.as_bytes()),
            "planted value in a git object"
        );
    }
    for file in Fx::files(&fx.home.path) {
        assert!(
            !contains(&file, planted.as_bytes()),
            "planted value in a file under LOADOUT_HOME"
        );
    }
    assert!(!fx.th.root().join("lh/store/servers/leaky.toml").exists());
}

#[test]
fn classifier_hits_block_commits_until_marked_not_secret() {
    let fx = Fx::new();
    let store = fx.store();
    let github = fake_github();
    let head = store.head().unwrap();

    let err = try_commit(&store, "server: add tokened", |tx| {
        tx.add_server(server("tokened", &[("UPSTREAM", &github)]))
    })
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::LeakDetected);
    assert!(
        err.message()
            .contains("servers/tokened.toml line 8: looks like a GitHub personal access token"),
        "{err}"
    );
    assert!(!err.message().contains(&github), "{err}");

    // Key-aware checks in TOML, raw checks anywhere (a comment, a skill).
    let entropic = format!(
        "schema = 1\nid = \"entropic\"\ntransport = \"stdio\"\ncommand = \"npx\"\n\n[env]\nACCESS_TOKEN = \"{RANDOM}\"\n"
    );
    let err = try_commit(&store, "server: add entropic", |tx| {
        tx.write("servers/entropic.toml", entropic.clone())
    })
    .unwrap_err();
    assert!(
        err.message()
            .contains("servers/entropic.toml line 7 (env.ACCESS_TOKEN)"),
        "{err}"
    );
    let jwt = fake_jwt();
    let commented = format!(
        "schema = 1\nid = \"commented\"\ntransport = \"stdio\"\ncommand = \"npx\"\n# old: {jwt}\n"
    );
    let err = try_commit(&store, "server: add commented", |tx| {
        tx.write("servers/commented.toml", commented.clone())
    })
    .unwrap_err();
    assert!(err.message().contains("JSON Web Token"), "{err}");
    let err = try_commit(&store, "skill: add keys", |tx| {
        tx.write("skills/keys/SKILL.md", fake_private_key())
    })
    .unwrap_err();
    assert!(
        err.message().contains("skills/keys/SKILL.md line 1"),
        "{err}"
    );
    assert_eq!(store.head().unwrap(), head, "nothing was committed");
    for object in fx.objects() {
        assert!(!contains(&object, github.as_bytes()));
        assert!(!contains(&object, RANDOM.as_bytes()));
    }

    // "Mark as not secret" records a hash, never the text.
    let hash = allow_hash(&github);
    try_commit(&store, "server: add tokened", |tx| {
        tx.write(ALLOWLIST_FILE, Allowlist::append(None, &hash))?;
        tx.add_server(server("tokened", &[("UPSTREAM", &github)]))
    })
    .unwrap();
    let allow = store.read_to_string(ALLOWLIST_FILE).unwrap().unwrap();
    assert!(allow.contains(&hash) && !allow.contains(&github), "{allow}");
    // Later commits read the committed allowlist.
    try_commit(&store, "server: add entropic", |tx| {
        let text = tx.read_to_string(ALLOWLIST_FILE)?;
        tx.write(
            ALLOWLIST_FILE,
            Allowlist::append(text.as_deref(), &allow_hash(RANDOM)),
        )?;
        tx.write("servers/entropic.toml", entropic.clone())
    })
    .unwrap();
}

#[test]
fn ordinary_store_content_commits() {
    let fx = Fx::new();
    let store = fx.store();
    let mut remote = Server::new("remote");
    remote.body.transport = Some(Transport::Http);
    remote.body.url = Some("https://mcp.example.com/mcp?token=${secret:REMOTE_TOKEN}".to_owned());
    remote.body.headers = Some(BTreeMap::from([(
        "Authorization".to_owned(),
        "Bearer ${secret:REMOTE_TOKEN}".to_owned(),
    )]));
    try_commit(&store, "server: add two", |tx| {
        tx.add_server(server(
            "local",
            &[
                ("AUTH_MODE", "oauth"),
                ("LOG_LEVEL", "info"),
                ("GITHUB_TOKEN", "${secret:GITHUB_PAT}"),
                ("TOKEN_TTL_SECONDS", "3600"),
            ],
        ))?;
        tx.add_server(remote)?;
        tx.write(
            "secrets.toml",
            "schema = 1\n\n[secrets.GITHUB_PAT]\nref = \"op://Example Vault/Fake Item/credential\"\n\n[secrets.REMOTE_TOKEN]\nref = \"keyring://loadout/REMOTE_TOKEN\"\n",
        )
    })
    .unwrap();
}

/// `loadout doctor --scan-history`: a commit made by another git client
/// (a store imported from elsewhere) never passed the scanner; the history
/// scan finds it and leaves history alone.
#[test]
fn history_scan_reports_leaks_committed_outside_loadout() {
    let fx = Fx::new();
    let store = fx.store();
    try_commit(&store, "server: add clean", |tx| {
        tx.add_server(server("clean", &[("LOG_LEVEL", "info")]))
    })
    .unwrap();
    assert_eq!(store.scan_history().unwrap(), []);

    let repo = Repository::open(fx.home.store_dir()).unwrap();
    let github = fake_github();
    let blob = repo
        .blob(format!("schema = 1\n# {github}\n").as_bytes())
        .unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    let mut builder = repo.treebuilder(Some(&head.tree().unwrap())).unwrap();
    builder.insert("notes.toml", blob, 0o100_644).unwrap();
    let tree = repo.find_tree(builder.write().unwrap()).unwrap();
    let signature = Signature::new("someone", "someone@example.invalid", &Time::new(0, 0)).unwrap();
    let leaked = repo
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "notes: add",
            &tree,
            &[&head],
        )
        .unwrap();

    let findings = store.scan_history().unwrap();
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].commit.as_str(), leaked.to_string());
    assert_eq!(findings[0].finding.file, "notes.toml");
    assert_eq!(findings[0].finding.line, Some(2));
    assert_eq!(
        findings[0].finding.kind,
        LeakKind::Classified(Reason::TokenFormat(TokenKind::GithubClassic))
    );
    assert_eq!(repo.head().unwrap().target(), Some(leaked), "reports only");
}

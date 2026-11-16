//! Providers (SPEC 11.2, 11.3, 11.11) and the catalog with its machine
//! overlay (SPEC 5.3, 5.6). 1Password is the `op-stub` binary on a temp
//! `PATH`; the keyring is the in-memory backend. Nothing here touches the
//! real keyring or a real `op`.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use agent_loadout_core::paths::{LoadoutHome, loadout_home};
use agent_loadout_core::secrets::catalog::{self, AliasUpdate, Catalog, RefSource, SetScope};
use agent_loadout_core::secrets::grammar::SecretRef;
use agent_loadout_core::secrets::providers::onepassword::{
    OP_NOT_FOUND, OpCli, OpFeature, OpVersion,
};
use agent_loadout_core::secrets::providers::{ProviderState, Resolver};
use agent_loadout_core::secrets::scanner::SecretMemory;
use agent_loadout_core::secrets::status::{Resolution, SecretStatus};
use agent_loadout_core::store::{Operation, Store};
use agent_loadout_core::{Env, Os};
use agent_loadout_testkit::op_stub::{OpStub, OpStubMode};
use agent_loadout_testkit::{MemKeyring, MemKeyringMode, RootedFs, TempHome};

const CLIENT: &str = "loadout-test/0.0.0";
const ITEM: &str = "op://Example Vault/Fake Item/credential";
const ITEM_VALUE: &str = "fake-item-value-0001";
const ENVIRONMENT: &str = "fakeenv0000";

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
    keyring: MemKeyring,
    memory: Arc<SecretMemory>,
}

impl Fx {
    /// `PATH` is the temp root's `bin/` only.
    fn new() -> Self {
        let th = TempHome::new();
        let env = th
            .env_builder(host_os())
            .hostname("test-desk")
            .var("PATH", th.bin())
            .var("FAKE_ENV_SECRET", "fake-env-value-0001")
            .build();
        let fs = th.fs();
        let home = loadout_home(&env, &fs, Some(&th.root().join("lh"))).unwrap();
        Self {
            th,
            env,
            fs,
            home,
            keyring: MemKeyring::new(),
            memory: Arc::new(SecretMemory::new().unwrap()),
        }
    }

    fn op_stub(&self) -> OpStub {
        let mut stub = OpStub::install(&self.th.bin());
        stub.add_item(ITEM, ITEM_VALUE).add_environment_variable(
            ENVIRONMENT,
            "LINEAR_TOKEN",
            "fake-env-var-value-0001",
        );
        stub
    }

    fn op(&self) -> OpCli {
        OpCli::detect(&self.env, &self.fs)
    }

    fn check(&self, op: &OpCli, reference: &str) -> Resolution {
        Resolver::new(&self.env, &self.keyring, op, self.memory.clone())
            .check(&SecretRef::parse(reference).unwrap())
    }

    fn store(&self) -> Store<'_> {
        Store::open_or_init(&self.env, &self.fs, &self.home, CLIENT).unwrap()
    }
}

fn op_env(name: &str) -> String {
    format!("op-env://{ENVIRONMENT}/{name}")
}

#[test]
fn op_not_installed_is_provider_unavailable() {
    let fx = Fx::new();
    let op = fx.op();
    assert_eq!(op, OpCli::NotFound);
    for reference in [ITEM.to_owned(), op_env("LINEAR_TOKEN")] {
        let resolution = fx.check(&op, &reference);
        assert_eq!(resolution.status, SecretStatus::ProviderUnavailable);
        assert_eq!(resolution.message.as_deref(), Some(OP_NOT_FOUND));
    }
    let info = op.info();
    assert_eq!(info.state, ProviderState::Unavailable);
    assert_eq!(info.detail, "1Password CLI not found");
    assert!(info.features.iter().all(|(_, available)| !available));
    // Env and keyring references work normally without `op`.
    fx.keyring
        .insert("loadout", "FAKE", "fake-keyring-value-01");
    assert_eq!(
        fx.check(&op, "keyring://loadout/FAKE"),
        Resolution::resolved()
    );
    assert_eq!(fx.check(&op, "env:FAKE_ENV_SECRET"), Resolution::resolved());
}

#[test]
fn op_signed_in_reads_items_without_a_newline() {
    let fx = Fx::new();
    let stub = fx.op_stub();
    let op = fx.op();
    assert_eq!(op.version(), Some(OpVersion::new(2, 42, 0)));
    assert_eq!(
        op.path(),
        Some(
            fx.th
                .bin()
                .join(format!("op{}", std::env::consts::EXE_SUFFIX))
                .as_path()
        )
    );
    let resolver = Resolver::new(&fx.env, &fx.keyring, &op, fx.memory.clone());
    let value = resolver.resolve(&SecretRef::parse(ITEM).unwrap()).unwrap();
    assert_eq!(value.expose(), ITEM_VALUE);
    assert_eq!(format!("{value:?}"), "SecretValue(<redacted>)");
    assert!(
        fx.memory.find(ITEM_VALUE.as_bytes()).is_some(),
        "the value is remembered for the scanner"
    );
    assert_eq!(
        stub.calls(),
        ["--version".to_owned(), format!("read --no-newline {ITEM}")]
    );
}

#[test]
fn op_missing_item_is_missing() {
    let fx = Fx::new();
    let _stub = fx.op_stub();
    let resolution = fx.check(&fx.op(), "op://Example Vault/No Such Item/credential");
    assert_eq!(resolution.status, SecretStatus::Missing, "{resolution}");
}

#[test]
fn op_locked_is_locked() {
    let fx = Fx::new();
    let mut stub = fx.op_stub();
    stub.set_mode(OpStubMode::Locked);
    let op = fx.op();
    assert_eq!(fx.check(&op, ITEM).status, SecretStatus::Locked);
    assert_eq!(
        fx.check(&op, &op_env("LINEAR_TOKEN")).status,
        SecretStatus::Locked
    );
    assert!(fx.memory.is_empty());
}

#[test]
fn op_2_40_reads_items_but_has_no_environments() {
    let fx = Fx::new();
    let mut stub = fx.op_stub();
    stub.set_version("2.40.0");
    let op = fx.op();
    assert_eq!(fx.check(&op, ITEM), Resolution::resolved());
    let resolution = fx.check(&op, &op_env("LINEAR_TOKEN"));
    assert_eq!(resolution.status, SecretStatus::ProviderTooOld);
    assert!(
        resolution.message.as_deref().unwrap().contains("2.41.0"),
        "{resolution}"
    );
    let resolver = Resolver::new(&fx.env, &fx.keyring, &op, fx.memory.clone());
    assert_eq!(
        resolver
            .resolve(&SecretRef::parse(&op_env("LINEAR_TOKEN")).unwrap())
            .unwrap_err()
            .status,
        SecretStatus::ProviderTooOld
    );
    assert_eq!(
        op.info().features,
        [
            (OpFeature::ItemRead, true),
            (OpFeature::Environments, false),
            (OpFeature::EnvironmentListing, false),
        ]
    );
    assert!(
        stub.calls()
            .iter()
            .all(|call| !call.starts_with("environment")),
        "{:?}",
        stub.calls()
    );
}

#[test]
fn op_2_41_cannot_verify_environment_variables() {
    let fx = Fx::new();
    let mut stub = fx.op_stub();
    stub.set_version("2.41.1");
    let resolution = fx.check(&fx.op(), &op_env("LINEAR_TOKEN"));
    assert_eq!(
        resolution.status,
        SecretStatus::Unverifiable,
        "{resolution}"
    );
    assert!(
        stub.calls()
            .iter()
            .all(|call| !call.starts_with("environment"))
    );
}

#[test]
fn op_2_42_checks_environment_variables_by_name() {
    let fx = Fx::new();
    let stub = fx.op_stub();
    let op = fx.op();
    assert_eq!(
        fx.check(&op, &op_env("LINEAR_TOKEN")),
        Resolution::resolved()
    );
    assert_eq!(
        fx.check(&op, &op_env("NOT_THERE")).status,
        SecretStatus::Missing
    );
    assert_eq!(
        fx.check(&op, "op-env://otherenv0000/LINEAR_TOKEN").status,
        SecretStatus::Missing
    );
    // Names only: the listed values are dropped, never kept or recorded.
    assert_eq!(
        op.list_environment(&fx.env, ENVIRONMENT).unwrap(),
        ["LINEAR_TOKEN"]
    );
    assert!(fx.memory.is_empty());
    // An Environment value is never fetched by Loadout itself.
    let resolver = Resolver::new(&fx.env, &fx.keyring, &op, fx.memory.clone());
    assert!(
        resolver
            .resolve(&SecretRef::parse(&op_env("LINEAR_TOKEN")).unwrap())
            .is_err()
    );
    let calls = stub.calls();
    assert!(calls.contains(&format!(
        "environment secret list {ENVIRONMENT} --format json"
    )));
    assert!(
        calls
            .iter()
            .all(|call| !call.starts_with("environment read")),
        "{calls:?}"
    );
}

/// The guard behind "`op environment read` is never invoked": a stub that
/// saw the call fails the test when it is dropped.
#[test]
fn the_stub_fails_a_test_that_calls_environment_read() {
    let fx = Fx::new();
    let stub = fx.op_stub();
    let op = fx
        .th
        .bin()
        .join(format!("op{}", std::env::consts::EXE_SUFFIX));
    let status = Command::new(&op)
        .args(["environment", "read", ENVIRONMENT])
        .env_clear()
        .status()
        .unwrap();
    assert!(!status.success());
    let dropped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(stub)));
    assert!(dropped.is_err(), "dropping the stub must fail the test");
}

#[test]
fn op_version_text() {
    assert_eq!(OpVersion::parse("2.42.0\n"), Some(OpVersion::new(2, 42, 0)));
    assert_eq!(
        OpVersion::parse("2.42.0-beta.01"),
        Some(OpVersion::new(2, 42, 0))
    );
    assert_eq!(OpVersion::parse("2.42"), None);
    assert_eq!(OpVersion::parse("op version two"), None);
    assert!(OpVersion::new(2, 40, 0) < OpFeature::Environments.min_version());
    assert!(OpVersion::new(2, 41, 1) < OpFeature::EnvironmentListing.min_version());
    assert!(OpVersion::new(1, 99, 0) < OpFeature::ItemRead.min_version());
}

/// SPEC 11.11: on Windows `op` is also found in the WinGet links folder;
/// relative `PATH` entries never are.
#[cfg(windows)]
#[test]
fn op_is_found_in_the_winget_links_folder() {
    let fx = Fx::new();
    let links = fx
        .th
        .local_appdata()
        .join("Microsoft")
        .join("WinGet")
        .join("Links");
    std::fs::create_dir_all(&links).unwrap();
    let _stub = OpStub::install(&links);
    let env = fx.th.env_builder(Os::Windows).var("PATH", "bin;.").build();
    let op = OpCli::detect(&env, &fx.fs);
    assert_eq!(op.path(), Some(links.join("op.exe").as_path()));
    assert_eq!(op.version(), Some(OpVersion::new(2, 42, 0)));
}

#[test]
fn env_and_keyring_statuses() {
    let fx = Fx::new();
    let op = OpCli::NotFound;
    assert_eq!(fx.check(&op, "env:FAKE_ENV_SECRET"), Resolution::resolved());
    assert_eq!(
        fx.check(&op, "env:FAKE_UNSET").status,
        SecretStatus::Missing
    );
    assert_eq!(
        fx.check(&op, "keyring://loadout/ABSENT").status,
        SecretStatus::Missing
    );
    fx.keyring
        .insert("loadout", "PRESENT", "fake-keyring-value-02");
    assert_eq!(
        fx.check(&op, "keyring://loadout/PRESENT"),
        Resolution::resolved()
    );
    fx.keyring.set_mode(MemKeyringMode::Locked);
    assert_eq!(
        fx.check(&op, "keyring://loadout/PRESENT").status,
        SecretStatus::Locked
    );
    fx.keyring.set_mode(MemKeyringMode::Unavailable);
    assert_eq!(
        fx.check(&op, "keyring://loadout/PRESENT").status,
        SecretStatus::ProviderUnavailable
    );
    assert!(fx.memory.find(b"fake-env-value-0001").is_some());
    assert!(fx.memory.find(b"fake-keyring-value-02").is_some());
}

#[test]
fn keyring_set_and_delete_through_the_resolver() {
    let fx = Fx::new();
    let op = OpCli::NotFound;
    let resolver = Resolver::new(&fx.env, &fx.keyring, &op, fx.memory.clone());
    let value = agent_loadout_core::secrets::SecretValue::new("fake-typed-value-0003");
    resolver.set_keyring("loadout", "TYPED", &value).unwrap();
    assert_eq!(
        fx.keyring.peek("loadout", "TYPED").as_deref(),
        Some("fake-typed-value-0003")
    );
    assert!(fx.memory.find(b"fake-typed-value-0003").is_some());
    assert!(resolver.delete_keyring("loadout", "TYPED").unwrap());
    assert!(!resolver.delete_keyring("loadout", "TYPED").unwrap());
}

const CATALOG: &str = "schema = 1\n\n# Shared by every machine.\n[secrets.GITHUB_PAT]\nref = \"op://Example Vault/Fake Item/credential\"\ndescription = \"Fake PAT\"\n\n[secrets.HASS_TOKEN]\nref = \"env:FAKE_ENV_SECRET\"\nenv_name = \"HASS\"\n";

fn server_text(id: &str, value: &str) -> String {
    format!(
        "schema = 1\nid = \"{id}\"\ntransport = \"stdio\"\ncommand = \"npx\"\n\n[env]\nTOKEN = \"{value}\"\n"
    )
}

/// One alias resolves from 1Password on one machine and from the keyring
/// on another: the machine overlay wins on its own machine only.
#[test]
fn catalog_applies_this_machines_overlay() {
    let fx = Fx::new();
    let store = fx.store();
    let machine = store
        .transaction(Operation::new("test.edit", "secrets: catalog"), |tx| {
            tx.write("secrets.toml", CATALOG)?;
            tx.write(
                "servers/github.toml",
                server_text("github", "${secret:GITHUB_PAT}"),
            )?;
            tx.write(
                "servers/hass.toml",
                server_text("hass", "${secret:HASS_TOKEN}"),
            )?;
            tx.write(
                "servers/orphan.toml",
                server_text("orphan", "${secret:UNDEFINED}"),
            )?;
            catalog::stage_set(
                tx,
                "GITHUB_PAT",
                &AliasUpdate {
                    reference: SecretRef::parse("keyring://loadout/GITHUB_PAT").unwrap(),
                    description: None,
                    env_name: None,
                },
                SetScope::Machine,
            )?;
            Ok(tx.machine().id.clone())
        })
        .unwrap()
        .value;
    let overlay = store
        .read_to_string(&catalog::overlay_path(&machine).unwrap())
        .unwrap()
        .unwrap();
    assert!(overlay.contains("[secrets.GITHUB_PAT]"), "{overlay}");
    assert!(overlay.contains("label = \"test-desk\""), "{overlay}");

    let here = Catalog::load(&store, Some(&machine)).unwrap();
    let pat = here.get("GITHUB_PAT").unwrap();
    assert_eq!(pat.reference, "keyring://loadout/GITHUB_PAT");
    assert_eq!(pat.source, RefSource::Machine);
    assert_eq!(pat.store_reference.as_deref(), Some(ITEM));
    assert_eq!(pat.description.as_deref(), Some("Fake PAT"));
    assert_eq!(pat.env_name(), "GITHUB_PAT");
    assert_eq!(here.get("HASS_TOKEN").unwrap().env_name(), "HASS");

    let elsewhere = Catalog::load(&store, Some("01JCX5B7K3M9N1P5Q7R3S9T1V5")).unwrap();
    assert_eq!(elsewhere.get("GITHUB_PAT").unwrap().reference, ITEM);
    assert_eq!(
        elsewhere.get("GITHUB_PAT").unwrap().source,
        RefSource::Store
    );

    let usage = catalog::usage(&store, Some(&machine)).unwrap();
    assert_eq!(
        usage.keys().map(String::as_str).collect::<Vec<_>>(),
        ["GITHUB_PAT", "HASS_TOKEN", "UNDEFINED"]
    );
    assert!(usage["GITHUB_PAT"].contains("github"));

    // No `op` on this machine: the overlay's keyring ref still resolves;
    // without the overlay the alias is blocked on this machine only.
    fx.keyring
        .insert("loadout", "GITHUB_PAT", "fake-keyring-pat-0004");
    let op = OpCli::NotFound;
    let resolver = Resolver::new(&fx.env, &fx.keyring, &op, store.secret_memory().clone());
    let statuses = |catalog: &Catalog| {
        catalog::check(catalog, &usage, &resolver, &[])
            .into_iter()
            .map(|status| (status.alias, status.resolution.status))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        statuses(&here),
        [
            ("GITHUB_PAT".to_owned(), SecretStatus::Resolved),
            ("HASS_TOKEN".to_owned(), SecretStatus::Resolved),
            ("UNDEFINED".to_owned(), SecretStatus::Missing),
        ]
    );
    assert_eq!(
        statuses(&elsewhere)[0],
        ("GITHUB_PAT".to_owned(), SecretStatus::ProviderUnavailable)
    );
    let only = catalog::check(&here, &usage, &resolver, &["HASS_TOKEN".to_owned()]);
    assert_eq!(only.len(), 1);
    assert_eq!(only[0].servers.iter().collect::<Vec<_>>(), ["hass"]);
}

#[test]
fn setting_an_alias_keeps_comments_and_validates() {
    let fx = Fx::new();
    let store = fx.store();
    store
        .transaction(Operation::new("test.edit", "secrets: catalog"), |tx| {
            tx.write("secrets.toml", CATALOG)
        })
        .unwrap();
    store
        .transaction(
            Operation::new("secret.set", "secret: set OPENAI_API_KEY"),
            |tx| {
                catalog::stage_set(
                    tx,
                    "OPENAI_API_KEY",
                    &AliasUpdate {
                        reference: SecretRef::parse("keyring://loadout/OPENAI_API_KEY").unwrap(),
                        description: Some("Fake key".to_owned()),
                        env_name: None,
                    },
                    SetScope::Store,
                )
            },
        )
        .unwrap();
    let text = store.read_to_string("secrets.toml").unwrap().unwrap();
    assert!(text.starts_with(CATALOG), "{text}");
    assert!(
        text.contains("[secrets.OPENAI_API_KEY]\nref = \"keyring://loadout/OPENAI_API_KEY\"\ndescription = \"Fake key\""),
        "{text}"
    );
    let err = store
        .transaction(Operation::new("secret.set", "secret: set bad"), |tx| {
            catalog::stage_set(
                tx,
                "bad-alias",
                &AliasUpdate {
                    reference: SecretRef::parse("env:X").unwrap(),
                    description: None,
                    env_name: None,
                },
                SetScope::Store,
            )
        })
        .unwrap_err();
    assert_eq!(err.code(), agent_loadout_core::ErrorCode::InvalidId);
    assert!(!Path::new(&fx.home.store_dir().join("machines")).exists());
}

//! Secret delivery (SPEC 11.4, 11.5, 11.9): the Tier A matrix of what each
//! client gets for env references, 1Password items and Environments,
//! keyring entries and an explicit `inline` server, and what blocks an
//! entry on this machine (no `op`, an old `op`, no launcher, a missing
//! keyring entry, a shared file).
//!
//! The matrix renders with a synthetic `Env` per OS, a fixed launcher path
//! and a pretend `op` that is never run (planning never reads a 1Password
//! value), so the golden is the same on every host.

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_loadout_core::adapters::detect::NoRegistry;
use agent_loadout_core::adapters::mapper::WriteCx;
use agent_loadout_core::adapters::{AdapterSpec, adapter};
use agent_loadout_core::apply::EntryState;
use agent_loadout_core::apply::render::{DesiredState, RenderCx, RenderOutput, render};
use agent_loadout_core::formats::Document;
use agent_loadout_core::secrets::delivery::{DeliveryEnv, Sharing};
use agent_loadout_core::secrets::providers::onepassword::{OpCli, OpVersion};
use agent_loadout_core::secrets::scanner::SecretMemory;
use agent_loadout_core::{CoreError, Env, ErrorCode, Os};
use agent_loadout_testkit::MemKeyring;
use support::{Fx, entry, json_entry};

const LINEAR_VALUE: &str = "fake-linear-token-0001";

const SECRETS: &str = r#"schema = 1

[onepassword]
environment = "fakeenvironment0001"

[secrets.GITHUB_PAT]
ref = "op://Example Vault/Fake PAT/credential"

[secrets.MCP_TOKEN]
ref = "op-env://fakeenvironment0001/MCP_TOKEN"

[secrets.LINEAR_TOKEN]
ref = "keyring://loadout/LINEAR_TOKEN"

[secrets.HASS_TOKEN]
ref = "env:HASS_TOKEN"
"#;

/// Server files, enabled for every Tier A client.
const SERVERS: [(&str, &str); 8] = [
    (
        "github",
        r#"transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
[env]
GITHUB_PERSONAL_ACCESS_TOKEN = "${secret:GITHUB_PAT}"
GITHUB_TOOLSETS = "repos,issues"
"#,
    ),
    (
        "envtoken",
        r#"transport = "stdio"
command = "example-mcp"
[env]
MCP_TOKEN = "${secret:MCP_TOKEN}"
PAT = "${secret:GITHUB_PAT}"
"#,
    ),
    (
        "hass",
        r#"transport = "stdio"
command = "hass-mcp"
[env]
HASS_TOKEN = "${env:HASS_TOKEN}"
"#,
    ),
    (
        "renamed",
        r#"transport = "stdio"
command = "hass-mcp"
[env]
API_KEY = "${secret:HASS_TOKEN}"
"#,
    ),
    (
        "linear",
        r#"transport = "http"
url = "https://mcp.example.test/mcp"
[headers]
Authorization = "Bearer ${secret:LINEAR_TOKEN}"
X-Team = "fake-team"
"#,
    ),
    (
        "tracker",
        r#"transport = "http"
url = "https://tracker.example.test/mcp?key=${secret:GITHUB_PAT}"
"#,
    ),
    (
        "stored",
        r#"transport = "stdio"
command = "stored-mcp"
[env]
STORED_TOKEN = "${secret:LINEAR_TOKEN}"
[delivery]
mode = "inline"
"#,
    ),
    (
        "envheader",
        r#"transport = "http"
url = "https://env.example.test/mcp"
[headers]
Authorization = "Bearer ${secret:MCP_TOKEN}"
"#,
    ),
];

const TIER_A: [&str; 5] = ["claude-code", "omp", "pi", "codex", "claude-desktop"];

fn server_text(id: &str, body: &str) -> String {
    let clients = TIER_A
        .iter()
        .map(|c| format!("{c:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("schema = 1\nid = \"{id}\"\n{body}\n[enable]\nclients = [{clients}]\n")
}

/// A store holding the catalog and every server of [`SERVERS`].
fn store(fx: &Fx) {
    fx.edit("secrets and servers", |tx| {
        tx.write("secrets.toml", SECRETS)?;
        for (id, body) in SERVERS {
            tx.write(&format!("servers/{id}.toml"), server_text(id, body))?;
        }
        Ok(())
    });
}

fn launcher(os: Os) -> PathBuf {
    match os {
        Os::Windows => PathBuf::from(r"C:\Program Files\Loadout\loadout.exe"),
        Os::Linux | Os::MacOs => PathBuf::from("/opt/loadout/bin/loadout"),
    }
}

fn synthetic_env(os: Os) -> Env {
    let home = match os {
        Os::Windows => r"C:\Users\example",
        Os::Linux | Os::MacOs => "/home/example",
    };
    Env::builder(os, home).build()
}

fn op(version: OpVersion) -> OpCli {
    OpCli::Found {
        path: PathBuf::from("/usr/bin/op"),
        version: Ok(version),
    }
}

fn keyring() -> Arc<MemKeyring> {
    Arc::new(MemKeyring::with("loadout", "LINEAR_TOKEN", LINEAR_VALUE))
}

/// Renders `server` for `client` with a synthetic environment of `os`.
fn render_one(
    fx: &Fx,
    desired: &DesiredState,
    delivery: &DeliveryEnv,
    env: &Env,
    client: &str,
    server: &str,
    sharing: Sharing,
) -> Result<RenderOutput, CoreError> {
    let cx = RenderCx {
        env,
        fs: &fx.fs,
        loadout_home: Path::new("/loadout-home"),
        registry: &NoRegistry,
        delivery,
        memory: Arc::new(SecretMemory::new().unwrap()),
        probes: None,
    };
    let spec = adapter(client).unwrap();
    render(&cx, desired, &desired.servers[server], spec, sharing)
}
fn desired(fx: &Fx) -> DesiredState {
    DesiredState::load(&fx.store(), None).unwrap()
}

/// Every server of [`SERVERS`] written into a new file of `client`, and
/// the delivery or blocking reason of each.
fn rendered_file(fx: &Fx, desired: &DesiredState, client: &str, os: Os) -> String {
    let spec: &AdapterSpec = adapter(client).unwrap();
    let mcp = spec.mcp.as_ref().unwrap();
    let delivery =
        DeliveryEnv::new(Some(launcher(os)), keyring()).with_op(op(OpVersion::new(2, 42, 0)));
    let env = synthetic_env(os);
    let mut doc = Document::parse(mcp.skeleton.as_bytes(), mcp.format).unwrap();
    let mut report = String::new();
    for (id, _) in SERVERS {
        let rendered = render_one(fx, desired, &delivery, &env, client, id, Sharing::Private)
            .and_then(|out| {
                let native = mcp
                    .mapper
                    .to_native(&out.server, &WriteCx { os, existing: None })
                    .map_err(CoreError::from)?;
                Ok((out, native))
            });
        match rendered {
            Ok((out, native)) => {
                doc.upsert(mcp.container.segments(), id, &native).unwrap();
                let mode = out.delivery.map_or("none", |mode| mode.as_str());
                report.push_str(&format!("{id}: {mode}\n"));
                for warning in out.warnings {
                    report.push_str(&format!(
                        "  warning {}: {}\n",
                        warning.code(),
                        warning.message()
                    ));
                }
            }
            Err(err) => report.push_str(&format!(
                "{id}: blocked {}: {}\n",
                err.code(),
                err.message()
            )),
        }
    }
    let text = String::from_utf8(doc.to_bytes()).unwrap();
    format!(
        "## {client} ({})\n{text}\n# delivery\n{report}",
        os.as_str()
    )
}

#[test]
fn tier_a_delivery_matrix() {
    let fx = Fx::new();
    store(&fx);
    let desired = desired(&fx);
    let mut golden = String::new();
    for (client, os) in [
        ("claude-code", Os::Linux),
        ("claude-code", Os::Windows),
        ("omp", Os::Linux),
        ("omp", Os::Windows),
        ("pi", Os::Linux),
        ("codex", Os::Linux),
        ("claude-desktop", Os::Windows),
    ] {
        golden.push_str(&rendered_file(&fx, &desired, client, os));
        golden.push('\n');
    }
    insta::assert_snapshot!("tier_a", golden);
}

#[test]
fn launch_entries_plan_as_provider_unavailable_without_op() {
    let fx = Fx::new();
    store(&fx);
    fx.keyring.insert("loadout", "LINEAR_TOKEN", LINEAR_VALUE);
    let plan = fx.plan();
    let claude = fx.claude();
    for name in ["github", "envtoken", "tracker"] {
        let entry = entry(&plan, &claude, name);
        assert_eq!(entry.state, EntryState::Blocked, "{name}");
    }
    let github = entry(&plan, &claude, "github").problem.clone().unwrap();
    assert_eq!(github.code(), ErrorCode::ProviderUnavailable);
    assert!(
        github.message().contains("1Password CLI not found")
            && github
                .message()
                .contains("map GITHUB_PAT to keyring/env for this machine"),
        "{github}"
    );
    assert_eq!(
        entry(&plan, &claude, "envtoken")
            .problem
            .as_ref()
            .unwrap()
            .code(),
        ErrorCode::ProviderUnavailable
    );
    // Not a 1Password problem: the URL has no secret-free delivery anyway.
    assert_eq!(
        entry(&plan, &claude, "tracker")
            .problem
            .as_ref()
            .unwrap()
            .code(),
        ErrorCode::NoSecretFreeDelivery
    );
    // Env and keyring references work normally.
    for (name, delivery) in [("hass", "env"), ("linear", "command"), ("stored", "inline")] {
        let entry = entry(&plan, &claude, name);
        assert_eq!(entry.state, EntryState::PendingCreate, "{name}: {entry:#?}");
        assert_eq!(entry.delivery, Some(delivery), "{name}");
    }

    // The machine overlay maps the alias to the keyring on this machine:
    // the entry launches, with the keyring reference in its argv.
    fx.apply();
    let machine = fx.machine_id();
    fx.edit("overlay", |tx| {
        tx.write(
            &format!("machines/{machine}.toml"),
            format!(
                "schema = 1\nmachine_id = \"{machine}\"\nlabel = \"test-desk\"\nos = \"{}\"\n\n\
                 [secrets.GITHUB_PAT]\nref = \"keyring://loadout/GITHUB_PAT\"\n",
                support::host_os().as_str()
            ),
        )
    });
    let github = entry(&fx.plan(), &claude, "github").clone();
    assert_eq!(github.state, EntryState::Blocked);
    assert_eq!(github.problem.unwrap().code(), ErrorCode::MissingSecret);
    fx.keyring
        .insert("loadout", "GITHUB_PAT", "fake-pat-value-0001");
    let github = entry(&fx.plan(), &claude, "github").clone();
    assert_eq!(github.state, EntryState::PendingCreate);
    assert_eq!(github.delivery, Some("launch"));
    let args = github
        .desired
        .as_ref()
        .unwrap()
        .as_object()
        .unwrap()
        .get("args")
        .cloned()
        .unwrap();
    let args: Vec<&str> = args
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(
        args,
        [
            "exec",
            "--ref",
            "GITHUB_PERSONAL_ACCESS_TOKEN=keyring://loadout/GITHUB_PAT",
            "--",
            "npx",
            "-y",
            "@modelcontextprotocol/server-github",
        ]
    );
}

#[test]
fn environments_need_op_2_41() {
    let fx = Fx::new();
    store(&fx);
    let desired = desired(&fx);
    let env = synthetic_env(Os::Linux);
    let render_with = |version| {
        let delivery = DeliveryEnv::new(Some(launcher(Os::Linux)), keyring()).with_op(op(version));
        render_one(
            &fx,
            &desired,
            &delivery,
            &env,
            "claude-code",
            "envtoken",
            Sharing::Private,
        )
    };
    let err = render_with(OpVersion::new(2, 40, 0)).unwrap_err();
    assert_eq!(err.code(), ErrorCode::ProviderTooOld);
    assert!(err.message().contains("2.41.0"), "{err}");
    let out = render_with(OpVersion::new(2, 41, 1)).unwrap();
    assert_eq!(out.delivery.unwrap().as_str(), "launch");
}

#[test]
fn the_op_account_stays_out_of_shared_files() {
    let fx = Fx::new();
    store(&fx);
    fx.edit("account", |tx| {
        tx.write(
            "secrets.toml",
            SECRETS.replace(
                "[onepassword]\n",
                "[onepassword]\naccount = \"family.example.com\"\n",
            ),
        )
    });
    let desired = desired(&fx);
    let env = synthetic_env(Os::Linux);
    let delivery = DeliveryEnv::new(Some(launcher(Os::Linux)), keyring())
        .with_op(op(OpVersion::new(2, 42, 0)));
    let args = |sharing| {
        let out = render_one(
            &fx,
            &desired,
            &delivery,
            &env,
            "claude-code",
            "github",
            sharing,
        )
        .unwrap();
        let agent_loadout_core::adapters::mapper::Launch::Stdio(stdio) = out.server.launch else {
            panic!("stdio");
        };
        stdio.args
    };
    // This machine's account in its own file; a shared file relies on
    // OP_ACCOUNT or each machine's setting.
    assert_eq!(
        args(Sharing::Private)[..3],
        ["exec", "--op-account", "family.example.com"]
    );
    let shared = args(Sharing::Shared);
    assert_eq!(shared[0], "exec");
    assert!(
        !shared
            .iter()
            .any(|arg| arg.contains("op-account") || arg.contains("family"))
    );
}

#[test]
fn inline_is_refused_for_shared_targets() {
    let fx = Fx::new();
    store(&fx);
    let desired = desired(&fx);
    let env = synthetic_env(Os::Linux).with_var("HASS_TOKEN", "fake-hass-value-0001");
    let delivery = DeliveryEnv::new(Some(launcher(Os::Linux)), keyring())
        .with_op(op(OpVersion::new(2, 42, 0)));
    let shared = |client: &str, server: &str| {
        render_one(
            &fx,
            &desired,
            &delivery,
            &env,
            client,
            server,
            Sharing::Shared,
        )
    };

    // Explicit inline.
    let err = shared("claude-code", "stored").unwrap_err();
    assert_eq!(err.code(), ErrorCode::SecretInSharedFile);
    // An env reference Claude Desktop cannot expand launches; it is never
    // written inline when `loadout exec` can carry it.
    let private = render_one(
        &fx,
        &desired,
        &delivery,
        &env,
        "claude-desktop",
        "hass",
        Sharing::Private,
    );
    assert_eq!(private.unwrap().delivery.unwrap().as_str(), "launch");

    // Launch in a shared file starts the bare `loadout`, so the file stays
    // portable; private files get the absolute path.
    let out = shared("claude-code", "github").unwrap();
    let agent_loadout_core::adapters::mapper::Launch::Stdio(stdio) = &out.server.launch else {
        panic!("stdio");
    };
    assert_eq!(stdio.command, "loadout");
    assert_eq!(stdio.args[0], "exec");
}

#[test]
fn inline_env_fallback_warns_and_is_refused_when_shared() {
    let fx = Fx::new();
    fx.edit("server", |tx| {
        tx.write(
            "servers/notes.toml",
            server_text(
                "notes",
                "transport = \"stdio\"\ncommand = \"notes-mcp\"\nargs = [\"--root\", \"${env:NOTES_DIR}\"]\n",
            ),
        )
    });
    let desired = desired(&fx);
    let delivery = DeliveryEnv::new(Some(launcher(Os::Linux)), keyring());
    let env = synthetic_env(Os::Linux).with_var("NOTES_DIR", "/srv/fake-notes");

    // Pi does not expand arguments, and `loadout exec` sets only `env`.
    let out = render_one(
        &fx,
        &desired,
        &delivery,
        &env,
        "pi",
        "notes",
        Sharing::Private,
    )
    .unwrap();
    assert_eq!(out.delivery.unwrap().as_str(), "inline");
    let agent_loadout_core::adapters::mapper::Launch::Stdio(stdio) = &out.server.launch else {
        panic!("stdio");
    };
    assert_eq!(stdio.args, ["--root", "/srv/fake-notes"]);
    assert_eq!(out.warnings.len(), 1);
    assert_eq!(out.warnings[0].code(), ErrorCode::InlineEnvFallback);

    // Claude Code expands arguments: env delivery, no warning.
    let out = render_one(
        &fx,
        &desired,
        &delivery,
        &env,
        "claude-code",
        "notes",
        Sharing::Private,
    )
    .unwrap();
    assert_eq!(out.delivery.unwrap().as_str(), "env");
    assert!(out.warnings.is_empty());

    let err = render_one(
        &fx,
        &desired,
        &delivery,
        &env,
        "pi",
        "notes",
        Sharing::Shared,
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::SecretInSharedFile);
}

#[test]
fn launch_without_a_launcher_is_blocked() {
    let fx = Fx::new();
    store(&fx);
    let desired = desired(&fx);
    let delivery = DeliveryEnv::new(None, keyring()).with_op(op(OpVersion::new(2, 42, 0)));
    let env = synthetic_env(Os::Linux);
    for (client, server) in [
        ("claude-code", "github"),
        ("omp", "linear"),
        ("codex", "renamed"),
    ] {
        let err = render_one(
            &fx,
            &desired,
            &delivery,
            &env,
            client,
            server,
            Sharing::Private,
        )
        .unwrap_err();
        assert_eq!(err.code(), ErrorCode::LauncherNotFound, "{client} {server}");
    }
    // Delivery without the launcher still works.
    let out = render_one(
        &fx,
        &desired,
        &delivery,
        &env,
        "claude-code",
        "hass",
        Sharing::Private,
    )
    .unwrap();
    assert_eq!(out.delivery.unwrap().as_str(), "env");
}

#[test]
fn exported_provider_secrets_use_env_where_nothing_else_fits() {
    let fx = Fx::new();
    store(&fx);
    fx.edit("server", |tx| {
        tx.write(
            "servers/argtoken.toml",
            server_text(
                "argtoken",
                "transport = \"stdio\"\ncommand = \"arg-mcp\"\nargs = [\"--token=${secret:GITHUB_PAT}\"]\n",
            ),
        )
    });
    let delivery = DeliveryEnv::new(Some(launcher(Os::Linux)), keyring())
        .with_op(op(OpVersion::new(2, 42, 0)));
    let env = synthetic_env(Os::Linux);
    let err = render_one(
        &fx,
        &desired(&fx),
        &delivery,
        &env,
        "claude-code",
        "argtoken",
        Sharing::Private,
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::NoSecretFreeDelivery);
    assert!(err.message().contains("export_provider_secrets"), "{err}");

    fx.apply();
    let machine = fx.machine_id();
    fx.edit("overlay", |tx| {
        tx.write(
            &format!("machines/{machine}.toml"),
            format!(
                "schema = 1\nmachine_id = \"{machine}\"\nlabel = \"test-desk\"\nos = \"linux\"\n\n\
                 [delivery]\nexport_provider_secrets = true\n"
            ),
        )
    });
    let desired = DesiredState::load(&fx.store(), Some(&machine)).unwrap();
    let out = render_one(
        &fx,
        &desired,
        &delivery,
        &env,
        "claude-code",
        "argtoken",
        Sharing::Private,
    )
    .unwrap();
    assert_eq!(out.delivery.unwrap().as_str(), "env");
    let agent_loadout_core::adapters::mapper::Launch::Stdio(stdio) = &out.server.launch else {
        panic!("stdio");
    };
    assert_eq!(stdio.args, ["--token=${GITHUB_PAT}"]);
    // Pi does not expand arguments: still nothing secret-free.
    let err = render_one(
        &fx,
        &desired,
        &delivery,
        &env,
        "pi",
        "argtoken",
        Sharing::Private,
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::NoSecretFreeDelivery);
}

#[test]
fn inline_values_are_written_masked_in_diffs_and_never_stored() {
    let fx = Fx::new();
    fx.keyring.insert("loadout", "LINEAR_TOKEN", LINEAR_VALUE);
    fx.edit("inline server", |tx| {
        tx.write("secrets.toml", SECRETS)?;
        tx.write(
            "servers/stored.toml",
            server_text("stored", SERVERS[6].1).replace(
                &format!(
                    "clients = [{}]",
                    TIER_A
                        .iter()
                        .map(|c| format!("{c:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                "clients = [\"claude-code\"]",
            ),
        )
    });
    let plan = fx.plan();
    let target = &plan.targets[0];
    let after = String::from_utf8(target.after.clone().unwrap()).unwrap();
    assert_eq!(
        json_entry(&after, "stored").unwrap()["env"]["STORED_TOKEN"],
        LINEAR_VALUE
    );
    let diff = target.diff();
    assert!(!diff.contains(LINEAR_VALUE), "{diff}");
    assert!(diff.contains("\"STORED_TOKEN\": \"********\""), "{diff}");

    let (report, _) = fx.apply();
    assert_eq!(report.writes(), 1);
    assert!(fx.read(&fx.claude()).contains(LINEAR_VALUE));
    // The value is in no store file, ledger or journal.
    for dir in [fx.home.store_dir(), fx.home.state_dir()] {
        for path in walk(&dir) {
            let bytes = std::fs::read(&path).unwrap();
            assert!(
                !String::from_utf8_lossy(&bytes).contains(LINEAR_VALUE),
                "{}",
                path.display()
            );
        }
    }

    // A rotated value: neither the old nor the new one shows in the diff,
    // and the change still does.
    const ROTATED: &str = "fake-linear-token-0002";
    fx.keyring.insert("loadout", "LINEAR_TOKEN", ROTATED);
    let plan = fx.plan();
    let diff = plan.targets[0].diff();
    assert!(
        diff.contains("-        \"STORED_TOKEN\": \"******** (before)\"")
            && diff.contains("+        \"STORED_TOKEN\": \"********\""),
        "{diff}"
    );
    for value in [LINEAR_VALUE, ROTATED] {
        assert!(!diff.contains(value), "{value} in\n{diff}");
    }
    fx.apply();

    // Removing the server: the value it held stays masked.
    fx.remove_server("stored");
    let plan = fx.plan();
    let diff = plan.targets[0].diff();
    assert!(diff.contains("-    \"stored\""), "{diff}");
    assert!(!diff.contains(ROTATED), "{diff}");
}

/// Gate V2 FB-9 (PI-7): Pi expands bare `$NAME` and runs a value starting
/// with `!` in `env` and `headers`, so literals are written with `$$` and
/// `$!` there and read back the same way; Loadout's own `!command`
/// delivery stays a command.
#[test]
fn pi_literals_are_escaped_and_read_back() {
    use agent_loadout_core::adapters::caps::FieldSet;
    use agent_loadout_core::apply::import::server_from_native;
    use agent_loadout_core::apply::render::native_to_template;

    let fx = Fx::new();
    fx.edit("pi literals", |tx| {
        tx.write("secrets.toml", SECRETS)?;
        tx.write(
            "servers/lit.toml",
            "schema = 1\nid = \"lit\"\ntransport = \"stdio\"\ncommand = \"lit-mcp\"\n\
             args = ['a$b', '!arg']\n[env]\nLIT = 'a$b$$c'\nBANG = '!echo literal'\n\
             MIXED = 'x$${env:HOME}-${env:HOME}'\n[enable]\nclients = [\"pi\"]\n",
        )?;
        tx.write(
            "servers/hook.toml",
            "schema = 1\nid = \"hook\"\ntransport = \"http\"\n\
             url = \"https://hook.example.test/mcp\"\n[headers]\n\
             X-Key = \"${secret:LINEAR_TOKEN}\"\nX-Lit = '!lit'\n\
             [enable]\nclients = [\"pi\"]\n",
        )
    });
    let desired = desired(&fx);
    let pi: &AdapterSpec = adapter("pi").unwrap();
    let mcp = pi.mcp.as_ref().unwrap();
    let os = Os::Linux;
    let delivery =
        DeliveryEnv::new(Some(launcher(os)), keyring()).with_op(op(OpVersion::new(2, 42, 0)));
    let env = synthetic_env(os);
    let native = |id: &str| {
        let out = render_one(&fx, &desired, &delivery, &env, "pi", id, Sharing::Private).unwrap();
        mcp.mapper
            .to_native(&out.server, &WriteCx { os, existing: None })
            .unwrap()
    };
    let string = |value: &agent_loadout_core::formats::NativeValue, path: &[&str]| {
        let mut value = value;
        for key in path {
            value = value.as_object().unwrap().get(key).unwrap();
        }
        value.as_str().unwrap().to_owned()
    };

    let lit = native("lit");
    assert_eq!(string(&lit, &["env", "LIT"]), "a$$b$$$$c");
    assert_eq!(string(&lit, &["env", "BANG"]), "$!echo literal");
    assert_eq!(string(&lit, &["env", "MIXED"]), "x$${env:HOME}-${HOME}");
    // Pi expands nothing in `args`.
    let args = lit
        .as_object()
        .unwrap()
        .get("args")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(args[0].as_str(), Some("a$b"));
    assert_eq!(args[1].as_str(), Some("!arg"));

    // Read back: the store values come out unchanged.
    let imported = server_from_native(pi, "lit", &lit).unwrap();
    let stored = &desired.servers["lit"].body;
    assert_eq!(imported.fields.env, stored.env);
    assert_eq!(imported.fields.args, stored.args);

    // A keyring header is Loadout's own `!command`; a literal `!` is not.
    let hook = native("hook");
    let key = string(&hook, &["headers", "X-Key"]);
    assert!(
        key.starts_with("!'/opt/loadout/bin/loadout' secret get"),
        "{key}"
    );
    assert_eq!(string(&hook, &["headers", "X-Lit"]), "$!lit");
    // A `!command` has no store form.
    let err = server_from_native(pi, "hook", &hook).unwrap_err();
    assert_eq!(err.code(), ErrorCode::UnsupportedField);

    // Pi's reading of `$` (its `resolve-config-value`).
    let read = |text: &str| native_to_template(text, &mcp.caps, FieldSet::ENV);
    for (native, template) in [
        ("a$HOMEDRIVE-b$$c", Some("a${env:HOMEDRIVE}-b$c")),
        ("${TOKEN}", Some("${env:TOKEN}")),
        ("$!x and $!", Some("!x and !")),
        ("${A:-b}", Some("$${A:-b}")),
        ("${}", Some("$${}")),
        ("$1 and a$", Some("$1 and a$")),
        ("open ${ only", Some("open $${ only")),
        ("$${X}", Some("$${X}")),
        ("$$$X", None),
        ("!echo hi", None),
    ] {
        assert_eq!(read(native).as_deref(), template, "{native}");
    }
    assert_eq!(
        native_to_template("a$$b", &mcp.caps, FieldSet::ARGS).as_deref(),
        Some("a$$b")
    );
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}

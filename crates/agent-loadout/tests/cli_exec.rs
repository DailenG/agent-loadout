//! `loadout exec` (SPEC 11.5) against `fake-mcp-server`, the `op-stub` and
//! the test keyring file, plus the commands around delivery: an apply that
//! writes a launch entry, `env --export`, `secret get`, `secret-header`,
//! `secrets map-op-env`, `secrets service-account` and the `secrets check`
//! checklist. Every process runs with `TempHome::command` (cleared
//! environment, `PATH` = the temp `bin/`, the test keyring file).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use agent_loadout_testkit::TempHome;
use agent_loadout_testkit::op_stub::{OpStub, OpStubMode, SERVICE_ACCOUNT_SEEN, built_binary};
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_loadout");
const ITEM: &str = "op://Example Vault/Fake PAT/credential";
const ITEM_VALUE: &str = "fake-item-value-0001";
const ENVIRONMENT: &str = "fakeenvironment0001";
const ENV_VALUE: &str = "fake-environment-value-0001";
const TOKEN: &str = "fake-service-account-token-0001";

struct Cli {
    th: TempHome,
    home: PathBuf,
}

impl Cli {
    fn new() -> Self {
        let th = TempHome::new();
        let home = th.root().join("lh");
        Self { th, home }
    }

    fn op_stub(&self) -> OpStub {
        let mut stub = OpStub::install(&self.th.bin());
        stub.add_item(ITEM, ITEM_VALUE)
            .add_environment_variable(ENVIRONMENT, "MCP_TOKEN", ENV_VALUE)
            .add_environment_variable(ENVIRONMENT, "OTHER_SECRET", "fake-other-value-0001");
        stub
    }

    fn keyring(&self, entries: &[(&str, &str)]) {
        let map: BTreeMap<&str, &str> = entries.iter().copied().collect();
        std::fs::write(self.th.keyring_file(), serde_json::to_string(&map).unwrap()).unwrap();
    }

    fn command(&self) -> Command {
        self.th.command(BIN)
    }

    /// `loadout --home <lh> <args>`, stdin closed.
    fn run(&self, args: &[&str]) -> Output {
        self.command()
            .arg("--home")
            .arg(&self.home)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("run loadout")
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let output = self.run(&[&["--json"], args].concat());
        let doc = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
            panic!(
                "{args:?}: stdout is not JSON ({err}):\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.code().unwrap(), doc)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (code, doc) = self.json(args);
        assert_eq!(code, 0, "{args:?}: {doc:#}");
        doc
    }

    /// `loadout exec <args>` with `stdin` written and closed.
    fn exec(&self, args: &[OsString], stdin: &[u8], vars: &[(&str, &str)]) -> Output {
        let mut command = self.command();
        command
            .arg("exec")
            .args(args)
            .envs(vars.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("spawn loadout exec");
        let mut input = child.stdin.take().unwrap();
        input.write_all(stdin).unwrap();
        drop(input);
        child.wait_with_output().unwrap()
    }
}

fn server() -> OsString {
    built_binary("fake-mcp-server").into_os_string()
}

/// `exec` arguments: `words`, then `-- <fake-mcp-server> <mode...>`.
fn args(words: &[&str], mode: &[&str]) -> Vec<OsString> {
    let mut out: Vec<OsString> = words.iter().map(OsString::from).collect();
    out.push("--".into());
    out.push(server());
    out.extend(mode.iter().map(OsString::from));
    out
}

/// The environment `fake-mcp-server env` printed.
fn server_env(output: &Output) -> BTreeMap<String, String> {
    assert!(
        output.status.success(),
        "exit {:?}, stderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let vars: BTreeMap<String, String> = serde_json::from_slice(&output.stdout).unwrap();
    // Windows names are case-insensitive: compare upper-cased.
    vars.into_iter()
        .map(|(name, value)| (name.to_ascii_uppercase(), value))
        .collect()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn stdin_and_stdout_bytes_pass_unchanged() {
    let cli = Cli::new();
    // CRLF, LF, a lone CR, NUL, invalid UTF-8 and no final line break.
    let bytes: &[u8] = b"{\"jsonrpc\":\"2.0\",\"id\":1}\r\n\x00\xff\xfe raw\rline\nlast";
    let output = cli.exec(
        &args(&["--ref", "TOKEN=env:FAKE_SOURCE"], &["echo"]),
        bytes,
        &[("FAKE_SOURCE", "fake-env-value-0001")],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(output.stdout, bytes);
    assert!(output.stderr.is_empty(), "{}", stderr(&output));

    // A JSON-RPC session: every response line arrives as the server wrote it.
    let session = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"env","arguments":{"name":"TOKEN"}}}"#,
        "\n",
    );
    let output = cli.exec(
        &args(&["--ref", "TOKEN=env:FAKE_SOURCE"], &[]),
        session.as_bytes(),
        &[("FAKE_SOURCE", "fake-env-value-0001")],
    );
    assert!(output.stderr.is_empty(), "{}", stderr(&output));
    let lines: Vec<Value> = output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["result"]["serverInfo"]["name"], "fake-mcp-server");
    assert_eq!(
        lines[1]["result"]["content"][0]["text"],
        json!("fake-env-value-0001").to_string()
    );
}

#[test]
fn errors_go_to_stderr_with_the_prefix_and_exit_77_or_78() {
    let cli = Cli::new();
    let check = |output: &Output, code: i32, needle: &str| {
        assert_eq!(output.status.code(), Some(code), "{}", stderr(output));
        assert!(output.stdout.is_empty(), "stdout must stay empty");
        let text = stderr(output);
        assert!(text.starts_with("loadout-exec: "), "{text}");
        assert!(text.contains(needle), "{needle:?} not in {text}");
    };
    let os = |words: &[&str]| -> Vec<OsString> { words.iter().map(OsString::from).collect() };

    // Configuration errors: 78.
    check(
        &cli.exec(&os(&["--ref", "TOKEN=env:X"]), b"", &[]),
        78,
        "expected `-- <command>",
    );
    check(
        &cli.exec(&os(&["--verbose", "--", "x"]), b"", &[]),
        78,
        "unknown option",
    );
    check(
        &cli.exec(&args(&["--ref", "TOKEN"], &["env"]), b"", &[]),
        78,
        "NAME=REF",
    );
    check(
        &cli.exec(&args(&["--ref", "TOKEN=vault://x"], &["env"]), b"", &[]),
        78,
        "invalid secret reference",
    );
    check(
        &cli.exec(&args(&["--map", "A=B"], &["env"]), b"", &[]),
        78,
        "--map needs --op-env",
    );
    check(
        &cli.exec(&args(&["--stage2"], &["env"]), b"", &[]),
        78,
        "--stage2 runs only under",
    );
    check(
        &cli.exec(&os(&["--", "no-such-mcp-server-0001"]), b"", &[]),
        78,
        "cannot start no-such-mcp-server-0001",
    );
    // No 1Password CLI on this machine (SPEC 11.5 wording).
    check(
        &cli.exec(
            &args(
                &["--ref", &format!("GITHUB_PERSONAL_ACCESS_TOKEN={ITEM}")],
                &["env"],
            ),
            b"",
            &[],
        ),
        78,
        "1Password CLI not found (op); install it or map the alias behind \
         GITHUB_PERSONAL_ACCESS_TOKEN to keyring/env for this machine",
    );

    // Secrets that cannot be had: 77.
    check(
        &cli.exec(
            &args(&["--ref", "TOKEN=env:FAKE_UNSET"], &["env"]),
            b"",
            &[],
        ),
        77,
        "TOKEN: missing",
    );
    check(
        &cli.exec(
            &args(&["--ref", "TOKEN=keyring://loadout/ABSENT"], &["env"]),
            b"",
            &[],
        ),
        77,
        "TOKEN: missing",
    );
    let mut stub = cli.op_stub();
    check(
        &cli.exec(
            &args(
                &["--ref", "TOKEN=op://Example Vault/Nope/credential"],
                &["env"],
            ),
            b"",
            &[],
        ),
        77,
        "TOKEN: missing",
    );
    stub.set_mode(OpStubMode::Locked);
    check(
        &cli.exec(
            &args(&["--ref", &format!("TOKEN={ITEM}")], &["env"]),
            b"",
            &[],
        ),
        77,
        "TOKEN: locked",
    );
    stub.set_mode(OpStubMode::Ok).set_version("2.40.0");
    check(
        &cli.exec(
            &args(&["--op-env", ENVIRONMENT, "--map", "T=MCP_TOKEN"], &["env"]),
            b"",
            &[],
        ),
        78,
        "2.41.0",
    );

    // The server's own exit code passes through; so does `--help`, on
    // stderr only.
    let output = cli.exec(&args(&[], &["exit", "9"]), b"", &[]);
    assert_eq!(output.status.code(), Some(9));
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    let output = cli.exec(&os(&["--help"]), b"", &[]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(stderr(&output).contains("usage: loadout exec"));
}

#[test]
fn values_reach_the_server_environment_only() {
    let cli = Cli::new();
    let stub = cli.op_stub();
    cli.keyring(&[("loadout/FAKE_KEY", "fake-keyring-value-0001")]);
    let output = cli.exec(
        &args(
            &[
                "--ref",
                &format!("GITHUB_PERSONAL_ACCESS_TOKEN={ITEM}"),
                "--ref=KEYED=keyring://loadout/FAKE_KEY",
                "--ref",
                "RENAMED=env:FAKE_SOURCE",
            ],
            &["env"],
        ),
        b"",
        &[("FAKE_SOURCE", "fake-env-value-0001")],
    );
    let env = server_env(&output);
    assert_eq!(env["GITHUB_PERSONAL_ACCESS_TOKEN"], ITEM_VALUE);
    assert_eq!(env["KEYED"], "fake-keyring-value-0001");
    assert_eq!(env["RENAMED"], "fake-env-value-0001");
    assert!(output.stderr.is_empty(), "{}", stderr(&output));
    // `op read --no-newline`, the reference in argv, never the value.
    let calls = stub.calls();
    assert!(
        calls
            .iter()
            .any(|call| call == &format!("read --no-newline {ITEM}")),
        "{calls:?}"
    );
    assert!(calls.iter().all(|call| !call.contains(ITEM_VALUE)));
}

#[test]
fn environment_refs_run_through_op_run_with_name_remapping() {
    let cli = Cli::new();
    let stub = cli.op_stub();
    let output = cli.exec(
        &args(
            &[
                "--op-env",
                ENVIRONMENT,
                "--map",
                "SERVER_TOKEN=MCP_TOKEN",
                "--ref",
                &format!("PAT={ITEM}"),
            ],
            &["env"],
        ),
        b"",
        &[],
    );
    let env = server_env(&output);
    // The server's own key, byte-exact (`--no-masking`), and nothing else
    // from the Environment or the stage handshake.
    assert_eq!(env["SERVER_TOKEN"], ENV_VALUE);
    assert_eq!(env["PAT"], ITEM_VALUE);
    for absent in [
        "MCP_TOKEN",
        "OTHER_SECRET",
        "LOADOUT_EXEC_ENV",
        "OP_SERVICE_ACCOUNT_TOKEN",
    ] {
        assert!(!env.contains_key(absent), "{absent} reached the server");
    }
    assert!(env.contains_key("PATH") && env.contains_key("LOADOUT_TEST_KEYRING"));
    let run = stub
        .calls()
        .into_iter()
        .find(|call| call.starts_with("run "))
        .expect("op run was called");
    assert!(
        run.starts_with(&format!("run --environment {ENVIRONMENT} --no-masking -- ")),
        "{run}"
    );
    assert!(
        run.contains("exec --stage2 --map SERVER_TOKEN=MCP_TOKEN -- "),
        "{run}"
    );
    assert!(!stub.calls().iter().any(|call| call.contains(ENV_VALUE)));

    // An op-env reference given as --ref is the same thing.
    let output = cli.exec(
        &args(
            &["--ref", &format!("KEY=op-env://{ENVIRONMENT}/MCP_TOKEN")],
            &["env"],
        ),
        b"",
        &[],
    );
    assert_eq!(server_env(&output)["KEY"], ENV_VALUE);

    // A name the Environment does not have: 77 from stage 2.
    let output = cli.exec(
        &args(&["--op-env", ENVIRONMENT, "--map", "KEY=NOPE"], &["env"]),
        b"",
        &[],
    );
    assert_eq!(output.status.code(), Some(77), "{}", stderr(&output));
    assert!(
        stderr(&output)
            .contains("loadout-exec: KEY: the 1Password Environment has no variable NOPE")
    );
    assert!(output.stdout.is_empty());

    // The client's environment has that name: still 77, its value never
    // stands in for the Environment's.
    let output = cli.exec(
        &args(&["--op-env", ENVIRONMENT, "--map", "KEY=NOPE"], &["env"]),
        b"",
        &[
            ("NOPE", "fake-client-value-0001"),
            ("KEY", "fake-client-value-0002"),
        ],
    );
    assert_eq!(output.status.code(), Some(77), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    // And when the Environment has it, the client's value under the same
    // name loses.
    let output = cli.exec(
        &args(
            &["--op-env", ENVIRONMENT, "--map", "SERVER_TOKEN=MCP_TOKEN"],
            &["env"],
        ),
        b"",
        &[("MCP_TOKEN", "fake-client-value-0003")],
    );
    let env = server_env(&output);
    assert_eq!(env["SERVER_TOKEN"], ENV_VALUE);
    assert!(!env.contains_key("MCP_TOKEN"));
}

#[test]
fn the_service_account_token_reaches_only_op() {
    let cli = Cli::new();
    let mut stub = cli.op_stub();
    // The desktop app is locked; only the service account signs in.
    stub.set_mode(OpStubMode::Locked)
        .set_service_account_token(TOKEN);
    let mut set = cli
        .command()
        .args(["--home"])
        .arg(&cli.home)
        .args(["secrets", "service-account", "set"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    set.stdin
        .take()
        .unwrap()
        .write_all(format!("{TOKEN}\n").as_bytes())
        .unwrap();
    let output = set.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!String::from_utf8_lossy(&output.stdout).contains(TOKEN));

    let output = cli.exec(
        &args(
            &[
                "--ref",
                &format!("PAT={ITEM}"),
                "--op-env",
                ENVIRONMENT,
                "--map",
                "SERVER_TOKEN=MCP_TOKEN",
            ],
            &["env"],
        ),
        b"",
        // A token in the client's environment never reaches the server
        // either.
        &[("OP_SERVICE_ACCOUNT_TOKEN", TOKEN)],
    );
    let env = server_env(&output);
    assert_eq!(env["PAT"], ITEM_VALUE);
    assert_eq!(env["SERVER_TOKEN"], ENV_VALUE);
    assert!(!env.contains_key("OP_SERVICE_ACCOUNT_TOKEN"));
    assert!(!env.values().any(|value| value == TOKEN));

    // From the keyring alone: `op read` and `op run` both had it.
    let output = cli.exec(
        &args(
            &[
                "--ref",
                &format!("PAT={ITEM}"),
                "--op-env",
                ENVIRONMENT,
                "--map",
                "S=MCP_TOKEN",
            ],
            &["env"],
        ),
        b"",
        &[],
    );
    let env = server_env(&output);
    assert!(!env.contains_key("OP_SERVICE_ACCOUNT_TOKEN"));
    let calls = stub.calls();
    let seen = calls
        .iter()
        .filter(|call| *call == SERVICE_ACCOUNT_SEEN)
        .count();
    assert!(seen >= 4, "{calls:?}");
    assert!(calls.iter().all(|call| !call.contains(TOKEN)));

    let cleared = cli.ok(&["secrets", "service-account", "clear"]);
    assert_eq!(cleared["changed"], true);
    let output = cli.exec(
        &args(&["--ref", &format!("PAT={ITEM}")], &["env"]),
        b"",
        &[],
    );
    assert_eq!(output.status.code(), Some(77), "locked without the token");
}

#[test]
fn killing_exec_kills_the_server() {
    let cli = Cli::new();
    let mut child = cli
        .command()
        .arg("exec")
        .args(args(&[], &["hold"]))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert_eq!(line, "ready\n");
    child.kill().unwrap();
    child.wait().unwrap();
    // The server holds the write end of the pipe: end of file means it is
    // gone (Windows: the Job Object closed with `loadout exec`; Unix: exec
    // was the server).
    let (done, wait) = mpsc::channel();
    std::thread::spawn(move || {
        let mut rest = Vec::new();
        let _ = stdout.read_to_end(&mut rest);
        let _ = done.send(rest);
    });
    let rest = wait
        .recv_timeout(Duration::from_secs(20))
        .expect("the server outlived `loadout exec`");
    assert!(rest.is_empty());
}

#[test]
fn applied_launch_entries_start_the_server_with_its_secret() {
    let cli = Cli::new();
    cli.ok(&["secrets", "set", "GITHUB_PAT", "--ref", ITEM]);
    let server = server();
    let server = server.to_str().unwrap();
    let (code, added) = cli.json(&[
        "server",
        "add",
        "github",
        "--env",
        "GITHUB_PERSONAL_ACCESS_TOKEN=${secret:GITHUB_PAT}",
        "--env",
        "GITHUB_TOOLSETS=repos",
        "--client",
        "claude-code",
        "--",
        server,
        "env",
    ]);
    // No 1Password CLI here: the store edit commits, the entry is blocked
    // on this machine, and the add exits 5 like `apply`.
    assert_eq!(code, 5, "{added:#}");
    assert!(added["commit"].is_string());
    let entry = &added["apply"]["targets"][0]["entries"][0];
    assert_eq!(entry["state"], "blocked");
    assert_eq!(entry["problem"]["code"], "provider_unavailable");
    assert!(!cli.th.home().join(".claude.json").exists());
    let (code, applied) = cli.json(&["apply"]);
    assert_eq!(code, 5, "{applied:#}");

    let _stub = cli.op_stub();
    let applied = cli.ok(&["apply"]);
    assert_eq!(applied["targets"][0]["entries"][0]["delivery"], "launch");
    let text = std::fs::read_to_string(cli.th.home().join(".claude.json")).unwrap();
    assert!(!text.contains(ITEM_VALUE));
    let doc: Value = serde_json::from_str(&text).unwrap();
    let entry = &doc["mcpServers"]["github"];
    let command = entry["command"].as_str().unwrap();
    assert_eq!(
        std::fs::canonicalize(command).unwrap(),
        std::fs::canonicalize(BIN).unwrap(),
        "the absolute path of this binary"
    );
    let entry_args: Vec<&str> = entry["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect();
    assert_eq!(
        entry_args,
        [
            "exec",
            "--ref",
            &format!("GITHUB_PERSONAL_ACCESS_TOKEN={ITEM}"),
            "--",
            server,
            "env"
        ]
    );
    assert_eq!(entry["env"], json!({"GITHUB_TOOLSETS": "repos"}));

    // Start it as the client would.
    let output = cli
        .th
        .command(command)
        .args(&entry_args)
        .env("GITHUB_TOOLSETS", "repos")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let env = server_env(&output);
    assert_eq!(env["GITHUB_PERSONAL_ACCESS_TOKEN"], ITEM_VALUE);
    assert_eq!(env["GITHUB_TOOLSETS"], "repos");
}

#[test]
fn hook_commands_print_resolved_values() {
    let cli = Cli::new();
    let _stub = cli.op_stub();
    cli.keyring(&[("loadout/LINEAR_TOKEN", "fake-linear-token-0001")]);

    let output = cli.run(&["secret", "get", "--ref", ITEM]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(output.stdout, ITEM_VALUE.as_bytes());

    let output = cli.run(&[
        "secret",
        "get",
        "--ref",
        "LINEAR_TOKEN=keyring://loadout/LINEAR_TOKEN",
        "--template",
        "Bearer {secret:LINEAR_TOKEN}",
    ]);
    assert_eq!(output.stdout, b"Bearer fake-linear-token-0001");

    let output = cli.run(&[
        "secret-header",
        "--ref",
        "LINEAR_TOKEN=keyring://loadout/LINEAR_TOKEN",
        "--ref",
        &format!("PAT={ITEM}"),
        "Authorization=Bearer {secret:LINEAR_TOKEN}",
        "X-Pat={secret:PAT}",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let headers: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        headers,
        json!({"Authorization": "Bearer fake-linear-token-0001", "X-Pat": ITEM_VALUE})
    );

    let output = cli.run(&["secret", "get", "--ref", "keyring://loadout/ABSENT"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
    assert!(stderr(&output).contains("error[missing_secret]"));

    // A test keyring variable that cannot be honored refuses every call
    // instead of falling back to the OS keyring.
    let output = cli
        .command()
        .env("LOADOUT_TEST_KEYRING", "relative-keyring.json")
        .args(["--home"])
        .arg(&cli.home)
        .args(["secret", "get", "--ref", "keyring://loadout/LINEAR_TOKEN"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
    assert!(
        stderr(&output).contains("LOADOUT_TEST_KEYRING is set"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn env_export_prints_statements_per_shell() {
    let cli = Cli::new();
    let _stub = cli.op_stub();
    cli.keyring(&[("loadout/LINEAR_TOKEN", "fake-it's-0001")]);
    cli.ok(&["secrets", "set", "GITHUB_PAT", "--ref", ITEM]);
    cli.ok(&[
        "secrets",
        "set",
        "LINEAR_TOKEN",
        "--ref",
        "keyring://loadout/LINEAR_TOKEN",
        "--env-name",
        "LINEAR_API_KEY",
    ]);
    cli.ok(&[
        "secrets",
        "set",
        "MCP_TOKEN",
        "--ref",
        &format!("op-env://{ENVIRONMENT}/MCP_TOKEN"),
    ]);

    let output = cli.run(&["env", "--export", "--shell", "bash"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("export GITHUB_PAT='{ITEM_VALUE}'\nexport LINEAR_API_KEY='fake-it'\\''s-0001'\n")
    );
    assert!(stderr(&output).contains("MCP_TOKEN is a 1Password Environment variable"));
    let output = cli.run(&[
        "env",
        "--export",
        "--shell",
        "pwsh",
        "--aliases",
        "LINEAR_TOKEN",
    ]);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "$env:LINEAR_API_KEY = 'fake-it''s-0001'\n"
    );
    let output = cli.run(&[
        "env",
        "--export",
        "--shell",
        "fish",
        "--aliases",
        "LINEAR_TOKEN",
    ]);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "set -gx LINEAR_API_KEY 'fake-it\\'s-0001'\n"
    );
    // PowerShell ends a single-quoted string at typographic quotes too.
    cli.keyring(&[(
        "loadout/LINEAR_TOKEN",
        "fake-\u{2019}; Remove-Item x; '-0001",
    )]);
    let output = cli.run(&[
        "env",
        "--export",
        "--shell",
        "pwsh",
        "--aliases",
        "LINEAR_TOKEN",
    ]);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "$env:LINEAR_API_KEY = 'fake-\u{2019}\u{2019}; Remove-Item x; ''-0001'\n"
    );
    let output = cli.run(&["env", "--export", "--aliases", "UNDEFINED"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(output.stdout.is_empty());
}

#[test]
fn map_op_env_creates_aliases_for_selected_names() {
    let cli = Cli::new();
    let mut stub = cli.op_stub();
    // Without a selection: the names only, nothing stored.
    let listed = cli.ok(&["secrets", "map-op-env", ENVIRONMENT]);
    assert_eq!(listed["names"], json!(["MCP_TOKEN", "OTHER_SECRET"]));
    assert!(!cli.home.join("store").join("secrets.toml").exists());
    let output = cli.run(&["secrets", "map-op-env", ENVIRONMENT]);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(ENV_VALUE));

    cli.ok(&[
        "secrets",
        "set",
        "OTHER_SECRET",
        "--ref",
        "env:OTHER_SECRET",
    ]);
    let mapped = cli.ok(&["secrets", "map-op-env", ENVIRONMENT, "--all"]);
    assert_eq!(mapped["mapped"], json!(["MCP_TOKEN"]));
    assert_eq!(mapped["kept"], json!(["OTHER_SECRET"]));
    let aliases = cli.ok(&["secrets", "ls"])["aliases"].clone();
    assert_eq!(aliases[0]["alias"], "MCP_TOKEN");
    assert_eq!(
        aliases[0]["ref"],
        format!("op-env://{ENVIRONMENT}/MCP_TOKEN")
    );
    let catalog = std::fs::read_to_string(cli.home.join("store").join("secrets.toml")).unwrap();
    assert!(
        catalog.contains(&format!("environment = \"{ENVIRONMENT}\"")),
        "{catalog}"
    );
    assert!(!catalog.contains(ENV_VALUE));

    let (code, doc) = cli.json(&["secrets", "map-op-env", ENVIRONMENT, "--name", "NOPE"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("not_found"))
    );

    // 2.41: no listing; names are typed instead.
    stub.set_version("2.41.1");
    let (code, doc) = cli.json(&["secrets", "map-op-env", ENVIRONMENT, "--all"]);
    assert_eq!(
        (code, doc["error"]["code"].as_str()),
        (1, Some("provider_too_old"))
    );
    let mapped = cli.ok(&[
        "secrets",
        "map-op-env",
        ENVIRONMENT,
        "--name",
        "THIRD_TOKEN",
    ]);
    assert_eq!(mapped["mapped"], json!(["THIRD_TOKEN"]));
    assert!(stub.calls().iter().all(|call| !call.contains(ENV_VALUE)));
}

#[test]
fn check_lists_servers_clients_and_fixes() {
    let cli = Cli::new();
    cli.ok(&["secrets", "set", "GITHUB_PAT", "--ref", ITEM]);
    let (code, _) = cli.json(&[
        "server",
        "add",
        "github",
        "--env",
        "GITHUB_PERSONAL_ACCESS_TOKEN=${secret:GITHUB_PAT}",
        "--client",
        "claude-code",
        "--client",
        "codex",
        "--",
        "npx",
        "-y",
        "@modelcontextprotocol/server-github",
    ]);
    assert_eq!(code, 5, "the entry is blocked on this machine");
    let (code, doc) = cli.json(&["secrets", "check"]);
    assert_eq!(code, 5);
    let alias = &doc["aliases"][0];
    assert_eq!(alias["status"], "provider_unavailable");
    assert_eq!(alias["servers"], json!(["github"]));
    assert_eq!(alias["clients"], json!(["claude-code", "codex"]));
    let fixes: Vec<&str> = alias["fixes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|fix| fix.as_str().unwrap())
        .collect();
    assert!(
        fixes
            .iter()
            .any(|fix| fix.contains("secrets set GITHUB_PAT --keyring --this-machine")),
        "{fixes:?}"
    );
    let output = cli.run(&["secrets", "check"]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("needed by github (claude-code, codex)"),
        "{text}"
    );
    assert!(text.contains("fix: "), "{text}");
}

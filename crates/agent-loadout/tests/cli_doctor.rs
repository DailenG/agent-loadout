use std::path::{Path, PathBuf};
use std::process::Output;

use agent_loadout_testkit::TempHome;
use serde_json::{Value, json};

fn loadout(th: &TempHome, args: &[&str]) -> Output {
    th.command(env!("CARGO_BIN_EXE_loadout"))
        .args(args)
        .output()
        .expect("run loadout")
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "stdout is not JSON ({err}):\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn host_os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// The default `LOADOUT_HOME` for the host OS inside a `TempHome::command`.
fn host_default_home(th: &TempHome) -> PathBuf {
    if cfg!(windows) {
        th.local_appdata().join("Loadout")
    } else if cfg!(target_os = "macos") {
        th.home()
            .join("Library")
            .join("Application Support")
            .join("Loadout")
    } else {
        th.xdg().join("data").join("loadout")
    }
}

fn expected_paths(th: &TempHome, home: &Path, source: &str) -> Value {
    let (local, roaming) = if cfg!(windows) {
        (json!(s(&th.local_appdata())), json!(s(&th.appdata())))
    } else {
        (Value::Null, Value::Null)
    };
    json!({
        "os": host_os(),
        "profile": s(&th.home()),
        "local_app_data": local,
        "roaming_app_data": roaming,
        "loadout_home": s(home),
        "loadout_home_source": source,
        "store": s(&home.join("store")),
        "state": s(&home.join("state")),
        "backups": s(&home.join("backups")),
        "cache": s(&home.join("cache")),
        "imports": s(&home.join("imports")),
        "logs": s(&home.join("logs")),
    })
}

#[test]
fn version_prints_binary_name_and_version() {
    let th = TempHome::new();
    let output = loadout(&th, &["--version"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim_end(),
        format!("loadout {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn doctor_paths_json_reports_flag_home_and_layout() {
    let th = TempHome::new();
    let home = th.root().join("data").join("lh");
    let home_arg = s(&home);

    // `--json` and `--home` are global: accepted before or after the subcommand.
    for args in [
        vec!["--home", &home_arg, "doctor", "--paths", "--json"],
        vec!["--json", "doctor", "--paths", "--home", &home_arg],
    ] {
        let output = loadout(&th, &args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert_eq!(
            stdout_json(&output),
            json!({
                "schema": 1,
                "paths": expected_paths(&th, &home, "flag"),
                "findings": [],
            }),
            "{args:?}"
        );
    }
    assert!(!home.exists(), "doctor must not create LOADOUT_HOME");
}

#[test]
fn doctor_default_home_uses_os_folders() {
    let th = TempHome::new();
    let mut command = th.command(env!("CARGO_BIN_EXE_loadout"));
    command.args(["doctor", "--paths", "--json"]);
    if cfg!(windows) {
        // A Git Bash style HOME must not move the profile or the data dir.
        command.env("HOME", th.root().join("gitbash-home"));
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout_json(&output)["paths"],
        expected_paths(&th, &host_default_home(&th), "default")
    );
}

#[test]
fn doctor_honors_loadout_home_var() {
    let th = TempHome::new();
    let home = th.root().join("from-env");
    let output = th
        .command(env!("CARGO_BIN_EXE_loadout"))
        .args(["doctor", "--paths", "--json"])
        .env("LOADOUT_HOME", &home)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout_json(&output)["paths"],
        expected_paths(&th, &home, "env")
    );
}

#[test]
fn doctor_reports_sync_root_detected() {
    let th = TempHome::new();
    let sync = th.root().join("Sync");
    std::fs::create_dir_all(sync.join(".sync")).unwrap();
    std::fs::write(sync.join(".sync").join("ID"), b"").unwrap();
    let home = s(&sync.join("loadout"));

    let output = loadout(&th, &["--home", &home, "doctor", "--json"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout_json(&output),
        json!({
            "schema": 1,
            "findings": [{
                "code": "sync_root_detected",
                "severity": "warning",
                "kind": "resilio",
                "root": s(&sync),
            }],
        })
    );

    let output = loadout(&th, &["--home", &home, "doctor"]);
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("warning[sync_root_detected]") && text.contains("Resilio Sync"),
        "{text}"
    );
}

#[test]
fn relative_loadout_home_var_fails_with_invalid_path() {
    let th = TempHome::new();
    let output = th
        .command(env!("CARGO_BIN_EXE_loadout"))
        .args(["doctor", "--json"])
        .env("LOADOUT_HOME", "relative")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let doc = stdout_json(&output);
    assert_eq!(doc["schema"], 1);
    assert_eq!(doc["error"]["code"], "invalid_path");
    assert_eq!(doc["error"]["path"], "relative");
}

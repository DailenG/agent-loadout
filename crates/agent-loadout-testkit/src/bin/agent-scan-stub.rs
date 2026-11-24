//! Fake Snyk Agent Scan for tests. Installed as the scanner by
//! `agent_loadout_testkit::agent_scan_stub::AgentScanStub`, which writes the
//! `agent-scan-stub.json` next to it. Logs every call (arguments, the whole
//! environment, bytes on stdin, the files below the scanned path) to
//! `agent-scan-stub.log`, then prints the configured stdout and exits with
//! the configured code; with `hang`, starts a heartbeat child and never
//! exits.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use agent_loadout_testkit::agent_scan_stub::{
    CONFIG_FILE, HEARTBEAT_ARG, LOG_FILE, StubCall, StubConfig,
};

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some(HEARTBEAT_ARG) {
        heartbeat(argv.get(1).map(Path::new));
    }
    let dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(ToOwned::to_owned))
        .expect("stub directory");
    let mut stdin = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut stdin);
    let files = argv
        .last()
        .map(|path| list(Path::new(path)))
        .unwrap_or_default();
    let call = StubCall {
        env: std::env::vars().collect::<BTreeMap<_, _>>(),
        argv,
        stdin_bytes: stdin.len(),
        files,
    };
    if let Ok(mut log) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(LOG_FILE))
    {
        let _ = writeln!(log, "{}", serde_json::to_string(&call).expect("call"));
    }
    let config: StubConfig = std::fs::read_to_string(dir.join(CONFIG_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    if config.hang {
        let exe = std::env::current_exe().expect("current exe");
        let mut child = Command::new(exe);
        child.arg(HEARTBEAT_ARG);
        if let Some(file) = &config.heartbeat {
            child.arg(file);
        }
        // The grandchild inherits stdout and stderr, as PyInstaller's
        // child does: the pipes stay open until the whole tree is gone.
        let _ = child.stdin(Stdio::null()).spawn();
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    let mut out = std::io::stdout();
    let _ = out.write_all(config.stdout.as_bytes());
    let pad = vec![b' '; 64 * 1024];
    let mut left = config.pad;
    while left > 0 {
        let n = left.min(pad.len());
        if out.write_all(&pad[..n]).is_err() {
            break;
        }
        left -= n;
    }
    let _ = out.flush();
    match &config.stderr {
        Some(text) => eprint!("{text}"),
        None => eprintln!("agent-scan stub: scan finished"),
    }
    ExitCode::from(u8::try_from(config.exit).unwrap_or(1))
}

/// Writes a growing counter to `file` every 50 ms, forever.
fn heartbeat(file: Option<&Path>) -> ! {
    let mut beat = 0u64;
    loop {
        beat += 1;
        if let Some(file) = file {
            let _ = std::fs::write(file, beat.to_string());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Files below `root`, relative and `/`-separated, sorted.
fn list(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    out
}

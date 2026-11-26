//! Fake SkillSpector, uvx and uv for tests. Copied into a `bin` folder by
//! `agent_loadout_testkit::skillspector_stub::SkillSpectorStub` under the
//! name it acts as; reads `skillspector-stub.json` beside it, logs every
//! call (program, arguments, the whole environment, bytes on stdin, the
//! files below the last argument) to `skillspector-stub.log`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use agent_loadout_testkit::skillspector_stub::{
    CONFIG_FILE, HEARTBEAT_ARG, LOG_FILE, PREPARED_FILE, SKILL_ESCAPED_PLACEHOLDER,
    SKILL_PLACEHOLDER, StubCall, StubConfig, UV_OFFLINE_ERROR,
};

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some(HEARTBEAT_ARG) {
        heartbeat(argv.get(1).map(Path::new));
    }
    let exe = std::env::current_exe().expect("current exe");
    let dir = exe.parent().expect("stub directory").to_owned();
    let config: StubConfig = std::fs::read_to_string(dir.join(CONFIG_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    if let Some(file) = &config.early_heartbeat {
        // First thing, as `uvx` starts `uv`: only a job joined before the
        // process runs at all holds this child.
        let _ = Command::new(&exe)
            .arg(HEARTBEAT_ARG)
            .arg(file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    let program = exe
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let mut stdin = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut stdin);
    let files = argv
        .last()
        .map(|path| list(Path::new(path)))
        .unwrap_or_default();
    let call = StubCall {
        program: program.clone(),
        env: std::env::vars().collect::<BTreeMap<_, _>>(),
        argv: argv.clone(),
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
    let rest: Vec<String> = if program == "uvx" || program == "uv" {
        let mut args = argv.as_slice();
        if program == "uv" {
            if args.get(..2) != Some(&["tool".to_owned(), "run".to_owned()]) {
                eprintln!("uv stub: expected `tool run`");
                return ExitCode::from(2);
            }
            args = &args[2..];
        }
        // `--from skillspector==X skillspector`: SkillSpector's arguments
        // follow the command name.
        let Some(from) = args.iter().position(|arg| arg == "--from") else {
            eprintln!("uv stub: no --from");
            return ExitCode::from(2);
        };
        if args.get(from + 2).map(String::as_str) != Some("skillspector") {
            eprintln!("uv stub: no skillspector command");
            return ExitCode::from(2);
        }
        let offline = args[..from].iter().any(|arg| arg == "--offline");
        let prepared = dir.join(PREPARED_FILE);
        if offline {
            if !prepared.is_file() {
                eprint!("{UV_OFFLINE_ERROR}");
                return ExitCode::from(2);
            }
        } else if !prepared.is_file() {
            for line in &config.prepare_lines {
                eprintln!("{line}");
            }
            if config.prepare_hang {
                hang(&config);
            }
            if config.prepare_exit != 0 {
                eprintln!("error: Failed to fetch: `https://pypi.org/simple/skillspector/`");
                return ExitCode::from(u8::try_from(config.prepare_exit).unwrap_or(2));
            }
            let _ = std::fs::write(&prepared, b"");
        }
        args[from + 3..].to_vec()
    } else {
        argv
    };

    if rest.first().map(String::as_str) == Some("--version") {
        println!("{}", config.version);
        return ExitCode::from(u8::try_from(config.version_exit).unwrap_or(1));
    }
    if rest.first().map(String::as_str) != Some("scan") {
        eprintln!("skillspector stub: unknown command {rest:?}");
        return ExitCode::from(2);
    }
    if config.hang {
        hang(&config);
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
        Some(text) => {
            let skill = rest.last().map(String::as_str).unwrap_or_default();
            let text = text
                .replace(SKILL_ESCAPED_PLACEHOLDER, &skill.replace('\\', "\\\\"))
                .replace(SKILL_PLACEHOLDER, skill);
            eprint!("{text}");
        }
        None => eprintln!("skillspector stub: scan finished"),
    }
    ExitCode::from(u8::try_from(config.exit).unwrap_or(1))
}

/// Starts a heartbeat child (inheriting stdout and stderr, so the pipes
/// stay open until the whole tree is gone), then never exits.
fn hang(config: &StubConfig) -> ! {
    let exe = std::env::current_exe().expect("current exe");
    let mut child = Command::new(exe);
    child.arg(HEARTBEAT_ARG);
    if let Some(file) = &config.heartbeat {
        child.arg(file);
    }
    let _ = child.stdin(Stdio::null()).spawn();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Writes a growing counter to `file` every 50 ms, forever. Each value is
/// written beside it and renamed over it, so a kill mid-write never
/// leaves the file empty.
fn heartbeat(file: Option<&Path>) -> ! {
    let mut beat = 0u64;
    loop {
        beat += 1;
        if let Some(file) = file {
            let next = file.with_extension("next");
            if std::fs::write(&next, beat.to_string()).is_ok() {
                let _ = std::fs::rename(&next, file);
            }
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

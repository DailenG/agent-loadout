//! A fake stdio MCP server for `loadout exec` tests. The first argument
//! picks the mode:
//!
//! - none or `serve`: newline-delimited JSON-RPC 2.0. Answers
//!   `initialize`, `tools/list` and `tools/call` of its one tool `env`
//!   (the value of one environment variable, or `null`); ignores
//!   notifications; anything else is a `-32601` error.
//! - `echo`: copies stdin to stdout byte for byte until end of input.
//! - `env`: prints every environment variable as one JSON object.
//! - `exit <N>`: exits with code N.
//! - `hold`: prints `ready`, then waits (up to a minute) to be killed.

use std::io::{BufRead, Read, Write};
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["serve"] => serve(),
        ["echo"] => echo(),
        ["env"] => env(),
        ["exit", code] => ExitCode::from(code.parse::<u8>().unwrap_or(1)),
        ["hold"] => hold(),
        _ => {
            eprintln!("fake-mcp-server: unknown arguments {args:?}");
            ExitCode::from(64)
        }
    }
}

fn serve() -> ExitCode {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    let mut line = Vec::new();
    let mut input = stdin.lock();
    loop {
        line.clear();
        match input.read_until(b'\n', &mut line) {
            Ok(0) => return ExitCode::SUCCESS,
            Ok(_) => {}
            Err(_) => return ExitCode::from(1),
        }
        let Ok(request) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let method = request["method"].as_str().unwrap_or_default();
        let response = match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "fake-mcp-server", "version": "0.0.0"}
                }
            }),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"tools": [{
                    "name": "env",
                    "description": "The value of one environment variable",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"name": {"type": "string"}},
                        "required": ["name"]
                    }
                }]}
            }),
            "tools/call" if request["params"]["name"] == "env" => {
                let name = request["params"]["arguments"]["name"]
                    .as_str()
                    .unwrap_or_default();
                let value = std::env::var(name).ok();
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {"content": [{"type": "text", "text": json!(value).to_string()}]}
                })
            }
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "method not found"}
            }),
        };
        let mut bytes = serde_json::to_vec(&response).expect("JSON serializes");
        bytes.push(b'\n');
        if stdout
            .write_all(&bytes)
            .and_then(|()| stdout.flush())
            .is_err()
        {
            return ExitCode::from(1);
        }
    }
}

fn echo() -> ExitCode {
    let mut input = Vec::new();
    if std::io::stdin().read_to_end(&mut input).is_err() {
        return ExitCode::from(1);
    }
    let mut stdout = std::io::stdout().lock();
    match stdout.write_all(&input).and_then(|()| stdout.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(1),
    }
}

fn env() -> ExitCode {
    let vars: serde_json::Map<String, Value> = std::env::vars_os()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                Value::String(value.to_string_lossy().into_owned()),
            )
        })
        .collect();
    println!("{}", Value::Object(vars));
    ExitCode::SUCCESS
}

fn hold() -> ExitCode {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(b"ready\n").and_then(|()| stdout.flush());
    std::thread::sleep(Duration::from_secs(60));
    ExitCode::SUCCESS
}

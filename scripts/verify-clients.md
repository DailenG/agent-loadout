# Real-client verification

Manual checks for the maintainer at gates V1 (Tier A) and V2 (Tier B), per
SPEC.md section 19 item 10. The implementer never runs these. Each check
names the `[verify]` marker (or the assumption in code) it clears. Record
results in `docs/verify/<client>.md`; a failure becomes a fix brief before
M9.

## Ground rules

- Use a throwaway `LOADOUT_HOME` and, where the client supports it, a
  throwaway config dir (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
  `PI_CODING_AGENT_DIR`). Back up any real client file a check touches
  before starting, and restore it afterwards.
- Run each client's own lister to confirm what it loaded (`claude mcp list`,
  the Claude Desktop developer settings, `codex mcp list`, omp `/mcp list`,
  `pi mcp list`).
- Write Loadout's entries with `loadout server add <id> --client <client>`
  (instant apply, M6) against the throwaway `LOADOUT_HOME`, or by hand from
  the goldens in
  `crates/agent-loadout-core/tests/snapshots/adapters_claude__*.snap` and
  `adapters_tier_a__*.snap`.
- Use synthetic server names and placeholder commands only (for example the
  `example-*` names from the fixtures, or a real public test server); never
  paste tokens into a client file for these checks.
- PowerShell helpers below are saved as `.ps1` files and run with
  `pwsh -File <name>.ps1`.
- Pi moved to Tier B on 2026-11-21. Its section, and the Pi parts of
  OMP-10, AP-3, AP-4, AP-5, DX-1 and DX-6, run at gate V2; at V1 skip them
  and record them as deferred.

## Claude Code (`claude-code`), gate V1

Machines: Windows 11 and Arch Linux.

| # | Check | Expected | Clears |
|---|---|---|---|
| CC-1 | Set `CLAUDE_CONFIG_DIR` to an empty throwaway dir, start `claude` once, quit. List the dir. | Claude Code creates and reads `$CLAUDE_CONFIG_DIR/.claude.json`; `~/.claude.json` is not touched. A server added to `mcpServers` in that file shows in `claude mcp list`. | SPEC 7.3 `$CLAUDE_CONFIG_DIR/.claude.json` `[verify]`; `claude_code::USER_FILE` |
| CC-2 | Windows: in the CC-1 setup, open a project at `C:\loadout-verify\app` (accept the trust dialog), quit, read the `projects` keys in `.claude.json`. | Record the exact key text: slash style (`C:/loadout-verify/app` or `C:\\loadout-verify\\app`) and drive letter case. Loadout writes `/` for a new key when the file has no drive-letter key yet. | SPEC 7.3 project key format `[verify]`; SPEC 20.3 M4 default style `[verify]`; `location::project_key` |
| CC-3 | Windows: hand-add `projects["<key from CC-2>"].mcpServers.verify-local` (a stdio entry), run `claude mcp list` inside the project. Repeat with the key rewritten in the other slash style and other drive letter case. | The entry is listed under the key Claude Code wrote. Record whether the other spellings are also honored (Loadout reuses whatever key exists, ignoring slash style, case and a trailing separator). | `location::project_key` matching rules |
| CC-4 | Add a user-scope server `verify-user`, then add `"verify-user"` to `projects["<P>"].disabledMcpServers`. Run `claude mcp list` inside and outside `<P>`. | Disabled (or absent) inside `<P>`, enabled elsewhere. | SPEC 7.4 project suppression via `disabledMcpServers` |
| CC-5 | Add `"verify-bare": {"url": "https://example.com/mcp"}` (no `type`). | Record whether Claude Code loads it, and as which transport. Loadout reads a bare `url` as http (SPEC 7.5) but never treats it as equal to the typed entry it writes. | SPEC 7.3 "`type` required for remote entries" |
| CC-6 | Add an entry with `"type": "ws"`. | Record whether it is accepted. Loadout reads it as `unsupported_transport` (the store has no `ws`). | SPEC 7.4 transport list |
| CC-7 | Hand-add an entry keyed `verify.dotted`. | Record whether Claude Code loads it. Loadout blocks such names for Claude Code (`invalid_id`, fix: `native_name`). | SPEC 5.1 name rule `[A-Za-z0-9_-]` |
| CC-8 | Add an `sse` entry with `headersHelper` pointing to a script that prints `{"X-Verify": "1"}`. | Record whether the helper runs for `sse` (the spec names http and ws). | SPEC 7.4 `headersHelper` (http/ws) |
| CC-9 | Windows: add a stdio entry with plain `"command": "npx"` (no `cmd /c`), for example `npx -y @modelcontextprotocol/server-everything`. | Record whether it connects. Loadout writes plain `npx` for Claude Code (`windows_shell_shim = false`, SPEC 7.4); if it fails, the flag must be set for `claude-code` too. | SPEC 7.4 notes (no shim listed for claude-code) |
| CC-10 | Put a server in `<P>/.mcp.json` and start `claude` in `<P>`. | An approval prompt appears before the server runs. | SPEC 7.4 trust gate `.mcp.json` approval |
| CC-11 | Detection: with only `claude.exe` (native installer) and, separately, only `claude.cmd` (npm global) on `PATH`. | Both are real install shapes; the PATH probe looks for `claude` plus each `PATHEXT` extension. | SPEC 7.7, `Probe::Executable("claude")` |

## Claude Desktop (`claude-desktop`), gate V1

Machine: Windows 11 (Decision 12). macOS is best effort; Linux has no target.

| # | Check | Expected | Clears |
|---|---|---|---|
| CD-1 | Classic (non-MSIX) install: add a stdio entry to `%APPDATA%\Claude\claude_desktop_config.json`, fully quit the app (tray icon, Quit), start it again. | The server is listed in Settings, Developer. It is not picked up before the restart. | SPEC 7.3 Roaming path; `restart_required` |
| CD-2 | MSIX install: list `%LOCALAPPDATA%\Packages` for `Claude_*`. With the app having saved settings once, check for `LocalCache\Roaming\Claude\claude_desktop_config.json` inside it. Add a different entry to each copy, restart. | Record the package dir name prefix (Loadout matches `Claude_*`), which copy the app reads, and that writing both is harmless. | SPEC 7.3 MSIX copy and "write both, warn"; `claude_desktop::MSIX_CONFIG` |
| CD-3 | Run `uninstall-names.ps1` (below) on a machine with Claude Desktop installed, both installer kinds if possible. | Record the exact `DisplayName`. Loadout's registry probe looks for `Claude`. | `Probe::UninstallEntry("Claude")` `[verify]` |
| CD-4 | Add `"command": "npx"` without a wrapper, then the Loadout form `"command": "cmd", "args": ["/c", "npx", ...]`. | The plain form fails to start, the wrapped form starts. | SPEC 5.2 `windows_shell_shim` |
| CD-5 | Add an entry with `"type": "http"` and a `url`. | Not loaded (remote servers are Connectors). Loadout blocks http and sse as `unsupported_transport`. | SPEC 7.3 stdio only |
| CD-6 | After CD-1, change an app setting in the UI, then diff `claude_desktop_config.json` against your last edit. | Record whether the app rewrites the file at runtime and whether it keeps unknown keys and formatting. If it rewrites, `rewrites_file_at_runtime` must be set. | `claude_desktop` caps `rewrites_file_at_runtime = false` |
| CD-7 | macOS: repeat CD-1 with `~/Library/Application Support/Claude/claude_desktop_config.json`. | Same as CD-1. | SPEC 7.3 macOS best effort |

`uninstall-names.ps1` (read-only, ASCII only):

```powershell
$roots = @(
    'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
    'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
    'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall'
)
foreach ($root in $roots) {
    if (-not (Test-Path $root)) { continue }
    foreach ($key in Get-ChildItem $root) {
        $name = (Get-ItemProperty -Path $key.PSPath -ErrorAction SilentlyContinue).DisplayName
        if ($name -and $name -like '*Claude*') {
            Write-Output ("{0}`t{1}" -f $key.PSChildName, $name)
        }
    }
}
```

## Codex (`codex`), gate V1

Machines: Windows 11 and Arch Linux. Use `CODEX_HOME` pointing at a
throwaway dir for every check.

| # | Check | Expected | Clears |
|---|---|---|---|
| CX-1 | Set `CODEX_HOME` to an empty throwaway dir, add `[mcp_servers.verify-user]` (stdio) to `$CODEX_HOME/config.toml`, run `codex mcp list`. | The server is listed; `~/.codex` is not read. | SPEC 7.3 `$CODEX_HOME/config.toml`; `codex::USER_FILE` |
| CX-2 | Put `[mcp_servers.verify-project]` in `<P>/.codex/config.toml`. Run `codex mcp list` in `<P>` before and after marking `<P>` trusted. | Listed only once `<P>` is trusted. | SPEC 7.3 trusted projects; `TrustGate::TrustedProject` |
| CX-3 | With `verify-user` from CX-1, add only `[mcp_servers.verify-user]` `enabled = false` to the trusted `<P>/.codex/config.toml`. Run `codex mcp list` inside and outside `<P>`. Then replace it with a project table holding only `args = ["--other"]`. | Disabled inside `<P>`, enabled elsewhere. Record whether the `args`-only table merges over the user entry (deep merge) or replaces it. | SPEC 7.4 project `enabled = false` `[verify merge]`; DeepMerge `[verify]`; `ProjectDisable::EnabledFalse { partial: true }` |
| CX-4 | Add `env_vars = ["VERIFY_FORWARD"]` to a stdio entry whose server reports its environment (a small script), set `VERIFY_FORWARD=1` in the shell, start `codex`. | The server sees `VERIFY_FORWARD=1`. | SPEC 11.4 Codex `env` delivery (`env_vars`) |
| CX-5 | Point an http entry at a local echo endpoint with `bearer_token_env_var = "VERIFY_BEARER"` and `env_http_headers = { "X-Verify" = "VERIFY_HEADER" }`, both set in the shell. | Requests carry `Authorization: Bearer <value>` and `X-Verify: <value>`. | SPEC 7.3 Codex remote fields |
| CX-6 | Add `[mcp_servers."verify.dotted"]`. | Record whether Codex loads it. Loadout blocks names outside `[A-Za-z0-9_-]` for Codex (`invalid_id`, fix: `native_name`). | `codex::CodexToml` name rule `[verify]` |
| CX-7 | Add `[mcp.servers.verify-legacy]` (stdio) and run `codex mcp list`. | Record whether current Codex still loads the legacy table. Loadout reads it for import and never edits or deletes it. | SPEC 7.3 legacy `[mcp.servers]` read-only |
| CX-8 | Windows: a stdio entry with plain `command = "npx"` (no `cmd /c`). | Record whether it connects. Loadout writes plain `npx` for Codex (`windows_shell_shim = false`). | SPEC 7.4 notes (no shim listed for codex) |
| CX-9 | Add `enabled_tools = ["<one tool>"]` to a server, then replace it with `disabled_tools = ["<that tool>"]`. | Only that tool is offered, then every tool but it. | SPEC 7.3 tool lists; `ToolFilterMech::AllowDeny` |
| CX-10 | Set `startup_timeout_sec = 1` on a slow-starting server, then `2.5`. | Startup times out after about 1 s; a fractional value is accepted. | `TimeoutField` seconds for `startup_timeout_sec`, `tool_timeout_sec` |
| CX-11 | Detection: `codex.exe` (standalone) and, separately, `codex.cmd` (npm global) on `PATH`. | Both are real install shapes; the PATH probe tries each `PATHEXT` extension. | SPEC 7.7, `Probe::Executable("codex")` |

## oh-my-pi (`omp`), gate V1

Machines: Windows 11 and Arch Linux. Use `PI_CODING_AGENT_DIR` (default
profile) or a throwaway `PI_CONFIG_DIR` name for every check.

| # | Check | Expected | Clears |
|---|---|---|---|
| OMP-1 | Set `PI_CODING_AGENT_DIR` to a throwaway dir, add `verify-user` to `$PI_CODING_AGENT_DIR/mcp.json`, run `/mcp list`. | Listed, from that file. | SPEC 7.3 `PI_CODING_AGENT_DIR`; `omp::USER_FILE` |
| OMP-2 | Unset it, set `PI_CONFIG_DIR=.omp-verify`, add `verify-root` to `~/.omp-verify/agent/mcp.json`, run `/mcp list`. Repeat with an absolute `PI_CONFIG_DIR`. | The relative name is a directory under home. Record what omp does with an absolute value (Loadout ignores absolute values and values with `..`, falling back to `.omp`). | SPEC 7.3 `PI_CONFIG_DIR`; `Location::HomeNamedDir` |
| OMP-3 | Keep `PI_CODING_AGENT_DIR` set, start `omp --profile verify`, add `verify-profile` to `~/.omp/profiles/verify/agent/mcp.json`, run `/mcp list`. | Lists `verify-profile`, not `verify-user`: a named profile ignores `PI_CODING_AGENT_DIR`. | SPEC 7.3 named profiles; `omp_profile_targeting_from_clients_toml` |
| OMP-4 | With `verify-user` in the user file, put the whole entry with `"enabled": false` in `<P>/.omp/mcp.json`; run `/mcp list` inside and outside `<P>`. Then replace it with `{ "enabled": false }` alone. | The full disabled entry suppresses `verify-user` inside `<P>`. Record whether the `enabled`-only entry is accepted and suppresses too (Loadout writes the full entry because the project entry replaces the user one). | SPEC 7.4 project `enabled: false`; `ProjectDisable::EnabledFalse { partial: false }` `[verify]` |
| OMP-5 | Start from no file, write the `omp_new_file` golden (with the `$schema` line) by hand, run `/mcp list`, then `/mcp disable notes` and diff the file. | Loads. Record whether omp's own write keeps every other key and entry (it rewrites the file with `JSON.stringify`, two-space indent). | SPEC 7.2 omp skeleton; `rewrites_file_at_runtime = false` |
| OMP-6 | Set `"timeout": 1000` on a tool that takes longer than a second. | The call times out after about 1 s (milliseconds). | SPEC 7.3 `timeout` (ms); `TimeoutField` milliseconds |
| OMP-7 | Add `"env": { "VERIFY": "!echo verified" }` to a stdio server that reports its environment. | The server sees `VERIFY=verified`. | SPEC 7.4 `!command`; `CommandRef::Bang` |
| OMP-8 | Windows: a stdio entry with plain `"command": "npx"`. | Record whether it connects (`windows_shell_shim = false`). | SPEC 7.4 notes |
| OMP-9 | Add `verify-user` to the user file and also to `disabledServers`; then remove it from the list and add it to `enabledServers` with `"enabled": false` in the entry. | Hidden, then force-enabled. Loadout never edits either list. | SPEC 7.3 `disabledServers`/`enabledServers` preserved |
| OMP-10 | With both omp and Pi installed, set `PI_CODING_AGENT_DIR` and start each. | Record whether both really read `$PI_CODING_AGENT_DIR/mcp.json`. If so, the two clients share one file whose `timeout` units differ (omp ms, Pi s); the planner must treat it as a shared file (SPEC 6.5). | `omp::USER_FILE`, `pi` agent dir |
| OMP-11 | Detection: `omp` installed with Bun (`omp.exe` or `omp.cmd` on `PATH`), and a `~/.omp` dir alone. | Either detects omp. | SPEC 7.7, `Probe::Executable("omp")`, `Probe::Path(~/.omp)` |

## Pi (`pi`), gate V2

Machines: Windows 11 and Arch Linux. Use `PI_CODING_AGENT_DIR` for every
check.

| # | Check | Expected | Clears |
|---|---|---|---|
| PI-1 | Set `PI_CODING_AGENT_DIR` to an absolute throwaway dir, add `verify-user` to `$PI_CODING_AGENT_DIR/mcp.json`, run `pi mcp list`. Repeat with a `~/`-prefixed value. | Listed, from that file. Pi expands `~`; record it. Loadout uses only absolute values and otherwise falls back to `~/.pi/agent`. | SPEC 19 `PI_CODING_AGENT_DIR` `[verify]`; `pi` agent dir |
| PI-2 | Put `verify-project` in `<P>/.pi/mcp.json`; run `pi mcp list` in `<P>` before and after trusting it. | Listed only after trust. | SPEC 7.3 project trust; `TrustGate::TrustedProject` |
| PI-3 | With `verify-user` in the user file, put `"verify-user": { "enabled": false }` in the trusted `<P>/.pi/mcp.json`. | Disabled inside `<P>`, enabled elsewhere, with the user entry's other fields kept. | SPEC 7.4 project `{ "enabled": false }`; `ProjectDisable::EnabledFalse { partial: true }` |
| PI-4 | Add both `verify-tool` and `verify_tool`. | Pi rejects the second as the same server. Loadout reports `collision` for such a pair. | SPEC 5.1, 20.3 M5 `-`/`_` equivalence |
| PI-5 | Add an http entry without `type`, one with `"type": "streamable-http"` and one with `"type": "sse"`. | The first two connect over streamable HTTP; `sse` is rejected. Loadout writes no `type` and blocks SSE. | SPEC 7.3 optional `type`, no SSE |
| PI-6 | Set `"timeout": 1` on a tool that takes longer than a second. | The call times out after about 1 s (seconds). | SPEC 7.3 `timeout` (s); `TimeoutField` seconds |
| PI-7 | Add `"env": { "VERIFY": "!echo verified" }` and `"headers": { "X-Verify": "!echo verified" }`. | The server sees the command output. Also record whether a literal value that starts with `!` can be escaped: Loadout writes values as rendered, so a literal `!` value would run as a command. | SPEC 7.4 `!command`; `CommandRef::Bang` |
| PI-8 | Install `pi-mcp-adapter` the documented way (`pi install npm:pi-mcp-adapter`), then with `-l` in `<P>`. Record the change to `~/.pi/agent/settings.json` and `<P>/.pi/settings.json`, and any files under `extensions/`. Start a session and run `/mcp`. | Pi's own MCP no longer reads `mcp.json`. Loadout reports `client_mcp_replaced_by_extension` from a `packages` entry naming `pi-mcp-adapter` or an `extensions/pi-mcp-adapter[.ts/.js]` path; record any other install shape. | SPEC 7.3 `pi-mcp-adapter`; `pi::MCP_ADAPTER` `[verify]` |
| PI-9 | Set `"extensions": ["-builtin:mcp"]` in the user settings. | Pi stops loading MCP without a replacement. Loadout does not report this yet; record it for a follow-up brief. | SPEC 7.3 (not covered by Loadout) |
| PI-10 | Windows: a stdio entry with plain `"command": "npx"`. | Record whether it connects (`windows_shell_shim = false`). | SPEC 7.4 notes |
| PI-11 | Map `[tools] deny` by hand: `"toolExposure": { "<tool>": "hidden" }`. | The tool is unreachable. Loadout does not write `toolExposure` yet (`Caps.tool_filter = None` for Pi); this confirms the mapping for a later brief. | SPEC 5.2 `[tools]` via Pi `toolExposure` |
| PI-12 | Detection: `pi.exe`/`pi.cmd` on `PATH`, and a `~/.pi/agent` dir alone. | Either detects Pi. | SPEC 7.7, `Probe::Executable("pi")` |

## Apply engine (M6), gate V1

Machines: Windows 11 and Arch Linux. Every check uses the throwaway
`LOADOUT_HOME` and override dirs from the client sections. None of these is
a `[verify]` marker of SPEC 8; each checks an assumption the apply engine
makes about running clients.

| # | Check | Expected | Clears |
|---|---|---|---|
| AP-1 | In the CC-1 setup, start `claude` and keep it running. Run `loadout server add verify-apply --client claude-code -- npx -y @modelcontextprotocol/server-everything`. Use Claude Code for a minute (start a new conversation, change a setting), quit, then run `loadout status --client claude-code`. | `verify-apply` is still `synced`. If Claude Code's own rewrite of `.claude.json` dropped it, status shows `missing` (the next apply re-creates it); record when that happens. | `claude_code` caps `rewrites_file_at_runtime = true`; SPEC 8.4 compare-and-swap |
| AP-2 | Windows: give a throwaway client file an explicit ACE and the hidden attribute (`icacls <file> /grant "<a local test group>:(R)"`, `attrib +h <file>`), record `icacls <file>` and `attrib <file>`, run `loadout apply` with a change for that file, record both again. | Same ACEs and attributes before and after. Loadout replaces files by rename after copying the original DACL and attributes onto the temp file (no `ReplaceFileW`, see `docs/milestones/M6.md`). | SPEC 8.4 step 4 "keeps ACLs and attributes"; `RealFs::write_new` and `RealFs::replace` |
| AP-3 | Windows: with each Tier A client running, run `loadout apply` with a change to that client's file. | Record any client that keeps its config file open without delete sharing: Loadout then retries for 2 s and reports `file_locked` for that file only. | SPEC 8.4 step 4 sharing-violation retry |
| AP-4 | Add two stdio servers that print their environment and arguments, for `claude-code`, `omp`, `pi` and `codex`: `verify-env` with `--env VERIFY_VALUE='${env:VERIFY_VALUE}'`, and `verify-arg` with the argument `'--tag=${env:VERIFY_VALUE}'`. Set `VERIFY_VALUE=verified` in the shell, start each client, connect both. | `verify-env`: Claude Code, omp and Pi get `${VERIFY_VALUE}` in `env`, Codex gets `env_vars = ["VERIFY_VALUE"]`; each server sees `verified`. `verify-arg`: Claude Code and omp get `${VERIFY_VALUE}` in `args` and pass `verified`; Pi and Codex do not expand arguments and `loadout exec` sets only `env`, so Loadout writes the value from its own environment and warns `inline_env_fallback` (M8; without `VERIFY_VALUE` in Loadout's environment the entry is blocked `missing_secret`). | `render.rs` `env` delivery per `Caps::env_ref` and `env_ref_fields`; SPEC 11.4 rule 1 |
| AP-5 | Hand-edit the throwaway store's `clients.toml` to `[omp] native_disable = true` (and the same for `pi`, `codex`), add a server for those clients, then run `loadout server rm <id>`. | Each entry stays with `enabled = false` and the client's lister shows it disabled. | SPEC 8.4 "native disable mechanism when clients.toml asks"; `ClientPrefs::native_disable` |
| AP-6 | Windows: apply a server for `claude-desktop` while Claude Desktop runs. | The server appears only after a full restart. Loadout does not prompt for the restart yet (`restart_required` is not surfaced by the CLI); record whether it should. | `claude_desktop` caps `restart_required` |
| AP-7 | Hand-add to each client file one entry with a `${VAR}` value, one with `${VAR:-default}`, one with the literal env value `LOG_LEVEL=info` and one with a literal `VERIFY_API_KEY=not-a-secret`, then run `loadout import --scan`. | The `${VAR}` and `LOG_LEVEL` entries are importable (as `${env:VAR}` and `info`), `${VAR:-default}` is `unsupported_field`, the `VERIFY_API_KEY` literal is `needs_secret_extraction` (SPEC 11.7 classifier, M7). `loadout import` then adopts the importable ones without rewriting the file. | SPEC 8.6 import, 11.7; `import::server_from_native` |

## Secrets (M7), gate V1

Machines: Windows 11 with the 1Password desktop app and CLI 2.42.0 or newer,
and Arch Linux without the 1Password CLI (then, if possible, with the AUR
`1password-cli` 2.41.x). Use a throwaway `LOADOUT_HOME`, a throwaway
1Password vault with a fake item (for example `Loadout Verify/Fake Item`,
field `credential`, value `verify-not-a-secret`) and a throwaway 1Password
Environment holding one fake variable. Never use a real token. SPEC 11.1 to
11.3, 11.7, 11.8 and 11.11 carry no `[verify]` markers; each check below
covers an assumption the code makes about `op` or the OS keyring.

| # | Check | Expected | Clears |
|---|---|---|---|
| SE-1 | Windows: install the CLI with WinGet, open a new shell whose `PATH` lacks the CLI folder but keeps `%LOCALAPPDATA%\Microsoft\WinGet\Links`, then remove that folder from `PATH` too and run `loadout secrets providers` in both shells. Record `op --version`. | Both report `onepassword available <version>` with the path; `op --version` prints only the dotted version on its first line. No sign-in prompt appears. | SPEC 11.11 detection; `onepassword::locate`, `OpVersion::parse` |
| SE-2 | Windows: `loadout secrets set VERIFY_OP --ref "op://Loadout Verify/Fake Item/credential"`, then `loadout secrets check VERIFY_OP` four times: signed in; with the desktop app locked; dismissing the approval prompt; after renaming the item. Record `op read --no-newline <ref>`'s stderr for each. | `resolved`; `locked`; `locked`; `missing`. The value never shows in Loadout's output. If a message is classified wrongly, record its exact text: `onepassword::failure` matches wording (`not currently signed in`, `authorization prompt`, `isn't an item`, `could not find`, ...). | SPEC 11.3 statuses; `onepassword::failure` |
| SE-3 | Windows, CLI 2.42.0+: run `op environment secret list <env-id> --format json` with one "show value by default" variable and one hidden one. Then `loadout secrets set VERIFY_ENV --ref op-env://<env-id>/<NAME>` and `loadout secrets check`. | The output is a JSON array of objects with a `name` field (record the other fields). Loadout reports `resolved` for a listed name and `missing` for an unknown one, and never prints a value. | SPEC 11.10 output shape; `OpCli::list_environment` |
| SE-4 | Arch with CLI 2.41.x (AUR): `loadout secrets providers`, then `loadout secrets check VERIFY_ENV`. | `environments yes`, `environment_listing no`; the alias is `unverifiable` and `op environment secret list` is not run. (Below 2.41.0 the alias is `provider_too_old`.) | SPEC 11.11 version gates |
| SE-5 | Arch without the CLI: `loadout secrets providers`, `loadout secrets check`, then `printf verify-not-a-secret \| loadout secrets set VERIFY_OP --this-machine --keyring` and `loadout secrets check` again. On the Windows machine (after syncing the store, M12/M13, or by copying `secrets.toml`) run `loadout secrets ls`. | "1Password CLI not found"; `VERIFY_OP` `provider_unavailable` (exit 5) while env and keyring aliases resolve; after the override (`keyring://loadout/VERIFY_OP` in `machines/<id>.toml`) it is `resolved` on Arch only; Windows still lists the `op://` reference. | SPEC 11.11 per-machine blocking and machine overlay |
| SE-6 | Windows: `"verify-not-a-secret" \| loadout secrets set VERIFY_KR --keyring` (PowerShell pipes a trailing newline), check Credential Manager > Windows Credentials, `loadout secrets check VERIFY_KR`, `loadout secrets clear VERIFY_KR`, check again. Arch: the same with gnome-keyring or KeePassXC running, then with the Secret Service stopped, then with the collection locked. | One generic credential for service `loadout`, account `VERIFY_KR`, value without the newline; `resolved`; deleted; `missing`. Arch: `resolved`; `provider_unavailable` without a Secret Service; `locked` for a locked collection. | `OsKeyring` and its error mapping (`keyring` 4 `NoDefaultStore`, `NoStorageAccess`) |
| SE-7 | Real-shaped store files: `loadout server add` with a description holding a URL with `user:password@`, then with a fake token assembled by hand in the terminal (`gh` + `p_` + 36 characters). | Both are refused with `leak_detected`, naming the file and line but not the text. Record any false positive met in daily use (a legitimate value refused): the classifier is `secrets::classify`. | SPEC 11.8 scanner on every store commit |

## Secret delivery and `loadout exec` (M8), gate V1

Machines and setup as in the Secrets section, plus a stdio server that
reports its environment: build the test server with `cargo build -p
agent-loadout-testkit --bins` and use `target/debug/fake-mcp-server`
(`fake-mcp-server.exe` on Windows; no argument = a JSON-RPC server whose
tool `env` returns one variable; `env` = print the whole environment and
exit). Add it with `loadout server add verify-launch --env
VERIFY_TOKEN='${secret:VERIFY_OP}' --client <client> -- <path to
fake-mcp-server>`; ask the client to call the `env` tool with `VERIFY_TOKEN`.
The goldens are in
`crates/agent-loadout-core/tests/snapshots/delivery_matrix__tier_a.snap`.

| # | Check | Expected | Clears |
|---|---|---|---|
| DX-1 | For each Tier A client, add `verify-launch` (VERIFY_OP is the fake `op://` item), apply, start the client and call the tool. Then quit the client and list processes (`Get-Process fake-mcp-server` / `pgrep fake-mcp-server`). | The entry's `command` is the absolute path of `loadout`, `args` start with `exec --ref VERIFY_TOKEN=op://...`; the tool returns `verify-not-a-secret`; no server process is left after the client quits. The client file never holds the value. | SPEC 11.5 launch; Tier A row of SPEC 11.4 table |
| DX-2 | Windows: with DX-1 running in Claude Code, end the `loadout.exe` process in Task Manager (not the server). | `fake-mcp-server.exe` disappears with it (kill-on-close Job Object). | SPEC 11.5 Job Object; `exec/windows.rs` |
| DX-3 | Windows, desktop app integration (no service account): start Claude Code with two `verify-launch`-style servers, then start a second client process. Record how many approval prompts appear and for how long one approval covers new server starts in the same client process. | Record the numbers. | SPEC 11.5 `[verify how long one approval lasts per client process on Windows]` |
| DX-4 | Create a throwaway service account read-only for the throwaway vault and Environment, `"<token>" \| loadout secrets service-account set`, lock the desktop app, repeat DX-1. Then point VERIFY_OP at an item in the Personal/Private vault and repeat. | No prompt; the tool returns the value; the server's environment (the `env` mode) has no `OP_SERVICE_ACCOUNT_TOKEN`. The Personal-vault item fails with exit 77 in the client's server log. | SPEC 11.5 service account; `[verify]` service accounts cannot read Personal/Private vaults |
| DX-5 | Environment: `loadout secrets map-op-env <env-id> --all` (CLI 2.42.0+), then a server with `--env SERVER_TOKEN='${secret:<NAME>}'` for each client; call the tool for `SERVER_TOKEN` and for `<NAME>`. | `args` hold `--op-env <env-id> --map SERVER_TOKEN=<NAME>`; `SERVER_TOKEN` has the value, `<NAME>` and other Environment variables are absent. Record any variable `op run` adds besides the Environment's (stage 2 drops everything stage 1 did not pass on). | SPEC 11.5 Environment refs, 11.10; `exec` stage 2 |
| DX-6 | omp and Pi: an http server with `--header 'Authorization=Bearer ${secret:VERIFY_KR}'` (keyring alias), pointed at a local endpoint that logs request headers. Windows and Linux. | The entry header is `!"<loadout>" secret get --ref "VERIFY_KR=keyring://loadout/VERIFY_KR" --template "Bearer {secret:VERIFY_KR}"` (single quotes on Linux); requests carry `Bearer verify-not-a-secret`. Record which shell runs `!command` on each OS, whether omp expands `${...}` inside it, and whether trailing whitespace is trimmed. | SPEC 11.4 `command` delivery; `render::shell_quote` |
| DX-7 | Claude Code: the DX-6 server for `claude-code`, Windows and Linux; also as `sse`. | `headersHelper` runs `loadout secret-header ...` and the requests carry the header. Record which shell runs `headersHelper` on Windows (cmd or Git Bash) and whether the double-quoted form works there. | SPEC 7.4 `headersHelper`; `render::shell_quote` |
| DX-8 | Codex: the DX-6 server for `codex`; export `VERIFY_KR` with `loadout env --export --aliases VERIFY_KR --shell pwsh \| Invoke-Expression` (bash: `eval "$(loadout env --export --aliases VERIFY_KR)"`), start Codex from that shell. | The entry has `bearer_token_env_var = "VERIFY_KR"`; requests carry the bearer token. | SPEC 11.4 Codex `bearer_token_env_var` through env; SPEC 11.6 |
| DX-9 | Arch without `op`: `loadout apply` with `verify-launch` enabled. Then `printf verify-not-a-secret \| loadout secrets set VERIFY_OP --this-machine --keyring` and apply again; start each client. | First apply exits 5, the entry is `blocked: provider_unavailable` naming "1Password CLI not found" and the per-machine fix; nothing is written for it. After the override the entry launches with `--ref VERIFY_TOKEN=keyring://loadout/VERIFY_OP` and the tool returns the value. | SPEC 11.4 rule 2, 11.11 |
| DX-10 | Run a client with `verify-launch` while `VERIFY_OP` points at a missing item, then with an unknown option added to `args` by hand. | The client's server log shows `loadout-exec: VERIFY_TOKEN: missing: ...` (exit 77), then `loadout-exec: unknown option ...` (exit 78); nothing appears on the JSON-RPC channel. Record how each client surfaces the exit code. | SPEC 11.5 exit codes and stderr prefix |
| DX-11 | Windows: `loadout server add verify-npx --env VERIFY_TOKEN='${secret:VERIFY_OP}' --client claude-desktop -- npx -y @modelcontextprotocol/server-everything`, apply, fully restart Claude Desktop. | The entry starts `loadout.exe` directly (no `cmd /c`); `loadout exec` finds `npx.cmd` through `PATHEXT` and the server connects. | SPEC 5.2 shim with launch delivery; `exec/windows.rs` `resolve` |
| DX-12 | Linux and macOS: run `loadout apply` with a keyring-backed `verify-launch` while the Secret Service collection or Keychain is locked. | Record whether planning (which checks the keyring entry exists) shows an unlock prompt. Windows Credential Manager never prompts. | `delivery::check_available` keyring presence check |

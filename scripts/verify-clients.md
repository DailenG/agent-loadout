# Real-client verification

Manual checks for the maintainer at gates V1 (Tier A) and V2 (Tier B), per
SPEC.md section 19 item 10. The implementer never runs these. Each check
names the `[verify]` marker (or the assumption in code) it clears. Record
results in `docs/verify/<client>.md`; a failure becomes a fix brief before
M9.

## Ground rules

- Use a throwaway `LOADOUT_HOME` and, where the client supports it, a
  throwaway config dir (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
  `PI_CODING_AGENT_DIR`, `COPILOT_HOME`). Back up any real client file a
  check touches before starting, and restore it afterwards.
- Run each client's own lister to confirm what it loaded (`claude mcp list`,
  the Claude Desktop developer settings, `codex mcp list`, omp `/mcp list`,
  `pi mcp list`, VS Code's `MCP: List Servers` command, Copilot CLI `/mcp`,
  Cursor's Settings, MCP page).
- Write Loadout's entries with `loadout server add <id> --client <client>`
  (instant apply, M6) against the throwaway `LOADOUT_HOME`, or by hand from
  the goldens in
  `crates/agent-loadout-core/tests/snapshots/adapters_claude__*.snap`,
  `adapters_tier_a__*.snap` and `adapters_tier_b__*.snap`.
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

## GitHub Copilot (`copilot`), gate V2

Machines: Windows 11 (VS Code Stable with the Copilot Chat extension, and
the Copilot CLI); the CLI checks also on Arch Linux. Use a throwaway
`COPILOT_HOME` for the CLI checks. The adapter's assumptions are listed in
`crates/agent-loadout-core/src/adapters/copilot.rs`.

| # | Check | Expected | Clears |
|---|---|---|---|
| COP-1 | VS Code: unset `COPILOT_HOME`, add `verify-user` (stdio, the `copilot_stdio` golden without `tools`) to `~/.copilot/mcp-config.json`, reload the window, run `MCP: List Servers` and open the agent-mode tool picker. | `verify-user` is listed and its tools are offered. Record whether VS Code also needs a setting to read this file. | SPEC 7.3 `copilot` user file ("VS Code's current Copilot-harness location") `[verify]`; `copilot::copilot_home` |
| COP-2 | Copilot CLI: set `COPILOT_HOME` to an empty throwaway dir, add `verify-cli` to `$COPILOT_HOME/mcp-config.json`, start `copilot`, run `/mcp`. Then point `COPILOT_HOME` at a relative path. | Listed from `$COPILOT_HOME`; `~/.copilot` is not read. Record what the CLI does with a relative value (Loadout ignores it and uses `~/.copilot`). | SPEC 7.3 `$COPILOT_HOME/mcp-config.json`; `Location::EnvDir` |
| COP-3 | Put `verify-project` in `<P>/.mcp.json`, open `<P>` in VS Code in Restricted Mode, then trust the workspace; also start the Copilot CLI in `<P>`. | VS Code lists it only once the workspace is trusted; record what the CLI does (folder trust prompt or none). | SPEC 7.4 `copilot` trust gate `[verify]`; `TrustGate::WorkspaceTrust` |
| COP-4 | Add stdio and http servers that report their environment, arguments and request headers, with `${VERIFY_VALUE}` in `command` (a directory prefix), `args`, `env`, a header value and the `url` query, plus `${VERIFY_UNSET:-fallback}` in `args`. Start VS Code and the CLI from a shell with `VERIFY_VALUE` set. | `command`, `args`, `env` and header values are expanded and the default applies; record whether `url` is expanded (Loadout assumes not). Record any validation warning VS Code shows for `${...}` in `<P>/.mcp.json` while the runtime still expands it. | SPEC 7.4 `copilot` env refs `[verify]`; `copilot::CAPS.env_ref_fields` |
| COP-5 | Put `verify-both` in the user file and, with other `args`, in `<P>/.mcp.json`. | Record which entry Copilot runs in `<P>` and whether fields merge (Loadout reports `ProjectSemantics::Unknown` and precedence project over user until this is known). | SPEC 7.4 `copilot` project precedence `[verify]` |
| COP-6 | Add `"disabled": true`, then `"enabled": false`, to `verify-user`; also stop it from VS Code's server list and diff the file. | Record whether either field hides the server and whether VS Code writes one. Loadout writes neither (`native_disable = None`) and removes entries instead. | SPEC 7.4 `copilot` native disable `[verify]` |
| COP-7 | CLI: an entry with `"type": "local"` and `"tools": ["*"]`, then `"tools": ["<one tool>"]`; the same filter in VS Code. Also an entry without `tools`. | `local` runs as stdio; `["*"]` and a missing `tools` expose every tool; a list limits the tools in both. | `copilot::CopilotJson` read spellings; `ToolFilterMech::Allow("tools")` |
| COP-8 | Add servers named `verify.dotted` and `verify spaced`. | Record which load. Loadout accepts `[A-Za-z0-9_.-]+` for Copilot (Claude Code's rule, since `.mcp.json` is shared). | `copilot` `valid_name = dotted_word_name` `[verify]` |
| COP-9 | Windows: a stdio entry with plain `"command": "npx"` in VS Code and the CLI. | Record whether it connects (`windows_shell_shim = false`). | SPEC 7.4 notes |
| COP-10 | Detection: the Copilot CLI on `PATH` (`copilot.cmd` from npm), a `~/.copilot` dir alone, and the Copilot Chat extension dir alone. | Any of them detects `copilot`. Record the extension's directory name under `~/.vscode/extensions` (Loadout matches `github.copilot-chat-*`). | SPEC 7.7; `copilot::SPEC.detect` `[verify]` |
| COP-11 | Project shared file: a project manifest with `docs` for `["claude-code", "copilot"]`, `claude-only` for `claude-code` and `copilot-only` for `copilot`; `loadout apply`; open `<P>` in Claude Code and in VS Code. Then remove `claude-code` from every entry and apply again. | One `<P>/.mcp.json` holds all three; each client lists every entry in the file (Copilot also sees `claude-only`, and the effective view says so). After the second apply only the Copilot entries remain. No entry has `inputs`, `envFile`, `dev` or `sandbox`. | SPEC 6.5 shared files; planner target join and clean-up (`copilot_shares_the_project_mcp_json_with_claude_code`) |

## Cursor (`cursor`), gate V2

Machines: Windows 11 with Cursor. The adapter's assumptions are listed in
`crates/agent-loadout-core/src/adapters/cursor.rs`.

| # | Check | Expected | Clears |
|---|---|---|---|
| CUR-1 | Add `verify-user` (the `cursor_stdio` golden) to `~/.cursor/mcp.json`, open Settings, MCP. | Listed and connected. | SPEC 7.3 `cursor` user file |
| CUR-2 | Put `verify-project` in `<P>/.cursor/mcp.json` and open `<P>`. | Listed in `<P>` only. Record any approval prompt (Loadout assumes no trust gate). | SPEC 7.3 project file; `trust_gate = None` |
| CUR-3 | Servers that report environment, arguments and headers with `${env:VERIFY_VALUE}` in `command`, `args`, `env`, `url` and a header; start Cursor from a shell with `VERIFY_VALUE` set. | Each field is expanded; record any that is not. | SPEC 7.4 `cursor` `${env:NAME}`; `cursor::CAPS.env_ref_fields` `[verify]` |
| CUR-4 | Turn `verify-user` off with Cursor's toggle and diff `~/.cursor/mcp.json`; then hand-add `"disabled": true` to another entry. | Record where Cursor keeps the toggle and whether `disabled` is honored. Loadout writes neither (`native_disable = None`). | SPEC 7.4 `cursor` native disable `[verify]` |
| CUR-5 | Put `verify-both` in the user file and, with other `args`, in `<P>/.cursor/mcp.json`; then try `{ "disabled": true }` alone in the project file. | Record which entry runs in `<P>`, whether fields merge, and whether any project entry hides the user one (Loadout: Replace, no project suppression, `cannot_suppress_in_project`). | SPEC 7.4 `cursor` project suppression and precedence `[verify]` |
| CUR-6 | An SSE endpoint as `{ "type": "sse", "url": ... }` and as a bare `url`; a Streamable HTTP endpoint as a bare `url`. | All connect; record if `type` is rejected. Loadout writes `"type": "sse"` for SSE only. | SPEC 7.5 transport inference; `cursor` mapper `[verify]` |
| CUR-7 | Servers named `verify.dotted` and `verify spaced`. | Record which load (Loadout accepts any non-empty name for Cursor). | `cursor` `valid_name` `[verify]` |
| CUR-8 | A stdio entry with plain `"command": "npx"`. | Record whether it connects (`windows_shell_shim = false`). | SPEC 7.4 notes |
| CUR-9 | Detection: `cursor.cmd` on `PATH`, `~/.cursor` alone, `%APPDATA%\Cursor` alone; record the uninstall entry `DisplayName` (`Get-ItemProperty HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*` in a `.ps1` file). | Any probe detects Cursor; the `DisplayName` is `Cursor` or the value to put in `Probe::UninstallEntry`. | SPEC 7.7; `cursor::SPEC.detect` `[verify]` |
| CUR-10 | `"envFile": "<abs path>/verify.env"` (a `VERIFY_VALUE=verified` line) on a server that reports its environment. | The server sees `verified` (`envFile` is a passthrough field). | SPEC 7.3 `envFile`; `passthrough_of("cursor")` |

## VS Code legacy `mcp.json` (`vscode-legacy`), gate V2

Machines: Windows 11 (VS Code Stable, and Insiders or VSCodium if
installed); VSL-1 also on Arch Linux. Loadout only reads these files. The
adapter's assumptions are listed in
`crates/agent-loadout-core/src/adapters/vscode_legacy.rs`.

| # | Check | Expected | Clears |
|---|---|---|---|
| VSL-1 | With `MCP: Open User Configuration`, add `verify-legacy` in each installed variant; note the file it opens. Set `import_variants` to the installed variants in the throwaway store's `clients.toml` and run `loadout import --scan`. | The files are `%APPDATA%\<variant>\User\mcp.json` (`$XDG_CONFIG_HOME/<variant>/User/mcp.json` on Linux), and the scan lists `verify-legacy` from each. | SPEC 7.3 `vscode-legacy` user paths `[verify]`; `vscode_legacy::user_files` |
| VSL-2 | Create a VS Code profile, open its user MCP configuration, add `verify-profile`. Then set `import_profiles = "all"` and scan again. | The file is `User\profiles\<id>\mcp.json`; the scan lists `verify-profile`. | SPEC 7.3 profile layout `[verify]` |
| VSL-3 | With `verify-legacy` only in the user `mcp.json` and `verify-workspace` only in `<P>/.vscode/mcp.json`, open `<P>` and the Copilot agent tool picker; run `loadout effective <P> --client copilot`. | Copilot offers both (record any setting it needs); the effective view lists them as `client-compat:` layers and flags a same-named `<P>/.mcp.json` entry `duplicate_via_compat`. | SPEC 7.3 forwarding into Copilot `[verify]`; `copilot` `compat_reads` |
| VSL-4 | Entries without `type`: one with `command`, one with `url`. | Both start; Loadout reads them as stdio and http. | SPEC 7.5 inference; `vscode-legacy` mapper |
| VSL-5 | Detection: `code.cmd`, `code-insiders.cmd` or `codium.cmd` on `PATH`, or a variant's `User` dir alone. | Any detects `vscode-legacy`, and `all-detected` still never writes it. | SPEC 7.7; `vscode_legacy::SPEC.detect` `[verify]` |
| VSL-6 | Record `Get-FileHash` of every file from VSL-1 to VSL-3 (in a `.ps1` file), then run `loadout server add verify-ro --client vscode-legacy --client copilot -- npx -y @modelcontextprotocol/server-everything`, `loadout import --scan`, `loadout import`, `loadout adopt`, `loadout apply` and `loadout server rm verify-ro`; hash again. Add one entry with `${env:VERIFY_VALUE}` and one with `${input:verify-token}` (plus its `inputs[]` item) before the scan. | Every hash is unchanged; the apply warns `read_only_client`; the scan imports `${env:VERIFY_VALUE}` as a reference and reports the `${input:...}` entry `needs_manual_secret`; nothing imported is enabled for `vscode-legacy`. | SPEC 7.3 "never written", 8.6 import; `McpSpec::read_only`, `import::server_from_native` |

## Tier C clients (M21)

Gemini CLI, Antigravity, OpenCode and Grok Build. SPEC 19 schedules no
gate for Tier C yet; run these before Tier C ships, on Windows 11 and Arch
Linux, and record results the same way. The ground rules above apply:
throwaway `LOADOUT_HOME`, backups of any real client file, and for OpenCode
a throwaway `XDG_CONFIG_HOME`. Hand-written entries come from the goldens
in `crates/agent-loadout-core/tests/snapshots/adapters_tier_c__*.snap`.
Each adapter lists its assumptions in its header comment
(`crates/agent-loadout-core/src/adapters/{gemini,antigravity,opencode,grok}.rs`).

### Gemini CLI (`gemini`)

Lister: `/mcp` inside `gemini` (and `gemini mcp list`).

| # | Check | Expected | Clears |
|---|---|---|---|
| GEM-1 | Add `verify-user` (the `gemini_stdio` golden) to `~/.gemini/settings.json`, run `/mcp`. | Listed and connected. | SPEC 7.3 `gemini` user file |
| GEM-2 | Put `verify-project` in `<P>/.gemini/settings.json`; start `gemini` in `<P>` before and after trusting the folder. | Listed only once the folder is trusted. | SPEC 7.3 project file; `TrustGate::TrustedFolder` |
| GEM-3 | Point entries at an SSE endpoint as a bare `url`, at a Streamable HTTP endpoint as `httpUrl`, and at the HTTP endpoint as `{"type": "http", "url": ...}`; then one entry with both `httpUrl` and `url`, and one with `url` and `command`. | `url` connects over SSE, `httpUrl` and the typed form over HTTP. Record which transport the mixed entries use (Loadout assumes `httpUrl`, then `url`, then `command`, and treats `url` beside `httpUrl` as ignored). | SPEC 7.5 Gemini inference; `gemini::GeminiJson` `[verify]` |
| GEM-4 | With `verify-user` in the user file, add `"mcp": { "excluded": ["verify-user"] }` to the trusted `<P>/.gemini/settings.json`. Run `/mcp` inside and outside `<P>`. | Hidden inside `<P>`, listed elsewhere. | SPEC 7.4 project `mcp.excluded` `[verify]`; `ProjectDisable::ListMember` in the project file |
| GEM-5 | Put `verify-both` in the user file and, with other `args` only, in `<P>/.gemini/settings.json`. | Record whether the project entry replaces the user one or merges field by field (Loadout: `ProjectSemantics::DeepMerge`, so `unset` reports `unset_not_expressible`). | SPEC 7.4 `gemini` precedence `[verify]` |
| GEM-6 | A server that reports its environment, arguments and headers with `${VERIFY_VALUE}` in `command` (a directory prefix), `args`, `env`, `cwd`, `httpUrl` and a header; also a literal `$VERIFY_VALUE` (no braces), `$VERIFY_UNSET`, `p$ss` and `costs $5` in `args`. Start `gemini` from a shell with `VERIFY_VALUE` set. | Every `${...}` is expanded. Record what each bare `$` text becomes, and whether any escape keeps a `$` literal. Until this is recorded, Loadout refuses (`delivery_unavailable`) literal text and inline secret values holding `${` or `$` before `[A-Za-z0-9_]` for Gemini and Qwen Code (`EnvRefSyntax::DollarBrace { bare: true }`), and imports a bare `$NAME` as `${env:NAME}`. | SPEC 7.4 `gemini` `${VAR}`; `gemini::CAPS.env_ref_fields`, `bare` `[verify]` |
| GEM-7 | Add `"disabled": true`, then `"enabled": false`, to `verify-user`; also disable it with `/mcp` commands and diff the file. | Record whether any field disables it. Loadout writes neither (`native_disable = None`). | SPEC 7.4 `gemini` native disable |
| GEM-8 | Servers named `verify.dotted`, `verify_under` and `verify spaced`. | Record which load and any warning (SPEC 5.1 says Gemini recommends no underscores; Loadout accepts any non-empty name). | `gemini` `valid_name` `[verify]` |
| GEM-9 | Windows: a stdio entry with plain `"command": "npx"`. | Record whether it connects (`windows_shell_shim = false`). | SPEC 7.4 notes |
| GEM-10 | Detection: `gemini.cmd` on `PATH`, `~/.gemini/settings.json` alone, and `~/.gemini/config` alone (Antigravity's dir). | The first two detect Gemini; the third does not. | SPEC 7.7; `gemini::SPEC.detect` `[verify]` |
| GEM-11 | Set `"includeTools": ["<one tool>"]`, then `"excludeTools": ["<that tool>"]`; set `"timeout": 1000` on a slow tool. | The filter applies; the call times out after about 1 s (milliseconds). | SPEC 7.3 tool lists and `timeout`; `ToolFilterMech::AllowDeny`, `TimeoutField` milliseconds |

### Antigravity (`antigravity`)

Lister: the MCP servers panel (Manage MCP Servers) in the agent view.

| # | Check | Expected | Clears |
|---|---|---|---|
| ANT-1 | Open Manage MCP Servers, then View raw config; note the file it opens. Add `verify-user` (the `antigravity_stdio` golden) there. | Record the path: SPEC 7.3 names `~/.gemini/config/mcp_config.json`; if the app uses another file (for example `~/.gemini/antigravity/mcp_config.json`), `antigravity::SPEC` must change. | SPEC 7.3 `antigravity` user file `[verify]` |
| ANT-2 | Put `verify-project` in `<P>/.agents/mcp_config.json` and open `<P>`. | Listed in `<P>` only. | SPEC 7.3 project file `[verify]` |
| ANT-3 | A server that reports its environment with `${VERIFY_VALUE}`, `${env:VERIFY_VALUE}` and `$VERIFY_VALUE` in `env` values, started with `VERIFY_VALUE` set. | Record which spelling (if any) is expanded. Loadout writes literal values only (`env_ref = None`). | SPEC 7.4 `antigravity` env ref `[verify]` |
| ANT-4 | Toggle `verify-user` off in the panel and diff the file; then hand-add `"disabled": true`. | Record the field the app writes and whether it is honored. Loadout keeps an existing `disabled` or `disabledTools` and writes neither (`native_disable = None`). | SPEC 7.4 `antigravity` native disable `[verify]` |
| ANT-5 | Put `verify-both` in the user file and, with other `args`, in the project file; then `{ "disabled": true }` alone in the project file. | Record which entry runs and whether any project entry hides the user one (Loadout: `ProjectSemantics::Unknown`, `cannot_suppress_in_project`). | SPEC 7.4 `antigravity` precedence and suppression `[verify]` |
| ANT-6 | Open an untrusted folder holding `<P>/.agents/mcp_config.json`. | Record any trust prompt (Loadout assumes none). | SPEC 7.4 `antigravity` trust gate `[verify]` |
| ANT-7 | A Streamable HTTP endpoint as `serverUrl` (with `timeoutSeconds`), then as `url`; an SSE endpoint as `serverUrl`. | `serverUrl` connects over HTTP. Record whether `url` or SSE work (Loadout reads `serverUrl` only and blocks SSE). | SPEC 7.3, 7.5 `serverUrl` |
| ANT-8 | Servers named `verify.dotted` and `verify spaced`. | Record which load (Loadout accepts any non-empty name). | `antigravity` `valid_name` `[verify]` |
| ANT-9 | Windows: a stdio entry with plain `"command": "npx"`. | Record whether it connects (`windows_shell_shim = false`). | SPEC 7.4 notes |
| ANT-10 | Detection: `antigravity.exe` or `.cmd` on `PATH`, `%APPDATA%\Antigravity` alone, `~/.antigravity` alone, `~/.gemini/config` alone. | Any detects Antigravity. Record the real install dirs. | SPEC 7.7; `antigravity::SPEC.detect` `[verify]` |

### OpenCode (`opencode`)

Lister: `opencode mcp list`. Use a throwaway `XDG_CONFIG_HOME` for every
check.

| # | Check | Expected | Clears |
|---|---|---|---|
| OC-1 | Add `verify-user` (the `opencode_stdio` golden, with its `$schema` line) to `$XDG_CONFIG_HOME/opencode/opencode.json`; on Windows also check that `~/.config/opencode/opencode.json` is read with `XDG_CONFIG_HOME` unset. Then put a different `verify-user` in `opencode.jsonc` next to it. | Listed from `opencode.json`. Record which file wins when both exist (Loadout writes `opencode.json`, or `opencode.jsonc` when only it exists). | SPEC 7.3 `XdgConfigAllOs`; `opencode::SPEC.mcp.user` `[verify]` |
| OC-2 | Put `verify-project` in `<P>/opencode.json`, then only in `<P>/.opencode/opencode.json`. | Listed in `<P>` both times. | SPEC 7.3 project files `[verify]` |
| OC-3 | A `remote` entry pointing at an SSE-only endpoint whose path does not end in `/sse`, and one that does. | Both connect (OpenCode tries Streamable HTTP, then SSE). Loadout writes SSE servers as `remote` and reads one back as SSE only when the path ends in `/sse`. | SPEC 7.5 OpenCode `remote`; `opencode::is_sse_url` `[verify]` |
| OC-4 | With `verify-user` in the user file, put the whole entry with `"enabled": false` in `<P>/opencode.json`; then replace it with `{ "enabled": false }` alone. | The full disabled entry hides `verify-user` inside `<P>` only. Record whether the `enabled`-only entry is accepted (Loadout writes the full entry). | SPEC 7.4 project `enabled: false`; `ProjectDisable::EnabledFalse { partial: false }` `[verify]` |
| OC-5 | Put `verify-both` in the user file and, with another `command` array only, in the project file. | Record whether the project entry merges over the user one (Loadout: DeepMerge). | SPEC 7.4 `opencode` precedence `[verify]` |
| OC-6 | An entry without `type` (`{"command": ["x"]}`). | Record whether OpenCode rejects the file or the entry. Loadout reads it but reports that the client skips it. | `opencode::OpenCodeJson::skipped_by_client` `[verify]` |
| OC-7 | A server that reports its environment, arguments and headers with `{env:VERIFY_VALUE}` in the command array, `environment`, `url` and a header; also `${VERIFY_VALUE}` in `args` and `{file:<abs path>/verify.txt}` in `environment`. | Every `{env:...}` is expanded and `{file:...}` is replaced by the file's text; `${...}` reaches the server unchanged. Loadout writes literal `${` as is and refuses literal text and inline secret values holding `{env:` or `{file:` (`delivery_unavailable`). | SPEC 7.4 `{env:NAME}` (also `{file:path}`); `opencode::CAPS.env_ref_fields` `[verify]` |
| OC-8 | Servers named `verify.dotted` and `verify spaced`. | Record which load (Loadout accepts any non-empty name). | `opencode` `valid_name` `[verify]` |
| OC-9 | Windows: `"command": ["npx", "-y", "@modelcontextprotocol/server-everything"]`. | Record whether it connects (`windows_shell_shim = false`). | SPEC 7.4 notes |
| OC-10 | Detection: `opencode.exe` or `opencode.cmd` on `PATH`, `$XDG_CONFIG_HOME/opencode` alone. | Either detects OpenCode. | SPEC 7.7; `opencode::SPEC.detect` `[verify]` |

### Grok Build (`grok`)

Lister: Grok's MCP list command (record its name).

| # | Check | Expected | Clears |
|---|---|---|---|
| GRK-1 | Add `[mcp_servers.verify-user]` (the `grok_stdio` golden) to `~/.grok/config.toml`. | Listed and connected. | SPEC 7.3 `grok` user file `[verify]` |
| GRK-2 | Put `verify-project` in `<P>/.grok/config.toml` and start Grok in `<P>/src/deep` (inside the same git repository). | Listed: Grok walks up to the git root. | SPEC 7.3 project file `[verify]` |
| GRK-3 | Start Grok in `<P>` before and after trusting the folder. | Project servers load only once trusted. | SPEC 7.4 `grok` folder trust; `TrustGate::TrustedFolder` |
| GRK-4 | Put `verify-compat` in `~/.claude.json` (top-level `mcpServers`) and in `projects["<P>"].mcpServers`, in `<P>/.cursor/mcp.json` and in `<P>/.mcp.json`, each with a different `args` marker; also put a same-named entry in `<P>/.grok/config.toml`. Remove them one at a time. Run `loadout effective <P> --client grok`. | Record which files Grok reads and which entry wins at each step (Loadout: Grok's own files first, then `~/.claude.json` top level, `.cursor/mcp.json`, `.mcp.json`; `projects[...]` not read). The effective view flags the native entry `duplicate_via_compat`. | SPEC 6.4, 7.4 `grok` compat reads; `grok::CAPS.compat_reads` `[verify]` |
| GRK-5 | Add `enabled = false`, then `disabled = true`, to `verify-user`. | Record whether either disables it (Loadout: `native_disable = None`). | SPEC 7.4 `grok` native disable `[verify]` |
| GRK-6 | Put `verify-both` in the user file and, with other `args` only, in the project file. | The project entry replaces the user one (no field merge). | SPEC 7.4 `grok` Replace; `cannot_suppress_in_project` |
| GRK-7 | A server that reports its environment, arguments and headers with `${VERIFY_VALUE}` and `${VERIFY_UNSET:-fallback}` in `command`, `args`, `env`, `url` and a header. | `env` and header values are expanded (Loadout writes references only there); record the other fields. | SPEC 7.4 `grok` `${VAR}`, `${VAR:-d}`; `grok::CAPS.env_ref_fields` `[verify]` |
| GRK-8 | Servers named `verify.dotted` and `"verify spaced"`. | Record which load (Loadout accepts any non-empty name). | `grok` `valid_name` `[verify]` |
| GRK-9 | Windows: a stdio entry with plain `command = "npx"`; set `startup_timeout_sec = 1` on a slow server. | Record whether `npx` connects (`windows_shell_shim = false`); the startup times out after about 1 s. | SPEC 7.4 notes; `TimeoutField` seconds |
| GRK-10 | Detection: `grok.exe` or `grok.cmd` on `PATH`, `~/.grok` alone. | Either detects Grok. | SPEC 7.7; `grok::SPEC.detect` `[verify]` |

## Tier D clients (M22)

Windsurf, Devin Desktop and the ported clients of SPEC 7.5, plus the YAML
and JSON5 editors they rely on. Best effort: SPEC 19 schedules no gate for
Tier D; run whichever checks a machine has the client for, on Windows 11
and Arch Linux, and record results the same way. The ground rules above
apply (throwaway `LOADOUT_HOME`, backups of any real client file; for
Goose a throwaway `GOOSE_PATH_ROOT`, for Crush a throwaway
`XDG_CONFIG_HOME`). Hand-written entries come from the goldens in
`crates/agent-loadout-core/tests/snapshots/adapters_tier_d__*.snap`. The
adapters list their assumptions in their header comments
(`crates/agent-loadout-core/src/adapters/{windsurf,devin,ported}.rs`).

### Windsurf (`windsurf`)

Lister: Windsurf's Cascade MCP panel (Settings, Cascade, MCP servers).

| # | Check | Expected | Clears |
|---|---|---|---|
| WSF-1 | Add the `github` entry of the `windsurf_goldens` golden to `~/.codeium/windsurf/mcp_config.json` and refresh the MCP panel. | Listed and connected. | SPEC 7.3 `windsurf` user file |
| WSF-2 | Put `verify-project` in `<P>/.windsurf/mcp_config.json` and open `<P>`. | Listed in `<P>` only; record any approval prompt. | SPEC 7.3 project file `[verify]` |
| WSF-3 | A Streamable HTTP server as `{ "serverUrl": ... }`, then as `{ "url": ... }`; an SSE server as `{ "type": "sse", "serverUrl": ... }`. | Record which spellings connect. Loadout writes `serverUrl`, and `"type": "sse"` for SSE. | SPEC 7.3 `url` vs `serverUrl` `[verify]`; `windsurf::MAPPER` |
| WSF-4 | Servers that report environment, arguments and headers with `${env:VERIFY_VALUE}` in `command`, `args`, `env`, `serverUrl` and a header; start Windsurf from a shell with `VERIFY_VALUE` set. | Each field is expanded; record any that is not. | SPEC 7.4 `windsurf` `${env:NAME}`; `windsurf::CAPS.env_ref_fields` `[verify]` |
| WSF-5 | Toggle `verify-user` off in the MCP panel and diff the file; then hand-add `"disabled": true` to another entry. | The toggle writes `"disabled": true` and a hand-written one hides the server. | SPEC 7.4 `windsurf` native disable `[verify]`; `DisableMech::DisabledTrue` |
| WSF-6 | Put `verify-both` in the user file and, with other `args`, in the project file; then try `{ "disabled": true }` alone in the project file. | Record which entry runs in `<P>`, whether fields merge and whether the project entry hides the user one (Loadout: `ProjectSemantics::Unknown`, `cannot_suppress_in_project`). | SPEC 7.4 `windsurf` project suppression and precedence `[verify]` |
| WSF-7 | Windows: a stdio entry with plain `"command": "npx"`. | Record whether it connects (`windows_shell_shim = false`). | SPEC 7.4 notes |
| WSF-8 | Detection: `windsurf.cmd` on `PATH`, `~/.codeium/windsurf` alone; record the uninstall entry `DisplayName` (in a `.ps1` file). | Any probe detects Windsurf; the `DisplayName` is `Windsurf` or the value to put in `Probe::UninstallEntry`. | SPEC 7.7; `windsurf::SPEC.detect` `[verify]` |

### Devin Desktop (`devin`)

Lister: Devin Desktop's MCP settings page.

| # | Check | Expected | Clears |
|---|---|---|---|
| DEV-1 | Add the `github` entry of the `devin_goldens` golden to `%APPDATA%\devin\mcp_config.json` (Windows) or `~/.config/devin/mcp_config.json` (Linux; also try with `XDG_CONFIG_HOME` set elsewhere). | Listed and connected; on Linux the file under `~/.config` is read whatever `XDG_CONFIG_HOME` says. | SPEC 7.3 `devin` user file `[verify]`; `devin::user_files` |
| DEV-2 | Put `verify-project` in `<P>/.devin/mcp_config.json` and `verify-local` in `<P>/.devin/mcp_config.local.json`; open `<P>`. | Both listed in `<P>` only; record any approval prompt. | SPEC 7.3 project and project-local files |
| DEV-3 | Put `verify-both` in the user, project and project-local files, each with a different `args` marker. Remove them one at a time. | The project-local entry wins, then the project one, then the user one; record whether fields merge (Loadout: `ProjectSemantics::Unknown`). | SPEC 7.4 `devin` precedence local > project > user `[verify]` |
| DEV-4 | Remote servers as `{ "url": ..., "transport": "http" }`, `"transport": "sse"`, `"transport": "streamable-http"`, no `transport`, and one with `headers`. | Record which connect and whether `headers` is sent. Loadout writes `transport` `http` or `sse` and `headers`. | SPEC 7.3 `{url, transport}` `[verify]`; `devin::MAPPER` |
| DEV-5 | Servers that report environment, arguments and headers with `${env:VERIFY_VALUE}` in `command`, `args`, `env`, `url` and a header. | Each field is expanded; record any that is not. | SPEC 7.4 `devin` `${env:VAR}` `[verify fields]` |
| DEV-6 | Turn `verify-user` off in the settings page and diff the file; try `"disabled": true` and `"enabled": false` by hand; try `{ "disabled": true }` alone in the project file. | Record any disable flag and project suppression (Loadout: neither, `cannot_suppress_in_project`). | SPEC 7.4 `devin` native disable and suppression `[verify]` |
| DEV-7 | Windows: a stdio entry with plain `"command": "npx"`. | Record whether it connects (`windows_shell_shim = false`). | SPEC 7.4 notes |
| DEV-8 | Detection: `devin` on `PATH`, the config dir alone; record the uninstall entry `DisplayName`. | Any probe detects Devin Desktop. | SPEC 7.7; `devin::SPEC.detect` `[verify]` |

### Ported clients (SPEC 7.5)

User scope only. Lister: each client's own MCP settings or `/mcp` command.

| # | Check | Expected | Clears |
|---|---|---|---|
| TD-1 | For each installed client, compare the file it creates or edits with the path in `adapters_tier_d__ported_user_files_<os>.snap` (Qoder on every OS; Goose with and without `GOOSE_PATH_ROOT`; Crush with `XDG_CONFIG_HOME`). | Same path. | SPEC 7.5 paths `[verify per OS]` |
| TD-2 | Add the two entries of the client's `adapters_tier_d__ported_<id>.snap` golden to its file (Augment, Junie, Droid, Command Code, Crush, Qoder, Trae, Trae CN, CodeBuddy, iFlow, Zencoder, WorkBuddy, ZCode). | Both listed; the stdio one connects. | SPEC 7.5 unified `{type, command, args, env}` / `{type, url, headers}` |
| TD-3 | Cline, Roo Code, Kilo Code: toggle a server off in the extension and diff the file; add an HTTP server through the UI. | The toggle writes `"disabled": true`; the UI writes `streamableHttp` (Cline) or `streamable-http` (Roo, Kilo). | SPEC 7.5 `cline` native `disabled`; `ported::{CLINE,ROO_CODE,KILO_CODE}` |
| TD-4 | Kiro: toggle a server off and diff `~/.kiro/settings/mcp.json`. | `"disabled": true`. | `ported::KIRO` `DisableMech::DisabledTrue` `[verify]` |
| TD-5 | ZCode: an entry with `"enable": false`, then `true`. | Hidden, then listed. | SPEC 7.5 `zcode` `enable` flag |
| TD-6 | Zencoder: add a server in the UI and diff `~/.zencoder/settings.json`. | It lands under `"zencoder": { "mcpServers": ... }` (nested), not a `"zencoder.mcpServers"` key. | SPEC 7.5 `zencoder` nested vs dotted `[verify]` |
| TD-7 | Trae: create a profile, add a server in it, and diff `User/mcp.json` and `User/profiles/<id>/mcp.json`. | Record which file the profile reads; Loadout writes the user file and every existing profile file. | SPEC 7.5 `trae` profiles; `ported::trae_files` |
| TD-8 | CodeBuddy and iFlow: with several of the candidate files present, add a server in the client. | The client edits the first existing one in SPEC 7.5 order. Also hand-write a CodeBuddy file with unquoted keys (JSON5) and run `loadout apply`: the target reports `read_only_format` and the file is unchanged. | SPEC 7.5 first-of; SPEC 7.2 JSON5 |
| TD-9 | OpenClaw and MiMo Code: add the `ported_openclaw` / `ported_mimo_code` golden entries; an SSE server whose URL ends in `/sse`; `{env:VERIFY_VALUE}` in MiMo's `environment`; `"enabled": false`. | Listed; SSE connects; MiMo expands `{env:NAME}`; `enabled: false` hides the server. | SPEC 7.5 agent-JSON shape `[verify]` |
| TD-10 | Qwen Code: the `ported_qwen_code` golden plus an SSE `url` entry and `excludeTools`. | Same behavior as Gemini CLI (GEM-3, GEM-6). | SPEC 7.5 `qwen-code` Gemini mapping |
| TD-11 | Continue: the `ported_continue` golden; an SSE server with `type: sse`; a header under `requestOptions.headers`; an entry SkillDock wrote with `transport: sse`. | Listed and connected; the header is sent; record whether `transport` is honored (Loadout reads it as `type`). | SPEC 7.5 `continue` list matched by `name` `[verify]` |
| TD-12 | Goose: the `ported_goose` golden (`uri`, `headers`), then the same HTTP entry with `url` instead of `uri`; an entry without `name` or `enabled`; toggle an extension off in `goose configure`. | `uri` connects (record `url`); record which keys Goose requires; the toggle writes `enabled: false`. | SPEC 7.5 `goose` `[verify]` headers; `ported::GooseYaml` |
| TD-13 | Hermes: the `ported_hermes` golden plus an SSE server with `transport: sse`. | Both connect. | SPEC 7.5 `hermes`; `ported::HERMES` |
| TD-14 | For each installed client, an env value `${VERIFY_VALUE}` and `${env:VERIFY_VALUE}` on a server that reports its environment. | Record any expansion (Loadout assumes none for the ported clients but Qwen Code and MiMo Code, so `env` delivery falls back as it does for clients without references). | SPEC 7.5 best effort; `ported::ported_caps` `env_ref = None` |
| TD-15 | Detection: each client's config dir alone, and its executable on `PATH` where listed in `ported.rs`. | Any probe detects the client; IntelliJ is detected but never gets an MCP target. | SPEC 7.7; `ported::*.detect` `[verify]` |
| TD-16 | YAML editing: in a real Goose `config.yaml` and Continue `config.yaml` with comments, run `loadout server add verify-yaml --client goose --client continue -- npx -y @modelcontextprotocol/server-everything`, then `loadout server rm verify-yaml`. Then add an anchor (`&base`) and an alias (`<<: *base`) to `extensions` and add the server again. | Only the entry's lines change and the removal restores the file byte for byte; with the anchor, the Goose target reports `read_only_format` and the file is unchanged. | SPEC 7.2 YAML span splice |
| TD-17 | Continue with only `~/.continue/config.json` (models and an MCP server in it): run `loadout server add verify-cfg --client continue -- npx -y @modelcontextprotocol/server-everything` and `loadout apply`. Then let Loadout create `config.yaml` with no `config.json` present, open Continue, and finally put both files side by side with different models. | With only `config.json`, apply warns `read_only_format` naming `config.json` and creates no `config.yaml`. The created `config.yaml` (`name`, `version`, `schema: v1`, `mcpServers`) validates and its server loads. Record which file Continue uses when both exist. | SPEC 7.5 `continue`; `ported::continue_user_files`, `CONTINUE_SKELETON` |

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

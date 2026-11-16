# Real-client verification

Manual checks for the maintainer at gates V1 (Tier A) and V2 (Tier B), per
SPEC.md section 19 item 10. The implementer never runs these. Each check
names the `[verify]` marker (or the assumption in code) it clears. Record
results in `docs/verify/<client>.md`; a failure becomes a fix brief before
M9.

## Ground rules

- Use a throwaway `LOADOUT_HOME` and, where the client supports it, a
  throwaway config dir (`CLAUDE_CONFIG_DIR`). Back up any real client file a
  check touches before starting, and restore it afterwards.
- Run each client's own lister to confirm what it loaded (`claude mcp list`,
  the Claude Desktop developer settings).
- Until M6 (apply) lands, write entries by hand from the goldens in
  `crates/agent-loadout-core/tests/snapshots/adapters_claude__*.snap`.
- Use synthetic server names and placeholder commands only (for example the
  `example-*` names from the fixtures, or a real public test server); never
  paste tokens into a client file for these checks.
- PowerShell helpers below are saved as `.ps1` files and run with
  `pwsh -File <name>.ps1`.

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

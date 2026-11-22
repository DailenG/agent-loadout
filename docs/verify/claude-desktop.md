# Claude Desktop: gate V1, Windows

CD-3 ran on the maintainer's machine (read-only). The other checks ran in
Windows Sandbox:
- the classic app 3.6556.2, `lib\net45` of the offline Squirrel package
  copied in, since `Update.exe --install` exited 0 there but left no
  `claude.exe`;
- release builds of `loadout`, `fake-mcp-server` and `op-stub`, with
  `vcruntime140.dll` next to them, since the sandbox has no VC++ runtime;
- a wrapper `envdump.cmd` that records each server start.

The app was not signed in. It showed "Get started" and started no MCP
server, not even after a full restart, and wrote nothing to
`%APPDATA%\Claude\logs`. Every check that needs a server to start is
therefore blocked until a run with a signed-in account. The harness is in
`%TEMP%\loadout-v1\sandbox` (`run.ps1`) and can be rerun as is.

Gate V2 (2026-11-22) reran CD-1, CD-4 and CD-5 in Windows Sandbox with the
same app build (3.6556.2, copied app) signed in by the maintainer, Loadout's
current debug build, node 26.13.1 and `envdump.cmd`. Evidence is under
`%TEMP%\loadout-v2\sandbox-results\` (`files\cd-*.json`, `markers\`,
screenshots, `claude-desktop-logs-*`). In this build the servers are listed
under Settings > Desktop app > Extensions > "Local MCP servers", not
Settings > Developer.

| # | Result | Evidence |
|---|---|---|
| CD-1 | Pass (V2) | `loadout server add verify-a --client claude-desktop` wrote `"command": "cmd", "args": ["/c", ...]`; verify-a started at the app's first launch. verify-b, added while the app ran, started only after a full restart (see AP-6). V1: blocked (signed out). |
| CD-2 | Not run | The MSIX package is not installable offline in the sandbox. Package folder name on the maintainer's machine: `Claude_pzs8sxrjxfjjc` (matches `Claude_*`). |
| CD-3 | Pass, note | Classic install: `HKCU\...\Uninstall\AnthropicClaude`, `DisplayName` = `Claude`. MSIX: package `Claude`, family `Claude_pzs8sxrjxfjjc`, version 6.28899.2.0. The same key also holds `Claude Code` (WinGet), so the future registry probe must match `DisplayName` exactly, not as a substring. (The CLI has no registry probe yet, `NoRegistry`.) |
| CD-4 | Pass (V2) | Plain `"command": "npx"` did not connect; the `cmd /c` form Loadout writes started; a full path to a `.cmd` (`envdump.cmd`, no `cmd /c`) also started. So `windows_shell_shim` is needed for a bare `npx` (a `.cmd` shim found through `PATHEXT`), while a full path to a `.cmd` works without it. (The harness labeled this FAIL; the observed outcome is the expected one.) |
| CD-5 | Pass (V2) | An `http` entry written by hand: the local HTTP server got 0 requests and the entry is not listed under Local MCP servers (maintainer's screenshot). Loadout's side (`unsupported_transport`) is covered by the automated tests. |
| CD-6 | Finding | The app rewrites the file at launch, even signed out. It re-serializes the whole file as 2-space JSON with LF, puts each array item on its own line (Loadout's `["/c", ...]` became multi-line), and adds a `preferences` table. It kept our `mcpServers` entries and their values. Loadout's next writes kept `preferences` and the app's formatting outside our entries. The ownership hash is canonical, so reformatting causes no drift. `rewrites_file_at_runtime` is now `true` for claude-desktop. |
| CD-7 | Not run | macOS. |

# VS Code legacy `mcp.json`: gate V2, Windows

Run 2026-11-22 in Windows Sandbox: VS Code 1.147.0 (Stable only; no
Insiders or VSCodium) with the Copilot Chat extension, signed in by the
maintainer; Loadout the current debug build. Each run used a throwaway
profile inside the sandbox. Servers: an `envdump.cmd` wrapper that writes
start markers and a small HTTP server that logs requests. Evidence:
`%TEMP%\loadout-v2\sandbox-results\` (`results.txt`, `log.txt`,
`markers\`, `out\vsl3-effective.txt`, `files\vsl6-hashes-*.txt`,
`remote\out\` with `vscode-logs\` and screenshots). The harness labeled
VSL-1, VSL-3 and VSL-4 FAIL partly because VS Code escapes `.cmd`
arguments (`^`), which broke the wrapper's marker names; the results below
are the rescored ones. VSL-1 on Arch Linux was not run.

| # | Result | Evidence |
|---|---|---|
| VSL-1 | Pass, note | VS Code reads `%APPDATA%\Code\User\mcp.json` (log channel `mcp.config.usrlocal`) and started `verify-legacy` from it (`args-^vsl-legacy` at 02:46); `loadout import --scan` with `import_variants = ["Code"]` lists it. Which file "MCP: Open User Configuration" opens is unverified (the maintainer answered No to the harness's question). |
| VSL-2 | Pass | `code --profile verify-prof` created `User\profiles\456b4ea9`; with `import_profiles = "all"` the scan lists `verify-profile`. The profile's `mcp.json` was written by the harness, not through the UI. |
| VSL-3 | Partial | VS Code reads `<P>/.vscode/mcp.json` (`mcp.config.ws0`) and `<P>/.mcp.json`; for `verify-workspace` in both it lists one server and ran the `.vscode/mcp.json` entry (`args-^vsl-ws`). The tool picker question was not verified. `loadout effective <P> --client copilot` printed "No project here: user scope only." because `<P>` was not a Loadout project, so no project layer (and no `duplicate_via_compat`) could appear; this is a harness error, not a Loadout result (fix brief FB-15). |
| VSL-4 | Pass (stdio and http) | `verify-legacy` has no `type` and a `command` and started (above). `verify-legacy-url` has no `type` and a `url` and served `tools/list` over HTTP (`markers\http-vsl-url.txt`); after a later restart VS Code showed it as Error. |
| VSL-5 | Pass | `code.cmd` on `PATH` and `%APPDATA%\Code\User` present: detected; `--all-detected` wrote no `vscode-legacy` file. |
| VSL-6 | Pass | Hashes unchanged (`files\vsl6-hashes-before.txt`, `-after.txt`); the apply warned `read_only_client`; the scan reports the `${input:...}` entry `needs_manual_secret`. |

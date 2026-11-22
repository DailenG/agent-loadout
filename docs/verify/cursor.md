# Cursor: gate V2, Windows

Run 2026-11-22 in Windows Sandbox: Cursor 3.31.15, node 26.13.1, Loadout
the current debug build. The maintainer signed in and answered the
harness's UI questions; in Cursor 3.x the MCP settings are under Customize
> MCPs. Each run used a throwaway profile inside the sandbox. Servers:
`fake-mcp-server`, an `envdump.cmd` wrapper that writes start markers, a
small HTTP server that logs requests and headers, and
`@modelcontextprotocol/server-everything` through `npx`. Evidence:
`%TEMP%\loadout-v2\sandbox-results\` (`results.txt`, `log.txt`,
`markers\`, `files\cur-*.json`, `remote\out\` with `cursor-logs\` and
screenshots).

| # | Result | Evidence |
|---|---|---|
| CUR-1 | Pass | `verify-user` in `~/.cursor/mcp.json` (`files\cur-mcp-user-written.json`) was listed and started. |
| CUR-2 | Pass, one Finding | `verify-project` in `<P>/.cursor/mcp.json` is listed only in `<P>`, under "Disabled", until it is enabled per server (toggle "Enable verify-project for cur source"). Loadout assumed no approval step (`trust_gate = None`); fix brief FB-13. |
| CUR-3 | Pass | `${env:VERIFY_VALUE}` was expanded in `command`, `args`, `env`, `url` and a header (markers and HTTP log). `verify-cur3` shows "Needs Attention" in the UI because its command path is a test path; the markers show it started. |
| CUR-4 | Pass, recorded | Cursor ignores `"disabled": true` (`verify-disabled` connected). The UI toggle does not change `~/.cursor/mcp.json` (`files\cur-mcp-user-after-toggle.json`); Cursor keeps that state in its own storage. Loadout writes neither (`native_disable = None`). |
| CUR-5 | Pass, recorded | `verify-both` in the user and the project file: both start, listed separately per source; fields do not merge. `{ "disabled": true }` alone in the project file removes the project entry and the user one stays Connected. Confirms that a project cannot hide a user server (`cannot_suppress_in_project`). |
| CUR-6 | Pass | SSE with `"type": "sse"`, SSE as a bare `url`, and Streamable HTTP as a bare `url` all served `tools/list`. |
| CUR-7 | Pass | `verify.dotted` and `verify spaced` load. |
| CUR-8 | Pass | Plain `"command": "npx"` connects without `cmd /c`; `windows_shell_shim = false` is right. |
| CUR-9 | Pass | Detected from each probe; the uninstall entry's `DisplayName` is "Cursor (User)". `cursor::SPEC.detect` has `Probe::UninstallEntry("Cursor")`; since the registry probe must match `DisplayName` exactly (CD-3), it should name "Cursor (User)" once that probe is implemented (the CLI has no registry probe yet). |
| CUR-10 | Pass | `envFile` with `VERIFY_VALUE=verified`: the server saw `verified`. |

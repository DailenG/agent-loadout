# GitHub Copilot: gate V2, Windows

Run 2026-11-22 in Windows Sandbox: VS Code 1.147.0 with the Copilot Chat
extension, Copilot CLI 1.0.114, Claude Code 2.1.330 (COP-11), node 26.13.1,
Loadout the current debug build. The maintainer signed in to each client.
Each run used a throwaway profile inside the sandbox; checks were driven by
the sandbox harness and a remote runner (UI Automation and screenshots).
Servers: `fake-mcp-server`, an `envdump.cmd` wrapper that writes start
markers, and `@modelcontextprotocol/server-everything` through `npx`.
Evidence: `%TEMP%\loadout-v2\sandbox-results\` (`results.txt`, `log.txt`,
`markers\`, `files\cop*.json`, `remote\out\` with `vscode-logs\` and
`shot-0NN-*.png`). The harness's own PASS/FAIL labels are wrong for COP-1;
the results below are the rescored ones.

**Environment note, Copilot CLI policy.** The maintainer's personal GitHub
account got "Third-party MCP servers are disabled by your organization's
Copilot policy" and "Failed to load models: access denied by Copilot
policy" in the CLI. This is known CLI behavior on personal accounts
([github/copilot-cli#1707](https://github.com/github/copilot-cli/issues/1707),
[#1976](https://github.com/github/copilot-cli/issues/1976)): check
<https://github.com/settings/copilot/features> ("Copilot CLI", "MCP servers
in Copilot"), then `copilot logout` and `copilot login`. It is not a Loadout
result; the CLI parts it blocked are marked below.

| # | Result | Evidence |
|---|---|---|
| COP-1 | Pass | VS Code 1.147 reads `~/.copilot/mcp-config.json` with no extra setting (log channel `copilot.null.verify-user`) and starts it: MCP: List Servers shows "verify-user, Running", start marker `args-^vsc-copuser` at 02:46:34. The harness looked for env markers that VS Code's `.cmd` argument escaping prevented, so it labeled this FAIL. |
| COP-2 | Pass | The CLI reads `$COPILOT_HOME/mcp-config.json`, not `~/.copilot` (`files\cop2-copilot-home-mcp-config.json`). A relative `COPILOT_HOME` resolves against the working directory; Loadout ignores a relative value and uses `~/.copilot`. |
| COP-3 | Pass | CLI: "Skipped workspace MCP servers (.mcp.json) because the folder ... is not trusted"; trust is kept in `trustedFolders` in `~/.copilot/config.json`. VS Code: `verify-project` listed in Restricted Mode, not started before trust, started after trust (`args-^vsc-proj` marker at 02:46). Matches `TrustGate::WorkspaceTrust`. |
| COP-4 | Pass (VS Code), CLI blocked | VS Code expanded `${VERIFY_VALUE}` in `args` and `env`, and `${VERIFY_UNSET:-fallback}` to `fallback` (remote run 029, `c4vsc-env`/`c4vsc-args`). Header and `url` expansion in VS Code not tested. The CLI part was blocked by the Copilot policy. |
| COP-5 | Pass (VS Code), CLI blocked | `verify-both` in the user file and in `<P>/.mcp.json`: VS Code lists one `verify-both`; before trust the user entry ran (`c5user-vsc` 02:26), after trust the project one (`c5proj-vsc` 02:46). The project entry replaces the user entry. CLI blocked by policy and trust. |
| COP-6 | Pass, recorded | The CLI ignores `"disabled": true` and `"enabled": false` in `mcp-config.json`. `copilot mcp disable` writes `disabledMcpServers` in `~/.copilot/settings.json`, not `mcp-config.json` (`files\cop6-mcp-config-after-disable.json`). Loadout writes neither (`native_disable = None`). |
| COP-7 | Not run | The CLI was blocked by policy; VS Code's tool filter needs the chat tool picker. |
| COP-8 | Pass | `verify.dotted` and `verify spaced` load in the CLI. `dotted_word_name` is stricter than the client, which is fine. |
| COP-9 | Pass (VS Code), CLI blocked | Plain `"command": "npx"` started server-everything under VS Code (`cmd.exe` -> `node`). `windows_shell_shim = false` is right for VS Code. |
| COP-10 | Pass | Detection. |
| COP-11 | Pass | `loadout apply` wrote one `<P>/.mcp.json` with the union of the three entries (`files\cop11-mcp-1.json`, `cop11-mcp-2.json`); Claude Code lists all three. The Copilot CLI listed none because the folder was untrusted (expected, COP-3). |

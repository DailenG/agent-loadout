# Apply engine: gate V1, Windows

Method in [README](README.md).

| # | Result | Evidence |
|---|---|---|
| AP-1 | Pass (V2) | V1: Claude Code ran (onboarding screen; no login in the throwaway profile) while `loadout server add verify-apply` applied. Claude Code wrote its own keys (`firstStartTime`, `userID`, migrations) into the same `.claude.json` during the session; after it quit, `verify-apply` was still there and `loadout status` showed it `synced`. V2 sandbox run (signed in, Claude Code 2.1.330): `claude -p` (haiku) called `mcp__verify-apply-a__env` while Loadout applied; both entries were `synced` after two sessions (`files\ap1-claude-json-mcpServers.json`). |
| AP-2 | Pass | A client file with an explicit `BUILTIN\Users:(R)` ACE and the hidden attribute kept both, byte for byte in `icacls` and `attrib` output, across an apply that changed it. |
| AP-3 | Pass | Applies to `.claude.json` and to omp's `mcp.json` while each client ran succeeded; no `file_locked`. |
| AP-4 | Pass | `--env VERIFY_VALUE='${env:VERIFY_VALUE}'` and the argument `--tag=${env:VERIFY_VALUE}`: Claude Code and omp got `${VERIFY_VALUE}` in `env` and `args`, and both servers saw `verified` in each. Codex got `env_vars = ["VERIFY_VALUE"]` and, for the argument, the inline value `--tag=verified` (the planned fallback). Pi deferred to V2. |
| AP-5 | Pass | With `[omp]` and `[codex]` `native_disable = true` in `clients.toml`, `server rm` left each entry with `enabled = false`; `codex mcp list` shows it disabled and omp's `/mcp list` shows it inactive. |
| AP-6 | Pass, note (V2) | Windows Sandbox, signed in: `server add verify-b --client claude-desktop` while the app ran wrote the entry (exit 0) and the app kept it; verify-b started only after a full restart of the app. Loadout printed no restart hint: claude-desktop's `restart_required` is not surfaced (fix brief FB-16). V1: blocked (signed out). See [claude-desktop.md](claude-desktop.md). |
| AP-7 | Pass | `import --scan` over Claude Code, omp (also read as Pi, same file) and Codex: `${VAR}` and `LOG_LEVEL=info` importable, `${VAR:-fallback}` `unsupported_field`, a literal `VERIFY_API_KEY` `needs_secret_extraction`. Entries that differ between clients are a `collision` until `--client` picks one; `import --client claude-code` adopted the importable ones (`TOKEN = "${env:VAR}"`) without changing any client file. |

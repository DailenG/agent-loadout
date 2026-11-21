# Secret delivery and `loadout exec`: gate V1, Windows

`op` was `op-stub` (version 2.42.0, one fake item, one fake Environment) on
a `PATH` without the real CLI; keyring aliases used the debug build's test
keyring file. The client-side mechanics are verified; 1Password's own
behavior (prompts, sign-in, service accounts) is not. Method in
[README](README.md).

| # | Result | Evidence |
|---|---|---|
| DX-1 | Pass (stub) | For Claude Code, omp and Codex, an `op://` alias and a `keyring://` alias each planned as `launch`: `command` is the absolute `loadout.exe`, `args` start `exec --ref VERIFY_TOKEN=...`. Every server saw the value; no client file holds it; no `OP_SERVICE_ACCOUNT_TOKEN` in any server environment; no `fake-mcp-server` or `loadout` process left after the clients quit. Codex note: it forwards a fixed environment, so the test keyring variable had to be added with `env_vars` by hand; with the real OS keyring nothing extra is needed. |
| DX-2 | Not run here | Covered by the automated Job Object test in `cli_exec` (verified to fail when job setup is disabled). |
| DX-3 | Not run | Real 1Password approval prompts. |
| DX-4 | Not run | Real service account. |
| DX-5 | Pass (stub) | `secrets map-op-env verifyenv0001 --all`, then `--env SERVER_TOKEN='${secret:MCP_TOKEN}'` per client: `args` hold `--op-env verifyenv0001 --map SERVER_TOKEN=MCP_TOKEN`; each server saw `SERVER_TOKEN`, and neither `MCP_TOKEN` nor the Environment's other variable. |
| DX-6 | Pass | omp http entry with header `Authorization: !"<loadout.exe>" secret get --ref "VERIFY_KR=keyring://loadout/VERIFY_KR" --template "Bearer {secret:VERIFY_KR}"`: requests carried `Bearer <value>`. (Some of omp's requests had no header; the connecting ones did.) |
| DX-7 | Pass | Claude Code `headersHelper` with the double-quoted `loadout.exe secret-header ...` form: requests carried `Bearer <value>`. |
| DX-8 | Pass | Codex `bearer_token_env_var = "VERIFY_KR"`, with the value from `loadout env --export --aliases VERIFY_KR --shell pwsh` (`$env:VERIFY_KR = '...'`): requests carried `Bearer <value>`. |
| DX-9 | Not run | Arch. |
| DX-10 | Pass (Claude Code) | A missing item and an unknown option: Claude Code lists `Failed to connect`, restarts the server once, and logs the server's stderr: `loadout-exec: VERIFY_TOKEN: ... 1Password item, field or Environment variable not found` (Claude Code redacts the word after the alias) and `loadout-exec: unknown option "--bogus-option"`. Exit codes themselves are not shown. omp and Codex not checked. |
| DX-11 | Not run | Claude Desktop. |
| DX-12 | Not run | Linux and macOS. |

While setting up DX-5, a hand edit of a Codex entry made earlier was
reported as `drifted ... (needs a decision: --on-conflict or adopt)` and
left alone, as designed.

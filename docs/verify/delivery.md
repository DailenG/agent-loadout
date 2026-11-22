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
| DX-3 | Partial | Desktop-app approval: every Loadout command that reached `op` (map, check) asked once; counting per client process was skipped to spare the maintainer the prompts. |
| DX-4 | Pass | Family-account service account, read access to the Environment only, token stored with `loadout secrets service-account set` (Windows Credential Manager). No prompts, and no `OP_ACCOUNT` needed (the token names its account). Each server's environment had no `OP_SERVICE_ACCOUNT_TOKEN`; no client file holds the token. (Service accounts are available on 1Password Families: created through the 1Password.com wizard.) |
| DX-5 | Pass (real) | `secrets map-op-env <env-id> --all`, then `--env SERVER_TOKEN='${secret:VERIFY_TOKEN}'` for Claude Code, omp and Codex: `args` hold `--op-env <id> --map SERVER_TOKEN=VERIFY_TOKEN`; each server, started by its client, got `SERVER_TOKEN` with the Environment's value (as typed, including two backticks copied with it) and neither `VERIFY_TOKEN` nor `VERIFY_PLAIN`. (Also passed earlier against `op-stub`.) |
| DX-6 | Pass | omp http entry with header `Authorization: !"<loadout.exe>" secret get --ref "VERIFY_KR=keyring://loadout/VERIFY_KR" --template "Bearer {secret:VERIFY_KR}"`: requests carried `Bearer <value>`. (Some of omp's requests had no header; the connecting ones did.) |
| DX-7 | Pass | Claude Code `headersHelper` with the double-quoted `loadout.exe secret-header ...` form: requests carried `Bearer <value>`. |
| DX-8 | Pass | Codex `bearer_token_env_var = "VERIFY_KR"`, with the value from `loadout env --export --aliases VERIFY_KR --shell pwsh` (`$env:VERIFY_KR = '...'`): requests carried `Bearer <value>`. |
| DX-9 | Not run | Arch. |
| DX-10 | Pass (Claude Code) | A missing item and an unknown option: Claude Code lists `Failed to connect`, restarts the server once, and logs the server's stderr: `loadout-exec: VERIFY_TOKEN: ... 1Password item, field or Environment variable not found` (Claude Code redacts the word after the alias) and `loadout-exec: unknown option "--bogus-option"`. Exit codes themselves are not shown. omp and Codex not checked. |
| DX-11 | Pass, one Finding (V2) | Windows Sandbox, signed in: a keyring alias on a claude-desktop entry rendered `"command": "<loadout.exe>", "args": ["exec", "--ref", "VERIFY_TOKEN=keyring://loadout/VERIFY_KR", "--", ...]` with no `cmd /c` around `loadout.exe`; `verify-npx` (`loadout.exe exec --ref ... -- npx ...`) ran server-everything under `loadout.exe`, and the `envdump.cmd` launch entry received `VERIFY_TOKEN`. **Finding (FB-12):** the value was `\ufeff` + `verify-not-a-secret` (20 chars): Windows PowerShell 5.1 piped a UTF-8 BOM into `secrets set --keyring`, and Loadout stored it. Reproduced on the host. See [claude-desktop.md](claude-desktop.md). |
| DX-12 | Not run | Linux and macOS. |

While setting up DX-5, a hand edit of a Codex entry made earlier was
reported as `drifted ... (needs a decision: --on-conflict or adopt)` and
left alone, as designed.

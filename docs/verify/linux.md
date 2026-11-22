# Gate V1: Linux half (Arch Linux in WSL 2)

Run 2026-11-21 in a throwaway `archlinux` WSL 2 distribution (kernel
6.18.33.2-microsoft-standard-WSL2), user `v1`, with Windows interop and the
Windows `PATH` turned off in `/etc/wsl.conf`. No 1Password CLI. Clients:
Claude Code 2.1.330 (native installer; the npm package's native binary did
not install), Codex 0.171.0 and omp 19.1.0 (npm, with Bun 1.4.5). Loadout:
a debug build of the `v1` branch. Every case had its own `HOME`,
`LOADOUT_HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME` and `PI_CODING_AGENT_DIR`
under `/tmp/v1/<case>`. Keyring checks ran in a private D-Bus session with
gnome-keyring's Secret Service. The harness is
`%TEMP%\loadout-v1\linux\v1.py` (not committed).

Limits: WSL has no desktop session, so no unlock prompt can appear (see
FB-6); the AUR 1Password CLI 2.41 case (SE-4) was not set up.

| # | Result | Evidence |
|---|---|---|
| CC-1 | Pass | `$CLAUDE_CONFIG_DIR/.claude.json` read; `~/.claude.json` not created. |
| CC-2/CC-3 | Pass | Project key is the path as given (`/tmp/v1/cc/proj/app`). |
| CC-4 | Pass | `disabledMcpServers` disables inside the project only. |
| CC-5 | Finding (FB-3) | Remote entry without `type` skipped. |
| CC-6 | Recorded | `ws` accepted. |
| CC-7 | Finding (FB-2) | Dotted name loads. |
| CC-8 | Pass | `headersHelper` runs for `sse`. |
| CC-10 | Pass | `.mcp.json` server pending approval. |
| CC-11, CX-11, OMP-11 | Pass | All three detected from `PATH` in a clean profile. |
| CX-1 to CX-4 | Pass | As on Windows: list, trust gate, `enabled = false` and deep merge, `env_vars` forwarding. |
| CX-6 | Finding (FB-2) | Dotted name loads. |
| CX-7 | Finding (FB-3) | Legacy `[mcp.servers]` not loaded by 0.171 either. |
| OMP-1 | Pass | `$PI_CODING_AGENT_DIR/mcp.json` listed. |
| OMP-2 | Pass, recorded | Relative `PI_CONFIG_DIR` read from home. An absolute value is joined onto home (`<home>//tmp/...`) and the intended folder is not read; no crash on Linux (Windows: `ENOENT` at startup). |
| OMP-3 | Pass | Named profile ignores `PI_CODING_AGENT_DIR`. |
| OMP-4 | Pass | Project `{ "enabled": false }` makes the user entry inactive. |
| OMP-7 | Pass | `!echo verified` in `env` runs; server saw `verified`. |
| AP-4 | Pass | Claude Code, omp: `${VERIFY_VALUE}` in `env` and `args`, both expanded. Codex: `env_vars` and the inline argument fallback. |
| AP-5 | Pass | `native_disable`: entries kept with `enabled = false`, shown disabled by `codex mcp list` and inactive by omp. |
| SE-5 | Pass | Without `op`: `secrets providers` says "1Password CLI not found"; `secrets check` exits 5 with `provider_unavailable` and the per-machine fix; after `secrets set VERIFY_OP --this-machine --keyring` (Secret Service) it is `resolved`. |
| SE-6 | Pass (after FB-6) | With the Secret Service running: set, `resolved`, clear, `missing`. Without a Secret Service: `provider_unavailable`. With the collection locked: first `secrets check` hung (FB-6); after the fix it exits 5 with `locked`. |
| DX-1 (keyring) | Pass | Launch entry with `--ref VERIFY_TOKEN=keyring://loadout/VERIFY_OP`; the server saw the value; the client file does not hold it. |
| DX-6 | Pass | omp header command `!'<loadout>' secret get --ref '...' --template '...'` (single quotes on Linux); requests carried the bearer token. |
| DX-7 | Pass | Claude Code `headersHelper` with the single-quoted form; requests carried the token. |
| DX-8 | Pass | `loadout env --export --aliases VERIFY_KR` prints `export VERIFY_KR=...`; Codex sent `Bearer <value>`. |
| DX-9 | Pass, one finding (FB-5) | Without `op`: the entry is `blocked: provider_unavailable`, nothing is written, `loadout apply` exits 5. After the keyring override the entry launches with `keyring://` and the server gets the value. `server add`, whose instant apply blocked the entry, exited 0 (FB-5). |
| DX-12 | Finding, fixed (FB-6) | With the collection locked, `secrets check`, `status`, `apply` and `loadout exec` each hung for more than 25 s: the keyring lookup waited for an unlock prompt. After FB-6 none prompts: `status` and `apply` return at once, `exec` exits 77 `locked`. |
| OMP-5, 6, 8, 9; CX-5, 8 to 10; CC-9; AP-1 to AP-3, AP-7; DX-5, DX-10 | Not repeated | OS-independent client behavior or Windows-only items, covered on Windows; tool calls need a model. |
| SE-4 | Not run | Needs the AUR 1Password CLI 2.41. |

# Gate V1 results (Tier A)

Run 2026-11-21 by the implementer at the maintainer's request (SPEC 20.1
item 7 note). Windows half: Windows 11 Pro with Claude Code 2.1.318, Codex
0.168.0, omp 19.1.0 and git 2.56, Loadout the m13 debug build (this page and
the per-client pages). Linux half: Arch Linux in a throwaway WSL 2
distribution with Claude Code 2.1.330, Codex 0.171.0 and omp 19.1.0, no
1Password CLI ([linux.md](linux.md)).

## Method

- Every client ran with a throwaway profile under
  `%TEMP%\loadout-v1\<case>`: `USERPROFILE`, `HOME`, `APPDATA`,
  `LOCALAPPDATA`, `LOADOUT_HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME` and
  `PI_CODING_AGENT_DIR` all pointed there, and inherited `PI_*`, `OMP*`,
  `CLAUDE*`, `CODEX*`, `LOADOUT*`, `OP_*`, `GIT_*` variables were removed.
- Real client files (`~/.claude.json`, `~/.omp/agent/mcp.json`,
  `~/.omp/agent/config.yml`, `%APPDATA%\Claude\claude_desktop_config.json`,
  and the absent `~/.codex/config.toml`, `~/.claude/settings.json`,
  `~/.pi/agent/mcp.json`) were hashed before the run and checked after each
  step: unchanged throughout.
- Listers: `claude mcp list` and `codex mcp list --json` directly; omp's
  `/mcp list` in its TUI through a pseudo-terminal (pywinpty in a temp
  virtual environment). Servers: `fake-mcp-server` and a `.cmd` wrapper that
  records the environment and arguments a server is started with.
- Codex sessions (needed to make Codex start servers) ran `codex exec` with a
  dummy `OPENAI_API_KEY`: servers start, the model call then fails with 401.
- Secrets used the debug build's test keyring file and `op-stub` as `op`; the
  real `op` was taken off `PATH` for those steps. One exception: Codex passes
  only a fixed environment to servers, so one `loadout exec` under Codex did
  not see the test keyring variable and read the real Windows Credential
  Manager once (a lookup of `loadout/VERIFY_KR`, which does not exist;
  nothing was written).

## Summary

| Area | Run | Pass | Finding | Not run |
|---|---|---|---|---|
| [Claude Code](claude-code.md) | CC-1 to CC-11 | 7 | 4 (CC-3, CC-5, CC-6, CC-7) | none (CC-2 and CC-10 without the TUI) |
| [Codex](codex.md) | CX-1 to CX-8, CX-11 | 7 | 2 (CX-6, CX-7) | CX-9, CX-10 (need a model) |
| [omp](omp.md) | OMP-1 to OMP-5, 7 to 9, 11 | 8 | 1 (OMP-9 unclear) | OMP-6 (needs a model), OMP-10 (Pi, V2) |
| [Claude Desktop](claude-desktop.md) | CD-3 | 1 | 1 note | CD-1, 2, 4 to 7 (need Windows Sandbox or the maintainer's OK) |
| [Apply engine](apply.md) | AP-1 to AP-5, AP-7 | 6 | none | AP-6 (Claude Desktop) |
| [Secrets](secrets.md) | SE-7 (Windows); SE-5, SE-6 ([Linux](linux.md)) | 3 | FB-6 | SE-1 to SE-3 (real 1Password), SE-4 (AUR CLI 2.41), SE-6 on Windows |
| [Delivery](delivery.md) | DX-1, 5 to 8, 10 (Windows); DX-1, 6 to 9, 12 ([Linux](linux.md)) | 11 | 1 note (Codex environment), FB-5, FB-6 | DX-2 (covered by the automated Job Object test), DX-3, DX-4, DX-11 |
| [Linux](linux.md) | 29 rows | 24 | FB-2, FB-3, FB-5, FB-6 | SE-4 |
| Pi | none | | | moved to V2 |

## Fix briefs (before M9)

- **FB-1 Claude Code project keys (CC-3). Done.** Claude Code keys
  `projects` by the working directory exactly as given, with `\` turned
  into `/`: same case, no trailing separator. `C:/x/App` and `c:/x/app` are
  two different projects to it, and a backslash key is never read.
  `location::project_key` used to reuse any key that matched loosely and
  copy the backslash style of existing keys. It now always uses the exact
  form Claude Code uses (the project path, `/` separators, its own case, no
  trailing separator) and leaves keys of other spellings alone
  (`windows_project_keys_are_claude_codes_exact_spelling`). Left for M9:
  the project path's case must be the one Claude Code is started with; the
  M9 planner should warn when the file holds the same folder under another
  spelling.
- **FB-2 Dotted server names (CC-7, CX-6).** Claude Code and Codex both load
  `verify.dotted`. Loadout blocks such names for these clients
  (`invalid_id`). Decide whether to allow them as native names; low impact,
  since store ids keep their own rule.
- **FB-3 Entries the client skips (CC-5, CX-7).** Claude Code skips a remote
  entry without `type` (with a warning), and Codex 0.168 no longer loads the
  legacy `[mcp.servers]` table. Import should say these are not loaded by
  the client today instead of importing them silently.
- **FB-4 Codex header helper (opportunity).** Codex 0.168 reports an
  `http_headers_helper` field for http servers. It could give Codex
  secret-free header delivery without `loadout env --export` (SPEC 11.4
  Codex row). Needs the field's semantics checked first.
- **FB-5 Instant apply exit codes (DX-9).** `loadout server add` (and the
  other store edits that apply instantly: `server rm`, `secrets set`,
  `secrets clear`, `secrets map-op-env`) exit 0 even when their instant
  apply blocked an entry, left one unresolved or failed a file. `loadout
  apply` exits 5, 4 or 1 for the same plan. Change: return
  `apply_exit_code` of the instant apply report; existing CLI tests that add
  servers needing absent secrets expect 0 and must be adjusted.
- **FB-6 Locked Secret Service hangs (SE-6, DX-12). Done.** On Linux with
  the keyring collection locked and no prompt able to appear (WSL, SSH, a
  service), every command that touched a `keyring://` alias hung:
  `secrets check`, `status`, `apply`, `loadout exec` (so a client's server
  never started and the client waited). The `keyring` crate unlocks locked
  items and waits for the prompt. `OsKeyring` now asks the Secret Service
  first whether the entry (or, to write, the default collection) is locked,
  without unlocking, and reports `locked`; Loadout never asks for an
  unlock (DX-12: planning should not prompt). Re-run in WSL with the
  collection locked: `secrets check` exits 5 with `locked`, `status` and
  `apply` return at once, `loadout exec` exits 77 with
  `loadout-exec: T: locked: ...`; unlocked, SE-5, SE-6 and DX-9 still pass.
  New direct dependency on Linux: `secret-service` 5 (MIT OR Apache-2.0),
  already in the tree through `keyring`.

## Not run, and what it needs

- **Linux, desktop session:** WSL has none, so the unlock prompt itself
  (DX-12 on a real Arch desktop) and SE-4 (AUR CLI 2.41) are open.
- **Claude Desktop (CD-1, 2, 4 to 7, AP-6, DX-11):** Windows Sandbox
  (Windows Features, needs admin and a reboot), or the maintainer's OK to
  back up and restore the real `%APPDATA%\Claude` and MSIX configs.
- **1Password (SE-1 to SE-3, DX-3, DX-4) and the OS keyring (SE-6):** the
  maintainer's OK, ideally with a separate trial 1Password account.
- **Tool-call checks (CX-9, CX-10, OMP-6) and a full Claude Code session
  (AP-1 beyond onboarding):** a model login inside the throwaway profile.

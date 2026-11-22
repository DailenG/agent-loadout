# Secrets: gate V1, Windows

Method in [README](README.md).

| # | Result | Evidence |
|---|---|---|
| SE-1 | Partial | CLI 2.42.0 from WinGet (`Links\op.exe`), found by Loadout. With two accounts signed in to the desktop app, every `op` call without an account fails: "multiple accounts found. Use the --account flag or set the OP_ACCOUNT environment variable" (FB-7, now done). After the fix, with the service account: `secrets op-account <account>.1password.com` saved the account, `secrets check` resolved both Environment aliases, and the rendered Claude Code entry (`loadout exec --op-account <account>.1password.com --op-env ...`) started the server with `SERVER_TOKEN` set and neither `OP_ACCOUNT` nor the token in its environment. The desktop-app path with the setting was not repeated (it needs approval prompts). The `PATH`-without-WinGet case was not repeated. |
| SE-2 | Not run | `op://` item states (locked, dismissed, renamed) need a vault item; only an Environment was used. |
| SE-3 | Pass | Real Environment (family account) with `VERIFY_TOKEN` (concealed) and `VERIFY_PLAIN`. `op environment secret list <id> --format json` is a JSON array of `{name, value, concealed}`; the concealed value is masked even there. `secrets map-op-env <id> --all` mapped both, `secrets check` reports both `resolved`, and no value appeared in Loadout's output. Desktop-app approval: each `op` call took about 20 s and one prompt (one timed out at 60 s when the prompt went unnoticed); with the service account the same calls took 4 to 7 s and no prompt. |
| SE-4 | Not run | Arch with the AUR CLI. |
| SE-5 | Pass (Linux) | See [linux.md](linux.md). |
| SE-6 | Pass (sandbox) | In Windows Sandbox (its own Credential Manager, release build): `'verify-not-a-secret' \| loadout secrets set VERIFY_KR --keyring` stored it, `cmdkey /list` showed `LegacyGeneric:target=VERIFY_KR.loadout`, `secrets check VERIFY_KR` reported `resolved` (exit 0), `secrets clear VERIFY_KR` deleted it, and `check` then reported `missing` (exit 5) with the three fixes. The stored value itself was not read back there; newline stripping is covered on Linux. Linux: see [linux.md](linux.md). |
| SE-7 | Pass | `server add` with a description holding `https://user:password@...` and one holding a hand-built `ghp_` token: both refused with `leak_detected`, naming the file and line, neither value echoed. |

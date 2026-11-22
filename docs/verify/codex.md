# Codex: gate V1, Windows

Codex 0.168.0 with `CODEX_HOME` in the throwaway profile. Method in
[README](README.md). Sessions ran `codex exec` with a dummy API key: MCP
servers start, the model call fails.

| # | Result | Evidence |
|---|---|---|
| CX-1 | Pass | `codex mcp list --json` lists the entry Loadout wrote to `$CODEX_HOME/config.toml`. |
| CX-2 | Pass | `<P>/.codex/config.toml` servers are listed only after `[projects.'<P>'] trust_level = "trusted"`. |
| CX-3 | Pass | A project table holding only `enabled = false` disables the user entry inside `<P>` (enabled outside). A table holding only `args` merges over the user entry: `command` kept, `args` replaced (deep merge). |
| CX-4 | Pass | `env_vars = ["VERIFY_FORWARD"]`: the server saw `VERIFY_FORWARD=1` from Codex's environment. |
| CX-5 | Pass | An http entry sent `authorization: Bearer <VERIFY_BEARER>` and `x-verify: <VERIFY_HEADER>` to a local endpoint. `codex mcp get` also shows an `http_headers_helper` field (FB-4). |
| CX-6 | Finding (FB-2) | `[mcp_servers."verify.dotted"]` loads. |
| CX-7 | Finding (FB-3) | The legacy `[mcp.servers.verify-legacy]` table is not listed: Codex 0.168 ignores it. |
| CX-8 | Pass | Plain `command = "npx"` resolves to `npx.cmd` (a probe `npx.cmd` first on `PATH` was started). |
| CX-9 | Pass (V2) | Windows Sandbox, Codex 0.171.0 signed in: `enabled_tools` and `disabled_tools` were honored in a session. |
| CX-10 | Pass (V2) | Windows Sandbox: `startup_timeout_sec = 1` stopped a server that takes 3 s to start at about 760 ms; `2.5` is accepted (fractional seconds). Codex prints no timeout message. |
| CX-11 | Pass | Detected from `codex.exe` alone and `codex.cmd` alone on `PATH`. |

Also observed: Codex starts stdio servers with a fixed environment
(`APPDATA`, `COMSPEC`, `HOMEDRIVE`, `HOMEPATH`, `LOCALAPPDATA`, `PATH`,
`PATHEXT`, `PROGRAMDATA`, the `PROGRAMFILES` variables, `SYSTEMDRIVE`,
`SYSTEMROOT`, `TEMP`, `TMP`, `USERDOMAIN`, `USERNAME`, `USERPROFILE`,
`WINDIR`) plus `env_vars`. `loadout exec` works within it (see
[delivery](delivery.md)). Codex refused to create its PATH helper binaries
under a temp `CODEX_HOME` (a warning only).

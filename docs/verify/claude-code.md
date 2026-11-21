# Claude Code: gate V1, Windows

Claude Code 2.1.318 (native installer). Method in [README](README.md).

| # | Result | Evidence |
|---|---|---|
| CC-1 | Pass | With `CLAUDE_CONFIG_DIR` set, `claude mcp list` read `$CLAUDE_CONFIG_DIR/.claude.json` (the entry Loadout wrote showed `Connected`) and created no `.claude.json` in the profile. It also wrote `backups/.claude.json.backup.<n>` next to it. |
| CC-2 | Pass | `claude mcp add --scope local` in `<P>` wrote the key `C:/Users/.../proj/App`: forward slashes, upper-case drive letter, the folder's case as given, no trailing separator. Matches Loadout's default for a new key. (Taken without the trust dialog: the local-scope add writes the same key.) |
| CC-3 | Finding (FB-1) | Only the exact key is read. Rewriting it with backslashes, a lower-case drive letter, a lower-cased path or a trailing `/` hides the entry from `claude mcp list` in `<P>`. Started from `<P>` spelled in lower case, Claude Code wrote a second key in that spelling. Loadout's loose reuse of existing keys can therefore write where Claude Code does not read. |
| CC-4 | Pass | `disabledMcpServers: ["verify-user"]` under the project: `Disabled for this project` inside `<P>`, connected elsewhere. |
| CC-5 | Finding (FB-3) | A remote entry with `url` and no `type` is skipped: "has a `url` but no `type`; add `type`". Loadout reads it as http on import. |
| CC-6 | Recorded | `"type": "ws"` is accepted and listed as `(WS)`. Loadout reads it as `unsupported_transport`, as intended. |
| CC-7 | Finding (FB-2) | `verify.dotted` loads and connects. Loadout blocks such names for Claude Code. |
| CC-8 | Pass | `headersHelper` runs for `sse` as well as `http` (each wrote its marker file before connecting). |
| CC-9 | Pass | Plain `"command": "npx"` with `@modelcontextprotocol/server-everything` connects without `cmd /c`; `windows_shell_shim = false` is right. |
| CC-10 | Pass | A server in `<P>/.mcp.json` lists as `Pending approval (run claude to approve)` and is not started. |
| CC-11 | Pass | Loadout detects Claude Code from `claude.exe` alone and from `claude.cmd` alone on `PATH` (`server add --all-detected`, clean profile, no override dirs). |

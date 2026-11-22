# Pi: gate V2, Windows

Pi 1.6.0 (`@earendil-works/pi-coding-agent`, installed with
`npm install --prefix` into `%TEMP%\loadout-v2\pi`), omp 19.1.0, Loadout
the current debug build (copies of `loadout.exe`, `fake-mcp-server.exe`).
Every run set `PI_CODING_AGENT_DIR` to a throwaway dir under
`%TEMP%\loadout-v2`; Loadout runs also pointed `LOADOUT_HOME`, `HOME`,
`USERPROFILE`, `APPDATA` and `LOCALAPPDATA` at throwaway dirs and used the
test keyring file. Lister: `pi mcp list [--json]`, which needs no model or
login. Servers: `fake-mcp-server`, an `envdump.cmd` wrapper that records
the environment it starts with, and a small Python streamable-HTTP server
that logs request headers. Project trust was granted by writing
`$PI_CODING_AGENT_DIR/trust.json` (`{"<P>": true}`), the file `/trust`
saves to. The real `~/.claude.json`, `~/.omp/agent/mcp.json` and
`%APPDATA%\Claude\claude_desktop_config.json` had the same SHA-256 before
and after; `~/.codex/config.toml` and `~/.pi` stayed absent.

| # | Result | Evidence |
|---|---|---|
| PI-1 | Pass, one Finding | An absolute `PI_CODING_AGENT_DIR`: `verify-user` listed, `--json` `source` is `$PI_CODING_AGENT_DIR\mcp.json`. `~/piagent` and `~\piagent` are expanded by Pi to `<home>\piagent` and that file is listed. **Finding:** with `PI_CODING_AGENT_DIR=~/piagent`, `loadout server add --client pi` wrote `<home>\.pi\agent\mcp.json` (the fallback of `Location::EnvDir` in `pi::agent`), and `pi mcp list` then reported "No MCP servers configured" in `<home>\piagent\mcp.json`. Loadout should expand a leading `~/` or `~\` the way Pi does instead of falling back. |
| PI-2 | Pass | Before trust: only `verify-user`, plus "`<P>\.pi\mcp.json` is ignored because the project is not trusted". After the trust decision is saved: `verify-project` listed with scope `project`. `pi mcp list` takes no `-a`/`--approve` (usage error), so a saved decision is the only headless way. |
| PI-3 | Pass | User entry with `env` and `"exposure": "direct"`; `{ "enabled": false }` in the trusted project file: inside `<P>` the entry is `disabled`, with `override` naming the project file and `exposure` still `direct`; outside `<P>` it is `connected`. `{ "exposure": "hidden" }` alone also overrides partially (the entry still connects). Matches `ProjectDisable::EnabledFalse { partial: true }`. |
| PI-4 | Pass, recorded | Both names written by hand: `verify-tool` connects and `pi mcp list` exits 1 with `server "verify_tool" conflicts with "verify-tool"`. `pi mcp add` itself accepts the second name and writes both entries without a warning. Loadout's `collision` matches what Pi enforces at load time. |
| PI-5 | Pass | No `type` and `"type": "streamable-http"`: both connected over streamable HTTP (2 tools). `"type": "sse"`: config error "legacy SSE transport is not supported". |
| PI-6 | Pass (via connect) | A tool call needs a model, so the unit was checked on connect instead (the `timeout` applies per request). HTTP server that answers after 3 s: `"timeout": 1` failed with "MCP request timed out after 1000ms", `"timeout": 5` connected. Seconds, as in `TimeoutField` `TimeUnit::Seconds`. |
| PI-7 | Pass, one Finding | `env` `"VERIFY": "!echo verified"`: server saw `verified`; header `"X-Verify": "!echo verified"`: request carried `verified`. A literal leading `!` can be escaped as `$!` (`"$!echo literal"` reached the server as `!echo literal`; header `$!lit` as `!lit`). Loadout writes values as rendered: `loadout server add --env "VERIFY=!echo ran-as-command"` wrote that value and the server saw `ran-as-command`. **Finding:** Pi also expands bare `$NAME`, and `$$` is a literal `$` (`"a$HOMEDRIVE-b$$c"` reached the server as `aC:-b$c`); an unset variable fails the whole server ("Failed to resolve MCP server ... env ... from environment variable"). Loadout models Pi as `EnvRefSyntax::DollarBrace` only and writes literals unescaped: `--env "LIT=a$b$$c"` was written as is and the server then failed to start. For Pi, a literal `$` must be written as `$$` and a literal leading `!` as `$!` (Pi `CAPS`, `CommandRef::Bang`, `native_to_template` for reading back). |
| PI-8 | Pass, one Finding | `pi install npm:pi-mcp-adapter` added `"packages": ["npm:pi-mcp-adapter"]` to `$PI_CODING_AGENT_DIR/settings.json` and installed the package under `$PI_CODING_AGENT_DIR/npm/node_modules/pi-mcp-adapter` (plus `npm/package.json`, `package-lock.json`, `.gitignore`); nothing under `extensions/`. With `-l`: the same entry in `<P>/.pi/settings.json` and the package under `<P>/.pi/npm/`. `loadout status` reported `client_mcp_replaced_by_extension` (path `settings.json`) for the user install. **Finding:** with only the `-l` install, in a trusted Loadout project that writes `<P>/.pi/mcp.json`, no warning appears: `apply::plan` calls `replacing_extension(spec, .., None)`, so the `Location::Project` paths and `<P>/.pi/settings.json` in `pi::MCP_ADAPTER` are never checked. Also, `loadout effective` shows the servers as loaded with no warning while the adapter is installed. `pi mcp list` still lists `mcp.json` servers (shell commands always use the built-in support); `/mcp` in a session not run (needs a model). |
| PI-9 | Pass, recorded | `"extensions": ["-builtin:mcp"]` in the user settings: `pi mcp list` still lists and connects the servers (per Pi's docs, shell commands always use the built-in support; sessions skip MCP). `loadout status` reports nothing, as expected for now. |
| PI-10 | Pass | `"command": "npx", "args": ["-y", "<local package dir>"]` connected (the package's bin starts `fake-mcp-server`). A `.cmd` file as `command` also connects. `windows_shell_shim = false` is right. |
| PI-11 | Pass (listing) | `"toolExposure": { "secret": "hidden" }` on the HTTP server: `pi mcp list` shows `tools: ping, secret [hidden]` and `--json` echoes `toolExposure`. That the tool is unreachable from the model was not run (needs a model); Pi's docs define `hidden` as "registered but unreachable". |
| PI-12 | Pass | Empty profile, `PATH` with only `System32` plus the dir holding `pi.cmd`: `server add --all-detected` wrote `<home>\.pi\agent\mcp.json` (Pi detected, omp not). Same with only a `<home>\.pi\agent` dir and no `pi` on `PATH`. With neither, no file was written. |

End to end: `loadout server add verify-pi --client pi -- <fake-mcp-server>`
wrote `$PI_CODING_AGENT_DIR/mcp.json` and `pi mcp list` showed `verify-pi`
connected. With `secrets set VERIFY_KR --ref keyring://loadout/VERIFY_KR`
and `--env VERIFY=${secret:VERIFY_KR}`, the entry was written as launch
delivery (`loadout.exe exec --ref VERIFY=keyring://loadout/VERIFY_KR --
envdump.cmd`); it connected, the server saw `VERIFY=verify-not-a-secret`,
and the value is not in `mcp.json`.

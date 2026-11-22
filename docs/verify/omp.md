# oh-my-pi (omp): gate V1, Windows

omp 19.1.0, driven through its TUI in a pseudo-terminal (onboarding skipped
with Esc). `/mcp list` works without a model; `omp -p` and tool commands
need one. Method in [README](README.md).

| # | Result | Evidence |
|---|---|---|
| OMP-1 | Pass | With `PI_CODING_AGENT_DIR` set, `/mcp list` shows "User level (`$PI_CODING_AGENT_DIR\mcp.json`)" and the entry Loadout wrote, connected. |
| OMP-2 | Pass, recorded | `PI_CONFIG_DIR=.omp-verify` reads `~/.omp-verify/agent/mcp.json`. An absolute `PI_CONFIG_DIR` is joined onto the home folder (`<home>\C:\...\agent`) and omp exits with `ENOENT` at startup. Loadout ignoring absolute values is right. |
| OMP-3 | Pass | `omp --profile verify` with `PI_CODING_AGENT_DIR` set lists only `~/.omp/profiles/verify/agent/mcp.json`. |
| OMP-4 | Pass | Inside `<P>`, both the full entry with `"enabled": false` and `{ "enabled": false }` alone in `<P>/.omp/mcp.json` leave the user entry not connected and the project entry inactive. |
| OMP-5 | Pass | From the `omp_new_file` golden plus an extra entry and unknown keys, `/mcp disable notes` added `"enabled": false` to `notes` and kept `$schema`, the other entry, `x-custom` and `x-top`, with two-space indent. It dropped the final newline. |
| OMP-6 | Not run | Calling a tool needs a model. |
| OMP-7 | Pass | `"env": {"VERIFY": "!echo verified", "VERIFY_PAD": "!echo padded   "}`: the server saw `verified` and `padded` (output trimmed). |
| OMP-8 | Pass | Plain `"command": "npx"` connects. |
| OMP-9 | Pass, one unclear | In `disabledServers`, the entry is not connected. In `enabledServers` with `"enabled": false` in the entry, omp's startup line says it connected, but `/mcp list` shows it inactive; which wins is unclear. Loadout never edits either list. |
| OMP-10 | Pass, one Finding (gate V2) | One `$PI_CODING_AGENT_DIR/mcp.json` with `verify-shared` (`fake-mcp-server`, `"timeout": 5`), throwaway home. omp 19.1.0 `/mcp list` (TUI in a pseudo-terminal): "User level (`$PI_CODING_AGENT_DIR\mcp.json`)", `verify-shared` not connected, "timed out after 5ms". Pi 1.6.0 `pi mcp list`: same file, `verify-shared` connected (5 s). Both read the file, and `timeout` means ms to omp and s to Pi. Loadout treats it as shared: `loadout effective` lists the entry for both clients from `%PI_CODING_AGENT_DIR%/mcp.json`. **Finding:** a plain stdio server enabled for both (`server add verify-both --client omp --client pi`, no timeout) is blocked with `collision` ("clients omp, pi share this file and want different entries"), because the omp renderer writes `"type": "stdio"` and the Pi one omits `type`. Pi accepts `"type": "stdio"`, so the two renderings could agree; as is, no server can be enabled for both clients while `PI_CODING_AGENT_DIR` is set. Details in [pi.md](pi.md). |
| OMP-11 | Pass | Detected from `omp.cmd` alone on `PATH` and from a `~/.omp` folder alone. |

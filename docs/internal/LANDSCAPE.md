# Landscape: tools to borrow from (verified 2026-11-14)

Base: SkillDock (MIT), Dailen's fork https://github.com/DailenG/skilldock, cloned at ../skilldock (READ-ONLY reference). Tauri (Rust backend in src-tauri, React/TS frontend in src). Upstream wanghuan9/skilldock is 404.
SkillDock model: central registry ~/.skilldock/mcp-servers.json (one command/args/env record + enabled_app_ids); enabling upserts into each client's native config (Cursor ~/.cursor/mcp.json, Claude Code ~/.claude.json, Codex ~/.codex/config.toml, JSONC/TOML/YAML adapters for many incl. Pi/OMP); disable removes; "local-align" detects out-of-band edits. Skills in ~/.skilldock/skills symlinked into tools; scans ~/.agents/skills. GitHub sync (device flow / PAT) to private skilldock-backup repo with 3-way merge; snapshots strip secret values (required_secrets) and restore prompts for them. FAILURE: its built-in GitHub OAuth client id died with upstream -> sync 404s; only a SKILLDOCK_GITHUB_CLIENT_ID runtime override worked around it.

Best ideas to steal:
- CC Switch https://github.com/farion1231/cc-switch (MIT, Tauri, very active): MCP matrix with per-app toggles; import existing client configs; writes stdio/HTTP/SSE into Claude Code, Codex, Gemini, OpenCode, Grok, Pi; skills; masked env editor. Gap: no Cursor/Windsurf/VS Code/Claude Desktop MCP.
- agentsync https://github.com/spxrogers/agentsync (MIT, Go CLI, active): canonical ~/.agentsync/mcp/*.toml (git-committable); `apply` writes native entries; per-server agents allowlist; ${secret:} (age vault) and ${env:} refs resolved at apply, never captured back; bidirectional drift detect/reconcile; project + global Cursor paths; skills.
- rulesync https://github.com/dyoshikawa/rulesync (MIT, active): project .rulesync/ + --global mode; supports Pi and oh-my-pi (omp); "delete": false so it never sweeps entries other tools installed.
- MCPM https://github.com/pathintegral-institute/mcpm.sh (MIT): install-once, profiles, writes Claude Desktop, Cursor, Windsurf, VS Code, Cline, Continue, Goose.
- gaal https://github.com/getgaal/gaal (AGPL - ideas only, copy NO code): writes env REFERENCES instead of values; upserts without clobbering other keys; git multi-machine sync.
- ruler https://github.com/intellectronica/ruler (MIT): project-scoped .ruler/ruler.toml [mcp_servers.*] -> per-agent project MCP files.
Excluded on purpose: proxy/gateway/container runtimes (ToolHive, 1MCP, Docker MCP gateway, MetaMCP).

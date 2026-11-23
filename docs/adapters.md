# Client adapters

Loadout writes MCP servers (and, for some clients, skills) into the native
config files of 38 AI coding clients. This page lists every adapter, where
it writes, what it can express, and how far each one has been checked
against the real client. The design is in [SPEC.md](../SPEC.md) section 7;
the user guide is [user-guide.md](user-guide.md).

omp (oh-my-pi) is the recommended client: Loadout is built and tested with
omp first, and the app lists it first and marks it "Recommended" wherever it
shows clients. That order is presentation only. The generated table below
keeps the adapter table's order, which detection, scans and apply follow.

## Supported clients

The table below is generated from the adapter table in
`crates/agent-loadout-core/src/adapters/` by the test
`crates/agent-loadout-core/tests/docs_adapters.rs`. Do not edit it by hand:
change the code, then run

```
LOADOUT_UPDATE_DOCS=1 cargo test -p agent-loadout-core --test docs_adapters
```

(in PowerShell, set `$env:LOADOUT_UPDATE_DOCS = "1"` first).

How to read it:

- **Tier**: A and B are the daily clients (checked at gates V1 and V2), C is
  supported, D is best effort or ported from SkillDock (SPEC 20.2). "read
  only" means Loadout imports from and shows the client's files but never
  writes them.
- **OS**: the operating systems the client has an MCP config on ("all" is
  Windows, Linux and macOS).
- **Format and container**: the file format and the key path of the server
  map inside the file. `[name]` marks a YAML list matched by its `name` key;
  `<P>` in a key path is the project folder.
- **Paths**: `~` is your home folder (on Windows, your profile folder;
  `HOME` is ignored). `<AppData>` is `%APPDATA%` on Windows,
  `~/Library/Application Support` on macOS and `$XDG_CONFIG_HOME` (default
  `~/.config`) on Linux. `$XDG_CONFIG_HOME` means that variable, default
  `~/.config`, on every OS including Windows. `${VAR:-x}` means the value of
  the environment variable `VAR` when it is set to an absolute path (for
  `PI_CONFIG_DIR`, a folder name under home), else `x`. `<P>` is the project
  folder. When a cell lists several files joined by "or", Loadout writes the
  first one that exists, or creates the first one.
- **Project file / Project-local file**: the shared project file (meant to
  be committed) and the private one. "none" means the client has no such
  scope in Loadout, and project servers for that client are reported as
  not supported at that scope.
- **Native disable**: how Loadout turns an entry off without removing it.
  "none" means a disabled server's entry is removed instead.
- **Hide a user server in a project**: how a project manifest's
  `enabled = false` for a system server is written for this client. "none"
  means it cannot be expressed (`cannot_suppress_in_project`).
- **Skill folders**: where `loadout skills enable` links or copies skills
  for the client (user scope, then project scope). "none" means Loadout does
  not distribute skills to that client yet.

<!-- adapters:begin -->
| Id | Name | Tier | OS | Format and container | User file | Project file | Project-local file | Transports | Native disable | Hide a user server in a project | Skill folders (user, project) |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `claude-code` | Claude Code | A | all | json `mcpServers` | `${CLAUDE_CONFIG_DIR:-~}/.claude.json` | `<P>/.mcp.json` | `${CLAUDE_CONFIG_DIR:-~}/.claude.json` at `projects.<P>.mcpServers` | stdio, http, sse | none | listed in `projects.<P>.disabledMcpServers` | `${CLAUDE_CONFIG_DIR:-~/.claude}/skills`, `<P>/.claude/skills` |
| `omp` | oh-my-pi | A | all | json `mcpServers` | `${PI_CODING_AGENT_DIR:-~/${PI_CONFIG_DIR:-.omp}/agent}/mcp.json`; profiles: `~/${PI_CONFIG_DIR:-.omp}/profiles/<name>/agent/mcp.json` | `<P>/.omp/mcp.json` | none | stdio, http, sse | `enabled: false` | full entry with `enabled: false` | `${PI_CODING_AGENT_DIR:-~/${PI_CONFIG_DIR:-.omp}/agent}/skills`, `<P>/.omp/skills` |
| `pi` | Pi | B | all | json `mcpServers` | `${PI_CODING_AGENT_DIR:-~/.pi/agent}/mcp.json` | `<P>/.pi/mcp.json` | none | stdio, http | `enabled: false` | `{ enabled: false }` entry | `${PI_CODING_AGENT_DIR:-~/.pi/agent}/skills`, `<P>/.pi/skills` |
| `codex` | Codex | A | all | toml `mcp_servers` | `${CODEX_HOME:-~/.codex}/config.toml` | `<P>/.codex/config.toml` | none | stdio, http | `enabled: false` | `{ enabled: false }` entry | `${CODEX_HOME:-~/.codex}/skills`, `<P>/.agents/skills` |
| `claude-desktop` | Claude Desktop | A | windows, macos | json `mcpServers` | `<AppData>/Claude/claude_desktop_config.json`; mirror: `%LOCALAPPDATA%/Packages/Claude_*/LocalCache/Roaming/Claude/claude_desktop_config.json` | none | none | stdio | none | none | none |
| `copilot` | GitHub Copilot | B | all | json `mcpServers` | `${COPILOT_HOME:-~/.copilot}/mcp-config.json` | `<P>/.mcp.json` | none | stdio, http, sse | none | none | `${COPILOT_HOME:-~/.copilot}/skills`, `<P>/.github/skills` |
| `cursor` | Cursor | B | all | json `mcpServers` | `~/.cursor/mcp.json` | `<P>/.cursor/mcp.json` | none | stdio, http, sse | none | none | `~/.cursor/skills`, `<P>/.cursor/skills` |
| `gemini` | Gemini CLI | C | all | json `mcpServers` | `~/.gemini/settings.json` | `<P>/.gemini/settings.json` | none | stdio, http, sse | none | listed in `mcp.excluded` | none |
| `antigravity` | Antigravity | C | all | json `mcpServers` | `~/.gemini/config/mcp_config.json` | `<P>/.agents/mcp_config.json` | none | stdio, http | none | none | none |
| `opencode` | OpenCode | C | all | jsonc `mcp` | `$XDG_CONFIG_HOME/opencode/opencode.json` or `$XDG_CONFIG_HOME/opencode/opencode.jsonc` | `<P>/opencode.json` or `<P>/opencode.jsonc` or `<P>/.opencode/opencode.json` or `<P>/.opencode/opencode.jsonc` | none | stdio, http, sse | `enabled: false` | full entry with `enabled: false` | none |
| `grok` | Grok Build | C | all | toml `mcp_servers` | `~/.grok/config.toml` | `<P>/.grok/config.toml` | none | stdio, http | none | none | none |
| `vscode-legacy` | VS Code (legacy mcp.json) | D, read only | all | jsonc `servers` | `<AppData>/Code/User/mcp.json` (computed, see notes) | `<P>/.vscode/mcp.json` | none | stdio, http, sse | none | none | none |
| `windsurf` | Windsurf | D | all | json `mcpServers` | `~/.codeium/windsurf/mcp_config.json` | `<P>/.windsurf/mcp_config.json` | none | stdio, http, sse | `disabled: true` | none | none |
| `devin` | Devin Desktop | D | all | json `mcpServers` | `~/.config/devin/mcp_config.json` (computed, see notes) | `<P>/.devin/mcp_config.json` | `<P>/.devin/mcp_config.local.json` | stdio, http, sse | none | none | none |
| `augment` | Augment | D | all | json `mcpServers` | `~/.augment/settings.json` | none | none | stdio, http, sse | none | none | none |
| `cline` | Cline | D | all | json `mcpServers` | `~/.cline/data/settings/cline_mcp_settings.json` | none | none | stdio, http, sse | `disabled: true` | none | none |
| `roo-code` | Roo Code | D | all | json `mcpServers` | `<AppData>/Code/User/globalStorage/RooVeterinaryInc.roo-cline/settings/mcp_settings.json` | none | none | stdio, http, sse | `disabled: true` | none | none |
| `kilo-code` | Kilo Code | D | all | json `mcpServers` | `<AppData>/Code/User/globalStorage/kilocode.kilo-code/settings/mcp_settings.json` | none | none | stdio, http, sse | `disabled: true` | none | none |
| `kiro` | Kiro | D | all | json `mcpServers` | `~/.kiro/settings/mcp.json` | none | none | stdio, http, sse | `disabled: true` | none | none |
| `junie` | Junie | D | all | json `mcpServers` | `~/.junie/mcp/mcp.json` | none | none | stdio, http, sse | none | none | none |
| `droid` | Factory Droid | D | all | json `mcpServers` | `~/.factory/mcp.json` | none | none | stdio, http, sse | none | none | none |
| `commandcode` | Command Code | D | all | json `mcpServers` | `~/.commandcode/mcp.json` | none | none | stdio, http, sse | none | none | none |
| `crush` | Crush | D | all | json `mcp` | `$XDG_CONFIG_HOME/crush/crush.json` | none | none | stdio, http, sse | none | none | none |
| `qoder` | Qoder | D | all | json `mcpServers` | `~/.config/Qoder/SharedClientCache/mcp.json` | none | none | stdio, http, sse | none | none | none |
| `trae` | Trae | D | all | json `mcpServers` | `<AppData>/Trae/User/mcp.json` (computed, see notes) | none | none | stdio, http, sse | none | none | none |
| `trae-cn` | Trae CN | D | all | json `mcpServers` | `<AppData>/Trae CN/User/mcp.json` (computed, see notes) | none | none | stdio, http, sse | none | none | none |
| `codebuddy` | CodeBuddy | D | all | json5 `mcpServers` | `~/.codebuddy/.mcp.json` or `~/.codebuddy/mcp.json` or `~/.codebuddy.json` | none | none | stdio, http, sse | none | none | none |
| `iflow` | iFlow CLI | D | all | json `mcpServers` | `~/.iflow/settings.json` or `~/.iflow/mcp/config.json` | none | none | stdio, http, sse | none | none | none |
| `qwen-code` | Qwen Code | D | all | json `mcpServers` | `~/.qwen/settings.json` | none | none | stdio, http, sse | none | none | none |
| `zencoder` | Zencoder | D | all | json `zencoder.mcpServers` | `~/.zencoder/settings.json` | none | none | stdio, http, sse | none | none | none |
| `openclaw` | OpenClaw | D | all | json5 `mcp.servers` | `~/.openclaw/openclaw.json` | none | none | stdio, http, sse | `enabled: false` | none | none |
| `mimo-code` | MiMo Code | D | all | json `mcp` | `~/.config/mimocode/mimocode.json` | none | none | stdio, http, sse | `enabled: false` | none | none |
| `workbuddy` | WorkBuddy | D | all | json `mcpServers` | `~/.workbuddy/.mcp.json` | none | none | stdio, http, sse | none | none | none |
| `zcode` | ZCode | D | all | json `mcp.servers` | `~/.zcode/cli/config.json` | none | none | stdio, http, sse | none | none | none |
| `continue` | Continue | D | all | yaml `mcpServers.[name]` | `~/.continue/config.yaml` (computed, see notes) | none | none | stdio, http, sse | none | none | none |
| `goose` | Goose | D | all | yaml `extensions` | `$XDG_CONFIG_HOME/goose/config.yaml` (computed, see notes) | none | none | stdio, http | `enabled: false` | none | none |
| `hermes` | Hermes Agent | D | all | yaml `mcp_servers` | `~/.hermes/config.yaml` | none | none | stdio, http, sse | none | none | none |
| `intellij` | IntelliJ IDEA | D | none | none | none | none | none | none | none | none | none |
<!-- adapters:end -->

### Notes on computed user files

Some clients' user files are computed at run time rather than taken from a
fixed path; the table shows the default.

| Client | How the user file is chosen |
|---|---|
| `vscode-legacy` | `<AppData>/<variant>/User/mcp.json` plus `User/profiles/<id>/mcp.json` for the selected profiles. Variants (`Code`, `Code - Insiders`, `VSCodium`) and profiles come from `import_variants` and `import_profiles` in `store/clients.toml` (default: `Code`, default profile only). Read only: Loadout never writes these files. |
| `devin` | `%APPDATA%\devin\mcp_config.json` on Windows; `~/.config/devin/mcp_config.json` on Linux and macOS (whatever `XDG_CONFIG_HOME` says). |
| `trae`, `trae-cn` | `User/mcp.json` plus every existing `User/profiles/<id>/mcp.json`, as SkillDock did. |
| `continue` | `~/.continue/config.yaml`. While Continue still uses only `config.json`, the target is refused (`read_only_format`) instead of creating `config.yaml`, which would replace the JSON setup. |
| `goose` | `$GOOSE_PATH_ROOT/config/config.yaml` when that variable is set, else `<AppData>/Block/goose/config/config.yaml` on Windows and `$XDG_CONFIG_HOME/goose/config.yaml` elsewhere. |
| `omp` | The default profile is the listed file; named profiles are selected with `profiles = [...]` under `[omp]` in `store/clients.toml`. |
| `claude-desktop` | The Microsoft Store (MSIX) build keeps a virtualized copy of the config; when it exists, both files are written and the apply reports `msix_dual_write`. |

## Verification status

`verified` means the adapter's paths, entry shape and capabilities were
checked against the real client at gate V1 (Tier A) or V2 (Tier B); results
are in [docs/verify/](verify/README.md). `[verify]` means the adapter is
built from documentation and SkillDock's tables and still has open checks,
listed by id from [scripts/verify-clients.md](../scripts/verify-clients.md).
Every adapter is covered by automated tests (golden files, a fake
environment, the apply engine); the ids below are about real clients only.

| Id | Status | Checked | Open checks |
|---|---|---|---|
| `claude-code` | verified | V1, Windows and Arch Linux (WSL): CC-1 to CC-11 | macOS not checked |
| `omp` | verified | V1, Windows and Arch Linux (WSL): OMP-1 to OMP-5, OMP-7 to OMP-9, OMP-11; V2: OMP-6, OMP-10 | OMP-9 unclear (omp's own `enabledServers` list, which Loadout never edits); macOS not checked |
| `pi` | verified | V2, Windows: PI-1 to PI-12 | Linux and macOS not checked |
| `codex` | verified | V1, Windows and Arch Linux (WSL): CX-1 to CX-8, CX-11; V2: CX-9, CX-10 | macOS not checked |
| `claude-desktop` | verified (Windows) | V1 and V2, Windows: CD-1, CD-3 to CD-6 | CD-2 (MSIX copy), CD-7 (macOS) |
| `copilot` | verified | V2, Windows: COP-1 to COP-6, COP-8 to COP-11 | COP-7 (tool filter); Copilot CLI parts of COP-4, COP-5, COP-9 (blocked by an account policy) |
| `cursor` | verified | V2, Windows: CUR-1 to CUR-10 | Linux and macOS not checked |
| `gemini` | `[verify]` | none | GEM-1 to GEM-11 |
| `antigravity` | `[verify]` | none | ANT-1 to ANT-10 |
| `opencode` | `[verify]` | none | OC-1 to OC-10 |
| `grok` | `[verify]` | none | GRK-1 to GRK-10 |
| `vscode-legacy` | verified (Windows) | V2, Windows: VSL-1 to VSL-6 | VSL-1 on Arch Linux; VSL-3 tool picker |
| `windsurf` | `[verify]` | none | WSF-1 to WSF-8 |
| `devin` | `[verify]` | none | DEV-1 to DEV-8 |
| `augment` | `[verify]` | none | TD-1, TD-2, TD-14, TD-15 |
| `cline` | `[verify]` | none | TD-1, TD-3, TD-14, TD-15 |
| `roo-code` | `[verify]` | none | TD-1, TD-3, TD-14, TD-15 |
| `kilo-code` | `[verify]` | none | TD-1, TD-3, TD-14, TD-15 |
| `kiro` | `[verify]` | none | TD-1, TD-2, TD-4, TD-14, TD-15 |
| `junie` | `[verify]` | none | TD-1, TD-2, TD-14, TD-15 |
| `droid` | `[verify]` | none | TD-1, TD-2, TD-14, TD-15 |
| `commandcode` | `[verify]` | none | TD-1, TD-2, TD-14, TD-15 |
| `crush` | `[verify]` | none | TD-1, TD-2, TD-14, TD-15 |
| `qoder` | `[verify]` | none | TD-1, TD-2, TD-14, TD-15 |
| `trae` | `[verify]` | none | TD-1, TD-2, TD-7, TD-14, TD-15 |
| `trae-cn` | `[verify]` | none | TD-1, TD-2, TD-7, TD-14, TD-15 |
| `codebuddy` | `[verify]` | none | TD-1, TD-2, TD-8, TD-14, TD-15 |
| `iflow` | `[verify]` | none | TD-1, TD-2, TD-8, TD-14, TD-15 |
| `qwen-code` | `[verify]` | none | TD-1, TD-10, TD-14, TD-15 |
| `zencoder` | `[verify]` | none | TD-1, TD-2, TD-6, TD-14, TD-15 |
| `openclaw` | `[verify]` | none | TD-1, TD-9, TD-14, TD-15 |
| `mimo-code` | `[verify]` | none | TD-1, TD-9, TD-14, TD-15 |
| `workbuddy` | `[verify]` | none | TD-1, TD-2, TD-14, TD-15 |
| `zcode` | `[verify]` | none | TD-1, TD-2, TD-5, TD-14, TD-15 |
| `continue` | `[verify]` | none | TD-1, TD-11, TD-14 to TD-17 |
| `goose` | `[verify]` | none | TD-1, TD-12, TD-14 to TD-16 |
| `hermes` | `[verify]` | none | TD-1, TD-13, TD-14, TD-15 |
| `intellij` | `[verify]` | none | TD-15 (detection only; no MCP target) |

Findings from the gates that changed adapter behavior (fix briefs FB-1 to
FB-17) are described in [docs/verify/README.md](verify/README.md). Notable
ones for users:

- Claude Code keys `projects` in `~/.claude.json` by the exact spelling of
  the folder path it was started in (CC-3). Start Claude Code from the
  folder path with the same case Loadout uses.
- Claude Desktop starts newly added servers only after a full restart
  (quit it from the system tray or menu bar too); the apply report says so
  (`restart_required`).
- Cursor lists servers from `<P>/.cursor/mcp.json` as disabled until you
  turn each on once in Cursor (Customize > MCPs); Loadout reports
  `client_enable_required`. Cursor runs a same-named user and project
  server side by side (`duplicate_across_layers`).
- omp applies an entry's `timeout` (milliseconds) to server startup too, so
  a small value can stop a slow server (for example `npx` on its first run)
  from starting at all.
- omp and Pi share one file when `PI_CODING_AGENT_DIR` is set; a `timeout`
  in such a file is blocked (`timeout_unit_conflict`) because omp reads it
  in milliseconds and Pi in seconds.

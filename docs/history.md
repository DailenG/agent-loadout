# Development history

Loadout was built in milestones defined in [SPEC.md](../SPEC.md) section
20. Each milestone's completion notes are in [milestones/](milestones);
they record what was true when the milestone landed, not current behavior.
For user-visible changes per release, see the [changelog](../CHANGELOG.md).

| Milestone | What landed |
|---|---|
| M0 | Workspace bootstrap |
| M1 | Store schemas and validation |
| M2 | Git-backed store; `loadout server add\|ls\|show\|rm`, `loadout history` |
| M3 | Format-preserving JSON, JSONC and TOML editors |
| M4 | Client adapter framework; Claude Code and Claude Desktop adapters |
| M5 | Codex, omp and Pi adapters |
| M6 | User-scope apply engine with the ownership ledger; `loadout status\|diff\|apply\|rollback\|import\|adopt`; one store commit per apply run |
| M7 | Secret references; env, keyring and 1Password providers; the leak scanner on every store commit; `loadout secrets ls\|check\|providers\|set\|clear` |
| M8 | Secret delivery per client (`env`, `launch` through `loadout exec`, `command` hooks, opt-in `inline`); `loadout env --export`; `loadout secrets map-op-env\|service-account` |
| M9 | Project scope: manifests and trust, project apply, `loadout project` and `loadout effective` |
| M10 | Tier B adapters: VS Code with GitHub Copilot, Cursor, and the legacy VS Code `mcp.json` (read only) |
| M11 | History views, tags and restore: `loadout show\|tag\|restore\|backup`, `loadout diff --rev` |
| M12 | Folder remotes with per-entity pull merge; `loadout remote add\|ls\|rm\|push\|pull\|machines`; `restore --from` |
| M13 | Git remotes through the system git (`system`, `gh` or `token` auth); `loadout remote token\|create-github` |
| M14 | Desktop app shell: Tauri 2 with typed IPC, the SkillDock shell port |
| M15 | Desktop MCP page with per-client pills, Form/TOML/Preview editor and paste-anything; Tools page; Pending changes drawer with Undo this apply |
| M16 | Projects page, effective matrix, drift resolver |
| M17 | History page, versioning and secrets settings, first-run backup remote |
| M18 | Skills store and CLI: installs from git, folders and archives; junction links on Windows; project links hidden through `.git/info/exclude` |
| M19 | Desktop Skills and Install pages |
| M20 | Migration from SkillDock (`loadout migrate skilldock`, the wizard) |
| M21 | Tier C adapters: Gemini CLI, Antigravity, OpenCode, Grok Build |
| M22 | Tier D: Windsurf, Devin Desktop, YAML and JSON5 editors, the ported user-scope clients |
| M23 | Packaging: NSIS installer with `loadout` on the user `PATH`, AppImage/deb/rpm, AUR PKGBUILDs, winget manifests, the release workflow with Azure Trusted Signing, the optional updater. First release 0.1.2. |
| M24 | Desktop MCP page at project scope; Discard pending |
| M25 | Chocolatey package |
| M26 | The sidebar Scope switcher drives Skills, MCP and Tools; "System" renamed "User" |
| M27 | omp listed first and marked Recommended |
| M28 | Header toolbar alignment, installer branding, design review |

Gates V1 (Tier A) and V2 (Tier B), the real-client checks, are partly run;
see [verify/README.md](verify/README.md).

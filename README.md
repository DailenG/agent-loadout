# Loadout

Loadout (`agent-loadout`) manages MCP servers, skills and secret references for
AI coding clients (Claude Code, Codex, omp, Pi, Claude Desktop, VS Code with
GitHub Copilot, Cursor and more) from one versioned, offline store. It writes
each client's native config files, never touches entries it did not create,
and keeps secret values out of the store.

Status: early development. The design is in [SPEC.md](SPEC.md); milestones M0
(workspace bootstrap), M1 (store schemas and validation), M2 (git-backed
store, `loadout server add|ls|show|rm`, `loadout history`), M3
(format-preserving JSON, JSONC and TOML editors), M4 (client adapter
framework with Claude Code and Claude Desktop adapters), M5 (Codex, omp
and Pi adapters), M6 (system-scope apply engine with an ownership ledger,
`loadout status|diff|apply|rollback|import|adopt`, one store commit per apply
run), M7 (secret references, env/keyring/1Password providers, the leak
scanner on every store commit, `loadout secrets ls|check|providers|set|clear`),
M8 (secret delivery per client: `env`, `launch` through `loadout exec`,
`command` hooks and opt-in `inline`; `loadout env --export`,
`loadout secrets map-op-env|service-account`), M9 (project scope: project
manifests and trust, project-scope apply, `loadout project` and
`loadout effective`), M10 (Tier B adapters: VS Code with GitHub Copilot,
Cursor, and the legacy VS Code `mcp.json` read-only), M11 (store history
views, tags and restore: `loadout show|tag|restore|backup`,
`loadout diff --rev`), M12 (folder remotes with per-entity pull merge:
`loadout remote add|ls|rm|push|pull|machines`, `backup` pushes,
`restore --from`), M13 (git remotes through the system git with `system`,
`gh` or `token` auth; `loadout remote token|create-github`), M14 (desktop
app shell: Tauri 2 with typed IPC, the SkillDock shell port, empty pages),
M15 (desktop MCP page with per-client pills, Form/TOML/Preview editor and
paste-anything, Tools page, pending-changes drawer with "Undo this apply"),
M18 (skills store and CLI: `loadout skills ls|add|rm|enable|disable|update|scan`,
installs from git, folders and `.zip`/`.skill` archives, junction links on
Windows, project links hidden through `.git/info/exclude`), M19 (desktop
Skills page with list, grid and grouped views, per-tool "Enable in Tools"
pills, tool folders as managed/unmanaged/conflict, file browser and update
diff, a project scope; Install page: Skill from Git or a local folder or
archive, MCP by form or pasted config), M20 core and CLI
(`loadout migrate skilldock [--apply]`: SkillDock's `~/.skilldock`, read only,
into the store in one commit, with a report of every item; the migration
wizard page follows),
M21 (Tier C adapters: Gemini CLI, Antigravity, OpenCode, Grok Build),
M22 (Tier D: Windsurf, Devin Desktop, format-preserving YAML and JSON5
editors, and the ported user-scope clients), M23 (packaging: the NSIS
installer with `loadout` on the user PATH, AppImage/deb/rpm, AUR PKGBUILDs,
winget manifests, the release workflow with Azure Trusted Signing, and the
optional updater) and M24 (desktop MCP page at project scope: a project's
manifest servers with per-client pills, add/edit/remove in `project.toml`
or `project.local.toml`, read only until trusted; Discard pending in the
pending-changes drawer) are in place. Gates V1 (Tier A)
and V2 (Tier B),
the real-client checks, are partly run; see
[docs/verify/README.md](docs/verify/README.md).
Using Loadout: [docs/user-guide.md](docs/user-guide.md); supported clients:
[docs/adapters.md](docs/adapters.md). JSON Schemas for store files, for
editor completion, are in [docs/schema](docs/schema).

## Install

Downloads are on the [latest release](https://github.com/DailenG/agent-loadout/releases/latest);
`SHA256SUMS` lists every asset's hash.

- **Windows:** `Loadout_<version>_x64-setup.exe` (per-user, signed; puts
  `loadout` on your PATH and updates itself). winget
  (`winget install DailenG.AgentLoadout`) once the package is accepted.
  Chocolatey (`choco install agent-loadout`, per-user like the installer)
  once the package is approved.
- **Linux:** the AppImage (updates itself), or the `.deb` / `.rpm`.
- **Arch Linux:** the AUR packages `agent-loadout` (source) and
  `agent-loadout-bin` are not published yet; until then build them from this
  repository:

  ```sh
  git clone https://github.com/DailenG/agent-loadout
  cd agent-loadout/packaging/aur/agent-loadout-bin   # or agent-loadout
  makepkg -si
  ```

## Build

Requires a stable Rust toolchain (see `rust-toolchain.toml`).

```sh
cargo build --workspace
cargo test --workspace
cargo run -p agent-loadout -- --version
cargo run -p agent-loadout -- doctor --paths
```

The desktop app also needs Node.js and npm (dependencies install into
`node_modules`; the Tauri CLI is a dev dependency):

```sh
npm install
npm test                       # Vitest against the mock transport
npx tsc --noEmit
npm run dev:mock               # the frontend in a browser, mock backend
npx tauri dev                  # the app
npx tauri build --debug
npm run bindings               # regenerate src/ipc/bindings.ts after command changes
npm run test:e2e               # tauri-driver smoke test (see tests/e2e/smoke_mcp.spec.ts)
```

`src-tauri` is a Cargo workspace of its own, so the root workspace builds
and tests without WebView tooling; run its checks with
`--manifest-path src-tauri/Cargo.toml`.

Once published, install the CLI with `cargo install agent-loadout`; the binary
is `loadout`. (The crates.io crate named `loadout` is an unrelated project.)

## Data directory

| OS | Default `LOADOUT_HOME` |
|---|---|
| Windows | `%LOCALAPPDATA%\Loadout` |
| Linux | `${XDG_DATA_HOME:-~/.local/share}/loadout` |
| macOS | `~/Library/Application Support/Loadout` |

Override it with `--home <PATH>` or the `LOADOUT_HOME` environment variable.
Keep it out of file-sync folders (OneDrive, Dropbox, Google Drive, Syncthing,
Resilio Sync); `loadout doctor` reports `sync_root_detected` when it is inside one.

## Layout

| Path | Contents |
|---|---|
| `crates/agent-loadout-core` | Core library; no process-global state, takes an injected `Env` and `Fs` |
| `crates/agent-loadout` | CLI, binary `loadout` |
| `crates/agent-loadout-testkit` | Hermetic test helpers (`TempHome`, `RootedFs`), unpublished |
| `src-tauri` | Desktop app crate `agent-loadout-app` (Tauri 2), commands over the core |
| `src` | React frontend: shell, typed IPC (`src/ipc`), English catalog (`src/i18n/en.ts`) |

## License

MIT, see [LICENSE](LICENSE). Portions are derived from SkillDock; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

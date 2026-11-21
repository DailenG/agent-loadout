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
`loadout secrets map-op-env|service-account`), M11 (store history views,
tags and restore: `loadout show|tag|restore|backup`, `loadout diff --rev`)
and M12 (folder remotes with per-entity pull merge: `loadout remote
add|ls|rm|push|pull|machines`, `backup` pushes, `restore --from`) are in
place.
Nothing is released yet. JSON Schemas for store files, for editor completion,
are in [docs/schema](docs/schema).

## Build

Requires a stable Rust toolchain (see `rust-toolchain.toml`).

```sh
cargo build --workspace
cargo test --workspace
cargo run -p agent-loadout -- --version
cargo run -p agent-loadout -- doctor --paths
```

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

## License

MIT, see [LICENSE](LICENSE). Portions are derived from SkillDock; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

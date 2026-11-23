# Contributing to Loadout

Thanks for helping. This page covers setting up a development environment,
the checks every change must pass, the test rules, and how releases work.
How the code is organized is in [docs/architecture.md](docs/architecture.md);
the design contract is [SPEC.md](SPEC.md).

## Where things are decided

- **[SPEC.md](SPEC.md)** is the design contract. Code comments cite it by
  section (`SPEC 8.3`). A change in behavior updates SPEC.md in the same
  commit.
- **[docs/milestones/](docs/milestones)** are notes written when each
  milestone landed. They are history; do not update them for later
  changes.
- **User-facing docs** ([docs/](docs/README.md)) describe current
  behavior and are updated with the change.
- **Issues** on GitHub track bugs and planned work.

## Development setup

All platforms need:

- Rust stable with `rustfmt` and `clippy` (`rust-toolchain.toml` selects
  it; `rustup` installs it on first use);
- Node.js 24 and npm, for the desktop app and its tests;
- git.

### Windows

- Visual Studio Build Tools with the "Desktop development with C++"
  workload (MSVC and the Windows SDK).
- WebView2 (included in Windows 11).

### Linux

The desktop app needs WebKitGTK 4.1 and its build libraries. On Ubuntu
and Debian:

```sh
sudo apt-get install -y build-essential libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev
```

On Arch: `sudo pacman -S --needed base-devel webkit2gtk-4.1 rustup nodejs npm`.

The root Cargo workspace (core, CLI, testkit) needs none of these.

### macOS

Xcode command line tools (`xcode-select --install`). No macOS bundle is
published, but the workspace builds and tests there.

### First build

```sh
cargo build --workspace
cargo run -p agent-loadout -- --version
npm install
npx tauri dev            # the desktop app
npm run dev:mock         # the frontend in a browser against the mock backend
```

When running the CLI by hand during development, pass a throwaway data
directory so your real store is not touched:
`cargo run -p agent-loadout -- --home /tmp/loadout-dev status`.
Note that `apply` writes real client files under your profile; test against
a temp `HOME` (as the test suite does) rather than your own configs.

## Gates

Every change must pass these before it is merged (CI runs them on Windows,
Ubuntu, macOS and an Arch container):

```sh
# Root workspace: core, CLI, testkit
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked

# Desktop app crate (its own workspace)
cargo fmt --manifest-path src-tauri/Cargo.toml --all --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --locked

# Frontend
npm test
npx tsc --noEmit
```

CI also runs `cargo deny` (licenses: no GPL, AGPL or SSPL dependencies),
`scripts/check-no-baked-ids.sh`, `npx tauri build --debug`, and the AUR
package validation.

### Generated files

Regenerate these in the same commit as the change that affects them; never
edit them by hand:

| After changing | Run |
|---|---|
| A Tauri command or IPC type | `npm run bindings` (updates `src/ipc/bindings.ts`) |
| The adapter table | `LOADOUT_UPDATE_DOCS=1 cargo test -p agent-loadout-core --test docs_adapters` (updates the table in `docs/adapters.md`) |
| A store schema type | `LOADOUT_BLESS=1 cargo test -p agent-loadout-core --test model` (updates `docs/schema/`) |
| A CLI command, option or help text | `node scripts/gen-cli-docs.mjs` (updates `docs/cli.md`; CI checks it) |

In PowerShell, set the variable first: `$env:LOADOUT_UPDATE_DOCS = "1"`.

## Test rules

Tests must never touch real client configs, real secrets, the real OS
keyring, a real `op` or `gh`, or the network.

- **Hermetic environment.** Use `agent_loadout_testkit::TempHome`: it
  builds a temp root with `home/`, `appdata/`, `localappdata/` and `xdg/`
  and gives you an `Env` and a `RootedFs`. `RootedFs` panics on any path
  outside the root.
- **Core never reads the process.** Pass the `Env` and `Fs`; do not call
  `std::env`, home-folder lookups or the system clock in core.
- **CLI tests** clear the environment, pass `--home`, and set only the
  home, AppData, XDG and client override variables inside the temp root.
- **Stubs.** `op-stub` stands in for the 1Password CLI, `gh-stub` for the
  GitHub CLI, `MemKeyring` for the OS keyring; `git_env` gives system git
  an isolated configuration.
- **Fixtures** for client files live in `fixtures/` (comments, CRLF, BOM,
  foreign entries, legacy shapes). Golden tests use `insta`; property
  tests use `proptest`.
- **Frontend tests** use Vitest and Testing Library against the explicit
  mock transport (`src/test/mockTransport.ts`).
- **E2E** (`npm run test:e2e`, `tests/e2e`) drives a built app with
  `tauri-driver` against a temp profile.
- **Real-client checks** are manual and done by the maintainer
  (`scripts/verify-clients.md`, results in `docs/verify/`).

## Code conventions

- Core owns the logic; the CLI (`crates/agent-loadout/src/cmd`) and the
  Tauri commands (`src-tauri/src/commands`) stay thin.
- Errors are `CoreError` with a stable `ErrorCode`; user-facing text lives
  only in `src/i18n/en.ts` (English, American spelling).
- Every store change goes through one transaction and makes one commit.
- No secret value in the store, in logs, or across IPC.
- Files derived from SkillDock keep their attribution header; see
  [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Commits

Commit subjects use a short type and optional scope, then an imperative
summary, as in the history:

```
feat(gui): MCP page at project scope and Discard pending (M24)
fix(windows): installer PATH broadcast skips hung windows
packaging(aur): real 0.1.3 checksums
ci(release): check the extracted deb tree for both binaries
docs: ...
release: 0.1.3
```

The body is a short list of what changed and why, plus the gates that
passed. Keep one logical change per commit.

## Releases

Maintainers only.

1. Update the version in every place the release workflow checks: the
   workspace `Cargo.toml`, `src-tauri/Cargo.toml`, `package.json` and
   `packaging/aur/agent-loadout/PKGBUILD` (plus the `-bin` PKGBUILD and the
   `.SRCINFO` files). Move the `Unreleased` section of
   [CHANGELOG.md](CHANGELOG.md) under the new version. Commit as
   `release: X.Y.Z`.
2. Tag `vX.Y.Z` and push the tag. `.github/workflows/release.yml` checks
   that the tag matches every version, builds the Windows NSIS installer
   (signed with Azure Trusted Signing through `scripts/sign-windows.ps1`
   when the `release` environment is configured) and the Linux AppImage,
   `.deb`, `.rpm` and tarball, smoke-tests them, and creates a **draft**
   GitHub release with `SHA256SUMS`, the rendered winget manifests and,
   when the updater key is set, `latest.json` and `.sig` files. The draft's
   notes are the version's CHANGELOG section followed by the generated
   list of pull requests.
3. Review the draft and publish it. Release tags are protected: a failed
   run means a new version, not a re-tag.
4. After publishing:
   - **Chocolatey**: `.github/workflows/chocolatey.yml` runs on the
     published release (and daily), updates
     `packaging/chocolatey/agent-loadout` from the release's `SHA256SUMS`,
     pushes the package and commits the updated files.
   - **winget**: submit the manifests from `winget-manifests-X.Y.Z.zip`
     (rendered by `packaging/winget/render.ps1`) to `microsoft/winget-pkgs`.
     While an earlier version's pull request is still open, replace its
     manifests with the new version's on the same branch (and update its
     title) rather than opening a second pull request.
   - **AUR**: fill the real checksums (`updpkgsums`), regenerate
     `.SRCINFO`, run `packaging/aur/verify-release.sh` on Arch, commit as
     `packaging(aur): real X.Y.Z checksums`. (Publishing the packages to
     the AUR itself waits for AUR account registration to reopen.)

## License

By contributing you agree that your contributions are licensed under the
[MIT License](LICENSE).

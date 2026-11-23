# Changelog

All notable changes to Loadout are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). Releases are on
[GitHub](https://github.com/DailenG/agent-loadout/releases); work in progress
is tracked in [issues](https://github.com/DailenG/agent-loadout/issues).

## [Unreleased] (0.2.0)

### Added

- Sidebar **Scope** switcher: **User** or one of your projects, plus
  **Add folder...**. The Skills, MCP and Tools pages follow the selected
  scope; the choice resets to User on restart (#7).
- MCP page at project scope: lists a project's manifest servers with
  per-client pills; add, edit and remove write `project.toml` or
  `project.local.toml`, trust exactly that content and apply. Inherited
  user-scope servers offer **Override here** and **Hide here**. Untrusted or
  changed projects are read only until **Review & trust**.
- Tools > **Manage** at project scope edits that project's manifest.
- **Discard pending** in the Pending changes drawer (review mode): throws
  away store edits this machine has not applied yet, per item or all, as
  one commit tagged `auto/pre-restore/...`; client files are not touched.
- omp is listed first wherever Loadout lists clients and marked
  **Recommended**; the Tools page offers **Get omp** when it is not
  installed (#2). This is presentation only.
- Windows installer branding: sidebar and header images, an "Also add a
  desktop shortcut" option.
- Chocolatey package `agent-loadout` (0.1.3 and later), kept current by a
  scheduled workflow; awaiting approval on the Community Repository.

### Changed

- "System" is now called **User** everywhere in the app (scope names,
  History filter, effective matrix layer `U`).
- The per-page scope selects on Skills and MCP are replaced by the sidebar
  switcher.
- Header toolbars are one aligned row with one control height (#1).
- Faster GUI on large stores (#10): skill, MCP and tool cards re-render only
  when their own data changes (a window focus no longer re-renders all
  cards); search filters in the background while you type; CodeMirror and
  every page but Skills load on first use (the startup script is less than
  half its former size); the apply drawer reads the plan and the server list
  from the cache; tooltips are opaque, without a backdrop blur.
- History shows the newest 100 commits, with **Load more** for the next 100.
- The WebView2 runtime no longer downloads browser components (Widevine,
  speech recognition and others) into the app's profile after install
  (`--disable-component-update`).
- Release builds use thin LTO, one codegen unit and stripped symbols:
  smaller `agent-loadout-app.exe` and `loadout.exe`.

### Fixed

- `loadout project trust <path>` exits 0 when that project applied cleanly,
  even if another known project still waits for trust; the app's trust
  notice reports the same.

## [0.1.3] - 2026-11-22

### Fixed

- Windows installer: broadcasting the `PATH` change no longer waits 5
  seconds for each unresponsive window (it now uses
  `SendMessageTimeout` with `SMTO_ABORTIFHUNG` and 1 second), so installs
  on machines with hung windows finish in seconds instead of minutes.
- Windows: `loadout.exe` links the Visual C++ runtime statically. In 0.1.2
  the CLI failed to start (`STATUS_DLL_NOT_FOUND`) on machines without the
  Visual C++ Redistributable.

## [0.1.2] - 2026-11-22

First published release.

### Added

- Desktop app (Tauri 2) for Windows and Linux with the Skills, MCP,
  Projects, Tools, Install, History and Settings pages, the Pending changes
  drawer and the drift resolver.
- `loadout` CLI: servers, apply, status, diff, rollback, import, adopt,
  projects and trust, effective config, secrets, history, tags, restore,
  backup, remotes, skills and SkillDock migration.
- 38 client adapters (see [docs/adapters.md](docs/adapters.md)).
- Windows NSIS installer (per user, signed, adds `loadout` to the user
  `PATH`), AppImage, `.deb`, `.rpm`, a Linux tarball for the AUR
  `agent-loadout-bin` package, winget manifests, `SHA256SUMS`, and the
  in-app updater feed.

## 0.1.1 and 0.1.0

Tagged but never published: both release runs failed before publishing
anything (0.1.1 at the Linux `.deb` check). The tags are protected, so the
first published release is 0.1.2.

[Unreleased]: https://github.com/DailenG/agent-loadout/compare/v0.1.3...HEAD
[0.1.3]: https://github.com/DailenG/agent-loadout/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/DailenG/agent-loadout/releases/tag/v0.1.2

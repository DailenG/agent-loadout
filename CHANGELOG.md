# Changelog

All notable changes to Loadout are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). Releases are on
[GitHub](https://github.com/DailenG/agent-loadout/releases); work in progress
is tracked in [issues](https://github.com/DailenG/agent-loadout/issues).

## [Unreleased]

## [0.4.0] - 2026-11-23

### Added

- Read-only contexts: client files and folders your account can read but
  not write (a project managed by a separate security-focused installer, a
  file marked read only, a read-only share) are listed with the reason
  (`acl`, `attribute`, `owner`, `share`) in `loadout status`, `effective`,
  `apply --dry-run`, `import --scan` and their `--json` output
  (`read_only`), and in the app's data for the Projects list, Tools cards,
  MCP pills, Skills, the effective matrix, Import and the Pending changes
  drawer. Loadout never plans a write into them: those entries are blocked
  (`read_only_target`), the rest applies, and the exit code ignores them.
  A write the OS refuses as read only (the file changed after the check)
  is reported the same way instead of failing, and the other files of the
  run still apply; skill links and project manifest edits report it too.
  Import never overwrites an entry in a read-only file. The check never
  writes, gives up after two seconds on a stalled share, and is cached
  with the app's plan (#4).

## [0.3.1] - 2026-11-23

The first published release of the 0.3 line: it contains everything listed
under 0.3.0 below, plus these two changes from the 0.3.0 release check.

### Added

- An import that moves literal secrets out of client files now says that
  the backups its apply took still hold the old values, lists those
  backups, and removes exactly them on request (overwritten with zeros in
  place, then deleted; only that run's own backups, never a link):
  **Remove those backups now** in the app's import result (after a
  confirmation), `loadout import --remove-old-backups` in the same run, or
  `loadout import --remove-old-backups=<APPLY-ID>` later. Undoing that
  import (`loadout rollback`, **Undo this apply**) and
  `loadout restore --files` then report which files' previous content is
  no longer kept, and restore the rest. In review mode, or when an entry
  is left to decide, the import says the file still holds the old values
  until an apply rewrites it (#3).

### Changed

- The app's not-managed notice is one line in the sidebar ("7 not
  managed"); a click or Enter opens the message with Review and Dismiss
  above it, and the collapsed sidebar shows only the count. The sidebar is
  denser in short windows, so every nav item and the notice fit at the
  default 1088x680 and at the 960x560 minimum (#9).

## 0.3.0 (tagged, not published)

### Added

- `loadout import --scan` reads the files of every project this machine
  knows too, trusted or not (read only), and shows each definition of an
  entry with its sources; an entry that differs between clients or scopes
  is a conflict. `--pick NAME=CLIENT` takes one definition and
  `--as NAME=CLIENT` imports another as `NAME-CLIENT`; `--skip`, `--scope`
  and `--project` narrow the import (#3).
- Import extracts literal secrets instead of refusing the entry:
  `--extract NAME[:FIELD]=keyring|op://...|op-env://...|env:VAR` stores a
  `${secret:ALIAS}` reference and sends the value to the OS keyring (taken
  from the client file, never shown) or maps it to 1Password or an
  environment variable, in the same commit (SPEC 11.7, #3).
- App backend for the import view and the unmanaged-entry notice:
  `scan_import`, `apply_import`, `unmanaged_summary`, `new_unmanaged` and
  `acknowledge_unmanaged`; acknowledged entries are kept per machine in
  `state/unmanaged-seen.json` (#3, #9).
- `loadout skills adopt` brings unmanaged skill folders into the store and
  hands them to Loadout: identical folders in several clients become one
  skill enabled for all of them, differing ones are a conflict
  (`--pick NAME=CLIENT`, or `--both NAME=CLIENT` to import the others as
  `NAME-CLIENT`), and each folder whose content equals the store copy is
  replaced by the managed link (a junction on Windows) and moved to
  `backups/trash/`. `--all` adopts every name with one content, `--tool`
  one client's folder, `--project` a trusted project's folders. One commit
  per action (SPEC 12.5, #3).
- `loadout project discover` lists folders the clients already know
  (Claude Code's `projects`, Codex trusted projects, omp and Pi sessions)
  that are not projects on this machine yet; `--add ID` or `--all` adds
  them in one commit, untrusted, writing a private manifest where a
  folder has none (SPEC 13.3, #3).
- App backend for skill adopt and Discover projects:
  `scan_unmanaged_skills`, `adopt_skills`, `discover_projects` and
  `add_discovered_projects` (#3).
- App: the Import page's **Skills** section lists the unmanaged skill
  folders of the active scope (every client or one): a name with the same
  content everywhere is one row, a name with several contents asks to use
  one content, keep each (the others as `name-<client>`, with a hint that
  their `SKILL.md` keeps the original name) or skip. **Adopt** shows what
  was adopted, what was kept and why (differs, holds a repository, tracked
  by the project, an empty folder or executable file the store would
  lose), and where the originals went in the trash. An untrusted project
  offers **Review & trust** first (#3).
- App: a client's skill folder on the Skills page gains **Adopt** on each
  real folder and **Adopt all identical** for every name with one content
  (#3).
- App: **Discover projects** on the Import page and in the Projects page
  toolbar and empty state: the folders your clients know, with the clients
  that know each and its manifest, as a checklist; **Add selected** adds
  them untrusted and offers **Review & trust** for each added project
  (#3).
- App: an **Import** page (after Install) gathers everything that brings an
  existing setup in: MCP servers, skills, projects and the SkillDock
  migration, whose entry point moved here from Settings (#3).
- App: the Import page's **MCP servers** section scans the user files and
  every known project's files (filter by User files or Projects). Identical
  entries are one row with their files; a conflict shows its definitions
  side by side to use one, import both under their own IDs (editable,
  checked against the store) or skip. Each literal secret goes to the OS
  keyring (default, the value never shown), a 1Password reference, an
  environment variable, or the server is skipped. **Review import** sums
  up what is imported, for which clients, and where each secret goes;
  one apply follows, and the result lists imported, skipped (with the
  reason) and moved secrets, with **Open Pending changes** in review mode.
  An entry found only in project files is marked **Definition only** and
  offers **Add to project manifest**, enabled for the clients whose files
  hold it: an entry in a shared file goes to the shared manifest by
  default; for Claude Code and Devin, which also have a private target,
  **Private** adds a private entry instead and leaves the shared one. An
  untrusted project asks for **Review & trust** first (#3).
- App: **Not managed by Loadout** lists the foreign MCP entries of the
  active scope per client and file on the MCP page and in Tools > Manage,
  with "N managed, M not managed by Loadout" and **Import...** on each
  entry, which opens the Import page with that entry picked; each Tools
  card shows its client's count (#3).
- App: a sidebar notice after the watcher, window focus or Refresh, such
  as "omp has 5 MCP servers not managed by Loadout.", with skill folders
  counted too. **Review** opens the Import page; Review and Dismiss
  acknowledge what was shown, so the same entries never show again.
  Focus and watcher events scan client files and skill folders at most
  once a minute; Settings > Preferences > **Not-managed notice** turns it
  off (#9).

### Changed

- An entry found only in project files imports as a store definition
  without enablement, to be named in the project's manifest (`from`);
  `loadout import` applies only the imported servers' files (#3).
- The secret extraction of the SkillDock migration moved to a shared core
  module (`secrets::extract`) that the import uses as well; the
  migration's behavior is unchanged.

## [0.2.1] - 2026-11-23

### Changed

- Planning is much faster on machines with many servers and client files:
  each client file is parsed once per plan instead of once per entry, files
  shared by several targets (`~/.claude.json`) are read once, ledger lookups
  are indexed, and `loadout effective` plans in one run (#14).
- The app plans once per change: the drawer, the MCP pills, the Projects
  page and the effective matrix share one cached plan, served again until a
  file it read changes (#15).
- A store change applies only what it touched: a server change plans the
  files that hold or own that server's entries, and `project trust`,
  `init`, project server edits and `project share` plan that project only.
  A project still waiting for trust elsewhere no longer slows the change or
  turns its exit code into 7 (#15).

### Fixed

- Dialogs and the Pending changes drawer manage keyboard focus: opening one
  moves focus inside, Tab and Shift+Tab stay inside, the page behind is
  inert, Escape closes only the top dialog, and focus returns to the button
  that opened it (#11).

### Security

- `cookie` 0.18.3 in the desktop app's dependency tree (RUSTSEC-2026-0594;
  Loadout never forks or uses private cookies, so it was not affected).

## [0.2.0] - 2026-11-23

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

[Unreleased]: https://github.com/DailenG/agent-loadout/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/DailenG/agent-loadout/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/DailenG/agent-loadout/compare/v0.2.1...v0.3.1
[0.2.1]: https://github.com/DailenG/agent-loadout/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/DailenG/agent-loadout/compare/v0.1.3...v0.2.0
[0.1.3]: https://github.com/DailenG/agent-loadout/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/DailenG/agent-loadout/releases/tag/v0.1.2

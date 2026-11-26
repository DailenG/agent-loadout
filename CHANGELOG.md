# Changelog

All notable changes to Loadout are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/). Releases are on
[GitHub](https://github.com/DailenG/agent-loadout/releases); work in progress
is tracked in [issues](https://github.com/DailenG/agent-loadout/issues).

## [Unreleased]

## [0.7.1] - 2026-11-25

### Changed

- Snyk Agent Scan (#27) is disabled and hidden. Snyk's free tier refuses
  new accounts by every path we tried, so the optional scanner could not
  be used, and Loadout no longer integrates with Snyk. The scanner never
  runs (even if 0.7.0 turned it on), is never downloaded or executed, and
  Settings > Security, the External scan section of reviews and **Scan
  with Snyk** are gone; `loadout scanner` and `--external` /
  `--no-external` are hidden (`--external` is ignored with a note), and
  every `scanner` subcommand except `status` and `remove` fails with
  `scanner_unavailable`. If you installed it in 0.7.0, run `loadout
  scanner remove` to delete the download and its settings. The built-in
  offline scan is unchanged.
- Windows installer: running a newer installer over an installed version
  now preselects **Upgrade in place (recommended)**, what the in-app
  updater does, instead of uninstalling first, and the page says your
  Loadout data, settings and history are kept either way. **Replace the
  installed version (removes it first)** is still offered. The
  uninstaller's checkbox now reads "Delete the app's window cache (your
  Loadout data folder is kept)", which is all it removes.

### Fixed

- The omp logo in the app is omp's own mark (the gradient π from
  omp.sh), not a placeholder.
- Skills page in a project: paths inside the project folder read
  ".claude/skills in the project folder" instead of the raw `<P>/`
  placeholder (also in read-only tooltips, adopt results, Pending changes
  and History); long descriptions end before the card's buttons instead
  of running under them; the skill icon's corner mark no longer crowds
  the letter.

## [0.7.0] - 2026-11-24

### Added

- Snyk Agent Scan from the command line (#27): `loadout scanner
  ls|status|install|remove|enable|disable|token|run` and `--external` /
  `--no-external` on `skills add` and `skills update`. `install` downloads
  the pinned v0.7.1 build from GitHub and checks it against SHA-256 values
  compiled into Loadout (a tampered download is deleted); `token` takes a
  secret alias or `env:`/`keyring://`/`op://` reference, never the token,
  which reaches the scanner only in its environment. Each run is bounded
  (60 s, the whole process tree killed on timeout or cancel), never passes
  `--dangerously-run-mcp-servers`, and is reported as `Snyk Agent Scan:
  <verdict>` (or `not scanned (<reason>)`) beside the built-in scan; a
  dangerous verdict needs `--allow-dangerous` (exit 8). New error codes
  `scanner_unavailable`, `scanner_download_failed`,
  `scanner_verification_failed`, `canceled` and `external_scan_required`
  (the app refuses an install from a source, or an update, while the
  scanner is on and its scan has not run). Hardening from the review: the
  installed scanner is hashed before each command runs it (a changed file
  counts as not installed and is downloaded again); a pasted token is
  refused and never echoed (an alias must be a defined secret); the
  scanner unpacks into its run folder, not your temp folder; leftover run
  folders and partial downloads are cleaned up; Ctrl+C stops a running
  scan; turning the scanner off (or removing it, which now also turns it
  off) cancels scans in progress; an untrusted or blocked git source is
  never sent to Snyk; a crash with no output is "failed" (or "token
  rejected"), not "no token".

- Settings > Security > **Snyk Agent Scan (optional)** (#27): an optional
  second scanner for skills, off by default. The section says in plain
  words what it does and when (as soon as an install or update review
  opens), that skills are sent to Snyk's cloud (`api.snyk.io`, with links
  to Snyk's terms and privacy policy) and exactly what is sent (the
  skill's files with detected secrets redacted, its name, the scanned
  folder's path, your user name, the machine's host name where the
  scanner's platform reports one, the scanner version), that it needs a
  Snyk token and that the free tier has a daily limit. **Install**
  downloads the pinned version with a progress bar and **Cancel** (kept
  when you leave Settings; no second install meanwhile) and shows the
  verified version; **Remove** asks first (Cancel is the default) and
  turns it off. The token is a secret reference (a Secrets page alias, an
  environment variable, 1Password or the OS keyring), never a value;
  aliases to a 1Password Environment aren't offered, and the dialog says
  why Save is off. **Use Snyk Agent Scan** stays off until it's installed
  and a token is set.
- When it's on, the skill install and update reviews show an **External
  scan** section beside the built-in findings: Snyk's verdict and findings
  (severity from Snyk's score, risk, `file:line`, malicious or
  unverifiable URLs), a running state with **Cancel scan**, and "Not
  scanned (<reason>)" or "Scanner output not understood" when it couldn't
  scan, with **Scan again**. Content from a source is scanned as the
  review opens; a folder or archive from this computer only through **Scan
  with Snyk**. Install / Update waits while the scanner's status loads or
  the scan runs. A dangerous external verdict needs **Install anyway** /
  **Update anyway** like the built-in scan, and the box names which
  scanner(s) found it; not-scanned results never block. The run and its
  result are announced to screen readers. A library skill's details offer
  **Scan with Snyk** (a report only).

### Fixed

- Without Git installed, Install > Browse no longer shows each Git catalog
  as a red error with the same words twice (#29). It says, in neutral
  styling, "Git isn't installed, so Git catalogs can't be searched.
  Install Git to use them.", with **How to fix this** (opens the new
  troubleshooting entry) and **Try again**. The missing-Git message is now
  one sentence everywhere: skills from Git, MCP servers from Git and Git
  remotes say "Git isn't installed or isn't on PATH; install Git to use
  ..." instead of the generic "A required program is not installed or not
  on PATH." text. A Git that exists but fails to start, and other missing
  programs, keep the generic sentence (never the diagnostic detail).

## [0.6.0] - 2026-11-24

### Added

- The Loadout skill for agents (#8): a read-only skill, bundled with
  Loadout, that teaches coding agents the read and plan commands
  (`status`, `effective`, `diff`, `import --scan`, `apply --dry-run`,
  `skills ls|scan`, `server ls|show`, `secrets ls|check` and more, with
  `--json`), the error and exit codes, and playbooks such as "why isn't
  server X loading in client Y". For any change it hands you the exact
  command or app page; it never passes `--yes`, `--trust-source` or
  `--allow-dangerous`, and treats third-party text as data. Install it
  per client in Settings > Agent integration or with `loadout agent
  install [--client C]`; `loadout agent remove`, `agent status` and
  `agent skill-path` complete the set. It is a normal store skill (id
  `loadout`, origin "Bundled with Loadout"), so every change is a commit.
  When Loadout is updated, the next app start, CLI command that changes
  the store, or `loadout agent install` updates the store copy and says
  so (read commands never write and take no lock; `agent status` shows
  `outdated` with the command to run). A copy you edited is never
  overwritten on its own and is reported once as "customized, not
  updated"; junk files such as `.DS_Store` and line-ending resaves do not
  count as edits. `loadout agent install --reset` (Settings: "Restore
  Loadout's version") puts the bundled copy back and keeps its clients.
  Store schema: skill metadata gains a `[bundled]` table (no new
  `[source] kind`, so 0.5.x machines syncing the store keep working).
- Install > Browse (#26): search the online catalogs from the app. The
  first visit asks once, naming the hosts it would contact, and nothing is
  requested before you answer. Each catalog is searched on its own with
  its status shown (searching, up to date, saved with its age, stale,
  timed out, offline, query too short), so a slow one never holds up the
  rest. Results show source, publisher ("namespace ownership checked by
  the registry" for the MCP Registry, with its "Data from the Official
  MCP Registry" attribution), the Official mark only for
  `github.com/anthropics` taps, transports, the names of required
  secrets, version and pinning ("Pinned to ...", "Follows branch ...",
  "Not pinned"), deprecation, last update, repository and license; no
  catalog icon or image is ever loaded. Details hands an entry to the
  existing reviews: an MCP server to the From Git or folder review
  (drafts, trust, scan, client picker), a skill to the skill install
  review (a plugin with several skills asks which one); plugins from
  npm, archive or command sources say why they can't be installed.
  skills.sh is a link-out card that also takes a pasted
  `https://skills.sh/<owner>/<repo>/<skill>` address.
- Settings > Catalogs: the Online catalogs switch, and the catalog list
  (turn each on or off, add an MCP registry address or a Git tap, remove
  the ones you added).
- Catalogs from the command line (#26): `loadout catalog
  ls|add|rm|enable|disable|online|search|show|refresh`. Built in: the
  Official MCP Registry (`/v0.1`), the `anthropics/skills` Git tap and a
  skills.sh link; add any MCP Registry API server or Git tap (skill
  folders and Claude Code plugin marketplaces). Catalogs live in
  `store/catalogs.toml` (synced; new JSON Schema `catalogs.schema.json`).
  Nothing is contacted until you run `loadout catalog online on` (a
  per-machine setting, off by default), and only when you search, show,
  refresh or install. Each catalog is searched on its own with timeouts;
  results are saved for 24 hours and shown labeled with their age when a
  catalog can't be reached. Install an entry with `loadout server add
  --from catalog:<source>/<id>` (a registry entry's `server.json` or a
  plugin's `.mcp.json` and `mcpServers`, through the same review, trust
  prompt and scan as servers from Git; a registry server's trust prompt
  names what it runs, its package or its remote's host, never the
  repository the publisher claims; `--ref` pins a registry version) or `loadout skills add
  catalog:<source>/<id>`; `skills add` also takes a
  `https://skills.sh/<owner>/<repo>/<skill>` address. Servers and skills
  record `catalog = "<source>/<id>"` in `[meta.source]`/`[source]`; a
  registry server is the new `kind = "registry"` and `loadout server
  update` asks the registry for a newer version (only while that catalog
  is turned on at the same address). Deleted registry entries
  are refused, deprecated ones marked; plugins from npm, archive or command
  sources are refused, and plugin hooks, commands and agents are never
  imported. New error codes `catalogs_off` and `catalog_unreachable`. App
  commands for the Browse page: `catalogs_overview`,
  `set_online_catalogs`, `add_catalog`, `remove_catalog`,
  `set_catalog_enabled`, `search_catalog`, `show_catalog_entry`,
  `refresh_catalog`, `review_catalog_mcp`, `review_catalog_skill`,
  `open_catalog_link` (https links only). A tap address with a user name,
  password or token is refused: use your git credential helper.
- `loadout skills add --ref` takes a full 40-character commit, which pins
  the skill (updates fetch that commit); a short commit is refused. MCP
  drafts from a committed `.mcp.json` read a Claude
  Code plugin's `${CLAUDE_PLUGIN_ROOT}` as the folder, and skip entries
  with neither a URL nor a command instead of drafting them.

## [0.5.0] - 2026-11-24

### Added

- Built-in offline scanner: every skill install and update is scanned
  locally with Loadout's own rules, shipped as data (`scan/rules.toml`,
  versioned; a custom `state/scan-rules.toml` can add rules or raise
  severities, never remove or lower one, and results then say "custom
  rules in use"): download-and-run idioms (`curl | sh` also across line
  continuations, `env` and `tee`; download then run; `iwr | iex`),
  encoded payloads, reverse shells, credential reads and uploads next to
  network calls, hidden Unicode (tag characters, variation selectors,
  zero-width and bidi characters) and HTML-comment instructions, prompt
  injection, turned off safety prompts, agent memory and config edits,
  native binaries (by their first bytes), and for MCP server commands
  unpinned packages, Git URLs and images. In Markdown prose (outside
  fenced code blocks) mentions of installers, safety prompts or "ignore
  previous instructions" are warnings; in code blocks and scripts they
  are dangerous. Files are typed by their bytes, UTF-16 is decoded, files
  read by name only are listed (`not-content-scanned`) and only the first
  4 MiB of a file are scanned (`not-fully-scanned`). Findings carry
  severity, rule, file, line and an excerpt with secrets masked; the
  verdict is the worst finding. A `dangerous` verdict blocks the install
  or update (`scan_blocked`, exit code 8) unless you pass
  `--allow-dangerous`; `loadout skills add` and `skills update` (also
  `--check`) print the findings, and `--json` adds `scan`. Results are
  cached by content hash in `cache/scan`. App: install and update results
  carry the scan, and a new install review (`review_git_skill`,
  `review_local_skill`, `install_reviewed_skill`, `discard_skill_review`)
  shows findings before anything is installed; `update_skill` takes
  `allowDangerous` (#25).
- Source trust: installing or updating a skill from a git source you
  have not trusted asks first: **Trust this time only**, **Always trust
  this source** (the repository, its owner, or a self-hosted host), or
  cancel. Decisions live in `store/sources.toml` and sync with your
  remotes; a block wins over any trust. Without a terminal (scripts,
  agents, `--json`) an untrusted source fails with `source_trust_required`
  unless you pass `--trust-source[=once|repo|owner|host]`; `--yes` never
  answers it. A force-pushed, renamed or transferred repository makes its
  trust lapse, and the update check marks new or changed executable files.
  New: `loadout source ls|trust|block|untrust`; app commands
  `list_sources`, `check_source_trust`, `revoke_source`, `block_source`,
  `block_source_url`, and a `trust` answer on `install_git_skill`,
  `install_reviewed_skill` and `update_skill`. Git skills installed before
  this release ask once on their next update. Sources over plain `http://`
  or `git://` get their own keys and are never admitted by HTTPS or SSH
  trust; a redirect (a renamed or transferred repository) only adds a
  question for the new location, and a blocked source is never contacted
  (#24).
- MCP servers from a Git repository or a local folder:
  `loadout server add --from <GIT-URL|PATH> [--path] [--ref] [--pick]
  [--all] [--dry-run]` proposes servers from `server.json` (MCP Registry
  packages and remotes), an MCPB/DXT `manifest.json`, committed client
  configs (`.mcp.json`, `.vscode/mcp.json`, `.cursor/mcp.json`, ...) or,
  without any of those, the repository itself (`package.json` bin,
  `pyproject.toml` scripts, `Dockerfile`), with commands pinned to the
  package version or commit (a version-less registry package is marked
  not pinned; `registryBaseUrl` is honored; a repository's Docker image is
  `loadout-local/<id>:...`, run with `--pull=never`). Nothing runs while
  proposing; dependency and build folders are not read or scanned;
  identical definitions are proposed once; secrets become references and
  secret values in committed files are dropped. Saving goes through source
  trust (local folders too) and the built-in scan of every launch line, OS
  overrides included. A server that runs from the repository's files
  keeps a machine-local clone in `LOADOUT_HOME/servers/<id>`; while that
  copy is missing or at another commit the server's entries are blocked
  (`checkout_out_of_date`) until `loadout server checkout <ID>` restores it
  behind the same gates. A folder's server runs only on the machine that
  added it (`folder_on_other_machine`). The server file records only where
  it came from (`[meta.source]`, origin `source:git` or `source:local`).
  `loadout server update [ID] [--check]` reads the source again and shows
  the fields that change, keeping the id and secret names; a checkout is
  replaced only once the update commits. App commands
  `review_mcp_git_source`, `review_mcp_local_source`,
  `save_mcp_source_drafts`, `discard_mcp_source_review`,
  `check_mcp_source_update`, `update_mcp_source_server`,
  `review_mcp_checkout`, `restore_mcp_checkout` (#5).
- App: the trust dialog (**Trust this time only**, **Always trust** the
  repository or its owner, or a self-hosted host, **Cancel**; official
  sources marked) is shared by Install skill from Git, skill updates and
  MCP servers from Git or a folder. Install skill (Git and Local) now
  fetches and scans first and shows a review (source, trust state, files,
  findings) before installing; the skill update review marks executable
  files and shows the trust state and findings. Findings show severity,
  rule, `file:line`, message and excerpt everywhere; a dangerous verdict
  needs an explicit **Install anyway** / **Update anyway** / **Save
  anyway** (#24, #25).
- App: **Install > MCP > From Git or folder** (Git URL with optional
  folder and ref, or a picked folder): proposed servers with where they
  came from, the exact command, needed secrets and setup steps, the
  plain "This server runs code from this repository every time a client
  starts it" note for repository-as-server drafts, and their findings;
  edit each one in the server form, pick clients, save. Every launch line
  (OS overrides included) is shown, and an unpinned command is marked.
  Servers from a source get **Check for update** on the MCP page with a
  field-by-field review, and **Restore copy** for a missing or out-of-date
  checkout, reviewed and gated like an install (#5).
- App: **Settings > Sources** lists trusted and blocked sources with what
  each decision means, and offers Revoke, Unblock, Block and **Block a
  source...** by URL (#24).

### Changed

- Skill listings no longer read every skill's files: sizes come from file
  metadata and only `SKILL.md` is read; client detection runs once per
  listing or sync instead of once per skill, and link bookkeeping uses
  sets. `list_skills`, `loadout skills ls` and every install and enable
  get faster as the store grows (#16; numbers in
  `docs/perf/2026-11-23-baseline.md` section 11).
- Long lists in the app: Skills and MCP render only the cards near the
  visible area (each card keeps its place and its name in the list, so
  screen readers and find-in-page still reach it); History loads 200
  commits at a time, and "Load more" fetches only the next page (#17).
- The import review reads whether a client has a private project target
  from the Tools data (`ToolInfo.privateProjectTarget`, from the adapter
  table) instead of a list of its own (#22).

### Fixed

- Refresh now checks file and folder permissions again: a change of only
  the permissions shows without restarting the app. Switching back to the
  app after five minutes or more does the same (#23).

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

[Unreleased]: https://github.com/DailenG/agent-loadout/compare/v0.7.1...HEAD
[0.7.1]: https://github.com/DailenG/agent-loadout/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/DailenG/agent-loadout/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/DailenG/agent-loadout/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/DailenG/agent-loadout/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/DailenG/agent-loadout/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/DailenG/agent-loadout/compare/v0.2.1...v0.3.1
[0.2.1]: https://github.com/DailenG/agent-loadout/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/DailenG/agent-loadout/compare/v0.1.3...v0.2.0
[0.1.3]: https://github.com/DailenG/agent-loadout/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/DailenG/agent-loadout/releases/tag/v0.1.2

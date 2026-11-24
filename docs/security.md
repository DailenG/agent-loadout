# Security model

What Loadout guarantees about your secrets and your client files, and what
it does not. To report a vulnerability, see [SECURITY.md](../SECURITY.md).
The design is in [SPEC.md](../SPEC.md) sections 11 and 17.

## What Loadout touches

Loadout reads and writes:

- its own data directory (`LOADOUT_HOME`): the store, `state/`,
  `servers/` (local copies of MCP servers that run from their repository),
  `backups/`, `cache/`, `logs/`;
- the MCP config files and skill folders of the clients listed in
  [adapters.md](adapters.md), at user scope and in the project folders you
  add;
- your OS keyring (entries under the service `loadout`) and, when you use
  it, the 1Password CLI `op`.

It uses the network only for remotes you add, git sources of skills and
MCP servers, and the optional update check. Everything else, the built-in
scan included, works offline.

## Secrets

- **No secret values in the store.** Servers refer to secrets by alias
  (`${secret:ALIAS}`); `secrets.toml` holds provider references
  (`env:`, `keyring://`, `op://`, `op-env://`), never values. Because the
  store has no values, pushing it to a remote does not publish secrets.
- **Leak scanner.** Every store commit is scanned; a commit that contains
  something that looks like a secret value is refused before anything is
  written. `loadout doctor --scan-history` scans older commits too (it
  reports, and never rewrites history). `loadout import` replaces a
  literal that looks like a secret with a reference and sends the value
  to the OS keyring, or maps it to a 1Password item or an environment
  variable, as you choose; without a choice it skips the entry
  (`needs_secret_extraction`). The SkillDock migration asks where each
  plaintext value should go and stores only a reference.
- **No values across IPC.** The desktop app's backend never sends a secret
  value to the frontend. Previews mask values.
- **Delivery.** By default (`launch` delivery), clients start
  `loadout exec`, which fetches values into memory and starts the real
  server with them; nothing is written to disk. `env` delivery lets the
  client expand an environment variable itself. `command` delivery lets the
  client run a Loadout command at connect time. Only `inline` delivery
  writes a value into a client file, only when you opt in for a server,
  and never into a shared project file.
- **Missing secrets block, never blank.** A server whose secret does not
  resolve on this machine is reported as blocked for the clients that need
  it instead of being written with an empty value.
- **1Password service account tokens** (optional) are kept in the OS
  keyring and passed only to `op`, never to the server. The tradeoff is a
  long-lived token at rest in the keyring.
- **Ledger hashes are keyed.** The ownership ledger stores HMAC-SHA256
  hashes of what was written, with a per-machine key, so an inline value
  cannot be guessed offline from the ledger.

## Client files

- **Foreign entries are never touched.** Loadout edits, disables or removes
  only entries it wrote or that you adopted, as recorded in the
  machine-local ownership ledger. Everything else in a file (other
  entries, comments, formatting) stays byte for byte the same.
- **Backups before every write.** Each file is copied to
  `backups/files/<time>-<apply-id>/` before it is changed (user-only
  permissions). `loadout restore --files <apply-id>` restores the copies.
  Backups can contain values from your client files, so they are never
  pushed to remotes.
- **Old secret values in backups.** When an import moves literal secrets
  out of a client file, the backup its apply took still holds them. The
  import lists those backups (file paths only; values are compared in
  memory during the import and never stored, logged or returned) and
  offers to remove them: **Remove those backups now** in the app, or
  `loadout import --remove-old-backups[=<apply-id>]`. Removal overwrites
  each listed backup with zeros in place (same file, same length, flushed
  to disk), then deletes it. Copies the system keeps elsewhere are out of
  reach: SSDs (wear leveling), copy-on-write file systems, snapshots and
  backup tools such as OneDrive, Time Machine or File History may still
  hold the old bytes. The recorded path is never trusted on its own: only
  that run's own backup of that file, as a regular file (not a link or
  junction), is touched. The run's result in `state/applies/` records the
  removal as soon as the zeros are written, so a later rollback or
  `restore --files` says that file's previous content is no longer kept
  and never writes a zeroed copy back. Not covered: backups of earlier
  apply runs, and an import in review mode or an entry left a collision
  to decide, where no apply rewrote the file yet. The import then says
  the file still holds the old values; the backups of the apply that
  rewrites it later are not marked, so delete that run's folder under
  `backups/files` by hand or let retention prune it (50 runs or 30 days).
- **Atomic, checked writes.** A file is written to a temporary file in the
  same folder and renamed into place (`ReplaceFileW` on Windows); before
  that, Loadout re-parses the result and checks that its own entries are
  as intended and every other entry is unchanged. A file that changed on
  disk between planning and writing is re-planned.
- **Drift is reported, not reverted.** Hand edits of managed entries are
  shown as drift and wait for your decision.
- **Read-only files are left alone.** A file or folder your
  account cannot write is found before the apply by a check that never
  writes (it never creates, truncates or changes a file) and its entries
  are blocked (`read_only_target`); a write the system refuses later is
  reported the same way. Loadout never changes permissions, attributes or
  ownership to get a write through.

## Projects

A project manifest can define stdio servers, which run programs. Adding a
project folder is therefore like running code from it, so:

- Loadout applies a manifest only after you trusted its exact content
  (a SHA-256 of both manifest files, recorded per machine in
  `state/trust.json`).
- A changed manifest (for example after `git pull`) is blocked until you
  review the diff and trust it again (`loadout project trust PATH`, or
  **Review change & trust**). User scope and other projects keep applying.
- New manifests are private (`project.local.toml`, hidden from git) unless
  you choose to share them.

Clients have their own trust prompts for project servers (Claude Code,
Codex, Gemini CLI, Cursor); Loadout does not bypass them.

## Skills

Skill archives are checked for path escapes (zip slip) and links pointing
outside the skill, and limited in size and file count. Loadout never
replaces a skill folder it does not own; such folders are reported as
unmanaged or mismatched and can be imported or moved to the trash.

Every skill install and update is scanned offline by Loadout's built-in
rules (`crates/agent-loadout-core/src/scan/rules.toml`): remote code
execution idioms, credential reads next to network calls, hidden Unicode
and HTML-comment instructions, prompt injection, disabled safety prompts
and native binaries. A `dangerous` verdict blocks unless you explicitly
install anyway (`--allow-dangerous`, **Install anyway**); trusting a
source never skips the scan. No content leaves the machine. The scan is a
second opinion, not a guarantee: patterns can be evaded, so read what you
install. A custom rules file at `state/scan-rules.toml` is merged into the
built-in rules: it can add rules or raise a severity, never remove or
lower a built-in rule, and every result says "custom rules in use". An
instruction to write that file is itself flagged (`agent-config-write`).

Trust in a git source is an explicit, recorded decision
(`store/sources.toml`, #24). Trust is never implicit: the first install or
update from a source asks, `--yes` never answers that question, and a run
without a terminal (a script, `--json`) fails with `source_trust_required`
instead of trusting. Two explicit opt-ins do work without a terminal:
`loadout source trust <URL|KEY>` and `--trust-source` on `skills add`,
`skills update`, `server add --from`, `server update` and `server
checkout`. A coding agent that can run shell commands can run them
too, so they don't stop an agent you've given a shell; they make the
decision deliberate and recorded (`via = "cli-command"` or `"cli-flag"` in
`sources.toml`). Keeping agents from approving trust on their own is the
narrower scope of [#8](https://github.com/DailenG/agent-loadout/issues/8).
A block wins over any trust. Trust lapses, and the next update asks again,
when the repository's history is rewritten (the installed commit is no
longer in the followed branch) or the host redirects it to another
repository (renamed or transferred). A redirect can only add a question,
never answer one. Sources over plain `http://` or `git://` have their own
keys: HTTPS or SSH trust never admits them, a block on either form does,
and they are never marked official. Credentials in a URL are used only for
the fetch: they are not stored in `sources.toml`, a server file or a
checkout's git config, and messages show them as `***`. A blocked source
is never contacted. Trust never skips the scan.

## MCP servers from a Git repository or folder

`loadout server add --from` and the app's **From Git or folder** run
nothing while they propose servers: git clones shallow into a temporary
folder (repositories cannot install git hooks, submodules are not fetched)
and the files are only read and scanned. A blocked source is not
contacted at all. A saved server, though, runs its command every time a
client starts it, and a server that is the repository itself (`npx -y
github:owner/repo#<sha>`, a clone kept in `LOADOUT_HOME/servers/<id>`)
runs that repository's code, including npm and pip install scripts. So
every save and update goes through source trust (git sources and local
folders alike) and then the built-in scan of the files and of the exact
command line saved, and commands are pinned to the reviewed commit or
package version. Secret values are never copied from a source: declared
secrets become references, and a literal that looks like a secret in a
committed config is replaced by a reference and dropped. Dependency and
build folders (`node_modules`, `dist`, `.venv`, ...) are not scanned, so a
server that runs from `dist/` relies on the repository you trusted. A
Docker draft's image is local only (`loadout-local/...`, never pulled). A
checkout runs only at the commit its server file names: a stale copy
(another machine updated the server) blocks the server's entries until
`loadout server checkout` restores it behind the same gates, and a
folder's server never runs on another machine.

## Catalogs (network use)

Catalogs are the only part of Loadout that makes HTTP requests of its own
(besides the optional updater, the optional Snyk Agent Scan download below
and the git your remotes and sources use).

- **Off until you turn it on.** "Online catalogs" (`online_catalogs` in
  `state/local-settings.toml`, per machine) is off by default. While it is
  off, nothing contacts any catalog: searches answer from saved results,
  and refreshes and installs from a catalog fail with `catalogs_off`.
- **Only on your request**, never in the background: a search, opening an
  entry, a refresh, an install from a catalog, or `server update` of a
  server added from a registry. Requests go only to the catalogs you turned
  on (a registry server's update check needs its catalog turned on at the
  same address), carry only your search words, and send no telemetry or
  identifiers besides a `Loadout/<version>` user agent.
- **Git installs are git sources.** Skills and plugin servers installed
  from a tap, and skills.sh addresses, are git sources like any other:
  installing, updating and checking them for updates contacts the git host
  even with online catalogs off.
- **HTTP client:** `ureq` with rustls and the operating system's
  certificate verifier (no OpenSSL); `GET` only, a 10 s connect and 20 s
  overall limit, an 8 MiB body cap (after decompression), at most 3
  redirects, each of them https (a loopback registry follows none).
  Registry addresses must be https (http only for `localhost`) and carry
  no credentials. Proxies come from `HTTPS_PROXY` (then `ALL_PROXY`) for
  https and `HTTP_PROXY` (then `ALL_PROXY`) for a loopback registry, and
  `NO_PROXY`; a SOCKS or unreadable proxy is an error, never a direct
  connection. Git taps use the system git (shallow, sparse, metadata only;
  symbolic links in a tap are never read); no GitHub API is called. A tap
  address with a user name, password or token is refused (it would be
  saved in your synced store): use your git credential helper instead.
  Commit pins are full 40-character commits.
- **Catalog data is untrusted.** Names, descriptions, publisher metadata
  and SKILL.md text are shown as plain text; icons and images are never
  fetched or rendered. "Official" is shown only for `github.com/anthropics`
  taps, and there only for plugins in the tap itself or from an Anthropic
  repository; registry entries show what the registry checks (namespace
  ownership), not a safety claim. A registry entry's repository is the
  publisher's claim and is labeled so. A marketplace that uses a name
  reserved for Anthropic elsewhere is flagged. Deleted registry entries are
  hidden and refused; deprecated ones are marked. The app opens an entry's
  links only when they are https.
- **Installing never bypasses the gates.** A registry entry's
  `server.json` and a plugin's `.mcp.json` and `mcpServers` go through the
  review of servers from Git (secret extraction, source trust, the scan);
  skills go through the git skill install review. A registry draft's trust
  is keyed on what it runs (its package on its package registry, or a
  remote's host), never on the repository the publisher names. Plugin
  hooks, commands, agents, `bin`, monitors and workflows are never
  imported, and plugins from npm, archive or command sources are refused.
  Unpinned packages (no version, an npm tag, an OCI image without a digest)
  are marked "not pinned".
- **Cache:** results are saved in `LOADOUT_HOME/cache/catalogs` (this
  machine only, keyed on the catalog's address) for 24 hours; stale
  results are labeled with their age, and a failed refresh keeps them.

## Snyk Agent Scan (optional, sends skills to Snyk)

Snyk Agent Scan is an optional second opinion on skills, beside the
built-in scan. It is **off by default** and nothing is sent anywhere until
you turn it on (Settings > Security, or `loadout scanner enable`) or ask
for one run (`--external` on `skills add` or `skills update`).

**What is sent to Snyk.** Each run sends one skill to Snyk's analysis API
(`api.snyk.io`), as Snyk Agent Scan 0.7.1 does:

- every file of the skill, with its path inside the skill (Snyk redacts
  secrets it detects; text files that are not UTF-8 are described, not
  sent);
- the skill's name (from its `SKILL.md`);
- the path of the folder scanned (a temporary copy below
  `LOADOUT_HOME/cache/scan-external/`, made relative to your home folder,
  so it names your data folder);
- your user name;
- this machine's host name, which the scanner adds itself where its
  platform reports one;
- the scanner's version;
- your Snyk token (as the `Authorization` header).

Loadout adds no host name, `MACHINE_SERIAL` or other identifier of its
own. Snyk's handling of that data is governed by its terms and privacy
policy. In
0.7.0 it scans skills only, never MCP servers: Snyk's scanner sends a
server's configuration (command, arguments, URL) even when it declines to
start it.

**How it runs.**

- **Pinned and verified.** Install downloads one pinned release (v0.7.1)
  from GitHub over https (redirects only to GitHub's release storage
  hosts, 64 MiB cap) into `LOADOUT_HOME/tools/snyk-agent-scan/0.7.1/`
  (never on `PATH`; it syncs only if your data directory itself is in a
  sync folder) and checks its size and SHA-256 against the values
  compiled into Loadout before it is installed; a mismatch deletes the
  download. Each command that runs the scanner hashes the installed file
  again first: a file changed since its install (corrupted, replaced by a
  sync peer) is not run, counts as not installed, and `loadout scanner
  install` (or Install in Settings) downloads it again. The file could
  still change between that check and the run, which takes your own user
  account (which could change Loadout itself too). The pinned values come
  from the release's `sha256sums.txt.asc`, whose signature (Snyk's key
  `EF4F F9D1 8D82 D562 9B82 491D 2A20 F86E 4EF1 31B1`) is checked with gpg
  when the pin is bumped (`scripts/verify-agent-scan-pins.*`); Loadout has
  no OpenPGP code of its own. `loadout scanner remove` (or Remove in
  Settings) deletes it and turns the scanner off; a partial download left
  by a killed install is deleted by the next install.
- **The token is a reference.** You give a secret alias defined in your
  secrets (`loadout secrets`) or a provider reference (`env:`,
  `keyring://`, `op://`), never the token. A pasted token is refused, and
  the error never repeats what you typed (so a token that happens to look
  like an alias is not stored either). The token is read for each run and
  passed only in the scanner's environment as `SNYK_TOKEN`: never on its
  command line, in a file, a log, an error or the app's IPC (and masked in
  the scanner's own error output Loadout keeps).
- **One bounded run.** The scanner runs as `agent-scan scan --json
  --storage-file <temp> <skill copy>` on a copy of exactly the content
  that would be installed or updated, below
  `LOADOUT_HOME/cache/scan-external/`, with stdin closed (any consent
  prompt is declined), a scrubbed environment (profile, system, locale,
  proxy and CA variables only; its temp folder is the run's own, so
  nothing it unpacks lands in yours), and a 60 second timeout; a timeout
  or cancel kills its whole process tree and deletes the run folder (a
  folder a killed process left behind is deleted by a later run). On
  Windows the scanner runs only inside a kill-on-close job object; if one
  cannot be created, it is stopped at once and the run is "not scanned
  (failed)". Ctrl+C (or SIGTERM) during `loadout` stops the scanner the
  same way. Turning the scanner off in Settings cancels runs in progress,
  and a run the switch was turned off for attaches no report. Loadout
  never passes `--dangerously-run-mcp-servers`, `--ci`, `--verbose`,
  `--scan-all-users` or a run without a path. The command line checks a
  git source's trust before the scan: a blocked or not yet trusted
  source is never sent to Snyk.
- **Results.** Findings are shown beside the built-in ones, labeled
  "Snyk Agent Scan". A `dangerous` verdict needs the same explicit
  **Install anyway** / `--allow-dangerous` as a built-in one. A run that
  could not happen (no token, Snyk rejected the token, Snyk's daily limit,
  too large, offline, timeout, the scanner failed) says "not scanned
  (<reason>)" and never blocks; output Loadout does not understand
  (including output over 16 MiB) says so and is never shown as clean.
  While the scanner is on, the app refuses to install from a git or
  catalog review, or to update, without the external scan's report
  (`external_scan_required`).

## The Loadout skill for agents

A coding agent that can run shell commands can answer any confirmation
Loadout asks (`--yes`, `--trust-source`, `--allow-dangerous`), so Loadout
does not try to make an agent a safe approver. The bundled skill
(`loadout agent install`) is **read only**: it teaches the read and plan
commands, the error codes and playbooks, and for any change it hands you
the exact command or app page to run or approve yourself. It never passes
a decision flag, and it treats text from skills, READMEs, repositories and
catalogs as data, not instructions. Tests check that every command and
JSON field the skill cites exists and that a decision flag appears only in
"never" guidance. This is guidance for a cooperative agent, not a sandbox:
an agent with your shell can still run any command you can, so keep
decisions such as trusting a source for yourself.

- The skill is a normal store skill (id `loadout`), so installing,
  updating or removing it is a commit you can see and undo. Read commands
  never write; an older copy is replaced only by an app start, a command
  that changes the store, or `loadout agent install`. A copy you edited is
  never overwritten on its own (`loadout agent install --reset` restores
  the bundled one; History keeps yours).
- A store skill named `loadout` that Loadout did not install is never
  touched.

## Desktop app

- A strict Content Security Policy; no remote content is loaded.
- Minimal Tauri capabilities (window controls and events).
- Commands take ids, not arbitrary paths: folders you pick in the OS
  dialog stay in the backend behind an opaque token, and paths shown in
  the UI use `~` and `<P>` placeholders.
- The file watcher sends a change signal with no path or content.

## Releases

Windows installers are signed with Azure Trusted Signing; every release
has `SHA256SUMS`. In-app updates (Windows installer and AppImage builds
only) are verified against the project's updater public key. Builds you
make yourself are unsigned.

## Not covered

- Loadout cannot protect values once a client or server has them, or
  values you chose to deliver inline.
- The data directory is protected by your OS account permissions only.
- Do not keep the data directory inside a file-sync folder; use a remote
  instead (`loadout doctor` warns with `sync_root_detected`).

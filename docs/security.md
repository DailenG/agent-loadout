# Security model

What Loadout guarantees about your secrets and your client files, and what
it does not. To report a vulnerability, see [SECURITY.md](../SECURITY.md).
The design is in [SPEC.md](../SPEC.md) sections 11 and 17.

## What Loadout touches

Loadout reads and writes:

- its own data directory (`LOADOUT_HOME`): the store, `state/`,
  `backups/`, `cache/`, `logs/`;
- the MCP config files and skill folders of the clients listed in
  [adapters.md](adapters.md), at user scope and in the project folders you
  add;
- your OS keyring (entries under the service `loadout`) and, when you use
  it, the 1Password CLI `op`.

It uses the network only for remotes you add, git skill sources, and the
optional update check. Everything else works offline.

## Secrets

- **No secret values in the store.** Servers refer to secrets by alias
  (`${secret:ALIAS}`); `secrets.toml` holds provider references
  (`env:`, `keyring://`, `op://`, `op-env://`), never values. Because the
  store has no values, pushing it to a remote does not publish secrets.
- **Leak scanner.** Every store commit is scanned; a commit that contains
  something that looks like a secret value is refused before anything is
  written. `loadout doctor --scan-history` scans older commits too (it
  reports, and never rewrites history). `loadout import` does not import
  an entry that holds a literal that looks like a secret (it reports
  `needs_secret_extraction`); the SkillDock migration asks where each
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
- **Atomic, checked writes.** A file is written to a temporary file in the
  same folder and renamed into place (`ReplaceFileW` on Windows); before
  that, Loadout re-parses the result and checks that its own entries are
  as intended and every other entry is unchanged. A file that changed on
  disk between planning and writing is re-planned.
- **Drift is reported, not reverted.** Hand edits of managed entries are
  shown as drift and wait for your decision.

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

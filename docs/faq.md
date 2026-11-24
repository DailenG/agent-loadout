# FAQ

**What does Loadout do, in one sentence?**
It keeps one versioned store of MCP servers, skills and secret references
and writes them into the native config files of your AI coding clients,
without touching anything it did not write. See [concepts.md](concepts.md).

**Why does Manage or the MCP page show 0 servers when my client has some?**
Those servers were not written by Loadout, so they are *foreign* entries:
Loadout leaves them alone, and the list shows only your store. Look under
**Not managed by Loadout** below the list (and in Tools > Manage): each
entry has **Import...**. The Import page, or `loadout import --scan` and
`loadout import`, brings them in. See
[troubleshooting](troubleshooting.md#manage-or-the-mcp-page-shows-0-servers-but-my-client-has-some).

**How do I bring in the setup I already have?**
Open the **Import** page: MCP servers (from your user files and every
project this machine knows), skill folders, the project folders your
clients already know, and the SkillDock migration. On the command line:
`loadout import`, `loadout skills adopt`, `loadout project discover`.

**What does "7 not managed" in the sidebar mean?**
Seven MCP entries or skill folders in your clients are not managed by
Loadout and are new since you last looked. Click it for details, then
**Review** or **Dismiss**. Settings, Preferences, **Not-managed notice**
turns it off.

**Will Loadout overwrite or delete my existing client config?**
No. It edits only entries it created or you adopted, keeps comments and
formatting, backs up every file before writing, and can restore any file
byte for byte (`loadout restore --files`). See [security.md](security.md).

**Where are my secrets stored?**
Not in Loadout. The store holds references to an environment variable,
your OS keyring or 1Password. Values are fetched when a client starts a
server, by default through `loadout exec`, and kept in memory.

**Why "User" and not "System"?**
Earlier versions called the user scope "System". It is your user-level
client configuration, so the app now says **User**. The CLI's `--scope`
option still takes `system` for it.

**Why do the pages show a different set of servers after I picked a project?**
Skills, MCP and Tools follow the sidebar **Scope** switcher. Pick **User**
for your user-level setup. The choice is not saved; the app starts at User.

**Why is a project read only?**
Usually its manifest is untrusted or changed since you trusted it.
Manifests can start programs, so Loadout waits for **Review & trust**
(`loadout project trust PATH`). A lock marked **Read only** means
something else: your account cannot write the project's files (a folder
managed by a security-focused installer, a read-only share). Loadout then
leaves them alone and applies everything else; see
[troubleshooting](troubleshooting.md#a-file-or-project-is-marked-read-only).

**Does an import leave my secret values anywhere?**
The store gets only a `${secret:ALIAS}` reference; the value goes to the
keyring, 1Password or an environment variable. The backup the import's
apply took of the client file still holds the old value until you choose
**Remove those backups now** (or `loadout import --remove-old-backups`).
See [troubleshooting](troubleshooting.md#my-client-files-old-secret-values-are-still-in-backups).

**Why did `adopt` keep some skill folders?**
Loadout replaces a folder with its link only when the store copy holds
everything the folder does; the result gives each kept folder's reason.
See [troubleshooting](troubleshooting.md#adopt-kept-some-skill-folders).

**What is the difference between instant and review mode?**
Instant (default) applies each edit at once. Review collects edits in
Pending changes for you to apply or discard. Set
`apply_mode = "review"` in `store/settings.toml`.

**Can I undo an apply?**
Yes: **Undo this apply** in History, or `loadout rollback last`.

**Why is omp marked "Recommended"?**
Loadout is built and tested with [omp](https://github.com/can1357/oh-my-pi)
first, so it is listed first and badged. It is presentation only: Loadout
never enables anything in omp, or any client, on its own.

**Does Loadout need the network?**
No. Only remotes, git skill sources and the optional update check use it.

**Does Loadout need git installed?**
No for the store (it has git built in). Git remotes use your system `git`,
so they need it.

**Can I share my setup between machines?**
Yes, with a remote: a folder (USB drive, network share, synced folder) or a
git repository. Each machine keeps its own ledger, trust and secret
values. On a new machine, `loadout restore --from <folder|zip|bare-repo>`.

**Can I keep the data directory in OneDrive or Dropbox?**
No; sync tools corrupt live git repositories. Use a remote instead.

**Is there a macOS app?**
Not yet. The CLI builds on macOS with
`cargo install --git https://github.com/DailenG/agent-loadout agent-loadout`.

**How do I get 1Password to stop prompting?**
Store a service account token (`loadout secrets service-account set`); see
[troubleshooting](troubleshooting.md#1password-asks-for-approval-every-time-a-client-starts-a-server).

**Where do I report a bug or a security problem?**
Bugs: [GitHub issues](https://github.com/DailenG/agent-loadout/issues).
Security: privately, as described in [SECURITY.md](../SECURITY.md).

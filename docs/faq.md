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

**Why does Loadout ask whether I trust a source?**
Skills instruct your agents and MCP servers run programs with your
permissions, so installing from a repository is like running its code.
Loadout asks the first time you install, update or save from a Git source
(or a folder for an MCP server) and records your answer in
`store/sources.toml`: **Trust this time only**, **Always trust** the
repository (or its owner, or a self-hosted host), or **Cancel**. See
[concepts](concepts.md#sources-trust-and-the-scan).

**How do I revoke or block a source?**
Settings > Sources: **Revoke** removes a trust (the next install or update
asks again), **Block** turns it into a block, **Block a source...** blocks a
URL you never decided on, and **Unblock** removes a block. On the command
line: `loadout source ls`, `loadout source untrust <KEY>`, `loadout source
block <URL|KEY> [--scope repo|owner|host]`. None of these uninstalls
anything. A block wins over any trust, and a blocked source is never
contacted.

**What does "Install anyway" mean?**
The built-in scan found
something `dangerous` (for example `curl ... | sh` in a script, hidden
instructions, a credential upload), so Loadout stopped
before saving anything. **Install anyway** (or **Update anyway**, **Save
anyway**, `--allow-dangerous`) installs exactly the content you reviewed
despite the findings; it is offered only after you tick that you read
them. Use it only for content whose author you trust. Warnings never
block.

**Is my skill sent anywhere when it is scanned?**
No. The built-in scan runs offline on your computer; nothing is sent
anywhere.

**Can a coding agent approve a source for me?**
Not by accident: `--yes` never answers the trust question, and a run
without a terminal fails with `source_trust_required`. But
`--trust-source[=once|repo|owner|host]` and `loadout source trust` are
explicit opt-ins that need no terminal, so an agent with shell access can
run them. They are recorded (`via = "cli-flag"` or `"cli-command"`); keep
them for sources you trust yourself. The bundled Loadout skill for agents
never uses them (see below).

**Can my agent install things with Loadout?**
Not through the Loadout skill. The skill (`loadout agent install`,
Settings > Agent integration) is read only: it lets your agent check
status, diffs, effective config, import scans, dry runs and catalog
search, and explain errors, but for any install, enable, import, apply,
trust decision or restore it gives you the exact command or the app page
to use yourself. Any confirmation Loadout asks could be answered by an
agent with a shell, so the skill never asks the agent to make a change at
all, never passes `--yes`, `--trust-source` or `--allow-dangerous`, and
treats text from repositories and catalogs as data. It guides a
cooperative agent; it is not a sandbox, so an agent you let run any
command can still run Loadout's write commands. See
[concepts](concepts.md#the-loadout-skill-for-agents).

**Why are online catalogs off?**
Searching a catalog sends your search words to someone else's server, so
Loadout never does it without asking. The first time you open **Install >
Browse** it asks once, naming the hosts; `loadout catalog online on` does
the same on the command line. Even when they are on, nothing runs in the
background: Loadout contacts only the catalogs you turned on, only when
you browse, search, open an entry, refresh or install. Settings > Catalogs
turns it off again; the setting is per machine.

**What does "Official" mean in a catalog?**
Only that the code comes from Anthropic: it marks the
`github.com/anthropics` taps (such as Anthropic skills) and their skills
and plugins that live there, not third-party plugins an Anthropic
marketplace lists. The MCP Registry has no "official" or "verified" mark;
it checks that a publisher owns its namespace, which Loadout shows as
"namespace ownership checked by the registry". Neither is a safety claim:
installs still go through the trust question and the scan.

**Why can't this plugin be installed?**
Loadout installs Claude Code plugins only from a folder of the tap or a
Git repository (`github`, `url` and `git-subdir` sources). Plugins
published as an npm package, an archive or a command are listed with
"Can't be installed" and the reason, because Loadout can't review and scan
them the same way. Deleted registry entries are refused too. A plugin's
hooks, commands, agents and other code that would run on its own are
never imported; only its skills and MCP servers are.

**Why is a server blocked as "checkout out of date"?**
It runs from a copy of its repository that Loadout keeps on each machine
(`LOADOUT_HOME/servers/<id>`), and this machine's copy is missing or at
another commit than the server file names (another machine updated the
server, for example). Restore it with **Restore copy** in the server's
details on the MCP page, or `loadout server checkout <ID>`. See
[troubleshooting](troubleshooting.md#checkout_out_of_date-the-servers-local-copy-is-out-of-date).

**Why is a server blocked as "added from a folder on another machine"?**
A server added from a local folder runs from that folder, which exists only
on the machine that added it, so its entries are blocked on your other
machines (`folder_on_other_machine`). To use the server on several
machines, add it from a Git repository instead.

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
No. Only remotes, git sources of skills and MCP servers, online catalogs
(off until you turn them on) and the optional update check use it. The built-in scan runs
offline.

**Does Loadout need git installed?**
No for the store (it has git built in). Git remotes and installing skills
or MCP servers from Git repositories use your system `git`, so they need
it.

**Can I share my setup between machines?**
Yes, with a remote: a folder (USB drive, network share, synced folder) or a
git repository. Each machine keeps its own ledger, project trust, MCP
server checkouts and secret values; source trust decisions sync with the
store. On a new machine, `loadout restore --from <folder|zip|bare-repo>`.

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

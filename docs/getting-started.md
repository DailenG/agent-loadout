# Getting started

This walk-through takes you from installing Loadout to your first apply and
back again. For the ideas behind it, read [concepts.md](concepts.md); for
every option, the [user guide](user-guide.md) and the
[CLI reference](cli.md).

## 1. Install

Download from the [latest release](https://github.com/DailenG/agent-loadout/releases/latest).

| OS | Install |
|---|---|
| Windows | Run `Loadout_X.Y.Z_x64-setup.exe` (per user, no administrator rights, signed). It installs the app and puts `loadout` on your user `PATH`; open a new terminal afterward. Also `winget install --id DailenG.AgentLoadout` or `choco install agent-loadout` once those packages are published. |
| Arch Linux | AUR `agent-loadout-bin` or `agent-loadout` once published; until then `makepkg -si` in `packaging/aur/agent-loadout-bin` of this repository. |
| Debian, Ubuntu | `sudo apt install ./agent-loadout_X.Y.Z_amd64.deb` |
| Fedora, openSUSE | `sudo dnf install ./agent-loadout-X.Y.Z-1.x86_64.rpm` |
| Other Linux | `chmod +x agent-loadout_X.Y.Z_amd64.AppImage` and run it (desktop app only) |
| macOS, or CLI only | `cargo install --git https://github.com/DailenG/agent-loadout agent-loadout` (no desktop app) |

Check the CLI: `loadout --version` prints `loadout X.Y.Z`. If it prints
something else, another program named `loadout` comes first on your `PATH`;
see [troubleshooting](troubleshooting.md#another-program-named-loadout).

Full details per OS (checksums, packages, updater): [user guide, Install](user-guide.md#install).

## 2. First run

Start **Loadout** from the Start menu (or `agent-loadout-app` on Linux).
The data directory is created on first change: `%LOCALAPPDATA%\Loadout` on
Windows, `~/.local/share/loadout` on Linux. `loadout doctor --paths` shows
the folder in use.

The app offers **Add a backup remote**. It is optional; choose **Not now**
to stay local. Then open **Tools** to see which clients Loadout detected
and where their config files are.

Nothing has been written to any client yet. If your clients already have
MCP servers or skill folders, a one-line notice at the bottom of the
sidebar counts them, for example "7 not managed"; click it to see the
details, then **Review** (opens the Import page) or **Dismiss**.

## 3. Bring in what you already have

Your clients probably have MCP servers already. Loadout does not manage
them until you import them (they are *foreign* entries, see
[concepts](concepts.md#ownership-managed-and-foreign-entries)). The MCP
page lists them under **Not managed by Loadout**, and each Tools card
counts its client's.

**In the app**, open **Import** (in the sidebar, after Install). Scanning
only reads; nothing changes until you confirm.

- **MCP servers**: the entries of your user files and of every project
  this machine knows. An entry that is the same in several files is one
  row; a name with different definitions shows them side by side (use
  one, import both under their own IDs, or skip). A value that looks
  like a secret goes to the OS keyring by default, or to a 1Password
  reference or an environment variable; the store keeps only a
  `${secret:ALIAS}` reference. **Review import...**, then **Import**.
- **Skills**: the skill folders of your clients that Loadout does not
  manage. **Adopt** copies each into the store, enables it for the
  clients where it was found, and replaces each folder whose content
  matches with Loadout's link (the original goes to `backups/trash`).
- **Discover projects**: folders Claude Code, Codex, omp and Pi already
  know. **Add selected** adds them untrusted; each one waits for
  **Review & trust**.
- **From SkillDock**: **Migrate from SkillDock...** (SkillDock's folder is
  only read).

Details for each section: [user guide, Importing in the app](user-guide.md#importing-in-the-app).

**On the command line:**

```sh
loadout import --scan             # list the servers your clients have; changes nothing
loadout import                    # import all of them and take ownership
loadout import github linear      # or only some, by name
loadout import --client omp       # or only one client's
loadout import --only             # copy definitions without taking ownership
loadout import --pick docs=omp    # an entry that differs between clients: take omp's
loadout import --extract chatty=keyring  # a literal secret: send it to the OS keyring
loadout skills adopt              # list unmanaged skill folders; changes nothing
loadout skills adopt --all        # adopt every name that has one content
loadout project discover          # folders your clients know; --add ID or --all adds them
loadout migrate skilldock         # report; add --apply to migrate
```

An import that moved secrets out of a client file says that the backup
its apply took still holds the old values; **Remove those backups now**
in the app, or `loadout import --remove-old-backups`, deletes exactly
those backups.

## 4. Add a server and apply

In the app: **MCP**, **Add** (or **Paste config** with an entry copied from
any client), fill in the form, turn on the clients you want, and save. In
the default instant mode the change is applied at once and a notification
shows the result.

On the command line:

```sh
loadout server add github --client omp --client claude-code \
  --env 'GITHUB_PERSONAL_ACCESS_TOKEN=${secret:GITHUB_PAT}' \
  -- npx -y @modelcontextprotocol/server-github
loadout secrets set GITHUB_PAT --ref 'op://Developer/GitHub PAT/credential'
```

Use single quotes so your shell leaves `${...}` alone. The server stays
blocked for its clients until the alias resolves on this machine
(`loadout secrets check`).

To see what Loadout would do before it does it:

```sh
loadout status          # state of every entry
loadout diff            # the file diffs the next apply would make
loadout apply --dry-run # the plan, without writing
loadout apply           # write it (one commit; every file backed up first)
```

Then restart the client (Claude Desktop in particular reads servers only at
launch) and check that the server appears in its own MCP view.

## 5. Install a skill or MCP server from Git or a folder

Skills and MCP servers can come from any Git repository (https, ssh or a
local repository) or a folder on this machine.

**In the app:**

- **Install > Skill**, **Git** (a repository URL, optionally a branch or
  tag and a folder in it) or **Local** (a folder or a `.zip`/`.skill`
  archive), then **Review and install**. Loadout fetches and scans first
  and shows a review: the source, whether you trust it, the files and the
  scan's findings. Nothing is installed until you press **Install**.
- **Install > MCP > From Git or folder**: a repository URL (with an
  optional folder and branch, tag or commit) or a folder you pick, then
  **Find servers**. Loadout reads the repository's `server.json` (MCP
  Registry), MCPB `manifest.json`, committed client configs such as
  `.mcp.json`, or treats the repository itself as the server, and
  proposes servers. Nothing runs and nothing is saved yet: tick the ones
  you want, check the exact command each one runs (**Review and edit**
  opens it in the usual server form), choose **Enable for**, then
  **Save**.

**On the command line:**

```sh
loadout skills add https://github.com/acme/skills --path skills/release-notes
loadout server add --from https://github.com/acme/weather --dry-run   # list proposed servers and findings
loadout server add --from https://github.com/acme/weather --pick weather --client omp
```

Two checks run before anything from the source is saved:

- **Do you trust this source?** The first install, update or save from a
  Git source asks: **Trust this time only** (nothing recorded),
  **Always trust** the repository (or everything from its owner, or on a
  self-hosted host), or **Cancel**. In a terminal it is a prompt; in a
  script or with `--json` the command stops with `source_trust_required`
  unless you pass `--trust-source` (once) or `--trust-source=repo`
  (always). `--yes` never answers it. A folder you pick for an MCP server
  is asked about too; a skill folder or archive you pick is not.
  **Settings > Sources** and `loadout source ls` list your decisions. Only trust a
  source whose author you know: skills instruct your agents, and MCP
  servers run programs on your computer.
- **The built-in scan.** Loadout scans the fetched files (and an MCP
  server's command lines) offline with its own rules: download-and-run
  commands, credential theft, hidden text, prompt injection, native
  binaries, unpinned packages. Warnings are shown and never block. A
  `dangerous` finding blocks the install (exit code 8 on the command line)
  until you read the findings and choose **Install anyway**
  (`--allow-dangerous`). Trusting a source never skips the scan, and a
  clean scan is not a guarantee: still read what you install.

Commands are pinned to the commit or package version Loadout reviewed. A
server that runs code from the repository's own files keeps a copy of
it on this machine; a server added from a folder runs only on this
machine. Details: [user guide, Servers from a Git repository or
folder](user-guide.md#servers-from-a-git-repository-or-folder),
[The built-in scan](user-guide.md#the-built-in-scan) and
[Source trust](user-guide.md#source-trust).

## 6. Undo

- App: the notification's undo, or **History**, pick the apply, **Undo this
  apply**.
- CLI: `loadout rollback last` (or an apply id from `loadout history`).

Rollback restores the store to before that run and applies the result;
entries you edited by hand since are not overwritten without asking. To put
a client file back exactly as it was, byte for byte:
`loadout restore --files <apply-id>` (History, Client file backups tab).

## Next steps

- Per-project servers and trust: [user guide, Projects and trust](user-guide.md#projects-and-trust).
- Install from anywhere, safely: [concepts, Sources, trust and the scan](concepts.md#sources-trust-and-the-scan).
- Secrets from the keyring or 1Password: [user guide, Secrets](user-guide.md#secrets).
- Backups and other machines: [user guide, History, restore and remotes](user-guide.md#history-restore-and-remotes).
- Something wrong? [troubleshooting.md](troubleshooting.md) and [faq.md](faq.md).

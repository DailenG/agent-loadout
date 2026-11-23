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

Nothing has been written to any client yet.

## 3. Bring in what you already have

Your clients probably have MCP servers already. Loadout does not manage
them until you import them (they are *foreign* entries, see
[concepts](concepts.md#ownership-managed-and-foreign-entries)), so the MCP
page starts empty even when your clients are not.

Importing currently happens on the command line:

```sh
loadout import --scan             # list the servers your clients have; changes nothing
loadout import                    # import all of them and take ownership
loadout import github linear      # or only some, by name
loadout import --client omp       # or only one client's
loadout import --only             # copy definitions without taking ownership
loadout import --pick docs=omp    # an entry that differs between clients: take omp's
loadout import --extract chatty=keyring  # a literal secret: send it to the OS keyring
```

Skills work the same way:

```sh
loadout skills scan                       # managed, unmanaged and mismatched skill folders
loadout skills import claude-code my-skill  # bring one unmanaged folder into the store
```

Coming from SkillDock? Use Settings, **Migrate from SkillDock...** in the
app, or `loadout migrate skilldock` (report) then
`loadout migrate skilldock --apply`. SkillDock's folder is only read.

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

## 5. Undo

- App: the notification's undo, or **History**, pick the apply, **Undo this
  apply**.
- CLI: `loadout rollback last` (or an apply id from `loadout history`).

Rollback restores the store to before that run and applies the result;
entries you edited by hand since are not overwritten without asking. To put
a client file back exactly as it was, byte for byte:
`loadout restore --files <apply-id>` (History, Client file backups tab).

## Next steps

- Per-project servers and trust: [user guide, Projects and trust](user-guide.md#projects-and-trust).
- Secrets from the keyring or 1Password: [user guide, Secrets](user-guide.md#secrets).
- Backups and other machines: [user guide, History, restore and remotes](user-guide.md#history-restore-and-remotes).
- Something wrong? [troubleshooting.md](troubleshooting.md) and [faq.md](faq.md).

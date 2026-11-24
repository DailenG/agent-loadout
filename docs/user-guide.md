# Loadout user guide

Loadout keeps one store of MCP servers, skills and secret references and
writes them into the config files of your AI coding clients (oh-my-pi, the
recommended client, plus Claude Code, Codex, Pi, Claude Desktop, GitHub
Copilot, Cursor and 31 more). Loadout is built and tested with
[omp](https://github.com/can1357/oh-my-pi) first, so the app lists it first
and marks it "Recommended"; the Tools page offers "Get omp" when it isn't
installed. The recommendation is presentation only: Loadout never enables
anything in omp, or any other client, on its own.
Every change is a commit in a local git repository, so you can see what
changed, undo an apply, and restore any earlier version. Secret values
never enter the store: servers refer to secrets by alias, and the alias
points at an environment variable, the OS keyring or 1Password.

Loadout has a desktop app and a command-line tool, `loadout`. Both work on
the same data and can be used side by side. New to Loadout? Start with
[getting-started.md](getting-started.md); the ideas this guide relies on
(store, scopes, ownership, drift, trust) are explained in
[concepts.md](concepts.md). Every command and option is in the
[CLI reference](cli.md), the per-client details are in
[adapters.md](adapters.md), and the full design is in
[SPEC.md](../SPEC.md).

Contents:

1. [Install](#install)
2. [First run](#first-run)
3. [Where Loadout keeps its data](#where-loadout-keeps-its-data)
4. [The desktop app](#the-desktop-app)
5. [The command line](#the-command-line)
6. [Servers](#servers)
7. [Clients and apply](#clients-and-apply)
8. [Projects and trust](#projects-and-trust)
9. [Secrets](#secrets)
10. [History, restore and remotes](#history-restore-and-remotes)
11. [Skills](#skills)
12. [Migrating from SkillDock](#migrating-from-skilldock)
13. [Updates](#updates)
14. [Troubleshooting](#troubleshooting)
15. [Uninstall](#uninstall)

## Install

Releases are published on GitHub at
<https://github.com/DailenG/agent-loadout/releases>, tagged `vX.Y.Z` (the
first published release is `v0.1.2`). Each release has these assets
(`X.Y.Z` is the version):

| Asset | What it is |
|---|---|
| `Loadout_X.Y.Z_x64-setup.exe` | Windows installer (NSIS, per user) |
| `agent-loadout_X.Y.Z_amd64.AppImage` | Linux AppImage |
| `agent-loadout_X.Y.Z_amd64.deb` | Debian and Ubuntu package |
| `agent-loadout-X.Y.Z-1.x86_64.rpm` | Fedora and openSUSE package |
| `agent-loadout-X.Y.Z-x86_64-linux.tar.gz` | Linux binaries (used by the AUR `agent-loadout-bin` package) |
| `SHA256SUMS` | SHA-256 checksums of the assets above |
| `latest.json`, `*.sig` | The in-app updater feed and update signatures (only on releases built with the updater key) |
| `winget-manifests-X.Y.Z.zip` | The winget manifests the maintainer submits to winget-pkgs |

To check a download against `SHA256SUMS`:

- Linux: `sha256sum -c SHA256SUMS --ignore-missing` in the download folder.
- Windows (PowerShell): `Get-FileHash .\Loadout_X.Y.Z_x64-setup.exe -Algorithm SHA256`,
  then compare the hash with the line for that file in `SHA256SUMS`.

### Windows

**winget** (once the package is accepted; the submission, kept at the latest
release, is waiting for a moderator's review in winget-pkgs):

```
winget install --id DailenG.AgentLoadout
```

**Chocolatey** (once the package is approved; it is waiting for moderation
on the Chocolatey Community Repository):

```
choco install agent-loadout
```

The package downloads the release installer (SHA-256 checked) and runs it
silently, so it installs for the account that runs `choco`, like the
installer below; run it from an elevated prompt of the account that will
use Loadout (the package refuses to install as SYSTEM). `choco upgrade
agent-loadout` updates it. If the in-app updater updates Loadout first,
Chocolatey keeps showing the older version until the next upgrade; to
leave updates to Chocolatey alone, clear "Check for updates" in Settings,
Updates.

**Installer**: download and run `Loadout_X.Y.Z_x64-setup.exe`. It needs no
administrator rights. It installs into `%LOCALAPPDATA%\Programs\Loadout`:

- the desktop app, `agent-loadout-app.exe`, with a Start menu entry named
  "Loadout";
- the command-line tool, `loadout.exe`, next to it.

The installer adds the install folder to your user `PATH`, so `loadout`
works in a new terminal (open a new one after installing). Uninstalling
removes it from `PATH` again.

Release builds are signed with Azure Trusted Signing. Builds you make
yourself from source, and contributor builds, are unsigned, so Windows
SmartScreen may warn about them.

### Arch Linux

Two AUR packages, not on the AUR yet (AUR account registration is closed
for now):

- `agent-loadout`: builds from source.
- `agent-loadout-bin`: repackages the release tarball.

Until they are published, build either from this repository:

```sh
git clone https://github.com/DailenG/agent-loadout
cd agent-loadout/packaging/aur/agent-loadout-bin   # or agent-loadout
makepkg -si
```

Once they are on the AUR, install either with your AUR helper, for example
`yay -S agent-loadout-bin` or `paru -S agent-loadout`. Both install
`/usr/bin/loadout` (the CLI), `/usr/bin/agent-loadout-app` (the desktop
app) and a desktop entry.

### Other Linux distributions

- **AppImage**: `chmod +x agent-loadout_X.Y.Z_amd64.AppImage`, then run it.
- **Debian, Ubuntu**: `sudo apt install ./agent-loadout_X.Y.Z_amd64.deb`
- **Fedora, openSUSE**: `sudo dnf install ./agent-loadout-X.Y.Z-1.x86_64.rpm`
  (or `sudo zypper install ./agent-loadout-X.Y.Z-1.x86_64.rpm`)

The `.deb` and `.rpm` packages are named `agent-loadout` (not `loadout`, the
name of an unrelated tool's packages). They install the desktop app as
`/usr/bin/agent-loadout-app`, with a "Loadout" menu entry, and the CLI as
`/usr/bin/loadout`. The AppImage holds both, but only the app is started
from it; for the CLI on `PATH`, use a package or `cargo install`.

The desktop app needs WebKitGTK 4.1. If the window opens blank, see
[Blank window on Linux](troubleshooting.md#blank-window-on-linux). To store secrets in the OS
keyring, you need a Secret Service provider (see
[Secrets on Linux](troubleshooting.md#secrets-on-linux)).

### Command line only (any OS)

```
cargo install --git https://github.com/DailenG/agent-loadout agent-loadout
```

This builds and installs `loadout` from the repository; it needs a current
stable Rust toolchain. Once the crate is published on crates.io,
`cargo install agent-loadout` will do the same. There is no desktop app
this way, and no in-app updater: rerun the command to update.

### macOS

No macOS installer is published. Install the CLI with cargo as above.

## First run

1. Start the app ("Loadout" in the Start menu, or `agent-loadout-app` on
   Linux), or run `loadout` commands. The data directory and an empty
   store are created the first time something is saved.
2. The app offers **Add a backup remote**. A remote is optional: it backs
   the store up to a git host or a folder (a USB drive, a network share, a
   synced folder). Choose "Not now" to stay local; you can add one later
   in Settings.
3. Open **Tools** to see which clients Loadout detected and where their
   config files are.
4. Bring in what you already have, from the **Import** page or the
   command line:
   - From SkillDock: Import, **Migrate from SkillDock...** (or
     `loadout migrate skilldock`). See
     [Migrating from SkillDock](#migrating-from-skilldock).
   - From your clients' own config files: the Import page's **MCP
     servers**, **Skills** and **Discover projects** sections (see
     [Importing in the app](#importing-in-the-app)), or
     `loadout import --scan`, which lists the MCP servers your clients
     already have; `loadout import` brings them into the store and takes
     ownership of them (`--only` copies the definitions without enabling
     them). `loadout skills adopt` lists the skill folders your clients
     have that Loadout does not manage, and `loadout skills adopt --all`
     brings them in (see [Skills](#skills)). `loadout project discover`
     lists the project folders your clients already know.
5. Add or edit servers on the **MCP** page and turn them on per client.
   Each change is applied to the client files at once (instant mode), and
   the result is shown in a notification you can undo.

Nothing is written to a client file until a server is enabled for that
client, and each write keeps a backup copy of the file first.

## Where Loadout keeps its data

| OS | Default data directory |
|---|---|
| Windows | `%LOCALAPPDATA%\Loadout` |
| Linux | `${XDG_DATA_HOME:-~/.local/share}/loadout` |
| macOS | `~/Library/Application Support/Loadout` |

Override it with the environment variable `LOADOUT_HOME`, or per command
with `loadout --home <PATH>`. `loadout doctor --paths` prints the folder in
use and where it came from. Inside it:

- `store/`: the git repository with your servers (`servers/<id>.toml`),
  secret references (`secrets.toml`), client preferences (`clients.toml`),
  portable settings (`settings.toml`), skills, and project and machine
  files. It never contains secret values.
- `state/`: this machine only (ownership ledger, remotes, trusted projects,
  `local-settings.toml`, apply records).
- `backups/`: copies of client files taken before each write, and the
  trash. These can contain secrets from your client files and are never
  pushed.
- `cache/`, `imports/`, `logs/`.

Do not put the data directory inside a file-sync folder (OneDrive,
Dropbox, Google Drive, Syncthing, Resilio Sync): sync tools corrupt live
git repositories. Loadout warns (`sync_root_detected`) when it finds one.
To share the store between machines, use a remote instead (a folder remote
inside a synced folder is supported).

## The desktop app

The sidebar has a **Scope** switcher at the top: **User** (the user scope,
for all your projects), one entry per project with its trust state (the
tooltip shows its folder), and **+ Add folder...**, which adds a project and
selects it. Skills, MCP and Tools follow the scope you pick; switching
closes any open card or dialog. The choice lasts until you quit: the app
always starts at User. The pages:

- **Skills**: installed skills in list, grid or grouped view; Check
  updates; per-skill details with **Enable in Tools** (per client), Enable
  in Project, files and update diffs. Source tabs show each client's skill
  folder with Managed, Unmanaged and Conflict entries, plus the Agents CLI
  skills; unmanaged ones can be adopted (**Adopt** on one folder, **Adopt
  all identical** for every name with one content), imported into the
  store or moved to the trash.
- **MCP**: your servers with their per-client state (enabled, disabled,
  pending, drift, blocked, not supported). At project scope it lists that
  project's manifest servers (see [Projects and trust](#projects-and-trust)).
  Toolbar: search, Refresh, **Paste config** (paste a server entry from any
  client's JSON or TOML) and **Add**. The edit dialog has Form, TOML and
  Preview tabs; Preview shows the entry each client gets, with secrets
  masked. Below the list, **Not managed by Loadout** shows the entries
  the client files of the scope hold that Loadout leaves alone, per client
  and file, with **Import...** on each; the line above the list counts
  both ("7 managed, 3 not managed by Loadout").
- **Projects**: Add folder, Init manifest..., **Discover projects**, and
  per project Open, Effective config (which server each client loads in
  that folder and from which file), Apply, Remove from list, and Review
  change & trust. A project your account cannot write is marked **Read
  only** (see [Read-only files and folders](#read-only-files-and-folders)).
- **Tools**: every client with its tier, whether it is installed, its user
  and project config files (with Open folder), and a capability summary.
  At project scope the project files are named under that project's
  folder. **Manage** turns servers on and off for one client: at User in
  the store, at project scope in that project's manifest (the store
  servers the project inherits are listed read only, with **Open on MCP
  page**; an untrusted or changed project is read only until **Review &
  trust**). Each card counts the client's entries not managed by Loadout,
  and **Manage** lists them with **Import...**.
- **Install**: Skill (Git or Local folder or `.zip`/`.skill` archive) and
  MCP (Manual form or Paste config).
- **Import**: bring in what your clients already have: MCP servers,
  skill folders, project folders, and the SkillDock migration (see
  [Importing in the app](#importing-in-the-app)).
- **History**: every store commit, filtered by All, User or Projects
  (History covers every scope and does not follow the switcher), with Show
  diff, Undo this apply, Restore this version, Restore entity..., Tag and
  Download zip; a Client file backups tab; and the remotes strip with sync
  state and conflicts.
- **Settings**: App Preferences (Appearance, the not-managed notice),
  Versioning & Backup (remotes), Migration (a link to the Import page),
  Secrets, and Updates (see [Updates](#updates)).
- **About**: version, app identifier, repository and data directory.

A **pending changes** pill appears in the header when changes wait for
review or an apply left something blocked or undecided; it opens the
**Pending changes** drawer with per-file diffs, Apply all, Apply selected,
Discard pending (review mode) and Resolve... for entries that need a
decision.

### Importing in the app

The **Import** page's **MCP servers** section reads the MCP entries of
your user files and of every project this machine knows (trusted or not;
reading changes nothing) and lists those Loadout does not manage. Filter
them by **User files** or **Projects**.

- An entry that is the same in several files is one row, with the files
  listed. It is checked; uncheck what you want to leave alone.
- A name with different definitions shows them side by side. Choose
  **Use the definition from ...**, **Import both under their own IDs**
  (the second gets `NAME-<client>`; you can edit each ID) or **Skip for
  now**.
- **Already in the store** marks a name the store has; import it under
  another ID or leave it.
- A value that looks like a secret needs a destination: **OS keyring**
  (the default; the value moves from the client file into the keyring and
  is never shown), a **1Password reference** (`op://vault/item/field` or
  `op-env://<environment>/<NAME>`), an **Environment variable**, or **Skip
  this server**. The store keeps a `${secret:ALIAS}` reference.
- **Definition only** marks an entry found only in project files: it is
  stored but enabled nowhere. After the import, **Add to project
  manifest** names it in that project's manifest, for the clients whose
  files hold it. An entry in a shared file (`.mcp.json`) goes to the
  shared manifest by default, which manages that entry for everyone using
  the repository; for Claude Code and Devin, **Private** instead leaves
  the shared entry alone and adds a private one. An untrusted project asks
  for **Review & trust** first.
- Under **Import both**, a secret both definitions hold can only go to the
  keyring (each value gets its own alias) or skip the server.
- Only the entries the filter shows are imported.

**Review import...** shows each server with the clients it is enabled for
and where each secret goes; **Import** makes one commit and, in instant
mode, one apply. The result lists what was imported, what was skipped and
why, and where the secrets went; in review mode, **Open Pending changes**
applies it. **Import...** on a not-managed entry (MCP page, Tools >
Manage) opens this page with only that entry picked.

When the import moved secrets out of a file, the apply backed that file
up first, so the backup still holds the old values. The result says so:
"The previous file contents, including the old values, are kept in
backups so you can undo this import." **Remove those backups now** asks
first, then overwrites exactly those backups with zeros and deletes them
(other backups stay). Your disk, file system snapshots or backup tools
(OneDrive, Time Machine, File History) may still keep older copies. After
that, undoing the import still restores the store, but it reports the
files whose previous content can no longer be restored. On the command
line, `loadout import --remove-old-backups ...` does the same in the
import run, and `loadout import --remove-old-backups=<APPLY-ID>` does it
later (the import prints the apply id).

In review mode, or when an entry is left to decide, the import writes
nothing yet: the result says the file still holds the old values. The
apply that rewrites it later backs it up again, and that backup is not
marked for removal: delete that apply's folder under `backups/files` in
Loadout's data folder, or let retention remove it (50 runs or 30 days).

After the file watcher, window focus or **Refresh** find something new, a
one-line notice at the bottom of the sidebar counts it, for example
"7 not managed" (MCP entries plus skill folders). Click it, or press
Enter or Space, to open the message, such as "omp has 2 MCP servers not
managed by Loadout. 5 skill folders are not managed by Loadout.", with
**Review** and **Dismiss** above the line; Escape or a click elsewhere
closes it. With the sidebar collapsed, only the count shows; a click on
it reviews. Focus and watcher events rescan at most once a minute;
**Refresh** always does. **Review** opens the Import page; **Review** and
**Dismiss** both mark what was shown as seen on this machine, so it does
not come back (opening and closing the message marks nothing). Turn the
notice off in Settings, Preferences, **Not-managed notice**.

## The command line

```
loadout [--home <PATH>] [--json] [--yes] <COMMAND>
```

- `--home <PATH>`: data directory to use instead of `LOADOUT_HOME` or the
  per-OS default.
- `--json`: machine-readable JSON on stdout.
- `--yes`: answer yes to confirmations.

`loadout --help` and `loadout <command> --help` describe every option; the
[CLI reference](cli.md) has all of them on one page.
Commands:

| Command | What it does |
|---|---|
| `doctor [--paths] [--scan-history]` | Check the installation: data directory, sync-folder warning, optional secret scan of every store commit |
| `server ls\|show\|add\|rm` | Manage MCP servers in the store |
| `skills ls\|add\|rm\|enable\|disable\|update\|scan\|import\|adopt\|trash\|vendor` | Install skills, enable them for clients and projects, update, scan and adopt client folders |
| `project init\|add\|discover\|ls\|rm\|trust\|share` | Project manifests, and the folders your clients already know |
| `status [PATH] [--client C] [--scope system\|project\|all] [--check]` | State of every owned and desired entry |
| `diff [PATH] [--client C] [--exit-code]`, `diff --rev A..B [--raw]` | The apply plan as file diffs, or a store diff between revisions |
| `effective [PATH] [--client C]` | What each client loads in a folder, and from where |
| `apply [PATH] [--client C] [--scope ...] [--dry-run] [--on-conflict ask\|skip\|overwrite\|import]` | Reconcile client files with the store (one commit per run) |
| `rollback <APPLY-ID\|last>` | Roll back an apply run |
| `history [--entity E] [--limit N]`, `show <REV> [--raw]` | Store commits and what they changed |
| `tag [NAME] [REV] [-m MSG]`, `tag --list`, `tag --delete NAME` | Store tags |
| `restore <REV\|TAG> [--path ENTITY]... [--apply]` | Restore the store from an earlier state, as a new commit |
| `restore --files <APPLY-ID> [--file PATH]` | Restore client files from the backups of an apply run |
| `restore --from <DIR\|ZIP\|BARE-REPO> [--machine ID\|LABEL]` | Set up this machine from a folder remote, zip or repository |
| `backup [-m MSG] [--tag NAME] [--remote NAME\|--all\|--no-push]` | Commit hand edits of the store, optionally tag and push |
| `remote add folder\|add git\|ls\|rm\|push\|pull\|machines\|token\|create-github` | Backup and sync remotes (none by default) |
| `import [NAME]... [--scan] [--client C] [--scope ...] [--project ID] [--only] [--pick\|--as NAME=CLIENT] [--skip NAME] [--extract NAME[:FIELD]=DEST] [--remove-old-backups[=APPLY-ID]]` | Import client entries Loadout does not manage into the store, extracting literal secrets |
| `adopt [CLIENT/NAME]... [--all-identical] [--theirs]` | Take ownership of foreign or changed client entries |
| `migrate skilldock [--apply]` | Migrate from SkillDock |
| `secrets ls\|check\|providers\|set\|clear\|map-op-env\|op-account\|service-account` | Secret aliases, providers and their status |
| `env --export [--shell pwsh\|bash\|zsh\|fish] [--aliases A,B]` | Print export statements for secret aliases |
| `exec [--ref NAME=REF]... [--op-env ID --map KEY=NAME...] -- <cmd> [args]` | Start an MCP server with its secrets (used inside client configs) |
| `secret get`, `secret-header` | Internal, used by client command hooks |

`PATH` arguments take any folder inside a project. Without one, `status`,
`diff` and `apply` cover the user scope and every project this machine
knows.

Exit codes:

| Code | Meaning |
|---|---|
| 0 | OK |
| 1 | Error (or a client file could not be written; since 0.4.0 a read-only file is blocked instead and does not change the exit code) |
| 2 | Usage error |
| 3 | Pending changes or drift (`status --check`, `diff --exit-code`) |
| 4 | Entries need a decision (conflicts, collisions, drift) |
| 5 | Blocked by missing secrets |
| 6 | Timed out waiting for the store lock |
| 7 | A project manifest needs `loadout project trust` |

Commands that edit the store and apply at once (`server add`, `server rm`,
`secrets set`, `secrets map-op-env`, `secrets op-account`, `project init`,
`project rm`, `project trust`, `project share`) commit the edit and then
exit the way `loadout apply` would. `loadout exec` has its own codes: 77
when a secret is missing, 78 for a configuration error.

## Servers

A server is one file, `store/servers/<id>.toml`. Ids are 1 to 64
characters from `a-z`, `0-9`, `.`, `_` and `-`.

Add a stdio server and enable it for two clients:

```
loadout server add github --client claude-code --client codex --env 'GITHUB_PERSONAL_ACCESS_TOKEN=${secret:GITHUB_PAT}' -- npx -y @modelcontextprotocol/server-github
```

Add a remote server for every detected client:

```
loadout server add linear --url https://mcp.linear.app/mcp --header 'Authorization=Bearer ${secret:LINEAR_TOKEN}' --all-detected
```

Use single quotes around values holding `${...}` so your shell does not
expand them. Other options: `--transport stdio|http|sse`, `--cwd`,
`--display-name`, `--description`, `--tag`.

`loadout server ls` lists servers, `loadout server show <ID>` prints one,
and `loadout server rm <ID>...` removes servers (and the client entries
Loadout wrote for them) in one commit.

In the app, the MCP page's **Add** and **Paste config** buttons, and the
Install page's MCP tab, do the same; the edit dialog also sets the
environment, headers, enabled clients, secret delivery and allowed or
denied tools. The TOML tab edits the file directly; the format is in SPEC
section 5.2. Values in `env`, `headers`, `args`, `url` and `command` may
hold references:

| Reference | Meaning |
|---|---|
| `${secret:ALIAS}` | A secret alias from `secrets.toml` |
| `${env:NAME}` | An environment variable |
| `${home}`, `${project}`, `${loadout_home}` | Path variables, filled in at apply time |
| `$${` | A literal `${` |

On Windows, clients that start servers without a shell (Claude Desktop)
get `npx` and other `.cmd` launchers wrapped as `cmd /c npx ...`
automatically; the store keeps the portable `npx`.

## Clients and apply

Loadout supports 38 clients; [adapters.md](adapters.md) lists each one's
files, capabilities and verification status. A client is **detected** when
its config folder exists or its program is on `PATH`; detection only
reads. Client preferences live in `store/clients.toml` (for example
`enabled = false` to keep Loadout away from a client, omp `profiles`, or
the VS Code variants and profiles to import from).

Loadout owns only the entries it wrote (or that you adopted). It never
edits other entries, comments or formatting in a client file. Each entry
has a state: in sync, to create, to update, to remove, changed outside
(drift), missing, conflict, not managed (foreign), collision or blocked.

- `loadout status` shows the states; `loadout diff` shows the file diffs
  the next apply would make; `loadout apply --dry-run` prints the plan.
- `loadout apply` writes every pending change. Each run is one commit, and
  every file is backed up first.
- When a client file changed outside Loadout, apply asks (or follows
  `--on-conflict skip|overwrite|import`). In the app, **Resolve...** opens
  the resolver with Last written, Current file and Desired columns and the
  actions Overwrite, Import theirs into store, Release ownership, Skip,
  Re-create and Respect deletion.
- `loadout adopt CLIENT/NAME` takes over an entry that is already there;
  `--all-identical` adopts every entry identical to a store server, and
  `--theirs` takes the client's version into the store instead.
- `loadout rollback last` (or the **Undo this apply** action) undoes an
  apply run.

**Instant or review.** By default every edit applies at once. To collect
changes and apply them yourself, set `apply_mode = "review"` in
`store/settings.toml` and run `loadout backup` to commit that edit. Changes
then wait in Pending changes (or for `loadout apply`).

**Discard pending** (review mode, in the Pending changes drawer) throws
away store edits this machine has not applied yet: server definitions and
client preferences (`clients.toml`) go back to the store revision of this
machine's last apply. **Discard** does it for one item; **Discard
pending** for all of them, after a confirmation. No client file is
touched. It is one commit with an `auto/pre-restore/...` tag, so History
can bring the discarded version back. Settings, secrets, skills, project
manifests and this machine's known projects are not changed by it.

### Read-only files and folders

A client file, skill folder or project folder can be readable but not
writable for your account, for example a project managed by a separate,
security-focused installer, a file marked read only, or a read-only
network share. Loadout shows these, says why, and never plans a write
into them; the idea is in
[concepts](concepts.md#read-only-files-and-folders).

On the command line, `loadout status`, `loadout apply --dry-run`, the
`loadout apply` report, `loadout effective` (after the winning file) and
`loadout import --scan` print `read-only (<kind>)` after the file, where
the kind is `acl`, `attribute`, `owner` or `share`. With `--json`, each
target, apply result, import source and effective entry has
`"read_only": {"kind": "..."}` (or `null`). The entries Loadout would write
there are **blocked** with `read_only_target`; the rest of the run
applies, and the exit code does not count them.

In the app, a lock and **Read only** mark the file, folder or project, and
the tooltip says why in plain words:

| Kind | The app says |
|---|---|
| `acl` | its permissions do not let your account write it |
| `attribute` | it is marked read only |
| `owner` | another user owns it and your account has no write permission |
| `share` | the network share or drive it is on does not allow writing |

Controls that would write such a file are turned off (never hidden), with
the reason next to them; viewing, diffs, copying a definition and
importing it into the store stay available:

- **Projects**: a read-only project shows the badge and a note, and
  **Apply** is off; a partly read-only project lists its read-only files.
  The Scope switcher shows the lock next to the project.
- **MCP**: a client pill whose file is read only shows the lock and a
  dashed border and cannot be switched. **Enable all**, **Disable all**
  and the power button still send every client; the plan and the result
  report the read-only one as blocked. At project scope, rows whose
  manifest layer is read only cannot be edited, removed or switched, and
  **Add**, **Override here** and **Hide here** are off when no layer can be
  written.
- **Tools**: each read-only user or project file path carries the badge;
  in **Manage**, the switches that would write a read-only file are off.
- **Skills**: a read-only client skill folder shows the lock on its tab
  and pill; **Adopt**, **Adopt all identical** and **Delete** are off
  there (**Import** stays).
- **Effective config**: a cell whose winning file is read only shows the
  lock.
- **Import**: a read-only source file or not-managed group is marked; its
  definition can still be imported, but Loadout never overwrites the entry
  in that file, and **Add to project manifest** is off when that manifest
  is read only. In **Discover projects**, a read-only folder without a
  manifest cannot be added (adding would write one); one with a manifest
  can.
- **Pending changes**: a read-only file shows the badge and a blocked
  row ("Blocked: its permissions do not let your account write it.
  Loadout does not write this file; the other files are applied."). The
  apply result adds "1 file read only, not written." (or N files).

File permissions are a fact about this machine and may be temporary, so
they never change what the store records. The check never writes: it asks
the system whether your account may write the file and its folder (on
Windows by opening the file for writing without changing it), gives up
after two seconds (a stalled network share), and then counts the file as
writable. A write the system refuses anyway, because the file turned read
only after the check, is reported the same way, and the other files of
the run still apply. To have Loadout manage such a file again, make it
writable for your account (or ask whoever manages that folder). The CLI
checks on every run; the app keeps its answer for a file until the file
watcher sees a change in that folder, so when only the permissions
changed and the app still shows the lock, restart the app (tracked in
[#23](https://github.com/DailenG/agent-loadout/issues/23)).

## Projects and trust

A project is a folder with a manifest in `<P>/.loadout/`:

- `project.local.toml`: private to you. **New manifests are private by
  default**; Loadout adds `.loadout/` to the repository's
  `.git/info/exclude`, so teammates see nothing.
- `project.toml`: shared, committed with the project, for everyone on the
  team.

A manifest can add project-only servers, override a user-scope server for
the folder, or hide a user-scope server there (`enabled = false`). Loadout
writes the project servers into each client's project files (for example
`<P>/.mcp.json` for Claude Code); see [adapters.md](adapters.md) for which
clients have project and project-local files. The manifest format is in
SPEC section 5.5.

| Task | CLI | App |
|---|---|---|
| Create a manifest | `loadout project init [PATH] [--shared] [--name NAME]` | Projects, Init manifest... |
| Register a project someone else created | `loadout project add PATH` | Projects, Add folder |
| Add the folders your clients already know | `loadout project discover [--add ID]... [--all]` | Projects (or Import), Discover projects |
| Review and trust | `loadout project trust PATH` | Review change & trust |
| Move private servers into the shared manifest | `loadout project share ID [--server NAME]...` | |
| List and forget | `loadout project ls`, `loadout project rm ID` | Remove from list |
| See what each client loads | `loadout effective PATH` | Effective config |
| Add, edit or remove a project server; per-client enablement | edit the manifest, then `loadout project trust PATH` | MCP page or Tools > Manage, with the project picked in the sidebar's Scope switcher |

**Discover projects.** `loadout project discover` lists folders Claude
Code (`~/.claude.json`), Codex (folders you trusted in `config.toml`), omp
and Pi (their session history) already know, that exist and are not
projects here yet, each with an id. `--add ID` (repeatable) or `--all` adds
them in one commit. A folder without a manifest gets a private
`project.local.toml`, as Init writes it. Nothing is trusted: each project
waits for `loadout project trust` like any other. VS Code and Cursor keep
their recent folders in a database without a documented format, so they
are not read. In the app, **Discover projects** (Projects toolbar, the
empty Projects page, or the Import page) lists the same folders with the
clients that know each and its manifest, if any; check the ones you want
and choose **Add selected**. Each added project shows **Review & trust**;
a folder that cannot be added says why.

**MCP page at project scope.** Pick a project in the sidebar's Scope
switcher. The page lists the servers in that project's manifests (both
files merged; each card shows **Project** or **Local** for the file that
defines it, and whether it is a project server, an override of a store
server, or a store server hidden here) with per-client pills from the
project's plan; clients without a project config are shown as not
supported. Below them, **From your user scope** lists the store servers
the folder inherits, each with **Override here** and **Hide here**. **Add**
and the edit dialog write one `[servers.NAME]` table; **Write to** picks
**Project (shared, project.toml)** or **Local (private,
project.local.toml)** (a private project has only the local file; use
`loadout project share` to share one). Each edit is one commit: Loadout
writes the manifest, trusts that content, and applies the project's files.
A project that is not trusted, or whose manifest changed since it was
trusted, is read only on this page until you choose **Review & trust**.

**Trust.** Project manifests can start programs through stdio servers, so
Loadout applies a manifest only after you trusted its exact content. A
manifest that is new, or that changed since you trusted it (for example
after a `git pull`), is not applied: commands exit with code 7 and the app
shows "Review change & trust" with the diff. `loadout project trust`
shows the manifests (or their diff) and trusts them.

Clients have their own trust gates too: Claude Code asks before using
servers from `.mcp.json`, Codex, Gemini CLI and others load project servers
only in trusted folders, and Cursor lists project servers as disabled
until you turn each on once in Cursor (Customize > MCPs). Loadout reports
these as hints; it cannot change them for you.

## Secrets

Servers refer to secrets by alias (`${secret:GITHUB_PAT}`); the alias lives
in `store/secrets.toml` and points at a provider reference:

| Reference | Provider |
|---|---|
| `env:NAME` | An environment variable |
| `keyring://<service>/<account>` | The OS keyring: Windows Credential Manager, macOS Keychain, Secret Service on Linux |
| `op://vault/item/field` | A 1Password item (through the 1Password CLI, `op`) |
| `op-env://<environment-id>/<NAME>` | A variable of a 1Password Environment (needs `op` 2.41.0 or newer) |

Set an alias:

```
loadout secrets set GITHUB_PAT --ref 'op://Developer/GitHub PAT/credential'
loadout secrets set HASS_TOKEN --ref env:HASS_TOKEN
```

Store a value in the OS keyring by piping it on stdin (it becomes
`keyring://loadout/<ALIAS>`). Do not put the value on the command line,
where shell history keeps it:

```
# bash or zsh
read -rs VALUE && printf %s "$VALUE" | loadout secrets set OPENAI_API_KEY --keyring; unset VALUE
```

```
# PowerShell 7
Read-Host -MaskInput | loadout secrets set OPENAI_API_KEY --keyring
```

`--this-machine` writes the reference to this machine only, so one alias
can come from 1Password on one machine and from the keyring on another.
`loadout secrets clear ALIAS` deletes the keyring entry on this machine
(the reference stays).

Check them:

- `loadout secrets ls`: aliases, this machine's reference and the servers
  using them.
- `loadout secrets providers`: which providers this machine has.
- `loadout secrets check`: resolves every alias and reports its status
  (resolved, missing, locked, provider unavailable, too old, unverifiable),
  never the value; exits 5 when one cannot be resolved. It may prompt for
  1Password approval.

In the app: Settings, **Secrets** (providers, aliases, **Check all**, the
Secrets checklist, **Map a 1Password Environment...**, and the 1Password
service account token).

**How a secret reaches the server** (the server's delivery mode, `auto` by
default, SPEC 11.4):

- `env`: the client expands the variable itself (for clients that support
  references in that field). The value is never written by Loadout.
- `launch`: the client starts `loadout exec ...`, which fetches the values
  and starts the real server with them. Values exist only in memory. This
  is the default for keyring and 1Password references on stdio servers.
- `command`: the client runs a Loadout command at connect time (omp and Pi
  `!command` values, Claude Code `headersHelper` for HTTP headers).
- `inline`: Loadout writes the value into the client file. Only when you
  opt in for a server, and never into shared project files.

A server whose secret cannot be resolved on this machine is **blocked** for
the clients that need it (exit code 5) instead of being written with a
missing value.

**`loadout exec`.** Launch entries look like
`<abs path>/loadout exec --ref NAME=op://... -- npx -y <server>`. It never
writes to stdout (that is the MCP channel), reports errors on stderr with
the prefix `loadout-exec:`, and does not read the store. In user and
private files Loadout writes the absolute path of the `loadout` binary, so
another program named `loadout` on `PATH` cannot take its place (see
[Another program named loadout](troubleshooting.md#another-program-named-loadout)).

**1Password.** Install the 1Password CLI (`winget install
AgileBits.1Password.CLI` on Windows, `yay -S 1password-cli` on Arch,
1Password's packages elsewhere), then turn on the desktop app's CLI
integration (1Password, Settings, Developer). Then:

- `loadout secrets map-op-env <ENV-ID> --all` maps every variable of a
  1Password Environment to aliases (`--name NAME` picks some; `op` below
  2.42.0 cannot list names, so name them).
- With several accounts signed in to the desktop app, set the one to use:
  `loadout secrets op-account example.1password.com` (`--clear` removes
  it).
- To avoid approval prompts when clients start servers, store a service
  account token: pipe it to `loadout secrets service-account set`. It is
  kept in the OS keyring and passed only to `op`, never to the server. The
  tradeoff is a long-lived token at rest in the keyring.

On a machine without the 1Password CLI, 1Password aliases are "provider
unavailable" and their servers are blocked on that machine only; map those
aliases to the keyring or an environment variable with
`loadout secrets set ALIAS --ref ... --this-machine`.

**Environment variables for GUI clients.** `loadout env --export` prints
export statements (`--shell pwsh|bash|zsh|fish`, `--aliases A,B`) for
clients that read secrets from their environment.

## History, restore and remotes

Every change to the store is a commit: server edits, project changes,
secrets references, skills and each apply run.

- `loadout history [--entity server/github] [--limit N]` lists commits;
  `loadout show <REV>` shows one (`--raw` for the git diff).
- `loadout diff --rev A..B` compares two revisions.
- `loadout tag NAME [REV] [-m MSG]`, `loadout tag --list`,
  `loadout tag --delete NAME`.
- `loadout restore <REV|TAG>` restores the store as a new commit (your
  previous state is tagged first); `--path server/<id>` (repeatable)
  restores only some entities; `--apply` applies the result.
- `loadout restore --files <APPLY-ID>` puts client files back byte for byte
  from the backups taken before that apply (`--file PATH` for one file).
  A backup removed because it held old secret values is reported as
  `backup_removed`; the run's other files are still restored.
- `loadout backup` commits hand edits of the store files, and pushes to
  your remotes unless `--no-push`.

In the app, the **History** page does all of this (Show diff, Undo this
apply, Restore this version, Restore entity..., Tag, Download zip, Client
file backups).

**Remotes** are optional and none is set up by default. Two kinds:

- **Folder**: a local, network (UNC) or mounted folder, a USB drive, or a
  folder a sync tool keeps.
  `loadout remote add folder NAME PATH [--mode repo-per-machine|mirror|archive]`.
  The default `repo-per-machine` keeps one subfolder per machine, which is
  safe inside sync tools.
- **Git**: any git URL, through your system git.
  `loadout remote add git NAME URL [--auth system|gh|token]`. With
  `--auth token`, pipe the token to `loadout remote token NAME`; it is kept
  in the OS keyring. `loadout remote create-github REPO` creates a private
  GitHub repository with the GitHub CLI and adds it.

`loadout remote push` and `loadout remote pull` sync with every remote (or
one by name); pull merges other machines' changes and reports conflicts,
which you fix in the files and then pull again. `loadout remote machines
NAME` lists the machines in a folder remote. On a new machine,
`loadout restore --from <folder|zip|repo> [--machine LABEL]` sets up the
store from a backup; then run `loadout secrets check` to see which secrets
this machine still needs.

In the app: Settings, **Versioning & Backup** (Add remote, Push all, Pull
all, Restore from remote...).

## Skills

Skills live in the store (`store/skills/<id>`) and are linked or copied
into each client's skill folder. Loadout distributes skills to Claude
Code, oh-my-pi, Codex, Pi, GitHub Copilot and Cursor; the folders are in
[adapters.md](adapters.md).

- Install: `loadout skills add <git URL|folder|archive> [--path DIR] [--ref REF] [--id ID]`.
  A GitHub `/tree/<ref>/<path>` URL selects one folder.
- Enable: `loadout skills enable ID [--client C]...` (default: every
  detected client); `--project PATH` enables it in a project's manifest
  instead.
- Links are the default. On Windows they are directory junctions (no
  administrator rights or Developer Mode needed); `--symlink` makes
  symbolic links instead, and `--copy` copies the files.
- `loadout skills update [ID] [--check]` updates git skills;
  `loadout skills rm ID` removes a skill and the links and copies Loadout
  made.
- `loadout skills scan` classifies each client skill folder: managed,
  unmanaged (a real folder), or mismatch (a link to something else).
  `loadout skills import CLIENT NAME` brings an unmanaged folder into the
  store; `loadout skills trash CLIENT NAME` moves one to
  `backups/trash` (never deletes it).
- `loadout skills adopt` lists the unmanaged folders of every client,
  grouped by name: the same content in several clients is one skill, and
  different contents of one name are a conflict. `--all` adopts every name
  with one content: the skill goes into the store, enabled for the clients
  where it was found, and each folder whose content equals the store copy
  is replaced by the managed link and moved to `backups/trash`. For a
  conflict, `--pick NAME=CLIENT` adopts that client's content (other
  folders are kept), and `--both NAME=CLIENT` adopts it as `NAME` and every
  other content as `NAME-<client>`. `--tool CLIENT` limits it to one
  client's folder, and `--project PATH` adopts a trusted project's folders
  into its manifest. A folder is never replaced when the store copy would
  lose something (it differs, holds a git repository, an empty folder, or
  on Linux and macOS an executable file) or when the project's repository
  tracks it; it is reported and left as it is. When Loadout's data folder
  is on another drive, the original is copied into `backups/trash` and
  checked before it is removed.
- In the app, the Import page's **Skills** section does the same for the
  scope picked in the sidebar (all clients, or one with the filter): names
  with one content are checked, and for a name with several contents you
  choose **Use one content**, **Keep each content** (the others become
  `NAME-<client>`; their `SKILL.md` keeps `name: NAME`, which some clients
  expect to match the folder) or **Skip for now**. The result lists what
  was adopted, what was kept and why, and the trash folder that holds the
  originals. On the Skills page, a client's source tab has **Adopt** on
  each real folder and **Adopt all identical**. A project must be trusted
  first: the section offers **Review & trust**.
- Skills installed by the agents CLI (`~/.agents/skills`) are listed read
  only; `loadout skills vendor NAME` copies one into the store.

Loadout never overwrites or removes a folder it does not own: a real
folder or a foreign link with the same name is reported and left alone.

## Migrating from SkillDock

Loadout imports SkillDock's servers, skills, tags, settings and backup
repository. `~/.skilldock` is only read, never changed, so you can go back.

- App: Import, **From SkillDock**, **Migrate from SkillDock...** (Settings,
  Migration, **Open Import** leads there too). The wizard shows what would
  be migrated, asks where each plaintext secret value should go (OS
  keyring, a 1Password reference, an environment variable, or Set later),
  then migrates in one commit after tagging the store.
- CLI: `loadout migrate skilldock` reports what would happen;
  `loadout migrate skilldock --apply` migrates.

No client file is written by the migration; the first apply is a separate,
previewed step. Afterwards, turn off SkillDock's MCP sync or uninstall
SkillDock: two tools writing the same client files fight each other.

## Updates

The in-app updater exists only in the Windows installer build and the
AppImage, and only in releases built with the project's updater key.

- Settings, **Updates** has a **Check for updates** switch (on by default:
  Loadout looks for a new release when it starts) and a **Check now**
  button. When a newer release is found, **Install and restart** installs
  it.
- The switch is stored per machine as `check_for_updates` in
  `state/local-settings.toml`. When it is off, Loadout never contacts the
  network for updates.

In builds without the updater, Settings, Updates says so ("In-app
updates": this build has no in-app updater).

AUR, `.deb`, `.rpm`, source and `cargo install` builds have no updater:
update them with your package manager (or rerun `cargo install
agent-loadout`). Loadout is fully usable offline; only remotes, git skill
sources and update checks use the network.

## Troubleshooting

Start with `loadout doctor`. It checks the data directory, warns when it is
inside a sync folder (`sync_root_detected`), and with `--paths` prints the
resolved folders; `--scan-history` scans every store commit for secrets
(it reports, and never rewrites history).

Known problems and their fixes (installer and PATH issues, a blank window
on Linux, another program named `loadout`, servers that do not show up,
read-only projects, 1Password prompts, the Linux keyring) are in
[troubleshooting.md](troubleshooting.md); short answers are in the
[FAQ](faq.md).

## Uninstall

Remove Loadout's entries from your clients first, if you want them gone:
`loadout server rm <id>...` removes the servers and the client entries
Loadout wrote for them, and `loadout skills rm <id>` removes the skill
links and copies it made. Entries you did not create with Loadout are left
alone.

Then remove the program:

- **Windows**: Settings, Apps, Installed apps, Loadout, Uninstall (or
  `winget uninstall --id DailenG.AgentLoadout`, or
  `choco uninstall agent-loadout` if you installed it with Chocolatey). This
  removes the install folder from your user `PATH`.
- **Arch Linux**: `sudo pacman -R agent-loadout` (or `agent-loadout-bin`).
- **Debian, Ubuntu, Fedora, openSUSE**: the package is named `agent-loadout`:
  `sudo apt remove agent-loadout` or `sudo dnf remove agent-loadout`.
- **AppImage**: delete the file.
- **cargo**: `cargo uninstall agent-loadout`.

Uninstalling the program leaves your data directory (store, history,
backups) in place, so a reinstall picks up where you left off. To remove
everything, delete the data directory listed by `loadout doctor --paths`
(see [Where Loadout keeps its data](#where-loadout-keeps-its-data)), and
delete any keyring entries named `loadout` in your OS keyring.

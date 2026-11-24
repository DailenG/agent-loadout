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
12. [Catalogs](#catalogs)
13. [The Loadout skill for agents](#the-loadout-skill-for-agents)
14. [Migrating from SkillDock](#migrating-from-skilldock)
15. [Updates](#updates)
16. [Troubleshooting](#troubleshooting)
17. [Uninstall](#uninstall)

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
  secret references (`secrets.toml`), source trust decisions
  (`sources.toml`), your catalog list (`catalogs.toml`), client
  preferences (`clients.toml`), portable settings (`settings.toml`),
  skills, and project and machine files. It never contains secret values.
- `state/`: this machine only (ownership ledger, remotes, trusted projects,
  `local-settings.toml` with the Online catalogs switch, apply records, an
  optional `scan-rules.toml`).
- `servers/<id>/`: local copies (checkouts) of MCP servers that run from
  their repository's files; this machine only, never synced.
- `backups/`: copies of client files taken before each write, and the
  trash. These can contain secrets from your client files and are never
  pushed.
- `cache/` (git clones of skill and server sources, scan results, saved
  catalog results, the bundled agent skill's copy), `imports/`, `logs/`.

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
  both ("7 managed, 3 not managed by Loadout"). A server added from a Git
  repository or folder has **Check for update** in its details, and
  **Restore copy** when its local copy is missing or out of date (see
  [Servers from a Git repository or folder in the app](#servers-from-a-git-repository-or-folder-in-the-app)).
  Long lists render only the cards near the visible area; every card
  stays in the list for search, screen readers and find-in-page.
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
- **Install**: Skill (Git or Local folder or `.zip`/`.skill` archive),
  MCP (**Form or paste**, or **From Git or folder**) and **Browse** (search
  the online catalogs; see [Catalogs](#catalogs)). Skills are fetched and
  scanned first and shown in a review (files, source, trust state and the
  built-in scan's findings) before **Install**; see
  [Servers from a Git repository or folder in the app](#servers-from-a-git-repository-or-folder-in-the-app)
  for MCP.
- **Import**: bring in what your clients already have: MCP servers,
  skill folders, project folders, and the SkillDock migration (see
  [Importing in the app](#importing-in-the-app)).
- **History**: every store commit, filtered by All, User or Projects
  (History covers every scope and does not follow the switcher), with Show
  diff, Undo this apply, Restore this version, Restore entity..., Tag and
  Download zip; a Client file backups tab; and the remotes strip with sync
  state and conflicts. It loads 200 commits at a time; **Load more**
  fetches the next 200.
- **Settings**: App Preferences (Appearance, the not-managed notice),
  Versioning & Backup (remotes), Migration (a link to the Import page),
  Secrets, Sources (see [Source trust](#source-trust)), Catalogs (see
  [Catalogs](#catalogs)), Agent integration
  (see [The Loadout skill for agents](#the-loadout-skill-for-agents)) and
  Updates (see [Updates](#updates)).
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
| `server ls\|show\|add\|rm\|update\|checkout` | Manage MCP servers in the store; `add --from <GIT-URL\|PATH>` proposes servers from a repository or folder, `update` reads a server's source again, `checkout` restores its local copy |
| `source ls\|trust\|block\|untrust` | Trusted and blocked sources of skills and MCP servers |
| `catalog ls\|add\|rm\|enable\|disable\|online\|search\|show\|refresh` | Online catalogs of MCP servers and skills (off until `catalog online on`) |
| `agent install [--client C]... [--reset]`, `agent remove [--client C]...`, `agent status`, `agent skill-path` | The read-only Loadout skill for coding agents: install and enable it per client (`--reset` puts the bundled copy back over your edits), disable or remove it, show its state |
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
| 8 | The built-in scan blocked an install, update, save or checkout (`--allow-dangerous` overrides it) |

Commands that edit the store and apply at once (`server add`, `server rm`,
`server update`, `server checkout`,
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

### Servers from a Git repository or folder

`loadout server add --from <GIT-URL|PATH>` proposes servers from a Git
repository (https, ssh, `file://` or a bare repository; `--ref` picks a
branch, tag or commit, `--path` a folder inside it) or a local folder. It
reads, in this order: `server.json` (MCP Registry: npm, PyPI, NuGet and OCI
packages become `npx`, `uvx`, `dnx` and `docker run` commands pinned to the
package version; remotes become http or sse servers), `manifest.json`
(MCPB/DXT bundles), committed client configs (`.mcp.json`, `mcp.json`,
`.vscode/mcp.json`, `.cursor/mcp.json`, `.codex/config.toml`,
`.omp/mcp.json`, `claude_desktop_config.json`) and, only when none of those
give a server, the repository itself as the server (`package.json` with a
`bin`, `pyproject.toml` scripts, a `Dockerfile`), pinned to the commit:
`npx -y github:<owner>/<repo>#<sha>`, `uvx --from git+<url>@<sha> <script>`.
Identical definitions found in several files are proposed once.

```
loadout server add --from https://github.com/acme/weather --dry-run
loadout server add --from https://github.com/acme/weather --pick weather --client claude-code
```

- Nothing runs while proposing: git clones shallow into
  `LOADOUT_HOME/cache/mcp-sources` and the clone is removed afterwards
  (leftovers of an interrupted run are removed after an hour).
  Dependency and build folders (`node_modules`, `.venv`, `venv`,
  `__pycache__`, `target`, `dist`, `build`) are not read or scanned; the
  output lists them. `--dry-run` lists the proposed servers, every launch
  line (an OS-specific command too), what they need and the scan findings,
  and saves nothing.
- With one proposed server it is saved; with several, choose with `--pick
  <KEY|ID>` (repeatable) or `--all`. A positional `ID` renames the one
  server saved; `--client`, `--all-detected`, `--display-name`,
  `--description` and `--tag` apply to what is saved.
- **Secrets are never copied.** A secret the file declares becomes a
  `${secret:ALIAS}` reference; a literal that looks like a secret in a
  committed config (or in a client-specific setting such as omp's `auth`)
  is replaced by a reference or left out, and its value is dropped. Set
  the secrets afterwards (`loadout secrets set ALIAS`).
- A registry package without a version is marked **Not pinned**: each start
  can run different code. A package from another registry runs from that
  registry (`--registry=...`, `--index-url=...`).
- A server that runs code from the repository (the repository as a
  package, a bundle's own files, a committed config that starts a file of
  the repository) runs that code every time a client starts it. When it
  runs from the repository's files, the clone is kept at
  `LOADOUT_HOME/servers/<id>` on this machine only (never synced) and the
  command names it as `${loadout_home}/servers/<id>`. Its client entries
  are blocked while that copy is missing or at another commit (after
  another machine updated the server, for example): run `loadout server
  checkout <ID>`, which asks the same trust question and runs the same scan
  as adding it (`--allow-dangerous` for a server you saved anyway).
  `server rm` removes the copy.
- A server added from a folder belongs to this machine: on your other
  machines its entries are blocked ("added from a folder on another
  machine"). Setup steps (`npm install`, `docker build`) are printed with
  the real paths; run them yourself. A Docker server's image is named
  `loadout-local/<id>:...` and never pulled: build it first.
- Saving goes through the source trust question (see [Source
  trust](#source-trust); a local folder is asked about too) and the
  built-in scan: a dangerous verdict saves nothing (exit code 8) unless
  you pass `--allow-dangerous`. Without a terminal, pass
  `--trust-source[=once|repo|owner|host]`.
- The server file records where it came from (`[meta.source]`: URL or
  folder, folder inside the repository, ref, commit, file and converter).
  `loadout server update [ID] --check` reads the source again and lists
  the fields that would change (and the changed files of a checkout);
  `loadout server update [ID]` applies it, keeping the server's id, your
  values for environment variables and headers that still exist, and
  your secret names. A checkout is replaced only once the update is
  committed; run its setup steps again afterwards.

### Servers from a Git repository or folder in the app

**Install > MCP > From Git or folder** proposes servers from a Git
repository (URL, with an optional folder in it and a branch, tag or
commit) or a folder you pick. Loadout reads `server.json` (MCP Registry),
`manifest.json` (MCPB/DXT bundles), committed client configs (`.mcp.json`,
`.vscode/mcp.json`, `.cursor/mcp.json`, ...) and, when none of those
exist, `package.json`, `pyproject.toml` or a `Dockerfile` (the repository
is the server). **Find servers** fetches and scans; nothing runs and
nothing is saved yet.

- Each proposed server shows where it came from (file, entry, converter),
  every command line it launches with (the base one and each
  operating-system override), **Not pinned** when the command names no
  version or commit, what it needs (secrets to set after saving, values the
  file held that Loadout did not keep, setup steps such as `docker build`
  that you run yourself) and its scan findings. Dependency and build folders
  (`node_modules`, `dist`, ...) are not read or scanned; the review names
  them.
- A server that runs code from the repository says so: "This server runs
  code from this repository every time a client starts it." Its command is
  pinned to the reviewed commit.
- Tick the servers to add. **Review and edit** opens one in the usual
  server form (ID, command, environment, clients, ...). **Enable for**
  picks the clients of every proposed server at once.
- **Save** asks the source's trust question first when needed (below),
  then the scan gate: a dangerous verdict saves only through **Save
  anyway**, after you tick that you read the findings. **Discard** drops
  the review and its temporary clone.
- A saved server's details on the MCP page have **Check for update**. An
  update opens a review with each field that changes (now and after), every
  new command line, the setup steps (and a reminder to run them again when
  the update replaces the local copy), changed files of its checkout
  (scripts marked), the trust state and the scan, then **Update** (or
  **Update anyway**).
- A server that runs from a copy of its repository is not given to clients
  while that copy is missing or out of date on this machine. **Restore
  copy** fetches and scans the copy the server file names and shows it in a
  review: **Restore** asks the trust question when needed, and a dangerous
  verdict restores only through **Restore anyway**.

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
watcher sees a change in that folder. When only the permissions changed
and the app still shows the lock (or no longer should), click
**Refresh** on any page: the app forgets its cached answers and checks
every file and folder again. Switching back to the app after five
minutes or more does the same.

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
  A GitHub `/tree/<ref>/<path>` URL selects one folder. The first install
  from a git source asks whether you trust it (see
  [Source trust](#source-trust)).
- Enable: `loadout skills enable ID [--client C]...` (default: every
  detected client); `--project PATH` enables it in a project's manifest
  instead.
- Links are the default. On Windows they are directory junctions (no
  administrator rights or Developer Mode needed); `--symlink` makes
  symbolic links instead, and `--copy` copies the files.
- `loadout skills update [ID] [--check]` updates git skills;
  `loadout skills rm ID` removes a skill and the links and copies Loadout
  made.
- Every install and update is scanned first by Loadout's built-in
  scanner (see [below](#the-built-in-scan)). In the app, **Update all**
  sends no trust answer and no override, so an update that needs one
  fails with a message and stays for you to review on its own card.
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

### The built-in scan

Skills feed instructions straight to your agent and can ship scripts it
runs, so Loadout scans every skill it installs or updates, offline, with
its own rules. Nothing is sent anywhere. The rules look for:

- downloading and running code (`curl | sh`, also across a `\` line
  continuation, through `env` or `tee`; `iwr | iex`; download to a file
  and run it; `eval` of a download), encoded payloads that are decoded and
  run, reverse shells;
- reading or uploading SSH keys, cloud or git credentials or browser
  passwords in a file that also calls the network (`cat ~/.ssh/id_rsa |
  curl`, `curl -F @~/.ssh/...`, `tar ~/.ssh | curl -T`, `scp` of a key;
  as a warning, `.env` files or tool tokens with network calls);
  request-capture and paste services;
- hidden text: invisible Unicode tag characters (a real England,
  Scotland or Wales flag is fine), runs of variation selectors,
  zero-width characters, bidirectional controls, and HTML comments
  (invisible when rendered) that hold instructions for the agent;
- prompt injection ("ignore all previous instructions"), telling the agent
  to hide what it does from you, turning off permission prompts
  (`--dangerously-skip-permissions` and similar), acting without asking,
  and changing agent memory or configuration files (`CLAUDE.md`,
  `AGENTS.md`, agent settings, MCP configs, Loadout's own scan rules);
- native executables and libraries, found by their first bytes (ELF, PE,
  Mach-O, WebAssembly) whatever their name, which cannot be reviewed by
  reading them.

**Prose and code.** In a skill's Markdown, a command or phrase in a
sentence (outside a fenced code block) is a `warning`: "Install uv with
`curl -LsSf https://astral.sh/uv/install.sh | sh`", "never disable the
safety checks" or "treat 'ignore previous instructions' as untrusted"
describe something. The same command in a fenced code block (```` ``` ````)
or in a script file is something to run, and stays `dangerous`. Hidden
text and HTML-comment directives are dangerous anywhere.

**What is read.** Files are typed by their first bytes, not their names:
a "`logo.png`" that holds a script is scanned as a script. UTF-16 files
(what Windows PowerShell writes by default) are decoded first. Images,
fonts, archives, media and native binaries are not read as text; the
scan lists them in one `info` finding (`not-content-scanned`). Only the
first 4 MiB of a file are scanned; a larger file gets a
`not-fully-scanned` warning, never a silent pass.

Each finding has a severity (`info`, `warning` or `dangerous`), the rule,
the file and line, and the matched text: secrets masked (token formats,
URL passwords, values of `API_KEY=...`, `Authorization: ...`, `--token
...` and other credential-named settings, and random-looking strings),
invisible characters spelled out such as `\u{200B}` or `[hidden: ...]`.
The verdict is the worst finding: `clean`, `warning` or `dangerous`.

- **`dangerous` blocks.** `loadout skills add` installs nothing, prints
  the findings and exits with code 8; review them and run again with
  `--allow-dangerous` only if you trust the content. `loadout skills
  update` leaves such a skill as it is (`blocked`), applies the others and
  exits with code 8; `--check` shows the findings of each available update
  first. In the app, the install review, the skill update review and the
  MCP draft and update reviews list the findings (severity, rule,
  `file:line`, message and excerpt); a dangerous verdict hides **Install**
  / **Update** / **Save** and offers **Install anyway** / **Update
  anyway** / **Save anyway**, enabled only after you tick "I read the
  findings and want to go ahead anyway".
- **Warnings and info** are shown with the result and never block.
- Importing a client's unmanaged folder and vendoring an agents CLI skill
  report findings but never block: that content is already on this
  machine.
- Trusting a source never skips the scan. A clean verdict means no rule
  matched, not that the skill is safe: still read what you install.
- Results are cached by content in `cache/scan`, so unchanged content is
  not scanned again.
- The rules are data (`scan-rules.toml`, versioned). A custom rules file
  saved as `state/scan-rules.toml` in the data folder is merged in: it may
  add rules or raise a built-in rule's severity, never remove or lower
  one (such entries are skipped with a warning). The built-in rules
  always run, and the result says "custom rules in use". A file that does
  not load is reported with every scan.

### Snyk Agent Scan (optional)

Loadout can ask a second scanner for its opinion on a skill:
[Snyk Agent Scan](https://github.com/snyk/agent-scan). It is **off by
default** and only scans skills (never MCP servers). Unlike the built-in
scan, it doesn't run on your computer alone: **it sends the skill to
Snyk's analysis service in the cloud (`api.snyk.io`).** For each scan Snyk
receives:

- every file of the skill (Snyk redacts secrets it detects in them);
- the skill's name (from `SKILL.md`);
- the full path of the folder it scans (a temporary copy in Loadout's
  data folder);
- your user name on this computer;
- this machine's host name, which the scanner adds itself where its
  platform reports one;
- the scanner's version.

Loadout adds no host name, machine serial number, other environment
variables or Loadout data, and never starts or contacts MCP servers for a
scan. Snyk's terms and privacy policy apply to what it receives; the
section links both. It needs a Snyk API token (`SNYK_TOKEN`); the free
tier has a daily limit.

**Turning it on in the app** (Settings > Security > **Snyk Agent Scan
(optional)**):

1. **Install** downloads the scanner version pinned in Loadout from
   Snyk's GitHub releases into `tools/snyk-agent-scan/` in the data folder
   (this computer only, not added to PATH; it syncs only if the data
   folder itself is in a sync folder), with a progress bar and **Cancel**.
   It is checked against the SHA-256 built into Loadout before it counts as
   installed; a mismatch deletes it. Each scan checks it again first: a
   file that changed since (corrupted, or replaced by a sync peer) isn't
   run, the scanner shows as not installed, and **Install** downloads it
   again. The row then says which version is installed and verified. The
   download keeps going (with its progress and **Cancel**) if you leave
   Settings, and a
   second install can't start meanwhile. **Remove** deletes it after a
   confirmation (Cancel is the default) and turns Snyk Agent Scan off;
   the token reference is kept.
2. **Set token...** says where the token is: a secret from the Secrets
   page (an alias such as `SNYK_TOKEN`), an environment variable, a
   1Password reference (`op://vault/item/field`) or an OS keyring entry
   (`keyring://service/account`). Only this reference is saved, on this
   computer; Loadout never asks for, shows or stores the token itself and
   hands it only to the scanner while it runs. To keep the token in the
   keyring, add it as a secret on the Secrets page and pick that secret.
   **Check** tells you whether Loadout can read it (Snyk isn't contacted).
   Secrets that point to a 1Password Environment (`op-env://`) aren't
   offered: Loadout never reads those values, so it can't hand one to the
   scanner. While what you typed isn't a reference, the dialog says why
   Save is off. A secret alias must be one of your secrets; a pasted token
   is refused, and the error never repeats it.
3. **Use Snyk Agent Scan** turns it on. It stays unavailable until the
   scanner is installed and a token is set.

**What happens when it's on.**

- Install and update reviews of content from a source (a Git repository,
  a catalog, an update) run it once **as soon as the review opens, before
  you choose Install or Update**. A folder or archive from this computer
  is not sent on its own: its review shows **Scan with Snyk**, so you can
  look at the files first. The result is listed under **External scan**,
  beside the built-in findings: the verdict, and per finding the severity
  (Snyk's score: high and critical, or malicious code, are `dangerous`,
  medium is `warning`, low is `info`), the risk, `file:line`, Snyk's
  explanation and any malicious or unverifiable URL. **Install** /
  **Update** waits for it (up to the configured timeout, 60 seconds by
  default) and while Loadout checks whether it's on; **Cancel scan**
  stops it. If Loadout can't read the scanner's status, the review says
  "Snyk Agent Scan status unknown, not scanned" and doesn't block.
- A **dangerous** external verdict needs the same **Install anyway** /
  **Update anyway** and "I read the findings" tick as the built-in scan;
  the box says which scanner (or both) rated it dangerous. The tick covers
  the result on screen: a new result asks again. Turning the scanner off
  (or removing it) cancels scans in progress and forgets earlier Snyk
  results, so they can't block a later update. While it's on, Loadout
  won't install from a source or update without the Snyk result (a
  canceled or not-scanned result counts); the review then runs the scan
  first.
- When it couldn't scan, the review says **Not scanned** and why (Snyk
  limit reached, timed out, canceled, Snyk couldn't be reached, too large
  for Snyk, no token, the token couldn't be read or was rejected), with
  **Scan again**. Output Loadout can't read says "Scanner output not
  understood (version ...)". Neither ever blocks an install, and neither
  counts as clean: the built-in scan's findings still apply.
- A skill's details in the library offer **Scan with Snyk** (while it's on
  and installed), a report on the stored copy that changes nothing.
- The scan's start and its result are announced to screen readers, with
  what the Install or Update button now needs.
- While it's off, nothing is ever sent to Snyk.

### Snyk Agent Scan from the command line

Snyk Agent Scan is off until you turn it on, and when it runs it **sends
the skill to Snyk** (see above and [Security](security.md)).

```sh
loadout scanner install                 # download the pinned build, SHA-256 checked
loadout secrets set SNYK_TOKEN --keyring < token.txt   # the token, in your OS keyring
loadout scanner token SNYK_TOKEN        # a reference: an alias, env:NAME, keyring://... or op://...
loadout scanner enable                  # prints what is sent, then turns it on
loadout scanner status                  # version, installed, on or off, token reference
```

From then on `loadout skills add` and `loadout skills update` run it once
per skill, after the built-in scan, and print `Snyk Agent Scan: <verdict>
(N findings)` with its findings (`--json`: an `external` object). A
`dangerous` Snyk verdict blocks like a built-in one (exit 8) until you
pass `--allow-dangerous`. When it could not run it says `not scanned
(<reason>)` (no token, Snyk rejected the token, Snyk limit reached, too
large, offline, timed out, the scanner failed) and never blocks. A git
source is sent only once you trust it: an untrusted or blocked source is
refused before the scan, and `skills update --check` skips the scan for a
source you haven't trusted yet. `--external` runs it once even when it is
off; `--no-external` skips it once. Ctrl+C while it runs stops the
scanner and deletes its copy of the skill. `loadout scanner run <SKILL>`
scans a skill already in your library, `loadout scanner disable` turns it
off and `loadout scanner remove` deletes the download and turns it off.
`loadout scanner token` takes a reference only: a secret alias you
defined with `loadout secrets`, or `env:`, `keyring://` or `op://`.

### Source trust

Only install from sources you trust. Skills and MCP servers can run
commands and read your files. Loadout treats trust in a source as a
decision you make and it records, like trusting a project.

- **The first install from a git source asks** (for a skill, and for an
  MCP server added or updated from a source). A terminal prompt shows
  the source (`github.com/example-org/skills`), whether it is official
  (`github.com/anthropics`, `github.com/modelcontextprotocol`), and which
  skills you already installed from it, then offers **Trust this time
  only**, **Always trust this source**, or **Cancel**. "Always" asks what
  it covers: the repository (the default), every repository of its owner,
  or, on a host that isn't a public service such as github.com, every
  repository on the host.
- **Trust is never implicit.** Without a terminal (a script, `--json`), an
  untrusted source fails with `source_trust_required` unless you pass
  `--trust-source` (this time only) or `--trust-source=repo|owner|host`
  (always); write it with `=`. `--yes` never answers the trust prompt.
  The flag and `loadout source trust` are explicit opt-ins that need no
  terminal, so a coding agent with shell access can run them too: only
  use them, or let an agent use them, for a source you trust. Keeping
  agents from approving trust is tracked in
  [#8](https://github.com/DailenG/agent-loadout/issues/8).
- **A block wins.** A blocked source is refused (`source_blocked`) even
  when an owner or host above it is trusted, and even with
  `--trust-source`.
- **Updates still ask when something changed.** A trusted source's update
  check still shows the changed files (scripts and other executable files
  are marked), and the scan still runs. If the repository's history was
  rewritten (a force-push removed the commit you installed) or it was
  renamed or transferred (the host redirects it elsewhere), its trust
  lapses and the next update asks again. A source you never trusted asks
  on every install and update, including skills installed before 0.5.0.
  A redirect only adds a question: the CLI prints `note: X moved to Y`
  and asks about the new location separately (`--trust-source` does not
  carry over), and an update of a moved repository then records its new
  URL.
- **Unencrypted sources are separate.** An `http://` or `git://` address
  has its own key (`http://host/path`), so trusting the HTTPS or SSH
  address never admits it; a block on either does block it. The prompt
  and the dialog say the connection is unencrypted.
- **Managing trust:** `loadout source ls` lists trusted and blocked
  sources with when, where and how each decision was made;
  `loadout source trust <URL|KEY> [--scope repo|owner|host]`,
  `loadout source block <URL|KEY> [--scope ...]` and
  `loadout source untrust <KEY>` change them. Keys look like
  `github.com/acme/skills` (a repository), `github.com/acme/*` (an owner)
  or `git.example.com/*` (a host). Untrusting never uninstalls anything;
  the next install or update from that source asks again. Decisions live
  in `store/sources.toml` and sync with your remotes.
- Skill folders and archives you pick aren't asked about (they're already
  on this machine; the scan still runs). A git repository given by a local
  path or `file://` URL is asked about like any other source, and so is a
  folder you pick for an MCP server.
- **In the app** the same question is a dialog, asked when you press
  **Install**, **Update**, **Save** or **Restore** in a review: **Trust
  this time only** (selected by default; nothing is recorded), **Always
  trust** *repository*, **Always trust everything from** *owner* (or, on a
  self-hosted server, everything on it), then **Cancel** (nothing happens)
  or the confirm button (**Trust once and continue** or **Always trust and
  continue**). Focus starts on **Cancel**, so a held or doubled Enter
  grants nothing. It says why it asks (first time, not trusted, or trust
  lapsed after a force-push or move), names both addresses when the host
  redirects to another repository (asked separately), warns about an
  unencrypted connection, lists the skills you already installed from the
  source, and marks official sources without skipping the question. A
  folder picked for an MCP server is asked about at the repository scope
  only.
- **Settings > Sources** lists every decision: trusted or blocked, for a
  repository, an owner or a host, when, how (the app, a prompt,
  `--trust-source` or `loadout source`) and on which machine, with a note
  when trust lapsed. **Revoke** (or **Unblock**) removes a decision, so the
  next install or update asks again; **Block** turns a trust into a block;
  **Block a source...** blocks a URL you have no decision for yet. None of
  these uninstall anything.

## Catalogs

Catalogs are places that list MCP servers and skills. Loadout can search
them and install an entry, always through the same review, trust question
and scan as any other install: a catalog never installs anything on its own.

Built in (each can be turned off):

- **Official MCP Registry** (`registry.modelcontextprotocol.io`). Its
  results say "Data from the Official MCP Registry". The registry has no
  "verified" or "official" badge; what it does check is that whoever
  published an entry owns its namespace (a GitHub user or organization, or
  a domain), so a result shows, for example, "Publisher: io.github.acme
  (namespace ownership checked by the registry)". It doesn't prove the
  server is safe.
- **Anthropic skills** (`github.com/anthropics/skills`), a Git tap: its
  skill folders and its Claude Code plugin marketplace. It is marked
  **Official** because it is published under `github.com/anthropics`.
- **skills.sh**: a link only. skills.sh has no search Loadout can use, so
  Browse opens it in your browser; paste a skill's address
  (`https://skills.sh/<owner>/<repo>/<skill>`) to install that skill from
  its GitHub repository. In the app the paste field appears only while
  online catalogs are on, like any install from Browse; on the command
  line, `loadout skills add https://skills.sh/...` is a plain git install
  of that repository. Skills that skills.sh lists from other websites
  can't be installed this way.

You can add any server that answers the MCP Registry API, and any Git
repository of skills or Claude Code plugin marketplace (a "tap").

### Online catalogs and privacy

Searching contacts the catalogs, so it is off until you say yes. The first
time you open **Install > Browse**, Loadout asks once, naming the hosts it
would contact; nothing is requested before you answer. With online
catalogs on, Loadout contacts only the catalogs you turned on, only when you
open Browse, search, open an entry or install, and sends them only your
search words: nothing runs in the background and there is no telemetry.
Installing an entry also contacts its own repository or package registry,
and installing or updating a skill or plugin server from a tap (or a
skills.sh address) is a git install that contacts the git host even with
online catalogs off. Requests go through the proxy in `HTTPS_PROXY` (or
`ALL_PROXY`), and hosts in `NO_PROXY` are reached directly; a SOCKS proxy
is not supported and is reported as an error, never bypassed.
Every request has a time limit, and each catalog is searched on its own, so
one slow or failing catalog never holds up the others.

Results are saved on this computer for 24 hours. A catalog that can't be
reached shows its saved results labeled with their age ("Stale: 4 results
saved 1 day ago") and why it couldn't refresh; **Try again** fetches it
again. With online catalogs off, Browse shows only saved results and says
so, and installing from a catalog (or from a pasted skills.sh address)
asks you to turn them on first. If this computer goes offline while Loadout
is open, Browse keeps answering from the saved results and says "Offline".
A **More results** page that fails says why, with **Try again**. Only
`https` repository and website links get an **Open** button.

### Catalogs in the app

**Install > Browse** has a search box; each catalog you turned on gets its
own section with its status (searching, up to date, saved with its age,
stale, timed out, offline, or "type at least 3 characters" for a registry).
Each result shows its kind (MCP server, skill or plugin), where it comes
from, the publisher, the version and how it is pinned ("Pinned to version
1.2.0", "Pinned to a1b2c3d", "Follows branch main" or "Not pinned"), when
it was last updated, and a **Deprecated** or **Can't be installed** mark.
Catalog text is shown as plain text, and Loadout never loads a catalog's
icons or images.

**Details** opens everything the catalog says: transports, the names of the
secrets the server needs (never values), its packages and remote endpoints,
the repository and website (**Open** opens them in your browser), the
license when known, and for a plugin, its skills, its MCP servers and the
parts Loadout doesn't import (hooks, commands, agents and other code that
would run on its own). Then:

- **Review MCP server** opens the same review as **From Git or folder**:
  drafts, the trust question, the built-in scan and the client picker. For
  a registry entry the trust question names what the server runs (its
  package on npm, PyPI, NuGet or crates.io, its container image, or a
  remote server's host), never the repository the publisher names: the
  details show that one as "Repository (claimed by the publisher, not
  verified)". The saved server records the catalog entry it came from.
- **Review skill** opens the skill install review. A plugin with several
  skills asks which one first.
- A plugin from an npm package, an archive or a command can't be installed,
  and the details say so.

**Official** appears only on taps under `github.com/anthropics` (such as
Anthropic skills) and on their entries whose code is Anthropic's own: skill
folders and plugins that live in the tap or in another `github.com/anthropics` repository. A third-party
plugin that the Anthropic marketplace lists is not marked Official, and a
marketplace elsewhere that uses a name reserved for Anthropic is flagged
with a warning. Registry entries are never marked Official.

**Settings > Catalogs** has the **Online catalogs** switch (this computer
only) and the list of catalogs: turn each on or off, **Add catalog...**
(an MCP registry address, or a Git tap with an optional branch, tag or
commit) and **Remove** for the ones you added. The list is part of your
store, so it syncs with your other machines; nothing there contacts a
catalog.

### Catalogs from the command line

```
loadout catalog ls                         # catalogs and the online switch
loadout catalog online on                  # once, to allow catalog requests (off: `online off`)
loadout catalog search weather             # every catalog that is on; --catalog <id> for one
loadout catalog show mcp-registry/io.github.acme/weather
loadout server add --from catalog:mcp-registry/io.github.acme/weather
loadout skills add catalog:anthropic-skills/pdf
loadout skills add https://skills.sh/anthropics/skills/pdf
loadout catalog refresh                    # drop saved results and fetch again
```

- `loadout catalog add <url>` adds an MCP registry or a Git tap (`--kind
  registry|tap`; GitHub, GitLab, Bitbucket, SSH, `file://` and `.git`
  addresses are taps). `--id`, `--title` and, for a tap, `--ref <branch,
  tag or full 40-character commit>` (a commit pins it; a short commit is
  refused) are optional. A registry address must use https (http only for
  `localhost`), and a tap address may not hold a user name, password or
  token (it would be saved in your synced store; use your git credential
  helper). `catalog rm <id>` removes one you
  added; `catalog enable|disable <id>` turns any catalog on or off,
  built-ins included. These change `store/catalogs.toml` (one commit each).
- `catalog search <query>` needs at least 3 characters for a registry; a
  tap lists everything for an empty query. Each catalog reports its own
  status (`fresh`, `cached` with its age, `stale` when it could not be
  reached, `off`, `error`). A registry's next page:
  `--catalog <id> --cursor <next_cursor>`. The command exits 1 when no
  catalog answered.
- Entry ids: a registry server's name (`io.github.acme/weather`), a tap
  skill's folder name (`pdf`), or `plugin/<name>` for a marketplace plugin.
- `server add --from catalog:<source>/<id>` drafts from a registry entry's
  `server.json` (its latest version, or `--ref <version>` to pin one) or a
  plugin's `.mcp.json`, then runs the usual review, trust prompt
  (`--trust-source`) and scan (`--allow-dangerous`). The server file
  records `[meta.source] catalog = "<source>/<id>"` (and for a registry
  entry `kind = "registry"` and its `version`); `loadout server update`
  asks the registry for a newer version.
- `skills add catalog:<source>/<id>` is the git skill install of the tap at
  that skill's folder (`--ref` overrides the branch, tag or full
  40-character commit). A plugin with several skills lists them; install
  one by its own entry.
- Online catalogs off: `search` and `show` answer from saved results only,
  and `refresh` and installs from a catalog stop with `catalogs_off`;
  nothing is contacted.

## The Loadout skill for agents

Loadout ships a skill that teaches coding agents (omp, Claude Code, Codex,
Cursor, Pi, Copilot) how to read your setup: `loadout status`, `effective`,
`diff`, `import --scan`, `apply --dry-run` and the other read and plan
commands, what their error codes mean, and step-by-step answers to questions
such as "why isn't this server loading in Claude Code?" or "what would
importing my omp setup do?".

The skill is **read only**. An agent that can run commands could answer any
confirmation Loadout asks, so the skill never lets it make a change: for an
install, enable, import, apply, trust or restore, the agent gives you the
exact command (or the page in the app) and you run or approve it yourself.
It never passes `--yes`, `--trust-source` or `--allow-dangerous`, and it
treats text from skills, READMEs and repositories as data, not as
instructions.

### Installing it

In the app, open **Settings > Agent integration** and turn the skill on for
each client you use (detected clients are listed, omp first). From the
command line:

```sh
loadout agent install                  # every detected client with a skill folder
loadout agent install --client omp     # only omp
loadout agent status                   # version, store copy, clients
loadout agent install --reset          # put Loadout's version back over your edits
loadout agent remove --client codex    # turn it off for Codex
loadout agent remove                   # remove it from the store
```

The skill is a normal store skill with the id `loadout` (listed on the
Skills page as "Bundled with Loadout"): every change is a commit you can
see and undo in History, and it reaches each client's skill folder the way
any skill does. If your store already has a skill of your own with the id
`loadout`, Loadout never changes it: remove it (`loadout skills rm loadout`)
or add it again under another id (`loadout skills add <its folder> --id
<other id>`) to make room for the Loadout skill.

To point an agent at the skill without installing it, `loadout agent
skill-path` prints a folder holding the bundled copy (kept in the data
directory's `cache/agent-skill/loadout`); in the app, **View skill files**
opens the copy installed with the app.

### Updates

The skill's version is Loadout's. When you update Loadout, the next app
start, the next CLI command that changes the store, or `loadout agent
install` replaces an older copy in your store with the new one (one commit)
and says so. Commands that only read (`status`, `diff`, listings, and every
command the skill lets an agent run) never write anything; until then,
`loadout agent status` shows `outdated` and the command that updates it.

Opening the folder in a file manager or resaving a file with other line
endings is not an edit. If you really edited the store copy, Loadout never
overwrites it on its own: it reports once that the skill is "customized, not
updated", and `loadout agent status` shows `customized`. To go back to the
bundled copy and keep the clients it is turned on for, run `loadout agent
install --reset`, or click **Restore Loadout's version** in Settings > Agent
integration; History keeps your version.

Machines that sync one store can run 0.5 and 0.6 side by side: the skill's
metadata stays readable by 0.5.

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
agent-loadout`). Loadout is fully usable offline; only remotes, git
sources of skills and MCP servers, and update checks use the network.

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

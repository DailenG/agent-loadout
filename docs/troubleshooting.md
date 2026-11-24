# Troubleshooting

Start with `loadout doctor`. It checks the data directory, warns when it is
inside a sync folder (`sync_root_detected`), and with `--paths` prints the
resolved folders; `--scan-history` scans every store commit for secrets (it
reports, and never rewrites history).

For questions about how Loadout is meant to behave, see the [FAQ](faq.md).

## Installing

### Windows setup sits on "Create shortcut" for minutes (0.1.2)

The 0.1.2 installer told running programs about the `PATH` change one
window at a time and waited 5 seconds for each window that did not answer.
Fixed in 0.1.3, which skips hung windows and waits at most 1 second per
window. Install 0.1.3 or later.

### `loadout` fails to start on Windows with `STATUS_DLL_NOT_FOUND` (0.1.2)

The 0.1.2 CLI needed the Visual C++ Redistributable. Since 0.1.3
`loadout.exe` includes the runtime. Update to 0.1.3 or later (or install
the Visual C++ Redistributable).

### `loadout` is not found after installing on Windows

- Open a new terminal: the installer changes your user `PATH`, and
  terminals that were already open keep the old one.
- If your user `PATH` is longer than 1024 characters, the installer cannot
  read it safely, so it leaves it unchanged and says so. Add
  `%LOCALAPPDATA%\Programs\Loadout` to your user `PATH` by hand.

### Another program named loadout

Another tool also ships a binary named `loadout` (an AI-agent launcher
CLI, published on winget as `TheCodeSaiyan.Loadout` and as `.deb` and
`.rpm` packages). If both are installed, the one that comes first on
`PATH` wins when you type `loadout`.

- The launch entries Loadout writes into user-scope and private files use
  the absolute path of its own `loadout`, so clients start the right
  program. Shared project files (committed for a team) use the bare name
  `loadout` to stay portable, so on such a machine they can start the
  wrong one.
- Check which one you get: `where.exe loadout` (Windows) or
  `which -a loadout` (Linux). This project's `loadout --version` prints
  `loadout X.Y.Z`, and `loadout --help` starts with "Manage MCP servers,
  skills and secret references for AI coding clients".
- Fix it by putting Loadout's install folder first on `PATH`, or by
  uninstalling the other tool. `loadout doctor` does not check for this
  yet.

### Blank window on Linux

On some GPU and driver combinations, WebKitGTK shows an empty window. Start
the app with the DMA-BUF renderer turned off:

```
WEBKIT_DISABLE_DMABUF_RENDERER=1 agent-loadout-app
```

For the AppImage: `WEBKIT_DISABLE_DMABUF_RENDERER=1 ./agent-loadout_X.Y.Z_amd64.AppImage`.
To make it permanent for the menu entry, copy the desktop file to
`~/.local/share/applications/` and change its `Exec=` line to
`Exec=env WEBKIT_DISABLE_DMABUF_RENDERER=1 agent-loadout-app`. Loadout does
not apply this setting by itself yet.

## Servers and clients

### Manage or the MCP page shows 0 servers, but my client has some

The MCP list and the Manage switches show the servers in Loadout's store.
Servers already in a client's config that Loadout did not write are
*foreign* entries: Loadout never touches them. Since 0.3.1 they are listed
below, under **Not managed by Loadout** (per client and file, with
"N managed, M not managed by Loadout" above the list), and each Tools card
counts its client's. Choose **Import...** on an entry, or open the
**Import** page, to bring them in. On the command line:

```sh
loadout import --scan               # what is there; changes nothing
loadout import --client omp         # import one client's servers
```

If the section is empty too, check the **Scope** switcher (a project scope
lists only that project's files) and that the client is detected on the
Tools page. See [concepts: ownership](concepts.md#ownership-managed-and-foreign-entries).

### The sidebar says "N not managed"

Your clients have MCP entries or skill folders that Loadout does not
manage. Click the line to read which, then **Review** (the Import page) or
**Dismiss**; both mark what was shown as seen on this machine. Turn the
notice off in Settings, Preferences, **Not-managed notice**.

### `loadout import` skips an entry with `needs_secret_extraction`

The entry holds a literal value (in `env`, `headers`, `args` or the URL)
that looks like a secret, and Loadout never puts secret values in the
store. Tell the import where each value goes: `loadout import --scan`
lists them by field, then for example

```sh
loadout import --extract chatty=keyring                         # the OS keyring
loadout import --extract "chatty=op://Private/Chatty/credential" # a 1Password item
loadout import --extract chatty:env.CHATTY_API_KEY=env:CHATTY_KEY # an environment variable
```

The store gets a `${secret:ALIAS}` reference and the client entry is
rewritten to use it. A URL that looks like it holds a secret, with no part
Loadout can cut out, still needs a hand edit.

### `loadout import` skips an entry with `collision`

The entry differs between clients (or between your user file and a
project). Pick one definition with `--pick NAME=CLIENT`, or import both:
`--pick docs=claude-code --as docs=omp` stores `docs` and `docs-omp`.

### The MCP and Tools pages show my user servers when I expected a project

The Skills, MCP and Tools pages follow the **Scope** switcher at the top of
the sidebar. Pick the project there. The app always starts at **User**.
History and Projects cover every scope.

### A project waits for trust, or apply exits with code 7

The project's manifest is new, or changed since you trusted it (for
example after a `git pull`). Loadout applies a manifest only after you
trusted its exact content. Review and trust it:
`loadout project trust PATH`, or **Review & trust** in the app. Until then
its servers cannot be edited in the app and its targets are blocked; user
scope and other projects still apply.

### A file or project is marked read only

Your account can read the file or folder but not write it: its
permissions (`acl`), the read-only attribute (`attribute`), another
owner (`owner`), or a read-only share, mount or drive (`share`). A
project folder managed by a separate, security-focused installer is the
usual case. This is not the same as an untrusted project.

Loadout leaves such a file alone: the entries it would write there are
blocked (`read_only_target`, `read-only (<kind>)` in `loadout status`),
everything else applies, and the exit code is not affected. You can
still import a definition from it into the store. To let Loadout manage
it, make the file (or the project's `.loadout` folder) writable for your
account, or ask whoever manages that folder; otherwise there is nothing
to fix. The app keeps its answer until the file watcher sees a change in
that folder; after a permissions-only change, restart the app (tracked
in [#23](https://github.com/DailenG/agent-loadout/issues/23)). Details:
[user guide, Read-only files and folders](user-guide.md#read-only-files-and-folders).

### A server did not appear in the client

- Claude Desktop loads servers only at launch: quit it fully (also from the
  system tray or menu bar) and start it again.
- Cursor lists project servers as disabled until you turn each on once in
  Cursor (Customize > MCPs).
- Claude Code, Codex and Gemini CLI ask before using project servers, or
  load them only in trusted folders.
- A slow server never starts in omp: omp's `timeout` (milliseconds) also
  limits startup; raise or remove it.
- Run `loadout status --client <id>`: a **blocked** entry says why (for
  example `missing_secret`).

### Drift after editing a client file by hand

`loadout status` shows the entry as changed outside. Resolve it in the
Pending changes drawer (**Resolve...**) or with
`loadout apply --on-conflict overwrite|import|skip`.

### `adopt` kept some skill folders

`loadout skills adopt` and the app's **Adopt** replace a folder with
Loadout's link only when nothing would be lost. The result lists each
kept folder with its reason:

- another content of its name was chosen, or its name has several
  contents and you did not choose (`--pick NAME=CLIENT`, or `--both` to
  keep each as `NAME-<client>`);
- it holds a git repository, an empty folder, or (Linux, macOS) an
  executable file, which the store copy would not keep;
- the project's repository tracks it;
- it changed after the scan (scan again).

Kept folders stay as they are; the skill is still in the store for the
clients where it was adopted. When Loadout's data folder is on another
drive than the skill folder, the original is copied into
`backups/trash` and compared before it is removed, so adopting takes
longer there but loses nothing.

### My client file's old secret values are still in `backups/`

An import that moved literal secrets out of a client file backed the file
up first, so that backup still holds the old values. The import result
says so and lists the backups: choose **Remove those backups now** in the
app, or run `loadout import --remove-old-backups=<APPLY-ID>` (the import
printed the id). Exactly those backups are overwritten with zeros and
deleted; undoing that import then restores the store but reports the
files it can no longer put back. Copies kept by file system snapshots or
backup tools are out of Loadout's reach. In review mode the import itself
writes nothing; the apply that later rewrites the file takes a backup that
is not marked, so delete that run's folder under `backups/files` or let
retention remove it (50 runs or 30 days). See
[security: old secret values in backups](security.md#client-files).

### Exit code 6

Another Loadout process (the app or another CLI command) holds the store
lock. Wait for it or close the other instance.

### Long paths on Windows

Loadout handles client files and backups past the 260-character `MAX_PATH`
limit itself, so applies and `restore --files` work in deep folders. Some
clients and tools may not; keep project folders reasonably shallow if a
client fails to read its files.

## Secrets

### 1Password asks for approval every time a client starts a server

With `launch` delivery, `loadout exec` calls `op` each time a client starts
the server, and the 1Password app asks to approve the CLI. To avoid the
prompts, store a 1Password service account token: pipe it to
`loadout secrets service-account set` (Settings, Secrets in the app). It is
kept in the OS keyring and passed only to `op`. The tradeoff is a
long-lived token at rest in the keyring, limited to the vaults you grant
the service account.

### `multiple accounts found` from 1Password

Several accounts are signed in to the 1Password app. Set the one to use:
`loadout secrets op-account ACCOUNT` (for example
`example.1password.com`).

### Servers are blocked on one machine only

That machine cannot resolve an alias (for example no 1Password CLI, or no
keyring entry). `loadout secrets check` shows the status per alias. Point
the alias at something available there:
`loadout secrets set ALIAS --ref ... --this-machine`.

### Secrets on Linux

The OS keyring on Linux is the Secret Service, provided by gnome-keyring,
KWallet or KeePassXC (with its Secret Service integration turned on).
Without one, keyring aliases report `provider_unavailable`. When the
keyring collection is locked and no unlock prompt can appear (SSH, WSL, a
service), Loadout reports `locked` instead of waiting; unlock the keyring
in your desktop session and try again. Loadout never asks for an unlock
itself.

## Data directory

### `sync_root_detected`

The data directory is inside a file-sync folder (OneDrive, Dropbox, Google
Drive, Syncthing, Resilio Sync). Sync tools corrupt live git repositories.
Move it (set `LOADOUT_HOME`) and use a remote to share the store; a folder
remote inside a synced folder is fine.

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

Loadout lists only servers in its own store. Servers already in a client's
config that Loadout did not write are *foreign* entries: Loadout never
touches them, and the app does not show them yet
([#3](https://github.com/DailenG/agent-loadout/issues/3)). Import them on
the command line:

```sh
loadout import --scan               # what is there; changes nothing
loadout import --client omp         # import one client's servers
```

See [concepts: ownership](concepts.md#ownership-managed-and-foreign-entries).

### `loadout import` skips an entry with `needs_secret_extraction`

The entry holds a literal value (in `env`, `headers`, `args` or the URL)
that looks like a secret, and Loadout never puts secret values in the
store. Move the value to a secret alias first: set it up with
`loadout secrets set ALIAS ...`, change the client entry to use an
environment variable, or add the server to Loadout by hand with
`${secret:ALIAS}` and adopt or remove the old entry.

### The MCP and Tools pages show my user servers when I expected a project

The Skills, MCP and Tools pages follow the **Scope** switcher at the top of
the sidebar. Pick the project there. The app always starts at **User**.
History and Projects cover every scope.

### A project is read only, or apply exits with code 7

The project's manifest is new, or changed since you trusted it (for
example after a `git pull`). Loadout applies a manifest only after you
trusted its exact content. Review and trust it:
`loadout project trust PATH`, or **Review & trust** in the app. Until then
its servers are read only and its targets are blocked; user scope and other
projects still apply.

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

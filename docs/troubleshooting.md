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
that folder; after a permissions-only change, click **Refresh** (any
page's Refresh button), or switch back to the app after five minutes or
more, and it checks the permissions again. Details:
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
  example `missing_secret`, `checkout_out_of_date` or
  `folder_on_other_machine`, below).

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
`loadout secrets set ALIAS --ref ... --this-machine`. A server added from
a Git repository or folder can also be blocked on one machine only; see
[Sources and the scan](#sources-and-the-scan).

### Secrets on Linux

The OS keyring on Linux is the Secret Service, provided by gnome-keyring,
KWallet or KeePassXC (with its Secret Service integration turned on).
Without one, keyring aliases report `provider_unavailable`. When the
keyring collection is locked and no unlock prompt can appear (SSH, WSL, a
service), Loadout reports `locked` instead of waiting; unlock the keyring
in your desktop session and try again. Loadout never asks for an unlock
itself.

## Sources and the scan

### `source_trust_required`

You have not trusted the source, and nothing answered the trust question.
Nothing was fetched into the store or installed. In a terminal, Loadout
prompts (`[o]nce / [a]lways / [c]ancel`); in a script, an agent session or
with `--json` there is no prompt, so the command fails. Decide first:

- run the command again in a terminal and answer the prompt;
- pass `--trust-source` (this time only) or `--trust-source=repo`,
  `=owner` or `=host` (always; write it with `=`) to `skills add`,
  `skills update`, `server add --from`, `server update` or
  `server checkout`;
- or record the decision first: `loadout source trust <URL>
  [--scope repo|owner|host]`.

`--yes` never answers it. When the host redirected the repository (renamed
or transferred), the new location is a separate question: the CLI prints
`note: X moved to Y` and asks again, and `--trust-source` does not carry
over to it. In the app, **Update all** on the Skills page sends no trust
answer, so updates from sources that need one fail with a message and stay
for you to review one by one.

### `source_blocked`

A block in `store/sources.toml` matches the repository, its owner or its
host. Loadout does not contact the source and changes nothing. A block wins
over any trust and over `--trust-source`. To unblock it: Settings >
Sources, **Unblock**, or `loadout source ls` and `loadout source untrust
<KEY>`. Blocks sync with your remotes, so a block made on another machine
applies here too.

### The trust question comes back for a source I trusted

Trust lapses when the repository changes under you: its history was
rewritten (a force-push removed the commit you have) or the host redirects
it to another repository (renamed or transferred). Settings > Sources shows
**Asks again** with the reason and date, and `loadout source ls` shows
`lapsed`. Check what happened to the repository, then answer the question
again. An `http://` or `git://` address of the same repository is a
separate source: HTTPS or SSH trust never covers it.

### `scan_blocked`, or exit code 8

Loadout's built-in scan found something `dangerous` in what you were
installing, updating or saving (the files, or an MCP server's command
line). Nothing was saved. The command printed the findings (severity,
rule, `file:line`, message and a masked excerpt); `--json` has them under
`scan`. Read them. If you trust the content and its author, run the
command again with `--allow-dangerous`, or tick "I read the findings and
want to go ahead anyway" and choose **Install anyway**, **Update anyway**,
**Save anyway** or **Restore anyway** in the app. `loadout skills update`
leaves a blocked skill as it is (`blocked`), applies the other updates and
still exits 8. In the app's MCP review, editing a server's command after
the review means it is scanned again when you save; a dangerous edit turns
the gate on for that server.

A finding is not always malice: an installer command in a script, or an
instruction in an HTML comment, matches the rules on purpose. Trusting a
source never skips the scan, and custom rules in `state/scan-rules.toml`
can add rules or raise severities but never lower or remove one.

### A finding says `not-fully-scanned` or `not-content-scanned`

- `not-fully-scanned` (warning): a file is larger than 4 MiB, and only its
  first 4 MiB were scanned. Look at the rest yourself, or leave the file
  out of the source.
- `not-content-scanned` (info): files Loadout does not read as text
  (images, fonts, archives, media, native binaries) are listed once. A
  native binary also gets a `native-binary` warning: it cannot be reviewed
  by reading it.

Dependency and build folders of an MCP source (`node_modules`, `.venv`,
`venv`, `__pycache__`, `target`, `dist`, `build`) are not read or scanned
at all; the review names them. A server that runs from `dist/` relies on
the repository you trusted.

### `checkout_out_of_date`: the server's local copy is out of date

The server runs from a copy of its repository that Loadout keeps on each
machine in `LOADOUT_HOME/servers/<id>` (never synced), and this machine's
copy is missing or at another commit than the server file names. That
happens after another machine updated the server and you pulled the store,
on a new machine, or when the folder was deleted. Its client entries stay
blocked until the copy matches. Restore it: **Restore copy** in the
server's details on the MCP page (a review with the trust state and scan,
then **Restore**), or `loadout server checkout <ID>` (`--trust-source`,
`--allow-dangerous` as for adding it). Run the server's setup steps again
afterwards if it has any.

### `folder_on_other_machine`

The server was added from a local folder on another machine. It runs from
that folder, which does not exist here, so its entries are blocked on this
machine, and **Check for update** or `loadout server update` reports it.
Nothing is wrong on the machine that added it. To use the server
everywhere, add it from a Git repository instead (`loadout server add
--from <GIT-URL>`).

### `loadout server update` or **Check for update** says `source_unreachable`

The repository could not be fetched (offline, removed, or private without
credentials), or the folder is gone or belongs to another machine. The
server and its checkout are left as they are; other servers are still
checked.

## Data directory

### `sync_root_detected`

The data directory is inside a file-sync folder (OneDrive, Dropbox, Google
Drive, Syncthing, Resilio Sync). Sync tools corrupt live git repositories.
Move it (set `LOADOUT_HOME`) and use a remote to share the store; a folder
remote inside a synced folder is fine.

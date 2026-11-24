# Read and plan commands

These commands only read, scan or plan, and take no lock; run them with
`--json`. Nothing else from `loadout` is yours to run (see the rules in
SKILL.md).

## What Loadout manages

| Command | Shows |
|---|---|
| `loadout status --json` | Every client file Loadout plans: `targets[]` with `clients`, `scope`, `file`, `problem`, `read_only` and `entries[]` (`name`, `client`, `server`, `state`, `action`, `unresolved`, `problem`); `unfinished` apply runs. Add a project folder as `PATH`, or `--client <id>`, to narrow it |
| `loadout effective --json` | For the current folder (or `PATH`): `project`, and per client id the servers it loads: `name`, `loaded`, `winner` (layer and file), `shadowed`, `status`, `warnings` |
| `loadout server ls --json` | Store servers: `id`, `valid`, `transport`, `command`, `args`, `url`, `clients` |
| `loadout server show <id> --json` | One server definition |
| `loadout skills ls --json` | Store skills: `id`, `name`, `description`, `source`, `enable.clients`, `links`, `problems` |
| `loadout skills scan --json` | Client skill folders: `dirs[]` with `client`, `dir`, `exists` and `entries[]` classed `managed`, `unmanaged` or `mismatch`. `--project <path>` scans a project |
| `loadout project ls --json` | Projects this machine knows: `id`, `path`, `trust` |
| `loadout source ls --json` | Trusted and blocked skill and server sources |
| `loadout catalog ls --json` | Catalogs (`id`, `kind`, `enabled`) and whether online catalogs are on (`online`) |
| `loadout catalog search <query> --json` | Entries per catalog: `results[]` with `status` (`fresh`, `cached`, `stale`, `off`, `error`, ...), `age_secs` and `entries[]` (`ref`, `kind`, `name`, `description`, `publisher`, `transports`, `required_secrets`, `pinned`, `problem`). Contacts catalogs only when online catalogs are on; a registry needs 3 characters |
| `loadout catalog show <source>/<id> --json` | One entry with its packages, remotes, license, repository and how to install it |
| `loadout secrets ls --json` | Secret aliases, this machine's reference for each and the servers using them (names, never values) |
| `loadout secrets check --json` | Status of each alias (`resolved`, `missing`, `locked`, `provider_unavailable`, ...) and how to fix it; exits 5 when one cannot be resolved. It can make 1Password ask the user for approval, so run it only when they ask about secrets |
| `loadout secrets providers --json` | Which secret providers this machine has |
| `loadout remote ls --json` | Backup and sync remotes |
| `loadout agent status --json` | This skill: `bundled_version`, the store `copy` (`version`, `state`: `current`, `outdated`, `customized`, `newer`) with the `action` that updates or replaces it, and the `clients` it is enabled for |
| `loadout doctor --json` | Installation problems: data folder, sync-folder warning |

## Plans and previews

| Command | Shows |
|---|---|
| `loadout apply --dry-run --json` | What an apply would write: `targets[]` as in `status`, with each entry's `action` (`create`, `update`, `remove`, `disable`, ...). Writes nothing |
| `loadout diff --json` | The same plan as file diffs (secret values masked) |
| `loadout import --scan --json` | Client entries Loadout does not manage yet: `candidates[]` with `name`, `identical`, `conflict`, `in_store`, `sources[]` (`client`, `file`, `importable`) and `variants[]`, whose `secrets[]` (`field`, `alias`, `suggested_ref`) are the literal secrets an import would move into aliases. Changes nothing |
| `loadout history --json` | Store commits, newest first (`--limit <n>`, `--entity server/<id>`) |
| `loadout show <rev> --json` | One commit and what it changed |
| `loadout diff --rev <a>..<b> --json` | What changed in the store between two commits or tags |

## Commands to hand to the user

Never run these; give them to the user with one sentence on what they do.

| Change | Command |
|---|---|
| Write client files from the store | `loadout apply` |
| Import entries found by the scan | `loadout import <name>` |
| Enable a skill for a client | `loadout skills enable <id> --client <client>` |
| Install a skill | `loadout skills add <git-url-or-folder>` |
| Add a server from a repository | `loadout server add --from <git-url-or-folder>` |
| Add a server from a catalog | `loadout server add --from catalog:<source>/<id>` (the `ref` exactly as Loadout printed it) |
| Install a skill from a catalog | `loadout skills add catalog:<source>/<id>` (the `ref` exactly as Loadout printed it) |
| Turn online catalogs on | `loadout catalog online on` |
| Trust a project's manifests | `loadout project trust <path>` |
| Undo the last apply | `loadout rollback last` |
| Point a secret alias at a provider | `loadout secrets set <alias> --ref <reference>` |
| Update this skill's store copy | `loadout agent install` |
| Replace an edited copy of this skill with Loadout's | `loadout agent install --reset` |

Loadout asks the user to confirm trust, scan findings and conflicts itself;
leave those prompts to them. The Loadout desktop app has the same actions on
its MCP, Skills, Projects, Install, History and Settings pages.

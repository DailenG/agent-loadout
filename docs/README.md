# Loadout documentation

## Using Loadout

| Page | What it covers |
|---|---|
| [Getting started](getting-started.md) | Install, first run, bringing in what you have (Import page, CLI), first apply, installing a skill or MCP server from Git or a folder (the trust question and the scan), undo |
| [Concepts](concepts.md) | Store, scopes, clients and adapters, apply and drift, read-only files, ownership (managed and not managed entries, import, adopt), secrets, projects and trust, skills, sources, trust and the built-in scan, MCP servers from a source (checkouts, folders bound to a machine), remotes |
| [User guide](user-guide.md) | Every task in the app and on the command line, including importing, skill adopt, Discover projects, the not-managed notice, servers from a Git repository or folder, the built-in scan and source trust |
| [CLI reference](cli.md) | Every `loadout` command and option (generated from `--help`) |
| [Supported clients](adapters.md) | All 38 client adapters: files, formats, capabilities, verification |
| [Troubleshooting](troubleshooting.md) | Known problems and fixes, including trust, scan and checkout errors |
| [FAQ](faq.md) | Short answers to common questions |
| [Security model](security.md) | What Loadout guarantees about secrets, client files, sources and the scan |

## Working on Loadout

| Page | What it covers |
|---|---|
| [Architecture](architecture.md) | Crates, core modules, Tauri commands and IPC, frontend structure, data flow |
| [CONTRIBUTING](../CONTRIBUTING.md) | Setup, gates, test rules, commits, releases |
| [SPEC.md](../SPEC.md) | The design contract |
| [CHANGELOG](../CHANGELOG.md) | Changes per release |
| [History](history.md) | How Loadout was built, milestone by milestone |
| [schema/](schema) | JSON Schemas for the store files (generated) |
| [verify/](verify/README.md) | Real-client verification records |
| [milestones/](milestones) | Milestone completion notes (historical) |
| [internal/](internal/README.md) | Pre-build notes such as the survey of comparable tools (historical) |

You can also ask questions about the code on
[DeepWiki](https://deepwiki.com/DailenG/agent-loadout).

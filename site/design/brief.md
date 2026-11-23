# Loadout website: design brief (Phase 1)

## Goal

A product site for Loadout (`agent-loadout`) on GitHub Pages, at
`https://daileng.github.io/agent-loadout/` until a custom domain is chosen. It
has to explain the product in one screen, earn a skeptical developer's trust in
two more, and get them to a download or the source.

## Audience

1. **Developers who run several AI coding agents.** They use omp, Claude Code,
   Codex, Cursor, GitHub Copilot and others side by side, and keep the same MCP
   servers and skills in sync by hand across JSON, TOML, JSONC and YAML files.
   They read config diffs for fun and distrust tools that rewrite their files.
2. **MSP and IT pros.** They set up the same toolchain on many machines and
   care about signed installers, winget and Chocolatey, per-user installs
   without admin rights, offline operation, backups to a plain folder or NAS,
   and secrets that never land in a synced file.

Both groups read the page quickly and check claims against the repository.

## Positioning

One versioned, offline store for MCP servers, skills and secret references,
written into every AI coding client's own config files, without touching what
it didn't write.

What sets it apart, in order of importance:

1. **One store, many clients.** 38 client adapters, native formats, edited in
   place (comments and key order survive).
2. **It never touches entries it didn't create.** A machine-local ledger
   defines ownership; hand-written entries are "foreign" and left alone.
3. **Secrets stay references.** Env vars, the OS keyring or 1Password. A leak
   scanner checks every store commit.
4. **Every change is a commit.** Local git history, tags, restore, and
   "Undo this apply".
5. **Projects need trust.** A project manifest can run commands, so it stays
   read only until its diff is reviewed.
6. **No vendor in the loop.** Offline by default; optional remotes are any git
   URL or a plain folder; no baked-in OAuth app. Open source, MIT.

Not a gateway, proxy or container runtime, and not a marketplace. Say so only
if asked (FAQ in Phase 2), never as a headline.

## Tone

Plain, precise, calm. Short declarative sentences; concrete nouns (file paths,
commands, commit hashes) over adjectives. Confident about what ships, explicit
about what is pending. American English. No hype words ("revolutionary",
"supercharge", "10x"), no exclamation marks, no fake testimonials or usage
numbers.

## Factual guardrails

These come from README.md, docs/user-guide.md, docs/adapters.md and SPEC.md.
Recheck them before Phase 2 copy freeze.

- Clients: "38 client adapters". Daily (Tier A/B): omp (recommended, listed
  first), Claude Code, Codex, Claude Desktop (Windows, macOS), GitHub Copilot,
  Cursor, Pi. Supported (Tier C): Gemini CLI, Antigravity, OpenCode, Grok
  Build. Best effort (Tier D): Windsurf, Devin Desktop and the ported clients.
- Platforms: Windows 11 and Linux apps. **No macOS app**; the CLI builds on
  macOS with `cargo install agent-loadout`.
- Pending at the time of writing, label them as such: winget, Chocolatey, the
  AUR packages, and the crates.io release.
- Windows release builds are signed (Azure Trusted Signing); the installer is
  per user and needs no admin rights.
- Status is early development; the site must not imply a 1.0.
- Use client names as text. No third-party logos without permission.
- App screenshots: only the mock-data captures from `docs/milestones/M28-*`
  that show no user names or absolute paths.

## Content outline (single landing page)

1. **Header**: mark and wordmark; How it works, Clients, Safety, Install,
   Docs; GitHub.
2. **Hero**: one-line promise, two-sentence explanation, primary "Download"
   (jumps to Install, OS-aware in Phase 2), secondary "View source". The hero
   visual shows one store fanning out to many clients.
3. **Problem**: one server, six clients, six formats, one secret pasted six
   times, and no idea which entries you wrote.
4. **How it works**: Define once, review the plan, apply with an undo.
5. **One entry, many dialects**: the same server rendered for three clients,
   showing how each spells a secret reference.
6. **Clients grid**: by tier, omp first and marked Recommended, with user
   config paths for the daily clients.
7. **Safety and trust**: ownership ledger, secrets by reference, git history,
   project trust, format-preserving edits, no vendor lock-in.
8. **Screenshots**: MCP page, pending changes drawer, history, trust review.
9. **Install**: Windows (installer, winget, Chocolatey), Linux (AppImage, deb,
   rpm, AUR), CLI and macOS (cargo); SHA-256 note; pending labels.
10. **Open source**: GitHub, user guide, client adapters, DeepWiki, releases,
    MIT.
11. **Footer**: links, license, "built and tested with omp first", no
    affiliation with client vendors.

## Information architecture

- `/` (single landing page with in-page anchors). This is the whole site at
  launch.
- `/download/` (optional, Phase 2): OS-detected download page that reads the
  latest release from the GitHub API at build time, with checksums and the
  pending-package notes. Can start as the Install section's anchor.
- Docs stay on GitHub (`docs/user-guide.md`, `docs/adapters.md`) and DeepWiki;
  the site links out rather than duplicating them.

## Constraints

- Static HTML and CSS, minimal vanilla JS, no framework, no CDN, no trackers,
  no cookies. All fonts self-hosted woff2 under the SIL Open Font License.
- GitHub Pages base path `/agent-loadout/`: every asset URL relative.
- WCAG 2.2 AA: text contrast at least 4.5:1, focus visible on every control,
  semantic landmarks, a skip link, alt text on screenshots, motion only as
  enhancement and off under `prefers-reduced-motion`.
- Performance budget for Phase 2: under 300 KB transferred before images on
  the first view, fonts preloaded, screenshots as AVIF or WebP with PNG
  fallback, LCP under 2 s on a mid-range phone.

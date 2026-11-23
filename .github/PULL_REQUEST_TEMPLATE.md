## What and why

<!-- What this changes and why. Link the issue: "Fixes #123". -->

## Checks

<!-- See CONTRIBUTING.md, "Gates". Tick what you ran. -->

- [ ] `cargo fmt`, `cargo clippy` and `cargo test` (root workspace)
- [ ] The same for `src-tauri` (if the desktop app changed)
- [ ] `npm test` and `npx tsc --noEmit` (if the frontend changed)
- [ ] Generated files regenerated in this change (bindings, adapter table, schemas, `docs/cli.md`)
- [ ] Docs and `CHANGELOG.md` (`Unreleased`) updated for user-visible changes
- [ ] No real secrets, client configs or personal paths in code, tests or fixtures

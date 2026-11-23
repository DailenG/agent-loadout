# Internal notes

Historical working documents from before and during the first build of
Loadout. They record what was known and planned at the time and are **not a
description of current behavior**: for that, read the
[documentation](../README.md), [SPEC.md](../../SPEC.md) and the code.

- [LANDSCAPE.md](LANDSCAPE.md): the survey of comparable tools (verified
  2026-11-14) that SPEC.md's non-goals and licensing notes cite. It names
  SkillDock's dead upstream on purpose, so the baked-id checks
  (`scripts/check-no-baked-ids.sh` and its Rust twin) skip it.

The per-milestone work briefs (`*-TASK.md`) are kept out of the repository
(`.gitignore`); what each milestone delivered is in
[docs/milestones](../milestones/) and [docs/history.md](../history.md).

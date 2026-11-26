SkillSpector fixtures (#30, M50).

**Hand-made, not captured output.** `output/` holds `--format json` reports written by hand from SkillSpector 2.13.7's
documented report shape (`_format_json` in `src/skillspector/nodes/report.py`, `Finding.to_dict()` in `models.py`,
`analysis_completeness` in `inspection_ledger.py`, and the README's machine-readable example, all at tag v2.13.7).
SkillSpector publishes no JSON Schema. Every report says so in `skill.source`.

- `safe.json`: no issues, `SAFE`, complete.
- `caution.json`: `CAUTION` with a `HIGH` (E2) and a `MEDIUM` (P3) issue.
- `do-not-install.json`: `DO_NOT_INSTALL` with a `CRITICAL` issue (and a `HIGH` one).
- `do-not-install-high.json`: `DO_NOT_INSTALL` from repeated `HIGH` issues, no `CRITICAL`.
- `incomplete.json`: `analysis_completeness.is_complete: false` (`partial`, 87.5%), what `--fail-on-incomplete` exits 1 on.
- `osv-unreachable.json`: the `SC4` `LOW` finding SkillSpector adds when OSV.dev can't be reached.
- `execution-failed.json`: `execution_successful: false`.
- `llm-requested.json`: `metadata.llm_requested: true`.
- `malformed.txt`: a truncated report.

The test stub (`skillspector-stub` in the testkit) prints them, as `skillspector` or as `uvx`.

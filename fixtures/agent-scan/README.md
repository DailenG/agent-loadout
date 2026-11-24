Snyk Agent Scan fixtures (#27, M47).

`v0.7.1/`: the release's `sha256sums.txt.asc` (cleartext-signed, byte for byte as published) and Snyk's signing key
`snyk-code-signing-public.pgp` (from the agent-scan repository at tag v0.7.1,
`help/_about-this-project/snyk-code-signing-public.pgp`; fingerprint `EF4F F9D1 8D82 D562 9B82 491D 2A20 F86E 4EF1
31B1`). A unit test checks that the hashes compiled into Loadout equal the ones in the signed body;
`scripts/verify-agent-scan-pins.sh` / `.ps1` checks the signature with gpg.

`output/`: `--json` documents in the v0.6+ `ScanResponse` schema, written from the schema and the error messages in
the v0.7.1 source (`docs/json-output.md`, `verify_api.py`), not captured from a run (no token is used in development):
a clean skill, a warning, a dangerous verdict (score 600 with a malicious URL, and `malicious_code` at a low score),
the daily usage limit (429), a scope too large (413), a refused token (401), a truncated document and the old 0.5
schema. The scanner stub (`agent-scan-stub` in the testkit) prints them.

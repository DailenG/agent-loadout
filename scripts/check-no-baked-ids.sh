#!/usr/bin/env bash
# Fails when the repository contains a baked-in OAuth client id or a reference
# to SkillDock's dead upstream (SPEC.md section 10, milestone M0).
# Rust twin for cargo test (and Windows): crates/agent-loadout-testkit/tests/no_baked_ids.rs
set -euo pipefail

cd "$(dirname "$0")/.."

# Assembled from fragments so this script does not match itself.
regex="Ov2""3li|Iv1""\\.|wang""huan9"
# Design documents that name them on purpose, to explain what this project avoids.
allowed=("SPEC.md" "docs/internal/LANDSCAPE.md")

found=0
while IFS= read -r -d '' file; do
    for allow in "${allowed[@]}"; do
        [[ "$file" == "$allow" ]] && continue 2
    done
    # Deleted but still tracked files have nothing to scan.
    [[ -f "$file" ]] || continue
    if grep -nHIE -- "$regex" "$file"; then
        found=1
    fi
done < <(git ls-files -z --cached --others --exclude-standard)

if [[ "$found" -ne 0 ]]; then
    echo "error: baked-in ids found (see above)" >&2
    exit 1
fi
echo "ok: no baked-in ids"

#!/usr/bin/env bash
# Checks the pinned Snyk Agent Scan release (#27, M47) with the system gpg:
# the signing key in fixtures/agent-scan/<version>/ has the pinned
# fingerprint, it signs sha256sums.txt.asc ("Good signature"), and every
# hash compiled into crates/agent-loadout-core/src/scan/external/pin.rs is
# the signed hash of that asset. Run it when the pin is bumped (after
# replacing the fixture files with the new release's); Loadout itself has
# no OpenPGP code and checks downloads against the compiled hashes.
#
#   scripts/verify-agent-scan-pins.sh              # the recorded fixtures
#   scripts/verify-agent-scan-pins.sh --download   # fetch the .asc and key from GitHub first
#
# Run from the repository root. Needs gpg (and curl with --download).
set -euo pipefail

FINGERPRINT="EF4FF9D18D82D5629B82491D2A20F86E4EF131B1"
PIN="crates/agent-loadout-core/src/scan/external/pin.rs"
VERSION="$(sed -n 's/^pub const VERSION: &str = "\(.*\)";/\1/p' "$PIN")"
DIR="fixtures/agent-scan/v${VERSION}"

if [[ "${1:-}" == "--download" ]]; then
  mkdir -p "$DIR"
  curl -fsSL -o "$DIR/sha256sums.txt.asc" \
    "https://github.com/snyk/agent-scan/releases/download/v${VERSION}/sha256sums.txt.asc"
  curl -fsSL -o "$DIR/snyk-code-signing-public.pgp" \
    "https://raw.githubusercontent.com/snyk/agent-scan/v${VERSION}/help/_about-this-project/snyk-code-signing-public.pgp"
fi

GNUPGHOME="$(mktemp -d)"
export GNUPGHOME
trap 'rm -rf "$GNUPGHOME"' EXIT
chmod 700 "$GNUPGHOME"

gpg --batch --quiet --import "$DIR/snyk-code-signing-public.pgp"
primary="$(gpg --batch --with-colons --fingerprint | awk -F: '/^fpr:/ { print $10; exit }')"
if [[ "$primary" != "$FINGERPRINT" ]]; then
  echo "FAIL: the key's fingerprint is $primary, not the pinned $FINGERPRINT" >&2
  exit 1
fi

status="$(gpg --batch --status-fd 1 --verify "$DIR/sha256sums.txt.asc" 2>/dev/null || true)"
if ! grep -q "^\[GNUPG:\] VALIDSIG ${FINGERPRINT} " <<<"$status"; then
  echo "FAIL: sha256sums.txt.asc is not signed by $FINGERPRINT" >&2
  exit 1
fi
echo "ok: sha256sums.txt.asc v${VERSION} has a good signature from $FINGERPRINT"

# The signed text only (gpg strips the armor and undoes dash-escaping).
signed="$(gpg --batch --quiet --decrypt "$DIR/sha256sums.txt.asc" 2>/dev/null)"
failed=0
while read -r name hash; do
  expected="$(awk -v n="$name" '$2 == n { print $1 }' <<<"$signed")"
  if [[ "$expected" == "$hash" ]]; then
    echo "ok: $name"
  else
    echo "FAIL: $name is pinned as $hash, signed as ${expected:-<missing>}" >&2
    failed=1
  fi
done < <(awk '
  /name: "/ { gsub(/.*name: "|",.*/, ""); name = $0 }
  /sha256: "/ { gsub(/.*sha256: "|",.*/, ""); print name, $0 }
' "$PIN")
exit "$failed"

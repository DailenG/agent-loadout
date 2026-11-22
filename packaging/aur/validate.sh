#!/usr/bin/env bash
# Validates both AUR packages without a published release (SPEC 16.2:
# `makepkg --nobuild` in the Arch CI job). Run as a normal user on Arch with
# base-devel (makepkg refuses root).
#
# - .SRCINFO matches `makepkg --printsrcinfo` for each PKGBUILD.
# - agent-loadout: the release source tarball is stood in for by
#   `git archive HEAD` under the same name and top folder, so
#   `makepkg --nobuild` runs prepare() for real (cargo fetch, npm ci).
# - agent-loadout-bin: a stand-in tarball with the release layout (top
#   folder, usr/, LICENSE) checks the source name and extraction only.
# - namcap on each PKGBUILD when namcap is installed.
#
# Extra makepkg flags come from MAKEPKG_FLAGS (for example
# "--nodeps --noprepare" on a machine without the make dependencies), and
# LOADOUT_AUR_REV picks the revision archived for the source package
# (default HEAD).
set -euo pipefail

root="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
aur="$root/packaging/aur"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
read -r -a flags <<<"${MAKEPKG_FLAGS:-}"

srcinfo_matches() {
  local dir=$1
  if ! diff -u "$dir/.SRCINFO" <(cd "$dir" && makepkg --printsrcinfo); then
    echo "$(basename "$dir"): .SRCINFO is stale; run makepkg --printsrcinfo > .SRCINFO" >&2
    return 1
  fi
  echo "$(basename "$dir"): .SRCINFO matches"
}

version="$(sed -n 's/^pkgver=//p' "$aur/agent-loadout/PKGBUILD")"
bin_version="$(sed -n 's/^pkgver=//p' "$aur/agent-loadout-bin/PKGBUILD")"
if [ "$version" != "$bin_version" ]; then
  echo "pkgver differs: agent-loadout $version, agent-loadout-bin $bin_version" >&2
  exit 1
fi

# Source package.
src="$work/agent-loadout"
cp -r "$aur/agent-loadout" "$src"
srcinfo_matches "$src"
git -C "$root" archive --format=tar.gz --prefix="agent-loadout-$version/" \
  -o "$src/agent-loadout-$version.tar.gz" "${LOADOUT_AUR_REV:-HEAD}"
(cd "$src" && makepkg --nobuild --noconfirm "${flags[@]}")
echo "agent-loadout: makepkg --nobuild passed"

# Binary package.
bin="$work/agent-loadout-bin"
cp -r "$aur/agent-loadout-bin" "$bin"
srcinfo_matches "$bin"
stage="$work/stage/agent-loadout-$version-x86_64-linux"
mkdir -p "$stage/usr/bin"
cp "$root/LICENSE" "$root/THIRD_PARTY_NOTICES.md" "$stage/"
tar -C "$work/stage" -czf "$bin/agent-loadout-$version-x86_64-linux.tar.gz" \
  "agent-loadout-$version-x86_64-linux"
(cd "$bin" && makepkg --nobuild --noconfirm "${flags[@]}")
echo "agent-loadout-bin: makepkg --nobuild passed"

if command -v namcap >/dev/null; then
  namcap "$aur/agent-loadout/PKGBUILD"
  namcap "$aur/agent-loadout-bin/PKGBUILD"
  echo "namcap: done"
else
  echo "namcap: not installed, skipped"
fi

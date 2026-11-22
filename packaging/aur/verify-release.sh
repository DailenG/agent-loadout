#!/usr/bin/env bash
# Checks both AUR packages against the published release: downloads the real
# sources and verifies them against the PKGBUILD checksums
# (`makepkg --verifysource`), and checks each .SRCINFO is current. Run after
# a release, once `updpkgsums` has filled the sums. Normal user on Arch with
# base-devel (makepkg refuses root).
set -euo pipefail

root="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

for pkg in agent-loadout agent-loadout-bin; do
  cp -r "$root/packaging/aur/$pkg" "$work/$pkg"
  (
    cd "$work/$pkg"
    diff -u .SRCINFO <(makepkg --printsrcinfo)
    makepkg --verifysource --noconfirm
  )
  echo "$pkg: sources match the PKGBUILD checksums"
done

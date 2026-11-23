# Chocolatey package: agent-loadout

The [Chocolatey Community Repository](https://community.chocolatey.org/packages/agent-loadout)
package for Loadout on Windows. It downloads the release's
`Loadout_<version>_x64-setup.exe` from GitHub (SHA-256 checked) and runs it
silently. Nothing is embedded in the package.

## Files

- `agent-loadout/agent-loadout.nuspec`: metadata and description.
- `agent-loadout/tools/chocolateyinstall.ps1`: `Install-ChocolateyPackage`
  with the release URL and checksum, `/S`. Refuses to run as SYSTEM (the
  installer is per user).
- `agent-loadout/tools/chocolateyuninstall.ps1`: finds the per-user
  `HKCU\...\Uninstall\Loadout` entry with `Get-UninstallRegistryKey`, runs
  its `uninstall.exe /S` and waits until the entry and the uninstaller are
  gone (the NSIS uninstaller copies itself to TEMP and returns at once).
- `agent-loadout/update.ps1`: the [chocolatey-au](https://github.com/chocolatey-community/chocolatey-au)
  updater. Reads the latest GitHub release, takes the installer's checksum
  from the release's `SHA256SUMS`, and rewrites the nuspec version, the
  release notes link, `$url64` and `$checksum64`.
- `.github/workflows/chocolatey.yml`: runs the updater on every published
  release, daily, and on demand; pushes and commits.

The committed files always describe the newest version that was packaged.
Never hand-edit `$url64`, `$checksum64` or the nuspec version for a normal
release: the workflow does it.

## How a release reaches Chocolatey

1. The maintainer publishes the draft GitHub release.
2. `chocolatey.yml` runs (`release: published`): `update.ps1` sees the newer
   version and rewrites the files; because that version is not on
   community.chocolatey.org yet, the job packs and pushes it with
   `CHOCO_API_KEY`; the `commit` job commits the rewritten files to `main`
   as `github-actions[bot]` with `[skip ci]`.
3. The pushed version goes through Chocolatey's automated checks (validator,
   verifier) and, for a new package or a package without trusted status,
   human moderation. Messages arrive on the package page and by email to the
   Chocolatey account.

The daily run is the safety net: if a push or the commit failed, the next
run repeats whatever is missing (the files follow GitHub; the push happens
whenever the nuspec version is not on the site in any state).

## Local commands

```powershell
# Pack (output outside the repository)
choco pack packaging\chocolatey\agent-loadout\agent-loadout.nuspec --output-directory $env:TEMP\choco-out

# Check for an update without pushing (needs: choco install chocolatey-au)
pwsh -File packaging\chocolatey\agent-loadout\update.ps1
```

`update.ps1` changes the files in place when a newer release exists and
writes `agent-loadout.<version>.nupkg` next to the nuspec (ignored by git);
revert with `git checkout -- packaging/chocolatey` if you only wanted to
look. Test installs belong in a disposable VM or Windows Sandbox, not on a
working machine.

## Changing the package itself

A change to the scripts or the description without a new Loadout release
needs a new package version: use the
[package fix version notation](https://docs.chocolatey.org/en-us/create/create-packages/#package-fix-version-notation)
(`0.1.2.20261201`) in the nuspec; the workflow pushes it because that
version is not on the site. The next release replaces it normally.

## Repository settings

- Environment `chocolatey` (Settings, Environments): deployment branches and
  tags limited to `main` and `v*`; secret `CHOCO_API_KEY` (from
  https://community.chocolatey.org/account, API Keys).
- The `commit` job pushes to `main` with `GITHUB_TOKEN` (`contents: write`
  on that job only). If `main` gets a ruleset that blocks direct pushes,
  allow GitHub Actions to bypass it or the commit step fails (the package
  is still pushed; the next run retries the commit).

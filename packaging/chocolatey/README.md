# Chocolatey package: agent-loadout

The [Chocolatey Community Repository](https://community.chocolatey.org/packages/agent-loadout)
package for Loadout on Windows. It downloads the release's
`Loadout_<version>_x64-setup.exe` from GitHub (SHA-256 checked) and runs it
silently. Nothing is embedded in the package.

## Files

- `agent-loadout/agent-loadout.nuspec`: metadata and description. A
  template: `__VERSION__` and `__RELEASE_NOTES__` are filled in at build
  time.
- `agent-loadout/tools/chocolateyinstall.ps1`: `Install-ChocolateyPackage`
  with the release URL and checksum (`__URL64__`, `__CHECKSUM64__`), `/S`.
  Refuses to run as SYSTEM (the installer is per user).
- `agent-loadout/tools/chocolateyuninstall.ps1`: finds the per-user
  `HKCU\...\Uninstall\Loadout` entry with `Get-UninstallRegistryKey`, runs
  its `uninstall.exe /S` and waits until the entry and the uninstaller are
  gone (the NSIS uninstaller copies itself to TEMP and returns at once).
- `agent-loadout/build.ps1`: builds the package for one release. Reads the
  GitHub release (the latest unless `-Tag` is given), takes the installer's
  checksum from the release's `SHA256SUMS`, fills the templates in a copy
  under `-OutputDirectory` and runs `choco pack`. The committed files are
  never changed.
- `.github/workflows/chocolatey.yml`: builds and pushes the package when a
  release is published, or on demand.

## How a release reaches Chocolatey

1. The maintainer publishes the draft GitHub release.
2. `chocolatey.yml` runs (`release: published`): `build.ps1` builds the
   package for that tag (kept as a workflow artifact), and, unless that
   version is already on community.chocolatey.org in any moderation state,
   the job pushes it with `CHOCO_API_KEY`. Requests to Chocolatey are
   retried (its feed answers 5xx from time to time); nothing is installed
   from the Chocolatey feed.
3. The pushed version goes through Chocolatey's automated checks (validator,
   verifier) and, for a package without trusted status, human moderation.
   Messages arrive on the package page and by email to the Chocolatey
   account.

If the run fails anyway, run the workflow by hand (Actions, Chocolatey, Run
workflow) with the release tag. It is safe to run again: a version that is
already on the site is not pushed.

## Local commands

```powershell
# Build the package of the latest release (or -Tag v0.8.0) outside the repository
pwsh -File packaging\chocolatey\agent-loadout\build.ps1 -OutputDirectory $env:TEMP\choco-out
```

Test installs belong in a disposable VM or Windows Sandbox, not on a
working machine.

## Changing the package itself

A change to the scripts or the description without a new Loadout release
needs a new package version: run the workflow by hand with the release tag
and a `package_version` in Chocolatey's
[package fix version notation](https://docs.chocolatey.org/en-us/create/create-packages/#package-fix-version-notation)
(`0.8.0.20261201`). The next release replaces it normally.

## Repository settings

- Environment `chocolatey` (Settings, Environments): deployment branches and
  tags limited to `main` and `v*`; secret `CHOCO_API_KEY` (from
  https://community.chocolatey.org/account, API Keys).

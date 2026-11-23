$ErrorActionPreference = 'Stop'

# The installer registers HKCU\...\Uninstall\Loadout for the account that
# installed it; Get-UninstallRegistryKey also reads HKCU.
[array] $keys = Get-UninstallRegistryKey -SoftwareName 'Loadout'

if ($keys.Count -eq 0) {
  Write-Warning 'Loadout is not registered for this user; it was uninstalled already or installed by another account. Nothing to do.'
  return
}
if ($keys.Count -gt 1) {
  throw ('Found {0} uninstall entries named Loadout; remove it from Settings, Apps instead.' -f $keys.Count)
}

$key = $keys[0].PSPath
$uninstaller = $keys[0].UninstallString -replace '"', ''

$packageArgs = @{
  packageName    = $env:ChocolateyPackageName
  fileType       = 'exe'
  file           = $uninstaller
  silentArgs     = '/S'
  validExitCodes = @(0)
}
$null = Uninstall-ChocolateyPackage @packageArgs

# The NSIS uninstaller copies itself to TEMP and returns at once; wait for
# the copy to finish so choco reports the real outcome.
$deadline = (Get-Date).AddMinutes(2)
while ((Test-Path -LiteralPath $key) -or (Test-Path -LiteralPath $uninstaller)) {
  if ((Get-Date) -gt $deadline) {
    throw 'The Loadout uninstaller did not finish within 2 minutes.'
  }
  Start-Sleep -Seconds 1
}

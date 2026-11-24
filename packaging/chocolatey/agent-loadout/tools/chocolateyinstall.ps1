$ErrorActionPreference = 'Stop'

$url64 = 'https://github.com/DailenG/agent-loadout/releases/download/v0.3.1/Loadout_0.3.1_x64-setup.exe'
$checksum64 = 'f980ab4c9df9f1d343a6d9c73699ea2e9a56fc500c17e68cba81eeeff406235e'

# The installer is per user: it installs into the profile of the account
# running it. Under SYSTEM that profile belongs to nobody.
if ([System.Security.Principal.WindowsIdentity]::GetCurrent().IsSystem) {
  throw 'Loadout installs for the current user only, and this is the SYSTEM account. Run choco as the account that will use Loadout (an elevated prompt of that account).'
}

$packageArgs = @{
  packageName    = $env:ChocolateyPackageName
  fileType       = 'exe'
  url64bit       = $url64
  checksum64     = $checksum64
  checksumType64 = 'sha256'
  silentArgs     = '/S'
  validExitCodes = @(0)
  softwareName   = 'Loadout'
}

Install-ChocolateyPackage @packageArgs

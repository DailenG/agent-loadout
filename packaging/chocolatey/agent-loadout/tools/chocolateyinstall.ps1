$ErrorActionPreference = 'Stop'

$url64 = 'https://github.com/DailenG/agent-loadout/releases/download/v0.1.3/Loadout_0.1.3_x64-setup.exe'
$checksum64 = 'bc2d89fb6d576919de1a68df7cce068fca68691ab0af37bc96a0f033a9bf9931'

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

$ErrorActionPreference = 'Stop'

$url64 = 'https://github.com/DailenG/agent-loadout/releases/download/v0.4.0/Loadout_0.4.0_x64-setup.exe'
$checksum64 = '795e1bba6656c3421460b2f27c232a9014362f7a62886a15e8dda36cfdc17d47'

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

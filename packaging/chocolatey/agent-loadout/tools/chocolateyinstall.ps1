$ErrorActionPreference = 'Stop'

$url64 = 'https://github.com/DailenG/agent-loadout/releases/download/v0.6.0/Loadout_0.6.0_x64-setup.exe'
$checksum64 = '6d3f1eacd4e1c406d7ed53cf83211839be7bc5bd16615043fff32e99ea3b6cbe'

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

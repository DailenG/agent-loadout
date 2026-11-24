$ErrorActionPreference = 'Stop'

$url64 = 'https://github.com/DailenG/agent-loadout/releases/download/v0.7.0/Loadout_0.7.0_x64-setup.exe'
$checksum64 = '47756a2d61b65132508602edde283ce2dc8fb17c2359cce385d369d4b59ac645'

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

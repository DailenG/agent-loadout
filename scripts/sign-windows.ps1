<#
.SYNOPSIS
    Signs one file for a Loadout Windows build (SPEC 16.1). Tauri runs this
    as bundle.windows.signCommand for the app executable, loadout.exe, the
    NSIS uninstaller and the installer.

.DESCRIPTION
    Signing happens only when all four variables are set:

      LOADOUT_SIGN_KIT       folder of the code-signing kit checkout
      LOADOUT_SIGN_ACCOUNT   Azure Trusted Signing account name
      LOADOUT_SIGN_ENDPOINT  Trusted Signing endpoint URL
      LOADOUT_SIGN_PROFILE   certificate profile name

    They exist only in the release job and on the maintainer's signing
    machine. With any of them missing the script prints "unsigned dev build"
    and exits 0, so local and contributor builds stay unsigned.

    The kit's TrustedSigning module does the signing. When the kit's service
    principal variables are present (AZURE_CLIENT_SECRET with tenant and
    client id, the maintainer's machine), Invoke-TrustedSigning is used as is.
    Without a client secret (the release job, logged in to Azure through a
    GitHub OIDC federated credential), the kit's AzureTrustedSignTool is
    called directly so Azure.Identity picks up the existing login.

    This script never reads, prints or stores a credential.

.PARAMETER Path
    File to sign in place.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string] $Path
)

$ErrorActionPreference = 'Stop'

$names = @('LOADOUT_SIGN_KIT', 'LOADOUT_SIGN_ACCOUNT', 'LOADOUT_SIGN_ENDPOINT', 'LOADOUT_SIGN_PROFILE')
$missing = @($names | Where-Object { -not [Environment]::GetEnvironmentVariable($_, 'Process') })
if ($missing.Count -gt 0) {
    Write-Output ('unsigned dev build: {0} not signed ({1} not set)' -f (Split-Path -Leaf $Path), ($missing -join ', '))
    exit 0
}

if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    Write-Error ('sign-windows: no such file: {0}' -f $Path)
    exit 1
}
$file = (Resolve-Path -LiteralPath $Path).ProviderPath

$kit = $env:LOADOUT_SIGN_KIT
$manifest = Join-Path $kit 'module\TrustedSigning\TrustedSigning.psd1'
if (-not (Test-Path -LiteralPath $manifest -PathType Leaf)) {
    Write-Error ('sign-windows: LOADOUT_SIGN_KIT does not hold module\TrustedSigning: {0}' -f $kit)
    exit 1
}
Import-Module $manifest -Force

# The kit reads the Trusted Signing target from these names.
$env:AZURE_TRUSTED_SIGNING_ACCOUNT = $env:LOADOUT_SIGN_ACCOUNT
$env:AZURE_TRUSTED_SIGNING_ENDPOINT = $env:LOADOUT_SIGN_ENDPOINT
$env:AZURE_TRUSTED_SIGNING_PROFILE = $env:LOADOUT_SIGN_PROFILE

try {
    if ($env:AZURE_CLIENT_SECRET) {
        Invoke-TrustedSigning -Path $file | Out-Null
    }
    else {
        # Existing Azure login (OIDC in the release job): the kit's tool,
        # installed by Install-TrustedSigningTool, without the kit's check
        # for a stored client secret.
        $tools = Join-Path $env:USERPROFILE '.dotnet\tools'
        if (($env:PATH -split ';') -notcontains $tools) {
            $env:PATH = '{0};{1}' -f $env:PATH, $tools
        }
        if (-not (Get-Command AzureTrustedSignTool -ErrorAction SilentlyContinue)) {
            throw 'AzureTrustedSignTool is not installed. Run Install-TrustedSigningTool from the kit first.'
        }
        & AzureTrustedSignTool sign -g $file -t $env:LOADOUT_SIGN_ENDPOINT -a $env:LOADOUT_SIGN_ACCOUNT -p $env:LOADOUT_SIGN_PROFILE
        if ($LASTEXITCODE -ne 0) {
            throw ('AzureTrustedSignTool exited with {0}' -f $LASTEXITCODE)
        }
    }
}
catch {
    Write-Error ('sign-windows: signing failed for {0}: {1}' -f (Split-Path -Leaf $file), $_.Exception.Message)
    exit 1
}

Write-Output ('signed: {0}' -f (Split-Path -Leaf $file))
exit 0

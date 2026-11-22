<#
.SYNOPSIS
    Signs one file for a Loadout Windows build (SPEC 16.1). Tauri runs this
    as bundle.windows.signCommand for the app executable, loadout.exe, the
    NSIS uninstaller, the installer, and the NSIS plugin DLLs packed inside
    the installer (NSISdl, StartMenu, System, nsDialogs, nsis_tauri_utils).
    The plugins are signed on purpose (Tauri's default): they are extracted
    to %TEMP% and run as part of our installer, and unsigned DLLs there draw
    more antivirus false positives. Signing them means the Loadout identity
    vouches for that exact copy as shipped in the Loadout installer.

.DESCRIPTION
    Signing happens only when all four variables are set:

      LOADOUT_SIGN_KIT       folder of the code-signing kit checkout
      LOADOUT_SIGN_ACCOUNT   Azure Trusted Signing account name
      LOADOUT_SIGN_ENDPOINT  Trusted Signing endpoint URL
      LOADOUT_SIGN_PROFILE   certificate profile name

    They exist only in the release job and on the maintainer's signing
    machine. With any of them missing the script prints "unsigned dev build"
    and exits 0, so local and contributor builds stay unsigned.

    The kit's Invoke-TrustedSigning (code-signing-kit 1.1.0 or later) does
    the signing; this script maps the three LOADOUT_SIGN_* target values
    onto the kit's AZURE_TRUSTED_SIGNING_* names and passes the file. The
    service principal comes from AZURE_TENANT_ID, AZURE_CLIENT_ID and
    AZURE_CLIENT_SECRET in the environment: in the release job, the
    `release` environment's secrets, given to the build step only; on the
    maintainer's machine, the user variables Set-TrustedSigningCredential
    records, or a 1Password Environment the kit is pointed at with
    TRUSTED_SIGNING_OP_ENVIRONMENT. (SPEC 16.1 decision: client secret now;
    a GitHub OIDC federated credential is the documented future option.)

    This script never reads, prints or stores a credential itself.

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
Import-Module $manifest -Force -MinimumVersion 1.1.0

# The kit reads the Trusted Signing target from these names.
$env:AZURE_TRUSTED_SIGNING_ACCOUNT = $env:LOADOUT_SIGN_ACCOUNT
$env:AZURE_TRUSTED_SIGNING_ENDPOINT = $env:LOADOUT_SIGN_ENDPOINT
$env:AZURE_TRUSTED_SIGNING_PROFILE = $env:LOADOUT_SIGN_PROFILE

try {
    # The kit prints the signing tool's output and returns the signed path;
    # neither holds a credential.
    Invoke-TrustedSigning -Path $file | ForEach-Object { Write-Host $_ }
}
catch {
    Write-Error ('sign-windows: signing failed for {0}: {1}' -f (Split-Path -Leaf $file), $_.Exception.Message)
    exit 1
}

Write-Output ('signed: {0}' -f (Split-Path -Leaf $file))
exit 0

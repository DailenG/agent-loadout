<#
.SYNOPSIS
    Renders the winget manifests for DailenG.AgentLoadout from the templates
    in this folder (SPEC 16.1, Appendix C).

.DESCRIPTION
    The committed templates carry {{VERSION}} and {{INSTALLER_SHA256}}; the
    hash exists only once the signed installer is built, so the release job
    runs this script and attaches the result to the release, and the
    maintainer submits that folder to microsoft/winget-pkgs (for example with
    `wingetcreate submit <folder>`). Output goes to
    <OutDir>\manifests\d\DailenG\AgentLoadout\<Version>, the winget-pkgs
    layout.

.PARAMETER Version
    Release version without the leading v, for example 0.1.0.

.PARAMETER Installer
    Path to Loadout_<Version>_x64-setup.exe; its SHA-256 is computed here.

.PARAMETER OutDir
    Folder that receives the manifests tree.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string] $Version,
    [Parameter(Mandatory = $true)] [string] $Installer,
    [Parameter(Mandatory = $true)] [string] $OutDir
)

$ErrorActionPreference = 'Stop'

if ($Version -notmatch '^\d+\.\d+\.\d+([-+][0-9A-Za-z.-]+)?$') {
    throw ('render: not a release version: {0}' -f $Version)
}
$expected = 'Loadout_{0}_x64-setup.exe' -f $Version
if ((Split-Path -Leaf $Installer) -ne $expected) {
    throw ('render: expected the installer {0}, got {1}' -f $expected, (Split-Path -Leaf $Installer))
}
$sha256 = (Get-FileHash -LiteralPath $Installer -Algorithm SHA256).Hash.ToUpperInvariant()

$target = Join-Path $OutDir ('manifests\d\DailenG\AgentLoadout\{0}' -f $Version)
New-Item -ItemType Directory -Force -Path $target | Out-Null

$utf8 = New-Object System.Text.UTF8Encoding($false)
foreach ($template in Get-ChildItem -LiteralPath $PSScriptRoot -Filter 'DailenG.AgentLoadout*.yaml') {
    $text = [System.IO.File]::ReadAllText($template.FullName)
    $text = $text.Replace('{{VERSION}}', $Version).Replace('{{INSTALLER_SHA256}}', $sha256)
    # The template note is for this repository, not for winget-pkgs.
    $lines = $text -split "`r?`n" | Where-Object { $_ -notlike '# Template:*' }
    $out = Join-Path $target $template.Name
    [System.IO.File]::WriteAllText($out, (($lines -join "`n").TrimEnd() + "`n"), $utf8)
    Write-Output ('wrote {0}' -f $out)
}

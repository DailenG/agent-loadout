<#
.SYNOPSIS
    Builds the agent-loadout Chocolatey package for one GitHub release.

.DESCRIPTION
    Reads a release of DailenG/agent-loadout (the latest one unless -Tag is
    given), takes the installer's SHA-256 from the release's SHA256SUMS (the
    installer itself is not downloaded), copies this package folder to a
    staging folder under -OutputDirectory, fills in the version, release
    notes link, installer URL and checksum, and packs it with choco.

    The committed package files are templates (__VERSION__, __RELEASE_NOTES__,
    __URL64__, __CHECKSUM64__) and are never changed. Returns an object with
    the package Version and the Path of the .nupkg.

    Set GITHUB_TOKEN to raise the GitHub API rate limit (read access is
    enough). Needs choco on PATH.

.PARAMETER Tag
    The release tag, such as v0.8.0. Empty: the latest release.

.PARAMETER PackageVersion
    The package version. Empty: the release version. A package-only fix
    uses Chocolatey's fix version notation on top of the release version,
    such as 0.8.0.20261201.

.PARAMETER OutputDirectory
    Where the staging folder and the .nupkg go. Created when missing.
#>
[CmdletBinding()]
param(
    [string] $Tag = '',
    [string] $PackageVersion = '',
    [Parameter(Mandatory)] [string] $OutputDirectory
)

$ErrorActionPreference = 'Stop'
$repo = 'DailenG/agent-loadout'

function Invoke-GitHubApi([string] $Uri) {
    $headers = @{ 'Accept' = 'application/vnd.github+json'; 'User-Agent' = 'agent-loadout-chocolatey' }
    if ($env:GITHUB_TOKEN) { $headers['Authorization'] = 'Bearer ' + $env:GITHUB_TOKEN }
    Invoke-RestMethod -Uri $Uri -Headers $headers -UseBasicParsing
}

if ($Tag) {
    $release = Invoke-GitHubApi ('https://api.github.com/repos/{0}/releases/tags/{1}' -f $repo, $Tag)
} else {
    $release = Invoke-GitHubApi ('https://api.github.com/repos/{0}/releases/latest' -f $repo)
}
if ($release.draft -or $release.prerelease) {
    throw ('release {0} is a draft or a prerelease' -f $release.tag_name)
}
if ($release.tag_name -notmatch '^v(\d+\.\d+\.\d+)$') {
    throw ('unexpected release tag: {0}' -f $release.tag_name)
}
$version = $Matches[1]
if (-not $PackageVersion) { $PackageVersion = $version }
if ($PackageVersion -notmatch ('^{0}(\.\d+)?$' -f [regex]::Escape($version))) {
    throw ('package version {0} does not belong to release {1}' -f $PackageVersion, $version)
}

$installer = 'Loadout_{0}_x64-setup.exe' -f $version
$exe = @($release.assets | Where-Object { $_.name -eq $installer })
$sums = @($release.assets | Where-Object { $_.name -eq 'SHA256SUMS' })
if ($exe.Count -ne 1 -or $sums.Count -ne 1) {
    throw ('release {0} lacks {1} or SHA256SUMS' -f $release.tag_name, $installer)
}
$sumsText = (Invoke-WebRequest -Uri $sums[0].browser_download_url -UseBasicParsing).Content
if ($sumsText -is [byte[]]) { $sumsText = [System.Text.Encoding]::UTF8.GetString($sumsText) }
$line = @($sumsText -split "`r?`n" | Where-Object { $_ -match ('^([0-9a-fA-F]{{64}}) [ *]{0}$' -f [regex]::Escape($installer)) })
if ($line.Count -ne 1) {
    throw ('SHA256SUMS of {0} has no single line for {1}' -f $release.tag_name, $installer)
}
$checksum = ($line[0] -split ' ')[0].ToLowerInvariant()

$values = @{
    '__VERSION__'       = $PackageVersion
    '__RELEASE_NOTES__' = $release.html_url
    '__URL64__'         = $exe[0].browser_download_url
    '__CHECKSUM64__'    = $checksum
}

New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$stage = Join-Path $OutputDirectory 'agent-loadout'
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
Copy-Item -LiteralPath $PSScriptRoot -Destination $stage -Recurse
Remove-Item -LiteralPath (Join-Path $stage 'build.ps1') -Force
Get-ChildItem -LiteralPath $stage -Filter '*.nupkg' -Recurse | Remove-Item -Force

$utf8 = New-Object System.Text.UTF8Encoding($false)
foreach ($name in 'agent-loadout.nuspec', 'tools\chocolateyinstall.ps1') {
    $path = Join-Path $stage $name
    $text = [System.IO.File]::ReadAllText($path)
    foreach ($key in $values.Keys) { $text = $text.Replace($key, $values[$key]) }
    if ($text -match '__[A-Z0-9_]+__') {
        throw ('{0} still holds the placeholder {1}' -f $name, $Matches[0])
    }
    [System.IO.File]::WriteAllText($path, $text, $utf8)
}

choco pack (Join-Path $stage 'agent-loadout.nuspec') --output-directory $OutputDirectory --no-progress | Out-Host
if ($LASTEXITCODE -ne 0) { throw ('choco pack failed ({0})' -f $LASTEXITCODE) }
$nupkg = Join-Path $OutputDirectory ('agent-loadout.{0}.nupkg' -f $PackageVersion)
if (-not (Test-Path -LiteralPath $nupkg)) { throw ('choco pack wrote no {0}' -f $nupkg) }

[pscustomobject]@{ Version = $PackageVersion; Path = $nupkg }

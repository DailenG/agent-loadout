<#
.SYNOPSIS
    Chocolatey AU updater for the agent-loadout package (M25).

.DESCRIPTION
    Reads the latest GitHub release of DailenG/agent-loadout, takes the
    installer's SHA-256 from the release's SHA256SUMS (the installer itself
    is not downloaded) and, when the release is newer than the nuspec,
    rewrites the nuspec version and release notes link and the URL and
    checksum in tools\chocolateyinstall.ps1, then packs the package.

    Needs the chocolatey-au module (choco install chocolatey-au). Run it from
    any folder; it works on the package folder it lives in. Set GITHUB_TOKEN
    to raise the GitHub API rate limit (read-only access is enough).

    The Chocolatey site check is off (-NoCheckChocoVersion): the files always
    follow GitHub, and the workflow decides separately whether the version
    still needs to be pushed. -NoReadme keeps the nuspec description as
    written.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Import-Module Chocolatey-AU

$repo = 'DailenG/agent-loadout'

function Invoke-GitHubApi([string] $Uri) {
    $headers = @{ 'Accept' = 'application/vnd.github+json'; 'User-Agent' = 'agent-loadout-chocolatey-au' }
    if ($env:GITHUB_TOKEN) { $headers['Authorization'] = 'Bearer ' + $env:GITHUB_TOKEN }
    Invoke-RestMethod -Uri $Uri -Headers $headers -UseBasicParsing
}

function global:au_GetLatest {
    $release = Invoke-GitHubApi ('https://api.github.com/repos/{0}/releases/latest' -f $repo)
    if ($release.tag_name -notmatch '^v(\d+\.\d+\.\d+)$') {
        throw ('unexpected release tag: {0}' -f $release.tag_name)
    }
    $version = $Matches[1]
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

    @{
        Version        = $version
        URL64          = $exe[0].browser_download_url
        Checksum64     = $checksum
        ChecksumType64 = 'sha256'
        ReleaseNotes   = $release.html_url
    }
}

function global:au_SearchReplace {
    @{
        'tools\chocolateyinstall.ps1' = @{
            "(^[$]url64\s*=\s*)('.*')"      = "`$1'$($Latest.URL64)'"
            "(^[$]checksum64\s*=\s*)('.*')" = "`$1'$($Latest.Checksum64)'"
        }
        'agent-loadout.nuspec'        = @{
            '(<releaseNotes>).*(</releaseNotes>)' = "`${1}$($Latest.ReleaseNotes)`${2}"
        }
    }
}

function global:au_AfterUpdate($Package) {
    # AU rewrites these files with CRLF (and the .ps1 with a UTF-8 BOM).
    # Store them as git does: LF, no BOM, so the diff is only the new values
    # whatever core.autocrlf says.
    foreach ($name in 'agent-loadout.nuspec', 'tools\chocolateyinstall.ps1') {
        $path = Join-Path $Package.Path $name
        $text = [System.IO.File]::ReadAllText($path) -replace "`r`n", "`n"
        [System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding($false)))
    }
}

Push-Location $PSScriptRoot
try {
    $package = Update-Package -ChecksumFor none -NoCheckChocoVersion -NoReadme
} finally {
    Pop-Location
}
$package

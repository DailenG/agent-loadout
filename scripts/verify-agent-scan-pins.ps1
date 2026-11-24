# Checks the pinned Snyk Agent Scan release (#27, M47) with gpg: the signing
# key in fixtures/agent-scan/<version>/ has the pinned fingerprint, it signs
# sha256sums.txt.asc, and every hash compiled into
# crates/agent-loadout-core/src/scan/external/pin.rs is the signed hash of
# that asset. Run it when the pin is bumped. Loadout itself has no OpenPGP
# code and checks downloads against the compiled hashes.
#
#   pwsh -File scripts/verify-agent-scan-pins.ps1
#   pwsh -File scripts/verify-agent-scan-pins.ps1 -Download
#
# Run from the repository root. Needs gpg on PATH (Gpg4win, or the one in
# Git for Windows: C:\Program Files\Git\usr\bin).

param(
    [switch]$Download
)

$ErrorActionPreference = 'Stop'

$Fingerprint = 'EF4FF9D18D82D5629B82491D2A20F86E4EF131B1'
$Pin = 'crates/agent-loadout-core/src/scan/external/pin.rs'
$PinText = Get-Content -Raw -LiteralPath $Pin
$Version = [regex]::Match($PinText, 'pub const VERSION: &str = "([^"]+)";').Groups[1].Value
$Dir = "fixtures/agent-scan/v$Version"
$Asc = "$Dir/sha256sums.txt.asc"
$Key = "$Dir/snyk-code-signing-public.pgp"

if ($Download) {
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    Invoke-WebRequest -UseBasicParsing -OutFile $Asc `
        -Uri "https://github.com/snyk/agent-scan/releases/download/v$Version/sha256sums.txt.asc"
    Invoke-WebRequest -UseBasicParsing -OutFile $Key `
        -Uri "https://raw.githubusercontent.com/snyk/agent-scan/v$Version/help/_about-this-project/snyk-code-signing-public.pgp"
}

# A short relative folder below target/ with forward slashes: Gpg4win and
# the MSYS gpg of Git for Windows both read it (the MSYS one misreads C:/
# paths, and its agent socket needs a short path).
$HomeDir = 'target/gpg-' + [guid]::NewGuid().ToString('N').Substring(0, 8)
$Home_ = $HomeDir
New-Item -ItemType Directory -Path $Home_ | Out-Null
$Failed = $false
try {
    & gpg --homedir $HomeDir --batch --quiet --import $Key 2>$null
    if ($LASTEXITCODE -ne 0) { throw "gpg could not import $Key" }

    $Colons = & gpg --homedir $HomeDir --batch --with-colons --fingerprint
    $Primary = ($Colons | Where-Object { $_ -like 'fpr:*' } | Select-Object -First 1).Split(':')[9]
    if ($Primary -ne $Fingerprint) {
        throw "the key's fingerprint is $Primary, not the pinned $Fingerprint"
    }

    $Status = & gpg --homedir $HomeDir --batch --status-fd 1 --verify $Asc 2>$null
    if (-not ($Status | Where-Object { $_ -like "*VALIDSIG $Fingerprint *" })) {
        throw "sha256sums.txt.asc is not signed by $Fingerprint"
    }
    Write-Output "ok: sha256sums.txt.asc v$Version has a good signature from $Fingerprint"

    # The signed text only (gpg strips the armor and undoes dash-escaping).
    $Signed = & gpg --homedir $HomeDir --batch --quiet --decrypt $Asc 2>$null
    $SignedHashes = @{}
    foreach ($Line in $Signed) {
        $Parts = $Line.Trim() -split '\s+'
        if ($Parts.Count -eq 2) { $SignedHashes[$Parts[1]] = $Parts[0] }
    }

    $Pinned = [regex]::Matches($PinText, 'name: "([^"]+)",\s*sha256: "([0-9a-f]{64})"')
    if ($Pinned.Count -eq 0) { throw "no pinned assets found in $Pin" }
    foreach ($Match in $Pinned) {
        $Name = $Match.Groups[1].Value
        $Hash = $Match.Groups[2].Value
        if ($SignedHashes[$Name] -eq $Hash) {
            Write-Output "ok: $Name"
        } else {
            Write-Error "FAIL: $Name is pinned as $Hash, signed as $($SignedHashes[$Name])" -ErrorAction Continue
            $Failed = $true
        }
    }
} finally {
    Remove-Item -Recurse -Force -LiteralPath $Home_ -ErrorAction SilentlyContinue
}

if ($Failed) { exit 1 }
exit 0

<#
.SYNOPSIS
    CLI baseline (#10, docs/perf), run inside a disposable Windows Sandbox:
    generates a realistic store (200 servers, 300 skills, 20 projects,
    ~170 commits) and client files with foreign entries in the sandbox
    profile, then times common `loadout` commands.

.DESCRIPTION
    Expects the release `loadout.exe` at $Source (the sandbox maps the
    host's package folder read-only as C:\chocopkg). Everything is written
    under C:\perf and the sandbox profile; nothing is printed with user
    names or host paths. ASCII only.
#>
param(
    [string]$Source = 'C:\chocopkg\perf\loadout.exe',
    [int]$Runs = 7
)
$ErrorActionPreference = 'Stop'

$Root = 'C:\perf'
$LoadoutHome = Join-Path $Root 'lh'
$Cli = Join-Path $Root 'loadout.exe'
Remove-Item -LiteralPath $Root -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Root | Out-Null
Copy-Item -LiteralPath $Source -Destination $Cli
$env:LOADOUT_HOME = $LoadoutHome

function Invoke-Loadout([string[]]$CliArgs) {
    $out = & $Cli @CliArgs 2>&1
    if ($LASTEXITCODE -ne 0) { throw ('loadout {0} failed ({1}): {2}' -f ($CliArgs -join ' '), $LASTEXITCODE, ($out | Out-String)) }
}

function Write-Text([string]$Path, [string]$Text) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Path) | Out-Null
    [System.IO.File]::WriteAllText($Path, $Text, (New-Object System.Text.UTF8Encoding($false)))
}

function Measure-Step([string]$Label, [scriptblock]$Block) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $Block
    Write-Output ('setup: {0}: {1:N1} s' -f $Label, $sw.Elapsed.TotalSeconds)
}

# Client files with 25 foreign entries each, as on a machine in use.
$foreignJson = (0..24 | ForEach-Object { '"foreign-{0}": {{"command": "foreign-mcp", "args": ["--n", "{0}"]}}' -f $_ }) -join ",`n    "
$foreignToml = (0..24 | ForEach-Object { "[mcp_servers.foreign-$_]`ncommand = `"foreign-mcp`"`nargs = [`"--n`", `"$_`"]`n" }) -join "`n"
$home_ = $env:USERPROFILE
Write-Text (Join-Path $home_ '.claude.json') ("{`n  `"mcpServers`": {`n    $foreignJson`n  }`n}`n")
Write-Text (Join-Path $home_ '.cursor\mcp.json') ("{`n  `"mcpServers`": {`n    $foreignJson`n  }`n}`n")
Write-Text (Join-Path $home_ '.gemini\settings.json') ("{`n  `"mcpServers`": {`n    $foreignJson`n  }`n}`n")
Write-Text (Join-Path $home_ '.codex\config.toml') $foreignToml
$clients = @('claude-code', 'cursor', 'gemini', 'codex')

function Get-ServerToml([int]$i, [string]$Description) {
    $list = @()
    for ($c = 0; $c -lt $clients.Count; $c++) { if ((($i + $c) % 4) -ne 0) { $list += ('"{0}"' -f $clients[$c]) } }
    return ("schema = 1`nid = `"server-{0:D3}`"`ndescription = `"{1}`"`ntransport = `"stdio`"`ncommand = `"npx`"`nargs = [`"-y`", `"@perf/server-{0}`"]`n`n[env]`nLOG_LEVEL = `"info`"`n`n[enable]`nclients = [{2}]`n" -f $i, $Description, ($list -join ', '))
}

Write-Output ('== machine: {0} logical cores' -f [Environment]::ProcessorCount)
Measure-Step 'store + 200 servers (hand edit, backup, apply)' {
    Invoke-Loadout @('server', 'add', 'server-000', '--client', 'claude-code', '--', 'npx', '-y', '@perf/server-0')
    $servers = Join-Path $LoadoutHome 'store\servers'
    for ($i = 0; $i -lt 200; $i++) { Write-Text (Join-Path $servers ('server-{0:D3}.toml' -f $i)) (Get-ServerToml $i 'initial') }
    Invoke-Loadout @('backup', '-m', 'perf: 200 servers')
    Invoke-Loadout @('apply')
}
Measure-Step '150 edit commits' {
    $servers = Join-Path $LoadoutHome 'store\servers'
    for ($e = 0; $e -lt 150; $e++) {
        $i = $e % 200
        Write-Text (Join-Path $servers ('server-{0:D3}.toml' -f $i)) (Get-ServerToml $i ('edit {0}' -f $e))
        Invoke-Loadout @('backup', '-m', ('perf: edit {0}' -f $e))
    }
    Invoke-Loadout @('apply')
}
Measure-Step '300 skills (skills add)' {
    for ($i = 0; $i -lt 300; $i++) {
        $dir = Join-Path $Root ('skills\skill-{0:D3}' -f $i)
        Write-Text (Join-Path $dir 'SKILL.md') ("---`nname: skill-{0:D3}`ndescription: Synthetic skill {0} for the performance baseline.`n---`n`n# Skill {0}`n`n{1}" -f $i, ('Some instructions for the agent.' + "`n") * 40)
        Write-Text (Join-Path $dir 'scripts\check.sh') "echo ok`n"
        Invoke-Loadout @('skills', 'add', $dir)
    }
}
Measure-Step 'enable 100 skills for claude-code' {
    for ($i = 0; $i -lt 300; $i += 3) { Invoke-Loadout @('skills', 'enable', ('skill-{0:D3}' -f $i), '--client', 'claude-code') }
}
Measure-Step '20 projects (add + trust)' {
    for ($i = 0; $i -lt 20; $i++) {
        $dir = Join-Path $Root ('projects\project-{0}' -f $i)
        $text = "schema = 1`nid = `"proj-{0:D2}-perf`"`nname = `"project-{0}`"`n`n[defaults]`nclients = [`"claude-code`", `"cursor`"]`n" -f $i
        for ($n = 0; $n -lt 5; $n++) { $text += "`n[servers.local-$n]`ntransport = `"stdio`"`ncommand = `"project-mcp-$n`"`n" }
        Write-Text (Join-Path $dir '.loadout\project.toml') $text
        Invoke-Loadout @('project', 'add', $dir)
        if (($i % 5) -ne 4) { Invoke-Loadout @('--yes', 'project', 'trust', $dir) }
    }
    Invoke-Loadout @('apply')
}

function Measure-Cli([string]$Label, [string[]]$CliArgs, [string]$Cwd = $Root) {
    Push-Location $Cwd
    try {
        $times = @()
        $code = 0
        for ($i = 0; $i -lt $Runs; $i++) {
            $sw = [System.Diagnostics.Stopwatch]::StartNew()
            $null = & $Cli @CliArgs 2>&1
            $times += $sw.Elapsed.TotalMilliseconds
            $code = $LASTEXITCODE
        }
        $sorted = $times | Sort-Object
        Write-Output ('| {0} | {1:N1} | {2:N1} | {3:N1} | {4} |' -f $Label, $sorted[0], $sorted[[int]($Runs / 2)], $sorted[-1], $code)
    } finally { Pop-Location }
}

Write-Output ''
Write-Output '| Command | min ms | median ms | max ms | exit |'
Write-Output '|---|---|---|---|---|'
Measure-Cli 'loadout --version' @('--version')
Measure-Cli 'loadout status' @('status')
Measure-Cli 'loadout status --json' @('--json', 'status')
Measure-Cli 'loadout effective (project folder)' @('effective') (Join-Path $Root 'projects\project-0')
Measure-Cli 'loadout effective (no project)' @('effective')
Measure-Cli 'loadout diff (apply dry run)' @('diff')
Measure-Cli 'loadout server ls' @('server', 'ls')
Measure-Cli 'loadout skills ls' @('skills', 'ls')
Measure-Cli 'loadout history' @('history')
Measure-Cli 'loadout project ls' @('project', 'ls')
Measure-Cli 'loadout apply (nothing to do)' @('apply')

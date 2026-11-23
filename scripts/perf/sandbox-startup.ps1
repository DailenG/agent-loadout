<#
.SYNOPSIS
    Real-app startup and idle baseline (#10, docs/perf), run inside a
    disposable Windows Sandbox. Installs the NSIS installer silently,
    measures cold and warm start to the main window being shown, samples
    CPU and memory of the app and its WebView2 processes per process: a
    10 s series over the first launch (when the WebView2 runtime downloads
    its components into the new profile), then window focused, visible
    behind another window and minimized, and again once settled. Lists the
    WebView2 profile folders with their sizes, times a few CLI commands,
    then uninstalls.

.DESCRIPTION
    Expects the installer at $Installer (the sandbox maps the host's
    package folder read-only as C:\chocopkg). Prints a plain-text report;
    no user names or absolute host paths are printed. ASCII only.

    The app ignores WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS (Tauri passes its
    own browser arguments), so the DevTools protocol cannot be reached in
    the release build; the renderer's own idle work is measured on the mock
    bundle in Chromium instead (docs/perf).
#>
param(
    [string]$Installer = 'C:\chocopkg\perf\Loadout_0.1.3_x64-setup.exe',
    [int]$SeriesSeconds = 180,
    [int]$IdleSeconds = 60,
    [int]$ShortSeconds = 30,
    [int]$WarmRuns = 3
)
$ErrorActionPreference = 'Stop'

Add-Type -Namespace PerfWin -Name Native -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr hWnd, int nCmdShow);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr hWnd);
'@

$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\Loadout'
$App = Join-Path $InstallDir 'agent-loadout-app.exe'
$Cli = Join-Path $InstallDir 'loadout.exe'

function Get-Tree([int]$RootId) {
    $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name
    $ids = New-Object System.Collections.Generic.List[int]
    $ids.Add($RootId)
    $added = $true
    while ($added) {
        $added = $false
        foreach ($p in $all) {
            if ($ids.Contains([int]$p.ParentProcessId) -and -not $ids.Contains([int]$p.ProcessId)) {
                $ids.Add([int]$p.ProcessId)
                $added = $true
            }
        }
    }
    return $ids
}

# Label of a process: its name, plus the Chromium --type (and utility
# sub-type) for WebView2 children.
function Get-Labels([int]$RootId) {
    $labels = @{}
    foreach ($p in (Get-CimInstance Win32_Process)) {
        $labels[[int]$p.ProcessId] = $p
    }
    $result = @{}
    foreach ($id in (Get-Tree $RootId)) {
        $cim = $labels[$id]
        if (-not $cim) { continue }
        $label = $cim.Name
        if ($cim.CommandLine -match '--type=([a-z-]+)') { $label = $label + ' ' + $Matches[1] }
        if ($cim.CommandLine -match '--utility-sub-type=([A-Za-z.]+)') { $label = $label + ' (' + $Matches[1] + ')' }
        $result[$id] = $label
    }
    return $result
}

function Get-TreeSample([int]$RootId) {
    $cpu = 0.0
    $ws = 0
    $priv = 0
    $count = 0
    $byId = @{}
    foreach ($id in (Get-Tree $RootId)) {
        $p = Get-Process -Id $id -ErrorAction SilentlyContinue
        if ($p) {
            $cpu += $p.TotalProcessorTime.TotalMilliseconds
            $ws += $p.WorkingSet64
            $priv += $p.PrivateMemorySize64
            $count += 1
            $byId[$id] = $p.TotalProcessorTime.TotalMilliseconds
        }
    }
    return [pscustomobject]@{ CpuMs = $cpu; WorkingSet = $ws; Private = $priv; Processes = $count; ById = $byId }
}

function Start-Measured([string]$Label) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $App -PassThru
    $shown = $null
    while ($sw.Elapsed.TotalSeconds -lt 60) {
        $p.Refresh()
        if ($p.MainWindowHandle -ne [System.IntPtr]::Zero -and [PerfWin.Native]::IsWindowVisible($p.MainWindowHandle)) {
            $shown = $sw.Elapsed.TotalMilliseconds
            break
        }
        Start-Sleep -Milliseconds 10
    }
    # The WebView2 children start after the window; record when they exist.
    $webview = $null
    while ($sw.Elapsed.TotalSeconds -lt 60) {
        $kids = (Get-Tree $p.Id).Count - 1
        if ($kids -ge 3) { $webview = $sw.Elapsed.TotalMilliseconds; break }
        Start-Sleep -Milliseconds 50
    }
    # Host output only: the caller keeps the process object.
    Write-Host ('{0}: window shown {1:N0} ms; WebView2 processes up {2:N0} ms' -f $Label, $shown, $webview)
    return $p
}

function Stop-App($p) {
    $null = $p.CloseMainWindow()
    if (-not $p.WaitForExit(10000)) { Stop-Process -Id $p.Id -Force }
    Get-Process -Name 'agent-loadout-app' -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Seconds 3
}

function Measure-Idle($p, [int]$Seconds, [string]$Label) {
    $first = Get-TreeSample $p.Id
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $peakWs = 0
    $last = $first
    while ($sw.Elapsed.TotalSeconds -lt $Seconds) {
        Start-Sleep -Seconds 1
        $last = Get-TreeSample $p.Id
        if ($last.WorkingSet -gt $peakWs) { $peakWs = $last.WorkingSet }
    }
    $elapsed = $sw.Elapsed.TotalSeconds
    $cpuMs = $last.CpuMs - $first.CpuMs
    $cores = [Environment]::ProcessorCount
    Write-Output ('{0} ({1:N1} s): CPU {2:N0} ms total = {3:N2} % of one core; working set start {4:N1} MB, end {5:N1} MB, peak {6:N1} MB; private end {7:N1} MB; processes {8}; cores {9}' -f `
        $Label, $elapsed, $cpuMs, ($cpuMs / ($elapsed * 10.0)), ($first.WorkingSet / 1MB), ($last.WorkingSet / 1MB), ($peakWs / 1MB), ($last.Private / 1MB), $last.Processes, $cores)
    $labels = Get-Labels $p.Id
    foreach ($id in $last.ById.Keys) {
        $before = 0.0
        if ($first.ById.ContainsKey($id)) { $before = $first.ById[$id] }
        $delta = $last.ById[$id] - $before
        Write-Output ('    {0}: {1:N0} ms = {2:N2} % of one core' -f $labels[$id], $delta, ($delta / ($elapsed * 10.0)))
    }
}

function Measure-Cli([string[]]$CliArgs, [int]$Runs = 7) {
    $times = @()
    for ($i = 0; $i -lt $Runs; $i++) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $null = & $Cli @CliArgs 2>&1
        $times += $sw.Elapsed.TotalMilliseconds
    }
    $sorted = $times | Sort-Object
    Write-Output ('loadout {0}: min {1:N1} ms, median {2:N1} ms, max {3:N1} ms (exit {4})' -f ($CliArgs -join ' '), $sorted[0], $sorted[[int]($Runs / 2)], $sorted[-1], $LASTEXITCODE)
}

# Per-process CPU in 10 s buckets; only processes above 0.05 % are listed.
function Write-Series($p, [int]$Seconds, [string]$Label) {
    $size = 10
    Write-Output ('{0}: per-process CPU, % of one core per {1} s' -f $Label, $size)
    $prev = Get-TreeSample $p.Id
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    for ($b = 1; $b -le [int]($Seconds / $size); $b++) {
        while ($sw.Elapsed.TotalSeconds -lt ($b * $size)) { Start-Sleep -Milliseconds 500 }
        $cur = Get-TreeSample $p.Id
        $labels = Get-Labels $p.Id
        $parts = @()
        foreach ($id in $cur.ById.Keys) {
            $before = 0.0
            if ($prev.ById.ContainsKey($id)) { $before = $prev.ById[$id] }
            $pct = ($cur.ById[$id] - $before) / ($size * 10.0)
            if ($pct -ge 0.05) { $parts += ('{0} {1:N2}' -f ($labels[$id] -replace 'msedgewebview2.exe', 'wv2' -replace 'agent-loadout-app.exe', 'app'), $pct) }
        }
        Write-Output ('  t={0,4}s total {1,6:N2} | {2}' -f ($b * $size), (($cur.CpuMs - $prev.CpuMs) / ($size * 10.0)), ($parts -join '; '))
        $prev = $cur
    }
}

# Top-level folders of the app's WebView2 profile, oldest first: the
# runtime's component updater fills it during the first minutes.
function Write-ProfileFolders {
    foreach ($eb in (Get-ChildItem -Path $env:LOCALAPPDATA -Directory -ErrorAction SilentlyContinue | ForEach-Object { Join-Path $_.FullName 'EBWebView' } | Where-Object { Test-Path $_ })) {
        Write-Output ('WebView2 profile of {0}:' -f (Split-Path -Leaf (Split-Path -Parent $eb)))
        Get-ChildItem -LiteralPath $eb -Directory | Sort-Object LastWriteTime | ForEach-Object {
            $bytes = (Get-ChildItem -LiteralPath $_.FullName -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum
            Write-Output ('  {0}  {1,10:N0} KB  {2}' -f $_.LastWriteTime.ToString('HH:mm:ss'), ($bytes / 1KB), $_.Name)
        }
    }
}

Write-Output ('== machine: {0} logical cores, {1:N1} GB RAM, {2}' -f [Environment]::ProcessorCount, ((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB), (Get-CimInstance Win32_OperatingSystem).Caption)
$wv = Get-ItemProperty -Path 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue
Write-Output ('WebView2 runtime before install: {0}' -f $(if ($wv) { $wv.pv } else { 'not registered' }))
$video = @(Get-CimInstance Win32_VideoController | ForEach-Object { $_.Name }) -join '; '
Write-Output ('display adapters: {0}' -f $video)

# A repeat run in the same sandbox starts from a new WebView2 profile too.
Remove-Item -LiteralPath (Join-Path $env:LOCALAPPDATA 'dev.dailen.loadout') -Recurse -Force -ErrorAction SilentlyContinue

Write-Output '== install'
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$inst = Start-Process -FilePath $Installer -ArgumentList '/S' -PassThru -Wait
Write-Output ('installer exit {0} in {1:N1} s; app present: {2}; app size {3:N1} MB; cli size {4:N1} MB' -f $inst.ExitCode, $sw.Elapsed.TotalSeconds, (Test-Path $App), ((Get-Item $App).Length / 1MB), ((Get-Item $Cli).Length / 1MB))
Get-Process -Name 'agent-loadout-app' -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 5

Write-Output '== startup and idle'
$p = Start-Measured 'cold start (first run after install, new store)'
[PerfWin.Native]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
Write-Series $p $SeriesSeconds 'first launch, focused'
# A small console window takes the focus; the app stays visible behind it.
$other = Start-Process -FilePath 'cmd.exe' -ArgumentList '/k', 'title perf-focus-thief' -PassThru
Start-Sleep -Seconds 2
Measure-Idle $p $ShortSeconds 'idle, visible, another window focused'
Stop-Process -Id $other.Id -Force -ErrorAction SilentlyContinue
[PerfWin.Native]::ShowWindow($p.MainWindowHandle, 6) | Out-Null
Start-Sleep -Seconds 2
Measure-Idle $p $ShortSeconds 'idle, minimized'
Stop-App $p

for ($i = 1; $i -le $WarmRuns; $i++) {
    $p = Start-Measured ('warm start {0}' -f $i)
    Start-Sleep -Seconds 4
    $s = Get-TreeSample $p.Id
    Write-Output ('  after 4 s: working set {0:N1} MB, private {1:N1} MB, processes {2}' -f ($s.WorkingSet / 1MB), ($s.Private / 1MB), $s.Processes)
    Stop-App $p
}

Write-Output '== settled idle (a later launch)'
$p = Start-Measured 'launch'
Start-Sleep -Seconds 10
[PerfWin.Native]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
Measure-Idle $p $IdleSeconds 'idle, window open and focused'
[PerfWin.Native]::ShowWindow($p.MainWindowHandle, 6) | Out-Null
Start-Sleep -Seconds 2
Measure-Idle $p $ShortSeconds 'idle, minimized'
Stop-App $p

Write-Output '== WebView2 profile'
Write-ProfileFolders

Write-Output '== CLI (empty store created by the app)'
Measure-Cli @('--version')
Measure-Cli @('status')
Measure-Cli @('effective')

Write-Output '== uninstall'
$un = Join-Path $InstallDir 'uninstall.exe'
$u = Start-Process -FilePath $un -ArgumentList '/S' -PassThru -Wait
Start-Sleep -Seconds 5
Write-Output ('uninstaller exit {0}; app present after: {1}' -f $u.ExitCode, (Test-Path $App))

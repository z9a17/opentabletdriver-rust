<#
.SYNOPSIS
Runs the performance measurements (F04) and writes structured results.

.DESCRIPTION
Builds the Rust harness (examples/bench) and the upstream harness
(bench/upstream), exports one workload, and runs both harnesses on it
alternately, several times. Each run writes JSON; scripts/bench-summary.py
combines them into summary.md. See docs/PERFORMANCE.md.

-SendInput and -ReplaySeconds move the cursor (never clicking). -Idle starts
this driver, its panel, and upstream's daemon and UX in turn, and samples
their CPU time, context switches and memory; it refuses to run while any of
them is already running, and stops only the processes it started.
#>
[CmdletBinding()]
param(
    [string]$OutputDirectory,
    # A clean OpenTabletDriver checkout at the pinned revision.
    [string]$UpstreamRoot = 'target/upstream/OpenTabletDriver',
    # The unchanged RadialFollow 0.3.0 DLL; see docs/parity/VALIDATION.md.
    [string]$RadialFollow = 'target/radialfollow-test/RadialFollow/RadialFollow.dll',
    # 0 skips the harnesses, for example to sample idle processes only.
    [ValidateRange(0, 20)][int]$Runs = 3,
    [ValidateRange(1, 50)][int]$Rounds = 7,
    [ValidateRange(1000, 1000000)][int]$Reports = 20000,
    [switch]$SendInput,
    [ValidateRange(0, 600)][double]$ReplaySeconds = 0,
    [switch]$Idle,
    # The folder holding upstream's OpenTabletDriver.Daemon.exe, for -Idle.
    [string]$UpstreamInstall,
    [ValidateRange(10, 600)][int]$IdleSeconds = 60,
    # Runs only harness cases whose names contain this text; the replay always runs.
    [string]$Only,
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Native {
    param([string]$FilePath, [string[]]$Arguments)
    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$FilePath $($Arguments -join ' ') failed with exit code $LASTEXITCODE" }
}

Add-Type -Namespace OtdBench -Name Native -MemberDefinition @'
[DllImport("kernel32.dll", SetLastError = true)]
public static extern bool QueryProcessCycleTime(IntPtr process, out ulong cycles);
[DllImport("kernel32.dll")]
public static extern bool QueryThreadCycleTime(IntPtr thread, out ulong cycles);
[DllImport("kernel32.dll")]
public static extern IntPtr GetCurrentThread();
'@

# Cycles per second counted for a running thread: the median of three 200 ms spins.
function Get-CycleRate {
    $rates = foreach ($attempt in 1..3) {
        $thread = [OtdBench.Native]::GetCurrentThread()
        $before = [uint64]0
        $after = [uint64]0
        [void][OtdBench.Native]::QueryThreadCycleTime($thread, [ref]$before)
        $clock = [Diagnostics.Stopwatch]::StartNew()
        while ($clock.ElapsedMilliseconds -lt 200) { }
        [void][OtdBench.Native]::QueryThreadCycleTime($thread, [ref]$after)
        ($after - $before) / $clock.Elapsed.TotalSeconds
    }
    ($rates | Sort-Object)[1]
}

function Get-ProcessSample {
    param([System.Diagnostics.Process]$Process)
    $Process.Refresh()
    $cycles = [uint64]0
    if (-not [OtdBench.Native]::QueryProcessCycleTime($Process.Handle, [ref]$cycles)) {
        throw "QueryProcessCycleTime failed for $($Process.ProcessName)"
    }
    # Raw thread counters hold each thread's cumulative context switches.
    $switches = @{}
    foreach ($thread in Get-CimInstance Win32_PerfRawData_PerfProc_Thread -Filter "IDProcess = $($Process.Id)" -ErrorAction Stop) {
        $switches[[int]$thread.IDThread] = [double]$thread.ContextSwitchesPersec
    }
    [pscustomobject]@{
        Name = $Process.ProcessName
        Cycles = $cycles
        Switches = $switches
        WorkingSet = $Process.WorkingSet64
        Private = $Process.PrivateMemorySize64
        Handles = $Process.HandleCount
        Threads = $Process.Threads.Count
        Stamp = [Diagnostics.Stopwatch]::GetTimestamp()
    }
}

# Context switches between two samples. A thread that appeared, or whose ID a
# new thread reused, counts from zero; switches of threads that exited in
# between are missed.
function Get-SwitchCount {
    param([hashtable]$Before, [hashtable]$After)
    $total = 0.0
    foreach ($id in $After.Keys) {
        $previous = if ($Before.ContainsKey($id)) { $Before[$id] } else { 0.0 }
        $total += if ($After[$id] -ge $previous) { $After[$id] - $previous } else { $After[$id] }
    }
    $total
}

function Measure-Idle {
    param([string]$Scenario, [scriptblock]$Start, [double]$CycleRate)
    Write-Information "idle: $Scenario" -InformationAction Continue
    $processes = @(& $Start)
    try {
        Start-Sleep -Seconds 10
        $before = @($processes | ForEach-Object { Get-ProcessSample $_ })
        Start-Sleep -Seconds $IdleSeconds
        $after = @($processes | ForEach-Object { Get-ProcessSample $_ })
        for ($i = 0; $i -lt $processes.Count; $i++) {
            $seconds = ($after[$i].Stamp - $before[$i].Stamp) / [Diagnostics.Stopwatch]::Frequency
            [ordered]@{
                scenario = $Scenario
                process = $after[$i].Name
                seconds = [math]::Round($seconds, 1)
                cpu_percent_of_one_core = [math]::Round(($after[$i].Cycles - $before[$i].Cycles) / $CycleRate / $seconds * 100, 4)
                context_switches_per_s = [math]::Round((Get-SwitchCount $before[$i].Switches $after[$i].Switches) / $seconds, 1)
                working_set_mb = [math]::Round($after[$i].WorkingSet / 1MB, 1)
                private_mb = [math]::Round($after[$i].Private / 1MB, 1)
                handles = $after[$i].Handles
                threads = $after[$i].Threads
            }
        }
    } finally {
        foreach ($process in $processes) {
            if (-not $process.HasExited) { Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue }
        }
        Start-Sleep -Seconds 2
    }
}

$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $projectRoot
try {
    if (-not $OutputDirectory) { $OutputDirectory = Join-Path 'target/bench' (Get-Date -Format 'yyyyMMdd-HHmmss') }
    New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
    $inventory = Get-Content -LiteralPath 'docs/parity/upstream-inventory.json' -Raw | ConvertFrom-Json
    $pinned = $inventory.upstream.revision
    if (-not (Test-Path -LiteralPath $UpstreamRoot)) {
        throw "No upstream checkout at $UpstreamRoot. Clone OpenTabletDriver at $pinned there, or pass -UpstreamRoot."
    }
    $upstreamRevision = (git -C $UpstreamRoot rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) { throw "Cannot read the upstream checkout's revision" }
    if ($upstreamRevision -ne $pinned) { throw "Upstream checkout is at $upstreamRevision; the pinned revision is $pinned" }
    $radialFollowPath = if ($RadialFollow -and (Test-Path -LiteralPath $RadialFollow)) { (Resolve-Path -LiteralPath $RadialFollow).Path } else { $null }
    if (-not $radialFollowPath) { Write-Warning "RadialFollow.dll not found; the managed and osu! profile cases are skipped." }

    if (-not $SkipBuild) {
        Invoke-Native cargo @('build', '--locked', '--workspace', '--release')
        Invoke-Native cargo @('build', '--locked', '--release', '--example', 'bench')
        & (Join-Path $PSScriptRoot 'build-compat.ps1')
        Invoke-Native dotnet @('build', 'bench/upstream/OtdUpstreamBench.csproj', '-c', 'Release', "-p:OtdRoot=$((Resolve-Path -LiteralPath $UpstreamRoot).Path)", '-o', 'target/bench/upstream-bin', '--nologo', '-v', 'quiet')
    }
    $rustBench = 'target/release/examples/bench.exe'
    $upstreamBench = 'target/bench/upstream-bin/OtdUpstreamBench.dll'
    $workload = Join-Path $OutputDirectory 'workload'
    Invoke-Native $rustBench @('--reports', "$Reports", '--export-workload', $workload)

    $commit = (git rev-parse HEAD).Trim()
    $dirty = [bool](git status --porcelain --untracked-files=no)
    $os = Get-CimInstance Win32_OperatingSystem
    $cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
    $environment = [ordered]@{
        date = (Get-Date).ToString('o')
        commit = $commit
        dirty_worktree = $dirty
        upstream_revision = $upstreamRevision
        os = "$($os.Caption) $($os.Version) build $($os.BuildNumber)"
        cpu = $cpu.Name.Trim()
        logical_cpus = [Environment]::ProcessorCount
        memory_gb = [math]::Round($os.TotalVisibleMemorySize / 1MB, 1)
        power_plan = ((powercfg /getactivescheme) -join ' ').Trim()
        rustc = ((rustc --version) -join ' ').Trim()
        dotnet_runtimes = @(dotnet --list-runtimes | Where-Object { $_ -like 'Microsoft.NETCore.App *' } | ForEach-Object { ($_ -split ' ')[1] })
        displays = @(& 'target/release/opentabletdriver-rust.exe' displays)
        options = [ordered]@{ runs = $Runs; rounds = $Rounds; reports = $Reports; send_input = [bool]$SendInput; replay_seconds = $ReplaySeconds; idle_seconds = $(if ($Idle) { $IdleSeconds } else { $null }) }
    }
    $environment | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'environment.json') -Encoding utf8

    $env:OTD_BENCH_COMMIT = $commit
    $env:OTD_UPSTREAM_COMMIT = $upstreamRevision
    $compat = (Resolve-Path -LiteralPath 'target/release/compat').Path
    for ($run = 1; $run -le $Runs; $run++) {
        Write-Information "run $run of ${Runs}: Rust harness" -InformationAction Continue
        $arguments = @('--reports', "$Reports", '--rounds', "$Rounds", '--compat', $compat, '--out', (Join-Path $OutputDirectory "rust-$run.json"))
        if ($radialFollowPath) { $arguments += @('--radialfollow', $radialFollowPath) }
        if ($SendInput) { $arguments += '--send-input' }
        if ($ReplaySeconds -gt 0) { $arguments += @('--replay-seconds', "$ReplaySeconds") }
        if ($Only) { $arguments += @('--only', $Only) }
        Invoke-Native $rustBench $arguments

        Write-Information "run $run of ${Runs}: upstream harness" -InformationAction Continue
        $arguments = @($upstreamBench, '--workload', (Join-Path $workload 'workload.json'), '--rounds', "$Rounds", '--out', (Join-Path $OutputDirectory "upstream-$run.json"))
        if ($radialFollowPath) { $arguments += @('--radialfollow', $radialFollowPath) }
        if ($SendInput) { $arguments += '--send-input' }
        if ($ReplaySeconds -gt 0 -and $radialFollowPath) { $arguments += @('--replay-seconds', "$ReplaySeconds") }
        if ($Only) { $arguments += @('--only', $Only) }
        Invoke-Native dotnet $arguments
    }

    if ($Idle) {
        $names = @('opentabletdriver-rust', 'opentabletdriver-rust-ui', 'OpenTabletDriver.Daemon', 'OpenTabletDriver.UX.Wpf')
        $running = @(Get-Process -Name $names -ErrorAction SilentlyContinue)
        if ($running.Count -gt 0) { throw "Close $($running.ProcessName -join ', ') before sampling idle processes." }
        $results = @()
        $cycleRate = Get-CycleRate
        $profilePath = (Resolve-Path -LiteralPath (Join-Path $workload 'osu-profile.toml')).Path
        $driver = (Resolve-Path -LiteralPath 'target/release/opentabletdriver-rust.exe').Path
        $panel = (Resolve-Path -LiteralPath 'target/release/opentabletdriver-rust-ui.exe').Path
        $results += Measure-Idle -CycleRate $cycleRate -Scenario 'rust-daemon' -Start { Start-Process -FilePath $driver -ArgumentList @('run', '--config', "`"$profilePath`"") -WindowStyle Minimized -PassThru }
        $results += Measure-Idle -CycleRate $cycleRate -Scenario 'rust-panel' -Start { Start-Process -FilePath $panel -PassThru }
        if ($UpstreamInstall) {
            $daemon = Join-Path $UpstreamInstall 'OpenTabletDriver.Daemon.exe'
            $ux = Join-Path $UpstreamInstall 'OpenTabletDriver.UX.Wpf.exe'
            $results += Measure-Idle -CycleRate $cycleRate -Scenario 'upstream-daemon' -Start { Start-Process -FilePath $daemon -WindowStyle Minimized -PassThru }
            $results += Measure-Idle -CycleRate $cycleRate -Scenario 'upstream-daemon+ux' -Start {
                $started = Start-Process -FilePath $daemon -WindowStyle Minimized -PassThru
                Start-Sleep -Seconds 3
                $started
                Start-Process -FilePath $ux -PassThru
            }
        }
        $results | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'idle.json') -Encoding utf8
    }

    Invoke-Native python @('scripts/bench-summary.py', $OutputDirectory)
    Write-Output (Join-Path $OutputDirectory 'summary.md')
} finally { Pop-Location }

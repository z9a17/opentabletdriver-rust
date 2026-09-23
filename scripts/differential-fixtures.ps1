<#
.SYNOPSIS
Regenerates the differential fixtures in tests/differential (F05).

.DESCRIPTION
Builds the Rust harness (examples/bench) and the upstream harness
(bench/upstream), writes fixture skeletons from the Rust harness, and fills
their expected outputs by running OpenTabletDriver's own pipeline from a clean
checkout at the pinned revision. Nothing moves the cursor or opens a window.
See tests/differential/README.md.
#>
[CmdletBinding()]
param(
    # A clean OpenTabletDriver checkout at the pinned revision.
    [string]$UpstreamRoot = 'target/upstream/OpenTabletDriver',
    # The unchanged RadialFollow 0.3.0 DLL; see docs/parity/VALIDATION.md.
    [string]$RadialFollow = 'target/radialfollow-test/RadialFollow/RadialFollow.dll',
    [ValidateRange(100, 20000)][int]$Reports = 2000
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Native {
    param([string]$FilePath, [string[]]$Arguments)
    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$FilePath $($Arguments -join ' ') failed with exit code $LASTEXITCODE" }
}

$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $projectRoot
try {
    $inventory = Get-Content -LiteralPath 'docs/parity/upstream-inventory.json' -Raw | ConvertFrom-Json
    $pinned = $inventory.upstream.revision
    if (-not (Test-Path -LiteralPath $UpstreamRoot)) {
        throw "No upstream checkout at $UpstreamRoot. Clone OpenTabletDriver at $pinned there, or pass -UpstreamRoot."
    }
    $upstreamRevision = (git -C $UpstreamRoot rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) { throw "Cannot read the upstream checkout's revision" }
    if ($upstreamRevision -ne $pinned) { throw "Upstream checkout is at $upstreamRevision; the pinned revision is $pinned" }
    if (-not (Test-Path -LiteralPath $RadialFollow)) { throw "RadialFollow.dll not found at $RadialFollow" }

    Invoke-Native cargo @('build', '--locked', '--release', '--example', 'bench')
    Invoke-Native dotnet @('build', 'bench/upstream/OtdUpstreamBench.csproj', '-c', 'Release', "-p:OtdRoot=$((Resolve-Path -LiteralPath $UpstreamRoot).Path)", '-o', 'target/bench/upstream-bin', '--nologo', '-v', 'quiet')
    Invoke-Native 'target/release/examples/bench.exe' @('--reports', "$Reports", '--export-differential', 'tests/differential')
    $env:OTD_UPSTREAM_COMMIT = $upstreamRevision
    $fixtures = @('osu-trace.json', 'edges.json', 'thresholds.json') | ForEach-Object { @('--reference', "tests/differential/$_") }
    Invoke-Native dotnet (@('target/bench/upstream-bin/OtdUpstreamBench.dll') + $fixtures + @('--radialfollow', (Resolve-Path -LiteralPath $RadialFollow).Path))
    Write-Output 'tests/differential regenerated; review the diff and run: cargo test --locked -p otd-core differential'
} finally { Pop-Location }

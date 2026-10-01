[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $projectRoot
try {
    # Use the same layout, dependency inventory and provenance as published builds.
    python (Join-Path $PSScriptRoot 'release.py') build --platform win-x64 --rust-target x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Windows release packaging failed' }
} finally { Pop-Location }

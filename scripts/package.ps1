[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $projectRoot
try {
    cargo build --locked --workspace --release
    if ($LASTEXITCODE -ne 0) { throw 'Rust release build failed' }
    & (Join-Path $PSScriptRoot 'build-compat.ps1')
    $metadataText = cargo metadata --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read package version' }
    $metadata = $metadataText | ConvertFrom-Json
    $version = ($metadata.packages | Where-Object name -EQ 'opentabletdriver-rust').version
    $packageName = "opentabletdriver-rust-v$version-win-x64"
    $stage = Join-Path $projectRoot ("target/package-" + [guid]::NewGuid().ToString('N'))
    $package = Join-Path $stage $packageName
    New-Item -ItemType Directory -Path $package -ErrorAction Stop | Out-Null
    foreach ($file in @('opentabletdriver-rust.exe', 'opentabletdriver-rust-ui.exe', 'otd_ema_filter.dll')) {
        Copy-Item -LiteralPath (Join-Path 'target/release' $file) -Destination $package -ErrorAction Stop
    }
    Copy-Item -LiteralPath 'target/release/compat' -Destination $package -Recurse -ErrorAction Stop
    foreach ($file in @('README.md', 'AGENTS.md', 'LICENSE', 'LICENSE.LGPL-3.0', 'NOTICE.md', 'driver.example.toml', 'driver.relative.example.toml', 'driver.plugins.example.toml', 'docs')) {
        Copy-Item -LiteralPath $file -Destination $package -Recurse -ErrorAction Stop
    }
    $archive = Join-Path $projectRoot "target/$packageName.zip"
    Compress-Archive -LiteralPath $package -DestinationPath $archive -Force -ErrorAction Stop
    $hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    [System.IO.File]::WriteAllText("$archive.sha256", "$hash  $packageName.zip`n", [System.Text.UTF8Encoding]::new($false))
    Write-Output $archive
    Write-Output "SHA256: $hash"
} finally { Pop-Location }

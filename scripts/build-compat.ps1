[CmdletBinding()]
param([string]$OutputDirectory = 'target/release/compat')
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $projectRoot
try {
    dotnet restore compat/OtdCompat --locked-mode --nologo
    if ($LASTEXITCODE -ne 0) { throw 'Compatibility bridge restore failed' }
    dotnet build compat/OtdCompat -c Release --no-restore -o $OutputDirectory --nologo
    if ($LASTEXITCODE -ne 0) { throw 'Compatibility bridge build failed' }
    $dotnetDirectory = Split-Path -Parent (Get-Command dotnet -ErrorAction Stop).Source
    $hostPacks = Join-Path $dotnetDirectory 'packs/Microsoft.NETCore.App.Host.win-x64'
    $hostVersion = Get-ChildItem -LiteralPath $hostPacks -Directory |
        Where-Object { $_.Name -match '^\d+\.\d+\.\d+$' } |
        Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
    if (-not $hostVersion) { throw 'Install the Windows x64 .NET SDK to obtain nethost.dll' }
    $nativeHost = Join-Path $hostVersion.FullName 'runtimes/win-x64/native/nethost.dll'
    Copy-Item -LiteralPath $nativeHost -Destination (Join-Path $OutputDirectory 'nethost.dll') -ErrorAction Stop
    Copy-Item -LiteralPath 'compat/THIRD_PARTY_NOTICES.txt' -Destination $OutputDirectory -ErrorAction Stop
    foreach ($notice in @('LICENSE.txt', 'ThirdPartyNotices.txt')) {
        $noticePath = Join-Path $dotnetDirectory $notice
        if (Test-Path -LiteralPath $noticePath) {
            Copy-Item -LiteralPath $noticePath -Destination (Join-Path $OutputDirectory "DOTNET-$notice") -ErrorAction Stop
        }
    }
} finally { Pop-Location }

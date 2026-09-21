[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$UpstreamRoot,
    [Parameter(Mandatory)][string]$CatalogRoot,
    [string]$OutputPath = 'docs/parity/upstream-inventory.json'
)
$ErrorActionPreference = 'Stop'

function Get-SourceRevision([string]$Root) {
    $revision = git -C $Root rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw "Cannot resolve source revision: $Root" }
    $changes = git -C $Root status --porcelain --untracked-files=normal
    if ($LASTEXITCODE -ne 0) { throw "Cannot inspect source tree: $Root" }
    if ($changes) { throw "Inventory requires a clean source checkout: $Root" }
    return $revision.Trim()
}

function Get-SourceFile([string]$Root, [string]$Directory, [string]$Pattern) {
    Push-Location -LiteralPath $Root
    try {
        $paths = @(rg --files $Directory --glob $Pattern)
        if ($LASTEXITCODE -ne 0) { throw "Cannot enumerate $Directory in $Root" }
        return @($paths | ForEach-Object { $_.Replace('\', '/') } | Sort-Object -CaseSensitive)
    } finally { Pop-Location }
}

function Get-IdentifierSummary($Identifier) {
    [ordered]@{
        vendor_id = $Identifier.VendorID
        product_id = $Identifier.ProductID
        parser = $Identifier.ReportParser
        input_report_length = $Identifier.InputReportLength
        output_report_length = $Identifier.OutputReportLength
        feature_report_length = $Identifier.FeatureReportLength
        feature_init_reports = @($Identifier.FeatureInitReport | Where-Object { $null -ne $_ }).Count
        output_init_reports = @($Identifier.OutputInitReport | Where-Object { $null -ne $_ }).Count
        initialization_strings = @($Identifier.InitializationStrings | Where-Object { $null -ne $_ }).Count
        device_string_indices = @($Identifier.DeviceStrings.PSObject.Properties.Name | Sort-Object)
        attribute_names = @($Identifier.Attributes.PSObject.Properties.Name | Sort-Object)
    }
}

$upstream = (Resolve-Path -LiteralPath $UpstreamRoot).Path
$catalog = (Resolve-Path -LiteralPath $CatalogRoot).Path
$baseline = [version]'0.6.7.0'
$upstreamRevision = Get-SourceRevision $upstream
$catalogRevision = Get-SourceRevision $catalog
$tree = git -C $upstream ls-tree -r HEAD
if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect upstream source blobs' }
$blobIds = @{}
foreach ($entry in $tree) { if ($entry -match '^\d+ blob ([0-9a-f]+)\t(.+)$') { $blobIds[$Matches[2]] = $Matches[1] } }
$configurations = @(foreach ($path in (Get-SourceFile $upstream 'OpenTabletDriver.Configurations/Configurations' '*.json')) {
    $absolutePath = Join-Path $upstream $path
    $configuration = Get-Content -Raw -LiteralPath $absolutePath | ConvertFrom-Json
    $auxiliary = if ($configuration.AuxiliaryDeviceIdentifiers) {
        $configuration.AuxiliaryDeviceIdentifiers
    } else { $configuration.AuxilaryDeviceIdentifiers }
    [ordered]@{
        path = $path
        git_blob = $blobIds[$path]
        name = $configuration.Name
        manufacturer_directory = ($path -split '/')[2]
        digitizer = @($configuration.DigitizerIdentifiers | ForEach-Object { Get-IdentifierSummary $_ })
        auxiliary = @($auxiliary | Where-Object { $null -ne $_ } | ForEach-Object { Get-IdentifierSummary $_ })
    }
})
$parserNames = @($configurations | ForEach-Object { @($_.digitizer) + @($_.auxiliary) } | ForEach-Object { $_.parser } | Where-Object { $_ } | Sort-Object -Unique -CaseSensitive)
$parsers = @(foreach ($parser in $parserNames) {
    [ordered]@{
        type = $parser
        configurations = @($configurations | Where-Object { $parser -in @(@($_.digitizer) + @($_.auxiliary) | ForEach-Object { $_.parser }) } | ForEach-Object { $_.path })
    }
})
$entries = @(foreach ($path in (Get-SourceFile $catalog 'Repository' '*.json')) {
    $metadata = Get-Content -Raw -LiteralPath (Join-Path $catalog $path) | ConvertFrom-Json
    $minimum = [version]$metadata.SupportedDriverVersion
    $maximum = if ($metadata.MaxSupportedDriverVersion) { [version]$metadata.MaxSupportedDriverVersion } else { $null }
    # Matches v0.6.7 PluginMetadata.IsSupportedBy, including its build-component check.
    $eligible = $minimum.Major -eq $baseline.Major -and $minimum.Minor -eq $baseline.Minor -and $minimum.Build -le $baseline.Build -and ($null -eq $maximum -or $maximum -ge $baseline)
    [ordered]@{
        path = $path
        name = $metadata.Name
        owner = $metadata.Owner
        plugin_version = $metadata.PluginVersion
        minimum_driver_version = $metadata.SupportedDriverVersion
        maximum_driver_version = $metadata.MaxSupportedDriverVersion
        repository_url = $metadata.RepositoryUrl
        download_url = $metadata.DownloadUrl
        archive_sha256 = $metadata.SHA256
        license = $metadata.LicenseIdentifier
        metadata_allows_baseline = $eligible
    }
})
$eligibleEntries = @($entries | Where-Object { $_.metadata_allows_baseline })
$identities = @($eligibleEntries | ForEach-Object { [pscustomobject]$_ } | Group-Object name,owner,repository_url)
$inventory = [ordered]@{
    schema_version = 1
    scope = 'Source inventory only; metadata eligibility and configuration presence do not establish Rust compatibility or hardware support.'
    upstream = [ordered]@{ repository = 'https://github.com/OpenTabletDriver/OpenTabletDriver'; revision = $upstreamRevision; compatibility_version = $baseline.ToString() }
    catalog = [ordered]@{ repository = 'https://github.com/OpenTabletDriver/Plugin-Repository'; revision = $catalogRevision }
    counts = [ordered]@{
        configurations = $configurations.Count
        manufacturer_directories = @($configurations | ForEach-Object { $_.manufacturer_directory } | Sort-Object -Unique).Count
        referenced_parser_types = $parsers.Count
        parser_source_files = @(Get-SourceFile $upstream 'OpenTabletDriver.Configurations/Parsers' '*.cs').Count
        catalog_metadata_files = $entries.Count
        eligible_metadata_entries = $eligibleEntries.Count
        eligible_plugin_identities = $identities.Count
    }
    configurations = $configurations
    referenced_parsers = $parsers
    plugin_contract_sources = @(Get-SourceFile $upstream 'OpenTabletDriver.Plugin' '*.cs')
    catalog_entries = $entries
}
$destination = [System.IO.Path]::GetFullPath($OutputPath)
$parent = Split-Path -Parent $destination
New-Item -ItemType Directory -Path $parent -Force -ErrorAction Stop | Out-Null
[System.IO.File]::WriteAllText($destination, (($inventory | ConvertTo-Json -Depth 20) + "`n"), [System.Text.UTF8Encoding]::new($false))
$inventory.counts | ConvertTo-Json
Write-Output $destination

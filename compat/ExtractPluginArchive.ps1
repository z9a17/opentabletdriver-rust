$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = $null
try {
    $source = $env:OTD_PLUGIN_ARCHIVE
    $destination = [IO.Path]::GetFullPath($env:OTD_PLUGIN_STAGE)
    if ((Get-Item -LiteralPath $source -ErrorAction Stop).Length -gt 134217728) {
        throw 'Plugin archive exceeds 128 MiB.'
    }
    $archive = [IO.Compression.ZipFile]::OpenRead($source)
    if ($archive.Entries.Count -gt 4096) { throw 'Plugin archive exceeds 4096 entries.' }
    $rootPrefix = $destination.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $names = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $files = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $total = [long]0
    # Validate every entry before writing anything. Reject ambiguous Windows
    # names, aliases, links, devices and file/directory conflicts as well as
    # paths escaping the private staging directory.
    foreach ($entry in $archive.Entries) {
        $name = $entry.FullName.Replace('\', '/')
        if ([string]::IsNullOrEmpty($name) -or $name.StartsWith('/') -or $name.Contains(':')) {
            throw "Invalid archive path: $name"
        }
        $directory = $name.EndsWith('/')
        $parts = $name.TrimEnd('/').Split('/')
        foreach ($part in $parts) {
            if ([string]::IsNullOrEmpty($part) -or $part -eq '.' -or $part -eq '..' -or
                $part.EndsWith('.') -or $part.EndsWith(' ') -or
                $part.IndexOfAny([IO.Path]::GetInvalidFileNameChars()) -ge 0 -or
                $part -match '^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])($|\.)') {
                throw "Invalid archive component: $part"
            }
        }
        $normalized = [string]::Join('/', $parts)
        if (-not $names.Add($normalized)) { throw "Duplicate archive path: $name" }
        if (-not $directory) { $null = $files.Add($normalized) }
        $target = [IO.Path]::GetFullPath([IO.Path]::Combine($destination, $normalized))
        if (-not $target.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Archive path escapes staging: $name"
        }
        $unixType = ($entry.ExternalAttributes -shr 16) -band 61440
        if ($unixType -ne 0 -and $unixType -ne 32768 -and $unixType -ne 16384 -or
            ($entry.ExternalAttributes -band 1024) -ne 0) {
            throw "Links and special files are unsupported: $name"
        }
        if ($entry.Length -gt 67108864 -or ($directory -and $entry.Length -ne 0)) {
            throw "Invalid or excessive entry size: $name"
        }
        $total += $entry.Length
        if ($total -gt 268435456) { throw 'Expanded plugin exceeds 256 MiB.' }
    }
    foreach ($name in $names) {
        $parts = $name.Split('/')
        for ($index = 1; $index -lt $parts.Length; $index++) {
            $parent = [string]::Join('/', $parts[0..($index - 1)])
            if ($files.Contains($parent)) { throw "Archive file is also a directory: $parent" }
        }
    }
    $null = [IO.Directory]::CreateDirectory($destination)
    $buffer = [byte[]]::new(65536)
    foreach ($entry in $archive.Entries) {
        $name = $entry.FullName.Replace('\', '/')
        $target = [IO.Path]::Combine($destination, $name)
        if ($name.EndsWith('/')) { $null = [IO.Directory]::CreateDirectory($target); continue }
        $null = [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($target))
        $inputStream = $null
        $outputStream = $null
        try {
            $inputStream = $entry.Open()
            $outputStream = [IO.File]::Open($target, [IO.FileMode]::CreateNew)
            $copied = [long]0
            while (($count = $inputStream.Read($buffer, 0, $buffer.Length)) -ne 0) {
                $copied += $count
                if ($copied -gt $entry.Length) { throw "Entry exceeded declared size: $name" }
                $outputStream.Write($buffer, 0, $count)
            }
            if ($copied -ne $entry.Length) { throw "Truncated entry: $name" }
        } finally {
            if ($null -ne $outputStream) { $outputStream.Dispose() }
            if ($null -ne $inputStream) { $inputStream.Dispose() }
        }
    }
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
} finally {
    if ($null -ne $archive) { $archive.Dispose() }
}

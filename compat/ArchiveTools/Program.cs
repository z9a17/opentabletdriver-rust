using System.IO.Compression;
using System.Text.RegularExpressions;

// Portable counterpart of compat/ExtractPluginArchive.ps1. Every name/type/
// expanded-size is validated before the first destination file is created.
try {
    if (args.Length != 3 || args[0] != "extract-plugin") throw new ArgumentException("extract-plugin ARCHIVE STAGE");
    var source = Path.GetFullPath(args[1]);
    var destination = Path.GetFullPath(args[2]);
    if (new FileInfo(source).Length > 134217728) throw new InvalidDataException("Plugin archive exceeds 128 MiB.");
    if (Directory.Exists(destination) && (Directory.EnumerateFileSystemEntries(destination).Any()
        || new DirectoryInfo(destination).LinkTarget != null)) throw new IOException("Plugin stage must be an empty real directory.");
    using var archive = ZipFile.OpenRead(source);
    if (archive.Entries.Count > 4096) throw new InvalidDataException("Plugin archive exceeds 4096 entries.");
    var prefix = destination.TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar) + Path.DirectorySeparatorChar;
    var names = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
    var files = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
    long total = 0;
    foreach (var entry in archive.Entries) {
        var name = entry.FullName.Replace('\\', '/');
        if (string.IsNullOrEmpty(name) || name.StartsWith('/') || name.Contains(':')) throw new InvalidDataException($"Invalid archive path: {name}");
        var directory = name.EndsWith('/');
        var parts = name.TrimEnd('/').Split('/');
        foreach (var part in parts) {
            if (string.IsNullOrEmpty(part) || part is "." or ".." || part.EndsWith('.') || part.EndsWith(' ')
                || part.Any(c => c < 32 || "<>:\"/\\|?*".Contains(c))
                || Regex.IsMatch(part, "^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])($|\\.)", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant))
                throw new InvalidDataException($"Invalid archive component: {part}");
        }
        var normalized = string.Join('/', parts);
        if (!names.Add(normalized)) throw new InvalidDataException($"Duplicate archive path: {name}");
        if (!directory) files.Add(normalized);
        var target = Path.GetFullPath(Path.Combine(destination, normalized));
        if (!target.StartsWith(prefix, StringComparison.Ordinal)) throw new InvalidDataException($"Archive path escapes staging: {name}");
        var type = (entry.ExternalAttributes >> 16) & 61440;
        if ((type != 0 && type != 32768 && type != 16384) || (entry.ExternalAttributes & 1024) != 0)
            throw new InvalidDataException($"Links and special files are unsupported: {name}");
        if (entry.Length > 67108864 || (directory && entry.Length != 0)) throw new InvalidDataException($"Invalid or excessive entry size: {name}");
        total = checked(total + entry.Length);
        if (total > 268435456) throw new InvalidDataException("Expanded plugin exceeds 256 MiB.");
    }
    foreach (var name in names) {
        var parts = name.Split('/');
        for (int i = 1; i < parts.Length; i++)
            if (files.Contains(string.Join('/', parts.Take(i)))) throw new InvalidDataException($"Archive file is also a directory: {name}");
    }
    Directory.CreateDirectory(destination);
    var buffer = new byte[65536];
    foreach (var entry in archive.Entries) {
        var name = entry.FullName.Replace('\\', '/');
        var target = Path.Combine(destination, name);
        if (name.EndsWith('/')) { Directory.CreateDirectory(target); continue; }
        Directory.CreateDirectory(Path.GetDirectoryName(target)!);
        using var input = entry.Open();
        using var output = new FileStream(target, FileMode.CreateNew, FileAccess.Write, FileShare.None);
        long copied = 0;
        int count;
        while ((count = input.Read(buffer)) != 0) {
            copied = checked(copied + count);
            if (copied > entry.Length) throw new InvalidDataException($"Entry exceeded declared size: {name}");
            output.Write(buffer, 0, count);
        }
        if (copied != entry.Length) throw new InvalidDataException($"Truncated entry: {name}");
        output.Flush(true);
    }
    return 0;
} catch (Exception error) { Console.Error.WriteLine(error.Message); return 1; }

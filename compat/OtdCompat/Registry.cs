using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;
using OpenTabletDriver.Plugin;
using OpenTabletDriver.Plugin.Components;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Tablet;

namespace OtdCompat;

// Actual loaded assembly/type registry, independent of native reader ownership.
// A reload is atomic. Running instances retain the old generation until disposed.
// No upstream Driver, RootHub, device reader or daemon is constructed here.
sealed class RegistryGeneration : IDisposable
{
    internal readonly Dictionary<string, Assembly> Assemblies = new(StringComparer.OrdinalIgnoreCase);
    internal readonly List<PluginContext> Contexts = [];
    internal readonly Dictionary<string, List<Type>> Types = new(StringComparer.Ordinal);
    internal readonly List<string> SkippedNative = [];
    internal readonly List<(string Path, Type Type)> Entries = [];
    internal int References = 1;
    internal ulong Number;
    public void Dispose() { foreach (var context in Contexts) context.Unload(); }
    internal void Load(string root)
    {
        if (!Directory.Exists(root)) return;
        var directories = Directory.EnumerateDirectories(root).Order(StringComparer.OrdinalIgnoreCase).Take(257).ToArray();
        if (directories.Length > 256) throw new InvalidOperationException("Installed registry exceeds 256 plugin directories.");
        int files = 0;
        foreach (string directory in directories)
        {
            PluginContext? context = null;
            foreach (string path in Directory.EnumerateFiles(directory, "*.dll").Order(StringComparer.OrdinalIgnoreCase))
            {
                if (++files > 4096) throw new InvalidOperationException("Installed registry exceeds 4096 DLLs.");
                if (string.Equals(Path.GetFileName(path), "OpenTabletDriver.Plugin.dll", StringComparison.OrdinalIgnoreCase)
                    || string.Equals(Path.GetFileName(path), "OpenTabletDriver.Configurations.dll", StringComparison.OrdinalIgnoreCase)) continue;
                try { _ = AssemblyName.GetAssemblyName(path); }
                catch (BadImageFormatException) { SkippedNative.Add(path); continue; }
                context ??= new PluginContext(path, inspect: true);
                if (!Contexts.Contains(context)) Contexts.Add(context);
                // Stream-backed loading releases installation files immediately,
                // while this generation retains the actual managed assemblies.
                Assembly assembly = context.LoadPluginAssembly(path);
                Assemblies.Add(Path.GetFullPath(path), assembly);
                foreach (Type type in assembly.GetExportedTypes())
                {
                    if (!IsPluginType(type) || !PluginEligibility.IsDiscoverable(type)) continue;
                    if (Entries.Count == 20000) throw new InvalidOperationException("Installed registry exceeds 20000 plugin types.");
                    Entries.Add((path, type));
                    if (!Types.TryGetValue(type.FullName!, out var matches)) Types.Add(type.FullName!, matches = []);
                    matches.Add(type);
                }
            }
        }
    }
    static readonly Type[] Contracts = typeof(IDriver).Assembly.GetExportedTypes().Where(type => type.IsAbstract || type.IsInterface).ToArray();
    static bool IsPluginType(Type type) => !type.IsAbstract && !type.IsInterface && type.FullName != null
        && Contracts.Any(contract => contract.IsAssignableFrom(type)
            || type.GetInterfaces().Any(value => value.IsGenericType && value.GetGenericTypeDefinition() == contract));
}

static class InstalledRegistry
{
    static readonly object Gate = new();
    static RegistryGeneration? current;
    static ulong generation;
    static readonly Dictionary<string, Type> BuiltinParsers = new[] { typeof(OpenTabletDriver.Configurations.ReportParserProvider).Assembly, typeof(IDriver).Assembly }
        .SelectMany(assembly => assembly.GetExportedTypes()).Where(type => !type.IsAbstract && !type.IsInterface && typeof(IReportParser<IDeviceReport>).IsAssignableFrom(type))
        .ToDictionary(type => type.FullName!, type => type, StringComparer.Ordinal);
    internal static byte[] Reload(string root)
    {
        var next = new RegistryGeneration();
        try
        {
            next.Load(root);
            // Attributes/validated static choices can execute plugin code. Do
            // that outside the lifetime lock before exposing the new registry.
            var descriptions = next.Entries.Select(entry => new { assembly_path = entry.Path, metadata = EntryPoints.DescribeType(entry.Type) }).ToArray();
            var document = JObject.FromObject(new { generation = 0UL, assemblies = next.Assemblies.Count, skipped_native = next.SkippedNative, types = descriptions });
            lock (Gate)
            {
                if (generation == ulong.MaxValue) throw new InvalidOperationException("Installed registry generation exhausted.");
                next.Number = generation + 1; document["generation"] = next.Number;
                byte[] json = Encoding.UTF8.GetBytes(document.ToString(Formatting.None));
                if (json.Length > 1048576) throw new InvalidOperationException("Installed registry metadata exceeds 1 MiB.");
                generation = next.Number;
                var previous = current; current = next;
                if (previous != null && --previous.References == 0) previous.Dispose();
                return json;
            }
        }
        catch { next.Dispose(); throw; }
    }
    internal static RegistryGeneration? AcquirePath(string path)
    {
        lock (Gate) { if (current == null || !current.Assemblies.ContainsKey(path)) return null; current.References++; return current; }
    }
    internal static (Type Type, RegistryGeneration? Generation) AcquireParser(string name)
    {
        lock (Gate)
        {
            if (current != null && current.Types.TryGetValue(name, out var entries))
            {
                var matches = entries.Where(type => typeof(IReportParser<IDeviceReport>).IsAssignableFrom(type)).ToArray();
                if (matches.Length > 1) throw new InvalidOperationException($"Parser '{name}' occurs in multiple installed DLLs.");
                if (matches.Length == 1) { current.References++; return (matches[0], current); }
            }
            if (BuiltinParsers.TryGetValue(name, out var builtin)) return (builtin, null);
            throw new KeyNotFoundException($"Parser '{name}' is not in the loaded installed registry or pinned Configurations assembly.");
        }
    }
    internal static bool HasParser(string name)
    {
        lock (Gate) {
            if (current != null && current.Types.TryGetValue(name, out var entries)) {
                int count = entries.Count(type => typeof(IReportParser<IDeviceReport>).IsAssignableFrom(type));
                if (count > 1) throw new InvalidOperationException($"Parser '{name}' occurs in multiple installed DLLs.");
                if (count == 1) return true;
            }
            return BuiltinParsers.ContainsKey(name);
        }
    }
    internal static void Release(RegistryGeneration value) { lock (Gate) { if (--value.References == 0) value.Dispose(); } }
}

// Existing filter/tool/output/binding constructors use this lease; standalone
// explicit DLL paths remain usable without a prior LoadPlugins RPC.
sealed class PluginLoad
{
    readonly RegistryGeneration? generation;
    readonly PluginContext? context;
    int disposed;
    internal PluginLoad(string path)
    {
        generation = InstalledRegistry.AcquirePath(path);
        if (generation == null) context = new PluginContext(path);
    }
    internal Assembly LoadFromAssemblyPath(string path) => generation?.Assemblies[path] ?? context!.LoadFromAssemblyPath(path);
    internal void Unload()
    {
        if (Interlocked.Exchange(ref disposed, 1) != 0) return;
        if (generation != null) InstalledRegistry.Release(generation); else context!.Unload();
    }
}

sealed class ParserSession : IDisposable
{
    readonly Type type;
    readonly RegistryGeneration? generation;
    readonly int ownerThread = Environment.CurrentManagedThreadId;
    HostServices? services;
    IReportParser<IDeviceReport>? parser;
    bool disposed;
    internal byte[]? Pending { get; private set; }
    internal ParserSession(string name)
    {
        (type, generation) = InstalledRegistry.AcquireParser(name);
        try { Reset(); } catch { if (generation != null) InstalledRegistry.Release(generation); throw; }
    }
    void Check() { ObjectDisposedException.ThrowIf(disposed, this); if (Environment.CurrentManagedThreadId != ownerThread) throw new InvalidOperationException("Parser instances belong to their background stream thread."); }
    internal void Reset()
    {
        Check(); Pending = null;
        try { (parser as IDisposable)?.Dispose(); } finally { parser = null; services?.Dispose(); services = null; }
        object? created = null;
        try
        {
            created = Activator.CreateInstance(type) ?? throw new InvalidOperationException("Cannot construct report parser.");
            services = new HostServices(); services.Inject(type, created);
            HostServices.Complete(type, created, null);
            parser = (IReportParser<IDeviceReport>)created;
        }
        catch { try { (created as IDisposable)?.Dispose(); } finally { services?.Dispose(); services = null; } throw; }
    }
    internal unsafe int Decode(byte* raw, uint length)
    {
        Check(); Pending = null;
        if (raw == null || length == 0 || length > 65535) throw new ArgumentException("Debug parser raw length must be 1..65535.");
        // Exact upstream DebugReportData constructor: actual concrete Path and
        // Newtonsoft JToken.FromObject(report). Parse happens once per packet.
        IDeviceReport? report = parser!.Parse(new ReadOnlySpan<byte>(raw, checked((int)length)).ToArray());
        if (report == null) return 0;
        var data = new JObject { ["Path"] = report.GetType().FullName, ["Data"] = JToken.FromObject(report) };
        Pending = Encoding.UTF8.GetBytes(data.ToString(Formatting.None));
        if (Pending.Length > 262144) { Pending = null; throw new InvalidOperationException("Concrete debug report exceeds 256 KiB."); }
        return Pending.Length;
    }
    internal int Copy(Span<byte> output)
    {
        Check(); if (Pending == null) return 0;
        if (Pending.Length > output.Length) return Pending.Length;
        Pending.CopyTo(output); return Pending.Length;
    }
    public void Dispose()
    {
        if (disposed) return; Check(); disposed = true;
        try { (parser as IDisposable)?.Dispose(); }
        finally { parser = null; Pending = null; services?.Dispose(); if (generation != null) InstalledRegistry.Release(generation); }
    }
}

public static unsafe partial class EntryPoints
{
    // Retained bytes permit capacity retries without a second registry reload.
    [ThreadStatic] static byte[]? registryJson;
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int ReloadRegistry(byte* root, int length, byte* output, int capacity)
    {
        try
        {
            if (length < 0 || length > 32768 || capacity < 0 || capacity > 1048576) throw new ArgumentException("Invalid registry buffer.");
            if (root != null) registryJson = InstalledRegistry.Reload(Path.GetFullPath(Encoding.UTF8.GetString(new ReadOnlySpan<byte>(root, length))));
            if (registryJson == null) throw new InvalidOperationException("No staged registry result on this thread.");
            if (output == null || registryJson.Length > capacity) return registryJson.Length;
            registryJson.CopyTo(new Span<byte>(output, capacity)); return registryJson.Length;
        }
        catch (Exception error) { registryJson = null; lastError = error.GetBaseException().Message; return -1; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int HasReportParser(byte* name, int length)
    {
        try { if (name == null || length < 1 || length > 4096) throw new ArgumentException("Invalid parser name."); return InstalledRegistry.HasParser(Encoding.UTF8.GetString(new ReadOnlySpan<byte>(name, length))) ? 1 : 0; }
        catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static nint CreateDebugParser(byte* name, int length)
    {
        try
        {
            if (name == null || length < 1 || length > 4096) throw new ArgumentException("Invalid parser name.");
            var parser = new ParserSession(Encoding.UTF8.GetString(new ReadOnlySpan<byte>(name, length)));
            return GCHandle.ToIntPtr(GCHandle.Alloc(parser));
        }
        catch (Exception error) { lastError = error.GetBaseException().Message; return 0; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int DecodeDebugParser(nint context, byte* raw, uint length, byte* output, int capacity)
    {
        try
        {
            if (capacity < 0 || capacity > 262144) throw new ArgumentException("Invalid debug parser buffer.");
            var parser = (ParserSession)GCHandle.FromIntPtr(context).Target!;
            if (raw != null) { int size = parser.Decode(raw, length); if (output == null) return size; }
            else if (length != 0) throw new ArgumentException("Raw is missing.");
            return parser.Copy(output == null ? Span<byte>.Empty : new Span<byte>(output, capacity));
        }
        catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int ResetDebugParser(nint context)
    {
        try { ((ParserSession)GCHandle.FromIntPtr(context).Target!).Reset(); return 0; }
        catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static void DestroyDebugParser(nint context)
    {
        try { var handle = GCHandle.FromIntPtr(context); try { ((ParserSession)handle.Target!).Dispose(); } finally { handle.Free(); } }
        catch (Exception error) { lastError = error.GetBaseException().Message; }
    }
}

using System.Numerics;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Runtime.Loader;
using System.Text;
using Newtonsoft.Json.Linq;
using OpenTabletDriver.Plugin.Attributes;
using OpenTabletDriver.Plugin.DependencyInjection;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Tablet;

namespace OtdCompat;

[StructLayout(LayoutKind.Sequential)]
public struct Sample { public float X, Y; public ulong TimeNs; public uint Pressure, Flags; }

[StructLayout(LayoutKind.Sequential)]
public unsafe struct FilterApi
{
    public uint Version, Size;
    public fixed byte Name[64];
    public delegate* unmanaged[Cdecl]<byte*, nuint, nint> Create;
    public delegate* unmanaged[Cdecl]<nint, Sample*, int> Process;
    public delegate* unmanaged[Cdecl]<nint, void> Reset;
    public delegate* unmanaged[Cdecl]<nint, void> Destroy;
}

// Share the official OTD interface assembly with plugins, but resolve each
// plugin's other managed/native dependencies from its own directory.
sealed class PluginContext(string path) : AssemblyLoadContext(isCollectible: true)
{
    readonly AssemblyDependencyResolver resolver = new(path);
    readonly string directory = Path.GetDirectoryName(path)!;
    protected override Assembly? Load(AssemblyName name)
    {
        if (name.Name == typeof(ITabletReport).Assembly.GetName().Name)
            return typeof(ITabletReport).Assembly;
        string? dependency = resolver.ResolveAssemblyToPath(name);
        if (dependency == null)
        {
            string candidate = Path.Combine(directory, name.Name + ".dll");
            if (File.Exists(candidate)) dependency = candidate;
        }
        return dependency == null ? null : LoadFromAssemblyPath(dependency);
    }
    protected override nint LoadUnmanagedDll(string name)
    {
        string? path = resolver.ResolveUnmanagedDllToPath(name);
        return path == null ? 0 : LoadUnmanagedDllFromPath(path);
    }
}

static class PluginEligibility
{
    // Match DesktopPluginManager.ImportTypes: platform and ignore attributes
    // are checked on the concrete type, without inherited attributes.
    public static bool IsSupported(Type type) =>
        type.GetCustomAttribute<SupportedPlatformAttribute>(false)?.IsCurrentPlatform ?? true;

    public static bool IsIgnored(Type type) =>
        type.GetCustomAttributes(false).Any(attribute => attribute.GetType() == typeof(PluginIgnoreAttribute));

    public static bool IsDiscoverable(Type type) => IsSupported(type) && !IsIgnored(type);

    public static void RequireLoadable(Type type)
    {
        if (!IsSupported(type))
            throw new NotSupportedException($"Plugin type '{type.FullName}' does not support this platform ([SupportedPlatform]).");
        if (IsIgnored(type))
            throw new NotSupportedException($"Plugin type '{type.FullName}' is marked [PluginIgnore].");
    }
}

sealed class Report : ITabletReport, IEraserReport
{
    public byte[] Raw { get; set; } = [];
    public Vector2 Position { get; set; }
    public uint Pressure { get; set; }
    public bool[] PenButtons { get; set; } = new bool[2];
    public bool Eraser { get; set; }
}

sealed class Instance : IDisposable
{
    readonly PluginContext context;
    readonly IPositionedPipelineElement<IDeviceReport> filter;
    readonly Report report = new();
    readonly OutOfRangeReport outOfRange = new();
    readonly int ownerThread = Environment.CurrentManagedThreadId;
    IDeviceReport? emitted;
    int emissionCount;
    int consuming;
    int asyncEmission;
    public PipelinePosition Position { get; }

    public Instance(JObject config)
    {
        string path = Path.GetFullPath(config.Value<string>("assembly_path") ?? throw new ArgumentException("assembly_path missing"));
        context = new PluginContext(path);
        object? created = null;
        try
        {
            Type type = context.LoadFromAssemblyPath(path).GetType(config.Value<string>("type_name") ?? "", true)!;
            PluginEligibility.RequireLoadable(type);
            if (!typeof(IPositionedPipelineElement<IDeviceReport>).IsAssignableFrom(type)
                || typeof(AsyncPositionedPipelineElement<IDeviceReport>).IsAssignableFrom(type))
                throw new NotSupportedException("Only synchronous OTD position filters are supported; async filters, output modes, tools and bindings are not supported.");
            created = Activator.CreateInstance(type) ?? throw new InvalidOperationException("Cannot construct filter");
            filter = (IPositionedPipelineElement<IDeviceReport>)created;
            ApplySettings(type, created, config["settings"] as JObject ?? new JObject());
            InjectTablet(type, created, config["tablet"]?.ToObject<TabletConfiguration>()
                ?? throw new ArgumentException("tablet configuration missing"));
            Position = filter.Position;
            if (Position is not (PipelinePosition.PreTransform or PipelinePosition.PostTransform))
                throw new NotSupportedException($"Unsupported plugin pipeline position: {Position}.");
            filter.Emit += OnEmit;
        }
        catch
        {
            (created as IDisposable)?.Dispose();
            context.Unload();
            throw;
        }
    }

    static void ApplySettings(Type type, object value, JObject settings)
    {
        var properties = type.GetProperties().Where(p => p.GetCustomAttribute<PropertyAttribute>() != null).ToArray();
        foreach (var setting in settings.Properties())
            if (!properties.Any(p => p.Name == setting.Name))
                throw new ArgumentException($"Unknown plugin setting: {setting.Name}");
        foreach (var property in properties)
        {
            if (!property.CanWrite) continue;
            // PluginSettingStore.ApplySettings visits saved entries only. Missing
            // entries keep the constructor value; an explicit null selects the
            // attribute default, if one exists.
            if (!settings.TryGetValue(property.Name, out JToken? token)) continue;
            if (token == null || token.Type == JTokenType.Null)
            {
                if (property.GetCustomAttribute<DefaultPropertyValueAttribute>() is { } defaults)
                    property.SetValue(value, defaults.Value);
            }
            else
                property.SetValue(value, token.ToObject(property.PropertyType));
        }
    }

    static void InjectTablet(Type type, object value, TabletConfiguration configuration)
    {
        var tablet = new TabletReference(configuration, configuration.DigitizerIdentifiers.Take(1));
        // Walk each declaration so protected/private fields on base classes
        // are visible. All dependencies are assigned before any load callback.
        for (Type? owner = type; owner != null; owner = owner.BaseType)
        {
            const BindingFlags members = BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
            foreach (var property in owner.GetProperties(members))
            {
                bool resolved = property.GetCustomAttribute<ResolvedAttribute>() != null;
                bool tabletRef = property.GetCustomAttribute<TabletReferenceAttribute>() != null;
                if ((resolved || tabletRef) && property.PropertyType == typeof(TabletReference))
                    property.SetValue(value, tablet);
                else if (resolved || tabletRef)
                    throw new NotSupportedException($"Unsupported plugin dependency: {property.Name} ({property.PropertyType.Name})");
            }
            foreach (var field in owner.GetFields(members))
            {
                bool resolved = field.GetCustomAttribute<ResolvedAttribute>() != null;
                bool tabletRef = field.GetCustomAttribute<TabletReferenceAttribute>() != null;
                if ((resolved || tabletRef) && field.FieldType == typeof(TabletReference))
                    field.SetValue(value, tablet);
                else if (resolved || tabletRef)
                    throw new NotSupportedException($"Unsupported plugin field dependency: {field.Name} ({field.FieldType.Name})");
            }
        }
        foreach (var method in type.GetMethods())
            if (method.GetCustomAttribute<OnDependencyLoadAttribute>() != null)
                method.Invoke(value, []);
    }

    void OnEmit(IDeviceReport? value)
    {
        if (Environment.CurrentManagedThreadId != ownerThread || Volatile.Read(ref consuming) == 0)
        { Interlocked.Exchange(ref asyncEmission, 1); return; }
        emitted = value;
        emissionCount++;
    }

    public int Process(ref Sample sample)
    {
        if (Volatile.Read(ref asyncEmission) != 0) return 2;
        report.Position = new Vector2(sample.X, sample.Y);
        report.Pressure = sample.Pressure;
        report.Eraser = (sample.Flags & 2) != 0;
        emitted = null;
        emissionCount = 0;
        Volatile.Write(ref consuming, 1);
        try { filter.Consume(report); }
        finally { Volatile.Write(ref consuming, 0); }
        // A reusable report is safe only for synchronous, one-output filters.
        if (emissionCount != 1 || emitted is not IAbsolutePositionReport output || Volatile.Read(ref asyncEmission) != 0)
            return 3;
        sample.X = output.Position.X;
        sample.Y = output.Position.Y;
        return 0;
    }

    public void Reset()
    {
        // OTD filters receive a range-loss report; they decide how to reset.
        emitted = null;
        Volatile.Write(ref consuming, 1);
        try { filter.Consume(outOfRange); }
        finally { Volatile.Write(ref consuming, 0); }
    }

    public void Dispose()
    {
        filter.Emit -= OnEmit;
        (filter as IDisposable)?.Dispose();
        context.Unload();
    }
}

public static unsafe class EntryPoints
{
    [ThreadStatic] static string? lastError;
    static readonly nint Api = MakeApi();
    static nint MakeApi()
    {
        var api = (FilterApi*)NativeMemory.AllocZeroed((nuint)sizeof(FilterApi));
        api->Version = 1; api->Size = (uint)sizeof(FilterApi);
        Encoding.UTF8.GetBytes("OpenTabletDriver .NET compatibility", new Span<byte>(api->Name, 64));
        api->Create = &Create; api->Process = &Process; api->Reset = &Reset; api->Destroy = &Destroy;
        return (nint)api;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static FilterApi* GetApi() => (FilterApi*)Api;

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int GetPosition(nint context)
    {
        try { return (int)((Instance)GCHandle.FromIntPtr(context).Target!).Position; }
        catch (Exception e) { lastError = e.GetBaseException().Message; return -1; }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static nint Create(byte* json, nuint length)
    {
        try
        {
            if (length > 131072 || json == null) return 0;
            var settings = JObject.Parse(Encoding.UTF8.GetString(new ReadOnlySpan<byte>(json, (int)length)));
            return GCHandle.ToIntPtr(GCHandle.Alloc(new Instance(settings)));
        }
        catch (Exception e) { lastError = e.GetBaseException().Message; Console.Error.WriteLine($".NET plugin load failed: {lastError}"); return 0; }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static int Process(nint context, Sample* sample)
    {
        try { return ((Instance)GCHandle.FromIntPtr(context).Target!).Process(ref *sample); }
        catch (Exception e) { Console.Error.WriteLine($".NET plugin disabled: {e.GetBaseException().Message}"); return 1; }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static void Reset(nint context)
    {
        try { ((Instance)GCHandle.FromIntPtr(context).Target!).Reset(); }
        catch (Exception e) { Console.Error.WriteLine($".NET plugin reset failed: {e.GetBaseException().Message}"); }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static void Destroy(nint context)
    {
        var handle = GCHandle.FromIntPtr(context);
        try { ((Instance)handle.Target!).Dispose(); }
        catch (Exception e) { Console.Error.WriteLine($".NET plugin dispose failed: {e.GetBaseException().Message}"); }
        finally { handle.Free(); }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int Inspect(byte* path, int length, byte* output, int capacity)
    {
        try
        {
            string file = Path.GetFullPath(Encoding.UTF8.GetString(new ReadOnlySpan<byte>(path, length)));
            var context = new PluginContext(file);
            try
            {
                var types = context.LoadFromAssemblyPath(file).GetExportedTypes()
                    .Where(t => !t.IsAbstract && typeof(IPositionedPipelineElement<IDeviceReport>).IsAssignableFrom(t)
                        && PluginEligibility.IsDiscoverable(t))
                    .Select(t =>
                    {
                        var properties = t.GetProperties()
                            .Where(p => p.GetCustomAttribute<PropertyAttribute>() != null).ToArray();
                        return new {
                            type_name = t.FullName,
                            display_name = t.GetCustomAttribute<PluginNameAttribute>()?.Name,
                            // Omitted values preserve the plugin constructor's defaults.
                            // A null placeholder would instead coerce many value types to zero.
                            settings = properties.Where(p => p.GetCustomAttribute<DefaultPropertyValueAttribute>() != null)
                                .ToDictionary(p => p.Name, p => p.GetCustomAttribute<DefaultPropertyValueAttribute>()?.Value),
                            properties = properties.Select(p => new {
                                name = p.Name,
                                display_name = p.GetCustomAttribute<PropertyAttribute>()?.DisplayName,
                                unit = p.GetCustomAttribute<UnitAttribute>()?.Unit,
                                tooltip = p.GetCustomAttribute<ToolTipAttribute>()?.ToolTip
                            }).ToArray()
                        };
                    }).ToArray();
                byte[] bytes = Encoding.UTF8.GetBytes(Newtonsoft.Json.JsonConvert.SerializeObject(types));
                // Rust can retry with the required size for assemblies with long help text.
                if (bytes.Length > capacity) return bytes.Length;
                bytes.CopyTo(new Span<byte>(output, capacity));
                return bytes.Length;
            }
            finally { context.Unload(); }
        }
        catch (Exception e) { lastError = e.GetBaseException().Message; Console.Error.WriteLine($".NET plugin inspection failed: {lastError}"); return -1; }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int GetError(byte* output, int capacity)
    {
        try {
            byte[] bytes = Encoding.UTF8.GetBytes(lastError ?? "Unknown .NET plugin error");
            int count = Math.Min(bytes.Length, capacity);
            bytes.AsSpan(0, count).CopyTo(new Span<byte>(output, capacity));
            return count;
        } catch { return 0; }
    }
}

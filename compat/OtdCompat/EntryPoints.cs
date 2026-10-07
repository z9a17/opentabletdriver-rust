using System.Diagnostics;
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

// Separate extension to the managed bridge, not a change to native FilterApi v1.
[StructLayout(LayoutKind.Sequential)]
public unsafe struct NativePenReport
{
    public uint Version, Size;
    public byte* Raw;
    public uint RawLength, PenButtons, PenButtonCount;
    public float TiltX, TiltY;
    public uint NearProximity, HoverDistance, Capabilities;
}

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
sealed class PluginContext(string path, bool inspect = false) : OpenTabletDriver.Desktop.Reflection.DesktopPluginContext(new DirectoryInfo(Path.GetDirectoryName(Path.GetFullPath(path))!), hosted: true)
{
    readonly AssemblyDependencyResolver resolver = new(path);
    readonly string directory = Path.GetDirectoryName(path)!;
    protected override Assembly? Load(AssemblyName name)
    {
        if (name.Name == typeof(ITabletReport).Assembly.GetName().Name)
            return typeof(ITabletReport).Assembly;
        if (name.Name == typeof(OpenTabletDriver.Configurations.ReportParserProvider).Assembly.GetName().Name)
            return typeof(OpenTabletDriver.Configurations.ReportParserProvider).Assembly;
        if (name.Name == typeof(OpenTabletDriver.Driver).Assembly.GetName().Name)
            return typeof(OpenTabletDriver.Driver).Assembly;
        if (name.Name == typeof(OpenTabletDriver.Native.Windows.Windows).Assembly.GetName().Name)
            return typeof(OpenTabletDriver.Native.Windows.Windows).Assembly;
        if (name.Name == typeof(OpenTabletDriver.Desktop.Contracts.IDriverDaemon).Assembly.GetName().Name)
            return typeof(OpenTabletDriver.Desktop.Contracts.IDriverDaemon).Assembly;
        string? dependency = resolver.ResolveAssemblyToPath(name);
        if (dependency == null)
        {
            string candidate = Path.Combine(directory, name.Name + ".dll");
            if (File.Exists(candidate)) dependency = candidate;
        }
        return dependency == null ? null : LoadPluginAssembly(dependency);
    }
    internal Assembly LoadPluginAssembly(string file)
    {
        string? identity = AssemblyName.GetAssemblyName(file).FullName;
        foreach (Assembly shared in new[] { typeof(ITabletReport).Assembly,
            typeof(OpenTabletDriver.Configurations.ReportParserProvider).Assembly,
            typeof(OpenTabletDriver.Driver).Assembly, typeof(OpenTabletDriver.Native.Windows.Windows).Assembly,
            typeof(OpenTabletDriver.Desktop.Contracts.IDriverDaemon).Assembly })
            if (shared.FullName == identity) return shared;
        var loaded = Assemblies.FirstOrDefault(assembly => assembly.FullName == identity);
        if (loaded != null) return loaded;
        if (!inspect) return LoadFromAssemblyPath(file);
        // Inspection must not map installed managed DLLs into the panel.
        // Unloading a collectible context does not immediately unmap them.
        using var stream = File.OpenRead(file);
        return LoadFromStream(stream);
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

class PenSnapshot : ITabletReport
{
    // Transport-only fallback for the Rust profile's raw tip-switch policy.
    // Independent of Raw: changing bytes does not reparse any property.
    internal bool? NativeTipSwitch;
    uint pressure;
    public byte[] Raw { get; set; } = [];
    public Vector2 Position { get; set; }
    public uint Pressure
    {
        get => pressure;
        set
        {
            // Rust interop policy only: an actual pressure edit opts out of
            // the physical tip-switch fallback. Explicit profile thresholds
            // always use final pressure. Raw edits never affect this property.
            if (pressure != value) NativeTipSwitch = null;
            pressure = value;
        }
    }
    public bool[] PenButtons { get; set; } = [];
}

class Report : PenSnapshot, IEraserReport { public bool Eraser { get; set; } }

// Implement only real pinned 0.6.7 interfaces. There is no rotation interface;
// rotation and the other transport-only fields remain available through Raw.
class TiltReport : Report, ITiltReport
{
    public Vector2 Tilt { get; set; }
}

class ProximityReport : TiltReport, IProximityReport
{
    public bool NearProximity { get; set; }
    public uint HoverDistance { get; set; }
}

sealed class Instance : IDisposable
{
    readonly PluginLoad context;
    readonly HostServices? services;
    readonly IPositionedPipelineElement<IDeviceReport> filter;
    readonly int ownerThread = Environment.CurrentManagedThreadId;
    IDeviceReport? emitted;
    int emissionCount;
    int consuming;
    int asyncEmission;
    Action<IDeviceReport>? graphContinuation;
    // Timers injected into [Resolved] ITimer members. The report thread fires
    // them, so timer emissions continue the graph on the thread that owns it.
    readonly List<SessionTimer> timers = new();
    readonly bool providerTimers;
    public PipelinePosition Position { get; }
    public bool HasTimers => timers.Count != 0 || providerTimers;
    internal RegistryGeneration? SourceGeneration => context.Generation;

    public Instance(JObject config)
    {
        string path = Path.GetFullPath(config.Value<string>("assembly_path") ?? throw new ArgumentException("assembly_path missing"));
        context = new PluginLoad(path);
        object? created = null;
        try
        {
            Type type = context.LoadFromAssemblyPath(path).GetType(config.Value<string>("type_name") ?? "", true)!;
            PluginEligibility.RequireLoadable(type);
            if (!typeof(IPositionedPipelineElement<IDeviceReport>).IsAssignableFrom(type))
                throw new NotSupportedException("Only OTD position filters are supported; output modes and bindings are not supported.");
            created = HostServices.Construct(type) ?? throw new InvalidOperationException("Cannot construct filter");
            filter = (IPositionedPipelineElement<IDeviceReport>)created;
            // PluginManager.ConstructObject injects services before
            // PluginSettingStore.ApplySettings, so Frequency finds its timer.
            services = new HostServices(() => {
                if (Environment.CurrentManagedThreadId != ownerThread)
                    throw new InvalidOperationException("Pipeline timers must be acquired on the graph's owning thread.");
                var timer = new SessionTimer(); timers.Add(timer); return timer;
            });
            services.ConfigureSource(config);
            services.Inject(type, created);
            providerTimers = services.ProviderInjected;
            ApplySettings(type, created, config["settings"] as JObject ?? new JObject());
            HostServices.Complete(type, created, CreateTabletReference(config));
            Position = filter.Position;
            if (Position is not (PipelinePosition.PreTransform or PipelinePosition.PostTransform))
                throw new NotSupportedException($"Unsupported plugin pipeline position: {Position}.");
            filter.Emit += OnEmit;
        }
        catch
        {
            // Cleanup failures must not hide the construction/settings error.
            try { HostServices.DisposePlugin(created); }
            catch (Exception error) { Console.Error.WriteLine($".NET plugin cleanup failed: {error.GetBaseException().Message}"); }
            finally
            {
                foreach (var timer in timers) timer.Dispose();
                services?.Dispose();
                context.Unload();
            }
            throw;
        }
    }

    internal static void ApplySettings(Type type, object value, JObject settings)
    {
        using var setup = ServiceClient.Setup();
        var properties = type.GetProperties().Where(p => p.GetCustomAttribute<PropertyAttribute>() != null).ToArray();
        // Upstream ignores saved keys no longer declared by a plugin. Keep
        // those keys in the Rust profile for upgrades and rollback.
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

    /// Microseconds until this filter's next timer tick, or -1 without one.
    public long NextTickMicros(long now)
    {
        long best = -1;
        foreach (var timer in timers)
            if (timer.Enabled)
            {
                long remaining = Math.Max(0, timer.Due - now);
                // Multiply only the remainder: long intervals must not wrap
                // into an immediate tick and keep the report thread awake.
                long seconds = remaining / Stopwatch.Frequency;
                long fraction = (long)((remaining % Stopwatch.Frequency) * (1_000_000.0 / Stopwatch.Frequency));
                long micros = seconds > (long.MaxValue - fraction) / 1_000_000
                    ? long.MaxValue : seconds * 1_000_000 + fraction;
                if (best < 0 || micros < best) best = micros;
            }
        return best;
    }

    /// Fires this filter's due timers; emissions continue through `continuation`.
    public void TickGraph(long now, Action<IDeviceReport> continuation)
    {
        using var reportScope = ServiceClient.Report();
        if (Environment.CurrentManagedThreadId != ownerThread)
            throw new InvalidOperationException("Timers must fire on the graph's owning thread.");
        // Only the owning thread reads or writes `consuming` (OnEmit checks the
        // thread first), so a plain check avoids a locked instruction per call.
        if (consuming != 0)
            throw new InvalidOperationException("Reentrant timer tick of the same filter is unsupported.");
        Volatile.Write(ref consuming, 1);
        graphContinuation = continuation;
        try
        {
            // A callback can acquire another timer through IServiceProvider.
            // New timers participate on the next tick, without invalidating
            // this iteration or recursively firing an unbounded chain.
            int count = timers.Count;
            for (int index = 0; index < count; index++) timers[index].FireIfDue(now);
        }
        finally
        {
            graphContinuation = null;
            Volatile.Write(ref consuming, 0);
        }
    }

    // Properties remain the full matched configuration; Identifiers describe
    // the live InputDeviceTree, rather than its first configured alternative.
    internal static TabletReference CreateTabletReference(JObject config)
    {
        var configuration = config["tablet"]?.ToObject<TabletConfiguration>()
            ?? throw new ArgumentException("tablet configuration missing");
        var supplied = config["identifiers"];
        if (supplied is null || supplied.Type == JTokenType.Null)
            return new TabletReference(configuration, configuration.DigitizerIdentifiers.Take(1));
        if (supplied is not JArray array || array.Count is < 1 or > 2)
            throw new ArgumentException("identifiers must contain the opened digitizer and optional auxiliary endpoint");
        var identifiers = array.Select(item => item.ToObject<DeviceIdentifier>()
            ?? throw new ArgumentException("opened identifier missing")).ToArray();
        return new TabletReference(configuration, identifiers);
    }

    void OnEmit(IDeviceReport? value)
    {
        if (Environment.CurrentManagedThreadId != ownerThread || Volatile.Read(ref consuming) == 0)
        { Interlocked.Exchange(ref asyncEmission, 1); return; }
        if (graphContinuation is { } continuation)
        {
            continuation(value ?? throw new InvalidOperationException("A filter emitted a null report."));
            return;
        }
        emitted = value;
        emissionCount++;
    }

    public int Process(ref Sample sample)
    {
        // Legacy sample-only entry remains available for native-v1 callers.
        // Production Rust managed dispatch requires ProcessReport with raw data.
        return Consume(ref sample, new Report {
            Raw = new byte[0],
            Position = new Vector2(sample.X, sample.Y),
            Pressure = sample.Pressure,
            Eraser = (sample.Flags & 2) != 0,
            PenButtons = new bool[2]
        });
    }

    /// Compiles the plugin's Consume before the first report; see Precompiler.
    internal void PrecompileConsume()
    {
        try
        {
            Type type = filter.GetType();
            Type positioned = typeof(IPositionedPipelineElement<IDeviceReport>);
            foreach (Type contract in positioned.GetInterfaces().Append(positioned))
            {
                if (contract.GetMethod(nameof(filter.Consume)) is not { } consume) continue;
                InterfaceMapping map = type.GetInterfaceMap(contract);
                int index = Array.IndexOf(map.InterfaceMethods, consume);
                if (index >= 0) Precompiler.Prepare(map.TargetMethods[index]);
            }
        }
        catch (Exception) { }
    }

    public void ConsumeGraph(IDeviceReport report, Action<IDeviceReport> continuation)
    {
        using var reportScope = ServiceClient.Report();
        if (Environment.CurrentManagedThreadId != ownerThread || Volatile.Read(ref asyncEmission) != 0)
            throw new NotSupportedException("Asynchronous plugin emissions require the P05 scheduler.");
        // Owner-thread state, as in TickGraph: no locked instruction per report.
        if (consuming != 0)
            throw new InvalidOperationException("Reentrant consumption of the same filter is unsupported.");
        Volatile.Write(ref consuming, 1);
        graphContinuation = continuation;
        try
        {
            filter.Consume(report);
            if (Volatile.Read(ref asyncEmission) != 0)
                throw new NotSupportedException("Asynchronous plugin emissions require the P05 scheduler.");
        }
        finally
        {
            graphContinuation = null;
            Volatile.Write(ref consuming, 0);
        }
    }

    public unsafe int Process(ref Sample sample, in NativePenReport native)
    {
        // Every input has independent ownership: a synchronous filter may retain
        // the report, Raw, or PenButtons and inspect it after a later Consume.
        // Source interfaces: OpenTabletDriver.Plugin/Tablet/{ITabletReport,
        // ITiltReport,IEraserReport,IProximityReport}.cs at
        // 736003ed72c8bbb28033b039d5a0bb76c344145c.
        TiltReport report = (native.Capabilities & 1) != 0
            ? new ProximityReport {
                NearProximity = native.NearProximity != 0,
                HoverDistance = native.HoverDistance
            }
            : new TiltReport();
        report.Raw = new byte[native.RawLength];
        new ReadOnlySpan<byte>(native.Raw, (int)native.RawLength).CopyTo(report.Raw);
        report.PenButtons = new bool[native.PenButtonCount];
        for (int index = 0; index < report.PenButtons.Length; index++)
            report.PenButtons[index] = (native.PenButtons & (1u << index)) != 0;
        report.Position = new Vector2(sample.X, sample.Y);
        report.Pressure = sample.Pressure;
        report.Eraser = (sample.Flags & 2) != 0;
        report.Tilt = new Vector2(native.TiltX, native.TiltY);
        return Consume(ref sample, report);
    }

    int Consume(ref Sample sample, IDeviceReport report)
    {
        using var reportScope = ServiceClient.Report();
        if (Volatile.Read(ref asyncEmission) != 0) return 2;
        emitted = null;
        emissionCount = 0;
        Volatile.Write(ref consuming, 1);
        try { filter.Consume(report); }
        finally { Volatile.Write(ref consuming, 0); }
        // Owned reports make retention safe, but asynchronous emissions, report
        // suppression, and multiple outputs still require the P04/P05 scheduler.
        if (emissionCount != 1 || emitted is not IAbsolutePositionReport output || Volatile.Read(ref asyncEmission) != 0)
            return 3;
        sample.X = output.Position.X;
        sample.Y = output.Position.Y;
        return 0;
    }

    public void Reset(ReadOnlySpan<byte> raw)
    {
        using var reportScope = ServiceClient.Report();
        // OTD filters receive a range-loss report; they decide how to reset.
        // A fresh boxed struct and raw array keep retained loss reports stable.
        byte[] ownedRaw = new byte[raw.Length];
        raw.CopyTo(ownedRaw);
        IDeviceReport outOfRange = new OutOfRangeReport(ownedRaw);
        emitted = null;
        emissionCount = 0;
        Volatile.Write(ref consuming, 1);
        try { filter.Consume(outOfRange); }
        finally { Volatile.Write(ref consuming, 0); }
    }

    public void Dispose()
    {
        filter.Emit -= OnEmit;
        try { HostServices.DisposePlugin(filter); }
        finally
        {
            foreach (var timer in timers) timer.Dispose();
            services?.Dispose();
            context.Unload();
        }
    }
}

// The ITimer upstream's PluginManager supplies (WindowsTimer on Windows),
// except that it elapses on the report thread when the session reaches its
// due time instead of on a separate timer thread. A late tick fires once and
// the schedule restarts from now, as a periodic timer drops missed ticks.
sealed class SessionTimer : OpenTabletDriver.Plugin.Timers.ITimer
{
    long due;
    float interval = 1;
    long period = Math.Max(1, Stopwatch.Frequency / 1000);
    public float Interval
    {
        get => interval;
        set
        {
            double ticks = Math.Max(value, 0.05f) * (Stopwatch.Frequency / 1000.0);
            if (!float.IsFinite(value) || value <= 0 || ticks >= long.MaxValue)
                throw new ArgumentOutOfRangeException(nameof(value), "Timer interval must be finite, positive and representable.");
            period = Math.Max(1, (long)ticks);
            interval = value;
        }
    }
    public bool Enabled { get; private set; }
    public event Action? Elapsed;
    internal long Due => due;

    static long AddPeriod(long timestamp, long ticks) => timestamp > long.MaxValue - ticks
        ? long.MaxValue : timestamp + ticks;

    public void Start()
    {
        due = AddPeriod(Stopwatch.GetTimestamp(), period);
        Enabled = true;
    }

    public void Stop() => Enabled = false;

    internal void FireIfDue(long now)
    {
        if (!Enabled || now < due) return;
        due = AddPeriod(due, period);
        if (due <= now) due = AddPeriod(now, period);
        Elapsed?.Invoke();
        long finished = Stopwatch.GetTimestamp();
        if (Enabled && due <= finished) due = AddPeriod(finished, period);
    }

    public void Dispose()
    {
        Enabled = false;
        Elapsed = null;
    }
}

// A running ITool, as DriverDaemon.SetToolSettings constructs it: settings
// applied, dependency callbacks run, then Initialize. Tools get no tablet.
sealed class ToolInstance : IDisposable
{
    readonly PluginLoad context;
    readonly HostServices services = new();
    readonly OpenTabletDriver.Plugin.ITool tool;

    public ToolInstance(JObject config)
    {
        using var setup = ServiceClient.Setup();
        string path = Path.GetFullPath(config.Value<string>("assembly_path") ?? throw new ArgumentException("assembly_path missing"));
        context = new PluginLoad(path);
        object? created = null;
        try
        {
            Type type = context.LoadFromAssemblyPath(path).GetType(config.Value<string>("type_name") ?? "", true)!;
            PluginEligibility.RequireLoadable(type);
            if (!typeof(OpenTabletDriver.Plugin.ITool).IsAssignableFrom(type))
                throw new NotSupportedException($"'{type.FullName}' is not an OpenTabletDriver tool.");
            created = HostServices.Construct(type) ?? throw new InvalidOperationException("Cannot construct tool");
            tool = (OpenTabletDriver.Plugin.ITool)created;
            services.ConfigureSource(config);
            services.Inject(type, created);
            Instance.ApplySettings(type, created, config["settings"] as JObject ?? new JObject());
            HostServices.Complete(type, created, tablet: null);
            if (!tool.Initialize())
                throw new InvalidOperationException($"{type.FullName} failed to initialize.");
        }
        catch
        {
            try { HostServices.DisposePlugin(created); }
            catch (Exception error) { Console.Error.WriteLine($".NET tool cleanup failed: {error.GetBaseException().Message}"); }
            finally { services.Dispose(); context.Unload(); }
            throw;
        }
    }

    public void Dispose()
    {
        using var setup = ServiceClient.Setup();
        try { tool.Dispose(); }
        finally { services.Dispose(); context.Unload(); }
    }
}

public static unsafe partial class EntryPoints
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
    public static int ProcessReport(nint context, Sample* sample, NativePenReport* report)
    {
        try
        {
            if (sample == null || report == null || report->Version != 1
                || report->Size != sizeof(NativePenReport) || report->Raw == null
                || report->RawLength > 192 || report->RawLength == 0
                || report->PenButtonCount > 3 || (report->Capabilities & ~1u) != 0)
                throw new ArgumentException("Invalid owned pen-report bridge payload.");
            byte id = report->Raw[0];
            if (!((id == 0x10 && report->RawLength >= 17 && report->PenButtonCount == 2)
                || (id == 0x1e && report->RawLength >= 13 && report->PenButtonCount == 3)))
                throw new ArgumentException("Owned pen-report payload does not match a PTH-660 pen packet.");
            return ((Instance)GCHandle.FromIntPtr(context).Target!).Process(ref *sample, in *report);
        }
        catch (Exception e) { lastError = e.GetBaseException().Message; Console.Error.WriteLine($".NET plugin disabled: {lastError}"); return 1; }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int ResetReport(nint context, byte* raw, uint length)
    {
        try
        {
            if (length > 192 || (raw == null && length != 0))
                throw new ArgumentException("Invalid owned range-loss payload.");
            ((Instance)GCHandle.FromIntPtr(context).Target!).Reset(new ReadOnlySpan<byte>(raw, (int)length));
            return 0;
        }
        catch (Exception e) { lastError = e.GetBaseException().Message; Console.Error.WriteLine($".NET plugin reset failed: {lastError}"); return 1; }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static void Reset(nint context)
    {
        try { ((Instance)GCHandle.FromIntPtr(context).Target!).Reset(ReadOnlySpan<byte>.Empty); }
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
    public static nint CreateTool(byte* json, nuint length)
    {
        try
        {
            if (length > 131072 || json == null) return 0;
            var settings = JObject.Parse(Encoding.UTF8.GetString(new ReadOnlySpan<byte>(json, (int)length)));
            return GCHandle.ToIntPtr(GCHandle.Alloc(new ToolInstance(settings)));
        }
        catch (Exception e) { lastError = e.GetBaseException().Message; return 0; }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static void DestroyTool(nint handle)
    {
        var owner = GCHandle.FromIntPtr(handle);
        try { ((ToolInstance)owner.Target!).Dispose(); }
        catch (Exception e) { Console.Error.WriteLine($".NET tool dispose failed: {e.GetBaseException().Message}"); }
        finally { owner.Free(); }
    }

    internal static object DescribeType(Type t)
    {
        var properties = t.GetProperties()
            .Where(p => p.GetCustomAttribute<PropertyAttribute>() != null).ToArray();
        return new {
            kind = typeof(IOutputMode).IsAssignableFrom(t) ? "output" : typeof(OpenTabletDriver.Plugin.IBinding).IsAssignableFrom(t) ? "binding" : typeof(OpenTabletDriver.Plugin.ITool).IsAssignableFrom(t) ? "tool" : typeof(IPositionedPipelineElement<IDeviceReport>).IsAssignableFrom(t) ? "filter" : typeof(IReportParser<IDeviceReport>).IsAssignableFrom(t) ? "parser" : typeof(IDeviceReport).IsAssignableFrom(t) ? "report" : "provider",
            supported = typeof(IOutputMode).IsAssignableFrom(t) || typeof(OpenTabletDriver.Plugin.IStateBinding).IsAssignableFrom(t) || typeof(OpenTabletDriver.Plugin.ITool).IsAssignableFrom(t) || typeof(IPositionedPipelineElement<IDeviceReport>).IsAssignableFrom(t) || typeof(IReportParser<IDeviceReport>).IsAssignableFrom(t),
            relative_output = typeof(RelativeOutputMode).IsAssignableFrom(t),
            absolute_output = typeof(AbsoluteOutputMode).IsAssignableFrom(t),
            type_name = t.FullName,
            display_name = t.GetCustomAttribute<PluginNameAttribute>()?.Name,
            // Omitted values preserve the plugin constructor's defaults.
            // A null placeholder would instead coerce many value types to zero.
            // GeneratedControls also saves a slider's DefaultValue
            // for a property that has no value yet.
            settings = properties.Where(p => p.GetCustomAttribute<DefaultPropertyValueAttribute>() != null
                    || (p.GetCustomAttribute<SliderPropertyAttribute>() != null && p.PropertyType == typeof(float)))
                .ToDictionary(p => p.Name, p => p.GetCustomAttribute<DefaultPropertyValueAttribute>() is { } defaults
                    ? defaults.Value
                    : p.GetCustomAttribute<SliderPropertyAttribute>()!.DefaultValue),
            properties = properties.Select(p => new {
                name = p.Name,
                display_name = p.GetCustomAttribute<PropertyAttribute>()?.DisplayName,
                unit = p.GetCustomAttribute<UnitAttribute>()?.Unit,
                tooltip = p.GetCustomAttribute<ToolTipAttribute>()?.ToolTip,
                property_type = (Nullable.GetUnderlyingType(p.PropertyType) ?? p.PropertyType).FullName,
                writable = p.SetMethod?.IsPublic == true && p.GetIndexParameters().Length == 0,
                default_is_attribute = p.GetCustomAttribute<DefaultPropertyValueAttribute>() != null,
                enum_flags = (Nullable.GetUnderlyingType(p.PropertyType) ?? p.PropertyType).IsDefined(typeof(FlagsAttribute), false),
                enum_underlying_type = EnumUnderlyingType(Nullable.GetUnderlyingType(p.PropertyType) ?? p.PropertyType),
                enum_choices = EnumChoices(Nullable.GetUnderlyingType(p.PropertyType) ?? p.PropertyType),
                valid_values = ValidValues(p),
                slider = p.GetCustomAttribute<SliderPropertyAttribute>() is { } slider
                    ? new { min = slider.Min, max = slider.Max, default_value = slider.DefaultValue }
                    : null,
                description = p.GetCustomAttribute<BooleanPropertyAttribute>()?.Description
            }).ToArray()
        };
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int Inspect(byte* path, int length, byte* output, int capacity)
    {
        try
        {
            string file = Path.GetFullPath(Encoding.UTF8.GetString(new ReadOnlySpan<byte>(path, length)));
            var context = new PluginContext(file, inspect: true);
            try
            {
                var types = context.LoadPluginAssembly(file).GetExportedTypes()
                    .Where(t => !t.IsAbstract && (typeof(IPositionedPipelineElement<IDeviceReport>).IsAssignableFrom(t)
                            || typeof(OpenTabletDriver.Plugin.ITool).IsAssignableFrom(t)
                            || typeof(IOutputMode).IsAssignableFrom(t) || typeof(OpenTabletDriver.Plugin.IBinding).IsAssignableFrom(t) || typeof(IReportParser<IDeviceReport>).IsAssignableFrom(t))
                        && PluginEligibility.IsDiscoverable(t))
                    .Select(t =>
                    {
                        return DescribeType(t);
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

    // A [PropertyValidated] string's choices come from a static member, as
    // GeneratedControls reads them. This runs that member's code, like
    // upstream's settings page; a failure leaves the plain text field.
    static string[]? ValidValues(PropertyInfo property)
    {
        if (property.PropertyType != typeof(string)
            || property.GetCustomAttribute<PropertyValidatedAttribute>() is not { } validated)
            return null;
        try { return validated.GetValue<IEnumerable<string>>(property)?.ToArray(); }
        catch { return null; }
    }

    // Reflection only: inspecting controls must not construct or run a plugin.
    static string? EnumUnderlyingType(Type type) => type.IsEnum ? Enum.GetUnderlyingType(type).FullName : null;

    static object[] EnumChoices(Type type) => type.IsEnum
        ? Enum.GetNames(type).Select(name => (object)new {
            name,
            value = Convert.ChangeType(Enum.Parse(type, name), Enum.GetUnderlyingType(type), System.Globalization.CultureInfo.InvariantCulture)
        }).ToArray()
        : [];

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

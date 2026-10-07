using System.Reflection;
using System.Collections.Concurrent;
using System.Runtime.CompilerServices;
using Newtonsoft.Json.Linq;
using OpenTabletDriver.Desktop;
using OpenTabletDriver.Desktop.Contracts;
using OpenTabletDriver.Desktop.Diagnostics;
using OpenTabletDriver.Desktop.Reflection;
using OpenTabletDriver.Desktop.Reflection.Metadata;
using OpenTabletDriver.Desktop.RPC;
using OpenTabletDriver.Desktop.Updater;
using OpenTabletDriver.Plugin;
using OpenTabletDriver.Plugin.Components;
using OpenTabletDriver.Plugin.Devices;
using OpenTabletDriver.Plugin.Logging;
using OpenTabletDriver.Plugin.Tablet;

namespace OtdCompat;

// Real interface providers over host-owned readers/state. Concrete upstream
// Driver/InputDevice/RootHub are deliberately never constructed as empty stubs.
sealed partial class ManagedProviders : IDriver, IDeviceConfigurationProvider, IReportParserProvider,
    IDeviceHubsProvider, ICompositeDeviceHub, IDriverDaemon, IDisposable
{
    internal ManagedProviders(JObject? sourceSession = null) { this.sourceSession = sourceSession?.DeepClone() as JObject; }
    readonly JObject? sourceSession;
    HostedCore? core;
    HostedCore Core {
        get {
            lock (gate) {
                if (core != null) return core;
                var created = new HostedCore(this, scope, lifetime.Token, sourceSession);
                core = created; // Parser dependency injection sees this same concrete Driver.
                try { created.Refresh(); return created; }
                catch { core = null; created.Dispose(); throw; }
            }
        }
    }
    static long nextScope;
    readonly ulong scope = checked((ulong)Interlocked.Increment(ref nextScope));
    readonly CancellationTokenSource lifetime = new();
    readonly List<ProviderParser> parsers = [];
    readonly object gate = new();
    internal object Sync => gate;
    EventHandler<IEnumerable<TabletReference>>? tabletsChanged;
    EventHandler<DevicesChangedEventArgs>? devicesChanged;
    EventHandler<LogMessage>? message;
    EventHandler? resynchronize;
    Task? monitor;
    bool disposed;
    HostedNativeHub? nativeHub;
    DesktopDeviceConfigurationProvider? desktopConfigurations;
    OpenTabletDriver.Configurations.DeviceConfigurationProvider? builtinConfigurations;
    DesktopReportParserProvider? desktopParsers;
    OpenTabletDriver.Configurations.ReportParserProvider? builtinParsers;
    internal object? Get(Type type)
    {
        ObjectDisposedException.ThrowIf(disposed, this);
        if (!ServiceClient.Available) return null;
        if (type == typeof(OpenTabletDriver.Driver) || type == typeof(IDriver)) return Core.Driver;
        if (type == typeof(OpenTabletDriver.InputDeviceTree)) return Core.SelectedTree();
        if (type == typeof(OpenTabletDriver.InputDevice)) return Core.SelectedInput();
        if (type == typeof(OpenTabletDriver.Devices.RootHub) || type == typeof(ICompositeDeviceHub) || type == typeof(IDeviceHub)) return Core.Root;
        if (type == typeof(IDeviceConfigurationProvider)
            || type == typeof(IReportParserProvider) || type == typeof(IDeviceHubsProvider)
            || type == typeof(IDeviceHub) || type == typeof(ICompositeDeviceHub) || type == typeof(IDriverDaemon)) return this;
        if (type == typeof(PluginManager) || type == typeof(DesktopPluginManager)
            || type == typeof(IServiceManager)) return HostedDesktop.Manager;
        if (type == typeof(AppInfo)) return HostedDesktop.Application;
        if (type == typeof(PresetManager)) { HostedDesktop.Configure(); return AppInfo.PresetManager; }
        // These original concrete helpers are read-only/factory-only; none
        // constructs a Driver, RootHub or device reader.
        if (type == typeof(DesktopDeviceConfigurationProvider)) {
            HostedDesktop.Configure(); return desktopConfigurations ??= new DesktopDeviceConfigurationProvider();
        }
        if (type == typeof(OpenTabletDriver.Configurations.DeviceConfigurationProvider))
            return builtinConfigurations ??= new OpenTabletDriver.Configurations.DeviceConfigurationProvider();
        if (type == typeof(DesktopReportParserProvider)) {
            HostedDesktop.Configure(); return desktopParsers ??= new DesktopReportParserProvider();
        }
        if (type == typeof(OpenTabletDriver.Configurations.ReportParserProvider))
            return builtinParsers ??= new OpenTabletDriver.Configurations.ReportParserProvider();
        return null;
    }
    static JToken Require(JObject snapshot, string name) => snapshot[name] is { Type: not JTokenType.Null } value
        ? value : throw new NotSupportedException($"The native host has not published '{name}'.");
    internal static T Read<T>(string name) {
        var result = Require(ServiceClient.Snapshot(), name).ToObject<T>();
        return result is not null ? result : throw new InvalidOperationException($"Invalid managed '{name}' snapshot.");
    }
    T OwnedRead<T>(string name) { lifetime.Token.ThrowIfCancellationRequested(); return Read<T>(name); }
    public IEnumerable<TabletReference> Tablets => OwnedRead<TabletReference[]>("tablets");
    public IEnumerable<TabletConfiguration> TabletConfigurations => OwnedRead<TabletConfiguration[]>("configurations");
    public IEnumerable<IDeviceHub> DeviceHubs { get { lifetime.Token.ThrowIfCancellationRequested(); return [nativeHub ??= new HostedNativeHub(this)]; } }
    public void ConnectDeviceHub<T>() where T : IDeviceHub => Core.Root.ConnectDeviceHub<T>();
    public void ConnectDeviceHub(IDeviceHub instance) => Core.Root.ConnectDeviceHub(instance);
    public void DisconnectDeviceHub<T>() where T : IDeviceHub => Core.Root.DisconnectDeviceHub<T>();
    public void DisconnectDeviceHub(IDeviceHub instance) => Core.Root.DisconnectDeviceHub(instance);
    public IEnumerable<IDeviceEndpoint> GetDevices() { lifetime.Token.ThrowIfCancellationRequested(); return Endpoints(ServiceClient.Snapshot()); }
    IDeviceEndpoint[] Endpoints(JObject snapshot) => Require(snapshot, "devices").Children()
        .Select(data => (IDeviceEndpoint)new HostedEndpoint((JObject)data, scope, lifetime.Token)).ToArray();
    public bool Detect()
    {
        lifetime.Token.ThrowIfCancellationRequested();
        ServiceClient.RequireBlockingAllowed();
        return ServiceClient.Request(2, scope, null, lifetime.Token).GetAwaiter().GetResult().Value<bool>();
    }
    public IReportParser<IDeviceReport> GetReportParser(DeviceIdentifier identifier) => GetReportParser(identifier.ReportParser);
    public IReportParser<IDeviceReport> GetReportParser(string name)
    {
        lock (gate) { ObjectDisposedException.ThrowIf(disposed, this);
            if (parsers.Count >= 256) throw new InvalidOperationException("Provider scope exceeds 256 owned parser instances.");
            var parser = new ProviderParser(name, this); parsers.Add(parser); return parser; }
    }
    public event EventHandler<IEnumerable<TabletReference>> TabletsChanged {
        add { lock (gate) { ObjectDisposedException.ThrowIf(disposed, this); tabletsChanged += value; StartMonitor(); } }
        remove { lock (gate) tabletsChanged -= value; }
    }
    public event EventHandler<DevicesChangedEventArgs> DevicesChanged {
        add { lock (gate) { ObjectDisposedException.ThrowIf(disposed, this); devicesChanged += value; StartMonitor(); } }
        remove { lock (gate) devicesChanged -= value; }
    }
    public event EventHandler<LogMessage> Message {
        add { lock (gate) { ObjectDisposedException.ThrowIf(disposed, this); message += value; StartMonitor(); } }
        remove { lock (gate) message -= value; }
    }
    public event EventHandler Resynchronize {
        add { lock (gate) { ObjectDisposedException.ThrowIf(disposed, this); resynchronize += value; StartMonitor(); } }
        remove { lock (gate) resynchronize -= value; }
    }
    EventHandler<DebugReportData>? deviceReport;
    public event EventHandler<DebugReportData> DeviceReport {
        add { lock (gate) { ObjectDisposedException.ThrowIf(disposed, this); deviceReport += value; } }
        remove { lock (gate) deviceReport -= value; }
    }
    void StartMonitor() {
        if (monitor != null) return;
        JObject? baseline = null;
        try { baseline = ServiceClient.Snapshot(); } catch (Exception) { }
        monitor = Task.Run(() => Monitor(baseline));
    }
    async Task Monitor(JObject? previous)
    {
        while (!lifetime.IsCancellationRequested) {
            try {
                var current = ServiceClient.Snapshot();
                if (previous != null && current.Value<ulong>("version") != previous.Value<ulong>("version")) {
                    EventHandler<IEnumerable<TabletReference>>? tablets;
                    EventHandler<DevicesChangedEventArgs>? devices;
                    EventHandler<LogMessage>? logs; EventHandler? sync;
                    lock (gate) { if (disposed) return; tablets = tabletsChanged; devices = devicesChanged; logs = message; sync = resynchronize; }
                    if (!JToken.DeepEquals(previous["tablets"], current["tablets"]) && current["tablets"] is JArray tabletData)
                        Notify(() => tablets?.Invoke(this, tabletData.ToObject<TabletReference[]>()!));
                    if (!JToken.DeepEquals(previous["devices"], current["devices"]) && previous["devices"] is JArray && current["devices"] is JArray)
                        Notify(() => devices?.Invoke(this, new DevicesChangedEventArgs(Endpoints(previous), Endpoints(current))));
                    if (previous.Value<ulong>("resynchronize") != current.Value<ulong>("resynchronize"))
                        Notify(() => sync?.Invoke(this, EventArgs.Empty));
                    // The native log is bounded and rolls over. Its actual
                    // sequence identifies the available new tail even when
                    // every retained array has the same length. Missed entries
                    // outside that tail cannot be reconstructed or replayed.
                    ulong oldSequence = previous.Value<ulong>("log_sequence");
                    ulong newSequence = current.Value<ulong>("log_sequence");
                    if (previous["logs"] is JArray && current["logs"] is JArray newLog
                        && previous["daemon_identity"]?["instance"]?.Value<string>() == current["daemon_identity"]?["instance"]?.Value<string>()
                        && newSequence > oldSequence) {
                        int appended = (int)Math.Min(newSequence - oldSequence, (ulong)newLog.Count);
                        foreach (var entry in newLog.Skip(newLog.Count - appended)) Notify(() => logs?.Invoke(this, entry.ToObject<LogMessage>()!));
                    }
                }
                previous = current;
            } catch (Exception) when (!lifetime.IsCancellationRequested) { /* unavailable snapshots retry, never fabricated */ }
            try { await Task.Delay(250, lifetime.Token).ConfigureAwait(false); }
            catch (OperationCanceledException) { return; }
        }
    }
    static void Notify(Action callback) { try { callback(); } catch (Exception) { /* isolate third-party event handlers */ } }
    async Task<JToken> Call(string method, params object?[] parameters)
    {
        ObjectDisposedException.ThrowIf(disposed, this);
        if (ServiceClient.SetupDepth != 0)
            throw new InvalidOperationException("Queued daemon operations cannot be awaited during plugin construction/dependency loading; the native setup transaction owns this scope.");
        JObject payload = new() { ["method"] = method, ["params"] = JArray.FromObject(parameters) };
        if (ServiceClient.BindingOwner is { } owner) {
            payload["source_binding_owner"] = owner;
            payload["source_tablet_name"] = ServiceClient.BindingTablet;
        }
        // An intentional self-apply replaces and disposes the binding that
        // requested it. Its Task still owns the completion through that change;
        // native generation guards protect queued stale applies.
        Task<JToken> completion;
        lock (gate) {
            // Linearize admission with scope disposal. Detaching a self-apply's
            // completion must never let a retired scope admit a new mutation.
            ObjectDisposedException.ThrowIf(disposed, this);
            lifetime.Token.ThrowIfCancellationRequested();
            var cancellation = method is "SetSettings" or "ResetSettings" or "ForceResynchronize"
                ? CancellationToken.None : lifetime.Token;
            completion = ServiceClient.Request(1, scope, payload, cancellation);
        }
        JToken result = await completion.ConfigureAwait(false);
        ServiceClient.RefreshAfterCommit();
        return result;
    }
    public async Task WriteMessage(LogMessage value) => await Call("WriteMessage", value).ConfigureAwait(false);
    public async Task LoadPlugins() => await Call("LoadPlugins").ConfigureAwait(false);
    public async Task<bool> InstallPlugin(string value) => (await Call("InstallPlugin", value).ConfigureAwait(false)).Value<bool>();
    public async Task<bool> UninstallPlugin(string value) => (await Call("UninstallPlugin", value).ConfigureAwait(false)).Value<bool>();
    public async Task<bool> DownloadPlugin(PluginMetadata value) => (await Call("DownloadPlugin", value).ConfigureAwait(false)).Value<bool>();
    Task<T> Cached<T>(string name) {
        try { ObjectDisposedException.ThrowIf(disposed, this); return Task.FromResult(Read<T>(name)); }
        catch (Exception error) { return Task.FromException<T>(error); }
    }
    Task<IEnumerable<SerializedDeviceEndpoint>> IDriverDaemon.GetDevices() => Cached<IEnumerable<SerializedDeviceEndpoint>>("devices");
    public Task<IEnumerable<TabletReference>> GetTablets() => Cached<IEnumerable<TabletReference>>("tablets");
    public async Task<IEnumerable<TabletReference>> DetectTablets() => (await Call("DetectTablets").ConfigureAwait(false)).ToObject<TabletReference[]>()!;
    public async Task SetSettings(Settings value) => await Call("SetSettings", value).ConfigureAwait(false);
    public Task<Settings> GetSettings() => Cached<Settings>("settings");
    public async Task ResetSettings() => await Call("ResetSettings").ConfigureAwait(false);
    public Task<AppInfo> GetApplicationInfo() { try { return Task.FromResult(HostedDesktop.Application); } catch (Exception error) { return Task.FromException<AppInfo>(error); } }
    public Task SetTabletDebug(bool enabled) => ConfigureDebug(enabled);
    public async Task<string> RequestDeviceString(int vendor, int product, int index) => (await Call("RequestDeviceString", vendor, product, index).ConfigureAwait(false)).Value<string>()!;
    public Task<IEnumerable<LogMessage>> GetCurrentLog() => Cached<IEnumerable<LogMessage>>("logs");
    public Task<DiagnosticInfo> GetDiagnosticInfo()
    {
        try {
            ObjectDisposedException.ThrowIf(disposed, this);
            DiagnosticInfo.HostedAppVersion = () => Read<string>("diagnostic_app_version");
            DiagnosticInfo.HostedBuildDate = () => Read<string>("diagnostic_build_date");
            return Task.FromResult(new DiagnosticInfo(Read<LogMessage[]>("logs"), Read<SerializedDeviceEndpoint[]>("devices")));
        } catch (Exception error) { return Task.FromException<DiagnosticInfo>(error); }
    }
    public async Task<SerializedUpdateInfo?> CheckForUpdates() => (await Call("CheckForUpdates").ConfigureAwait(false)).ToObject<SerializedUpdateInfo>();
    public async Task InstallUpdate() => await Call("InstallUpdate").ConfigureAwait(false);
    public async Task ForceResynchronize() => await Call("ForceResynchronize").ConfigureAwait(false);
    public void Dispose()
    {
        lock (gate) {
            if (disposed) return; disposed = true; lifetime.Cancel();
            tabletsChanged = null; devicesChanged = null; message = null; resynchronize = null; deviceReport = null;
            CancelDebug();
            _ = StopDisposedDebug();
            List<Exception>? failures = null;
            foreach (var parser in parsers) {
                try { parser.Dispose(); } catch (Exception error) { (failures ??= []).Add(error); }
            }
            parsers.Clear();
            core?.Dispose();
            if (failures != null) throw new AggregateException("Provider parser disposal failed.", failures);
        }
        // No join of plugin event handlers here: a handler may be waiting on
        // the native settings transaction that is disposing this scope.
    }
    sealed class HostedNativeHub(ManagedProviders owner) : IDeviceHub
    {
        public IEnumerable<IDeviceEndpoint> GetDevices() => owner.GetDevices();
        public event EventHandler<DevicesChangedEventArgs> DevicesChanged {
            add => owner.DevicesChanged += value; remove => owner.DevicesChanged -= value;
        }
    }
}

sealed class ProviderParser : IReportParser<IDeviceReport>, IDisposable
{
    readonly object gate = new();
    readonly RegistryGeneration? generation;
    readonly HostServices services;
    readonly IReportParser<IDeviceReport> parser;
    bool disposed;
    internal ProviderParser(string name, ManagedProviders? owner = null)
    {
        services = new HostServices(input: owner == null ? null : owner.Get);
        var selected = InstalledRegistry.AcquireParser(name); generation = selected.Generation;
        object? value = null;
        try { value = HostServices.Construct(selected.Type) ?? throw new InvalidOperationException("Cannot construct original report parser.");
            services.Inject(selected.Type, value); parser = (IReportParser<IDeviceReport>)value; }
        catch { try { HostServices.DisposePlugin(value); } finally { services.Dispose(); if (generation != null) InstalledRegistry.Release(generation); } throw; }
    }
    public IDeviceReport Parse(byte[] data) { using var reportScope = ServiceClient.Report(); lock (gate) { ObjectDisposedException.ThrowIf(disposed, this); return parser.Parse(data); } }
    public void Dispose() { lock (gate) { if (disposed) return; disposed = true;
        try { HostServices.DisposePlugin(parser); }
        finally { services.Dispose(); if (generation != null) InstalledRegistry.Release(generation); }
    } }
}

sealed partial class HostedEndpoint(JObject data, ulong scope, CancellationToken cancellation) : OpenTabletDriver.IHostedDeviceEndpoint
{
    T Property<T>(string name) => data[name] is { Type: not JTokenType.Null } value ? value.ToObject<T>()!
        : throw new NotSupportedException($"Cached endpoint '{DevicePath}' has no '{name}' metadata.");
    public int ProductID => Property<int>(nameof(ProductID));
    public int VendorID => Property<int>(nameof(VendorID));
    public int InputReportLength => Property<int>(nameof(InputReportLength));
    public int OutputReportLength => Property<int>(nameof(OutputReportLength));
    public int FeatureReportLength => Property<int>(nameof(FeatureReportLength));
    public string Manufacturer => Property<string>(nameof(Manufacturer));
    public string ProductName => Property<string>(nameof(ProductName));
    public string FriendlyName => Property<string>(nameof(FriendlyName));
    public string SerialNumber => Property<string>(nameof(SerialNumber));
    public string DevicePath => data.Value<string>(nameof(DevicePath)) ?? throw new InvalidOperationException("Endpoint has no actual path.");
    public bool CanOpen => Property<bool>(nameof(CanOpen));
    public IDictionary<string, string> DeviceAttributes => Property<Dictionary<string, string>>(nameof(DeviceAttributes));
    public IDeviceEndpointStream Open() => HostedSharedStream.Open(data, scope, cancellation);
    public string GetDeviceString(byte index)
    {
        cancellation.ThrowIfCancellationRequested();
        if (data["DeviceStrings"] is JObject strings && strings[index.ToString(System.Globalization.CultureInfo.InvariantCulture)] is { } cached)
            return cached.Value<string>()!;
        ServiceClient.RequireIoAllowed();
        return ServiceClient.Request(3, scope, new JObject { ["path"] = DevicePath, ["index"] = index }, cancellation)
            .GetAwaiter().GetResult().Value<string>()!;
    }
}

static class HostedDesktop
{
    static readonly object Gate = new();
    static AppInfo? application;
    static HostPluginManager? manager;
    internal static AppInfo Application { get { Configure(); return application!; } }
    internal static HostPluginManager Manager { get { Configure(); return manager!; } }
    internal static void Configure()
    {
        lock (Gate) { if (application != null) return; Configure(ManagedProviders.Read<AppInfo>("application_info")); }
    }
    internal static void Configure(AppInfo info)
    {
        lock (Gate) {
            if (application != null) return;
            AppInfo.Current = info;
            var next = new HostPluginManager(info);
            AppInfo.PluginManager = next;
            AppInfo.PresetManager = new PresetManager();
            AppInfo.PresetManager.Refresh();
            manager = next; application = info;
        }
    }
}

// Actual Desktop type identity with exact host services instead of ResetServices
// constructing upstream Driver/RootHub and creating another input owner.
sealed class HostPluginManager : DesktopPluginManager
{
    readonly HostServices services = new();
    readonly TypeInfo[] builtinTypes;
    static readonly ConditionalWeakTable<object, ConstructedLease> constructed = new();
    internal HostPluginManager(AppInfo info) : base(new DirectoryInfo(info.PluginDirectory), new DirectoryInfo(info.TrashDirectory), new DirectoryInfo(info.TemporaryDirectory)) {
        builtinTypes = pluginTypes.ToArray(); ResetServices();
    }
    internal void RefreshTypes(RegistryGeneration registry) {
        pluginTypes = new ConcurrentBag<TypeInfo>(builtinTypes.Concat(registry.Entries.Select(entry => entry.Type.GetTypeInfo())).Distinct());
        Plugins.Clear(); Plugins.AddRange(registry.Contexts);
    }
    public override void ResetServices() {
        // Clear the actual pinned ServiceManager dictionary; a base call would
        // dispatch Desktop's input factories. No upstream source is modified.
        var field = typeof(ServiceManager).GetField("services", BindingFlags.Instance | BindingFlags.NonPublic)
            ?? throw new MissingFieldException("Pinned ServiceManager.services unavailable.");
        ((Dictionary<Type, Func<object>>)field.GetValue(this)!).Clear();
        // Register only owned providers, avoiding DesktopInterop input factories.
        // Delegates keep their real scope; Reset never changes reader ownership.
        AddService<IServiceProvider>(() => services);
        AddService<IDriver>(() => (IDriver)services.GetService(typeof(IDriver))!);
        AddService<OpenTabletDriver.Driver>(() => (OpenTabletDriver.Driver)services.GetService(typeof(OpenTabletDriver.Driver))!);
        AddService<OpenTabletDriver.InputDeviceTree>(() => (OpenTabletDriver.InputDeviceTree)services.GetService(typeof(OpenTabletDriver.InputDeviceTree))!);
        AddService<OpenTabletDriver.InputDevice>(() => (OpenTabletDriver.InputDevice)services.GetService(typeof(OpenTabletDriver.InputDevice))!);
        AddService<OpenTabletDriver.Devices.RootHub>(() => (OpenTabletDriver.Devices.RootHub)services.GetService(typeof(OpenTabletDriver.Devices.RootHub))!);
        AddService<IDriverDaemon>(() => (IDriverDaemon)services.GetService(typeof(IDriverDaemon))!);
        AddService<IDeviceConfigurationProvider>(() => (IDeviceConfigurationProvider)services.GetService(typeof(IDeviceConfigurationProvider))!);
        AddService<IReportParserProvider>(() => (IReportParserProvider)services.GetService(typeof(IReportParserProvider))!);
        AddService<IDeviceHubsProvider>(() => (IDeviceHubsProvider)services.GetService(typeof(IDeviceHubsProvider))!);
        AddService<IDeviceHub>(() => (IDeviceHub)services.GetService(typeof(IDeviceHub))!);
        AddService<ICompositeDeviceHub>(() => (ICompositeDeviceHub)services.GetService(typeof(ICompositeDeviceHub))!);
        AddService<OpenTabletDriver.Plugin.Timers.ITimer>(() => (OpenTabletDriver.Plugin.Timers.ITimer)services.GetService(typeof(OpenTabletDriver.Plugin.Timers.ITimer))!);
        AddService<OpenTabletDriver.Plugin.Platform.Pointer.IAbsolutePointer>(() => (OpenTabletDriver.Plugin.Platform.Pointer.IAbsolutePointer)services.GetService(typeof(OpenTabletDriver.Plugin.Platform.Pointer.IAbsolutePointer))!);
        AddService<OpenTabletDriver.Plugin.Platform.Pointer.IRelativePointer>(() => (OpenTabletDriver.Plugin.Platform.Pointer.IRelativePointer)services.GetService(typeof(OpenTabletDriver.Plugin.Platform.Pointer.IRelativePointer))!);
        AddService<OpenTabletDriver.Plugin.Platform.Keyboard.IVirtualKeyboard>(() => (OpenTabletDriver.Plugin.Platform.Keyboard.IVirtualKeyboard)services.GetService(typeof(OpenTabletDriver.Plugin.Platform.Keyboard.IVirtualKeyboard))!);
        AddService<OpenTabletDriver.Plugin.Platform.Pointer.IPressureHandler>(() => (OpenTabletDriver.Plugin.Platform.Pointer.IPressureHandler)services.GetService(typeof(OpenTabletDriver.Plugin.Platform.Pointer.IPressureHandler))!);
        AddService<OpenTabletDriver.Plugin.Platform.Keyboard.IVirtualPad>(() => (OpenTabletDriver.Plugin.Platform.Keyboard.IVirtualPad)services.GetService(typeof(OpenTabletDriver.Plugin.Platform.Keyboard.IVirtualPad))!);
        AddService<OpenTabletDriver.Plugin.Platform.Display.IVirtualScreen>(() =>
            (OpenTabletDriver.Plugin.Platform.Display.IVirtualScreen)services.GetService(typeof(OpenTabletDriver.Plugin.Platform.Display.IVirtualScreen))!);
        AddService<PluginManager>(() => this);
        AddService<DesktopPluginManager>(() => this);
        AddService<IServiceManager>(() => this);
        AddService<AppInfo>(() => HostedDesktop.Application);
        AddService<PresetManager>(() => AppInfo.PresetManager);
        AddService<DesktopDeviceConfigurationProvider>(() => (DesktopDeviceConfigurationProvider)services.GetService(typeof(DesktopDeviceConfigurationProvider))!);
        AddService<DesktopReportParserProvider>(() => (DesktopReportParserProvider)services.GetService(typeof(DesktopReportParserProvider))!);
        AddService<OpenTabletDriver.Configurations.DeviceConfigurationProvider>(() =>
            (OpenTabletDriver.Configurations.DeviceConfigurationProvider)services.GetService(typeof(OpenTabletDriver.Configurations.DeviceConfigurationProvider))!);
        AddService<OpenTabletDriver.Configurations.ReportParserProvider>(() =>
            (OpenTabletDriver.Configurations.ReportParserProvider)services.GetService(typeof(OpenTabletDriver.Configurations.ReportParserProvider))!);
    }
    public override T ConstructObject<T>(string name, object[] args)
    {
        using var setup = ServiceClient.Setup();
        var selected = InstalledRegistry.AcquireType(name);
        if (selected == null) return base.ConstructObject<T>(name, args);
        var (type, generation) = selected.Value;
        var ownedServices = new HostServices();
        object? value = null;
        try {
            value = HostServices.Construct(type, args) ?? throw new InvalidOperationException($"Cannot construct '{name}'.");
            if (value is not T result) throw new InvalidCastException($"'{name}' does not implement '{typeof(T)}'.");
            ownedServices.Inject(type, value);
            constructed.Add(value, new ConstructedLease(ownedServices, generation));
            return result;
        } catch { try { HostServices.DisposePlugin(value); } finally { ownedServices.Dispose(); InstalledRegistry.Release(generation); } throw; }
    }
    public override IReadOnlyCollection<TypeInfo> GetChildTypes<T>() => base.GetChildTypes<T>()
        .Concat(InstalledRegistry.TypeSnapshot().Where(type => typeof(T).IsAssignableFrom(type)).Select(type => type.GetTypeInfo()))
        .Distinct().ToArray();
    sealed class ConstructedLease(HostServices services, RegistryGeneration generation)
    {
        // Caller owns plugin IDisposable; this lease keeps its real dependencies
        // loaded until the returned object itself is no longer reachable.
        ~ConstructedLease() {
            // Third-party IDisposable errors remain visible during explicit
            // cleanup, but must never escape the CLR finalizer thread.
            try { services.Dispose(); } catch (Exception) { }
            try { InstalledRegistry.Release(generation); } catch (Exception) { }
        }
    }
}

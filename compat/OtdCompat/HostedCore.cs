using Newtonsoft.Json.Linq;
using OpenTabletDriver;
using OpenTabletDriver.Devices;
using OpenTabletDriver.Plugin;
using OpenTabletDriver.Plugin.Components;
using OpenTabletDriver.Plugin.Devices;
using OpenTabletDriver.Plugin.Tablet;

namespace OtdCompat;

// Concrete upstream objects retain exact identities. Their only readers are
// original managed parser threads consuming already owned native tee streams.
sealed class HostedCore : IDisposable, IServiceProvider
{
    readonly ManagedProviders providers;
    readonly ulong scope;
    readonly CancellationToken cancellation;
    readonly JObject? source;
    readonly object gate;
    readonly Dictionary<string, (InputDeviceTree Tree, JObject Identity)> trees = new();
    bool disposed;
    internal RootHub Root { get; }
    internal Driver Driver { get; }
    internal HostedCore(ManagedProviders providers, ulong scope, CancellationToken cancellation, JObject? source)
    {
        gate = providers.Sync;
        this.providers = providers; this.scope = scope; this.cancellation = cancellation; this.source = source;
        Root = RootHub.WithProvider(this);
        Root.HostedEndpointTransform = endpoint => endpoint is HostedEndpoint ? endpoint : HostedCustomDevices.Wrap(endpoint, scope, cancellation);
        Root.HostedEndpointsChanged = endpoints => HostedCustomDevices.Publish(scope, endpoints);
        Driver = new NativeDriver(this, Root, providers, providers);
        providers.DevicesChanged += NativeChanged;
        providers.TabletsChanged += NativeTabletsChanged;

    }
    void NativeChanged(object? sender, DevicesChangedEventArgs args) => RefreshAfterChange();
    void NativeTabletsChanged(object? sender, IEnumerable<TabletReference> args) => RefreshAfterChange();
    void RefreshAfterChange() {
        try { lock (gate) { if (!disposed) Refresh(); } }
        catch (Exception error) { Log.Exception(error); }
    }
    public object? GetService(Type type) => type == typeof(IDeviceHubsProvider) ? providers : providers.Get(type);
    internal bool Refresh()
    {
        ServiceClient.RequireIoAllowed();
        lock (gate) {
            ObjectDisposedException.ThrowIf(disposed, this);
            cancellation.ThrowIfCancellationRequested();
            var endpoints = ServiceClient.Snapshot()["devices"] as JArray
                ?? throw new NotSupportedException("Actual native endpoint metadata is unavailable.");
            var groups = endpoints.OfType<JObject>().Where(e => e["reader_generation"] != null && e["Configuration"] is JObject && e["Identifier"] is JObject)
                .GroupBy(e => (Session: e.Value<string>("session_id") ?? throw new InvalidOperationException("Owned endpoint has no session identity."), Reader: e.Value<ulong>("reader_generation")));
            var replacement = new Dictionary<string, (InputDeviceTree Tree, JObject Identity)>();
            try {
                foreach (var group in groups) {
                    var owned = group.ToArray();
                    string treeKey = group.Key.Session + ":" + group.Key.Reader;
                    // A cold prepared reader and the current reader may share a path.
                    // Exact reader identity, not VID/PID/path-first selection, owns DI.
                    if (source != null) {
                        if (source.Value<string>("id") != group.Key.Session) continue;
                        if (source.Value<ulong?>("reader_generation") is { } expected
                            && group.Key.Reader != expected) continue;
                    }
                    JObject identity = new() { ["session_id"] = group.Key.Session,
                        ["device_generation"] = owned[0]["device_generation"], ["reader_generation"] = owned[0]["reader_generation"],
                        ["members"] = new JArray(owned.Select(e => new JObject { ["path"] = e["DevicePath"], ["reader_generation"] = e["reader_generation"] })) };
                    if (trees.TryGetValue(treeKey, out var existing) && JToken.DeepEquals(existing.Identity, identity)) {
                        replacement.Add(treeKey, existing); continue;
                    }
                    var configuration = owned[0]["Configuration"]!.ToObject<TabletConfiguration>()!;
                    var readers = new List<InputDevice>();
                    try {
                        foreach (var endpoint in owned) {
                            var identifier = endpoint["Identifier"]!.ToObject<DeviceIdentifier>()!;
                            var reader = new InputDevice(Driver, new HostedEndpoint(endpoint, scope, cancellation), configuration, identifier);
                            if (reader.ReportStream == null) { reader.Dispose(); throw new IOException("Original reader could not acquire an actual shared stream."); }
                            reader.HostedReportScope = () => ServiceClient.Report();
                            readers.Add(reader);
                        }
                        var tree = new InputDeviceTree(configuration, readers);
                        tree.HostedOutputOwnership = enabled => SetOutput(identity, enabled);
                        replacement.Add(treeKey, (tree, identity));
                    } catch { foreach (var reader in readers) reader.Dispose(); throw; }
                }
                Driver.PublishHostedTrees(replacement.Values.Select(value => value.Tree));
                foreach (var previous in trees) {
                    if (!replacement.TryGetValue(previous.Key, out var retained) || !ReferenceEquals(previous.Value.Tree, retained.Tree)) {
                        var release = (JObject)previous.Value.Identity.DeepClone(); release.Remove("members"); release["managed_output"] = false;
                        _ = RetireAndRelease(previous.Value.Tree, release);
                    }
                }
                trees.Clear(); foreach (var item in replacement) trees.Add(item.Key, item.Value);
                return trees.Count != 0;
            } catch {
                foreach (var entry in replacement) if (!trees.TryGetValue(entry.Key, out var old) || !ReferenceEquals(old.Tree, entry.Value.Tree))
                    foreach (var reader in entry.Value.Tree.InputDevices) reader.Dispose();
                throw;
            }
        }
    }
    void SetOutput(JObject identity, bool enabled)
    {
        ServiceClient.RequireIoAllowed();
        cancellation.ThrowIfCancellationRequested();
        JObject request = (JObject)identity.DeepClone(); request.Remove("members"); request["managed_output"] = enabled;
        using var deadline = CancellationTokenSource.CreateLinkedTokenSource(cancellation); deadline.CancelAfter(TimeSpan.FromSeconds(5));
        ServiceClient.Request(11, scope, request, deadline.Token).GetAwaiter().GetResult();
    }
    internal InputDeviceTree SelectedTree()
    {
        lock (gate) {
            ObjectDisposedException.ThrowIf(disposed, this);
            if (Driver.InputDevices.Length != 1) throw new InvalidOperationException("Concrete InputDeviceTree injection requires an exact source context or one uniquely owned tablet.");
            return Driver.InputDevices.Single();
        }
    }
    internal InputDevice SelectedInput() => SelectedTree().InputDevices.FirstOrDefault()
        ?? throw new InvalidOperationException("Concrete InputDevice injection has no opened primary endpoint.");
    public void Dispose()
    {
        lock (gate) {
            if (disposed) return; disposed = true;
            foreach (var item in trees.Values) {
                var payload = (JObject)item.Identity.DeepClone(); payload.Remove("members"); payload["managed_output"] = false;
                // Scope retirement must not synchronously await its own native apply.
                _ = RetireAndRelease(item.Tree, payload);
            }
            providers.DevicesChanged -= NativeChanged;
            providers.TabletsChanged -= NativeTabletsChanged;
            Driver?.Dispose(); trees.Clear();
            Root.HostedDispose();
            HostedCustomDevices.Remove(scope);
        }
    }
    async Task RetireAndRelease(InputDeviceTree tree, JObject payload)
    {
        // Drain the shadow callback independently of the native apply thread.
        // Its output ownership stays delegated until this drain completes.
        await Task.Run(tree.HostedRetire).ConfigureAwait(false);
        await ReleaseOutput(payload).ConfigureAwait(false);
    }
    async Task ReleaseOutput(JObject payload)
    {
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(2));
        try { await ServiceClient.Request(11, scope, payload, deadline.Token).ConfigureAwait(false); }
        catch (Exception) { /* native retiring generation clears its ownership */ }
    }
    sealed class NativeDriver(HostedCore owner, ICompositeDeviceHub hub, IReportParserProvider parser, IDeviceConfigurationProvider configuration)
        : Driver(hub, parser, configuration)
    {
        public override bool Detect() => owner.Refresh();
    }
}

using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

namespace OtdCompat;

[StructLayout(LayoutKind.Sequential)]
public unsafe struct ServiceCallbacks
{
    public uint Version, Size;
    public delegate* unmanaged[Cdecl]<uint, ulong, byte*, uint, ulong*, int> Request;
    public delegate* unmanaged[Cdecl]<ulong, byte*, uint, int> Poll;
    public delegate* unmanaged[Cdecl]<ulong, void> Release;
}

// ABI admission never means applied. Mutating Tasks await a retained completion
// without blocking the report thread or reexecuting after a lost/capacity retry.
static class ServiceClient
{
    static ServiceCallbacks callbacks;
    static bool installed;
    static readonly object SnapshotGate = new();
    static JObject? snapshot;
    static Task? snapshotMonitor;
    internal static readonly object Gate = new();
    internal static bool Available { get { lock (Gate) return installed; } }
    [ThreadStatic] internal static int ReportDepth;
    [ThreadStatic] internal static int SetupDepth;
    [ThreadStatic] internal static uint? BindingOwner;
    [ThreadStatic] internal static string? BindingTablet;
    internal static unsafe void Install(ServiceCallbacks value)
    {
        if (value.Version != 1 || value.Size != sizeof(ServiceCallbacks)
            || value.Request == null || value.Poll == null || value.Release == null)
            throw new ArgumentException("Invalid managed host service ABI.");
        lock (Gate) { if (installed) throw new InvalidOperationException("Host services already installed."); callbacks = value; installed = true; }
    }
    static unsafe ulong Admit(uint operation, ulong scope, JToken? payload)
    {
        ServiceCallbacks api;
        lock (Gate) { if (!installed) throw new NotSupportedException("Native managed service owner is unavailable."); api = callbacks; }
        byte[] bytes = Encoding.UTF8.GetBytes(payload?.ToString(Formatting.None) ?? "null");
        ulong ticket = 0;
        fixed (byte* data = bytes) {
            int code = api.Request(operation, scope, data, (uint)bytes.Length, &ticket);
            if (code != 0) throw new InvalidOperationException($"Managed service admission failed ({code}); no operation was accepted.");
        }
        return ticket;
    }
    static unsafe JToken? Completed(ulong ticket)
    {
        int size = callbacks.Poll(ticket, null, 0);
        if (size == 0) return null;
        if (size < 0) throw new InvalidOperationException("Managed service ticket expired or owner stopped; application status may be unknown.");
        if (size > 4194304) throw new InvalidOperationException("Managed service reply exceeds 4 MiB.");
        byte[] data = new byte[size];
        fixed (byte* output = data) {
            int copied = callbacks.Poll(ticket, output, (uint)data.Length);
            if (copied != size) throw new InvalidOperationException("Managed service completion changed or expired.");
        }
        JObject reply = JObject.Parse(Encoding.UTF8.GetString(data));
        if (reply.Value<bool?>("ok") != true) throw new InvalidOperationException(reply.Value<string>("error") ?? "Managed host service failed.");
        return reply["result"] ?? JValue.CreateNull();
    }
    static JObject FetchSnapshot()
    {
        ulong ticket = Admit(0, 0, null);
        try { return Completed(ticket) as JObject ?? throw new InvalidOperationException("Managed host snapshot is unavailable."); }
        finally { Release(ticket); }
    }
    internal static JObject Snapshot()
    {
        if (Volatile.Read(ref snapshot) is { } cached) return cached;
        if (ReportDepth != 0 || SynchronousGraph.CurrentReport != null)
            throw new NotSupportedException("Managed host snapshot is not ready for report callbacks.");
        lock (SnapshotGate) {
            snapshotMonitor ??= Task.Run(MonitorSnapshots);
            if (snapshot == null) {
                // Cold setup may prime the first view. A report callback never
                // pulls/parses the configuration document on this path.
                Volatile.Write(ref snapshot, FetchSnapshot());
            }
            return snapshot;
        }
    }
    internal static void RefreshSnapshot()
    {
        // Serialize fresh reads to prevent an older periodic snapshot replacing
        // the post-commit view. Backend mutation completion never holds this gate.
        lock (SnapshotGate) Volatile.Write(ref snapshot, FetchSnapshot());
    }
    internal static void RefreshAfterCommit() {
        try { RefreshSnapshot(); }
        catch (Exception) { lock (SnapshotGate) Volatile.Write(ref snapshot, null); }
    }
    static async Task MonitorSnapshots()
    {
        for (;;) {
            await Task.Delay(250).ConfigureAwait(false);
            try { RefreshSnapshot(); }
            catch (Exception) { lock (SnapshotGate) Volatile.Write(ref snapshot, null); }
        }
    }
    internal static Task<JToken> Request(uint operation, ulong scope, JToken? payload, CancellationToken cancellation)
    {
        cancellation.ThrowIfCancellationRequested();
        ulong ticket;
        try { ticket = Admit(operation, scope, payload); }
        catch (Exception error) { return Task.FromException<JToken>(error); }
        // Delegate unsafe calls to synchronous methods; the async state machine
        // itself contains no pointer lifetime and works with net8/C#12.
        return AwaitCompletion(ticket, cancellation);
    }
    static async Task<JToken> AwaitCompletion(ulong ticket, CancellationToken cancellation)
    {
        using var cancel = cancellation.Register(() => Release(ticket));
        try {
            for (;;) {
                cancellation.ThrowIfCancellationRequested();
                if (Completed(ticket) is { } result) return result;
                await Task.Delay(10, cancellation).ConfigureAwait(false);
            }
        } finally { Release(ticket); }
    }
    static unsafe void Release(ulong ticket) => callbacks.Release(ticket);
    internal static void RequireBlockingAllowed()
    {
        if (ReportDepth != 0 || SetupDepth != 0 || SynchronousGraph.CurrentReport != null)
            throw new InvalidOperationException("Synchronous driver/device operations cannot wait on their own report callback. Use the asynchronous IDriverDaemon operation.");
    }
    internal readonly struct SetupScope : IDisposable {
        public SetupScope() { SetupDepth++; }
        public void Dispose() { SetupDepth--; }
    }
    internal static SetupScope Setup() => new();
    // Covers the whole managed callback, including filters, parser properties,
    // output modes and due timers. CurrentReport only surrounds native
    // continuations, so it cannot guard the plugin code on either side of them.
    // A using-local calls Dispose directly without boxing or report allocation;
    // nested callbacks restore the previous depth even when a plugin throws.
    internal readonly struct ReportScope : IDisposable {
        public ReportScope() { ReportDepth++; }
        public void Dispose() { ReportDepth--; }
    }
    internal static ReportScope Report() => new();
}

public static unsafe partial class EntryPoints
{
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int ConfigureHostedApplication(byte* json, uint length)
    {
        try {
            if (json == null || length == 0 || length > 1048576) throw new ArgumentException("Invalid AppInfo bootstrap.");
            var info = JsonConvert.DeserializeObject<OpenTabletDriver.Desktop.AppInfo>(Encoding.UTF8.GetString(new ReadOnlySpan<byte>(json, checked((int)length))))
                ?? throw new ArgumentException("AppInfo bootstrap is null.");
            HostedDesktop.Configure(info); return 0;
        } catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int InstallHostServices(ServiceCallbacks* callbacks)
    {
        try { if (callbacks == null) throw new ArgumentNullException(nameof(callbacks)); ServiceClient.Install(*callbacks); return 0; }
        catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }
}

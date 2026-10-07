using Newtonsoft.Json.Linq;
using OpenTabletDriver.Plugin.Devices;

namespace OtdCompat;

// Stream reads consume a bounded native tee, never a second physical Read.
sealed class HostedSharedStream : IDeviceEndpointStream
{
    readonly ulong scope, stream;
    readonly int reportLength;
    readonly CancellationTokenSource lifetime;
    readonly object readGate = new();
    ulong cursor;
    int disposed;
    HostedSharedStream(ulong scope, JObject opened, CancellationToken owner)
    {
        this.scope = scope;
        stream = opened.Value<ulong>("stream");
        if (stream == 0) throw new IOException("Native stream lease has no identity.");
        reportLength = opened.Value<int>("report_length");
        if (reportLength < 1 || reportLength > 65535) throw new IOException("Invalid native input report length.");
        cursor = opened.Value<ulong>("last_sequence");
        lifetime = CancellationTokenSource.CreateLinkedTokenSource(owner);
    }
    internal static HostedSharedStream Open(JObject endpoint, ulong scope, CancellationToken cancellation)
    {
        ServiceClient.RequireIoAllowed();
        JObject request = new() { ["path"] = endpoint["DevicePath"], ["lease_ms"] = 15000, ["capacity_reports"] = 256 };
        if (endpoint.Value<ulong?>("reader_generation") is { } reader) request["reader_generation"] = reader;
        if (endpoint["session_id"] != null) request["session_id"] = endpoint["session_id"];
        if (endpoint["device_generation"] != null) request["device_generation"] = endpoint["device_generation"];
        using var deadline = CancellationTokenSource.CreateLinkedTokenSource(cancellation);
        deadline.CancelAfter(TimeSpan.FromSeconds(5));
        var result = ServiceClient.Request(4, scope, request, deadline.Token).GetAwaiter().GetResult() as JObject
            ?? throw new IOException("Native open did not return a stream lease.");
        return new HostedSharedStream(scope, result, cancellation);
    }
    JToken Call(uint operation, JObject payload, CancellationToken cancellation)
    {
        ObjectDisposedException.ThrowIf(Volatile.Read(ref disposed) != 0, this);
        payload["stream"] = stream;
        using var deadline = CancellationTokenSource.CreateLinkedTokenSource(cancellation);
        deadline.CancelAfter(TimeSpan.FromSeconds(5));
        return ServiceClient.Request(operation, scope, payload, deadline.Token).GetAwaiter().GetResult();
    }
    public byte[] Read()
    {
        ServiceClient.RequireIoAllowed();
        lock (readGate) {
            for (;;) {
                lifetime.Token.ThrowIfCancellationRequested();
                var reply = Call(5, new JObject { ["after_sequence"] = cursor, ["limit"] = 1 }, lifetime.Token);
                if (reply.Value<ulong>("lost_reports") != 0) throw new IOException("Shared report stream overflowed; parser continuity is lost.");
                var reports = reply["reports"] as JArray ?? throw new IOException("Invalid shared report batch.");
                if (reports.Count > 1) throw new IOException("Shared stream exceeded requested report limit.");
                if (reports.Count == 1) {
                    ulong sequence = reports[0].Value<ulong>("sequence");
                    if (cursor == ulong.MaxValue || sequence != cursor + 1) throw new IOException("Shared report stream sequence gap.");
                    string hex = reports[0].Value<string>("data") ?? throw new IOException("Shared report has no bytes.");
                    if (hex.Length < 2 || hex.Length % 2 != 0 || hex.Length > reportLength * 2) throw new IOException("Invalid shared report length.");
                    byte[] bytes = Convert.FromHexString(hex);
                    cursor = sequence;
                    return bytes;
                }
                if (reply.Value<bool>("closed")) throw new IOException("I/O disconnected.");
                if (reply.Value<ulong>("next_sequence") != cursor) throw new IOException("Shared report cursor advanced without data.");
                // No CPU spin and no blocking native owner read. Disposal interrupts.
                if (lifetime.Token.WaitHandle.WaitOne(10)) lifetime.Token.ThrowIfCancellationRequested();
            }
        }
    }
    public void Write(byte[] buffer) => Write(6, buffer);
    public void SetFeature(byte[] buffer) => Write(8, buffer);
    void Write(uint operation, byte[] buffer)
    {
        ServiceClient.RequireIoAllowed();
        ArgumentNullException.ThrowIfNull(buffer);
        if (buffer.Length is < 1 or > 65535) throw new ArgumentException("I/O buffer must have 1..65535 bytes.");
        Call(operation, new JObject { ["data"] = Convert.ToHexString(buffer) }, lifetime.Token);
    }
    public void GetFeature(byte[] buffer)
    {
        ServiceClient.RequireIoAllowed();
        ArgumentNullException.ThrowIfNull(buffer);
        if (buffer.Length is < 1 or > 65535) throw new ArgumentException("Feature buffer must have 1..65535 bytes.");
        var value = Call(7, new JObject { ["data"] = Convert.ToHexString(buffer) }, lifetime.Token);
        string hex = value.Value<string>() ?? throw new IOException("Feature operation returned no data.");
        if (hex.Length != buffer.Length * 2) throw new IOException("Feature reply length differs from the supplied buffer.");
        Convert.FromHexString(hex).CopyTo(buffer, 0);
    }
    public void Dispose()
    {
        if (Interlocked.Exchange(ref disposed, 1) != 0) return;
        lifetime.Cancel();
        // Close admission is nonblocking; a callback disposing its own reader
        // must not wait for native output handoff. Lease expiry is the fallback.
        _ = Close();
    }
    async Task Close()
    {
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(2));
        try { await ServiceClient.Request(9, scope, new JObject { ["stream"] = stream }, deadline.Token).ConfigureAwait(false); }
        catch (Exception) { /* expired generation already closes this lease */ }
        finally { lifetime.Dispose(); }
    }
}

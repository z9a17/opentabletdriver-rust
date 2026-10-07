using Newtonsoft.Json.Linq;
using OpenTabletDriver.Desktop.RPC;
using OpenTabletDriver.Plugin;
using OpenTabletDriver.Plugin.Tablet;

namespace OtdCompat;

sealed partial class ManagedProviders
{
    CancellationTokenSource? debugCancellation;
    Task<JToken>? debugAdmission;
    long debugEpoch;
    Task ConfigureDebug(bool enabled)
    {
        lock (gate) {
            ObjectDisposedException.ThrowIf(disposed, this);
            if (!enabled) { CancelDebug(); return DisarmDebug(); }
            if (debugCancellation != null) return (Task?)debugAdmission ?? Task.CompletedTask;
            var cancellation = CancellationTokenSource.CreateLinkedTokenSource(lifetime.Token);
            debugCancellation = cancellation;
            long epoch = checked(++debugEpoch);
            // Admission is linearized with lifetime disposal, but waits never
            // hold the provider gate or run on the physical report thread.
            var admitted = ServiceClient.Request(10, scope, new JObject { ["enabled"] = true, ["cursor"] = 0, ["limit"] = 32 }, cancellation.Token);
            debugAdmission = admitted;
            _ = Task.Run(() => DeliverDebug(admitted, epoch, cancellation));
            return admitted;
        }
    }
    async Task StopDisposedDebug()
    {
        try { await DisarmDebug().ConfigureAwait(false); } catch (Exception) { /* native scope expiry also disarms */ }
    }
    void CancelDebug()
    {
        debugCancellation?.Cancel(); debugCancellation = null; debugAdmission = null;
        debugEpoch++;
    }
    async Task DebugFailure(Exception error)
    {
        var entry = new OpenTabletDriver.Plugin.Logging.LogMessage(error);
        EventHandler<OpenTabletDriver.Plugin.Logging.LogMessage>? handler;
        lock (gate) handler = message;
        Notify(() => handler?.Invoke(this, entry));
        try { await WriteMessage(entry).ConfigureAwait(false); }
        catch (Exception) { Console.Error.WriteLine(error.GetBaseException().Message); }
    }
    async Task DisarmDebug()
    {
        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(2));
        await ServiceClient.Request(10, scope, new JObject { ["enabled"] = false, ["cursor"] = 0, ["limit"] = 1 }, deadline.Token).ConfigureAwait(false);
    }
    async Task DeliverDebug(Task<JToken> admitted, long epoch, CancellationTokenSource cancellation)
    {
        var decoders = new Dictionary<string, ProviderParser>();
        try {
            JToken initial = await admitted.ConfigureAwait(false);
            ulong cursor = initial.Value<ulong>("next_sequence");
            if ((initial["events"] as JArray)?.Count > 0) throw new IOException("Debug arming must return a cursor before delivering events.");
            for (;;) {
                cancellation.Token.ThrowIfCancellationRequested();
                Task<JToken> reading;
                lock (gate) {
                    if (disposed || epoch != debugEpoch) return;
                    reading = ServiceClient.Request(10, scope, new JObject { ["cursor"] = cursor, ["limit"] = 32 }, cancellation.Token);
                }
                JToken reply = await reading.ConfigureAwait(false);
                if (reply.Value<ulong>("lost_reports") != 0) throw new IOException("Managed debug source overflowed; concrete parser continuity is lost.");
                var events = reply["events"] as JArray ?? throw new IOException("Managed debug source returned no event array.");
                if (events.Count > 32) throw new IOException("Managed debug source exceeded its requested limit.");
                foreach (var item in events) {
                    cancellation.Token.ThrowIfCancellationRequested();
                    ulong sequence = item.Value<ulong>("sequence");
                    if (cursor == ulong.MaxValue || sequence != cursor + 1) throw new IOException("Managed debug source sequence gap.");
                    string parserName = item.Value<string>("parser") ?? throw new IOException("Managed debug report has no original parser type.");
                    string session = item.Value<string>("session_id") ?? throw new IOException("Managed debug report has no physical source identity.");
                    string key = session + ":" + item.Value<ulong>("device_generation") + ":" + item.Value<ulong>("reader_generation") + ":" + item.Value<bool>("auxiliary") + ":" + parserName;
                    if (!decoders.TryGetValue(key, out var parser)) {
                        if (decoders.Count >= 256) throw new IOException("Managed debug endpoint parser budget exceeded.");
                        parser = new ProviderParser(parserName, sourceSession: new JObject {
                            ["id"] = session, ["device_generation"] = item["device_generation"],
                            ["reader_generation"] = item["reader_generation"], ["auxiliary"] = item["auxiliary"] });
                        decoders.Add(key, parser);
                    }
                    string hex = item.Value<string>("raw") ?? throw new IOException("Managed debug report has no raw bytes.");
                    if (hex.Length is < 2 or > 131070 || hex.Length % 2 != 0) throw new IOException("Managed debug report has invalid length.");
                    var report = parser.Parse(Convert.FromHexString(hex));
                    var tablet = item["tablet"]?.ToObject<TabletReference>() ?? throw new IOException("Managed debug report has no actual tablet reference.");
                    EventHandler<DebugReportData>? handler;
                    lock (gate) { if (disposed || epoch != debugEpoch) return; handler = deviceReport; }
                    if (report != null) {
                        using var callback = ServiceClient.Report();
                        Notify(() => handler?.Invoke(this, new DebugReportData(tablet, report)));
                    }
                    cursor = sequence;
                }
                if (reply.Value<ulong>("next_sequence") != cursor) throw new IOException("Managed debug cursor advanced past undelivered reports.");
                if (reply.Value<bool>("closed")) throw new IOException("Managed debug source closed.");
                if (events.Count == 0) await Task.Delay(10, cancellation.Token).ConfigureAwait(false);
            }
        } catch (OperationCanceledException) when (cancellation.IsCancellationRequested) { }
        catch (Exception error) {
            // The original event has no loss field. A visible log plus stopped
            // stream is honest; silently delivering a sampled stream is not.
            await DebugFailure(error).ConfigureAwait(false);
        } finally {
            foreach (var parser in decoders.Values) {
                try { parser.Dispose(); } catch (Exception error) { Log.Exception(error); }
            }
            Task? disarm = null;
            lock (gate) {
                if (epoch == debugEpoch) {
                    debugCancellation = null; debugAdmission = null;
                    // Admit before releasing the gate: a later enable cannot be
                    // canceled by this older monitor's delayed finalizer.
                    disarm = DisarmDebug();
                }
            }
            if (disarm != null) { try { await disarm.ConfigureAwait(false); } catch (Exception error) { Log.Exception(error); } }
            cancellation.Dispose();
        }
    }
}

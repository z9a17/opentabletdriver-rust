using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;
using OpenTabletDriver.Plugin.Devices;

namespace OtdCompat;

// Registry owns real custom endpoint references. The native broker owns exactly
// one actual stream, and all concrete Core readers consume its shared native tee.
static class HostedCustomDevices
{
    internal sealed class Endpoint(ulong token, ulong scope, IDeviceEndpoint original, CancellationToken cancellation)
    {
        internal readonly ulong Token = token, Scope = scope;
        internal readonly IDeviceEndpoint Original = original;
        internal readonly CancellationToken Cancellation = cancellation;
        internal bool Active, Opening;
        internal ulong Stream;
        internal JObject Metadata() => new() {
            ["endpoint"] = Token, ["scope"] = Scope, ["DevicePath"] = Original.DevicePath,
            ["VendorID"] = Original.VendorID, ["ProductID"] = Original.ProductID,
            ["InputReportLength"] = Original.InputReportLength, ["OutputReportLength"] = Original.OutputReportLength,
            ["FeatureReportLength"] = Original.FeatureReportLength, ["CanOpen"] = Original.CanOpen,
            ["Manufacturer"] = Original.Manufacturer, ["ProductName"] = Original.ProductName,
            ["FriendlyName"] = Original.FriendlyName, ["SerialNumber"] = Original.SerialNumber,
            ["DeviceAttributes"] = Original.DeviceAttributes == null ? JValue.CreateNull() : JObject.FromObject(Original.DeviceAttributes) };
    }
    internal sealed class Stream(Endpoint endpoint, IDeviceEndpointStream original)
    {
        internal readonly Endpoint Endpoint = endpoint;
        internal readonly IDeviceEndpointStream Original = original;
        internal readonly object Reading = new(), Writing = new();
        internal byte[]? Pending;
        internal int Closed;
        readonly TaskCompletionSource completion = new(TaskCreationOptions.RunContinuationsAsynchronously);
        internal void Close() {
            if (Interlocked.CompareExchange(ref Closed, 1, 0) == 0) {
                try { Original.Dispose(); completion.TrySetResult(); }
                catch (Exception error) { completion.TrySetException(error); throw; }
            } else completion.Task.GetAwaiter().GetResult();
        }
    }
    static readonly object Gate = new();
    static readonly Dictionary<ulong, Endpoint> Endpoints = new();
    static readonly Dictionary<ulong, Stream> Streams = new();
    static ulong endpointIdentity, streamIdentity;
    internal static IDeviceEndpoint Wrap(IDeviceEndpoint original, ulong scope, CancellationToken cancellation)
    {
        if (original is CustomEndpoint wrapper) return wrapper;
        lock (Gate) {
            var existing = Endpoints.Values.FirstOrDefault(e => e.Scope == scope && ReferenceEquals(e.Original, original));
            if (existing != null) return new CustomEndpoint(existing);
            cancellation.ThrowIfCancellationRequested();
            if (Endpoints.Count >= 1024) throw new InvalidOperationException("Custom endpoint registry exceeds 1024 entries.");
            var entry = new Endpoint(checked(++endpointIdentity), scope, original, cancellation);
            Endpoints.Add(entry.Token, entry); return new CustomEndpoint(entry);
        }
    }
    internal static void Publish(ulong scope, IDeviceEndpoint[] devices)
    {
        var live = devices.OfType<CustomEndpoint>().Select(e => e.Entry.Token).ToHashSet();
        List<ulong> retired = [];
        lock (Gate) {
            foreach (var endpoint in Endpoints.Values.Where(e => e.Scope == scope)) {
                endpoint.Active = live.Contains(endpoint.Token);
                if (!endpoint.Active && endpoint.Stream != 0 && Streams.TryGetValue(endpoint.Stream, out var stream)) {
                    retired.Add(endpoint.Stream);
                }
            }
        }
        foreach (ulong token in retired) _ = Task.Run(() => { try { Close(token); } catch (Exception error) { Console.Error.WriteLine(error); } });
    }
    internal static void Remove(ulong scope)
    {
        List<ulong> retired = [];
        lock (Gate) {
            foreach (var entry in Endpoints.Values.Where(e => e.Scope == scope).ToArray()) {
                entry.Active = false; Endpoints.Remove(entry.Token);
                if (entry.Stream != 0 && Streams.TryGetValue(entry.Stream, out var stream)) retired.Add(entry.Stream);
            }
        }
        // Dispose can unblock Read, but never wait for that Read lock. Native
        // reader retirement performs the join after closing the original stream.
        foreach (ulong token in retired) _ = Task.Run(() => { try { Close(token); } catch (Exception error) { Console.Error.WriteLine(error); } });
    }
    internal static JObject[] Snapshot()
    {
        Endpoint[] entries;
        lock (Gate) entries = Endpoints.Values.Where(e => e.Active && !e.Cancellation.IsCancellationRequested).ToArray();
        return entries.Select(e => e.Metadata()).ToArray();
    }
    internal static ulong Open(ulong token)
    {
        Endpoint endpoint;
        lock (Gate) {
            endpoint = Endpoints.TryGetValue(token, out var found) ? found : throw new IOException("Custom endpoint retired.");
            endpoint.Cancellation.ThrowIfCancellationRequested();
            if (!endpoint.Active || endpoint.Stream != 0 || endpoint.Opening) throw new IOException("Custom endpoint is not active or already has its sole reader.");
            if (Streams.Count >= 256) throw new IOException("Custom stream registry exceeds 256 readers.");
            endpoint.Opening = true;
        }
        IDeviceEndpointStream? opened = null;
        try {
            opened = endpoint.Original.Open() ?? throw new IOException("Original custom endpoint Open returned null.");
            lock (Gate) {
                endpoint.Cancellation.ThrowIfCancellationRequested();
                if (!endpoint.Active || !Endpoints.ContainsKey(token)) throw new IOException("Custom endpoint retired during Open.");
                ulong stream = checked(++streamIdentity);
                Streams.Add(stream, new Stream(endpoint, opened)); endpoint.Stream = stream;
                opened = null; return stream;
            }
        } finally {
            lock (Gate) endpoint.Opening = false;
            opened?.Dispose();
        }
    }
    internal static string DeviceString(ulong token, byte index)
    {
        Endpoint endpoint;
        lock (Gate) endpoint = Endpoints.TryGetValue(token, out var found) && found.Active
            ? found : throw new IOException("Custom endpoint retired.");
        endpoint.Cancellation.ThrowIfCancellationRequested();
        return endpoint.Original.GetDeviceString(index);
    }
    internal static Stream Get(ulong token) {
        lock (Gate) return Streams.TryGetValue(token, out var stream) && Volatile.Read(ref stream.Closed) == 0
            ? stream : throw new IOException("Custom stream closed.");
    }
    internal static void Close(ulong token)
    {
        Stream? stream;
        lock (Gate) {
            if (!Streams.TryGetValue(token, out stream)) return;
        }
        stream.Close();
        lock (Gate) {
            Streams.Remove(token);
            if (stream.Endpoint.Stream == token) stream.Endpoint.Stream = 0;
        }
    }
    sealed class CustomEndpoint(Endpoint entry) : OpenTabletDriver.IHostedDeviceEndpoint
    {
        internal Endpoint Entry => entry;
        public int ProductID => entry.Original.ProductID;
        public int VendorID => entry.Original.VendorID;
        public int InputReportLength => entry.Original.InputReportLength;
        public int OutputReportLength => entry.Original.OutputReportLength;
        public int FeatureReportLength => entry.Original.FeatureReportLength;
        public string Manufacturer => entry.Original.Manufacturer;
        public string ProductName => entry.Original.ProductName;
        public string FriendlyName => entry.Original.FriendlyName;
        public string SerialNumber => entry.Original.SerialNumber;
        public string DevicePath => entry.Original.DevicePath;
        public bool CanOpen => entry.Original.CanOpen;
        public IDictionary<string, string> DeviceAttributes => entry.Original.DeviceAttributes;
        public string GetDeviceString(byte index) => entry.Original.GetDeviceString(index);
        public IDeviceEndpointStream Open() {
            entry.Cancellation.ThrowIfCancellationRequested();
            return HostedSharedStream.Open(new JObject { ["DevicePath"] = DevicePath, ["custom_endpoint"] = entry.Token }, entry.Scope, entry.Cancellation);
        }
    }
}

public static unsafe partial class EntryPoints
{
    [ThreadStatic] static byte[]? customDeviceJson;
    [ThreadStatic] static byte[]? customString;
    [ThreadStatic] static ulong customStringEndpoint;
    [ThreadStatic] static byte customStringIndex;
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int GetHostedDeviceString(ulong token, byte index, byte* output, int capacity)
    {
        try {
            if (output == null) {
                customString = Encoding.UTF8.GetBytes(HostedCustomDevices.DeviceString(token, index));
                customStringEndpoint = token; customStringIndex = index;
            }
            if (customString == null || customStringEndpoint != token || customStringIndex != index)
                throw new InvalidOperationException("No retained custom string query.");
            return CopyOriginal(customString, output, capacity);
        } catch (Exception error) { customString = null; lastError = error.GetBaseException().Message; return -1; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int GetHostedDevices(byte* output, int capacity, int refresh)
    {
        try {
            if (refresh != 0 || customDeviceJson == null) customDeviceJson = Encoding.UTF8.GetBytes(JArray.FromObject(HostedCustomDevices.Snapshot()).ToString(Formatting.None));
            return CopyOriginal(customDeviceJson, output, capacity);
        } catch (Exception error) { customDeviceJson = null; lastError = error.GetBaseException().Message; return -1; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static ulong OpenHostedDevice(ulong endpoint)
    {
        try { ServiceClient.RequireIoAllowed(); return HostedCustomDevices.Open(endpoint); }
        catch (Exception error) { lastError = error.GetBaseException().Message; return 0; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int ReadHostedDevice(ulong token, byte* output, int capacity)
    {
        try {
            if (capacity < 0 || capacity > 65535) throw new ArgumentException("Invalid custom input capacity.");
            var stream = HostedCustomDevices.Get(token);
            lock (stream.Reading) {
                ObjectDisposedException.ThrowIf(Volatile.Read(ref stream.Closed) != 0, stream);
                if (stream.Pending == null) {
                    var bytes = stream.Original.Read() ?? throw new IOException("Original custom stream returned null.");
                    if (bytes.Length is < 1 or > 65535) throw new IOException("Original custom stream returned invalid report size.");
                    stream.Pending = bytes;
                }
                int length = stream.Pending.Length;
                if (output != null && length <= capacity) { stream.Pending.CopyTo(new Span<byte>(output, capacity)); stream.Pending = null; }
                return length;
            }
        } catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }
    static void WithHostedBuffer(ulong token, byte* data, uint length, uint operation)
    {
        if (data == null || length is < 1 or > 65535) throw new ArgumentException("Invalid custom I/O buffer.");
        var stream = HostedCustomDevices.Get(token);
        byte[] bytes = new ReadOnlySpan<byte>(data, (int)length).ToArray();
        lock (stream.Writing) {
            ObjectDisposedException.ThrowIf(Volatile.Read(ref stream.Closed) != 0, stream);
            if (operation == 6) stream.Original.Write(bytes);
            else if (operation == 7) { stream.Original.GetFeature(bytes); bytes.CopyTo(new Span<byte>(data, (int)length)); }
            else stream.Original.SetFeature(bytes);
        }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int WriteHostedDevice(ulong token, byte* data, uint length) { try { WithHostedBuffer(token, data, length, 6); return 0; } catch (Exception error) { lastError = error.GetBaseException().Message; return -1; } }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int GetHostedFeature(ulong token, byte* data, uint length) { try { WithHostedBuffer(token, data, length, 7); return 0; } catch (Exception error) { lastError = error.GetBaseException().Message; return -1; } }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int SetHostedFeature(ulong token, byte* data, uint length) { try { WithHostedBuffer(token, data, length, 8); return 0; } catch (Exception error) { lastError = error.GetBaseException().Message; return -1; } }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int CloseHostedDevice(ulong token) { try { HostedCustomDevices.Close(token); return 0; } catch (Exception error) { lastError = error.GetBaseException().Message; return -1; } }
}

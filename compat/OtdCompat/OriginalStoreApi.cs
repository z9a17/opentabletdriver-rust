using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;
using OpenTabletDriver.Desktop.Reflection;

namespace OtdCompat;

public static unsafe partial class EntryPoints
{
    // Capacity retries return retained bytes, never repeat plugin construction.
    [ThreadStatic] static byte[]? originalStoreJson;
    [ThreadStatic] static byte[]? originalTypesJson;
    static int CopyOriginal(byte[] bytes, byte* output, int capacity)
    {
        if (capacity < 0 || capacity > 4194304) throw new ArgumentException("Invalid original-store output capacity.");
        if (bytes.Length > 4194304) throw new InvalidOperationException("Original-store result exceeds 4 MiB.");
        if (output != null && bytes.Length <= capacity) bytes.CopyTo(new Span<byte>(output, capacity));
        return bytes.Length;
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int GetPluginTypes(byte* output, int capacity, int refresh)
    {
        try {
            if (refresh != 0 || originalTypesJson == null) {
                var entries = InstalledRegistry.TypeSnapshot().Select(type => {
                    JObject description = JObject.FromObject(DescribeType(type));
                    return new JObject { ["path"] = type.FullName,
                        ["name"] = description.Value<string>("display_name") ?? type.FullName, ["category"] = description["kind"],
                        ["supported"] = description["supported"],
                        ["absolute_output"] = description["absolute_output"],
                        ["relative_output"] = description["relative_output"] };
                });
                originalTypesJson = Encoding.UTF8.GetBytes(new JArray(entries).ToString(Formatting.None));
            }
            return CopyOriginal(originalTypesJson, output, capacity);
        } catch (Exception error) { originalTypesJson = null; lastError = error.GetBaseException().Message; return -1; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int ConstructPluginStore(byte* json, int length, byte* output, int capacity)
    {
        try {
            if (json != null) {
                originalStoreJson = null;
                if (length < 1 || length > 32768) throw new ArgumentException("Invalid original-store request size.");
                JObject request = JObject.Parse(Encoding.UTF8.GetString(new ReadOnlySpan<byte>(json, length)));
                string name = request.Value<string>("path") ?? throw new ArgumentException("Plugin path is required.");
                string category = request.Value<string>("category") ?? throw new ArgumentException("Plugin category is required.");
                var selected = InstalledRegistry.AcquireType(name) ?? throw new KeyNotFoundException($"'{name}' is not in the actual loaded registry.");
                object? instance = null;
                using var services = new HostServices();
                try {
                    JObject metadata = JObject.FromObject(DescribeType(selected.Type));
                    if (metadata.Value<string>("kind") != category || metadata.Value<bool>("supported") != true)
                        throw new ArgumentException($"'{name}' is not a supported '{category}' plugin.");
                    instance = HostServices.Construct(selected.Type) ?? throw new InvalidOperationException("Plugin construction returned null.");
                    services.Inject(selected.Type, instance);
                    // This is the actual upstream object constructor, preserving
                    // constructor values even without DefaultPropertyValue.
                    var store = new PluginSettingStore(instance, enable: true);
                    originalStoreJson = Encoding.UTF8.GetBytes(JsonConvert.SerializeObject(store, Formatting.None));
                } finally {
                    try { HostServices.DisposePlugin(instance); }
                    finally { InstalledRegistry.Release(selected.Generation); }
                }
            }
            if (originalStoreJson == null) throw new InvalidOperationException("No original store staged on this thread.");
            return CopyOriginal(originalStoreJson, output, capacity);
        } catch (Exception error) { originalStoreJson = null; lastError = error.GetBaseException().Message; return -1; }
    }
}

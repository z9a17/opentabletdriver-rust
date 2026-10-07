using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using Newtonsoft.Json.Linq;
using OpenTabletDriver.Plugin.Tablet;
using OtdCompat;

// Offline fixture only: all callbacks capture value records, never OS input.
// Added with P06; intentionally unexecuted under the repository suite policy.
unsafe static class EndpointProbe
{
    static int bindings, pointers;
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static int Callback(nint scope, uint operation, uint index, GraphReport* frame)
    {
        if (operation == 5) { if (frame->X != 120 || frame->Y != 130) return -1; frame->Pressure = 321; bindings++; }
        else if (operation == 6) { if (frame->Kind != 0 || frame->X != 120 || frame->Y != 130) return -1; pointers++; }
        else if (operation is 2 or 3 or 4) return -1; // Native mapping/output must not run.
        return 0;
    }
    static object Value(EndpointInstance endpoint) => typeof(EndpointInstance).GetProperty("Value", BindingFlags.NonPublic | BindingFlags.Instance)!.GetValue(endpoint)!;
    static int Count(object value, string property) => (int)value.GetType().GetProperty(property)!.GetValue(value)!;
    // Exercise both matched alternatives and an auxiliary-open failure without
    // constructing any plugin or host input service.
    static void MatchedIdentifiers()
    {
        var first = new DeviceIdentifier { VendorID = 1, ProductID = 10, ReportParser = "Fixture.First" };
        var matched = new DeviceIdentifier { VendorID = 1, ProductID = 20, ReportParser = "Fixture.Matched" };
        var auxiliary = new DeviceIdentifier { VendorID = 1, ProductID = 30, ReportParser = "Fixture.Auxiliary" };
        var properties = new TabletConfiguration { Name = "Matched alternatives", DigitizerIdentifiers = [first, matched], AuxiliaryDeviceIdentifiers = [auxiliary] };
        var config = new JObject { ["tablet"] = JObject.FromObject(properties), ["identifiers"] = JArray.FromObject(new[] { matched, auxiliary }) };
        var reference = Instance.CreateTabletReference(config);
        if (!reference.Identifiers.Select(identifier => identifier.ProductID).SequenceEqual(new[] { 20, 30 })
            || reference.Properties.DigitizerIdentifiers.Count != 2 || reference.Properties.DigitizerIdentifiers[0].ProductID != 10)
            throw new Exception("Live identifiers must describe matched opened endpoints while Properties preserve every configuration alternative.");
        config["identifiers"] = JArray.FromObject(new[] { matched });
        reference = Instance.CreateTabletReference(config);
        if (reference.Identifiers.Single().ProductID != 20 || reference.Properties.AuxiliaryDeviceIdentifiers?.Count != 1)
            throw new Exception("An unopened auxiliary must disappear from Identifiers without erasing configured Properties.");
        config["identifiers"] = new JArray();
        try { Instance.CreateTabletReference(config); throw new Exception("An empty explicit endpoint list was accepted."); }
        catch (ArgumentException) { }
    }
    public static void Run(string[] args)
    {
        MatchedIdentifiers();
        var config = new JObject { ["assembly_path"] = args[0], ["tablet"] = JObject.Parse(File.ReadAllText(args[1])),
            ["owner"] = 7, ["keys"] = new JObject { ["A"] = 4 }, ["settings"] = new JObject(), ["pen"] = false, ["relative"] = false };
        config["type_name"] = "SettingsFixture.EndpointBinding";
        using var binding = new BindingInstance(config);
        object bindingObject = Value(binding);
        var original = new PenSnapshot { Raw = new byte[] { 1, 2, 3 }, Pressure = 123, PenButtons = new bool[] { true } };
        SynchronousGraph.CurrentReport = original;
        try { binding.Set(true, 7); }
        finally { SynchronousGraph.CurrentReport = null; }
        if (!ReferenceEquals(bindingObject.GetType().GetProperty("LastReport")!.GetValue(bindingObject), original)) throw new Exception("Binding lost concrete report identity.");
        if (!binding.Queue.Take(out var press) || press.Kind != 2 || press.Owner != 7 || press.Value != 4 || press.Flags != 1) throw new Exception("Keyboard press projection changed.");
        binding.Release(); binding.Release();
        if (Count(bindingObject, "Presses") != 1 || Count(bindingObject, "Releases") != 1 || !binding.Queue.Take(out var release) || release.Flags != 0) throw new Exception("Binding release lifecycle changed.");
        binding.Dispose(); binding.Dispose();
        if (Count(bindingObject, "Disposes") != 1) throw new Exception("Binding disposal must be idempotent.");

        config["type_name"] = "SettingsFixture.EndpointOutput";
        config["input"] = new JObject { ["Width"] = 100, ["Height"] = 100, ["X"] = 50, ["Y"] = 50 };
        config["output"] = new JObject { ["Width"] = 1920, ["Height"] = 1080, ["X"] = 960, ["Y"] = 540 };
        using var output = new OutputInstance(config);
        using var graph = new SynchronousGraph([]); graph.AttachOutput(output);
        byte[] bytes = new byte[] { 1, 2, 3 };
        fixed (byte* raw = bytes)
        {
            var frame = new GraphReport { Version = 2, Size = (uint)sizeof(GraphReport), Raw = raw, RawLength = 3,
                Flags = SynchronousGraph.Position | SynchronousGraph.Tablet, X = 100, Y = 100, Pressure = 123, PenCount = 1, PenBits = 1 };
            if (graph.Dispatch(&frame, &Callback, 0) != 0) throw new Exception(graph.Error);
        }
        if (bindings != 1 || pointers != 1 || Count(Value(output), "Reads") != 1 || Count(Value(output), "Transforms") != 1 || (uint)Value(output).GetType().GetProperty("LastPressure")!.GetValue(Value(output))! != 321) throw new Exception("The unchanged mode did not own Read/Transform/output.");
        using var queue = new CommandQueue();
        for (int index = 0; index < 256; index++) queue.Add(new ManagedCommand { Kind = 2, Value = 4 });
        try { queue.Add(default); throw new Exception("Unbounded service queue."); } catch (InvalidOperationException) { }
        try { queue.Take(out _); throw new Exception("Queue overflow was silently dropped."); } catch (InvalidOperationException) { }
        Console.WriteLine("Owned managed output/binding lifecycle fixtures passed without input injection.");
    }
}

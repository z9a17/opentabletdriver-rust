using System.Numerics;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using Newtonsoft.Json.Linq;
using OpenTabletDriver.Plugin.Tablet;
using OtdCompat;

// Pure graph regression mode: parser bytes and reports are synthetic; every
// native callback captures values and never opens a device or injects input.
unsafe static class AdmissionProbe
{
    sealed class Packet : ITabletReport
    {
        public byte[] Raw { get; set; } = [1];
        public Vector2 Position { get; set; }
        public uint Pressure { get; set; }
        public bool[] PenButtons { get; set; } = [];
    }
    sealed class ReusingParser : IReportParser<IDeviceReport>
    {
        readonly Packet packet = new();
        public IDeviceReport Parse(byte[] raw)
        {
            packet.Raw = raw; packet.Pressure = raw[0];
            return packet;
        }
    }
    static readonly List<bool> admissions = [];
    static readonly List<uint> pressures = [];
    static readonly List<IDeviceReport> reports = [];
    static readonly int ownerThread = Environment.CurrentManagedThreadId;
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static int Callback(nint scope, uint operation, uint index, GraphReport* frame)
    {
        if (Environment.CurrentManagedThreadId != ownerThread) return -1;
        if (operation == 7)
        {
            admissions.Add(index != 0);
            if (index != 0) frame->Pressure += 100;
        }
        if (operation is 3 or 4 or 5)
        {
            pressures.Add(frame->Pressure);
            reports.Add(SynchronousGraph.CurrentReport!);
        }
        return 0;
    }
    static object Filter(Instance instance) => typeof(Instance).GetField("filter", BindingFlags.Instance | BindingFlags.NonPublic)!.GetValue(instance)!;
    static JObject Config(string[] args, string type) => new() {
        ["assembly_path"] = typeof(AdmissionProbe).Assembly.Location, ["type_name"] = "Upstream068Fixtures." + type,
        ["tablet"] = JObject.Parse(File.ReadAllText(args[1])), ["settings"] = new JObject()
    };
    static void Reset() { admissions.Clear(); pressures.Clear(); reports.Clear(); }
    static void Expect(bool[] first, uint[] pressure)
    {
        if (!admissions.SequenceEqual(first) || !pressures.SequenceEqual(pressure))
            throw new Exception($"Admission/pressure mismatch: first=[{string.Join(',', admissions)}], pressure=[{string.Join(',', pressures)}].");
    }
    static void Dispatch(SynchronousGraph graph, IDeviceReport packet, bool fused = false)
    {
        var frame = new GraphReport { Version = 2, Size = (uint)sizeof(GraphReport) };
        if (graph.Dispatch(&frame, &Callback, 0, fused, packet) != 0) throw new Exception(graph.Error);
    }
    static void Ordinary(string[] args)
    {
        using var retaining = new Instance(Config(args, "RetainedPressureFilter"));
        var handle = GCHandle.Alloc(retaining);
        try
        {
            using var graph = new SynchronousGraph([new GraphNode { Context = GCHandle.ToIntPtr(handle), Index = 0, Stage = 1 }]);
            var parser = new ReusingParser();
            var first = parser.Parse([30]);
            Reset(); Dispatch(graph, first);
            Expect([true], [130]);
            if (!ReferenceEquals(reports[0], first)) throw new Exception("Original parser report identity was replaced.");
            Filter(retaining).GetType().GetMethod("EmitRetained")!.Invoke(Filter(retaining), []);
            if (graph.Tick(&Callback, 0) != 0) throw new Exception(graph.Error);
            Expect([true], [130, 130]);
            if (ReferenceEquals(reports[1], first) || reports[1].GetType() != first.GetType())
                throw new Exception("Ordinary queued continuation must own a concrete report snapshot.");
            var second = parser.Parse([40]);
            if (!ReferenceEquals(first, second)) throw new Exception("Parser fixture failed to reuse its object.");
            Dispatch(graph, second, fused: true);
            Expect([true, true], [130, 130, 140]);
            if (((ITabletReport)reports[1]).Pressure != 130) throw new Exception("Parser reuse changed an owned continuation.");
        }
        finally { handle.Free(); }
    }
    static void Internal(string[] args)
    {
        using var filter = new Instance(Config(args, "RetainedInternalFilter"));
        var handle = GCHandle.Alloc(filter);
        try
        {
            using var graph = new SynchronousGraph([new GraphNode { Context = GCHandle.ToIntPtr(handle), Index = 0, Stage = 4 }]);
            var parser = new ReusingParser();
            var first = parser.Parse([10]);
            Reset(); Dispatch(graph, first);
            Expect([true, false], [110, 110]);
            // A distinct source must leave the retained first object admitted.
            Dispatch(graph, new Packet { Pressure = 20 });
            Expect([true, false, false, true, false], [110, 110, 110, 120, 120]);
            // Reusing that first parser object is a fresh source admission.
            Dispatch(graph, parser.Parse([30]));
            Expect([true, false, false, true, false, false, true, false], [110, 110, 110, 120, 120, 120, 130, 130]);
        }
        finally { handle.Free(); }
    }
    static void ManagedOutput(string[] args)
    {
        using var graph = new SynchronousGraph([]);
        var parser = new ReusingParser();
        Reset(); Dispatch(graph, parser.Parse([10]));
        Expect([true], [110]);
        using var output = new OutputInstance(Config(args, "ReplayPressureOutput"));
        graph.AttachOutput(output); // Attachment must enable tracking dynamically.
        Reset(); var original = parser.Parse([20]); Dispatch(graph, original);
        Expect([true, false], [120, 120]);
        output.Mode.GetType().GetMethod("ReplayBackground")!.Invoke(output.Mode, []);
        Expect([true, false], [120, 120]); // Foreign producer cannot call native output.
        if (graph.NextTickMicros() != 0 || graph.Tick(&Callback, 0) != 0) throw new Exception(graph.Error ?? "Output replay was stranded.");
        Expect([true, false, false], [120, 120, 120]);
        if (ReferenceEquals(reports[2], original) || reports[2].GetType() != original.GetType())
            throw new Exception("Managed output replay must retain admission across its owned snapshot.");
        Dispatch(graph, parser.Parse([30]));
        Expect([true, false, false, true, false], [120, 120, 120, 130, 130]);
        if ((uint)output.Mode.GetType().GetProperty("LastPressure")!.GetValue(output.Mode)! != 130)
            throw new Exception("Managed mode did not receive prepared pressure.");
    }
    internal static void Run(string[] args)
    {
        Ordinary(args); Internal(args); ManagedOutput(args);
        Console.WriteLine("PASS ordinary async continuation, reused parser identity, retained/duplicate Internal admission and attached output replay contracts");
    }
}

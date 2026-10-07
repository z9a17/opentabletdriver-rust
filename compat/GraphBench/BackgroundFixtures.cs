using System.Numerics;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using Newtonsoft.Json.Linq;
using OpenTabletDriver.Plugin.Tablet;
using OtdCompat;

// Offline only. This fixture has been authored but has not been executed.
unsafe static class BackgroundProbe
{
    sealed class Packet : ITabletReport, ITiltReport, IEraserReport
    {
        public byte[] Raw { get; set; } = [1, 2];
        public Vector2 Position { get; set; }
        public uint Pressure { get; set; }
        public bool[] PenButtons { get; set; } = [true, false];
        public Vector2 Tilt { get; set; } = new(3, 4);
        public bool Eraser { get; set; } = true;
        internal uint[][] Extra = [[7, 8]];
    }
    static readonly List<float> outputs = new();
    static readonly int owner = Environment.CurrentManagedThreadId;
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static int Output(nint scope, uint operation, uint index, GraphReport* frame)
    {
        if (Environment.CurrentManagedThreadId != owner) return -1;
        if (operation == 3)
        {
            if (SynchronousGraph.CurrentReport is not Packet packet
                || packet.Raw[0] != 1 || !packet.PenButtons[0] || packet.Extra[0][0] != 7
                || !packet.Eraser || packet.Tilt != new Vector2(3, 4)) return -1;
            outputs.Add(frame->X);
        }
        return 0;
    }
    internal static void Run(string[] args)
    {
        using var instance = new Instance(new JObject {
            ["assembly_path"] = args[0], ["type_name"] = "SettingsFixture.BackgroundFixtureFilter",
            ["tablet"] = JObject.Parse(File.ReadAllText(args[1])), ["settings"] = new JObject()
        });
        object filter = typeof(Instance).GetField("filter", BindingFlags.NonPublic | BindingFlags.Instance)!.GetValue(instance)!;
        MethodInfo emit = filter.GetType().GetMethod("EmitBackground")!;
        void Send(IDeviceReport[] reports) => emit.Invoke(filter, [reports]);
        var handle = GCHandle.Alloc(instance);
        try
        {
            var graph = new SynchronousGraph([new GraphNode { Context = GCHandle.ToIntPtr(handle), Index = 0, Stage = 1 }]);
            using (graph)
            {
                if (graph.NextTickMicros() is <= 0 or > 10_000) throw new Exception("Background-only graph was cached as timerless.");
                var first = new Packet { Position = new Vector2(11, 12), Pressure = 17 };
                var second = new Packet { Position = new Vector2(21, 22), Pressure = 27 };
                Send([first, second]);
                first.Raw[0] = 99; first.PenButtons[0] = false; first.Extra[0][0] = 99; first.Position = Vector2.Zero;
                if (outputs.Count != 0 || graph.NextTickMicros() != 0) throw new Exception("Foreign callback ran output or failed to schedule it.");
                if (graph.Tick(&Output, 0) != 0 || !outputs.SequenceEqual(new float[] {11, 21})) throw new Exception(graph.Error ?? "FIFO/owned report state lost.");
                Send(Enumerable.Range(0, 65).Select(_ => (IDeviceReport)new Packet()).ToArray());
                if (graph.Tick(&Output, 0) == 0 || graph.FailedIndex != 0 || graph.Error?.Contains("overflow") != true)
                    throw new Exception("Overflow was not an explicit emitting-node failure.");
                if (graph.NextTickMicros() == 0) throw new Exception("Consumed queue failure stranded an immediate tick.");
            }
            Send([new Packet()]); // Graph retirement detached any native continuation.
            if (outputs.Count != 2) throw new Exception("A retired graph delivered output.");
            var original = new Packet();
            var copy = (Packet)OwnedReportSnapshot.Capture(original, out _);
            original.Raw[0] = 8; original.Extra[0][0] = 9;
            if (copy.Raw[0] != 1 || copy.Extra[0][0] != 7) throw new Exception("Nested report ownership lost.");
            try { OwnedReportSnapshot.Capture(new Packet { Raw = new byte[OwnedReportSnapshot.MaxBytes + 1] }, out _); throw new Exception("Snapshot size limit ignored."); }
            catch (NotSupportedException) { }
            Console.WriteLine("PASS background FIFO, concrete type, ownership, overflow and retirement contracts");
        }
        finally { handle.Free(); }
    }
}

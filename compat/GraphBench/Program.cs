using System.Diagnostics;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using Newtonsoft.Json.Linq;
using OtdCompat;

unsafe class GraphProbe
{
    static int outputs;
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static int Output(nint scope, uint op, uint index, GraphReport* report)
    {
        if (op == 3) { if (report->X != 101) return -1; outputs++; }
        return 0;
    }

    static void Main(string[] args)
    {
        System.Globalization.CultureInfo.CurrentCulture = System.Globalization.CultureInfo.InvariantCulture;
        using var instance = new Instance(new JObject {
            ["assembly_path"] = args[0], ["type_name"] = "SettingsFixture.DefaultsFilter",
            ["tablet"] = JObject.Parse(File.ReadAllText(args[1])),
            ["settings"] = new JObject { ["InheritedOffset"] = 1, ["ConstructorOffset"] = 0, ["AttributeOffset"] = 0 }
        });
        var handle = GCHandle.Alloc(instance);
        try {
            var graph = new SynchronousGraph(new GraphNode[] {new() {Context = GCHandle.ToIntPtr(handle), Index = 0, Stage = 1}});
            byte[] bytes = new byte[17]; bytes[0] = 0x10;
            fixed (byte* raw = bytes) {
                GraphReport input = new() {Version = 2, Size = (uint)sizeof(GraphReport), Raw = raw, RawLength = 17,
                    Flags = SynchronousGraph.Position | SynchronousGraph.Tablet | SynchronousGraph.Eraser | SynchronousGraph.Tilt, X = 100, Y = 100, PenCount = 2};
                for (int i = 0; i < 10000; i++) { if(graph.Dispatch(&input, &Output, 0) != 0) throw new Exception(graph.Error); graph.NextTickMicros(); }
                for (int trial = 0; trial < 3; trial++) {
                    outputs = 0;
                    long allocated = GC.GetAllocatedBytesForCurrentThread(), started = Stopwatch.GetTimestamp();
                    for (int i = 0; i < 100000; i++) if(graph.Dispatch(&input, &Output, 0) != 0) throw new Exception(graph.Error);
                    long elapsed = Stopwatch.GetTimestamp() - started, used = GC.GetAllocatedBytesForCurrentThread() - allocated;
                    if (outputs != 100000) throw new Exception("Output was lost");
                    Console.WriteLine($"dispatch trial={trial} reports={outputs} bytes/report={used / 100000.0:F1} ns/report={elapsed * 1e9 / Stopwatch.Frequency / 100000:F1}");
                    allocated = GC.GetAllocatedBytesForCurrentThread(); started = Stopwatch.GetTimestamp();
                    for (int i = 0; i < 100000; i++) graph.NextTickMicros();
                    elapsed = Stopwatch.GetTimestamp() - started; used = GC.GetAllocatedBytesForCurrentThread() - allocated;
                    Console.WriteLine($"deadline trial={trial} bytes/query={used / 100000.0:F1} ns/query={elapsed * 1e9 / Stopwatch.Frequency / 100000:F1}");
                }
            }
        } finally { handle.Free(); }
        using var timedInstance = new Instance(new JObject {
            ["assembly_path"] = args[0], ["type_name"] = "SettingsFixture.LateTimerFilter",
            ["tablet"] = JObject.Parse(File.ReadAllText(args[1])), ["settings"] = new JObject()
        });
        var timerHandle = GCHandle.Alloc(timedInstance);
        try {
            var graph = new SynchronousGraph(new GraphNode[] {new() {Context = GCHandle.ToIntPtr(timerHandle), Index = 0, Stage = 1}});
            for (int i = 0; i < 10000; i++) graph.NextTickMicros();
            for (int trial = 0; trial < 3; trial++) {
                long allocated = GC.GetAllocatedBytesForCurrentThread(), started = Stopwatch.GetTimestamp();
                for (int i = 0; i < 100000; i++) if (graph.NextTickMicros() != -1) throw new Exception("Late timer should remain stopped");
                long elapsed = Stopwatch.GetTimestamp() - started, used = GC.GetAllocatedBytesForCurrentThread() - allocated;
                Console.WriteLine($"deadline_with_timer trial={trial} bytes/query={used / 100000.0:F1} ns/query={elapsed * 1e9 / Stopwatch.Frequency / 100000:F1}");
            }
        } finally { timerHandle.Free(); }
    }
}

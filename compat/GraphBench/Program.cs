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

    // Fused transform and output (DispatchGraph2), as the Rust host calls it.
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static int FusedOutput(nint scope, uint op, uint index, GraphReport* report)
    {
        if (op != 4 || report->X != 101) return -1;
        outputs++;
        return 0;
    }

    // The PTH-660 pen report the Rust host sends: a ProximityReport with its
    // native tip state and the complete 192-byte packet.
    static GraphReport PenReport(byte* raw) => new() {Version = 2, Size = (uint)sizeof(GraphReport), Raw = raw, RawLength = 192,
        Flags = SynchronousGraph.Position | SynchronousGraph.Tablet | SynchronousGraph.Eraser | SynchronousGraph.Tilt
            | SynchronousGraph.Proximity | SynchronousGraph.NativeTip,
        X = 100, Y = 100, PenCount = 2, Pressure = 500, TipSwitch = 1, Near = 1, Distance = 3};

    static Instance Defaults(string[] args) => new(new JObject {
        ["assembly_path"] = args[0], ["type_name"] = "SettingsFixture.DefaultsFilter",
        ["tablet"] = JObject.Parse(File.ReadAllText(args[1])),
        ["settings"] = new JObject { ["InheritedOffset"] = 1, ["ConstructorOffset"] = 0, ["AttributeOffset"] = 0 }
    });

    static double Percentile(List<double> sorted, double fraction) => sorted[Math.Min(sorted.Count - 1, (int)(sorted.Count * fraction))];

    /// `GraphBench <fixture> <tablet> cold`, in a fresh process: the first
    /// reports after graph creation, paced at 1000 Hz, before and after tier-up.
    static void Cold(string[] args)
    {
        using var instance = Defaults(args);
        var handle = GCHandle.Alloc(instance);
        try {
            long created = Stopwatch.GetTimestamp();
            var graph = new SynchronousGraph(new GraphNode[] {new() {Context = GCHandle.ToIntPtr(handle), Index = 0, Stage = 1}});
            double createMs = (Stopwatch.GetTimestamp() - created) * 1e3 / Stopwatch.Frequency;
            byte[] bytes = new byte[192]; bytes[0] = 0x10;
            var micros = new List<double>();
            fixed (byte* raw = bytes) {
                GraphReport input = PenReport(raw);
                long period = Stopwatch.Frequency / 1000, next = Stopwatch.GetTimestamp();
                for (int i = 0; i < 4000; i++) {
                    next += period; while (Stopwatch.GetTimestamp() < next) { }
                    long started = Stopwatch.GetTimestamp();
                    if (graph.Dispatch(&input, &FusedOutput, 0, fusedContinuations: true) != 0) throw new Exception(graph.Error);
                    micros.Add((Stopwatch.GetTimestamp() - started) * 1e6 / Stopwatch.Frequency);
                }
            }
            if (outputs != micros.Count) throw new Exception("Output was lost");
            var early = micros.GetRange(1, 299); early.Sort();
            var later = micros.GetRange(300, micros.Count - 300); later.Sort();
            Console.WriteLine($"cold graph_create_ms={createMs:F1} first_us={micros[0]:F1} "
                + $"next299_us p50={Percentile(early, 0.5):F2} p99={Percentile(early, 0.99):F2} max={early[^1]:F1} "
                + $"later_us p50={Percentile(later, 0.5):F2} p99={Percentile(later, 0.99):F2} p99.9={Percentile(later, 0.999):F2} max={later[^1]:F1}");
        } finally { handle.Free(); }
    }

    static void Main(string[] args)
    {
        System.Globalization.CultureInfo.CurrentCulture = System.Globalization.CultureInfo.InvariantCulture;
        if (args.Length > 2 && args[2] == "contracts") { Contracts(args); return; }
        if (args.Length > 2 && args[2] == "endpoints") { EndpointProbe.Run(args); return; }
        if (args.Length > 2 && args[2] == "cold") { Cold(args); return; }
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
            byte[] pen = new byte[192]; pen[0] = 0x10;
            fixed (byte* raw = pen) {
                GraphReport input = PenReport(raw);
                const int Reports = 200000, Trials = 15;
                for (int i = 0; i < 100000; i++) if (graph.Dispatch(&input, &FusedOutput, 0, fusedContinuations: true) != 0) throw new Exception(graph.Error);
                // Let tiered compilation finish before timing.
                Thread.Sleep(300);
                var results = new List<double>();
                double bytesPerReport = 0;
                for (int trial = 0; trial < Trials; trial++) {
                    outputs = 0;
                    long allocated = GC.GetAllocatedBytesForCurrentThread(), started = Stopwatch.GetTimestamp();
                    for (int i = 0; i < Reports; i++) if (graph.Dispatch(&input, &FusedOutput, 0, fusedContinuations: true) != 0) throw new Exception(graph.Error);
                    long elapsed = Stopwatch.GetTimestamp() - started, used = GC.GetAllocatedBytesForCurrentThread() - allocated;
                    if (outputs != Reports) throw new Exception("Output was lost");
                    results.Add(elapsed * 1e9 / Stopwatch.Frequency / Reports);
                    bytesPerReport = used / (double)Reports;
                }
                results.Sort();
                Console.WriteLine($"fused_pen trials={Trials} reports={Reports} bytes/report={bytesPerReport:F1} ns/report min={results[0]:F1} median={Percentile(results, 0.5):F1}");
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

    // Offline lifecycle contract mode; no display query, hardware or input.
    static void Contracts(string[] args)
    {
        var scope = new HostServices();
        if (scope.GetService(typeof(IDisposable)) != null)
            throw new Exception("Service lookup must use exact registered types.");
        scope.Dispose();
        try { scope.GetService(typeof(IServiceProvider)); throw new Exception("Retired services remain usable."); }
        catch (ObjectDisposedException) { }
        using var instance = new Instance(new JObject {
            ["assembly_path"] = args[0], ["type_name"] = "SettingsFixture.ServicesFilter",
            ["tablet"] = JObject.Parse(File.ReadAllText(args[1])),
            ["settings"] = new JObject { ["Frequency"] = 17 }
        });
        if (!instance.HasTimers || instance.NextTickMicros(Stopwatch.GetTimestamp()) < 0)
            throw new Exception("Injected filter timer was not scheduled.");
        string marker = Path.Combine(args.Length > 3 ? args[3] : Path.GetDirectoryName(args[0])!, "service-tool-marker.txt");
        using (var tool = new ToolInstance(new JObject {
            ["assembly_path"] = args[0], ["type_name"] = "SettingsFixture.ServicesTool",
            ["settings"] = new JObject { ["MarkerPath"] = marker }
        }))
            if (File.ReadAllText(marker) != "started") throw new Exception("Tool did not initialize.");
        if (File.ReadAllText(marker) != "started stopped") throw new Exception("Tool was not disposed.");
        File.Delete(marker);
        Console.WriteLine("PASS service/settings/tablet/callback/tool lifecycle contracts");
    }
}

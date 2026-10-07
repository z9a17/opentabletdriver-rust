using System.Diagnostics;
using System.Globalization;
using System.Numerics;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using Newtonsoft.Json.Linq;
using OtdCompat;

unsafe class OrderDiff
{
    static List<(double t, float x, float y)> outputs = new();
    static long origin;
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static int Output(nint scope, uint op, uint index, GraphReport* r)
    {
        outputs.Add(((Stopwatch.GetTimestamp() - origin) * 1e3 / Stopwatch.Frequency, r->X, r->Y));
        return 0;
    }

    // Pen path in tablet units (0.005 mm/unit) at time t ms: osu!-like aim:
    // eased jumps between targets, then a slider-like arc, then a held point.
    static Vector2 Pen(double t)
    {
        double cycle = t % 3000;
        if (cycle < 1600)
        {
            int k = (int)(cycle / 200); double u = (cycle % 200) / 200;
            Vector2[] targets = { new(20000, 4000), new(24500, 6800), new(19000, 7200), new(25500, 3600), new(22000, 5200), new(18500, 3900), new(24000, 7400), new(21000, 6000) };
            Vector2 a = targets[k % 8], b = targets[(k + 1) % 8];
            double e = u < 0.5 ? 4 * u * u * u : 1 - Math.Pow(-2 * u + 2, 3) / 2; // ease in-out
            return a + (b - a) * (float)e;
        }
        if (cycle < 2600)
        {
            double a = (cycle - 1600) / 1000 * Math.PI * 2;
            return new Vector2(22000 + 1600 * (float)Math.Cos(a), 5500 + 1000 * (float)Math.Sin(a));
        }
        return new Vector2(22000 + 1600, 5500);
    }

    static Instance Load(string dll, string type, JObject settings, string tablet) => new(new JObject {
        ["assembly_path"] = dll, ["type_name"] = type, ["tablet"] = JObject.Parse(File.ReadAllText(tablet)), ["settings"] = settings });

    static void Main(string[] args)
    {
        CultureInfo.CurrentCulture = CultureInfo.InvariantCulture;
        string rfDll = args[0], trDll = args[1], tablet = args[2], order = args[3];
        double rateHz = args.Length > 4 ? double.Parse(args[4]) : 200, seconds = 12;
        var rf = Load(rfDll, "RadialFollow.RadialFollowSmoothingTabletSpace", new JObject {
            ["OuterRadius"] = 0.7039, ["InnerRadius"] = 0.302, ["SmoothingCoefficient"] = 0.302, ["SoftKneeScale"] = 0.603, ["SmoothingLeakCoefficient"] = 0.201 }, tablet);
        // Temporal Resampler 1.5.1 defaults (its [DefaultPropertyValue]s), as the panel adds it.
        var tr = Load(trDll, "TemporalResampler", new JObject {
            ["frameShift"] = 0.5, ["followRadius"] = 0, ["latency"] = 0, ["reverseSmoothing"] = 1, ["extraFrames"] = true, ["loggingEnabled"] = false, ["Frequency"] = 1000 }, tablet);
        var hRf = GCHandle.Alloc(rf); var hTr = GCHandle.Alloc(tr);
        GraphNode Node(GCHandle h, uint i) => new() { Context = GCHandle.ToIntPtr(h), Index = i, Stage = 1 };
        var nodes = order == "rf-first" ? new[] { Node(hRf, 0), Node(hTr, 1) } : new[] { Node(hTr, 0), Node(hRf, 1) };
        var graph = new SynchronousGraph(nodes);
        byte[] raw = new byte[17]; raw[0] = 0x10; raw[1] = 0x61;
        var rng = new Random(11);
        origin = Stopwatch.GetTimestamp();
        long period = (long)(Stopwatch.Frequency / rateHz), nextReport = origin;
        int reports = 0;
        fixed (byte* p = raw)
        while (true)
        {
            long now = Stopwatch.GetTimestamp();
            double tMs = (now - origin) * 1e3 / Stopwatch.Frequency;
            if (tMs > seconds * 1000) break;
            if (now >= nextReport)
            {
                Vector2 pen = Pen(tMs);
                float nx = (float)(rng.NextDouble() - 0.5) * 3, ny = (float)(rng.NextDouble() - 0.5) * 3;
                GraphReport input = new() { Version = 2, Size = (uint)sizeof(GraphReport), Raw = p, RawLength = 17,
                    Flags = SynchronousGraph.Position | SynchronousGraph.Tablet | SynchronousGraph.Eraser | SynchronousGraph.Tilt
                        | SynchronousGraph.Proximity | SynchronousGraph.NativeTip,
                    X = MathF.Round(pen.X + nx), Y = MathF.Round(pen.Y + ny), PenCount = 2, Pressure = 600, TipSwitch = 1, Near = 1 };
                if (graph.Dispatch(&input, &Output, 0, true) != 0) throw new Exception(graph.Error);
                reports++;
                nextReport += period;
            }
            long due = graph.NextTickMicros();
            if (due == 0) { if (graph.Tick(&Output, 0, true) != 0) throw new Exception(graph.Error); }
        }
        // Score every output after warm-up against the true pen path at that moment.
        var lag = new List<double>();
        foreach (var (t, x, y) in outputs)
            if (t > 1000) lag.Add(Vector2.Distance(new Vector2(x, y), Pen(t)) * 0.005);
        lag.Sort();
        double rate = outputs.Count(o => o.t > 1000) / (seconds - 1);
        Console.WriteLine($"{order,-8} rate={rateHz}Hz reports={reports} outputs/s={rate:F0} distance from pen (mm): mean={lag.Average():F3} p50={lag[lag.Count / 2]:F3} p95={lag[(int)(lag.Count * .95)]:F3} max={lag[^1]:F3}");
        File.WriteAllLines(args.Length > 5 ? args[5] : $"{order}.csv", outputs.Select(o => $"{o.t:F3},{o.x},{o.y}"));
        hRf.Free(); hTr.Free();
    }
}

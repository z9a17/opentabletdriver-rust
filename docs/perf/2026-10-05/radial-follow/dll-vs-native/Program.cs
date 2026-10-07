using System.Globalization;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using Newtonsoft.Json.Linq;
using OtdCompat;

unsafe class RfDiff
{
    static float outX, outY; static int outputs;
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static int Output(nint scope, uint op, uint index, GraphReport* r) { outX = r->X; outY = r->Y; outputs++; return 0; }

    static void Main(string[] args)
    {
        CultureInfo.CurrentCulture = CultureInfo.InvariantCulture;
        using var instance = new Instance(new JObject {
            ["assembly_path"] = args[0], ["type_name"] = "RadialFollow.RadialFollowSmoothingTabletSpace",
            ["tablet"] = JObject.Parse(File.ReadAllText(args[1])),
            ["settings"] = new JObject { ["OuterRadius"] = 0.7039, ["InnerRadius"] = 0.302, ["SmoothingCoefficient"] = 0.302,
                ["SoftKneeScale"] = 0.603, ["SmoothingLeakCoefficient"] = 0.201 }
        });
        var handle = GCHandle.Alloc(instance);
        var graph = new SynchronousGraph(new GraphNode[] { new() { Context = GCHandle.ToIntPtr(handle), Index = 0, Stage = 1 } });
        byte[] raw = new byte[17]; raw[0] = 0x10; raw[1] = 0x60;
        var lines = File.ReadAllLines(args[2]);
        var sb = new System.Text.StringBuilder();
        long period = System.Diagnostics.Stopwatch.Frequency / 500, next = System.Diagnostics.Stopwatch.GetTimestamp();
        fixed (byte* p = raw)
        foreach (var line in lines)
        {
            var f = line.Split(',');
            int gap = int.Parse(f[2]);
            if (gap > 0) { Thread.Sleep(gap); next = System.Diagnostics.Stopwatch.GetTimestamp(); }
            next += period; while (System.Diagnostics.Stopwatch.GetTimestamp() < next) { }
            GraphReport input = new() { Version = 2, Size = (uint)sizeof(GraphReport), Raw = p, RawLength = 17,
                Flags = SynchronousGraph.Position | SynchronousGraph.Tablet | SynchronousGraph.Eraser | SynchronousGraph.Tilt
                    | SynchronousGraph.Proximity | SynchronousGraph.NativeTip,
                X = float.Parse(f[0]), Y = float.Parse(f[1]), PenCount = 2, Near = 1 };
            if (graph.Dispatch(&input, &Output, 0, true) != 0) throw new Exception(graph.Error);
            sb.Append(outX.ToString("R")).Append(',').Append(outY.ToString("R")).Append('\n');
        }
        File.WriteAllText(args[3], sb.ToString());
        Console.WriteLine($"outputs={outputs}");
        handle.Free();
    }
}

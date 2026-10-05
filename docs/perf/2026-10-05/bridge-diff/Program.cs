using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using Newtonsoft.Json.Linq;
using OtdCompat;
using G = OtdCompat.SynchronousGraph;

unsafe class BridgeDiff
{
    static StringBuilder log = new();
    static int calls;
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static int Callback(nint scope, uint op, uint index, GraphReport* f)
    {
        calls++;
        byte* p = (byte*)f;
        var sb = new StringBuilder();
        for (int i = 0; i < sizeof(GraphReport); i++) sb.Append(i is >= 8 and < 16 ? "--" : p[i].ToString("x2"));
        sb.Append(" raw=");
        for (int i = 0; i < f->RawLength; i++) sb.Append(f->Raw[i].ToString("x2"));
        log.Append($"  cb op={op} idx={index} {sb}\n");
        // Move the position in some continuations to exercise write-back.
        if (op == 1 && index == 0) { f->X += 1; f->Y -= 2; }
        if (op == 2) { f->X *= 0.5f; }
        if (op == 4 && calls % 4 == 0) return 1; // suppress some outputs
        return 0;
    }

    static GraphReport Make(uint flags, uint kind, int seed, byte* raw)
    {
        GraphReport f = default;
        f.Version = 2; f.Size = (uint)sizeof(GraphReport); f.Raw = raw; f.RawLength = 24; f.Kind = kind; f.Flags = flags;
        f.X = 1000.5f + seed; f.Y = 2000.25f - seed; f.TiltX = 12; f.TiltY = -7; f.ScrollX = 1.5f; f.ScrollY = -2.5f;
        f.Pressure = (uint)(100 + seed); f.Eraser = (uint)(seed & 1); f.Near = (uint)((seed >> 1) & 1); f.Distance = 9;
        f.ToolID = 0x802; f.ToolType = (uint)(seed & 1); f.Serial = 0x1234_5678_9abcUL;
        f.PenBits = 0b10; f.PenCount = 2; f.AuxBits = 0b1010_0101; f.AuxCount = 8; f.MouseBits = 0b101; f.MouseCount = 5;
        f.TipSwitch = (uint)((seed >> 2) & 1);
        f.AbsoluteCount = 2; f.AbsolutePresent = 0b01; f.AbsoluteValues[0] = 42; f.AbsoluteValues[1] = 77;
        f.RelativeCount = 3; f.RelativeValues[0] = -1; f.RelativeValues[1] = 2; f.RelativeValues[2] = 0;
        f.WheelCount = 2; f.WheelBits[0] = 1; f.WheelCounts[0] = 1; f.WheelBits[1] = 0b11; f.WheelCounts[1] = 2;
        f.TouchCount = 4; f.TouchPresent = 0b1010; f.TouchIDs[1] = 3; f.TouchXY[2] = 10; f.TouchXY[3] = 20; f.TouchIDs[3] = 5; f.TouchXY[6] = 30; f.TouchXY[7] = 40;
        return f;
    }

    static void Main(string[] args)
    {
        System.Globalization.CultureInfo.CurrentCulture = System.Globalization.CultureInfo.InvariantCulture;
        JObject tablet = JObject.Parse(File.ReadAllText(args[1]));
        using var mix = new Instance(new JObject { ["assembly_path"] = args[2], ["type_name"] = "DiffPlugin.MixFilter", ["tablet"] = tablet, ["settings"] = new JObject() });
        using var defaults = new Instance(new JObject { ["assembly_path"] = args[0], ["type_name"] = "SettingsFixture.DefaultsFilter", ["tablet"] = tablet, ["settings"] = new JObject() });
        var hMix = GCHandle.Alloc(mix); var hDefaults = GCHandle.Alloc(defaults);
        uint[] shapes = {
            0, G.Position | G.Tablet, G.Position | G.Tablet | G.Eraser, G.Position | G.Tablet | G.Tilt, G.Position | G.Tablet | G.Proximity,
            G.Position | G.Tablet | G.Eraser | G.Tilt, G.Position | G.Tablet | G.Eraser | G.Proximity, G.Position | G.Tablet | G.Tilt | G.Proximity,
            G.Position | G.Tablet | G.Eraser | G.Tilt | G.Proximity, G.Position | G.Tablet | G.Eraser | G.Proximity | G.Aux,
            G.Tool | G.Eraser | G.Proximity, G.Position | G.Mouse, G.Position | G.Mouse | G.Proximity, G.Position | G.Mouse | G.Aux, G.Aux,
            G.Aux | G.Absolute | G.AbsoluteWheel | G.WheelButtons, G.Aux | G.WheelButtons, G.Absolute, G.Absolute | G.AbsoluteWheel,
            G.Aux | G.Absolute, G.Relative, G.Relative | G.RelativeWheel, G.Aux | G.Relative | G.RelativeWheel, G.Touch, G.Aux | G.Touch,
            G.Position | G.Tool, G.NativeTip, // unsupported
        };
        var inputs = new List<(uint flags, uint kind)>();
        foreach (uint s in shapes)
        {
            inputs.Add((s, 0));
            if ((s & G.Tablet) != 0) inputs.Add((s | G.NativeTip, 0));
        }
        inputs.Add((0, 1)); inputs.Add((G.Position | G.Tablet, 1)); inputs.Add((0, 2));
        GraphNode[][] graphs = {
            new GraphNode[] { new() { Index = 0, Stage = 1 }, new() { Context = GCHandle.ToIntPtr(hMix), Index = 1, Stage = 1 },
                new() { Context = GCHandle.ToIntPtr(hDefaults), Index = 2, Stage = 2 }, new() { Index = 3, Stage = 2 } },
            new GraphNode[] { new() { Context = GCHandle.ToIntPtr(hMix), Index = 0, Stage = 1 } },
        };
        byte[] rawBytes = new byte[24];
        for (int i = 0; i < rawBytes.Length; i++) rawBytes[i] = (byte)(i * 7 + 1);
        fixed (byte* raw = rawBytes)
        for (int g = 0; g < graphs.Length; g++)
        foreach (bool fused in new[] { false, true })
        {
            var graph = new SynchronousGraph(graphs[g]);
            for (int round = 0; round < 3; round++)
            for (int n = 0; n < inputs.Count; n++)
            {
                GraphReport input = Make(inputs[n].flags, inputs[n].kind, n + round, raw);
                log.Append($"graph={g} fused={fused} round={round} flags=0x{inputs[n].flags:x} kind={inputs[n].kind}\n");
                int result;
                try { result = graph.Dispatch(&input, &Callback, 0, fused); }
                catch (Exception e) { log.Append($"  throw {e.GetType().Name}: {e.Message}\n"); continue; }
                log.Append($"  result={result} error={graph.Error} failed={graph.FailedIndex}\n");
            }
        }
        Console.Write(log.ToString());
        hMix.Free(); hDefaults.Free();
    }
}

using System.Numerics;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Tablet;
namespace DiffPlugin;

public sealed class CustomReport : ITabletReport, IEraserReport, ITiltReport, IToolReport
{
    public byte[] Raw { get; set; }
    public Vector2 Position { get; set; }
    public uint Pressure { get; set; }
    public bool[] PenButtons { get; set; }
    public bool Eraser { get; set; }
    public Vector2 Tilt { get; set; }
    public ulong Serial { get; set; }
    public uint RawToolID { get; set; }
    public ToolType Tool { get; set; }
}
public struct StructReport : ITabletReport, IProximityReport
{
    public byte[] Raw { get; set; }
    public Vector2 Position { get; set; }
    public uint Pressure { get; set; }
    public bool[] PenButtons { get; set; }
    public bool NearProximity { get; set; }
    public uint HoverDistance { get; set; }
}
public sealed class PlainReport : IDeviceReport { public byte[] Raw { get; set; } }

// Emits the input, then foreign class, boxed struct and plain report types,
// so the bridge's per-type capability cache sees alternating types.
public sealed class MixFilter : IPositionedPipelineElement<IDeviceReport>
{
    public PipelinePosition Position => PipelinePosition.PreTransform;
    public event Action<IDeviceReport> Emit;
    int n;
    public void Consume(IDeviceReport report)
    {
        n++;
        if (report is IAbsolutePositionReport p) p.Position += new Vector2(0.25f, -0.5f);
        Emit?.Invoke(report);
        if (report is ITabletReport t)
        {
            if (n % 3 == 0) Emit?.Invoke(new CustomReport { Raw = report.Raw, Position = t.Position + new Vector2(1, 2), Pressure = t.Pressure + 1,
                PenButtons = new[] { true, false, true }, Eraser = n % 2 == 0, Tilt = new Vector2(3, -4), Serial = (ulong)n, RawToolID = 7, Tool = n % 2 == 0 ? ToolType.Eraser : ToolType.Pen });
            if (n % 5 == 0) Emit?.Invoke(new StructReport { Raw = report.Raw, Position = t.Position * 2, Pressure = 9, PenButtons = new[] { false, true },
                NearProximity = n % 2 == 0, HoverDistance = (uint)n });
            if (n % 7 == 0) Emit?.Invoke(new PlainReport { Raw = report.Raw });
            if (n % 11 == 0) Emit?.Invoke(report);
            if (n % 97 == 0) Emit?.Invoke(new CustomReport { Raw = report.Raw, PenButtons = new bool[0], Tool = (ToolType)5 });
            if (n % 89 == 0) throw new InvalidOperationException("fixture failure");
        }
    }
}

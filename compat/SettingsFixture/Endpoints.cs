using System.Numerics;
using OpenTabletDriver.Plugin;
using OpenTabletDriver.Plugin.Attributes;
using OpenTabletDriver.Plugin.DependencyInjection;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Platform.Keyboard;
using OpenTabletDriver.Plugin.Platform.Pointer;
using OpenTabletDriver.Plugin.Tablet;

namespace SettingsFixture;

public sealed class EndpointBinding : IStateBinding, IDisposable
{
    [Resolved] public IVirtualKeyboard Keyboard { get; set; }
    public int Presses { get; private set; }
    public int Releases { get; private set; }
    public int Disposes { get; private set; }
    public IDeviceReport LastReport { get; private set; }
    public string TabletName { get; private set; }
    [OnDependencyLoad] public void Ready() { if (Keyboard == null || !Keyboard.SupportedKeys.Contains("A")) throw new InvalidOperationException("Real keyboard service missing."); }
    public void Press(TabletReference tablet, IDeviceReport report) { Presses++; LastReport = report; TabletName = tablet.Properties.Name; Keyboard.Press("A"); }
    public void Release(TabletReference tablet, IDeviceReport report) { Releases++; LastReport = report; Keyboard.Release("A"); }
    public void Dispose() => Disposes++;
}

public sealed class EndpointOutput : AbsoluteOutputMode
{
    public override IAbsolutePointer Pointer { get; set; }
    public int Reads { get; private set; }
    public int Transforms { get; private set; }
    public uint LastPressure { get; private set; }
    protected override void OnOutput(IDeviceReport report) { if (report is ITabletReport tablet) LastPressure = tablet.Pressure; base.OnOutput(report); }
    public override void Read(IDeviceReport report) { Reads++; base.Read(report); }
    protected override IAbsolutePositionReport Transform(IAbsolutePositionReport report) { Transforms++; report.Position += new Vector2(20, 30); return report; }
}

public sealed class MarkerBinding : IBinding { }

// Original custom parser/report identities used by the background registry fixture.
public sealed class StatefulReport : IDeviceReport
{
    public byte[] Raw { get; set; } = Array.Empty<byte>();
    public int Tick { get; set; }
}
public sealed class StatefulParser : IReportParser<IDeviceReport>
{
    int count;
    public IDeviceReport Parse(byte[] raw) => raw[0] == 255 ? null : new StatefulReport { Raw = raw, Tick = ++count };
}

public sealed class RewriteRawParser : IReportParser<IDeviceReport>
{
    public IDeviceReport Parse(byte[] raw) => new StatefulReport { Raw = new byte[] { 42 }, Tick = 9 };
}
public sealed class ConcreteReportFilter : IPositionedPipelineElement<IDeviceReport>
{
    public PipelinePosition Position => PipelinePosition.PreTransform;
    public IDeviceReport LastReport { get; private set; }
    public event Action<IDeviceReport> Emit;
    public void Consume(IDeviceReport report) { if (report is not StatefulReport) throw new InvalidOperationException("Original concrete report type was replaced."); LastReport = report; Emit?.Invoke(report); }
}

public sealed class BackgroundEndpointOutput : AbsoluteOutputMode
{
    public override IAbsolutePointer Pointer { get; set; }
    protected override IAbsolutePositionReport Transform(IAbsolutePositionReport report) { report.Position += new Vector2(20, 30); return report; }
    public void EmitBackground(IDeviceReport[] reports)
    {
        Exception failure = null;
        var worker = new Thread(() => {
            try { foreach (var report in reports) Read(report); }
            catch (Exception error) { failure = error; }
        });
        worker.Start(); worker.Join();
        if (failure != null) throw failure;
    }
}

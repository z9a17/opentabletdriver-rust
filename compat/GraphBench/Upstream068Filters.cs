using OpenTabletDriver.Plugin.Attributes;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Tablet;
using System.Numerics;

namespace Upstream068Fixtures;

[PluginName("Internal duplicate fixture")]
public sealed class InternalDuplicateFilter : IPositionedPipelineElement<IDeviceReport>
{
    public PipelinePosition Position => PipelinePosition.Internal;
    public event Action<IDeviceReport?>? Emit;
    public uint SeenPressure { get; private set; }
    public void Consume(IDeviceReport? report)
    {
        SeenPressure = (report as ITabletReport)?.Pressure ?? 0;
        Emit?.Invoke(report); Emit?.Invoke(report);
    }
    public void EmitLate(IDeviceReport report) => Emit?.Invoke(report);
}

[PluginName("Retained pressure fixture")]
public sealed class RetainedPressureFilter : IPositionedPipelineElement<IDeviceReport>
{
    IDeviceReport? retained;
    public PipelinePosition Position => PipelinePosition.PreTransform;
    public event Action<IDeviceReport?>? Emit;
    public void Consume(IDeviceReport? report) { retained = report; Emit?.Invoke(report); }
    public void EmitRetained() { if (retained != null) Emit?.Invoke(retained); }
}

[PluginName("Unsupported async Internal fixture")]
public sealed class AsyncInternalFilter : AsyncPositionedPipelineElement<IDeviceReport>
{
    public override PipelinePosition Position => PipelinePosition.Internal;
    protected override void ConsumeState() { }
    protected override void UpdateState() => OnEmit();
}

[PluginName("Ordinary async bootstrap fixture")]
public sealed class OrdinaryAsyncFilter : AsyncPositionedPipelineElement<IDeviceReport>
{
    public override PipelinePosition Position => PipelinePosition.PreTransform;
    protected override void ConsumeState() { }
    protected override void UpdateState() => OnEmit();
}

[PluginName("Managed pressure output fixture")]
public sealed class PressureOutput : OutputMode
{
    public uint LastPressure { get; private set; }
    protected override Matrix3x2 CreateTransformationMatrix() => Matrix3x2.Identity;
    protected override IAbsolutePositionReport Transform(IAbsolutePositionReport report) => report;
    protected override void OnOutput(IDeviceReport report) => LastPressure = (report as ITabletReport)?.Pressure ?? 0;
}

[PluginName("Retained Internal admission fixture")]
public sealed class RetainedInternalFilter : IPositionedPipelineElement<IDeviceReport>
{
    IDeviceReport? retained;
    public PipelinePosition Position => PipelinePosition.Internal;
    public event Action<IDeviceReport?>? Emit;
    public void Consume(IDeviceReport? report)
    {
        if (retained != null) Emit?.Invoke(retained);
        retained = report;
        Emit?.Invoke(report); Emit?.Invoke(report);
    }
}

// Deliberately repeats the entry before Prepare and later replays that same
// admitted object on a foreign thread. OnOutput only records pressure.
[PluginName("Replayed output admission fixture")]
public sealed class ReplayPressureOutput : OutputMode
{
    IDeviceReport? retained;
    public uint LastPressure { get; private set; }
    public override void Read(IDeviceReport report)
    {
        retained = report;
        base.Read(report); base.Read(report);
    }
    public void ReplayBackground()
    {
        Exception? failure = null;
        var worker = new Thread(() => {
            try { if (retained != null) base.Read(retained); }
            catch (Exception error) { failure = error; }
        });
        worker.Start(); worker.Join();
        if (failure != null) throw failure;
    }
    protected override Matrix3x2 CreateTransformationMatrix() => Matrix3x2.Identity;
    protected override IAbsolutePositionReport Transform(IAbsolutePositionReport report) => report;
    protected override void OnOutput(IDeviceReport report) => LastPressure = (report as ITabletReport)?.Pressure ?? 0;
}

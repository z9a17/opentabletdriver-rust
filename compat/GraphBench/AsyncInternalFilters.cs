using OpenTabletDriver.Plugin.Attributes;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Tablet;

namespace AsyncInternalFixtures;

[PluginName("Background Internal admission fixture")]
public sealed class InternalFilter : IPositionedPipelineElement<IDeviceReport>
{
    public PipelinePosition Position => PipelinePosition.Internal;
    public event Action<IDeviceReport?>? Emit;
    [Property("Copies")] public int Copies { get; set; } = 1;
    public void Consume(IDeviceReport? report)
    {
        if (report is OutOfRangeReport) return; // Deliberately retain stale state in the probe.
        for (int i = 0; i < Copies; i++) Emit?.Invoke(report);
    }
    public void Send(IDeviceReport[] reports)
    {
        foreach (var report in reports) Emit?.Invoke(report);
    }
}

[PluginName("SDK Internal repeated timer fixture")]
public sealed class TimedInternalFilter : AsyncPositionedPipelineElement<IDeviceReport>
{
    public override PipelinePosition Position => PipelinePosition.Internal;
    protected override void ConsumeState() { }
    protected override void UpdateState()
    {
        // Stop after one deterministic tick. The probe rearms the real injected
        // SessionTimer to exercise scheduling without sleeping or using a device.
        Scheduler!.Stop();
        if (State is ITabletReport) { OnEmit(); OnEmit(); }
    }
}

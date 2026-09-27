using System.Numerics;
using OpenTabletDriver.Plugin.Attributes;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Tablet;

namespace SettingsFixture;

/// A timer-driven filter in upstream's AsyncPositionedPipelineElement shape:
/// it suppresses each report and re-emits the latest one, moved by Offset, on
/// every Scheduler tick while the pen is in range.
[PluginName("Async fixture")]
public sealed class AsyncFixtureFilter : AsyncPositionedPipelineElement<IDeviceReport>
{
    public static int Ticks;
    private Vector2 last;

    [Property("Offset")]
    public float Offset { get; set; } = 1000;

    public override PipelinePosition Position => PipelinePosition.PreTransform;

    protected override void ConsumeState()
    {
        if (State is IAbsolutePositionReport report)
            last = report.Position;
    }

    protected override void UpdateState()
    {
        Ticks++;
        if (State is IAbsolutePositionReport report && PenIsInRange())
        {
            report.Position = last + new Vector2(Offset, 0);
            OnEmit();
        }
    }
}

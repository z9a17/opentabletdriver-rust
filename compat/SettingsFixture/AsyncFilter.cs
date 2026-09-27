using System.Numerics;
using OpenTabletDriver.Plugin.Attributes;
using OpenTabletDriver.Plugin.DependencyInjection;
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

/// No enabled timer exists at graph construction. Consume starts it later.
[PluginName("Late timer fixture")]
public sealed class LateTimerFilter : BaseFilter, IDisposable
{
    [Resolved]
    public OpenTabletDriver.Plugin.Timers.ITimer Scheduler { get; set; }
    [Property("Offset")]
    public float Offset { get; set; } = 1000;
    private IDeviceReport retained;
    private Vector2 last;

    [OnDependencyLoad]
    public void Initialize()
    {
        Scheduler.Interval = 1;
        Scheduler.Elapsed += Tick;
    }

    public override void Consume(IDeviceReport report)
    {
        retained = report;
        if (report is IAbsolutePositionReport positioned) last = positioned.Position;
        Scheduler.Start();
    }

    private void Tick()
    {
        if (retained is IAbsolutePositionReport positioned)
        {
            positioned.Position = last + new Vector2(Offset, 0);
            Publish(retained);
        }
    }

    public void Dispose() { Scheduler.Stop(); Scheduler.Elapsed -= Tick; }
}

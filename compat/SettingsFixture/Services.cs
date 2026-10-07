using OpenTabletDriver.Plugin;
using OpenTabletDriver.Plugin.Attributes;
using OpenTabletDriver.Plugin.DependencyInjection;
using OpenTabletDriver.Plugin.Tablet;
using OpenTabletDriver.Plugin.Timers;
using ITimer = OpenTabletDriver.Plugin.Timers.ITimer;

namespace SettingsFixture;

public abstract class ServiceBaseFilter : BaseFilter
{
    [Resolved] protected IServiceProvider Services;
    [Resolved] protected IDisposable Optional = new MemoryStream();
    protected IDisposable OriginalOptional;
    protected ServiceBaseFilter() => OriginalOptional = Optional;
}

// Fails construction unless the real runtime initialization order is observed.
public sealed class ServicesFilter : ServiceBaseFilter, IDisposable
{
    [Resolved] public ITimer Timer { get; set; }
    [TabletReference] public TabletReference Tablet { get; set; }
    [Property("Frequency")] public float Frequency
    {
        get => Timer.Interval;
        set { Timer.Interval = value; Timer.Start(); }
    }
    [OnDependencyLoad] public void Ready()
    {
        if (Services == null || Services.GetService(typeof(IServiceProvider)) != Services
            || Services.GetService(typeof(IDisposable)) != null
            || Optional != OriginalOptional || Timer.Interval != 17
            || Tablet?.Properties?.Name == null)
            throw new InvalidOperationException("Service/settings/tablet/callback ordering changed.");
    }
    public override void Consume(IDeviceReport report) => Publish(report);
    public void Dispose() => Optional.Dispose();
}

public sealed class ServicesTool : ITool
{
    [Resolved] public IServiceProvider Services;
    [TabletReference] public TabletReference Tablet = new(null, Array.Empty<DeviceIdentifier>());
    [Property("Marker path")] public string MarkerPath { get; set; }
    bool ready;
    [OnDependencyLoad] public void Ready()
    {
        if (Services == null || Services.GetService(typeof(ITimer)) != null || Tablet != null)
            throw new InvalidOperationException("Tool service scope changed.");
        ready = true;
    }
    public bool Initialize()
    {
        if (!ready) throw new InvalidOperationException("Tool initialized before dependencies.");
        File.WriteAllText(MarkerPath, "started");
        return true;
    }
    public void Dispose() => File.AppendAllText(MarkerPath, " stopped");
}

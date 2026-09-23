using System.Numerics;
using OpenTabletDriver.Plugin;
using OpenTabletDriver.Plugin.Attributes;
using OpenTabletDriver.Plugin.DependencyInjection;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Tablet;

namespace SettingsFixture;

public abstract class BaseFilter : IPositionedPipelineElement<IDeviceReport>
{
    [Property("Inherited offset")]
    public float InheritedOffset { get; set; } = 3;

    public PipelinePosition Position => PipelinePosition.PreTransform;
    public event Action<IDeviceReport> Emit;

    protected void Publish(IDeviceReport report) => Emit?.Invoke(report);
    public abstract void Consume(IDeviceReport report);
}

public sealed class DefaultsFilter : BaseFilter
{
    [Property("Constructor offset")]
    public float ConstructorOffset { get; set; } = 4.5f;

    [Property("Attribute offset"), DefaultPropertyValue(7f)]
    public float AttributeOffset { get; set; } = 2;

    private float activeOffset;

    [OnDependencyLoad]
    public void Initialize() => activeOffset = InheritedOffset + ConstructorOffset + AttributeOffset;

    public override void Consume(IDeviceReport report)
    {
        if (report is IAbsolutePositionReport positioned)
            positioned.Position += new Vector2(activeOffset, 0);
        Publish(report);
    }
}

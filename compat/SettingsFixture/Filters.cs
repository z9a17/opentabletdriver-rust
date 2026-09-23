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

public abstract class ReferenceBaseFilter : BaseFilter
{
    [Resolved]
    protected TabletReference InheritedField;

    [TabletReference]
    public TabletReference InheritedProperty { get; set; }

    protected bool BaseDependenciesReady => InheritedField?.Properties?.Name == "Wacom PTH-660"
        && InheritedProperty?.Properties?.Name == "Wacom PTH-660";
}

public sealed class ReferenceFilter : ReferenceBaseFilter
{
    [Resolved]
    public TabletReference DirectField;

    [TabletReference]
    public TabletReference DirectProperty { get; set; }

    private float offset;

    [OnDependencyLoad]
    public void Initialize()
    {
        if (!BaseDependenciesReady || DirectField?.Properties?.Name != "Wacom PTH-660"
            || DirectProperty?.Properties?.Name != "Wacom PTH-660")
            throw new InvalidOperationException("TabletReference members were not injected before OnDependencyLoad");
        offset = 7;
    }

    public override void Consume(IDeviceReport report)
    {
        if (report is IAbsolutePositionReport positioned)
            positioned.Position += new Vector2(offset, 0);
        Publish(report);
    }
}

public sealed class SelectedSpecificationFilter : BaseFilter
{
    [TabletReference]
    public TabletReference Tablet { get; set; }

    private float width;

    [OnDependencyLoad]
    public void Initialize()
    {
        if (Tablet?.Properties?.Name != "Wacom PTH-860"
            || Tablet.Identifiers?.Count() != 1
            || Tablet.Properties.Specifications?.Digitizer?.Width != 311
            || Tablet.Properties.Specifications.Digitizer.Height != 216
            || Tablet.Properties.Specifications.Digitizer.MaxX != 62200
            || Tablet.Properties.Specifications.Pen?.MaxPressure != 8191)
            throw new InvalidOperationException("Selected tablet specification was not injected");
        width = (float)Tablet.Properties.Specifications.Digitizer.Width;
    }

    public override void Consume(IDeviceReport report)
    {
        if (report is IAbsolutePositionReport positioned)
            positioned.Position += new Vector2(width, 0);
        Publish(report);
    }
}

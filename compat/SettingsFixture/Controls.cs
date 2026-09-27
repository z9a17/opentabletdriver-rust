using OpenTabletDriver.Plugin.Attributes;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Tablet;

namespace SettingsFixture;

/// Declares each generated-control attribute upstream's settings page reads.
public sealed class ControlsFilter : IPositionedPipelineElement<IDeviceReport>
{
    public static string[] Modes => ["Linear", "Smooth"];

    [Property("Mode"), PropertyValidated(nameof(Modes))]
    public string Mode { get; set; } = "Linear";

    [SliderProperty("Strength", 0f, 2f, 0.5f)]
    public float Strength { get; set; }

    [BooleanProperty("Snap", "Snap to the grid")]
    public bool Snap { get; set; }

    public PipelinePosition Position => PipelinePosition.PreTransform;
    public event Action<IDeviceReport> Emit;
    public void Consume(IDeviceReport report) => Emit?.Invoke(report);
}

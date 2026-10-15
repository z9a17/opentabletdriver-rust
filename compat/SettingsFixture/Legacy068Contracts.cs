using System.Numerics;
using OpenTabletDriver.Plugin.Devices;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Tablet;
using OpenTabletDriver.Plugin;
using OpenTabletDriver.Plugin.Attributes;

namespace SettingsFixture;

public static class LegacySdkConstruction
{
    public static Area CreateArea() => new Area();
    public static PropertyAttribute RenameProperty() => new PropertyAttribute("old") { DisplayName = "updated" };
    public static TabletConfiguration CreateConfiguration() => new TabletConfiguration();
}

// Keep this project on SDK 0.6.7. These classes intentionally implement none
// of the 0.6.8 additions; running their original DLL checks default-interface
// compatibility rather than merely recompiling the new contracts successfully.
public sealed class LegacyDirectOutput : IOutputMode
{
    public event Action<IDeviceReport> Emit;
    public IList<IPositionedPipelineElement<IDeviceReport>> Elements { get; set; } = [];
    public Matrix3x2 TransformationMatrix => Matrix3x2.Identity;
    public TabletReference Tablet { get; set; }
    public bool DisablePressure { get; set; }
    public bool DisableTilt { get; set; }
    public void Read(IDeviceReport report) => Consume(report);
    public void Consume(IDeviceReport report) => Emit?.Invoke(report);
    public void Dispose() { }
}

public sealed class LegacyEndpoint : IDeviceEndpoint
{
    public int ProductID => 1;
    public int VendorID => 1;
    public int InputReportLength => 1;
    public int OutputReportLength => 1;
    public int FeatureReportLength => 1;
    public string Manufacturer => "Offline fixture";
    public string ProductName => "Legacy endpoint";
    public string FriendlyName => ProductName;
    public string SerialNumber => "fixture";
    public string DevicePath => "fixture://legacy-endpoint";
    public bool CanOpen => false;
    public IDictionary<string, string> DeviceAttributes { get; } = new Dictionary<string, string>();
    public IDeviceEndpointStream Open() => throw new InvalidOperationException("The offline legacy fixture must never open a device.");
    public string GetDeviceString(byte index) => null;
}

using OpenTabletDriver.Plugin;
using OpenTabletDriver.Plugin.Attributes;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Tablet;

namespace DiscoveryFixture;

public abstract class EchoFilter : IPositionedPipelineElement<IDeviceReport>
{
    public PipelinePosition Position => PipelinePosition.PreTransform;
    public event Action<IDeviceReport> Emit;
    public void Consume(IDeviceReport report) => Emit?.Invoke(report);
}

public sealed class UnrestrictedFilter : EchoFilter { }

[SupportedPlatform(PluginPlatform.Windows)]
public sealed class WindowsFilter : EchoFilter { }

[SupportedPlatform(PluginPlatform.Windows | PluginPlatform.Linux)]
public sealed class WindowsAndLinuxFilter : EchoFilter { }

[SupportedPlatform(PluginPlatform.Linux)]
public sealed class LinuxFilter : EchoFilter { }

[SupportedPlatform(PluginPlatform.Unknown)]
public sealed class UnknownPlatformFilter : EchoFilter { }

[PluginIgnore]
public sealed class IgnoredFilter : EchoFilter { }

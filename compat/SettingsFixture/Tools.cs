using OpenTabletDriver.Plugin;
using OpenTabletDriver.Plugin.Attributes;

namespace SettingsFixture;

/// Records its lifecycle in a file, so tests can see that the driver applied
/// its settings, initialized it and disposed it.
[PluginName("Marker tool")]
public sealed class MarkerTool : ITool
{
    [Property("Marker path")]
    public string MarkerPath { get; set; } = string.Empty;

    public bool Initialize()
    {
        if (string.IsNullOrEmpty(MarkerPath))
            return false;
        File.WriteAllText(MarkerPath, "started");
        return true;
    }

    public void Dispose()
    {
        if (!string.IsNullOrEmpty(MarkerPath))
            File.AppendAllText(MarkerPath, " stopped");
    }
}

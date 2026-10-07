using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;
using OpenTabletDriver.Desktop.Interop;
using OpenTabletDriver.Plugin.Platform.Display;

namespace OtdCompat;

public static unsafe partial class EntryPoints
{
    // Capacity retries reuse the original enumeration, including its geometry.
    [ThreadStatic] static byte[]? originalDisplayJson;

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int OriginalDisplaySnapshot(byte* output, int capacity, int refresh)
    {
        try {
            ServiceClient.RequireBlockingAllowed();
            if (refresh != 0 || originalDisplayJson == null) {
                originalDisplayJson = null;
                var screen = DesktopInterop.CreateHostedDisplaySnapshot()
                    ?? throw new PlatformNotSupportedException("The original Desktop display provider is unavailable.");
                try {
                    static JObject Geometry(IDisplay display) => new() {
                        ["index"] = display.Index, ["x"] = display.Position.X,
                        ["y"] = display.Position.Y, ["width"] = display.Width,
                        ["height"] = display.Height
                    };
                    // The upstream Windows/Wayland/X11 providers include their
                    // aggregate object; it is not a physical monitor.
                    var children = screen.Displays.Where(d => !ReferenceEquals(d, screen)).Take(257).ToArray();
                    if (children.Length == 0 || children.Length > 256)
                        throw new InvalidOperationException("The original display provider returned an invalid physical monitor count.");
                    var snapshot = new JObject {
                        ["provider"] = screen.GetType().FullName ?? screen.GetType().Name,
                        ["virtual_screen"] = Geometry(screen),
                        ["displays"] = new JArray(children.Select(Geometry))
                    };
                    originalDisplayJson = Encoding.UTF8.GetBytes(snapshot.ToString(Formatting.None));
                } finally { (screen as IDisposable)?.Dispose(); }
            }
            return CopyOriginal(originalDisplayJson, output, capacity);
        } catch (Exception error) {
            originalDisplayJson = null;
            lastError = error.GetBaseException().Message;
            return -1;
        }
    }
}

using System.Globalization;
using System.Numerics;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV2;
using OpenTabletDriver.Plugin;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Platform.Pointer;
using OpenTabletDriver.Plugin.Tablet;

namespace OtdUpstreamBench;

/// <summary>
/// Fills the expected outputs of the differential fixtures in
/// tests/differential (F05) by running OpenTabletDriver's own pipeline, as
/// the Rust harness's skeletons describe it. Nothing reaches Windows: a
/// recording pointer keeps each report's position and button changes.
/// </summary>
static class Reference
{
    const int ReportBytes = 192;
    const string RadialFollowType = "RadialFollow.RadialFollowSmoothingTabletSpace";

    /// <summary>What the pointer received for one report.</summary>
    sealed class Recorder : IAbsolutePointer, IRelativePointer, IMouseButtonHandler, ISynchronousPointer
    {
        public Vector2? Position;
        public readonly StringBuilder Events = new();
        public void SetPosition(Vector2 pos) => Position = pos;
        public void MouseDown(MouseButton button) => Events.Append(button == MouseButton.Left ? 'D' : '?');
        public void MouseUp(MouseButton button) => Events.Append(button == MouseButton.Left ? 'U' : '?');
        public void Flush() { }
        public void Reset() { }

        public void Clear()
        {
            Position = null;
            Events.Clear();
        }

        /// <summary>"x,y", "x,y,EVENTS", ",,EVENTS" or "" in invariant digits.</summary>
        public string Format(int decimals)
        {
            string position = Position is { } p
                ? string.Create(CultureInfo.InvariantCulture, $"{p.X.ToString("F" + decimals, CultureInfo.InvariantCulture)},{p.Y.ToString("F" + decimals, CultureInfo.InvariantCulture)}")
                : Events.Length > 0 ? "," : "";
            return Events.Length > 0 ? $"{position},{Events}" : position;
        }
    }

    static byte[] Report(string hex)
    {
        var bytes = new byte[ReportBytes];
        var parts = hex.Split(' ', StringSplitOptions.RemoveEmptyEntries);
        for (int i = 0; i < parts.Length; i++)
            bytes[i] = byte.Parse(parts[i], NumberStyles.HexNumber, CultureInfo.InvariantCulture);
        return bytes;
    }

    static float Float(JsonNode? node) => (float)node!.GetValue<double>();

    static Reader Build(JsonNode upstream, string mode, string? radialFollowPath, Recorder recorder)
    {
        float tip = Float(upstream["tip_threshold_percent"]), eraser = Float(upstream["eraser_threshold_percent"]);
        if (mode == "relative")
        {
            var sensitivity = upstream["sensitivity"]!.AsArray();
            return Pipelines.Relative(
                new Vector2(Float(sensitivity[0]), Float(sensitivity[1])), Float(upstream["rotation"]),
                TimeSpan.FromMilliseconds(upstream["reset_ms"]!.GetValue<double>()), tip, eraser, recorder, recorder);
        }
        IPositionedPipelineElement<IDeviceReport>? filter = null;
        if (upstream["radial_follow"] is JsonObject settings)
        {
            if (radialFollowPath == null)
                throw new ArgumentException("this fixture needs --radialfollow");
            filter = Pipelines.Filter(radialFollowPath, RadialFollowType, settings);
        }
        return Pipelines.Absolute(
            Workload.ReadArea(upstream["display"]!), Workload.ReadArea(upstream["tablet"]!),
            upstream["clipping"]!.GetValue<bool>(), upstream["limiting"]!.GetValue<bool>(),
            tip, eraser, recorder, recorder, filter);
    }

    static JsonArray Expected(JsonNode @case, byte[][] reports, string? radialFollowPath)
    {
        string mode = @case["mode"]!.GetValue<string>();
        var upstream = @case["upstream"]!;
        // Compile everything once on a separate pipeline, so JIT pauses cannot
        // reach the recorded one's reset timers.
        var warm = Build(upstream, mode, radialFollowPath, new Recorder());
        for (int pass = 0; pass < 3; pass++)
            foreach (var report in reports)
                warm.OnData(report);
        var recorder = new Recorder();
        var reader = Build(upstream, mode, radialFollowPath, recorder);
        var expected = new JsonArray();
        foreach (var report in reports)
        {
            recorder.Clear();
            reader.OnData(report);
            expected.Add(recorder.Format(mode == "relative" ? 5 : 4));
        }
        return expected;
    }

    /// <summary>The first raw pressure that presses the tip at each threshold.</summary>
    static void Thresholds(JsonObject fixture)
    {
        int maxPressure = fixture["max_pressure"]!.GetValue<int>();
        var parser = new IntuosV2ReportParser();
        foreach (var entry in fixture["thresholds"]!.AsArray())
        {
            var recorder = new Recorder();
            var handler = Pipelines.Bindings(Float(entry!["percent"]), 100f, recorder);
            int first = -1;
            for (int raw = 0; raw <= maxPressure && first < 0; raw++)
            {
                var bytes = new byte[ReportBytes];
                bytes[0] = 0x10;
                bytes[1] = 0x61;
                bytes[8] = (byte)raw;
                bytes[9] = (byte)(raw >> 8);
                handler.Consume(parser.Parse(bytes));
                if (recorder.Events.Length > 0)
                    first = raw;
            }
            entry["first_pressing_raw"] = first;
        }
    }

    public static void Fill(string path, string? radialFollowPath)
    {
        var fixture = JsonNode.Parse(File.ReadAllText(path))!.AsObject();
        var provenance = fixture["provenance"]!.AsObject();
        provenance["upstream_revision"] = Environment.GetEnvironmentVariable("OTD_UPSTREAM_COMMIT")
            ?? throw new ArgumentException("set OTD_UPSTREAM_COMMIT to the pinned upstream revision");
        provenance["runtime"] = System.Runtime.InteropServices.RuntimeInformation.FrameworkDescription;
        if (fixture["schema"]!.GetValue<string>() == "otd-differential-thresholds/1")
        {
            Thresholds(fixture);
        }
        else
        {
            var reports = fixture["reports"]!.AsArray().Select(r => Report(r!.GetValue<string>())).ToArray();
            foreach (var @case in fixture["cases"]!.AsArray())
                @case!["expected"] = Expected(@case, reports, radialFollowPath);
            if (fixture["cases"]!.AsArray().Any(c => c!["upstream"]!["radial_follow"] is JsonObject))
                provenance["plugins"] = new JsonArray(new JsonObject
                {
                    ["name"] = "RadialFollow",
                    ["version"] = "0.3.0",
                    ["class"] = RadialFollowType,
                    ["dll_sha256"] = Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(radialFollowPath!))).ToLowerInvariant(),
                });
        }
        File.WriteAllText(path, fixture.ToJsonString(new JsonSerializerOptions
        {
            WriteIndented = true,
            TypeInfoResolver = new System.Text.Json.Serialization.Metadata.DefaultJsonTypeInfoResolver(),
        }) + "\n");
    }
}

using System.Text.Json;
using System.Text.Json.Nodes;
using OpenTabletDriver.Configurations;
using OpenTabletDriver.Plugin.Tablet;
using OpenTabletDriver.Plugin.Tablet.Touch;
using OpenTabletDriver.Plugin.Tablet.Wheel;

namespace OtdUpstreamBench;

/// <summary>
/// Writes tests/parsers/upstream.json: seeded packets for every report parser
/// the pinned configurations name, each decoded by OpenTabletDriver's own
/// parser class, in order, with one parser instance per type so stateful
/// parsers see the sequence. The Rust test crates/otd-core/src/decoders.rs
/// decodes the same packets and compares every field.
/// </summary>
static class Parsers
{
    const int Seed = 20260925;
    const int PacketsPerParser = 200;

    static readonly int[] Lengths = [2, 3, 4, 5, 6, 7, 8, 8, 9, 10, 10, 11, 12, 12, 13, 14, 15, 16, 17, 19, 20, 40, 64];
    static readonly byte[] FirstBytes = [0x00, 0x01, 0x02, 0x02, 0x03, 0x05, 0x0c, 0x10, 0x10, 0x11, 0x1e, 0x1f, 0x21, 0xd2];
    // Headers some parsers need before they decode a pen report: FlooGoo,
    // Acepen, Veikk tilt, RobotPen, BambooPad and IntuosV3.
    static readonly byte[][] Headers = [[0x01, 0x20], [0x01, 0x2a], [0x00, 0x41, 0xa0], [0x00, 0x41, 0xa6], [0x00, 0x41, 0x21], [0x00, 0x42], [0x10, 0x01], [0x1f, 0x01], [0x1f, 0x01, 0x60]];
    static readonly byte[] SecondBytes = [0x00, 0x01, 0x02, 0x20, 0x40, 0x41, 0x42, 0x80, 0x81, 0xa0, 0xa2, 0xaa, 0xac, 0xc0, 0xc2, 0xe0, 0xe3, 0xea, 0xec, 0xf0, 0xf8];

    public static void Write(string path)
    {
        var identifiers = new DeviceConfigurationProvider().TabletConfigurations
            .SelectMany(c => c.DigitizerIdentifiers.Concat(c.AuxiliaryDeviceIdentifiers ?? []))
            .ToList();
        var types = identifiers.Select(i => i.ReportParser).Distinct().Order(StringComparer.Ordinal).ToList();
        var assemblies = new[] { typeof(DeviceConfigurationProvider).Assembly, typeof(IReportParser<>).Assembly };
        var random = new Random(Seed);
        var cases = new JsonArray();
        foreach (string name in types)
        {
            var type = assemblies.Select(a => a.GetType(name)).FirstOrDefault(t => t != null)
                ?? throw new InvalidOperationException($"no parser type {name}");
            var parser = (IReportParser<IDeviceReport>)Activator.CreateInstance(type)!;
            // Most packets use a length a configuration declares for this parser.
            int[] configured = identifiers
                .Where(i => i.ReportParser == name && i.InputReportLength is > 0 and <= 64)
                .Select(i => (int)i.InputReportLength!.Value)
                .Distinct()
                .Order()
                .ToArray();
            var reports = new JsonArray();
            for (int n = 0; n < PacketsPerParser; n++)
            {
                byte[] packet = Packet(random, configured);
                var entry = new JsonObject { ["hex"] = Convert.ToHexString(packet).ToLowerInvariant() };
                try
                {
                    var report = parser.Parse(packet);
                    entry["report"] = report == null ? null : Describe(report);
                }
                catch (Exception error)
                {
                    entry["error"] = error.GetType().Name;
                }
                reports.Add(entry);
            }
            cases.Add(new JsonObject { ["parser"] = name, ["reports"] = reports });
        }
        var document = new JsonObject
        {
            ["source"] = "bench/upstream --parsers; OpenTabletDriver " + (Environment.GetEnvironmentVariable("OTD_UPSTREAM_COMMIT") ?? "(set OTD_UPSTREAM_COMMIT)"),
            ["seed"] = Seed,
            ["cases"] = cases,
        };
        using var stream = File.Create(path);
        using var writer = new Utf8JsonWriter(stream, new JsonWriterOptions { Indented = false });
        document.WriteTo(writer);
    }

    static byte[] Packet(Random random, int[] configured)
    {
        int length = configured.Length > 0 && random.Next(3) != 0
            ? configured[random.Next(configured.Length)]
            : Lengths[random.Next(Lengths.Length)];
        var packet = new byte[length];
        random.NextBytes(packet);
        if (random.Next(4) != 0)
            packet[0] = FirstBytes[random.Next(FirstBytes.Length)];
        if (packet.Length > 1 && random.Next(3) != 0)
            packet[1] = SecondBytes[random.Next(SecondBytes.Length)];
        // Prefixed parsers dispatch on the next byte pair.
        if (packet.Length > 2 && random.Next(4) == 0)
        {
            packet[1] = FirstBytes[random.Next(FirstBytes.Length)];
            packet[2] = SecondBytes[random.Next(SecondBytes.Length)];
        }
        if (random.Next(5) == 0)
        {
            byte[] header = Headers[random.Next(Headers.Length)];
            header.AsSpan(0, Math.Min(header.Length, packet.Length)).CopyTo(packet);
            return packet;
        }
        // Wacom touch chunks: a small count and the mask report.
        if (packet.Length > 2 && random.Next(6) == 0)
        {
            packet[1] = (byte)random.Next(4);
            if (random.Next(2) == 0)
                packet[2] = 0x81;
        }
        return packet;
    }

    static JsonArray Bools(bool[] values) => new(values.Select(v => (JsonNode)JsonValue.Create(v)).ToArray());

    static JsonNode Describe(IDeviceReport report)
    {
        var d = new JsonObject { ["kind"] = report is OutOfRangeReport ? "out_of_range" : "data" };
        if (report is IAbsolutePositionReport position)
            d["position"] = new JsonArray(position.Position.X, position.Position.Y);
        if (report is ITabletReport tablet)
        {
            d["pressure"] = tablet.Pressure;
            d["pen_buttons"] = Bools(tablet.PenButtons);
        }
        if (report is ITiltReport tilt)
            d["tilt"] = new JsonArray(tilt.Tilt.X, tilt.Tilt.Y);
        if (report is IEraserReport eraser)
            d["eraser"] = eraser.Eraser;
        if (report is IProximityReport proximity)
        {
            d["near_proximity"] = proximity.NearProximity;
            d["hover_distance"] = proximity.HoverDistance;
        }
        if (report is IAuxReport aux)
            d["aux_buttons"] = Bools(aux.AuxButtons);
        if (report is IMouseReport mouse)
        {
            d["mouse_buttons"] = Bools(mouse.MouseButtons);
            d["mouse_scroll"] = new JsonArray(mouse.Scroll.X, mouse.Scroll.Y);
        }
        if (report is IToolReport tool)
            d["tool"] = new JsonObject { ["serial"] = tool.Serial, ["raw_tool_id"] = tool.RawToolID, ["eraser"] = tool.Tool == ToolType.Eraser };
        if (report is ITouchReport touch)
            d["touches"] = new JsonArray(touch.Touches.Select(t => t == null ? null : (JsonNode)new JsonObject { ["id"] = t.TouchID, ["x"] = t.Position.X, ["y"] = t.Position.Y }).ToArray());
        if (report is IAbsoluteAnalogReport absolute)
            d["analog_positions"] = new JsonArray(absolute.AnalogPositions.Select(v => v == null ? null : (JsonNode)JsonValue.Create(v.Value)).ToArray());
        if (report is IRelativeAnalogReport relative)
            d["analog_deltas"] = new JsonArray(relative.AnalogDeltas.Select(v => (JsonNode)JsonValue.Create(v)).ToArray());
        if (report is IWheelButtonReport wheel)
            d["wheel_buttons"] = new JsonArray(wheel.WheelButtons.Select(b => (JsonNode)Bools(b)).ToArray());
        return d;
    }
}

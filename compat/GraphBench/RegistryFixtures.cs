using System.Text;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using Newtonsoft.Json.Linq;
using OtdCompat;

// Offline source fixtures only; deliberately unexecuted by repo suite policy.
unsafe static class RegistryProbe
{
    static int sourceOutputs;
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static int SourceOutput(nint scope, uint operation, uint index, GraphReport* frame)
    {
        if (operation == 4) { if (frame->RawLength != 1 || frame->Raw[0] != 42) return -1; sourceOutputs++; }
        return 0;
    }
    static void SourceGraph(string[] args, string path)
    {
        using var parser = new ParserSession("SettingsFixture.RewriteRawParser");
        using var filter = new Instance(new JObject { ["assembly_path"] = path, ["type_name"] = "SettingsFixture.ConcreteReportFilter", ["settings"] = new JObject(), ["tablet"] = JObject.Parse(File.ReadAllText(args[1])) });
        var handle = GCHandle.Alloc(filter);
        try {
            var graph = new SynchronousGraph([new GraphNode { Context = GCHandle.ToIntPtr(handle), Index = 0, Stage = 1 }]);
            byte[] wire = [7]; ParsedSourceReport projected = default;
            fixed (byte* raw = wire) if (parser.Project(raw, 1, &projected) != 1) throw new Exception("Original parser did not emit.");
            var original = ParserSession.Take(projected.Parser, projected.Sequence);
            if (original.GetType().FullName != "SettingsFixture.StatefulReport" || original.Raw[0] != 42) throw new Exception("Source parser identity/raw replaced.");
            if (graph.Dispatch(&projected.Report, &SourceOutput, 0, true, original) != 0 || sourceOutputs != 1) throw new Exception("Original object did not traverse filter/output graph.");
            try { ParserSession.Take(projected.Parser, projected.Sequence); throw new Exception("Consumed source token replayed."); } catch (InvalidOperationException) { }
            ulong prior = projected.Sequence;
            fixed (byte* raw = wire) parser.Project(raw, 1, &projected);
            try { ParserSession.Take(projected.Parser, prior); throw new Exception("Expired source sequence accepted."); } catch (InvalidOperationException) { }
            parser.Reset();
            try { ParserSession.Take(projected.Parser, projected.Sequence); throw new Exception("Reset source token accepted."); } catch (InvalidOperationException) { }
            fixed (byte* raw = wire) parser.Project(raw, 1, &projected);
            parser.Dispose();
            try { ParserSession.Take(projected.Parser, projected.Sequence); throw new Exception("Disposed source ID accepted."); } catch (InvalidOperationException) { }
        } finally { handle.Free(); }
    }
    static JObject Decode(ParserSession parser, byte[] raw)
    {
        int size; fixed (byte* bytes = raw) size = parser.Decode(bytes, (uint)raw.Length);
        byte[] output = new byte[size];
        if (parser.Copy(output) != size || parser.Copy(output) != size) throw new Exception("Capacity retry lost retained report.");
        return JObject.Parse(Encoding.UTF8.GetString(output));
    }
    public static void Run(string[] args)
    {
        using (var parser = new ParserSession("OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV2.IntuosV2ReportParser"))
        {
            byte[] first = new byte[40]; first[0] = 0x21; first[2] = 1; first[3] = 1; first[4] = 10;
            var data = Decode(parser, first);
            if ((string?)data["Path"] != "OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV2.IntuosV2TouchReport" || (float?)data["Data"]?["Touches"]?[0]?["Position"]?["X"] != 10) throw new Exception("Concrete upstream report identity/schema changed.");
            byte[] second = (byte[])first.Clone(); second[2] = 2; second[4] = 20;
            data = Decode(parser, second);
            if ((float?)data["Data"]?["Touches"]?[0]?["Position"]?["X"] != 10 || (float?)data["Data"]?["Touches"]?[1]?["Position"]?["X"] != 20) throw new Exception("Stateful parser lost the preceding touch.");
            parser.Reset(); data = Decode(parser, second);
            if (data["Data"]?["Touches"]?[0]?.Type != JTokenType.Null) throw new Exception("Reset retained pre-gap parser state.");
            parser.Reset(); ParsedSourceReport projection = default;
            fixed (byte* bytes = first) if (parser.Project(bytes, (uint)first.Length, &projection) != 1) throw new Exception("Touch source projection missing.");
            ulong primary = projection.Parser;
            using (var auxiliary = new ParserSession("OpenTabletDriver.Plugin.Tablet.PassthroughReportParser")) {
                ParsedSourceReport other = default; byte[] rawAux = [0];
                fixed (byte* bytes = rawAux) auxiliary.Project(bytes, 1, &other);
                if (other.Parser == primary) throw new Exception("Endpoint parser identities collided.");
                ParserSession.Take(other.Parser, other.Sequence);
            }
            fixed (byte* bytes = second) parser.Project(bytes, (uint)second.Length, &projection);
            if (projection.Report.TouchXY[0] != 10 || projection.Report.TouchXY[2] != 20) throw new Exception("Source projection lost touch state across auxiliary packet.");
            ParserSession.Take(projection.Parser, projection.Sequence);
            parser.Reset(); fixed (byte* bytes = second) parser.Project(bytes, (uint)second.Length, &projection);
            if ((projection.Report.TouchPresent & 1) != 0) throw new Exception("Source reset retained prior touch state.");
            try { fixed (byte* bytes = first) parser.Decode(bytes, 0); throw new Exception("Malformed empty packet accepted."); } catch (ArgumentException) { }
        }
        if (args.Length < 4) throw new ArgumentException("registry fixtures require an isolated E: scratch directory as argument 4.");
        string root = Path.GetFullPath(args[3]);
        if (Directory.Exists(root)) throw new InvalidOperationException("Use a fresh fixture directory.");
        string directory = Path.Combine(root, "fixture"); Directory.CreateDirectory(directory);
        string path = Path.Combine(directory, Path.GetFileName(args[0])); File.Copy(args[0], path);
        JObject registry = JObject.Parse(Encoding.UTF8.GetString(InstalledRegistry.Reload(root)));
        if (!registry["types"]!.Any(entry => (string?)entry["metadata"]?["type_name"] == "SettingsFixture.StatefulParser")) throw new Exception("Installed parser was not loaded.");
        SourceGraph(args, path);
        var lease = new PluginLoad(path); var assembly = lease.LoadFromAssemblyPath(path);
        using (var parser = new ParserSession("SettingsFixture.StatefulParser"))
        {
            if ((int?)Decode(parser, new byte[] { 1 })["Data"]?["Tick"] != 1 || (int?)Decode(parser, new byte[] { 2 })["Data"]?["Tick"] != 2) throw new Exception("Buffer fetch parsed a stateful packet twice.");
            File.Delete(path); Directory.Delete(directory);
            var replaced = JObject.Parse(Encoding.UTF8.GetString(InstalledRegistry.Reload(root)));
            if ((ulong?)replaced["generation"] <= (ulong?)registry["generation"] || replaced["types"]!.Any()) throw new Exception("Actual registry reload did not replace discovery state.");
            if (!ReferenceEquals(assembly, lease.LoadFromAssemblyPath(path))) throw new Exception("Active instance lost its retired generation.");
            parser.Reset(); if ((int?)Decode(parser, new byte[] { 3 })["Data"]?["Tick"] != 1) throw new Exception("Retired parser factory was unloaded during reset.");
            byte[] ignored = [255]; fixed (byte* bytes = ignored) if (parser.Decode(bytes, 1) != 0) throw new Exception("Null parser emission fabricated a report.");
        }
        lease.Unload(); lease.Unload(); Directory.Delete(root);
        Console.WriteLine("PASS actual installed registry lifetime and exact concrete stateful report contracts");
    }
}

using System.Text;
using Newtonsoft.Json.Linq;
using OtdCompat;

// Offline source fixtures only; deliberately unexecuted by repo suite policy.
unsafe static class RegistryProbe
{
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
            try { fixed (byte* bytes = first) parser.Decode(bytes, 0); throw new Exception("Malformed empty packet accepted."); } catch (ArgumentException) { }
        }
        if (args.Length < 4) throw new ArgumentException("registry fixtures require an isolated E: scratch directory as argument 4.");
        string root = Path.GetFullPath(args[3]);
        if (Directory.Exists(root)) throw new InvalidOperationException("Use a fresh fixture directory.");
        string directory = Path.Combine(root, "fixture"); Directory.CreateDirectory(directory);
        string path = Path.Combine(directory, Path.GetFileName(args[0])); File.Copy(args[0], path);
        JObject registry = JObject.Parse(Encoding.UTF8.GetString(InstalledRegistry.Reload(root)));
        if (!registry["types"]!.Any(entry => (string?)entry["metadata"]?["type_name"] == "SettingsFixture.StatefulParser")) throw new Exception("Installed parser was not loaded.");
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

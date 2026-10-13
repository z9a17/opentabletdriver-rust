using System.Reflection;
using Newtonsoft.Json.Linq;
using OpenTabletDriver.Plugin.Tablet;
using OtdCompat;

// Failure injection only: these scopes contain a fake IDisposable, never an
// actual pointer, keyboard, device, service callback or desktop application.
static class CleanupProbe
{
    sealed class RejectedCleanup : IDisposable
    {
        internal int Calls;
        public void Dispose() { Calls++; throw new IOException("fixture service disposal failure"); }
    }
    static FieldInfo Field(Type type, string name) => type.GetField(name, BindingFlags.Instance | BindingFlags.NonPublic)!;
    static RejectedCleanup FailServices(object owner, Type type)
    {
        var services = (HostServices)Field(type, "services").GetValue(owner)!;
        var inputs = new OriginalInputServices();
        var failure = new RejectedCleanup();
        ((List<IDisposable>)Field(typeof(OriginalInputServices), "owned").GetValue(inputs)!).Add(failure);
        Field(typeof(HostServices), "originalInputs").SetValue(services, inputs);
        return failure;
    }
    static void RetiresDespiteServiceFailure(IDisposable owner, Type type, RegistryGeneration generation)
    {
        int before = generation.References;
        var failure = FailServices(owner, type);
        try { owner.Dispose(); throw new Exception("Failed service cleanup was silently accepted."); }
        catch (AggregateException error) when (error.InnerExceptions.Any(value => value.Message == "fixture service disposal failure")) { }
        if (failure.Calls != 1 || generation.References != before - 1)
            throw new Exception($"{type.Name} leaked its registry lease when service disposal failed.");
        owner.Dispose();
        if (failure.Calls != 1 || generation.References != before - 1)
            throw new Exception($"{type.Name} repeated a failed disposal or registry release.");
    }
    internal static void Run(string[] args)
    {
        string path = Path.GetFullPath(args[0]);
        Assembly fixture = Assembly.LoadFrom(path);
        Type parser = fixture.GetType("SettingsFixture.StatefulParser", throwOnError: true)!;
        var generation = new RegistryGeneration();
        generation.Assemblies.Add(path, fixture);
        generation.Types.Add(parser.FullName!, [parser]);
        FieldInfo current = typeof(InstalledRegistry).GetField("current", BindingFlags.Static | BindingFlags.NonPublic)!;
        if (current.GetValue(null) != null) throw new InvalidOperationException("Cleanup fixtures require a fresh process.");
        current.SetValue(null, generation);
        try {
            var config = new JObject { ["assembly_path"] = path, ["type_name"] = "SettingsFixture.DefaultsFilter",
                ["tablet"] = JObject.Parse(File.ReadAllText(args[1])), ["settings"] = new JObject(),
                ["keys"] = new JObject { ["A"] = 4 } };
            RetiresDespiteServiceFailure(new Instance(config), typeof(Instance), generation);
            config["type_name"] = "SettingsFixture.EndpointBinding";
            RetiresDespiteServiceFailure(new BindingInstance(config), typeof(EndpointInstance), generation);
            RetiresDespiteServiceFailure(new ProviderParser(parser.FullName!), typeof(ProviderParser), generation);
            RetiresDespiteServiceFailure(new ParserSession(parser.FullName!), typeof(ParserSession), generation);
            config["type_name"] = "SettingsFixture.ThrowingUnsubscribeFilter";
            var rejecting = new Instance(config);
            object filter = Field(typeof(Instance), "filter").GetValue(rejecting)!;
            try { rejecting.Dispose(); throw new Exception("Failed event unsubscribe was silently accepted."); }
            catch (InvalidOperationException error) when (error.Message == "fixture unsubscribe failure") { }
            rejecting.Dispose();
            if ((int)filter.GetType().GetProperty("Disposes")!.GetValue(filter)! != 1)
                throw new Exception("Rejecting event unsubscribe skipped plugin disposal.");
            // An error before endpoint service construction must also retire
            // the lease acquired at the start of the endpoint constructor.
            config["owner"] = "invalid owner";
            try { _ = new BindingInstance(config); throw new Exception("Malformed endpoint configuration accepted."); }
            catch (FormatException) { }
            if (generation.References != 1) throw new Exception("Cleanup fixture left registry holders behind.");
        } finally { current.SetValue(null, null); InstalledRegistry.Release(generation); }
        Console.WriteLine("PASS failed managed service disposal releases every filter/endpoint/parser registry lease without OS input.");
    }
}

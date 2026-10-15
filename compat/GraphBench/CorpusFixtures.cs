using System.Collections.ObjectModel;
using System.Numerics;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using Newtonsoft.Json.Linq;
using OpenTabletDriver.Desktop.Reflection;
using OpenTabletDriver.Plugin.Attributes;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Platform.Pointer;
using OpenTabletDriver.Plugin.Tablet;
using OtdCompat;

// Intentionally small, source-reviewed allowlist. This is an offline harness,
// not a plugin sandbox: hashes are verified before any assembly is loaded.
unsafe static class CorpusProbe
{
    static readonly HashSet<string> Reviewed = [
        "RadialFollow.RadialFollowSmoothingTabletSpace",
        "RadialFollow.RadialFollowSmoothingScreenSpace",
        "BezierInterpolator.BezierInterp",
        "Flip_Axes.Flip_Tablet_Axes",
        "Flip_Axes.Flip_Area_Axes"
    ];
    sealed class ModelAbsolute : AbsoluteOutputMode
    {
        public override IAbsolutePointer? Pointer { get; set; }
    }
    sealed class Packet : ITabletReport, ITiltReport
    {
        public byte[] Raw { get; set; } = [0x10, 0x20, 0x30];
        public Vector2 Position { get; set; }
        public uint Pressure { get; set; }
        public bool[] PenButtons { get; set; } = [true, false];
        public Vector2 Tilt { get; set; }
    }
    sealed class AuxPacket : IAuxReport
    {
        public byte[] Raw { get; set; } = [0x11, 0x22];
        public bool[] AuxButtons { get; set; } = [false, true, false];
    }
    sealed class ManualTimer : OpenTabletDriver.Plugin.Timers.ITimer
    {
        public float Interval { get; set; }
        public bool Enabled { get; private set; }
        public event Action? Elapsed;
        public void Start() => Enabled = true;
        public void Stop() => Enabled = false;
        public void Dispose() => Stop();
        internal void Fire() { if (Enabled) Elapsed?.Invoke(); }
    }
    readonly record struct Frame(Vector2 Position, uint Pressure, Vector2 Tilt, Type Type, byte[] Raw, bool[] Buttons);
    static readonly List<Frame> outputs = [];
    static Frame Capture(IDeviceReport report) => report is ITabletReport tablet
        ? new(tablet.Position, tablet.Pressure, (report as ITiltReport)?.Tilt ?? default, report.GetType(), report.Raw.ToArray(), tablet.PenButtons.ToArray())
        : new(default, 0, default, report.GetType(), report.Raw.ToArray(), (report as IAuxReport)?.AuxButtons.ToArray() ?? []);
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static int Callback(nint scope, uint operation, uint index, GraphReport* frame)
    {
        if (operation is 3 or 4 or 5) outputs.Add(Capture(SynchronousGraph.CurrentReport!));
        return 0;
    }
    static object Filter(Instance instance) => typeof(Instance).GetField("filter", BindingFlags.Instance | BindingFlags.NonPublic)!.GetValue(instance)!;
    static void Pin(string path, string expected)
    {
        string observed = Convert.ToHexString(SHA256.HashData(File.ReadAllBytes(path)));
        if (!observed.Equals(expected, StringComparison.OrdinalIgnoreCase))
            throw new InvalidDataException($"Corpus SHA-256 mismatch: {Path.GetFileName(path)} ({observed}).");
    }
    static void Dispatch(SynchronousGraph graph, IDeviceReport packet)
    {
        GraphReport frame = new() { Version = 2, Size = (uint)sizeof(GraphReport) };
        if (graph.Dispatch(&frame, &Callback, 0, originalReport: packet) != 0) throw new Exception(graph.Error);
    }
    static void Due(Instance instance)
    {
        // Force only the test-owned session timer deadline; no wall-clock sleep,
        // OS timer, real endpoint or timing-jitter assertion is involved.
        foreach (object timer in (System.Collections.IEnumerable)typeof(Instance)
            .GetField("timers", BindingFlags.Instance | BindingFlags.NonPublic)!.GetValue(instance)!)
            timer.GetType().GetField("due", BindingFlags.Instance | BindingFlags.NonPublic)!.SetValue(timer, 0L);
    }
    static void Equivalent(Frame expected, Frame observed)
    {
        if (Vector2.Distance(expected.Position, observed.Position) > 0.001f || expected.Pressure != observed.Pressure
            || Vector2.Distance(expected.Tilt, observed.Tilt) > 0.001f || expected.Type != observed.Type
            || !expected.Raw.SequenceEqual(observed.Raw) || !expected.Buttons.SequenceEqual(observed.Buttons))
            throw new Exception($"Reference/bridge output differs: expected {expected}, observed {observed}.");
        if (!float.IsFinite(observed.Position.X) || !float.IsFinite(observed.Position.Y))
            throw new Exception("Corpus emitted a non-finite position.");
    }
    // Use the actual vendored upstream method, independently of Instance's
    // settings adapter. A null Type avoids discovery/AppInfo initialization.
    static void ReferenceSettings(object filter, JObject settings)
    {
        var store = new PluginSettingStore((Type)null!) { Settings = new ObservableCollection<PluginSetting>() };
        foreach (JProperty entry in settings.Properties())
            store.Settings.Add(new PluginSetting(entry.Name, null) { Value = entry.Value });
        store.ApplySettings(filter);
    }
    static void ReferenceTablet(object filter, JObject config)
    {
        var tablet = Instance.CreateTabletReference(config);
        foreach (PropertyInfo property in filter.GetType().GetProperties())
            if (property.GetCustomAttribute<TabletReferenceAttribute>() != null) property.SetValue(filter, tablet);
    }
    static ModelAbsolute? FlipModel(object filter, JObject config)
    {
        if (!filter.GetType().FullName!.StartsWith("Flip_Axes.", StringComparison.Ordinal)) return null;
        // The reviewed plugin discovers this cache through a concrete Driver.
        // Supply only the pure coordinate model: never construct a Driver,
        // query endpoints, attach a pointer or invoke an output mode.
        var model = new ModelAbsolute { Tablet = Instance.CreateTabletReference(config),
            Output = new OpenTabletDriver.Plugin.Area(1920, 1080, new(960, 540)) };
        Type owner = filter.GetType().BaseType!;
        owner.GetField("absolute_output_mode", BindingFlags.Instance | BindingFlags.NonPublic)!.SetValue(filter, model);
        FieldInfo kind = owner.GetField("output_mode_type")!;
        kind.SetValue(filter, Enum.ToObject(kind.FieldType, 0));
        return model;
    }
    static void FlipOracle(object filter, JObject config, Vector2 input, Vector2 output)
    {
        string type = filter.GetType().FullName!;
        if (!type.StartsWith("Flip_Axes.", StringComparison.Ordinal)) return;
        bool x = (bool)filter.GetType().GetProperty("Flip_X")!.GetValue(filter)!;
        bool y = (bool)filter.GetType().GetProperty("Flip_Y")!.GetValue(filter)!;
        var tablet = Instance.CreateTabletReference(config).Properties.Specifications.Digitizer;
        Vector2 extent = type.EndsWith("Flip_Tablet_Axes", StringComparison.Ordinal)
            ? new(tablet.MaxX, tablet.MaxY) : new(1920, 1080);
        Vector2 expected = new(x ? extent.X - input.X : input.X, y ? extent.Y - input.Y : input.Y);
        // Normalized-coordinate round trips include float cancellation at
        // full tablet resolution; 0.01 raw unit/pixel bounds that rounding.
        if (Vector2.Distance(expected, output) > 0.01f) throw new Exception("Flip Axes analytic oracle mismatch.");
    }
    static void CheckSettings(object expected, object observed)
    {
        foreach (PropertyInfo property in expected.GetType().GetProperties())
            if (property.CanRead && property.GetCustomAttribute<PropertyAttribute>() != null
                && !Equals(property.GetValue(expected), observed.GetType().GetProperty(property.Name)!.GetValue(observed)))
                throw new Exception($"Corpus setting mismatch for {property.Name}.");
    }
    static void ZeroRadialDelta(object filter)
    {
        if (!filter.GetType().FullName!.StartsWith("RadialFollow.", StringComparison.Ordinal)) return;
        object core = filter.GetType().GetField("radialCore", BindingFlags.Instance | BindingFlags.NonPublic)!.GetValue(filter)!;
        var watch = (OpenTabletDriver.Plugin.Timing.HPETDeltaStopwatch)core.GetType()
            .GetField("stopwatch", BindingFlags.Instance | BindingFlags.NonPublic)!.GetValue(core)!;
        // Restart() on a reset/stopped watch returns exactly zero. Keep the
        // smoothing differential independent of JIT and desktop scheduling;
        // the 50ms re-entry branch remains outside this corpus evidence.
        watch.Reset();
    }
    static void RunCase(JObject config, string name, bool async)
    {
        using var instance = new Instance(config);
        var context = new OtdCompat.PluginContext(config.Value<string>("assembly_path")!);
        IPositionedPipelineElement<IDeviceReport>? reference = null;
        var handle = GCHandle.Alloc(instance);
        try
        {
            Type type = context.LoadFromAssemblyPath(config.Value<string>("assembly_path")!).GetType(config.Value<string>("type_name")!, true)!;
            reference = (IPositionedPipelineElement<IDeviceReport>)Activator.CreateInstance(type)!;
            using var timer = new ManualTimer();
            if (reference is AsyncPositionedPipelineElement<IDeviceReport> timed)
            {
                timed.Frequency = 1000;
                timed.Scheduler = timer;
            }
            ReferenceSettings(reference, (JObject)config["settings"]!);
            ReferenceTablet(reference, config);
            using var referenceModel = FlipModel(reference, config);
            using var bridgeModel = FlipModel(Filter(instance), config);
            CheckSettings(reference, Filter(instance));
            var referenceOutputs = new List<Frame>();
            reference.Emit += report => referenceOutputs.Add(Capture(report ?? throw new Exception("Reference emitted null.")));
            using var graph = new SynchronousGraph([new GraphNode { Context = GCHandle.ToIntPtr(handle), Index = 0, Stage = (uint)instance.Position }]);
            outputs.Clear();
            if (!async)
            {
                for (int i = 0; i < 24; i++)
                {
                    // First point at zero deterministically initializes RadialFollow.
                    Packet Packet() => new() { Position = new(i * 41, i * 23), Pressure = (uint)(i * 31), Tilt = new(i, -i) };
                    ZeroRadialDelta(reference); ZeroRadialDelta(Filter(instance));
                    reference.Consume(Packet());
                    Dispatch(graph, Packet());
                    if (outputs.Count != referenceOutputs.Count) throw new Exception("Corpus lost synchronous output.");
                    Equivalent(referenceOutputs[^1], outputs[^1]);
                    FlipOracle(Filter(instance), config, new(i * 41, i * 23), outputs[^1].Position);
                }
                byte[] loss = [0xff, 0x00];
                int beforeReset = outputs.Count, referenceBeforeReset = referenceOutputs.Count;
                reference.Consume(new OutOfRangeReport(loss));
                instance.Reset(loss);
                if (graph.Tick(&Callback, 0) != 0) throw new Exception(graph.Error);
                // Reset delivers range loss into the filter but intentionally
                // discards its emissions. Mirror that host lifecycle boundary
                // on the direct reference before comparing resumed input.
                referenceOutputs.RemoveRange(referenceBeforeReset, referenceOutputs.Count - referenceBeforeReset);
                if (outputs.Count != beforeReset) throw new Exception("Reset emitted through the retired input boundary.");
                ZeroRadialDelta(reference); ZeroRadialDelta(Filter(instance));
                reference.Consume(new Packet { Position = new(33, 17), Pressure = 90 });
                Dispatch(graph, new Packet { Position = new(33, 17), Pressure = 90 });
                Equivalent(referenceOutputs[^1], outputs[^1]);
            }
            else
            {
                reference.Consume(new AuxPacket());
                Dispatch(graph, new AuxPacket());
                if (referenceOutputs.Count != 1 || outputs.Count != 1) throw new Exception("Bezier lost synchronous auxiliary passthrough.");
                Equivalent(referenceOutputs[0], outputs[0]);
                referenceOutputs.Clear(); outputs.Clear();
                // Constant zero trace removes wall-clock interpolation uncertainty.
                // Timing/jitter/nonzero interpolation remain distinct evidence.
                for (int attempt = 0; referenceOutputs.Count == 0 && attempt < 200; attempt++)
                { reference.Consume(new Packet()); timer.Fire(); }
                Dispatch(graph, new Packet());
                if (outputs.Count != 0) throw new Exception("Async corpus unexpectedly emitted synchronously.");
                int attempts = 0;
                while (outputs.Count == 0 && attempts++ < 200)
                {
                    Dispatch(graph, new Packet());
                    Due(instance);
                    if (graph.Tick(&Callback, 0) != 0) throw new Exception(graph.Error);
                }
                if (outputs.Count == 0 || referenceOutputs.Count == 0) throw new Exception("Async corpus never emitted.");
                Equivalent(referenceOutputs[^1], outputs[^1]);
                int before = outputs.Count;
                instance.Reset([0xff]);
                Due(instance);
                if (graph.Tick(&Callback, 0) != 0) throw new Exception(graph.Error);
                if (outputs.Count != before) throw new Exception("Bezier emitted stale tablet state after range loss.");

                // Invoke the reviewed SDK event method on a worker, with the
                // real Bezier class retaining its current state. This models
                // a foreign timer callback without creating an OS timer.
                var real = (AsyncPositionedPipelineElement<IDeviceReport>)Filter(instance);
                real.Scheduler!.Stop();
                var retained = new Packet();
                Dispatch(graph, retained);
                MethodInfo emit = typeof(AsyncPositionedPipelineElement<IDeviceReport>)
                    .GetMethod("OnEmit", BindingFlags.Instance | BindingFlags.NonPublic)!;
                Task.Run(() => emit.Invoke(real, null)).GetAwaiter().GetResult();
                retained.Position = new(999, 999); retained.Pressure = 999; retained.Raw[0] = 0x77;
                before = outputs.Count;
                if (graph.Tick(&Callback, 0) != 0) throw new Exception(graph.Error);
                if (outputs.Count != before + 1) throw new Exception("Queued Bezier emission was lost or duplicated.");
                Equivalent(Capture(new Packet()), outputs[^1]);
                // Queue one more callback immediately before retirement. The
                // graph must discard it and detach future foreign emissions.
                Task.Run(() => emit.Invoke(real, null)).GetAwaiter().GetResult();
                before = outputs.Count;
                graph.Dispose();
                Task.Run(() => emit.Invoke(real, null)).GetAwaiter().GetResult();
                if (outputs.Count != before) throw new Exception("Retired Bezier callback reached output.");
            }
            int retiredOutputs = outputs.Count;
            graph.Dispose(); graph.Dispose(); instance.Dispose(); instance.Dispose();
            try { graph.Tick(&Callback, 0); throw new Exception("Retired graph accepted a tick."); }
            catch (ObjectDisposedException) { }
            if (outputs.Count != retiredOutputs) throw new Exception("Retired corpus emitted output.");
            Console.WriteLine($"PASS corpus {config.Value<string>("type_name")} {name} settings/reference/reset/retire outputs={outputs.Count}");
        }
        finally { (reference as IDisposable)?.Dispose(); context.Unload(); handle.Free(); }
    }
    internal static void Run(string[] args)
    {
        if (args.Length < 5) throw new ArgumentException("corpus mode requires manifest and artifact directory arguments.");
        string root = Path.GetFullPath(args[4]);
        if (ServiceClient.Available) throw new InvalidOperationException("Corpus requires an isolated process without native services.");
        JObject manifest = JObject.Parse(File.ReadAllText(args[3]));
        int classes = 0, cases = 0;
        foreach (JObject item in (JArray)manifest["plugins"]!)
        {
            string archive = Path.GetFullPath(Path.Combine(root, item.Value<string>("archive")!));
            string dll = Path.GetFullPath(Path.Combine(root, item.Value<string>("dll")!));
            string prefix = root.TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
            if (!archive.StartsWith(prefix, StringComparison.OrdinalIgnoreCase) || !dll.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
                throw new InvalidDataException("Corpus artifacts must remain inside the supplied directory.");
            Pin(archive, item.Value<string>("archive_sha256")!); Pin(dll, item.Value<string>("dll_sha256")!);
            foreach (JObject entry in (JArray)item["classes"]!)
            {
                string type = entry.Value<string>("type_name")!;
                if (!Reviewed.Contains(type)) throw new InvalidDataException("Unreviewed corpus class: " + type);
                foreach (JObject scenario in (JArray)entry["cases"]!)
                {
                    var config = new JObject { ["assembly_path"] = dll, ["type_name"] = type,
                        ["tablet"] = JObject.Parse(File.ReadAllText(args[1])), ["settings"] = scenario["settings"]!.DeepClone() };
                    // Twice from clean instances catches plugin/graph state leaking between sessions.
                    for (int cycle = 0; cycle < 2; cycle++) RunCase(config, scenario.Value<string>("name")! + $"/cycle{cycle}", entry.Value<bool>("async"));
                    cases++;
                }
                classes++;
            }
        }
        Console.WriteLine($"PASS unchanged corpus classes={classes} settings_cases={cases} lifecycle_cycles={cases * 2}; no device/native output");
    }
}

using System.Numerics;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Runtime.Loader;
using Newtonsoft.Json.Linq;
using OpenTabletDriver.Plugin.DependencyInjection;
using OpenTabletDriver.Plugin.Devices;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Tablet;
using OpenTabletDriver.Plugin.Tablet.Touch;
using OpenTabletDriver.Plugin.Platform.Pointer;
using ITimer = OpenTabletDriver.Plugin.Timers.ITimer;
using OtdCompat;

// Authored offline regression fixtures; never inject OS input or open devices.
unsafe static class Upstream068Probe
{
    sealed class Packet : ITabletReport, IEraserReport, IRotationReport
    {
        public byte[] Raw { get; set; } = [1];
        public Vector2 Position { get; set; }
        public uint Pressure { get; set; } = 750;
        public bool[] PenButtons { get; set; } = [];
        public bool Eraser { get; set; }
        public int Rotation { get; set; } = int.MaxValue;
    }
    sealed class TouchPacket : ITouchReport
    {
        public byte[] Raw { get; set; } = [2];
        public TouchPoint[] Touches { get; set; } = [];
    }
    sealed class Timer : ITimer
    {
        public float Interval { get; set; }
        public bool Enabled { get; private set; }
        public event Action? Elapsed;
        public void Start() => Enabled = true;
        public void Stop() => Enabled = false;
        public void Dispose() => Stop();
        internal void Fire() => Elapsed?.Invoke();
    }
    sealed class Async : AsyncPositionedPipelineElement<IDeviceReport>
    {
        public override PipelinePosition Position => PipelinePosition.PreTransform;
        internal int Consumes;
        internal bool AcceptAll;
        protected override bool FilterState(IDeviceReport? value) => AcceptAll || base.FilterState(value);
        protected override void ConsumeState() => Consumes++;
        protected override void UpdateState() => OnEmit();
    }
    class ResolvedBase { [Resolved] public virtual ITimer? Timer { get; set; } }
    sealed class UnmarkedOverride : ResolvedBase { public override ITimer? Timer { get; set; } }
    sealed class MarkedOverride : ResolvedBase { [Resolved] public override ITimer? Timer { get; set; } }

    static int admissions, outputs;
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static int Callback(nint scope, uint operation, uint index, GraphReport* frame)
    {
        if (operation == 7 && index != 0)
        {
            if (frame->Pressure != 750 || frame->Eraser != 0) return -1;
            frame->Pressure = 500; admissions++;
        }
        if (operation is 3 or 4 or 5)
        {
            if (frame->Pressure != 500 || frame->Rotation != int.MaxValue) return -1;
            outputs++;
        }
        return 0;
    }
    static object Filter(Instance instance) => typeof(Instance).GetField("filter", BindingFlags.Instance | BindingFlags.NonPublic)!.GetValue(instance)!;
    static Instance Load(string[] args, string type) => new(new JObject {
        ["assembly_path"] = typeof(Upstream068Probe).Assembly.Location, ["type_name"] = "Upstream068Fixtures." + type,
        ["tablet"] = JObject.Parse(File.ReadAllText(args[1])), ["settings"] = new JObject()
    });
    static void VerifyLegacySdkSurface(string path)
    {
        var context = new AssemblyLoadContext("legacy-sdk-metadata", isCollectible: true);
        context.Resolving += (_, name) => AppDomain.CurrentDomain.GetAssemblies().FirstOrDefault(assembly => assembly.GetName().Name == name.Name);
        try
        {
            Assembly previous = context.LoadFromAssemblyPath(Path.GetFullPath(path));
            Assembly current = typeof(OpenTabletDriver.Plugin.Area).Assembly;
            const BindingFlags declared = BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance | BindingFlags.Static | BindingFlags.DeclaredOnly;
            static bool Exposed(MethodBase method) => method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly;
            var missing = new List<string>();
            int members = 0;
            foreach (Type oldType in previous.GetExportedTypes())
            {
                Type? newType = current.GetType(oldType.FullName!);
                if (newType == null) { missing.Add(oldType.FullName!); continue; }
                var available = newType.GetMethods(declared).Where(Exposed).Select(method => method.ToString())
                    .Concat(newType.GetConstructors(declared).Where(Exposed).Select(method => method.ToString())).ToHashSet();
                foreach (MethodBase member in oldType.GetMethods(declared).Where(Exposed).Cast<MethodBase>().Concat(oldType.GetConstructors(declared).Where(Exposed)))
                {
                    members++;
                    if (!available.Contains(member.ToString())) missing.Add(oldType.FullName + ": " + member);
                }
                foreach (FieldInfo field in oldType.GetFields(declared).Where(field => field.IsPublic || field.IsFamily || field.IsFamilyOrAssembly))
                {
                    members++;
                    FieldInfo? replacement = newType.GetField(field.Name, declared);
                    if (replacement == null || replacement.FieldType.ToString() != field.FieldType.ToString()
                        || (field.IsLiteral && !Equals(field.GetRawConstantValue(), replacement.GetRawConstantValue())))
                        missing.Add(oldType.FullName + ": " + field);
                }
            }
            if (missing.Count != 0) throw new Exception("Removed legacy SDK members: " + string.Join("; ", missing));
            Console.WriteLine($"PASS preserved legacy SDK public/protected surface ({members} members)");
        }
        finally { context.Unload(); }
    }
    public static void Run(string[] args)
    {
        if (args.Length > 3) VerifyLegacySdkSurface(args[3]);
        var legacyContext = new PluginContext(args[0]);
        try
        {
            var assembly = legacyContext.LoadFromAssemblyPath(Path.GetFullPath(args[0]));
            Type construction = assembly.GetType("SettingsFixture.LegacySdkConstruction", true)!;
            var legacyArea = (OpenTabletDriver.Plugin.Area)construction.GetMethod("CreateArea")!.Invoke(null, [])!;
            var legacyProperty = (OpenTabletDriver.Plugin.Attributes.PropertyAttribute)construction.GetMethod("RenameProperty")!.Invoke(null, [])!;
            var legacyConfiguration = (TabletConfiguration)construction.GetMethod("CreateConfiguration")!.Invoke(null, [])!;
            if (legacyArea.Width != 0 || legacyArea.Height != 0 || legacyArea.Position != Vector2.Zero || legacyArea.Rotation != 0
                || legacyProperty.DisplayName != "updated" || legacyConfiguration.Specifications == null || legacyConfiguration.DigitizerIdentifiers == null)
                throw new Exception("Legacy SDK constructors/setters lost their original behavior.");
            using var legacyOutput = (IOutputMode)Activator.CreateInstance(assembly.GetType("SettingsFixture.LegacyDirectOutput", true)!)!;
            legacyOutput.DisableRotation = false;
            if (legacyOutput.DisableRotation) throw new Exception("Legacy output fabricated a rotation setting.");
            try { legacyOutput.DisableRotation = true; throw new Exception("Unsupported legacy rotation suppression was accepted."); }
            catch (NotSupportedException) { }
            var legacyEndpoint = (IDeviceEndpoint)Activator.CreateInstance(assembly.GetType("SettingsFixture.LegacyEndpoint", true)!)!;
            try { legacyEndpoint.OpenAndSendFeatureInit([1]); throw new Exception("Unsupported legacy initialization was accepted."); }
            catch (NotSupportedException) { }
            try { legacyEndpoint.OpenAndSendOutputInit([1]); throw new Exception("Unsupported legacy output initialization was accepted."); }
            catch (NotSupportedException) { }
        }
        finally { legacyContext.Unload(); }
        using var timer = new Timer();
        using var async = new Async { Frequency = 0 };
        async.Scheduler = timer;
        if (async.Frequency != 1 || timer.Interval != 1000) throw new Exception("SDK frequency clamp changed.");
        int emitted = 0; async.Emit += _ => emitted++;
        var touch = new TouchPacket(); async.Consume(touch);
        if (emitted != 1 || async.Consumes != 0) throw new Exception("Excluded touch reports must bypass interpolation state.");
        async.AcceptAll = true; async.Consume(touch);
        if (async.Consumes != 1) throw new Exception("FilterState override was ignored.");
        timer.Fire();
        using var services = new HostServices(() => new Timer());
        var unmarked = new UnmarkedOverride(); services.Inject(unmarked.GetType(), unmarked);
        if (unmarked.Timer != null) throw new Exception("Unmarked override inherited dependency injection.");
        var marked = new MarkedOverride(); services.Inject(marked.GetType(), marked);
        if (marked.Timer == null) throw new Exception("Explicit override dependency was not injected.");

        using var internalFilter = Load(args, "InternalDuplicateFilter");
        using var retaining = Load(args, "RetainedPressureFilter");
        var first = GCHandle.Alloc(internalFilter); var second = GCHandle.Alloc(retaining);
        try
        {
            using var graph = new SynchronousGraph([
                new GraphNode { Context = GCHandle.ToIntPtr(first), Index = 0, Stage = 4 },
                new GraphNode { Context = GCHandle.ToIntPtr(second), Index = 1, Stage = 1 }
            ]);
            var packet = new Packet();
            var frame = new GraphReport { Version = 2, Size = (uint)sizeof(GraphReport) };
            if (graph.Dispatch(&frame, &Callback, 0, originalReport: packet) != 0) throw new Exception(graph.Error);
            if (admissions != 1 || outputs != 2 || packet.Pressure != 500
                || (uint)Filter(internalFilter).GetType().GetProperty("SeenPressure")!.GetValue(Filter(internalFilter))! != 750)
                throw new Exception("Internal ordering or one-time pressure admission changed.");
            Filter(retaining).GetType().GetMethod("EmitRetained")!.Invoke(Filter(retaining), []);
            if (graph.Tick(&Callback, 0) != 0 || admissions != 1 || outputs != 3 || packet.Pressure != 500)
                throw new Exception("A retained asynchronous report was rewritten twice.");
            Filter(internalFilter).GetType().GetMethod("EmitLate")!.Invoke(Filter(internalFilter), [packet]);
            if (graph.Tick(&Callback, 0) != 0 || admissions != 1 || outputs != 4 || packet.Pressure != 500)
                throw new Exception("Late Internal emissions must retain prepared pressure across their owned snapshot.");
        }
        finally { second.Free(); first.Free(); }
        using (var output = new OutputInstance(new JObject {
            ["assembly_path"] = typeof(Upstream068Probe).Assembly.Location, ["type_name"] = "Upstream068Fixtures.PressureOutput",
            ["tablet"] = JObject.Parse(File.ReadAllText(args[1])), ["settings"] = new JObject()
        }))
        {
            var handle = GCHandle.Alloc(retaining);
            try
            {
                using var graph = new SynchronousGraph([new GraphNode { Context = GCHandle.ToIntPtr(handle), Index = 0, Stage = 1 }]);
                graph.AttachOutput(output); admissions = 0; outputs = 0;
                var packet = new Packet(); var frame = new GraphReport { Version = 2, Size = (uint)sizeof(GraphReport) };
                if (graph.Dispatch(&frame, &Callback, 0, originalReport: packet) != 0) throw new Exception(graph.Error);
                Filter(retaining).GetType().GetMethod("EmitRetained")!.Invoke(Filter(retaining), []);
                if (graph.Tick(&Callback, 0) != 0 || admissions != 1 || outputs != 2 || packet.Pressure != 500)
                    throw new Exception("Managed output rebound or remapped retained pressure.");
                object actual = typeof(EndpointInstance).GetProperty("Value", BindingFlags.Instance | BindingFlags.NonPublic)!.GetValue(output)!;
                if ((uint)actual.GetType().GetProperty("LastPressure")!.GetValue(actual)! != 500)
                    throw new Exception("Managed output did not receive normalized retained pressure.");
            }
            finally { handle.Free(); }
        }
        using (var queue = new CommandQueue())
        {
            var mouse = new SessionPointer(queue, false, false);
            if (mouse.Resolve(typeof(IRotationHandler)) != null || (object)mouse is IRotationHandler)
                throw new Exception("A mouse provider fabricated rotation support.");
            var rotating = new RotatingSessionPointer(queue, false, true);
            ((IRotationHandler)rotating).SetRotation(0.5f); rotating.Flush();
            if (!queue.Take(out var command) || command.Rotation != 0.5f || (command.Flags & 128) == 0)
                throw new Exception("Normalized native rotation service command changed.");
        }
        using var internalAsync = Load(args, "AsyncInternalFilter");
        using var ordinaryAsync = Load(args, "OrdinaryAsyncFilter"); // SDK timer injection bootstrap must be finite.
        Console.WriteLine("PASS 0.6.8 admission, SDK async, injection and rotation fixtures");
    }
}

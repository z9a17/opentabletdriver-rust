using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using Newtonsoft.Json.Linq;
using OpenTabletDriver.Plugin.Tablet;
using OtdCompat;

// Offline only: callbacks count signals and never submit OS input.
unsafe static class WakeChecks
{
    sealed class Signal : IDisposable
    {
        internal int Count, TimedOut;
        internal bool Block;
        internal readonly ManualResetEventSlim Entered = new(), Release = new();
        public void Dispose() { Entered.Dispose(); Release.Dispose(); }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static void Wake(nint scope)
    {
        var signal = (Signal)GCHandle.FromIntPtr(scope).Target!;
        Interlocked.Increment(ref signal.Count);
        if (!signal.Block) return;
        signal.Entered.Set();
        if (!signal.Release.Wait(TimeSpan.FromSeconds(5))) Interlocked.Exchange(ref signal.TimedOut, 1);
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static int Output(nint scope, uint operation, uint index, GraphReport* frame) => 0;
    sealed class Packet : IDeviceReport { public byte[] Raw { get; set; } = [1]; }
    sealed class ProviderHolder
    {
        [OpenTabletDriver.Plugin.DependencyInjection.Resolved]
        public IServiceProvider Services { get; set; } = null!;
    }
    static void Require(bool condition, string message) { if (!condition) throw new Exception(message); }
    static void QueueChecks()
    {
        using var queue = new CommandQueue();
        using var signal = new Signal();
        var handle = GCHandle.Alloc(signal);
        try
        {
            queue.Add(default);
            queue.SetWake(&Wake, GCHandle.ToIntPtr(handle));
            Require(signal.Count == 1, "A ready queue was stranded by wake installation.");
            Require(queue.Take(out _), "Installation consumed a command.");
            signal.Block = true;
            var producer = Task.Run(() => queue.Add(default));
            Require(signal.Entered.Wait(TimeSpan.FromSeconds(5)), "Producer did not enter wake callback.");
            using var detaching = new ManualResetEventSlim();
            var detach = Task.Run(() => { detaching.Set(); queue.SetWake(null, 0); });
            Require(detaching.Wait(TimeSpan.FromSeconds(5)), "Detach task did not start.");
            bool detachedEarly = detach.Wait(50);
            signal.Release.Set();
            Require(Task.WaitAll([producer, detach], TimeSpan.FromSeconds(5)), "Wake detach did not finish.");
            Require(!detachedEarly && signal.TimedOut == 0, "Detach did not synchronize with the active native scope.");
            int detachedCount = signal.Count;
            handle.Free();
            queue.Add(default);
            Require(signal.Count == detachedCount, "A detached queue called its retired scope.");
        }
        finally { signal.Release.Set(); queue.SetWake(null, 0); if (handle.IsAllocated) handle.Free(); }
        using var full = new CommandQueue();
        for (int i = 0; i < 256; i++) full.Add(default);
        try { full.Add(default); throw new Exception("Queue overflow was not reported."); }
        catch (InvalidOperationException) { }
        using var failedSignal = new Signal();
        var failedHandle = GCHandle.Alloc(failedSignal);
        try
        {
            full.SetWake(&Wake, GCHandle.ToIntPtr(failedHandle));
            Require(failedSignal.Count == 1, "Failure pending before installation did not wake.");
            int before = failedSignal.Count;
            Task.Run(() => { try { full.Add(default); } catch (InvalidOperationException) { } }).GetAwaiter().GetResult();
            Require(failedSignal.Count == before, "An already reported failure was signaled repeatedly.");
            full.SetWake(null, 0);
        }
        finally { failedHandle.Free(); }
    }
    static void EndpointChecks(JObject config)
    {
        config = (JObject)config.DeepClone();
        config["type_name"] = "SettingsFixture.EndpointBinding";
        config["owner"] = 7; config["keys"] = new JObject { ["A"] = 4 };
        using var endpoint = new BindingInstance(config);
        using var signal = new Signal();
        var handle = GCHandle.Alloc(signal);
        try
        {
            Require(endpoint.NextTickMicros() == 5000, "Legacy endpoint fallback changed.");
            endpoint.Queue.SetWake(&Wake, GCHandle.ToIntPtr(handle));
            Require(endpoint.NextTickMicros() == -1, "Queue-only endpoint retained idle polling.");
            endpoint.Queue.Add(default);
            Require(signal.Count == 0 && endpoint.NextTickMicros() == 0, "Owner-thread command signaled unnecessarily or lost readiness.");
            endpoint.Queue.Take(out _);
            Task.Run(() => endpoint.Queue.Add(default)).GetAwaiter().GetResult();
            Require(signal.Count == 1 && endpoint.NextTickMicros() == 0, "Foreign command did not wake its endpoint.");
            endpoint.Queue.Take(out _);
            var services = (HostServices)typeof(EndpointInstance).GetField("services", BindingFlags.Instance | BindingFlags.NonPublic)!.GetValue(endpoint)!;
            var provider = new ProviderHolder(); services.Inject(typeof(ProviderHolder), provider);
            Require(services.ProviderInjected && endpoint.NextTickMicros() == 5000, "An injected provider lost fallback before requesting any timer.");
            var timer = (SessionTimer)provider.Services.GetService(typeof(OpenTabletDriver.Plugin.Timers.ITimer))!;
            Require(endpoint.NextTickMicros() == 5000, "Stopped endpoint timer lost its change fallback.");
            Task.Run(() => { timer.Interval = 1; timer.Start(); }).GetAwaiter().GetResult();
            Require(endpoint.NextTickMicros() is >= 0 and <= 5000, "Foreign timer Start lost its deadline.");
            Task.Run(timer.Stop).GetAwaiter().GetResult();
            Require(endpoint.NextTickMicros() == 5000, "Foreign timer Stop lost idle fallback.");
            endpoint.Queue.SetWake(null, 0);
        }
        finally { endpoint.Queue.SetWake(null, 0); handle.Free(); }
    }
    internal static void Run(string[] args)
    {
        QueueChecks();
        var config = new JObject { ["assembly_path"] = args[0], ["type_name"] = "SettingsFixture.BackgroundFixtureFilter",
            ["tablet"] = JObject.Parse(File.ReadAllText(args[1])), ["settings"] = new JObject() };
        EndpointChecks(config);
        using var instance = new Instance(config);
        object filter = typeof(Instance).GetField("filter", BindingFlags.Instance | BindingFlags.NonPublic)!.GetValue(instance)!;
        MethodInfo emit = filter.GetType().GetMethod("EmitBackground")!;
        void Send(IDeviceReport[] packets) => emit.Invoke(filter, [packets]);
        var instanceHandle = GCHandle.Alloc(instance);
        using var signal = new Signal();
        var signalHandle = GCHandle.Alloc(signal);
        try
        {
            using var graph = new SynchronousGraph([new GraphNode { Context = GCHandle.ToIntPtr(instanceHandle), Index = 0, Stage = 1 }]);
            Require(graph.NextTickMicros() is > 0 and <= 10_000, "Legacy graph fallback changed.");
            Send([new Packet()]);
            graph.SetWake(&Wake, GCHandle.ToIntPtr(signalHandle));
            Require(signal.Count == 1 && graph.NextTickMicros() == 0, "Ready graph was not signaled on installation.");
            Require(graph.Tick(&Output, 0) == 0 && graph.NextTickMicros() == -1, "Queue-only graph retained idle polling or failed to drain.");
            // A failed snapshot creates only an error, never a ready report.
            Send([new Packet { Raw = new byte[OwnedReportSnapshot.MaxBytes + 1] }]);
            Require(signal.Count > 1 && graph.NextTickMicros() == 0, "Error-only graph failure did not wake.");
            Require(graph.Tick(&Output, 0) != 0, "Snapshot failure was lost.");
            graph.SetWake(null, 0);
            int count = signal.Count;
            signalHandle.Free();
            graph.Dispose();
            Send([new Packet()]);
            Require(signal.Count == count, "Retired graph signaled a freed native scope.");
        }
        finally { if (signalHandle.IsAllocated) signalHandle.Free(); instanceHandle.Free(); }
        config["type_name"] = "SettingsFixture.LateTimerFilter";
        using var timed = new Instance(config);
        var timedHandle = GCHandle.Alloc(timed);
        var timerSignal = GCHandle.Alloc(signal);
        try
        {
            using var graph = new SynchronousGraph([new GraphNode { Context = GCHandle.ToIntPtr(timedHandle), Stage = 1 }]);
            graph.SetWake(&Wake, GCHandle.ToIntPtr(timerSignal));
            Require(graph.NextTickMicros() is > 0 and <= 10_000, "Disabled timers lost their bounded deadline-change fallback.");
            object timedFilter = typeof(Instance).GetField("filter", BindingFlags.Instance | BindingFlags.NonPublic)!.GetValue(timed)!;
            var timer = (OpenTabletDriver.Plugin.Timers.ITimer)timedFilter.GetType().GetProperty("Scheduler")!.GetValue(timedFilter)!;
            Task.Run(() => { timer.Interval = 1; timer.Start(); }).GetAwaiter().GetResult();
            Require(graph.NextTickMicros() is >= 0 and <= 10_000, "Foreign filter timer Start lost its deadline.");
            Task.Run(timer.Stop).GetAwaiter().GetResult();
            graph.SetWake(null, 0);
        }
        finally { timerSignal.Free(); timedHandle.Free(); }
        Console.WriteLine("PASS managed wake installation, FIFO drain, errors, synchronized detach, retirement and timer fallback; no OS input");
    }
}

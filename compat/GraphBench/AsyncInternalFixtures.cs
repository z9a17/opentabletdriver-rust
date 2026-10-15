using System.Numerics;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using Newtonsoft.Json.Linq;
using OpenTabletDriver.Plugin.Tablet;
using ITimer = OpenTabletDriver.Plugin.Timers.ITimer;
using OtdCompat;

// Synthetic reports, injected session timers and discarded native continuations.
// No devices, saved settings, driver process or OS output are used by this mode.
unsafe static class AsyncInternalProbe
{
    sealed class Packet : ITabletReport
    {
        public byte[] Raw { get; set; } = [1, 2];
        public Vector2 Position { get; set; }
        uint pressure = 750;
        public uint Pressure
        {
            get => pressure;
            set
            {
                pressure = value;
                if (blockPressureWrite && value == 500)
                {
                    pressureWritten.Set();
                    if (!releasePressure.Wait(TimeSpan.FromSeconds(5))) Interlocked.Exchange(ref setterTimeout, 1);
                }
            }
        }
        public bool[] PenButtons { get; set; } = [true];
        internal uint[][] Extra = [[42]];
    }
    sealed class UnownablePacket : IDeviceReport
    {
        public byte[] Raw { get; set; } = [1];
        internal nint Address = 1;
    }
    static readonly ManualResetEventSlim pressureWritten = new(), releasePressure = new();
    static volatile bool blockPressureWrite;
    static int setterTimeout;
    static readonly int owner = Environment.CurrentManagedThreadId;
    static readonly List<bool> preparations = [];
    static readonly List<uint> values = [];
    static readonly List<IDeviceReport> identities = [];
    static int suppress, wakes;
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static void Wake(nint scope) => Interlocked.Increment(ref wakes);
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static int Callback(nint scope, uint operation, uint index, GraphReport* frame)
    {
        if (Environment.CurrentManagedThreadId != owner) return -1;
        if (operation == 7)
        {
            preparations.Add(index != 0);
            if (index != 0) frame->Pressure = frame->Pressure <= 500 ? 0 : (frame->Pressure - 500) * 2;
            if (suppress > 0) { suppress--; return 1; }
        }
        if (operation is 3 or 4 or 5)
        {
            values.Add(frame->Pressure); identities.Add(SynchronousGraph.CurrentReport!);
        }
        return 0;
    }
    static object Plugin(Instance instance) => typeof(Instance).GetField("filter", BindingFlags.NonPublic | BindingFlags.Instance)!.GetValue(instance)!;
    static JObject Config(string[] args, string type, int copies = 1) => new() {
        ["assembly_path"] = typeof(AsyncInternalProbe).Assembly.Location,
        ["type_name"] = "AsyncInternalFixtures." + type,
        ["tablet"] = JObject.Parse(File.ReadAllText(args[1])),
        ["settings"] = new JObject { ["Copies"] = copies }
    };
    static void Check(bool result, string message) { if (!result) throw new Exception(message); }
    static void Clear() { preparations.Clear(); values.Clear(); identities.Clear(); suppress = 0; }
    static void Dispatch(SynchronousGraph graph, IDeviceReport report)
    {
        var frame = new GraphReport { Version = 2, Size = (uint)sizeof(GraphReport) };
        Check(graph.Dispatch(&frame, &Callback, 0, originalReport: report) == 0, graph.Error ?? "Dispatch failed.");
    }
    static void Tick(SynchronousGraph graph) => Check(graph.Tick(&Callback, 0) == 0, graph.Error ?? "Tick failed.");
    static void Send(Instance instance, params IDeviceReport[] reports)
    {
        object plugin = Plugin(instance);
        Exception? failure = null;
        var producer = new Thread(() => {
            try { plugin.GetType().GetMethod("Send")!.Invoke(plugin, [reports]); }
            catch (Exception error) { failure = error; }
        });
        producer.Start(); Check(producer.Join(TimeSpan.FromSeconds(10)), "Foreign producer blocked.");
        if (failure != null) throw failure;
    }
    static void OwnershipAndAdmission(string[] args)
    {
        using var filter = new Instance(Config(args, "InternalFilter"));
        var handle = GCHandle.Alloc(filter);
        try
        {
            using var graph = new SynchronousGraph([new GraphNode { Context = GCHandle.ToIntPtr(handle), Index = 3, Stage = 4 }]);
            graph.SetWake(&Wake, 0); Clear();
            var original = new Packet(); Dispatch(graph, original);
            Check(values.SequenceEqual(new uint[] { 500 }) && ReferenceEquals(identities[0], original), "Internal source identity or early pressure changed.");
            Send(filter, original, original);
            Check(values.Count == 1 && wakes != 0 && graph.NextTickMicros() == 0, "Foreign Internal emission escaped the owner thread or failed to wake.");
            original.Raw[0] = 9; original.PenButtons[0] = false; original.Extra[0][0] = 9;
            Tick(graph);
            Check(preparations.SequenceEqual(new[] { true, false, false }) && values.All(value => value == 500), "Prepared Internal snapshots were rewritten twice.");
            var owned = (Packet)identities[1];
            Check(!ReferenceEquals(owned, original) && owned.Raw[0] == 1 && owned.PenButtons[0] && owned.Extra[0][0] == 42,
                "Internal continuation lost concrete type or nested ownership.");
            var raw = new Packet(); Send(filter, raw, raw); Tick(graph);
            Check(raw.Pressure == 750 && preparations.TakeLast(2).All(first => first) && values.TakeLast(2).All(value => value == 500),
                "Independent raw snapshots must each be prepared once without mutating the producer's state.");
            Send(filter, new Packet { Pressure = 600 }, new Packet { Pressure = 900 }); Tick(graph);
            Check(values.TakeLast(2).SequenceEqual(new uint[] { 200, 800 }), "Foreign Internal snapshot FIFO order changed.");
            // A suppressed built-in result has already changed pressure, too.
            Clear(); suppress = 1; var suppressed = new Packet(); Dispatch(graph, suppressed);
            Check(values.Count == 0 && suppressed.Pressure == 500, "Suppression did not retain rewritten source pressure.");
            Send(filter, suppressed); Tick(graph);
            Check(preparations.SequenceEqual(new[] { true, false }) && values.SequenceEqual(new uint[] { 500 }), "Suppressed admission rewrote retained pressure on replay.");
        }
        finally { handle.Free(); }
    }
    static void MultipleNodesAndReset(string[] args)
    {
        using var first = new Instance(Config(args, "InternalFilter", 2));
        using var second = new Instance(Config(args, "InternalFilter", 2));
        var h1 = GCHandle.Alloc(first); var h2 = GCHandle.Alloc(second);
        try
        {
            using var graph = new SynchronousGraph([
                new GraphNode { Context = GCHandle.ToIntPtr(h1), Index = 0, Stage = 4 },
                new GraphNode { Context = GCHandle.ToIntPtr(h2), Index = 1, Stage = 4 }
            ]);
            Clear(); var source = new Packet(); Dispatch(graph, source);
            Check(preparations.SequenceEqual(new[] { true, false, false, false }) && values.Count == 4, "Multiple Internal nodes changed immediate duplicate semantics.");
            Send(first, source); Tick(graph);
            Check(preparations.TakeLast(2).All(prepared => !prepared) && values.All(value => value == 500), "Queued Internal continuation did not resume downstream of its node.");
            Send(first, source); first.Reset([]); second.Reset([]); Clear();
            Send(first, source); Send(second, source); Tick(graph);
            Check(values.Count == 0 && preparations.Count == 0, "Reset allowed a queued or retained old report to restore contact.");
            source.Pressure = 750; Dispatch(graph, source);
            Check(preparations.SequenceEqual(new[] { true, false, false, false }) && values.Count == 4, "Reused parser source did not receive a fresh admission after reset.");
        }
        finally { h2.Free(); h1.Free(); }
        using var silent = new Instance(Config(args, "InternalFilter", 0));
        var silentHandle = GCHandle.Alloc(silent);
        try
        {
            using var graph = new SynchronousGraph([new GraphNode { Context = GCHandle.ToIntPtr(silentHandle), Stage = 4 }]);
            Clear(); var raw = new Packet(); Dispatch(graph, raw); silent.Reset([]); Send(silent, raw); Tick(graph);
            Check(values.Count == 0 && raw.Pressure == 750, "A never-emitted but known pre-reset source escaped epoch retirement.");
        }
        finally { silentHandle.Free(); }
    }
    static void SdkTimer(string[] args)
    {
        using var filter = new Instance(Config(args, "TimedInternalFilter"));
        var handle = GCHandle.Alloc(filter);
        try
        {
            using var graph = new SynchronousGraph([new GraphNode { Context = GCHandle.ToIntPtr(handle), Stage = 4 }]);
            object plugin = Plugin(filter);
            var timer = (ITimer)plugin.GetType().GetProperty("Scheduler")!.GetValue(plugin)!;
            timer.Stop(); Clear(); var packet = new Packet(); Dispatch(graph, packet);
            Check(values.Count == 0 && packet.Pressure == 750, "SDK Internal state must observe the original pressure before admission.");
            void Pulse()
            {
                timer.Interval = 0.05f; timer.Start();
                Check(SpinWait.SpinUntil(() => graph.NextTickMicros() == 0, TimeSpan.FromSeconds(5)), "Internal timer deadline was lost.");
                Tick(graph);
            }
            Pulse(); Pulse();
            Check(preparations.SequenceEqual(new[] { true, false, false, false }) && values.SequenceEqual(new uint[] { 500, 500, 500, 500 }),
                "Repeated SDK Internal timers progressively rewrote retained pressure.");
            filter.Reset([]); Clear(); Pulse();
            Check(values.Count == 0, "SDK Internal reset emitted stale contact.");
        }
        finally { handle.Free(); }
    }
    static void ManagedMode(string[] args)
    {
        using var filter = new Instance(Config(args, "InternalFilter"));
        var config = Config(args, "unused"); config["type_name"] = "Upstream068Fixtures.ReplayPressureOutput";
        using var mode = new OutputInstance(config);
        var handle = GCHandle.Alloc(filter);
        try
        {
            using var graph = new SynchronousGraph([new GraphNode { Context = GCHandle.ToIntPtr(handle), Stage = 4 }]);
            graph.AttachOutput(mode); Clear(); var original = new Packet(); Dispatch(graph, original);
            Check(preparations.SequenceEqual(new[] { true, false }) && values.Count == 2, "Managed mode Internal ordering changed.");
            mode.Mode.GetType().GetMethod("ReplayBackground")!.Invoke(mode.Mode, []);
            Check(values.Count == 2 && graph.NextTickMicros() == 0, "Custom mode background entry ran native code.");
            Tick(graph);
            Check(preparations.SequenceEqual(new[] { true, false, false }) && values.All(value => value == 500)
                && !ReferenceEquals(identities[2], original), "Mode background entry through Internal lost snapshot admission.");
            filter.Reset([]); Clear(); mode.Mode.GetType().GetMethod("ReplayBackground")!.Invoke(mode.Mode, []); Tick(graph);
            Check(values.Count == 0, "Custom mode restored pre-reset retained contact.");
        }
        finally { handle.Free(); }
    }
    static void CaptureAdmissionRace(string[] args)
    {
        using var filter = new Instance(Config(args, "InternalFilter"));
        var handle = GCHandle.Alloc(filter);
        try
        {
            using var graph = new SynchronousGraph([new GraphNode { Context = GCHandle.ToIntPtr(handle), Stage = 4 }]);
            Clear(); pressureWritten.Reset(); releasePressure.Reset(); setterTimeout = 0;
            var packet = new Packet(); Exception? failure = null;
            var producer = new Thread(() => {
                try
                {
                    Check(pressureWritten.Wait(TimeSpan.FromSeconds(5)), "Pressure setter was never reached.");
                    Plugin(filter).GetType().GetMethod("Send")!.Invoke(Plugin(filter), [new IDeviceReport[] { packet }]);
                }
                catch (Exception error) { failure = error; }
            });
            FieldInfo inFlight = typeof(SynchronousGraph).GetField("inFlightBytes", BindingFlags.NonPublic | BindingFlags.Instance)!;
            var releaser = new Thread(() => {
                bool reserved = SpinWait.SpinUntil(() => (int)inFlight.GetValue(graph)! != 0, TimeSpan.FromSeconds(5));
                if (!reserved) Interlocked.Exchange(ref setterTimeout, 1);
                releasePressure.Set();
            });
            producer.Start(); releaser.Start(); blockPressureWrite = true;
            try { Dispatch(graph, packet); }
            finally { blockPressureWrite = false; releasePressure.Set(); }
            Check(producer.Join(TimeSpan.FromSeconds(5)) && releaser.Join(TimeSpan.FromSeconds(5)), "Admission race workers did not retire.");
            if (failure != null) throw failure;
            Tick(graph);
            Check(setterTimeout == 0 && preparations.SequenceEqual(new[] { true, false }) && values.SequenceEqual(new uint[] { 500, 500 }),
                "Concurrent capture observed rewritten pressure with an unprepared admission identity.");
        }
        finally { blockPressureWrite = false; releasePressure.Set(); handle.Free(); }
    }
    static void FailuresAndRetirement(string[] args)
    {
        using var filter = new Instance(Config(args, "InternalFilter"));
        var handle = GCHandle.Alloc(filter);
        try
        {
            using var graph = new SynchronousGraph([new GraphNode { Context = GCHandle.ToIntPtr(handle), Index = 5, Stage = 4 }]);
            graph.SetWake(&Wake, 0); Clear();
            Send(filter, Enumerable.Range(0, 65).Select(_ => (IDeviceReport)new Packet()).ToArray());
            Check(graph.Tick(&Callback, 0) != 0 && graph.FailedIndex == 5 && graph.Error?.Contains("overflow") == true && values.Count == 0,
                "Internal queue overflow did not discard the whole pending prefix and identify its emitter.");
            graph.Dispose(); int signals = wakes; Send(filter, new Packet());
            Check(wakes == signals && values.Count == 0, "A retired graph signaled or delivered an Internal emission.");
        }
        finally { handle.Free(); }
        using var invalid = new Instance(Config(args, "InternalFilter"));
        var invalidHandle = GCHandle.Alloc(invalid);
        try
        {
            using var graph = new SynchronousGraph([new GraphNode { Context = GCHandle.ToIntPtr(invalidHandle), Index = 6, Stage = 4 }]);
            Send(invalid, new UnownablePacket());
            Check(graph.Tick(&Callback, 0) != 0 && graph.FailedIndex == 6 && graph.Error?.Contains("unmanaged address") == true,
                "Internal support silently accepted an unownable concrete report.");
        }
        finally { invalidHandle.Free(); }
    }
    static void InFlightReset(string[] args)
    {
        using var filter = new Instance(Config(args, "InternalFilter"));
        var handle = GCHandle.Alloc(filter);
        try
        {
            using var graph = new SynchronousGraph([new GraphNode { Context = GCHandle.ToIntPtr(handle), Stage = 4 }]);
            Clear(); var old = new Packet(); Dispatch(graph, old); Clear();
            object gate = typeof(SynchronousGraph).GetField("admissionGate", BindingFlags.NonPublic | BindingFlags.Instance)!.GetValue(graph)!;
            FieldInfo inFlight = typeof(SynchronousGraph).GetField("inFlightBytes", BindingFlags.NonPublic | BindingFlags.Instance)!;
            Exception? failure = null;
            var producer = new Thread(() => {
                try { Plugin(filter).GetType().GetMethod("Send")!.Invoke(Plugin(filter), [new IDeviceReport[] { old }]); }
                catch (Exception error) { failure = error; }
            });
            lock (gate)
            {
                producer.Start();
                Check(SpinWait.SpinUntil(() => (int)inFlight.GetValue(graph)! != 0, TimeSpan.FromSeconds(5)), "Reset probe did not reserve an in-flight snapshot.");
                filter.Reset([]); // Invalidates the reservation before its copy can start.
            }
            Check(producer.Join(TimeSpan.FromSeconds(5)), "Reset did not release the in-flight snapshot.");
            if (failure != null) throw failure;
            // The stale head completes as an empty ready slot, and must not
            // strand or reorder the fresh report that follows it.
            Send(filter, old, new Packet()); Tick(graph);
            Check(preparations.SequenceEqual(new[] { true }) && values.SequenceEqual(new uint[] { 500 })
                && (int)inFlight.GetValue(graph)! == 0 && graph.NextTickMicros() != 0,
                "Reset leaked a snapshot reservation, restored stale pressure, or stranded FIFO work.");
            graph.SetWake(&Wake, 0); int signals = wakes; failure = null;
            var retiring = new Thread(() => {
                try { Plugin(filter).GetType().GetMethod("Send")!.Invoke(Plugin(filter), [new IDeviceReport[] { new Packet() }]); }
                catch (Exception error) { failure = error; }
            });
            lock (gate)
            {
                retiring.Start();
                Check(SpinWait.SpinUntil(() => (int)inFlight.GetValue(graph)! != 0, TimeSpan.FromSeconds(5)), "Retirement probe did not reserve a snapshot.");
                graph.Dispose();
            }
            Check(retiring.Join(TimeSpan.FromSeconds(5)), "Retired in-flight capture did not complete.");
            if (failure != null) throw failure;
            Check((int)inFlight.GetValue(graph)! == 0 && wakes == signals && values.Count == 1,
                "Retired capture leaked its budget or signaled/delivered through a detached scope.");
        }
        finally { handle.Free(); }
    }
    internal static void Run(string[] args)
    {
        OwnershipAndAdmission(args); MultipleNodesAndReset(args); SdkTimer(args); ManagedMode(args);
        CaptureAdmissionRace(args); InFlightReset(args); FailuresAndRetirement(args);
        Console.WriteLine("PASS async Internal ownership, pressure identity, SDK timers, multiple nodes, mode entry, capture race, reset, overflow and retirement; no OS input");
    }
}

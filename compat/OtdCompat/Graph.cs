using System.Diagnostics;
using System.Numerics;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using OpenTabletDriver.Plugin.Tablet;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Tablet.Touch;
using OpenTabletDriver.Plugin.Tablet.Wheel;

namespace OtdCompat;

// Managed bridge v2 projection, separate from the unchanged native FilterApi.
// The report itself stays in .NET throughout the graph, retaining its concrete
// type and identity. This temporary projection exists only during a native call.
[StructLayout(LayoutKind.Sequential)]
public unsafe struct GraphReport
{
    public uint Version, Size;
    public byte* Raw;
    public uint RawLength, Kind, Flags, Reserved;
    public ulong Serial, PenBits, AuxBits, MouseBits;
    public float X, Y, TiltX, TiltY, ScrollX, ScrollY;
    public uint Pressure, Eraser, Near, Distance, ToolID, ToolType;
    public uint PenCount, AuxCount, MouseCount, TipSwitch;
    public uint AbsoluteCount, AbsolutePresent, RelativeCount, WheelCount, TouchCount, TouchPresent;
    public fixed uint AbsoluteValues[16];
    public fixed int RelativeValues[16];
    public fixed ulong WheelBits[8];
    public fixed uint WheelCounts[8];
    public fixed uint TouchIDs[32];
    public fixed float TouchXY[64];
}

[StructLayout(LayoutKind.Sequential)]
public struct GraphNode { public nint Context; public uint Index, Stage; }

sealed class DeviceSnapshot : IDeviceReport { public byte[] Raw { get; set; } = []; }
class AuxSnapshot : IAuxReport
{
    public byte[] Raw { get; set; } = [];
    public bool[] AuxButtons { get; set; } = [];
}
sealed class RingSnapshot : AuxSnapshot, IAbsoluteWheelReport, IWheelButtonReport
{
    public uint?[] AnalogPositions { get; set; } = [];
    public bool[][] WheelButtons { get; set; } = [];
}
sealed class TouchSnapshot : ITouchReport
{
    public byte[] Raw { get; set; } = [];
    public TouchPoint[] Touches { get; set; } = [];
}

sealed class GraphAbort : Exception { }

static class GraphRetirement
{
    internal static async Task ReleaseAfterSnapshots(Task snapshots, RegistryGeneration generation)
    {
        await snapshots.ConfigureAwait(false);
        InstalledRegistry.Release(generation);
    }
}

unsafe sealed class SynchronousGraph : IDisposable
{
    internal const uint Position = 1, Tablet = 2, Eraser = 4, Tilt = 8, Proximity = 16,
        Tool = 32, Aux = 64, Mouse = 128, Absolute = 256, Relative = 512,
        AbsoluteWheel = 1024, RelativeWheel = 2048, WheelButtons = 4096, Touch = 8192, NativeTip = 16384;
    sealed class Node(GraphNode description, Action<IDeviceReport> emit)
    {
        public readonly uint Index = description.Index;
        public readonly Instance? Filter = description.Context == 0 ? null
            : (Instance)GCHandle.FromIntPtr(description.Context).Target!;
        public readonly Action<IDeviceReport> Emit = emit;
        public volatile bool Disabled;
        public int Pending;
        public Action<IDeviceReport>? ModeEmit;
    }
    [ThreadStatic] internal static IDeviceReport? CurrentReport;
    OutputInstance? managedOutput;
    RegistryGeneration? sourceGeneration;
    volatile bool disposed;
    internal RegistryGeneration? SourceGeneration { get { ObjectDisposedException.ThrowIf(disposed, this); return sourceGeneration; } }
    public void Dispose() {
        if (disposed) return;
        if (running || Environment.CurrentManagedThreadId != ownerThread) throw new InvalidOperationException("Invalid graph disposal lifecycle.");
        Task snapshots;
        lock (asyncGate)
        {
            disposed = true;
            snapshots = inFlightCompletion?.Task ?? Task.CompletedTask;
            ClearPending();
            asyncFailure = null; asyncError = null;
        }
        foreach (Node node in pre.Concat(post)) node.Filter?.AttachAsyncSink(null);
        if (sourceGeneration is { } generation)
        {
            sourceGeneration = null;
            ManagedRetirements.Track(GraphRetirement.ReleaseAfterSnapshots(snapshots, generation));
        }
        else ManagedRetirements.Track(snapshots);
    }
    readonly Node[] pre, post, timed;
    readonly Action<IDeviceReport> transform, output;
    readonly int ownerThread = Environment.CurrentManagedThreadId;
    delegate* unmanaged[Cdecl]<nint, uint, uint, GraphReport*, int> callback;
    nint scope;
    bool running, failed;
    string? outputFailure;
    readonly Node modeSource = new(new GraphNode { Index = uint.MaxValue }, _ => { });
    const int MaxPending = 1024, MaxNodePending = 64, MaxPendingBytes = 4 * 1024 * 1024;
    sealed class PendingEmission
    {
        internal Node? Node;
        internal IDeviceReport? Report;
        internal Action<IDeviceReport>? Resume;
        internal ulong Reservation;
        internal int Bytes;
        internal bool Ready;
    }
    readonly object asyncGate = new();
    // Slots themselves are allocated at setup only. The async path owns copied
    // report values; the native-only graph and synchronous Emit do no queue work.
    readonly PendingEmission[] pending = Enumerable.Range(0, MaxPending).Select(_ => new PendingEmission()).ToArray();
    int pendingHead, pendingCount, pendingBytes, inFlightBytes;
    TaskCompletionSource? inFlightCompletion;
    ulong reservation;
    Node? asyncFailure;
    string? asyncError;
    readonly bool hasManagedFilters;

    void ClearPending()
    {
        // Called under asyncGate, also invalidating unfinished reservations.
        while (pendingCount != 0)
        {
            PendingEmission item = pending[pendingHead];
            if (item.Node != null) item.Node.Pending--;
            item.Node = null; item.Report = null; item.Resume = null; item.Ready = false; item.Reservation = 0; item.Bytes = 0;
            pendingHead = (pendingHead + 1) % MaxPending; pendingCount--;
        }
        pendingBytes = 0;
    }
    void ResetPending() { lock (asyncGate) ClearPending(); }

    void AsyncError(Node node, string message)
    {
        if (asyncFailure == null) { asyncFailure = node; asyncError = message; }
    }
    void Enqueue(Node node, IDeviceReport? report, Action<IDeviceReport>? resume = null)
    {
        PendingEmission item;
        ulong ticket;
        lock (asyncGate)
        {
            if (disposed || outputFailure != null || node.Disabled || asyncFailure != null) return;
            // Reserve worst-case bytes before copying. Concurrent snapshots are
            // included in the same budget, and FIFO order is admission order.
            if (pendingCount == MaxPending || node.Pending == MaxNodePending
                || pendingBytes + inFlightBytes > MaxPendingBytes - OwnedReportSnapshot.MaxBytes || reservation == ulong.MaxValue)
            {
                AsyncError(node, ReferenceEquals(node, modeSource)
                    ? "Asynchronous output pipeline queue overflow; pending outputs were discarded and the mode must be restarted."
                    : "Asynchronous filter emission queue overflow; pending outputs were discarded and the emitting filter was disabled.");
                return;
            }
            item = pending[(pendingHead + pendingCount) % MaxPending];
            ticket = ++reservation; item.Reservation = ticket; item.Node = node;
            item.Report = null; item.Resume = resume; item.Ready = false; item.Bytes = 0;
            if (inFlightBytes == 0) inFlightCompletion = new(TaskCreationOptions.RunContinuationsAsynchronously);
            pendingCount++; node.Pending++; inFlightBytes += OwnedReportSnapshot.MaxBytes;
        }
        IDeviceReport? copy = null;
        int bytes = 0;
        string? error = null;
        try { copy = OwnedReportSnapshot.Capture(report, out bytes); }
        catch (Exception failure) { error = "Cannot own asynchronous pipeline report: " + failure.GetBaseException().Message; }
        TaskCompletionSource? completed = null;
        try
        {
            lock (asyncGate)
            {
                // Retirement/error can discard a reservation while its bounded
                // copy is in flight. Never fill a since-reused slot.
                inFlightBytes -= OwnedReportSnapshot.MaxBytes;
                if (inFlightBytes == 0) completed = inFlightCompletion;
                if (disposed || item.Reservation != ticket) return;
                if (error != null) { AsyncError(node, error); return; }
                pendingBytes += bytes;
                item.Bytes = bytes; item.Report = copy; item.Ready = true;
            }
        }
        finally { completed?.TrySetResult(); }

    }
    void DrainPending()
    {
        int limit;
        lock (asyncGate) limit = pendingCount;
        // A producer/downstream callback cannot make one tick drain forever.
        for (int i = 0; i <= limit; i++)
        {
            Node node;
            IDeviceReport? report;
            Action<IDeviceReport>? resume;
            lock (asyncGate)
            {
                if (asyncFailure is { } invalid)
                {
                    invalid.Disabled = true;
                    FailedIndex = ReferenceEquals(invalid, modeSource) ? -1 : checked((int)invalid.Index);
                    if (ReferenceEquals(invalid, modeSource)) outputFailure = asyncError;
                    Error = asyncError; failed = true;
                    asyncFailure = null; asyncError = null; ClearPending();
                    throw new GraphAbort();
                }
                if (i == limit || pendingCount == 0) return;
                PendingEmission item = pending[pendingHead];
                if (!item.Ready) return; // Earlier admitted copy must finish first.
                node = item.Node!; report = item.Report; resume = item.Resume;
                pendingBytes -= item.Bytes; pendingCount--; node.Pending--;
                item.Node = null; item.Report = null; item.Resume = null; item.Reservation = 0; item.Bytes = 0; item.Ready = false;
                pendingHead = (pendingHead + 1) % MaxPending;
            }
            if (node.Disabled) continue;
            int previous = currentEmitter; currentEmitter = ReferenceEquals(node, modeSource) ? -1 : checked((int)node.Index);
            try { (resume ?? node.ModeEmit ?? node.Emit)(report!); if (failed) throw new GraphAbort(); }
            catch (GraphAbort) { throw; }
            catch (Exception error)
            {
                node.Disabled = true; FailedIndex = ReferenceEquals(node, modeSource) ? -1 : checked((int)node.Index);
                Error = error.GetBaseException().Message;
                if (ReferenceEquals(node, modeSource)) outputFailure = Error;
                failed = true;
                throw new GraphAbort();
            }
            finally { currentEmitter = previous; }
        }
    }
    // Fused continuations: the host already ran its built-in filters, and
    // operation 4 transforms and outputs in one native call. Each native call
    // exports the report, crosses into Rust and decodes it there.
    bool fused;
    int currentEmitter = -1;
    public int FailedIndex { get; private set; } = -1;
    public string? Error { get; private set; }
    // The interfaces a concrete report type implements never change, so each
    // type is tested once instead of on every bridge call: 28 type tests per
    // report on the fused path before. Two entries cover a pen stream
    // interleaved with another report type. Owner thread only.
    Type? shapeType, previousShapeType;
    uint shape, previousShape;

    public SynchronousGraph(ReadOnlySpan<GraphNode> nodes, bool captureRegistry = true)
    {
        if (nodes.Length > 32) throw new ArgumentException("The synchronous graph supports at most 32 filters.");
        GraphNode[] owned = nodes.ToArray();
        if (owned.Any(node => node.Stage is not (1 or 2)))
            throw new ArgumentException("Invalid synchronous pipeline stage.");
        transform = Transform; output = Output;
        pre = CreateStage(owned, 1, transform);
        post = CreateStage(owned, 2, output);
        timed = pre.Concat(post).Where(node => node.Filter is { HasTimers: true }).ToArray();
        hasManagedFilters = pre.Concat(post).Any(node => node.Filter != null);
        Precompiler.Bridge();
        foreach (Node node in pre.Concat(post)) node.Filter?.PrecompileConsume();
        if (captureRegistry) {
            var generations = pre.Concat(post).Select(node => node.Filter?.SourceGeneration).Where(value => value != null).Distinct().ToArray();
            if (generations.Length > 1) throw new InvalidOperationException("Installed registry changed while constructing the report graph; retry startup.");
            sourceGeneration = InstalledRegistry.RetainForGraph(generations.FirstOrDefault());
        }
        foreach (Node node in pre.Concat(post)) node.Filter?.AttachAsyncSink(report => Enqueue(node, report), ResetPending);
    }

    Node[] CreateStage(GraphNode[] descriptions, uint stage, Action<IDeviceReport> end)
    {
        GraphNode[] selected = descriptions.Where(node => node.Stage == stage).ToArray();
        var nodes = new Node[selected.Length];
        for (int offset = 0; offset < selected.Length; offset++)
        {
            int next = offset + 1;
            uint index = selected[offset].Index;
            nodes[offset] = new Node(selected[offset], emitted =>
            {
                int previous = currentEmitter;
                currentEmitter = checked((int)index);
                try { Visit(nodes, next, emitted, end); }
                finally { currentEmitter = previous; }
            });
        }
        return nodes;
    }
    public int Dispatch(GraphReport* input,
        delegate* unmanaged[Cdecl]<nint, uint, uint, GraphReport*, int> native, nint nativeScope,
        bool fusedContinuations = false, IDeviceReport? originalReport = null)
    {
        using var reportScope = ServiceClient.Report();
        ObjectDisposedException.ThrowIf(disposed, this);
        if (running || Environment.CurrentManagedThreadId != ownerThread)
            throw new InvalidOperationException("The synchronous graph must run on its owning thread.");
        if (outputFailure != null) throw new InvalidOperationException(outputFailure);
        running = true; failed = false; FailedIndex = -1; currentEmitter = -1; Error = null;
        callback = native; scope = nativeScope; fused = fusedContinuations;
        try
        {
            if (hasManagedFilters || managedOutput != null) DrainPending();
            IDeviceReport report = originalReport ?? Import(input);
            // Host built-ins can modify position before the graph dispatch. The
            // concrete original report object remains the downstream identity.
            if (originalReport is IAbsolutePositionReport position && (input->Flags & Position) != 0)
                position.Position = new Vector2(input->X, input->Y);
            if (fused || Call(0, 0, report))
            {
                if (managedOutput is { } endpoint) { endpoint.Mode.Read(report); DrainOutput(); }
                else Visit(pre, 0, report, transform);
            }
            return failed ? -1 : 0;
        }
        catch (GraphAbort) { return -1; }
        finally { callback = null; scope = 0; running = false; }
    }

    /// Microseconds until the next tick; -1 when stopped; -2 when the graph
    /// has no timer capability. Only -2 is safe for the native host to cache.
    public long NextTickMicros()
    {
        ObjectDisposedException.ThrowIf(disposed, this);
        if (outputFailure != null) return -1;
        if (hasManagedFilters || managedOutput != null)
            lock (asyncGate)
            {
                if (asyncFailure != null || pendingCount != 0 && pending[pendingHead].Ready) return 0;
            }
        if (!hasManagedFilters && timed.Length == 0 && managedOutput == null) return -2;
        // Async callbacks have no native wake handle. A bounded idle poll keeps
        // newly admitted work live; native may cache only the -2 capability.
        long now = Stopwatch.GetTimestamp(), best = hasManagedFilters || managedOutput != null ? 10_000 : -1;
        foreach (Node node in timed)
            if (!node.Disabled && node.Filter is { HasTimers: true } filter)
            {
                long micros = filter.NextTickMicros(now);
                if (micros >= 0 && (best < 0 || micros < best)) best = micros;
            }
        if (managedOutput is { } endpoint) { long next = endpoint.NextTickMicros(); if (next >= 0 && (best < 0 || next < best)) best = next; }
        return best;
    }

    /// Fires due timers. A timer emission continues downstream of its filter,
    /// exactly as a synchronous emission does.
    public int Tick(delegate* unmanaged[Cdecl]<nint, uint, uint, GraphReport*, int> native, nint nativeScope,
        bool fusedContinuations = false)
    {
        using var reportScope = ServiceClient.Report();
        ObjectDisposedException.ThrowIf(disposed, this);
        if (running || Environment.CurrentManagedThreadId != ownerThread)
            throw new InvalidOperationException("The synchronous graph must run on its owning thread.");
        if (outputFailure != null) throw new InvalidOperationException(outputFailure);
        running = true; failed = false; FailedIndex = -1; currentEmitter = -1; Error = null;
        callback = native; scope = nativeScope; fused = fusedContinuations;
        try
        {
            if (hasManagedFilters || managedOutput != null) DrainPending();
            long now = Stopwatch.GetTimestamp();
            TickStage(pre, now);
            TickStage(post, now);
            if (managedOutput is { } endpoint) { endpoint.Tick(); DrainOutput(); }
            return failed ? -1 : 0;
        }
        catch (GraphAbort) { return -1; }
        finally { callback = null; scope = 0; running = false; }
    }

    void TickStage(Node[] nodes, long now)
    {
        for (int offset = 0; offset < nodes.Length; offset++)
        {
            Node node = nodes[offset];
            if (node.Disabled || node.Filter is not { HasTimers: true } filter) continue;

            try
            {
                filter.TickGraph(now, node.ModeEmit ?? node.Emit);
                if (failed) throw new GraphAbort();
            }
            catch (GraphAbort) { throw; }
            catch (Exception error)
            {
                if (!failed)
                {
                    node.Disabled = true;
                    FailedIndex = checked((int)node.Index);
                    Error = error.GetBaseException().Message;
                    failed = true;
                }
                throw new GraphAbort();
            }
        }
    }

    sealed class ModeElement : IPositionedPipelineElement<IDeviceReport>
    {
        readonly SynchronousGraph graph;
        readonly Node? node;
        readonly Action<IDeviceReport> queuedConsume;
        public PipelinePosition Position { get; }
        public ModeElement(SynchronousGraph graph, Node? node, PipelinePosition position)
        {
            this.graph = graph; this.node = node; Position = position;
            queuedConsume = Consume;
        }
        public event Action<IDeviceReport> Emit = delegate { };
        public void Continue(IDeviceReport report) => Emit(report);
        public void Consume(IDeviceReport report)
        {
            if (!graph.running || Environment.CurrentManagedThreadId != graph.ownerThread)
            {
                // This is an output mode's own pipeline handoff, before this
                // element (distinct from filter Emit continuing after a node).
                graph.Enqueue(graph.modeSource, report, queuedConsume);
                return;
            }
            if (graph.failed) throw new GraphAbort();
            if (node == null)
            {
                if (graph.Call(5, 0, report)) { Emit(report); graph.DrainOutput(); }
                return;
            }
            if (node.Disabled) { Emit(report); return; }
            if (node.Filter == null) { if (graph.Call(1, node.Index, report)) Emit(report); return; }
            try { node.Filter.ConsumeGraph(report, Continue); if (graph.failed) throw new GraphAbort(); }
            catch (GraphAbort) { throw; }
            catch (Exception error) { node.Disabled = true; graph.FailedIndex = checked((int)node.Index); graph.Error = error.GetBaseException().Message; graph.failed = true; throw new GraphAbort(); }
        }
    }
    internal void AttachOutput(OutputInstance endpoint)
    {
        if (managedOutput != null || running || Environment.CurrentManagedThreadId != ownerThread) throw new InvalidOperationException("Invalid output attachment lifecycle.");
        var elements = new List<IPositionedPipelineElement<IDeviceReport>>();
        foreach (Node node in pre) { var element = new ModeElement(this, node, PipelinePosition.PreTransform); elements.Add(element); node.ModeEmit = element.Continue; }
        foreach (Node node in post) { var element = new ModeElement(this, node, PipelinePosition.PostTransform); elements.Add(element); node.ModeEmit = element.Continue; }
        elements.Add(new ModeElement(this, null, PipelinePosition.PostTransform));
        endpoint.Mode.Elements = elements;
        managedOutput = endpoint;
    }
    void DrainOutput()
    {
        if (managedOutput is not { } endpoint) return;
        // Foreign producers cannot keep one native callback draining forever.
        int limit = endpoint.Queue.PendingCount;
        for (int index = 0; index < limit; index++)
        {
            ManagedCommand command;
            try { if (!endpoint.Queue.Take(out command)) return; }
            catch (Exception error)
            {
                outputFailure = Error = error.GetBaseException().Message;
                modeSource.Disabled = true; failed = true;
                lock (asyncGate) ClearPending();
                throw new GraphAbort();
            }
            GraphReport frame = new() { Version = 2, Size = (uint)sizeof(GraphReport), Kind = command.Kind, Reserved = command.Owner,
                X = command.X, Y = command.Y, Pressure = command.Value, Flags = command.Flags, TiltX = command.TiltX, TiltY = command.TiltY };
            if (callback(scope, 6, 0, &frame) != 0) { failed = true; throw new GraphAbort(); }
        }
    }

    void Transform(IDeviceReport report)
    {
        // Without post-transform filters nothing runs between the two.
        if (fused && post.Length == 0) { Call(4, 0, report); return; }
        if (Call(2, 0, report)) Visit(post, 0, report, output);
    }
    void Output(IDeviceReport report) { Call(3, 0, report); }

    // Exact upstream PipelineManager subscription semantics: Emit immediately
    // consumes the same object downstream, then returns to the emitting filter.
    // Source: Plugin/Output/PipelineManager.cs at 736003ed72c8bbb28033b039d5a0bb76c344145c.
    void Visit(Node[] nodes, int offset, IDeviceReport report, Action<IDeviceReport> end)
    {
        if (failed) throw new GraphAbort();
        if (offset == nodes.Length) { end(report); return; }
        Node node = nodes[offset];
        if (node.Disabled) { Visit(nodes, offset + 1, report, end); return; }
        if (node.Filter == null)
        {
            if (Call(1, node.Index, report)) Visit(nodes, offset + 1, report, end);
            return;
        }
        try
        {
            node.Filter.ConsumeGraph(report, node.Emit);
            // A plugin may catch an exception raised by a downstream consumer.
            // The failed prefix is still committed; never resume/replay it.
            if (failed) throw new GraphAbort();
        }
        catch (GraphAbort) { throw; }
        catch (Exception error)
        {
            if (!failed)
            {
                node.Disabled = true;
                FailedIndex = checked((int)node.Index);
                Error = error.GetBaseException().Message;
                failed = true;
            }
            throw new GraphAbort();
        }
    }

    bool Call(uint operation, uint index, IDeviceReport report)
    {
        if (failed) throw new GraphAbort();
        try
        {
            GraphReport frame;
            uint capabilities = Export(report, &frame);
            byte[] raw = report.Raw ?? throw new InvalidOperationException("Report.Raw is null.");
            fixed (byte* bytes = raw)
            {
                frame.Raw = bytes; frame.RawLength = checked((uint)raw.Length);
                var originalPosition = new Vector2(frame.X, frame.Y);
                IDeviceReport? previous = CurrentReport;
                int result;
                CurrentReport = report;
                try { result = callback(scope, operation, index, &frame); }
                finally { CurrentReport = previous; }
                if (result < 0) { failed = true; throw new GraphAbort(); }
                if (operation == 5 && (capabilities & Tablet) != 0)
                    Unsafe.As<ITabletReport>(report).Pressure = frame.Pressure;
                var updatedPosition = new Vector2(frame.X, frame.Y);
                // Transform (2, or 4 fused with output) moves the report itself,
                // as upstream's output mode does.
                if (operation != 3 && (capabilities & Position) != 0
                    && (operation is 2 or 4 || originalPosition != updatedPosition))
                    Unsafe.As<IAbsolutePositionReport>(report).Position = updatedPosition;
                return result == 0;
            }
        }
        catch (GraphAbort) { throw; }
        catch (Exception error)
        {
            Error = error.GetBaseException().Message;
            if (currentEmitter >= 0)
            {
                FailedIndex = currentEmitter;
                foreach (Node node in pre) if (node.Index == (uint)currentEmitter) node.Disabled = true;
                foreach (Node node in post) if (node.Index == (uint)currentEmitter) node.Disabled = true;
            }
            failed = true;
            throw new GraphAbort();
        }
    }

    static ulong Pack(bool[] values)
    {
        ArgumentNullException.ThrowIfNull(values);
        if (values.Length > 64) throw new NotSupportedException("Report exceeds 64 buttons.");
        ulong bits = 0;
        for (int index = 0; index < values.Length; index++) if (values[index]) bits |= 1UL << index;
        return bits;
    }
    static bool[] Unpack(ulong bits, uint count)
    {
        if (count > 64) throw new ArgumentException("Invalid button count.");
        var values = new bool[count];
        for (int index = 0; index < values.Length; index++) values[index] = (bits & (1UL << index)) != 0;
        return values;
    }

    /// The report's capabilities: SynchronousGraph flags plus OutOfRange and
    /// PenSnapshot markers. Computed once per concrete type.
    uint ShapeOf(IDeviceReport report)
    {
        Type type = report.GetType();
        if (ReferenceEquals(type, shapeType)) return shape;
        if (ReferenceEquals(type, previousShapeType))
        {
            (shapeType, previousShapeType) = (previousShapeType, shapeType);
            (shape, previousShape) = (previousShape, shape);
            return shape;
        }
        uint value = 0;
        if (report is OutOfRangeReport) value |= OutOfRangeShape;
        if (report is IAbsolutePositionReport) value |= Position;
        if (report is ITabletReport) value |= Tablet;
        if (report is IEraserReport) value |= Eraser;
        if (report is ITiltReport) value |= Tilt;
        if (report is IProximityReport) value |= Proximity;
        if (report is IToolReport) value |= Tool;
        if (report is IAuxReport) value |= Aux;
        if (report is IMouseReport) value |= Mouse;
        if (report is IAbsoluteAnalogReport) value |= Absolute;
        if (report is IAbsoluteWheelReport) value |= AbsoluteWheel;
        if (report is IRelativeAnalogReport) value |= Relative;
        if (report is IRelativeWheelReport) value |= RelativeWheel;
        if (report is IWheelButtonReport) value |= WheelButtons;
        if (report is ITouchReport) value |= Touch;
        if (report is PenSnapshot) value |= PenSnapshotShape;
        // COM and dynamically castable objects answer interface tests per
        // instance; test those on every call, as before.
        if (type.IsCOMObject || report is IDynamicInterfaceCastable) return value;
        previousShapeType = shapeType; previousShape = shape;
        shapeType = type; shape = value;
        return value;
    }
    const uint OutOfRangeShape = 1u << 30, PenSnapshotShape = 1u << 31;

    // Each Unsafe.As below is guarded by the cached shape of this exact type.
    internal uint Export(IDeviceReport report, GraphReport* frame)
    {
        uint s = ShapeOf(report);
        ref GraphReport f = ref *frame;
        *frame = default;
        f.Version = 2; f.Size = (uint)sizeof(GraphReport);
        f.Kind = (s & OutOfRangeShape) != 0 ? 1u : 0;
        if ((s & Position) != 0)
        { Vector2 value = Unsafe.As<IAbsolutePositionReport>(report).Position; f.Flags |= Position; f.X = value.X; f.Y = value.Y; }
        if ((s & Tablet) != 0)
        {
            var tablet = Unsafe.As<ITabletReport>(report);
            f.Flags |= Tablet; f.Pressure = tablet.Pressure;
            bool[] buttons = tablet.PenButtons;
            f.PenBits = Pack(buttons); f.PenCount = (uint)buttons.Length;
        }
        if ((s & Eraser) != 0) { f.Flags |= Eraser; f.Eraser = Unsafe.As<IEraserReport>(report).Eraser ? 1u : 0; }
        if ((s & Tilt) != 0)
        { Vector2 value = Unsafe.As<ITiltReport>(report).Tilt; f.Flags |= Tilt; f.TiltX = value.X; f.TiltY = value.Y; }
        if ((s & Proximity) != 0)
        {
            var proximity = Unsafe.As<IProximityReport>(report);
            f.Flags |= Proximity; f.Near = proximity.NearProximity ? 1u : 0; f.Distance = proximity.HoverDistance;
        }
        if ((s & Tool) != 0)
        {
            var tool = Unsafe.As<IToolReport>(report);
            if (tool.Tool is not (OpenTabletDriver.Plugin.Tablet.ToolType.Pen or OpenTabletDriver.Plugin.Tablet.ToolType.Eraser))
                throw new NotSupportedException("Unknown report tool type.");
            f.Flags |= Tool; f.Serial = tool.Serial; f.ToolID = tool.RawToolID; f.ToolType = (uint)tool.Tool;
        }
        if ((s & Aux) != 0)
        { bool[] buttons = Unsafe.As<IAuxReport>(report).AuxButtons; f.Flags |= Aux; f.AuxBits = Pack(buttons); f.AuxCount = (uint)buttons.Length; }
        if ((s & Mouse) != 0)
        {
            var mouse = Unsafe.As<IMouseReport>(report);
            bool[] buttons = mouse.MouseButtons; Vector2 scroll = mouse.Scroll;
            f.Flags |= Mouse; f.MouseBits = Pack(buttons); f.MouseCount = (uint)buttons.Length;
            f.ScrollX = scroll.X; f.ScrollY = scroll.Y;
        }
        if ((s & Absolute) != 0)
        {
            uint?[] values = Unsafe.As<IAbsoluteAnalogReport>(report).AnalogPositions ?? throw new InvalidOperationException("Null analog array.");
            if (values.Length > 16) throw new NotSupportedException("Report exceeds 16 analog channels.");
            f.Flags |= Absolute; if ((s & AbsoluteWheel) != 0) f.Flags |= AbsoluteWheel;
            f.AbsoluteCount = (uint)values.Length;
            for (int i = 0; i < values.Length; i++) if (values[i] is uint value)
            { f.AbsolutePresent |= 1u << i; f.AbsoluteValues[i] = value; }
        }
        if ((s & Relative) != 0)
        {
            int[] values = Unsafe.As<IRelativeAnalogReport>(report).AnalogDeltas ?? throw new InvalidOperationException("Null analog array.");
            if (values.Length > 16) throw new NotSupportedException("Report exceeds 16 analog channels.");
            f.Flags |= Relative; if ((s & RelativeWheel) != 0) f.Flags |= RelativeWheel;
            f.RelativeCount = (uint)values.Length;
            for (int i = 0; i < values.Length; i++) f.RelativeValues[i] = values[i];
        }
        if ((s & WheelButtons) != 0)
        {
            bool[][] values = Unsafe.As<IWheelButtonReport>(report).WheelButtons ?? throw new InvalidOperationException("Null wheel array.");
            if (values.Length > 8) throw new NotSupportedException("Report exceeds 8 wheels.");
            f.Flags |= WheelButtons; f.WheelCount = (uint)values.Length;
            for (int i = 0; i < values.Length; i++) { f.WheelBits[i] = Pack(values[i]); f.WheelCounts[i] = (uint)values[i].Length; }
        }
        if ((s & Touch) != 0)
        {
            TouchPoint[] values = Unsafe.As<ITouchReport>(report).Touches ?? throw new InvalidOperationException("Null touch array.");
            if (values.Length > 32) throw new NotSupportedException("Report exceeds 32 touches.");
            f.Flags |= Touch; f.TouchCount = (uint)values.Length;
            for (int i = 0; i < values.Length; i++) if (values[i] is { } point)
            {
                f.TouchPresent |= 1u << i; f.TouchIDs[i] = point.TouchID;
                f.TouchXY[i * 2] = point.Position.X; f.TouchXY[i * 2 + 1] = point.Position.Y;
            }
        }
        if ((s & PenSnapshotShape) != 0 && Unsafe.As<PenSnapshot>(report).NativeTipSwitch is bool tip)
        { f.Flags |= NativeTip; f.TipSwitch = tip ? 1u : 0; }
        return s;
    }

    internal IDeviceReport Import(GraphReport* f)
    {
        if (f == null || f->Version != 2 || f->Size != sizeof(GraphReport)
            || f->RawLength > ushort.MaxValue || (f->Raw == null && f->RawLength != 0))
            throw new ArgumentException("Invalid native graph report.");
        byte[] raw = new ReadOnlySpan<byte>(f->Raw, (int)f->RawLength).ToArray();
        if (f->Kind == 1) return new OutOfRangeReport(raw);
        if (f->Kind != 0) throw new ArgumentException("Unknown report kind.");
        if ((f->Flags & NativeTip) != 0 && (f->Flags & Tablet) == 0)
            throw new ArgumentException("Native tip state requires a tablet report.");
        // Supported parser shapes, without advertising absent interfaces.
        // Unsupported future shapes fail explicitly instead of losing fields.
        IDeviceReport report = (f->Flags & ~NativeTip) switch
        {
            0 => new DeviceSnapshot(),
            Position | Tablet => new PenSnapshot(),
            Position | Tablet | Eraser => new Report(),
            Position | Tablet | Tilt => new TiltPenSnapshot(),
            Position | Tablet | Proximity => new ProximityPenSnapshot(),
            Position | Tablet | Eraser | Tilt => new TiltReport(),
            Position | Tablet | Eraser | Proximity => new EraserProximitySnapshot(),
            Position | Tablet | Tilt | Proximity => new TiltProximityPenSnapshot(),
            Position | Tablet | Eraser | Tilt | Proximity => new ProximityReport(),
            Position | Tablet | Eraser | Proximity | Aux => new AuxPenSnapshot(),
            Tool | Eraser | Proximity => new ToolSnapshot(),
            Position | Mouse => new MouseSnapshot(),
            Position | Mouse | Proximity => new ProximityMouseSnapshot(),
            Position | Mouse | Aux => new AuxMouseSnapshot(),
            Aux => new AuxSnapshot(),
            Aux | Absolute | AbsoluteWheel | WheelButtons => new RingSnapshot(),
            Aux | WheelButtons => new AuxWheelButtonsSnapshot(),
            Absolute => new AbsoluteSnapshot(),
            Absolute | AbsoluteWheel => new AbsoluteWheelSnapshot(),
            Aux | Absolute => new AuxAbsoluteSnapshot(),
            Relative => new RelativeSnapshot(),
            Relative | RelativeWheel => new RelativeWheelSnapshot(),
            Aux | Relative | RelativeWheel => new AuxRelativeWheelSnapshot(),
            Touch => new TouchSnapshot(),
            Aux | Touch => new AuxTouchSnapshot(),
            _ => throw new NotSupportedException($"Unsupported native report capabilities: 0x{f->Flags:x}.")
        };
        report.Raw = raw;
        // Each Unsafe.As below is guarded by the cached shape of this exact type.
        uint s = ShapeOf(report);
        if ((s & Position) != 0) Unsafe.As<IAbsolutePositionReport>(report).Position = new Vector2(f->X, f->Y);
        if ((s & Tablet) != 0)
        {
            var tablet = Unsafe.As<ITabletReport>(report);
            tablet.Pressure = f->Pressure; tablet.PenButtons = Unpack(f->PenBits, f->PenCount);
        }
        if ((s & PenSnapshotShape) != 0)
            Unsafe.As<PenSnapshot>(report).NativeTipSwitch = (f->Flags & NativeTip) != 0 ? f->TipSwitch != 0 : null;
        if ((s & Eraser) != 0) Unsafe.As<IEraserReport>(report).Eraser = f->Eraser != 0;
        if ((s & Tilt) != 0) Unsafe.As<ITiltReport>(report).Tilt = new Vector2(f->TiltX, f->TiltY);
        if ((s & Proximity) != 0)
        {
            var proximity = Unsafe.As<IProximityReport>(report);
            proximity.NearProximity = f->Near != 0; proximity.HoverDistance = f->Distance;
        }
        if ((s & Tool) != 0)
        {
            var tool = Unsafe.As<IToolReport>(report);
            if (f->ToolType > 1) throw new ArgumentException("Invalid tool type.");
            tool.Serial = f->Serial; tool.RawToolID = f->ToolID; tool.Tool = (ToolType)f->ToolType;
        }
        if ((s & Aux) != 0) Unsafe.As<IAuxReport>(report).AuxButtons = Unpack(f->AuxBits, f->AuxCount);
        if ((s & Mouse) != 0)
        {
            var mouse = Unsafe.As<IMouseReport>(report);
            mouse.MouseButtons = Unpack(f->MouseBits, f->MouseCount); mouse.Scroll = new Vector2(f->ScrollX, f->ScrollY);
        }
        if ((s & Absolute) != 0)
        {
            if (f->AbsoluteCount > 16) throw new ArgumentException("Invalid absolute analog capacity.");
            var positions = new uint?[f->AbsoluteCount];
            for (int i = 0; i < positions.Length; i++) if ((f->AbsolutePresent & (1u << i)) != 0) positions[i] = f->AbsoluteValues[i];
            Unsafe.As<IAbsoluteAnalogReport>(report).AnalogPositions = positions;
        }
        if ((s & Relative) != 0)
        {
            if (f->RelativeCount > 16) throw new ArgumentException("Invalid relative analog capacity.");
            var deltas = new int[f->RelativeCount];
            for (int i = 0; i < deltas.Length; i++) deltas[i] = f->RelativeValues[i];
            Unsafe.As<IRelativeAnalogReport>(report).AnalogDeltas = deltas;
        }
        if ((s & WheelButtons) != 0)
        {
            if (f->WheelCount > 8) throw new ArgumentException("Invalid wheel capacity.");
            var buttons = new bool[f->WheelCount][];
            for (int i = 0; i < buttons.Length; i++) buttons[i] = Unpack(f->WheelBits[i], f->WheelCounts[i]);
            Unsafe.As<IWheelButtonReport>(report).WheelButtons = buttons;
        }
        if ((s & Touch) != 0)
        {
            if (f->TouchCount > 32) throw new ArgumentException("Invalid touch capacity.");
            var points = new TouchPoint[f->TouchCount];
            for (int i = 0; i < points.Length; i++) if ((f->TouchPresent & (1u << i)) != 0)
                points[i] = new TouchPoint { TouchID = checked((byte)f->TouchIDs[i]), Position = new Vector2(f->TouchXY[i * 2], f->TouchXY[i * 2 + 1]) };
            if (report is TouchSnapshot touch) touch.Touches = points;
            else if (report is AuxTouchSnapshot auxTouch) auxTouch.Touches = points;
        }
        return report;
    }
}

// The first report through a new graph used to wait while the runtime compiled
// the bridge's report path and the plugin's Consume: about 7 ms in the offline
// harness. The graph is created on the session thread before the first read,
// so compile them then. Preparing compiles a method without calling it. Best
// effort: a method that cannot be prepared here compiles on its first call.
static class Precompiler
{
    const BindingFlags Declared = BindingFlags.Instance | BindingFlags.Static
        | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
    static int bridgePrepared;

    public static void Bridge()
    {
        if (Interlocked.Exchange(ref bridgePrepared, 1) != 0) return;
        try
        {
            Prepare(typeof(SynchronousGraph));
            Prepare(typeof(Instance));
            foreach (Type type in typeof(SynchronousGraph).Assembly.GetTypes())
                if (!type.IsInterface && typeof(IDeviceReport).IsAssignableFrom(type)) Prepare(type);
            foreach (string name in new[] { nameof(EntryPoints.DispatchGraph), nameof(EntryPoints.DispatchGraph2),
                nameof(EntryPoints.TickGraph), nameof(EntryPoints.TickGraph2), nameof(EntryPoints.GraphNextTick) })
                if (typeof(EntryPoints).GetMethod(name, Declared) is { } entry) Prepare(entry);
        }
        catch (Exception error) { Console.Error.WriteLine($".NET bridge precompilation skipped: {error.GetBaseException().Message}"); }
    }

    public static void Prepare(Type type)
    {
        foreach (MethodInfo method in type.GetMethods(Declared)) Prepare(method);
        foreach (ConstructorInfo constructor in type.GetConstructors(Declared)) Prepare(constructor);
        // Lambdas and the graph's nodes.
        foreach (Type nested in type.GetNestedTypes(Declared)) Prepare(nested);
    }

    public static void Prepare(MethodBase method)
    {
        if (method.IsAbstract || method.ContainsGenericParameters || (method.DeclaringType?.ContainsGenericParameters ?? false))
            return;
        try { RuntimeHelpers.PrepareMethod(method.MethodHandle); }
        catch (Exception) { }
    }
}

public static unsafe partial class EntryPoints
{
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static nint CreateGraph(GraphNode* nodes, uint count)
    {
        try
        {
            if (count > 32 || (nodes == null && count != 0)) throw new ArgumentException("Invalid graph nodes.");
            return GCHandle.ToIntPtr(GCHandle.Alloc(new SynchronousGraph(new ReadOnlySpan<GraphNode>(nodes, (int)count))));
        }
        catch (Exception error) { lastError = error.GetBaseException().Message; return 0; }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int DispatchGraph(nint context, GraphReport* report,
        delegate* unmanaged[Cdecl]<nint, uint, uint, GraphReport*, int> callback, nint scope)
    {
        try
        {
            if (callback == null) throw new ArgumentException("Missing graph continuation.");
            var graph = (SynchronousGraph)GCHandle.FromIntPtr(context).Target!;
            int result = graph.Dispatch(report, callback, scope);
            if (result != 0) lastError = graph.Error ?? "A native graph continuation failed.";
            return result;
        }
        catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }

    /// DispatchGraph with fused continuations, for 0.15.3 hosts and later.
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int DispatchGraph2(nint context, GraphReport* report,
        delegate* unmanaged[Cdecl]<nint, uint, uint, GraphReport*, int> callback, nint scope)
    {
        try
        {
            if (callback == null) throw new ArgumentException("Missing graph continuation.");
            var graph = (SynchronousGraph)GCHandle.FromIntPtr(context).Target!;
            int result = graph.Dispatch(report, callback, scope, fusedContinuations: true);
            if (result != 0) lastError = graph.Error ?? "A native graph continuation failed.";
            return result;
        }
        catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }

    /// TickGraph with fused continuations, for 0.15.3 hosts and later.
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int TickGraph2(nint context,
        delegate* unmanaged[Cdecl]<nint, uint, uint, GraphReport*, int> callback, nint scope)
    {
        try
        {
            if (callback == null) throw new ArgumentException("Missing graph continuation.");
            var graph = (SynchronousGraph)GCHandle.FromIntPtr(context).Target!;
            int result = graph.Tick(callback, scope, fusedContinuations: true);
            if (result != 0) lastError = graph.Error ?? "A native graph continuation failed.";
            return result;
        }
        catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static long GraphNextTick(nint context)
    {
        try { return ((SynchronousGraph)GCHandle.FromIntPtr(context).Target!).NextTickMicros(); }
        catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int TickGraph(nint context,
        delegate* unmanaged[Cdecl]<nint, uint, uint, GraphReport*, int> callback, nint scope)
    {
        try
        {
            if (callback == null) throw new ArgumentException("Missing graph continuation.");
            var graph = (SynchronousGraph)GCHandle.FromIntPtr(context).Target!;
            int result = graph.Tick(callback, scope);
            if (result != 0) lastError = graph.Error ?? "A native graph continuation failed.";
            return result;
        }
        catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int GraphFailure(nint context)
    {
        try { return ((SynchronousGraph)GCHandle.FromIntPtr(context).Target!).FailedIndex; }
        catch { return -1; }
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static void DestroyGraph(nint context)
    {
        try { var handle = GCHandle.FromIntPtr(context); ((SynchronousGraph)handle.Target!).Dispose(); handle.Free(); }
        catch (Exception error) { lastError = error.GetBaseException().Message; }
    }
}

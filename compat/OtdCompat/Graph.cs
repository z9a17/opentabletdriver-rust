using System.Diagnostics;
using System.Numerics;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using OpenTabletDriver.Plugin.Tablet;
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

unsafe sealed class SynchronousGraph
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
        public bool Disabled;
    }
    readonly Node[] pre, post, timed;
    readonly Action<IDeviceReport> transform, output;
    readonly int ownerThread = Environment.CurrentManagedThreadId;
    delegate* unmanaged[Cdecl]<nint, uint, uint, GraphReport*, int> callback;
    nint scope;
    bool running, failed;
    // Fused continuations: the host already ran its built-in filters, and
    // operation 4 transforms and outputs in one native call. Each native call
    // exports the report, crosses into Rust and decodes it there.
    bool fused;
    int currentEmitter = -1;
    public int FailedIndex { get; private set; } = -1;
    public string? Error { get; private set; }

    public SynchronousGraph(ReadOnlySpan<GraphNode> nodes)
    {
        if (nodes.Length > 32) throw new ArgumentException("The synchronous graph supports at most 32 filters.");
        GraphNode[] owned = nodes.ToArray();
        if (owned.Any(node => node.Stage is not (1 or 2)))
            throw new ArgumentException("Invalid synchronous pipeline stage.");
        transform = Transform; output = Output;
        pre = CreateStage(owned, 1, transform);
        post = CreateStage(owned, 2, output);
        timed = pre.Concat(post).Where(node => node.Filter is { HasTimers: true }).ToArray();
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
        bool fusedContinuations = false)
    {
        if (running || Environment.CurrentManagedThreadId != ownerThread)
            throw new InvalidOperationException("The synchronous graph must run on its owning thread.");
        running = true; failed = false; FailedIndex = -1; currentEmitter = -1; Error = null;
        callback = native; scope = nativeScope; fused = fusedContinuations;
        try
        {
            IDeviceReport report = Import(input);
            if (fused || Call(0, 0, report)) Visit(pre, 0, report, transform);
            return failed ? -1 : 0;
        }
        catch (GraphAbort) { return -1; }
        finally { callback = null; scope = 0; running = false; }
    }

    /// Microseconds until the next tick; -1 when stopped; -2 when the graph
    /// has no timer capability. Only -2 is safe for the native host to cache.
    public long NextTickMicros()
    {
        if (timed.Length == 0) return -2;
        long now = Stopwatch.GetTimestamp(), best = -1;
        foreach (Node node in timed)
            if (!node.Disabled && node.Filter is { HasTimers: true } filter)
            {
                long micros = filter.NextTickMicros(now);
                if (micros >= 0 && (best < 0 || micros < best)) best = micros;
            }
        return best;
    }

    /// Fires due timers. A timer emission continues downstream of its filter,
    /// exactly as a synchronous emission does.
    public int Tick(delegate* unmanaged[Cdecl]<nint, uint, uint, GraphReport*, int> native, nint nativeScope,
        bool fusedContinuations = false)
    {
        if (running || Environment.CurrentManagedThreadId != ownerThread)
            throw new InvalidOperationException("The synchronous graph must run on its owning thread.");
        running = true; failed = false; FailedIndex = -1; currentEmitter = -1; Error = null;
        callback = native; scope = nativeScope; fused = fusedContinuations;
        try
        {
            long now = Stopwatch.GetTimestamp();
            TickStage(pre, now);
            TickStage(post, now);
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
                filter.TickGraph(now, node.Emit);
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
            GraphReport frame = Export(report);
            byte[] raw = report.Raw ?? throw new InvalidOperationException("Report.Raw is null.");
            fixed (byte* bytes = raw)
            {
                frame.Raw = bytes; frame.RawLength = checked((uint)raw.Length);
                var originalPosition = new Vector2(frame.X, frame.Y);
                int result = callback(scope, operation, index, &frame);
                if (result < 0) { failed = true; throw new GraphAbort(); }
                var updatedPosition = new Vector2(frame.X, frame.Y);
                // Transform (2, or 4 fused with output) moves the report itself,
                // as upstream's output mode does.
                if (operation != 3 && report is IAbsolutePositionReport position
                    && (operation is 2 or 4 || originalPosition != updatedPosition))
                    position.Position = updatedPosition;
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

    static GraphReport Export(IDeviceReport report)
    {
        GraphReport f = default;
        f.Version = 2; f.Size = (uint)sizeof(GraphReport);
        f.Kind = report is OutOfRangeReport ? 1u : 0;
        if (report is IAbsolutePositionReport position)
        { Vector2 value = position.Position; f.Flags |= Position; f.X = value.X; f.Y = value.Y; }
        if (report is ITabletReport tablet)
        {
            f.Flags |= Tablet; f.Pressure = tablet.Pressure;
            bool[] buttons = tablet.PenButtons;
            f.PenBits = Pack(buttons); f.PenCount = (uint)buttons.Length;
        }
        if (report is IEraserReport eraser) { f.Flags |= Eraser; f.Eraser = eraser.Eraser ? 1u : 0; }
        if (report is ITiltReport tilt)
        { Vector2 value = tilt.Tilt; f.Flags |= Tilt; f.TiltX = value.X; f.TiltY = value.Y; }
        if (report is IProximityReport proximity)
        { f.Flags |= Proximity; f.Near = proximity.NearProximity ? 1u : 0; f.Distance = proximity.HoverDistance; }
        if (report is IToolReport tool)
        {
            if (tool.Tool is not (OpenTabletDriver.Plugin.Tablet.ToolType.Pen or OpenTabletDriver.Plugin.Tablet.ToolType.Eraser))
                throw new NotSupportedException("Unknown report tool type.");
            f.Flags |= Tool; f.Serial = tool.Serial; f.ToolID = tool.RawToolID; f.ToolType = (uint)tool.Tool;
        }
        if (report is IAuxReport aux)
        { bool[] buttons = aux.AuxButtons; f.Flags |= Aux; f.AuxBits = Pack(buttons); f.AuxCount = (uint)buttons.Length; }
        if (report is IMouseReport mouse)
        {
            bool[] buttons = mouse.MouseButtons; Vector2 scroll = mouse.Scroll;
            f.Flags |= Mouse; f.MouseBits = Pack(buttons); f.MouseCount = (uint)buttons.Length;
            f.ScrollX = scroll.X; f.ScrollY = scroll.Y;
        }
        if (report is IAbsoluteAnalogReport absolute)
        {
            uint?[] values = absolute.AnalogPositions ?? throw new InvalidOperationException("Null analog array.");
            if (values.Length > 16) throw new NotSupportedException("Report exceeds 16 analog channels.");
            f.Flags |= Absolute; if (report is IAbsoluteWheelReport) f.Flags |= AbsoluteWheel;
            f.AbsoluteCount = (uint)values.Length;
            for (int i = 0; i < values.Length; i++) if (values[i] is uint value)
            { f.AbsolutePresent |= 1u << i; f.AbsoluteValues[i] = value; }
        }
        if (report is IRelativeAnalogReport relative)
        {
            int[] values = relative.AnalogDeltas ?? throw new InvalidOperationException("Null analog array.");
            if (values.Length > 16) throw new NotSupportedException("Report exceeds 16 analog channels.");
            f.Flags |= Relative; if (report is IRelativeWheelReport) f.Flags |= RelativeWheel;
            f.RelativeCount = (uint)values.Length;
            for (int i = 0; i < values.Length; i++) f.RelativeValues[i] = values[i];
        }
        if (report is IWheelButtonReport wheel)
        {
            bool[][] values = wheel.WheelButtons ?? throw new InvalidOperationException("Null wheel array.");
            if (values.Length > 8) throw new NotSupportedException("Report exceeds 8 wheels.");
            f.Flags |= WheelButtons; f.WheelCount = (uint)values.Length;
            for (int i = 0; i < values.Length; i++) { f.WheelBits[i] = Pack(values[i]); f.WheelCounts[i] = (uint)values[i].Length; }
        }
        if (report is ITouchReport touch)
        {
            TouchPoint[] values = touch.Touches ?? throw new InvalidOperationException("Null touch array.");
            if (values.Length > 32) throw new NotSupportedException("Report exceeds 32 touches.");
            f.Flags |= Touch; f.TouchCount = (uint)values.Length;
            for (int i = 0; i < values.Length; i++) if (values[i] is { } point)
            {
                f.TouchPresent |= 1u << i; f.TouchIDs[i] = point.TouchID;
                f.TouchXY[i * 2] = point.Position.X; f.TouchXY[i * 2 + 1] = point.Position.Y;
            }
        }
        if (report is PenSnapshot { NativeTipSwitch: bool tip }) { f.Flags |= NativeTip; f.TipSwitch = tip ? 1u : 0; }
        return f;
    }

    static IDeviceReport Import(GraphReport* f)
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
        if (report is IAbsolutePositionReport position) position.Position = new Vector2(f->X, f->Y);
        if (report is ITabletReport tablet)
        { tablet.Pressure = f->Pressure; tablet.PenButtons = Unpack(f->PenBits, f->PenCount); }
        if (report is PenSnapshot pen) pen.NativeTipSwitch = (f->Flags & NativeTip) != 0 ? f->TipSwitch != 0 : null;
        if (report is IEraserReport eraser) eraser.Eraser = f->Eraser != 0;
        if (report is ITiltReport tilt) tilt.Tilt = new Vector2(f->TiltX, f->TiltY);
        if (report is IProximityReport proximity)
        { proximity.NearProximity = f->Near != 0; proximity.HoverDistance = f->Distance; }
        if (report is IToolReport tool)
        {
            if (f->ToolType > 1) throw new ArgumentException("Invalid tool type.");
            tool.Serial = f->Serial; tool.RawToolID = f->ToolID; tool.Tool = (ToolType)f->ToolType;
        }
        if (report is IAuxReport aux) aux.AuxButtons = Unpack(f->AuxBits, f->AuxCount);
        if (report is IMouseReport mouse)
        { mouse.MouseButtons = Unpack(f->MouseBits, f->MouseCount); mouse.Scroll = new Vector2(f->ScrollX, f->ScrollY); }
        if (report is IAbsoluteAnalogReport absolute)
        {
            if (f->AbsoluteCount > 16) throw new ArgumentException("Invalid absolute analog capacity.");
            var positions = new uint?[f->AbsoluteCount];
            for (int i = 0; i < positions.Length; i++) if ((f->AbsolutePresent & (1u << i)) != 0) positions[i] = f->AbsoluteValues[i];
            absolute.AnalogPositions = positions;
        }
        if (report is IRelativeAnalogReport relative)
        {
            if (f->RelativeCount > 16) throw new ArgumentException("Invalid relative analog capacity.");
            var deltas = new int[f->RelativeCount];
            for (int i = 0; i < deltas.Length; i++) deltas[i] = f->RelativeValues[i];
            relative.AnalogDeltas = deltas;
        }
        if (report is IWheelButtonReport wheel)
        {
            if (f->WheelCount > 8) throw new ArgumentException("Invalid wheel capacity.");
            var buttons = new bool[f->WheelCount][];
            for (int i = 0; i < buttons.Length; i++) buttons[i] = Unpack(f->WheelBits[i], f->WheelCounts[i]);
            wheel.WheelButtons = buttons;
        }
        if (report is ITouchReport)
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
        try { GCHandle.FromIntPtr(context).Free(); }
        catch (Exception error) { lastError = error.GetBaseException().Message; }
    }
}

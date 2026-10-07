using System.Diagnostics;
using System.Numerics;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using Newtonsoft.Json.Linq;
using OpenTabletDriver.Plugin;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Platform.Keyboard;
using OpenTabletDriver.Plugin.Platform.Pointer;
using OpenTabletDriver.Plugin.Tablet;

namespace OtdCompat;

// Additive ABI: service requests are owned value records. No plugin or timer
// retains a borrowed Rust callback, report pointer, or stack scope.
[StructLayout(LayoutKind.Sequential)]
public struct ManagedCommand
{
    public uint Kind, Owner;
    public float X, Y;
    public uint Value, Flags;
    public float TiltX, TiltY;
}

sealed class CommandQueue : IDisposable
{
    readonly ManagedCommand[] entries = new ManagedCommand[256];
    readonly object gate = new();
    int head, count;
    bool closed;
    string? failure;
    public uint Owner { get; set; }
    public bool Pending { get { lock (gate) return count != 0 || failure != null; } }
    public void Add(ManagedCommand command)
    {
        lock (gate)
        {
            ObjectDisposedException.ThrowIf(closed, this);
            if (failure != null) throw new InvalidOperationException(failure);
            if (count == entries.Length) { failure = "Managed input service queue exceeded 256 commands."; throw new InvalidOperationException(failure); }
            command.Owner = Owner;
            entries[(head + count++) % entries.Length] = command;
        }
    }
    public bool Take(out ManagedCommand command)
    {
        lock (gate)
        {
            if (failure != null) throw new InvalidOperationException(failure);
            if (count == 0) { command = default; return false; }
            command = entries[head]; head = (head + 1) % entries.Length; count--; return true;
        }
    }
    public void Dispose() { lock (gate) { closed = true; count = 0; } }
}

// Services target the already prepared native session, never an independent
// SendInput injector. Per-plugin owners keep held keys/buttons independent.
sealed class SessionPointer(CommandQueue queue, bool relative, bool pen) : IAbsolutePointer, IRelativePointer,
    ISynchronousPointer, IMouseButtonHandler, IMouseScrollHandler, IPressureHandler,
    ITiltHandler, IEraserHandler, IPenActionHandler
{
    ManagedCommand state = new() { Kind = 0, Flags = relative ? 1u : 0u };
    bool dirty;
    readonly object stateGate = new();
    public void SetPosition(Vector2 position) { lock (stateGate) { state.X = position.X; state.Y = position.Y; state.Flags |= 2; dirty = true; } }
    public void SetPressure(float percentage)
    {
        if (!float.IsFinite(percentage) || percentage < 0 || percentage > 1) throw new ArgumentOutOfRangeException(nameof(percentage));
        lock (stateGate) { state.Value = BitConverter.SingleToUInt32Bits(percentage); state.Flags |= 4; dirty = true; }
    }
    public void SetTilt(Vector2 tilt) { lock (stateGate) { state.TiltX = tilt.X; state.TiltY = tilt.Y; state.Flags |= 8; dirty = true; } }
    public void SetEraser(bool eraser) { lock (stateGate) { state.Flags = (state.Flags & ~16u) | (eraser ? 16u : 0); dirty = true; } }
    // Synthetic Windows pointer injection has no physical hover-distance field.
    // Expose that optional provider only when a backend actually supports it.
    public void SetHoverDistance(uint distance) => throw new NotSupportedException("The native Windows pointer backend has no hover-distance injection field.");
    public void Reset() { lock (stateGate) { state.Flags |= 32; dirty = true; } }
    public void Flush() { lock (stateGate) { if (dirty) { queue.Add(state); dirty = false; state.Flags &= ~32u; } } }
    public void MouseDown(MouseButton button) => Mouse(button, true);
    public void MouseUp(MouseButton button) => Mouse(button, false);
    void Mouse(MouseButton button, bool pressed)
    {
        if (button == MouseButton.None) return;
        if (!Enum.IsDefined(button)) throw new ArgumentOutOfRangeException(nameof(button));
        queue.Add(new() { Kind = 1, Value = (uint)button, Flags = pressed ? 1u : 0u });
    }
    public void ScrollVertically(int amount) => queue.Add(new() { Kind = 3, Value = unchecked((uint)amount) });
    public void ScrollHorizontally(int amount) => queue.Add(new() { Kind = 3, Value = unchecked((uint)amount), Flags = 1 });
    public void Activate(PenAction action) => Pen(action, true);
    public void Deactivate(PenAction action) => Pen(action, false);
    void Pen(PenAction action, bool pressed)
    {
        if (!pen) throw new NotSupportedException("This session has no native pen backend.");
        if (!Enum.IsDefined(action)) throw new ArgumentOutOfRangeException(nameof(action));
        queue.Add(new() { Kind = 4, Value = (uint)action, Flags = pressed ? 1u : 0u });
    }
    public object? Resolve(Type type)
    {
        if (type == typeof(IAbsolutePointer) && !relative || type == typeof(IRelativePointer) && relative
            || type == typeof(ISynchronousPointer) || type == typeof(IMouseButtonHandler) || type == typeof(IMouseScrollHandler)) return this;
        if (pen && (type == typeof(IPenActionHandler) || type == typeof(IPressureHandler) || type == typeof(ITiltHandler) || type == typeof(IEraserHandler))) return this;
        return null;
    }
}

sealed class SessionKeyboard(CommandQueue queue, JObject keys) : IVirtualKeyboard
{
    readonly Dictionary<string, uint> names = keys.Properties().ToDictionary(property => property.Name, property => property.Value.Value<uint>());
    public IEnumerable<string> SupportedKeys => names.Keys;
    public void Press(string key) => Key(key, true);
    public void Release(string key) => Key(key, false);
    public void Press(IEnumerable<string> keys) { foreach (string key in keys) Press(key); }
    public void Release(IEnumerable<string> keys) { foreach (string key in keys.Reverse()) Release(key); }
    void Key(string key, bool pressed)
    {
        if (!names.TryGetValue(key, out uint usage)) throw new NotSupportedException($"Unsupported Windows key '{key}'.");
        queue.Add(new() { Kind = 2, Value = usage, Flags = pressed ? 1u : 0u });
    }
}

abstract class EndpointInstance : IDisposable
{
    readonly PluginLoad context;
    readonly HostServices services;
    readonly int thread = Environment.CurrentManagedThreadId;
    readonly List<SessionTimer> timers = new();
    bool disposed;
    protected object Value { get; }
    public readonly CommandQueue Queue = new();
    protected readonly SessionPointer Pointer;
    protected readonly TabletReference Tablet;
    protected EndpointInstance(JObject config, Type contract)
    {
        string path = Path.GetFullPath(config.Value<string>("assembly_path") ?? throw new ArgumentException("assembly_path missing"));
        if (config.Value<ulong?>("graph_context") is { } graphHandle) {
            var graph = (SynchronousGraph)GCHandle.FromIntPtr((nint)graphHandle).Target!;
            context = new PluginLoad(path, graph.SourceGeneration, frozen: true);
        } else context = new PluginLoad(path);
        Queue.Owner = config.Value<uint>("owner");
        Pointer = new SessionPointer(Queue, config.Value<bool>("relative"), config.Value<bool>("pen"));
        var keyboard = new SessionKeyboard(Queue, config["keys"] as JObject ?? new JObject());
        object? actualPointer = null;
        if (config.Value<ulong?>("output_context") is { } handle)
        {
            var output = (OutputInstance)GCHandle.FromIntPtr((nint)handle).Target!;
            actualPointer = output.Mode switch { AbsoluteOutputMode absolute => absolute.Pointer, RelativeOutputMode relative => relative.Pointer, _ => null };
            // Native providers still get one owner per binding. A plugin's own
            // pointer, such as VMulti, is the exact upstream output dependency.
            if (actualPointer is SessionPointer) actualPointer = null;
        }
        services = new HostServices(() => { CheckThread(); var timer = new SessionTimer(); timers.Add(timer); return timer; },
            type => type == typeof(IVirtualKeyboard) ? keyboard :
                actualPointer != null && IsPointerService(type) && type.IsInstanceOfType(actualPointer) ? actualPointer : Pointer.Resolve(type), authoritativeInput: true);
        object? value = null;
        try
        {
            Tablet = Instance.CreateTabletReference(config);
            Type type = context.LoadFromAssemblyPath(path).GetType(config.Value<string>("type_name") ?? "", true)!;
            PluginEligibility.RequireLoadable(type);
            if (!contract.IsAssignableFrom(type)) throw new NotSupportedException($"'{type.FullName}' does not implement {contract.Name}.");
            value = HostServices.Construct(type) ?? throw new InvalidOperationException("Cannot construct plugin.");
            Value = value;
            services.ConfigureSource(config);
            services.Inject(type, value);
            Instance.ApplySettings(type, value, config["settings"] as JObject ?? new JObject());
            HostServices.Complete(type, value, Tablet);
        }
        catch
        {
            Queue.Dispose();
            try { HostServices.DisposePlugin(value); } finally { foreach (var timer in timers) timer.Dispose(); services.Dispose(); context.Unload(); }
            throw;
        }
    }
    static bool IsPointerService(Type type) => type == typeof(IAbsolutePointer) || type == typeof(IRelativePointer)
        || type == typeof(ISynchronousPointer) || type == typeof(IMouseButtonHandler) || type == typeof(IMouseScrollHandler)
        || type == typeof(IPenActionHandler) || type == typeof(IPressureHandler) || type == typeof(ITiltHandler) || type == typeof(IEraserHandler)
        || type == typeof(IHoverDistanceHandler);
    protected void CheckThread()
    {
        ObjectDisposedException.ThrowIf(disposed, this);
        if (Environment.CurrentManagedThreadId != thread) throw new InvalidOperationException("Managed endpoint lifecycle must run on its owning session thread.");
    }
    public long NextTickMicros()
    {
        CheckThread();
        if (Queue.Pending) return 0;
        long now = Stopwatch.GetTimestamp(), best = -1;
        foreach (var timer in timers.Where(timer => timer.Enabled))
        {
            long micros = (long)Math.Min(long.MaxValue, Math.Max(0, (timer.Due - (double)now) * 1_000_000 / Stopwatch.Frequency));
            if (best < 0 || micros < best) best = micros;
        }
        // A foreign plugin timer can enqueue owned input requests. A bounded
        // poll discovers those without borrowing a live native callback.
        return best < 0 ? 5000 : Math.Min(best, 5000);
    }
    public void Tick()
    {
        using var reportScope = ServiceClient.Report();
        CheckThread(); long now = Stopwatch.GetTimestamp(); int count = timers.Count;
        for (int index = 0; index < count; index++) timers[index].FireIfDue(now);
    }
    public virtual void Dispose()
    {
        if (disposed) return;
        CheckThread(); disposed = true;
        try { HostServices.DisposePlugin(Value); }
        finally { Queue.Dispose(); foreach (var timer in timers) timer.Dispose(); services.Dispose(); context.Unload(); }
    }
}

sealed class BindingInstance(JObject config) : EndpointInstance(config, typeof(IStateBinding))
{
    readonly SynchronousGraph projector = new([], captureRegistry: false);
    IDeviceReport? report;
    bool pressed;
    public unsafe IDeviceReport Project(GraphReport* frame) { CheckThread(); return SynchronousGraph.CurrentReport ?? projector.Import(frame); }
    public void SetReport(IDeviceReport value) { CheckThread(); report = value; }
    public void Set(bool down, uint owner)
    {
        using var reportScope = ServiceClient.Report();
        CheckThread(); Queue.Owner = owner;
        var current = SynchronousGraph.CurrentReport ?? report ?? throw new InvalidOperationException("Binding has no source report.");
        report = current;
        uint? previousOwner = ServiceClient.BindingOwner;
        string? previousTablet = ServiceClient.BindingTablet;
        ServiceClient.BindingOwner = owner; ServiceClient.BindingTablet = Tablet.Properties.Name;
        try {
            if (down) { pressed = true; ((IStateBinding)Value).Press(Tablet, current); }
            else { try { ((IStateBinding)Value).Release(Tablet, current); } finally { pressed = false; } }
        } finally { ServiceClient.BindingOwner = previousOwner; ServiceClient.BindingTablet = previousTablet; }
    }
    public void Release() { if (pressed) Set(false, Queue.Owner); }
    public override void Dispose() { try { Release(); } finally { base.Dispose(); } }
}

sealed class OutputInstance : EndpointInstance
{
    public IOutputMode Mode => (IOutputMode)Value;
    public OutputInstance(JObject config) : base(config, typeof(IOutputMode))
    {
        try
        {
            Queue.Owner = 4096;
            Mode.DisablePressure = config.Value<bool>("disable_pressure"); Mode.DisableTilt = config.Value<bool>("disable_tilt");
            if (Mode is AbsoluteOutputMode absolute)
            {
                if (config.Value<bool>("relative")) throw new NotSupportedException("An absolute output mode requires absolute profile areas.");
                absolute.Input = Area(config["input"] as JObject ?? throw new ArgumentException("input area missing"));
                absolute.Output = Area(config["output"] as JObject ?? throw new ArgumentException("output area missing"));
                absolute.AreaClipping = config.Value<bool>("clipping"); absolute.AreaLimiting = config.Value<bool>("limiting");
                // Preserve a plugin's own pointer (including its real VMulti
                // prerequisites). Supply native services only when it requests one.
                absolute.Pointer ??= Pointer;
            }
            else if (Mode is RelativeOutputMode relative)
            {
                if (!config.Value<bool>("relative")) throw new NotSupportedException("A relative output mode requires profile sensitivity.");
                relative.Sensitivity = new Vector2(config.Value<float>("sensitivity_x"), config.Value<float>("sensitivity_y"));
                relative.Rotation = config.Value<float>("rotation"); relative.ResetTime = TimeSpan.FromMilliseconds(config.Value<double>("reset_ms"));
                relative.Pointer ??= Pointer;
            }
            Mode.Tablet = Tablet;
        }
        catch { Dispose(); throw; }
    }
    static Area Area(JObject value) => new(value.Value<float>("Width"), value.Value<float>("Height"), new Vector2(value.Value<float>("X"), value.Value<float>("Y")), value.Value<float>("Rotation"));
}

public static unsafe partial class EntryPoints
{
    static JObject EndpointConfig(byte* json, nuint length)
    {
        if (json == null || length > 262144) throw new ArgumentException("Invalid endpoint configuration.");
        return JObject.Parse(Encoding.UTF8.GetString(new ReadOnlySpan<byte>(json, checked((int)length))));
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static nint CreateBinding(byte* json, nuint length)
    {
        try { return GCHandle.ToIntPtr(GCHandle.Alloc(new BindingInstance(EndpointConfig(json, length)))); }
        catch (Exception error) { lastError = error.GetBaseException().Message; return 0; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static nint CreateOutput(byte* json, nuint length)
    {
        try { return GCHandle.ToIntPtr(GCHandle.Alloc(new OutputInstance(EndpointConfig(json, length)))); }
        catch (Exception error) { lastError = error.GetBaseException().Message; return 0; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int BindingsReport(nint* contexts, uint count, GraphReport* frame)
    {
        try {
            if (count == 0) return 0;
            if (contexts == null || count > 1024 || frame == null) throw new ArgumentException("Invalid binding report contexts.");
            var first = (BindingInstance)GCHandle.FromIntPtr(contexts[0]).Target!;
            IDeviceReport report = first.Project(frame);
            // Native threshold state mutates this same concrete report before
            // any tip/eraser or side binding sees it. Raw remains unchanged.
            if (report is ITabletReport tablet && (frame->Flags & SynchronousGraph.Tablet) != 0) tablet.Pressure = frame->Pressure;
            for (uint index = 0; index < count; index++) ((BindingInstance)GCHandle.FromIntPtr(contexts[index]).Target!).SetReport(report);
            return 0;
        }
        catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int BindingSet(nint context, uint owner, int pressed)
    {
        try { ((BindingInstance)GCHandle.FromIntPtr(context).Target!).Set(pressed != 0, owner); return 0; }
        catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int BindingRelease(nint context)
    {
        try { ((BindingInstance)GCHandle.FromIntPtr(context).Target!).Release(); return 0; }
        catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int EndpointDrain(nint context, ManagedCommand* commands, uint capacity)
    {
        try
        {
            if (commands == null || capacity > 256) throw new ArgumentException("Invalid service command buffer.");
            var endpoint = (EndpointInstance)GCHandle.FromIntPtr(context).Target!;
            int count = 0;
            while (count < capacity && endpoint.Queue.Take(out var command)) commands[count++] = command;
            return count;
        }
        catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static long EndpointNextTick(nint context)
    {
        try { return ((EndpointInstance)GCHandle.FromIntPtr(context).Target!).NextTickMicros(); }
        catch (Exception error) { lastError = error.GetBaseException().Message; return -3; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int EndpointTick(nint context)
    {
        try { ((EndpointInstance)GCHandle.FromIntPtr(context).Target!).Tick(); return 0; }
        catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static void DestroyEndpoint(nint context)
    {
        try { var handle = GCHandle.FromIntPtr(context); try { ((EndpointInstance)handle.Target!).Dispose(); } finally { handle.Free(); } }
        catch (Exception error) { lastError = error.GetBaseException().Message; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int AttachOutput(nint graphContext, nint outputContext)
    {
        try { ((SynchronousGraph)GCHandle.FromIntPtr(graphContext).Target!).AttachOutput((OutputInstance)GCHandle.FromIntPtr(outputContext).Target!); return 0; }
        catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }
}

using System.Numerics;
using OpenTabletDriver.Desktop.Interop;
using OpenTabletDriver.Plugin.Platform.Keyboard;
using OpenTabletDriver.Plugin.Platform.Pointer;
using ITimer = OpenTabletDriver.Plugin.Timers.ITimer;

namespace OtdCompat;

// Actual pinned platform implementations execute input. This scope owns only
// its held inputs and timers; native graph endpoints retain their typed queues.
sealed class OriginalInputServices : IDisposable
{
    readonly object gate = new();
    readonly List<IDisposable> owned = [];
    ScopedKeyboard? keyboard;
    ScopedAbsolute? absolute;
    ScopedRelative? relative;
    bool disposed;
    internal object? Get(Type type)
    {
        lock (gate) {
            ObjectDisposedException.ThrowIf(disposed, this);
            if (type == typeof(ITimer)) { var value = new ScopedTimer(DesktopInterop.Timer); owned.Add(value); return value; }
            if (type == typeof(IVirtualKeyboard)) {
                if (keyboard == null) { keyboard = new ScopedKeyboard(DesktopInterop.VirtualKeyboard ?? throw new PlatformNotSupportedException("Original virtual keyboard is unavailable on this platform.")); owned.Add(keyboard); }
                return keyboard;
            }
            if (type == typeof(IAbsolutePointer)) {
                if (absolute == null) { absolute = new ScopedAbsolute(DesktopInterop.AbsolutePointer ?? throw new PlatformNotSupportedException("Original absolute pointer is unavailable on this platform.")); owned.Add(absolute); }
                return absolute;
            }
            if (type == typeof(IRelativePointer)) {
                if (relative == null) { relative = new ScopedRelative(DesktopInterop.RelativePointer ?? throw new PlatformNotSupportedException("Original relative pointer is unavailable on this platform.")); owned.Add(relative); }
                return relative;
            }
            if (type == typeof(IPressureHandler)) return DesktopInterop.VirtualTablet;
            if (type == typeof(IVirtualPad)) return DesktopInterop.VirtualPad;
            if (type == typeof(OpenTabletDriver.Plugin.Platform.Display.IVirtualScreen)) return DesktopInterop.VirtualScreen;
            return null;
        }
    }
    public void Dispose() {
        lock (gate) {
            if (disposed) return; disposed = true;
            List<Exception>? failures = null;
            foreach (var value in owned) { try { value.Dispose(); } catch (Exception error) { (failures ??= []).Add(error); } }
            owned.Clear();
            if (failures != null) throw new AggregateException("Original input scope cleanup failed.", failures);
        }
    }
    // Original Windows platform objects are per-service instances. Scoped key
    // ownership also protects multiple managed tools sharing the same OS key.
    static readonly object KeysGate = new(), ButtonsGate = new();
    static readonly Dictionary<string,int> Keys = new(StringComparer.Ordinal);
    static readonly Dictionary<MouseButton,int> Buttons = new();
    sealed class ScopedKeyboard(IVirtualKeyboard original) : IVirtualKeyboard, IDisposable
    {
        readonly HashSet<string> held = new(StringComparer.Ordinal);
        volatile bool disposed;
        public IEnumerable<string> SupportedKeys => original.SupportedKeys;
        public void Press(string key) { lock (KeysGate) {
            ObjectDisposedException.ThrowIf(disposed, this);
            if (held.Contains(key)) { original.Press(key); return; }
            int count = Keys.GetValueOrDefault(key);
            if (count == 0) original.Press(key);
            held.Add(key); Keys[key] = checked(count + 1);
        } }
        public void Release(string key) { lock (KeysGate) {
            if (!held.Remove(key)) return;
            int count = Keys[key] - 1;
            if (count == 0) { Keys.Remove(key); original.Release(key); } else Keys[key] = count;
        } }
        public void Press(IEnumerable<string> keys) { foreach (string key in keys) Press(key); }
        public void Release(IEnumerable<string> keys) { foreach (string key in keys) Release(key); }
        public void Dispose() { lock (KeysGate) { if (disposed) return; disposed = true; foreach (string key in held.ToArray()) Release(key); } }
    }
    abstract class ScopedMouse(object original) : IMouseButtonHandler, IMouseScrollHandler, ISynchronousPointer, IDisposable
    {
        protected readonly object Original = original;
        readonly HashSet<MouseButton> held = [];
        volatile bool disposed;
        protected T Require<T>() => Original is T value ? value : throw new NotSupportedException($"Actual original pointer has no {typeof(T).Name} capability.");
        protected void Check() => ObjectDisposedException.ThrowIf(disposed, this);
        public void MouseDown(MouseButton button) { lock (ButtonsGate) {
            Check(); if (held.Contains(button)) return;
            int count = Buttons.GetValueOrDefault(button);
            if (count == 0) Require<IMouseButtonHandler>().MouseDown(button);
            held.Add(button); Buttons[button] = checked(count + 1);
        } }
        public void MouseUp(MouseButton button) { lock (ButtonsGate) {
            if (!held.Remove(button)) return;
            int count = Buttons[button] - 1;
            if (count == 0) { Buttons.Remove(button); Require<IMouseButtonHandler>().MouseUp(button); } else Buttons[button] = count;
        } }
        public void ScrollVertically(int amount) { Check(); Require<IMouseScrollHandler>().ScrollVertically(amount); }
        public void ScrollHorizontally(int amount) { Check(); Require<IMouseScrollHandler>().ScrollHorizontally(amount); }
        public void Flush() { Check(); if (Original is ISynchronousPointer sync) sync.Flush(); }
        public void Reset() { Check(); if (Original is ISynchronousPointer sync) sync.Reset(); }
        public void Dispose() { lock (ButtonsGate) {
            if (disposed) return;
            foreach (var button in held.ToArray()) MouseUp(button);
            if (Original is ISynchronousPointer sync) sync.Flush();
            disposed = true;
        } }
    }
    sealed class ScopedAbsolute(IAbsolutePointer original) : ScopedMouse(original), IAbsolutePointer
    { public void SetPosition(Vector2 position) { Check(); Require<IAbsolutePointer>().SetPosition(position); } }
    sealed class ScopedRelative(IRelativePointer original) : ScopedMouse(original), IRelativePointer
    { public void SetPosition(Vector2 position) { Check(); Require<IRelativePointer>().SetPosition(position); } }
    sealed class ScopedTimer : ITimer
    {
        readonly ITimer original;
        int disposed;
        internal ScopedTimer(ITimer original) { this.original = original; original.Elapsed += Fire; }
        public bool Enabled => Volatile.Read(ref disposed) == 0 && original.Enabled;
        public float Interval { get => original.Interval; set { ObjectDisposedException.ThrowIf(Volatile.Read(ref disposed) != 0, this); original.Interval = value; } }
        public event Action? Elapsed;
        void Fire() { if (Volatile.Read(ref disposed) != 0) return; using var callback = ServiceClient.Report(); Elapsed?.Invoke(); }
        public void Start() { ObjectDisposedException.ThrowIf(Volatile.Read(ref disposed) != 0, this); original.Start(); }
        public void Stop() => original.Stop();
        public void Dispose() {
            if (Interlocked.Exchange(ref disposed, 1) != 0) return;
            original.Elapsed -= Fire; Elapsed = null;
            // A callback may be awaiting the native settings transaction which
            // retires this scope. It must not be joined by that same transaction.
            _ = Task.Run(() => { try { original.Dispose(); } catch (Exception error) { Console.Error.WriteLine(error); } });
        }
    }
}

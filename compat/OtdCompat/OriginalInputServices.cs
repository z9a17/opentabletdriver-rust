using System.Numerics;
using Newtonsoft.Json.Linq;
using OpenTabletDriver.Desktop.Interop.Input.Keyboard;
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
                if (keyboard == null) { keyboard = new ScopedKeyboard(); owned.Add(keyboard); }
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
    // Key/button transitions enter the same native ownership ledger as tablet
    // bindings. Position/scroll still use the actual pinned platform pointer.
    sealed class InputOwner : IDisposable
    {
        static long identity;
        readonly ulong scope = checked((ulong)Interlocked.Increment(ref identity));
        readonly object gate = new();
        readonly CancellationTokenSource lifetime = new();
        Task? heartbeat;
        bool disposed;
        internal void Send(JObject payload) {
            lock (gate) {
                ObjectDisposedException.ThrowIf(disposed, this);
                payload["lease_ms"] = 15000;
                // This uses the independent native input lane, which never
                // waits for daemon/profile transactions or managed callbacks.
                using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(3));
                ServiceClient.Request(12, scope, payload, deadline.Token).GetAwaiter().GetResult();
                heartbeat ??= Task.Run(Renew);
            }
        }
        async Task Renew() {
            while (!lifetime.IsCancellationRequested) {
                try {
                    await Task.Delay(3000, lifetime.Token).ConfigureAwait(false);
                    lock (gate) {
                        if (disposed) return;
                        using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(3));
                        ServiceClient.Request(12, scope, new JObject { ["type"]="renew",["lease_ms"]=15000 },deadline.Token).GetAwaiter().GetResult();
                    }
                } catch (OperationCanceledException) when (lifetime.IsCancellationRequested) { return; }
                catch (Exception error) { LogFailure(error); return; }
            }
        }
        public void Dispose() {
            lock (gate) {
                if (disposed) return; disposed = true; lifetime.Cancel();
                using var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(3));
                ServiceClient.Request(13,scope,new JObject(),deadline.Token).GetAwaiter().GetResult();
            }
        }
        static void LogFailure(Exception error) => OpenTabletDriver.Plugin.Log.Exception(error);
    }
    sealed class ScopedKeyboard : IVirtualKeyboard, IDisposable
    {
        readonly InputOwner owner = new();
        readonly Dictionary<string,uint> codes;
        readonly string platform;
        internal ScopedKeyboard() {
            if (OperatingSystem.IsWindows()) { platform="windows"; codes=WindowsVirtualKeyboard.EtoKeysymToVK.ToDictionary(pair=>pair.Key,pair=>(uint)pair.Value,StringComparer.Ordinal); }
            else if (OperatingSystem.IsLinux()) { platform="linux"; codes=EvdevVirtualKeyboard.EtoKeysymToEventCode.ToDictionary(pair=>pair.Key,pair=>(uint)pair.Value,StringComparer.Ordinal); }
            else if (OperatingSystem.IsMacOS()) { platform="macos"; codes=MacOSVirtualKeyboard.EtoKeysymToVK.ToDictionary(pair=>pair.Key,pair=>(uint)pair.Value,StringComparer.Ordinal); }
            else throw new PlatformNotSupportedException("Original keyboard platform is unavailable.");
        }
        public IEnumerable<string> SupportedKeys => codes.Keys;
        void Send(string key,bool held) {
            if (!codes.TryGetValue(key,out uint code)) throw new KeyNotFoundException($"Original keyboard has no key '{key}'.");
            owner.Send(new JObject { ["type"]="key",["platform"]=platform,["code"]=code,["key"]=key,["held"]=held });
        }
        public void Press(string key) => Send(key,true);
        public void Release(string key) => Send(key,false);
        public void Press(IEnumerable<string> keys) { foreach (string key in keys) Press(key); }
        public void Release(IEnumerable<string> keys) { foreach (string key in keys) Release(key); }
        public void Dispose() => owner.Dispose();
    }
    abstract class ScopedMouse(object original) : IMouseButtonHandler, IMouseScrollHandler, ISynchronousPointer, IDisposable
    {
        protected readonly object Original = original;
        readonly InputOwner owner = new();
        volatile bool disposed;
        protected T Require<T>() => Original is T value ? value : throw new NotSupportedException($"Actual original pointer has no {typeof(T).Name} capability.");
        protected void Check() => ObjectDisposedException.ThrowIf(disposed,this);
        public void MouseDown(MouseButton button) { Check(); owner.Send(new JObject { ["type"]="button",["code"]=(uint)button,["held"]=true }); }
        public void MouseUp(MouseButton button) { Check(); owner.Send(new JObject { ["type"]="button",["code"]=(uint)button,["held"]=false }); }
        public void ScrollVertically(int amount) { Check(); Require<IMouseScrollHandler>().ScrollVertically(amount); }
        public void ScrollHorizontally(int amount) { Check(); Require<IMouseScrollHandler>().ScrollHorizontally(amount); }
        public void Flush() { Check(); if (Original is ISynchronousPointer sync) sync.Flush(); }
        public void Reset() { Check(); if (Original is ISynchronousPointer sync) sync.Reset(); }
        public void Dispose() { if (disposed) return; disposed=true; owner.Dispose(); }
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

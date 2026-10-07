using System.Numerics;
using System.Reflection;
using System.Runtime.InteropServices;
using OpenTabletDriver.Plugin.Attributes;
using OpenTabletDriver.Plugin.DependencyInjection;
using OpenTabletDriver.Plugin.Platform.Display;
using OpenTabletDriver.Plugin.Tablet;
using ITimer = OpenTabletDriver.Plugin.Timers.ITimer;

namespace OtdCompat;

// Exact-Type lookup matches Desktop/Reflection/ServiceManager at 736003ed.
// Missing services return null; they must never become fabricated devices,
// drivers or no-op input providers. Each plugin owns its service scope.
sealed class HostServices(Func<ITimer>? timer = null, Func<Type, object?>? input = null) : IServiceProvider, IDisposable
{
    IVirtualScreen? display;
    ManagedProviders? providers;
    int disposed;
    public bool ProviderInjected { get; private set; }
    public object? GetService(Type serviceType)
    {
        ObjectDisposedException.ThrowIf(Volatile.Read(ref disposed) != 0, this);
        if (serviceType == typeof(IServiceProvider)) return this;
        if (serviceType == typeof(ITimer)) return timer?.Invoke();
        if (serviceType == typeof(IVirtualScreen) && OperatingSystem.IsWindows())
            return display ??= new WindowsScreen();
        if (input?.Invoke(serviceType) is { } value) return value;
        return (providers ??= new ManagedProviders()).Get(serviceType);
    }

    public void Dispose() { if (Interlocked.Exchange(ref disposed, 1) == 0) providers?.Dispose(); }

    internal void Inject(Type type, object value)
    {
        using var setup = ServiceClient.Setup();
        foreach (MemberInfo member in Members(type))
        {
            if (member.GetCustomAttribute<ResolvedAttribute>() == null) continue;
            Type dependency = member is PropertyInfo property ? property.PropertyType : ((FieldInfo)member).FieldType;
            // Like upstream, an unavailable optional service preserves the
            // constructor value. Plugins can test provider.GetService(null).
            if (GetService(dependency) is { } service)
            {
                Assign(member, value, service);
                if (ReferenceEquals(service, this)) ProviderInjected = true;
            }
        }
    }

    internal static void Complete(Type type, object value, TabletReference? tablet)
    {
        using var setup = ServiceClient.Setup();
        foreach (MemberInfo member in Members(type))
        {
            Type dependency = member is PropertyInfo property ? property.PropertyType : ((FieldInfo)member).FieldType;
            if (dependency != typeof(TabletReference)) continue;
            if (member.GetCustomAttribute<TabletReferenceAttribute>() != null
                || (tablet != null && member.GetCustomAttribute<ResolvedAttribute>() != null))
                Assign(member, value, tablet);
        }
        // Preserve upstream GetMethods order and public inherited callbacks.
        foreach (MethodInfo method in type.GetMethods())
            if (method.GetCustomAttribute<OnDependencyLoadAttribute>() != null)
                method.Invoke(value, []);
    }
    internal static object? Construct(Type type, object[]? arguments = null) {
        using var setup = ServiceClient.Setup();
        return Activator.CreateInstance(type, arguments ?? []);
    }
    internal static void DisposePlugin(object? value) {
        using var setup = ServiceClient.Setup();
        (value as IDisposable)?.Dispose();
    }

    static IEnumerable<MemberInfo> Members(Type type)
    {
        const BindingFlags flags = BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
        for (Type? owner = type; owner != null; owner = owner.BaseType)
        {
            foreach (PropertyInfo property in owner.GetProperties(flags)) yield return property;
            foreach (FieldInfo field in owner.GetFields(flags)) yield return field;
        }
    }

    static void Assign(MemberInfo member, object value, object? service)
    {
        if (member is PropertyInfo property) property.SetValue(value, service);
        else ((FieldInfo)member).SetValue(value, service);
    }
}

// Read-only physical display queries. No process DPI changes: the Rust host
// owns DPI setup. Individual displays and Position are construction snapshots;
// Width/Height are live, exactly as upstream WindowsDisplay exposes them.
unsafe sealed class WindowsScreen : IVirtualScreen
{
    readonly record struct Monitor(float Width, float Height, Vector2 Position, bool Primary);
    sealed record Display(int Index, float Width, float Height, Vector2 Position) : IDisplay;
    public int Index => 0;
    public Vector2 Position { get; }
    public IEnumerable<IDisplay> Displays { get; }
    public float Width => Extent(ReadMonitors(), horizontal: true);
    public float Height => Extent(ReadMonitors(), horizontal: false);

    internal WindowsScreen()
    {
        List<Monitor> monitors = ReadMonitors();
        Monitor primary = monitors.FirstOrDefault(monitor => monitor.Primary);
        Position = primary.Position - new Vector2(monitors.Min(m => m.Position.X), monitors.Min(m => m.Position.Y));
        Displays = new IDisplay[] { this }.Concat(monitors.Select((monitor, index) =>
            (IDisplay)new Display(index + 1, monitor.Width, monitor.Height, monitor.Position))).ToArray();
    }

    static float Extent(List<Monitor> monitors, bool horizontal) => horizontal
        ? monitors.Max(m => m.Position.X + m.Width) - monitors.Min(m => m.Position.X)
        : monitors.Max(m => m.Position.Y + m.Height) - monitors.Min(m => m.Position.Y);

    [StructLayout(LayoutKind.Sequential)]
    struct Rect { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct MonitorInfo
    {
        public uint Size;
        public Rect Monitor, Work;
        public uint Flags;
        public fixed char Device[32];
    }
    delegate bool Enumerate(nint monitor, nint dc, ref Rect rectangle, nint data);
    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool EnumDisplayMonitors(nint dc, nint clip, Enumerate callback, nint data);
    [DllImport("user32.dll", EntryPoint = "GetMonitorInfoW", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool GetMonitorInfo(nint monitor, ref MonitorInfo info);
    [DllImport("user32.dll", EntryPoint = "EnumDisplaySettingsW", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool EnumDisplaySettings(char* device, int mode, byte* settings);

    static List<Monitor> ReadMonitors()
    {
        var monitors = new List<Monitor>();
        string? failure = null;
        Enumerate callback = (nint handle, nint dc, ref Rect rectangle, nint data) =>
        {
            var info = new MonitorInfo { Size = (uint)sizeof(MonitorInfo) };
            if (!GetMonitorInfo(handle, ref info)) { failure = "GetMonitorInfo failed."; return false; }
            // DEVMODEW has a fixed 220-byte public Win32 layout. Initialize
            // dmSize and query current physical pixels, independent of DPI.
            byte* mode = stackalloc byte[220];
            new Span<byte>(mode, 220).Clear();
            *(ushort*)(mode + 68) = 220;
            if (!EnumDisplaySettings(info.Device, -1, mode)) { failure = "EnumDisplaySettings failed."; return false; }
            monitors.Add(new Monitor(*(uint*)(mode + 172), *(uint*)(mode + 176),
                new Vector2(*(int*)(mode + 76), *(int*)(mode + 80)), (info.Flags & 1) != 0));
            return true;
        };
        if (!EnumDisplayMonitors(0, 0, callback, 0) || failure != null || monitors.Count == 0)
            throw new InvalidOperationException(failure ?? "Display enumeration returned no monitors.");
        return monitors;
    }
}

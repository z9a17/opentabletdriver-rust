using System.Numerics;
using System.Reflection;
using System.Text.Json.Nodes;
using OpenTabletDriver.Configurations;
using OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV2;
using OpenTabletDriver.Desktop.Binding;
using OpenTabletDriver.Desktop.Interop.Input.Absolute;
using OpenTabletDriver.Desktop.Output;
using OpenTabletDriver.Plugin;
using OpenTabletDriver.Plugin.Attributes;
using OpenTabletDriver.Plugin.DependencyInjection;
using OpenTabletDriver.Plugin.Output;
using OpenTabletDriver.Plugin.Platform.Pointer;
using OpenTabletDriver.Plugin.Tablet;

namespace OtdUpstreamBench;

/// <summary>The profile and trace exported by the Rust harness.</summary>
sealed class Workload
{
    public required byte[][] Reports { get; init; }
    public required JsonObject Json { get; init; }
    public required Area Display { get; init; }
    public required Area TabletArea { get; init; }
    public bool Clipping { get; init; }
    public bool Limiting { get; init; }
    public float TipPercent { get; init; }
    public required JsonObject RadialFollow { get; init; }
    public Vector2 RelativeSensitivity { get; init; }
    public float RelativeRotation { get; init; }
    public TimeSpan RelativeReset { get; init; }

    internal static Area ReadArea(JsonNode area) => new(
        area["width"]!.GetValue<float>(),
        area["height"]!.GetValue<float>(),
        new Vector2(area["x"]!.GetValue<float>(), area["y"]!.GetValue<float>()),
        area["rotation"]!.GetValue<float>());

    public static Workload Load(string path)
    {
        var json = JsonNode.Parse(File.ReadAllText(path))!.AsObject();
        var trace = json["trace"]!;
        int size = trace["report_bytes"]!.GetValue<int>();
        byte[] bytes = File.ReadAllBytes(Path.Combine(Path.GetDirectoryName(Path.GetFullPath(path))!, trace["file"]!.GetValue<string>()));
        ulong hash = 0xcbf29ce484222325;
        foreach (byte value in bytes)
            hash = (hash ^ value) * 0x100000001b3;
        if (hash.ToString("x16") != trace["fnv1a64"]!.GetValue<string>())
            throw new InvalidDataException("trace.bin does not match workload.json");
        var reports = Enumerable.Range(0, bytes.Length / size).Select(i => bytes.AsSpan(i * size, size).ToArray()).ToArray();
        var absolute = json["absolute"]!;
        var relative = json["relative"]!;
        return new Workload
        {
            Reports = reports,
            Json = json,
            Display = ReadArea(absolute["display"]!),
            TabletArea = ReadArea(absolute["tablet"]!),
            Clipping = absolute["clipping"]!.GetValue<bool>(),
            Limiting = absolute["limiting"]!.GetValue<bool>(),
            TipPercent = json["tip_threshold_percent"]!.GetValue<float>(),
            RadialFollow = json["radial_follow"]!.AsObject(),
            RelativeSensitivity = new Vector2(relative["x_sensitivity"]!.GetValue<float>(), relative["y_sensitivity"]!.GetValue<float>()),
            RelativeRotation = relative["rotation"]!.GetValue<float>(),
            RelativeReset = TimeSpan.FromMilliseconds(relative["reset_ms"]!.GetValue<double>()),
        };
    }
}

/// <summary>
/// One report's path in the daemon: <c>DeviceReader.Main</c> parses the
/// buffer and raises <c>Report</c>; <c>InputDeviceTree.HandleReport</c> takes
/// the tree's lock and calls <c>OutputMode.Read</c>.
/// </summary>
sealed class Reader
{
    readonly IReportParser<IDeviceReport> parser = new IntuosV2ReportParser();
    readonly object sync = new();
    readonly IOutputMode outputMode;

    public event EventHandler<IDeviceReport>? Report;

    public Reader(IOutputMode outputMode)
    {
        this.outputMode = outputMode;
        Report += HandleReport;
    }

    void HandleReport(object? sender, IDeviceReport report)
    {
        lock (sync)
            outputMode.Read(report);
    }

    public void OnData(byte[] data)
    {
        if (parser.Parse(data) is IDeviceReport report)
            Report?.Invoke(this, report);
    }
}

/// <summary>Keeps positions and discards buttons; nothing reaches Windows.</summary>
sealed class NullPointer : IAbsolutePointer, IRelativePointer, IMouseButtonHandler, ISynchronousPointer
{
    public Vector2 Position;
    public int Buttons;
    public void SetPosition(Vector2 pos) => Position = pos;
    public void MouseDown(MouseButton button) => Buttons++;
    public void MouseUp(MouseButton button) => Buttons--;
    public void Flush() { }
    public void Reset() { }
}

/// <summary>
/// Upstream's Windows pointer for moves; button changes are dropped so a
/// benchmark never clicks. Times each Flush, which is the SendInput call.
/// </summary>
sealed unsafe class MoveOnlyWindowsPointer : IAbsolutePointer, IMouseButtonHandler, ISynchronousPointer
{
    readonly WindowsAbsolutePointer inner = new();
    public ulong LastFlushTicks;
    public void SetPosition(Vector2 pos) => inner.SetPosition(pos);
    public void MouseDown(MouseButton button) { }
    public void MouseUp(MouseButton button) { }
    public void Flush()
    {
        ulong begin = Clock.Start();
        inner.Flush();
        LastFlushTicks = Clock.Stop() - begin;
    }
    public void Reset() => inner.Reset();
}

static class Pipelines
{
    public static readonly TabletReference Tablet = CreateTablet();

    static TabletReference CreateTablet()
    {
        var configuration = new DeviceConfigurationProvider().TabletConfigurations.Single(c => c.Name == "Wacom PTH-660");
        return new TabletReference(configuration, configuration.DigitizerIdentifiers.Take(1));
    }

    /// <summary>
    /// <c>DriverDaemon.CreateBindingHandler</c> for a profile that binds the
    /// tip and the eraser to the left button at the given thresholds.
    /// </summary>
    public static BindingHandler Bindings(float tipPercent, float eraserPercent, IMouseButtonHandler buttons) => new(Tablet)
    {
        Tip = new ThresholdBindingState
        {
            Binding = new MouseBinding { Pointer = buttons, Button = nameof(MouseButton.Left) },
            ActivationThreshold = tipPercent,
        },
        Eraser = new ThresholdBindingState
        {
            Binding = new MouseBinding { Pointer = buttons, Button = nameof(MouseButton.Left) },
            ActivationThreshold = eraserPercent,
        },
    };

    /// <summary>
    /// <c>DriverDaemon.SetSettings</c> for absolute mode: areas, clipping,
    /// then the enabled filters followed by the binding handler.
    /// </summary>
    public static Reader Absolute(
        Area display, Area tablet, bool clipping, bool limiting, float tipPercent, float eraserPercent,
        IAbsolutePointer pointer, IMouseButtonHandler buttons, IPositionedPipelineElement<IDeviceReport>? filter)
    {
        var mode = new AbsoluteMode { Pointer = pointer, Tablet = Tablet };
        mode.Output = display;
        mode.Input = tablet;
        mode.AreaClipping = clipping;
        mode.AreaLimiting = limiting;
        mode.Tablet = Tablet;
        var elements = new List<IPositionedPipelineElement<IDeviceReport>>();
        if (filter != null)
            elements.Add(filter);
        elements.Add(Bindings(tipPercent, eraserPercent, buttons));
        mode.Elements = elements;
        return new Reader(mode);
    }

    public static Reader Absolute(Workload workload, IAbsolutePointer pointer, IMouseButtonHandler buttons, IPositionedPipelineElement<IDeviceReport>? filter) =>
        Absolute(workload.Display, workload.TabletArea, workload.Clipping, workload.Limiting, workload.TipPercent, workload.TipPercent, pointer, buttons, filter);

    /// <summary><c>DriverDaemon.SetSettings</c> for relative mode.</summary>
    public static Reader Relative(
        Vector2 sensitivity, float rotation, TimeSpan reset, float tipPercent, float eraserPercent,
        IRelativePointer pointer, IMouseButtonHandler buttons)
    {
        var mode = new RelativeMode
        {
            Pointer = pointer,
            Tablet = Tablet,
            Sensitivity = sensitivity,
            Rotation = rotation,
            ResetTime = reset,
        };
        mode.Elements = [Bindings(tipPercent, eraserPercent, buttons)];
        return new Reader(mode);
    }

    public static Reader Relative(Workload workload, NullPointer pointer) =>
        Relative(workload.RelativeSensitivity, workload.RelativeRotation, workload.RelativeReset, workload.TipPercent, workload.TipPercent, pointer, pointer);

    /// <summary>
    /// Constructs an unchanged plugin filter as the daemon's plugin store does:
    /// [Property] values or their defaults, the tablet reference, then
    /// [OnDependencyLoad] methods.
    /// </summary>
    public static IPositionedPipelineElement<IDeviceReport> Filter(string assemblyPath, string typeName, JsonObject settings)
    {
        var type = Assembly.LoadFrom(assemblyPath).GetType(typeName, true)!;
        object filter = Activator.CreateInstance(type)!;
        foreach (var property in type.GetProperties().Where(p => p.GetCustomAttribute<PropertyAttribute>() != null && p.CanWrite))
        {
            if (settings[property.Name] is JsonNode value)
                property.SetValue(filter, Convert.ChangeType(value.GetValue<double>(), property.PropertyType));
            else if (property.GetCustomAttribute<DefaultPropertyValueAttribute>() is { } defaults)
                property.SetValue(filter, defaults.Value);
        }
        foreach (var property in type.GetProperties())
        {
            bool injected = property.GetCustomAttribute<ResolvedAttribute>() != null
                || property.GetCustomAttribute<TabletReferenceAttribute>() != null;
            if (injected && property.PropertyType == typeof(TabletReference))
                property.SetValue(filter, Tablet);
        }
        foreach (var method in type.GetMethods().Where(m => m.GetCustomAttribute<OnDependencyLoadAttribute>() != null))
            method.Invoke(filter, []);
        return (IPositionedPipelineElement<IDeviceReport>)filter;
    }
}

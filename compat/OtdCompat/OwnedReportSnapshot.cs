using System.Reflection;
using System.Runtime.CompilerServices;
using OpenTabletDriver.Plugin.Tablet;

namespace OtdCompat;

// Foreign Emit callbacks surrender an owned value, not a borrowed object which
// a plugin can mutate while its downstream consumer runs on another thread.
// Reflection touches fields only: arbitrary getters/serialization are never
// executed while taking the snapshot. Concrete SDK/plugin type identity stays
// intact, including private state in boxed original report structs.
static class OwnedReportSnapshot
{
    internal const int MaxBytes = 256 * 1024;
    const int MaxObjects = 2048, MaxDepth = 24, MaxFields = 256;
    static readonly MethodInfo clone = typeof(object).GetMethod("MemberwiseClone", BindingFlags.Instance | BindingFlags.NonPublic)!;
    // Ephemeron keys allow collectible original plugin assemblies to retire.
    static readonly ConditionalWeakTable<Type, Shape> shapes = new();
    sealed class Width(int bytes) { internal readonly int Bytes = bytes; }
    static readonly ConditionalWeakTable<Type, Width> widths = new();
    static int ManagedWidth<T>() => Unsafe.SizeOf<T>();
    static readonly MethodInfo width = typeof(OwnedReportSnapshot).GetMethod(nameof(ManagedWidth), BindingFlags.Static | BindingFlags.NonPublic)!;
    static int Storage(Type type) => type.IsValueType
        ? widths.GetValue(type, static key => new Width((int)width.MakeGenericMethod(key).Invoke(null, null)!)).Bytes
        : IntPtr.Size;
    sealed class Shape
    {
        internal readonly FieldInfo[] Fields;
        internal readonly long Bytes;
        internal Shape(Type type)
        {
            var fields = new List<FieldInfo>();
            for (Type? current = type; current != null; current = current.BaseType)
                fields.AddRange(current.GetFields(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly));
            if (fields.Count > MaxFields) throw new NotSupportedException("Asynchronous report has too many instance fields.");
            Fields = fields.ToArray();
            long fieldStorage = Fields.Sum(field => (long)Math.Max(16, Storage(field.FieldType)));
            Bytes = 24L + Math.Max(fieldStorage, type.IsValueType ? Storage(type) : type.StructLayoutAttribute?.Size ?? 0);
        }
    }
    sealed class Copier
    {
        readonly Dictionary<object, object> seen = new(ReferenceEqualityComparer.Instance);
        internal int Bytes;
        int objects;
        void Charge(long bytes)
        {
            if (bytes < 0 || bytes > MaxBytes - Bytes) throw new NotSupportedException("Asynchronous report exceeds the 256 KiB owned-data limit.");
            Bytes += (int)bytes;
        }
        internal object? Copy(object? source, int depth = 0)
        {
            if (source == null) return null;
            Type type = source.GetType();
            if (source is string text) { Charge(24L + text.Length * 2L); return source; }
            if (type == typeof(IntPtr) || type == typeof(UIntPtr))
                throw new NotSupportedException("Asynchronous report contains an unmanaged address.");
            if (type.IsPrimitive || type.IsEnum || source is decimal or DateTime or DateTimeOffset or TimeSpan or Guid) return source;
            if (seen.TryGetValue(source, out object? previous)) return previous;
            if (depth >= MaxDepth || ++objects > MaxObjects) throw new NotSupportedException("Asynchronous report object graph exceeds its bounded depth/count.");
            if (source is Delegate or Task or Thread or Stream or Type or MemberInfo or CancellationTokenSource or System.Runtime.InteropServices.SafeHandle or IServiceProvider
                || type.IsPointer || type.IsByRefLike || type == typeof(IntPtr) || type == typeof(UIntPtr))
                throw new NotSupportedException($"Asynchronous report contains live execution/device state: {type.FullName}.");
            if (source is Array array)
            {
                if (array.Rank != 1 || array.GetLowerBound(0) != 0)
                    throw new NotSupportedException("Asynchronous report arrays must be zero-based and one-dimensional.");
                Type element = type.GetElementType()!;
                if (element.IsPointer || element == typeof(IntPtr) || element == typeof(UIntPtr))
                    throw new NotSupportedException("Asynchronous report contains an unmanaged address array.");
                // Actual CLR value storage, not Marshal.SizeOf or a guessed
                // struct width: a plugin can define a very large value type.
                int storage = Storage(element);
                Charge(24L + array.LongLength * storage);
                Array owned = (Array)array.Clone();
                seen.Add(source, owned);
                if (!element.IsPrimitive && !element.IsEnum)
                    for (int i = 0; i < array.Length; i++) owned.SetValue(Copy(array.GetValue(i), depth + 1), i);
                return owned;
            }
            Shape shape = shapes.GetValue(type, static key => new Shape(key));
            Charge(shape.Bytes);
            object value = clone.Invoke(source, null)!;
            seen.Add(source, value);
            foreach (FieldInfo field in shape.Fields)
            {
                if (field.FieldType.IsPointer || field.FieldType.IsByRefLike)
                    throw new NotSupportedException($"Asynchronous report contains an unmanaged field: {field.Name}.");
                object? original = field.GetValue(source);
                object? copied = Copy(original, depth + 1);
                if (!ReferenceEquals(original, copied)) field.SetValue(value, copied);
            }
            return value;
        }
    }
    internal static IDeviceReport Capture(IDeviceReport? report, out int bytes)
    {
        ArgumentNullException.ThrowIfNull(report);
        var copier = new Copier();
        var owned = (IDeviceReport)copier.Copy(report)!;
        bytes = copier.Bytes;
        return owned;
    }
}

using System.Linq.Expressions;
using System.Numerics;
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
    static readonly Func<object, object> memberwiseClone = (Func<object, object>)Delegate.CreateDelegate(
        typeof(Func<object, object>),
        null,
        typeof(object).GetMethod("MemberwiseClone", BindingFlags.Instance | BindingFlags.NonPublic)!);
    // Ephemeron keys allow collectible original plugin assemblies to retire.
    static readonly ConditionalWeakTable<Type, Shape> shapes = new();
    sealed class Width(int bytes) { internal readonly int Bytes = bytes; }
    static readonly ConditionalWeakTable<Type, Width> widths = new();
    static readonly ConditionalWeakTable<Type, StrongBox<bool>> inlineValueCache = new();
    static int ManagedWidth<T>() => Unsafe.SizeOf<T>();
    static readonly MethodInfo width = typeof(OwnedReportSnapshot).GetMethod(nameof(ManagedWidth), BindingFlags.Static | BindingFlags.NonPublic)!;
    static readonly MethodInfo copyFieldMethod = typeof(Copier).GetMethod(nameof(Copier.Copy), BindingFlags.Instance | BindingFlags.NonPublic)!;
    static int Storage(Type type) => type.IsValueType
        ? widths.GetValue(type, static key => new Width((int)width.MakeGenericMethod(key).Invoke(null, null)!)).Bytes
        : IntPtr.Size;

    static bool IsInlineImmutableValue(Type type)
    {
        if (!type.IsValueType) return false;
        if (type == typeof(IntPtr) || type == typeof(UIntPtr) || type.IsPointer || type.IsByRefLike) return false;
        if (type.IsPrimitive || type.IsEnum
            || type == typeof(decimal) || type == typeof(DateTime) || type == typeof(DateTimeOffset)
            || type == typeof(TimeSpan) || type == typeof(Guid)
            || type == typeof(Vector2) || type == typeof(Vector3) || type == typeof(Vector4))
            return true;
        return inlineValueCache.GetValue(type, static key =>
        {
            for (Type? current = key; current != null && current != typeof(ValueType); current = current.BaseType)
            {
                foreach (FieldInfo field in current.GetFields(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly))
                {
                    if (!IsInlineImmutableValue(field.FieldType)) return new StrongBox<bool>(false);
                }
            }
            return new StrongBox<bool>(true);
        }).Value;
    }

    static byte[]? CloneBytes(byte[]? source, ref int bytes)
    {
        if (source == null) return null;
        long charge = 24L + source.LongLength;
        if (charge > MaxBytes - bytes) throw new NotSupportedException("Asynchronous report exceeds the 256 KiB owned-data limit.");
        bytes += (int)charge;
        if (source.Length == 0) return [];
        byte[] copy = new byte[source.Length];
        Buffer.BlockCopy(source, 0, copy, 0, source.Length);
        return copy;
    }

    static bool[]? CloneBools(bool[]? source, ref int bytes)
    {
        if (source == null) return null;
        long charge = 24L + source.LongLength;
        if (charge > MaxBytes - bytes) throw new NotSupportedException("Asynchronous report exceeds the 256 KiB owned-data limit.");
        bytes += (int)charge;
        if (source.Length == 0) return [];
        bool[] copy = new bool[source.Length];
        Array.Copy(source, copy, source.Length);
        return copy;
    }

    sealed class Shape
    {
        internal readonly FieldInfo[] ReferenceFields;
        internal readonly Action<object, object, Copier, int>? CopyReferenceFields;
        internal readonly long Bytes;
        internal readonly string? UnmanagedError;
        internal Shape(Type type)
        {
            var fields = new List<FieldInfo>();
            for (Type? current = type; current != null; current = current.BaseType)
                fields.AddRange(current.GetFields(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly));
            if (fields.Count > MaxFields) throw new NotSupportedException("Asynchronous report has too many instance fields.");
            long fieldStorage = 0;
            var referenceFields = new List<FieldInfo>();
            foreach (FieldInfo field in fields)
            {
                if (field.FieldType == typeof(IntPtr) || field.FieldType == typeof(UIntPtr))
                {
                    UnmanagedError = "Asynchronous report contains an unmanaged address.";
                    break;
                }
                if (field.FieldType.IsPointer || field.FieldType.IsByRefLike)
                {
                    UnmanagedError = $"Asynchronous report contains an unmanaged field: {field.Name}.";
                    break;
                }
                fieldStorage += Math.Max(16, Storage(field.FieldType));
                if (!IsInlineImmutableValue(field.FieldType)) referenceFields.Add(field);
            }
            ReferenceFields = referenceFields.ToArray();
            Bytes = 24L + Math.Max(fieldStorage, type.IsValueType ? Storage(type) : type.StructLayoutAttribute?.Size ?? 0);
            if (UnmanagedError == null && !type.IsValueType && ReferenceFields.Length > 0)
            {
                CopyReferenceFields = CompileFieldCopier(type, ReferenceFields);
            }
        }

        static Action<object, object, Copier, int>? CompileFieldCopier(Type type, FieldInfo[] fields)
        {
            // Expressions cannot assign initonly fields directly; fall back to FieldInfo.SetValue for those.
            foreach (FieldInfo field in fields)
                if (field.IsInitOnly) return null;
            try
            {
                var sourceParam = Expression.Parameter(typeof(object), "source");
                var targetParam = Expression.Parameter(typeof(object), "target");
                var copierParam = Expression.Parameter(typeof(Copier), "copier");
                var depthParam = Expression.Parameter(typeof(int), "depth");
                var typedSource = Expression.Variable(type, "s");
                var typedTarget = Expression.Variable(type, "t");
                var origVar = Expression.Variable(typeof(object), "orig");
                var copiedVar = Expression.Variable(typeof(object), "copied");
                var body = new List<Expression>
                {
                    Expression.Assign(typedSource, Expression.Convert(sourceParam, type)),
                    Expression.Assign(typedTarget, Expression.Convert(targetParam, type)),
                };
                var nextDepth = Expression.Add(depthParam, Expression.Constant(1));
                foreach (FieldInfo field in fields)
                {
                    Expression sourceAccess = field.DeclaringType == type
                        ? Expression.Field(typedSource, field)
                        : Expression.Field(Expression.Convert(typedSource, field.DeclaringType!), field);
                    Expression targetAccess = field.DeclaringType == type
                        ? Expression.Field(typedTarget, field)
                        : Expression.Field(Expression.Convert(typedTarget, field.DeclaringType!), field);
                    body.Add(Expression.Assign(origVar, Expression.Convert(sourceAccess, typeof(object))));
                    body.Add(Expression.Assign(copiedVar, Expression.Call(copierParam, copyFieldMethod, origVar, nextDepth)));
                    body.Add(Expression.IfThen(
                        Expression.Not(Expression.ReferenceEqual(origVar, copiedVar)),
                        Expression.Assign(targetAccess, Expression.Convert(copiedVar, field.FieldType))));
                }
                var block = Expression.Block([typedSource, typedTarget, origVar, copiedVar], body);
                return Expression.Lambda<Action<object, object, Copier, int>>(
                    block, sourceParam, targetParam, copierParam, depthParam).Compile();
            }
            catch
            {
                return null;
            }
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
            if (IsInlineImmutableValue(type)) return source;
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
                if (!IsInlineImmutableValue(element))
                    for (int i = 0; i < array.Length; i++) owned.SetValue(Copy(array.GetValue(i), depth + 1), i);
                return owned;
            }
            Shape shape = shapes.GetValue(type, static key => new Shape(key));
            if (shape.UnmanagedError != null)
                throw new NotSupportedException(shape.UnmanagedError);
            Charge(shape.Bytes);
            object value = memberwiseClone(source);
            seen.Add(source, value);
            if (shape.CopyReferenceFields is { } compiled)
            {
                compiled(source, value, this, depth);
                return value;
            }
            foreach (FieldInfo field in shape.ReferenceFields)
            {
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
        Type type = report.GetType();
        if (report is OutOfRangeReport oor)
        {
            bytes = 40;
            byte[]? raw = CloneBytes(oor.Raw, ref bytes);
            return new OutOfRangeReport(raw!);
        }
        if (report is PenSnapshot && type.Assembly == typeof(PenSnapshot).Assembly)
        {
            bytes = 128;
            var cloned = (PenSnapshot)memberwiseClone(report);
            cloned.Raw = CloneBytes(cloned.Raw, ref bytes)!;
            cloned.PenButtons = CloneBools(cloned.PenButtons, ref bytes)!;
            if (cloned is AuxPenSnapshot auxPen)
                auxPen.AuxButtons = CloneBools(auxPen.AuxButtons, ref bytes)!;
            return cloned;
        }
        if (ReferenceEquals(type, typeof(TabletReport)))
        {
            bytes = 128;
            var cloned = (TabletReport)memberwiseClone(report);
            cloned.Raw = CloneBytes(cloned.Raw, ref bytes)!;
            cloned.PenButtons = CloneBools(cloned.PenButtons, ref bytes)!;
            return cloned;
        }
        if (ReferenceEquals(type, typeof(AuxReport)))
        {
            bytes = 96;
            var cloned = (AuxReport)memberwiseClone(report);
            cloned.Raw = CloneBytes(cloned.Raw, ref bytes)!;
            cloned.AuxButtons = CloneBools(cloned.AuxButtons, ref bytes)!;
            return cloned;
        }
        var copier = new Copier();
        var owned = (IDeviceReport)copier.Copy(report)!;
        bytes = copier.Bytes;
        return owned;
    }
}

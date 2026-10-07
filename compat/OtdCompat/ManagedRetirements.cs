using System.Collections.Concurrent;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using Newtonsoft.Json.Linq;

namespace OtdCompat;

// Disposal cancels inline; cold callers may await all work belonging to this
// exact disposal. AsyncLocal follows child retirements without joining a native
// apply transaction from a managed callback or disposing thread.
static class ManagedRetirements
{
    internal sealed class Batch
    {
        readonly object gate = new();
        readonly List<Task> tasks = [];
        internal void Add(Task task) { lock (gate) tasks.Add(task); }
        internal async Task Drain() {
            int count = 0;
            List<Exception>? failures = null;
            for (;;) {
                Task[] pending;
                lock (gate) { if (count == tasks.Count) break; pending=tasks.Skip(count).ToArray();count=tasks.Count; }
                try { await Task.WhenAll(pending).ConfigureAwait(false); }
                catch { foreach (var task in pending) if (task.Exception != null) (failures ??= []).AddRange(task.Exception.InnerExceptions); }
            }
            if (failures != null) throw new AggregateException("Owned managed retirement failed.",failures);
        }
    }
    static readonly AsyncLocal<Batch?> active = new();
    internal static Batch? Current => active.Value;
    static readonly ConcurrentDictionary<ulong,Task> receipts = new();
    static long identity;
    internal static IDisposable Enter(Batch? batch) {
        var previous=active.Value;active.Value=batch;return new Restore(previous);
    }
    sealed class Restore(Batch? previous) : IDisposable { public void Dispose()=>active.Value=previous; }
    internal static void Track(Task task) {
        if (active.Value is { } batch) batch.Add(task);
        else Observe(task);
    }
    static async void Observe(Task task) { try { await task.ConfigureAwait(false); } catch (Exception error) { OpenTabletDriver.Plugin.Log.Exception(error); } }
    internal static ulong Retain(Batch batch) {
        long id=Interlocked.Increment(ref identity);
        if (id<=0) throw new InvalidOperationException("Managed retirement identity exhausted.");
        if (!receipts.TryAdd((ulong)id,batch.Drain())) throw new InvalidOperationException("Duplicate retirement identity.");
        return (ulong)id;
    }
    internal static int Wait(ulong receipt,uint timeout) {
        ServiceClient.RequireBlockingAllowed();
        if (timeout>60000 || receipt==0 || !receipts.TryGetValue(receipt,out var task)) throw new ArgumentException("Invalid retirement receipt/timeout.");
        if (!task.Wait(TimeSpan.FromMilliseconds(timeout))) return 1;
        task.GetAwaiter().GetResult();return 0;
    }
    internal static void Release(ulong receipt) {
        if (!receipts.TryGetValue(receipt,out var task)) return;
        if (!task.IsCompleted) throw new InvalidOperationException("Pending retirement must remain retained.");
        receipts.TryRemove(receipt,out _);
    }
}

public static unsafe partial class EntryPoints
{
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static nint CreateToolRetained(byte* json,nuint length,ulong* retirement)
    {
        var batch=new ManagedRetirements.Batch();
        try {
            if (retirement==null) throw new ArgumentException("Retirement output is required.");
            *retirement=0;
            if (json==null || length is 0 or >131072) throw new ArgumentException("Invalid tool configuration.");
            using var owned=ManagedRetirements.Enter(batch);
            var settings=JObject.Parse(Encoding.UTF8.GetString(new ReadOnlySpan<byte>(json,(int)length)));
            return GCHandle.ToIntPtr(GCHandle.Alloc(new ToolInstance(settings)));
        } catch (Exception error) {
            lastError=error.GetBaseException().Message;
            if (retirement!=null) *retirement=ManagedRetirements.Retain(batch);
            return 0;
        }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static ulong DestroyToolRetained(nint handle)
    {
        var batch=new ManagedRetirements.Batch();
        try {
            using var owned=ManagedRetirements.Enter(batch);
            var lease=GCHandle.FromIntPtr(handle);
            try { ((ToolInstance)lease.Target!).Dispose(); }
            catch (Exception error) { batch.Add(Task.FromException(error)); }
            finally { lease.Free(); }
            return ManagedRetirements.Retain(batch);
        } catch (Exception error) { lastError=error.GetBaseException().Message;return 0; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int WaitManagedRetirement(ulong receipt,uint timeout)
    { try { return ManagedRetirements.Wait(receipt,timeout); } catch(Exception error) {lastError=error.GetBaseException().Message;return -1;} }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int ReleaseManagedRetirement(ulong receipt)
    { try {ManagedRetirements.Release(receipt);return 0;} catch(Exception error) {lastError=error.GetBaseException().Message;return -1;} }
}

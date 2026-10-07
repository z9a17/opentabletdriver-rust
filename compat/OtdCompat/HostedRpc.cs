using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using OpenTabletDriver.Desktop.Contracts;
using OpenTabletDriver.Desktop.RPC;

namespace OtdCompat;

public static unsafe partial class EntryPoints
{
    static readonly object RpcGate = new();
    static readonly Dictionary<nint, HostedRpc> RpcHosts = new();
    static long rpcIdentity;
    sealed class HostedRpc : IDisposable
    {
        readonly CancellationTokenSource cancellation = new();
        readonly TaskCompletionSource ready = new(TaskCreationOptions.RunContinuationsAsynchronously);
        readonly TaskCompletionSource completion = new(TaskCreationOptions.RunContinuationsAsynchronously);
        readonly Thread worker;
        int disposed;
        internal HostedRpc(string pipe)
        {
            worker=new Thread(()=>Run(pipe)) { IsBackground=true,Name="Original OpenTabletDriver RPC owner" };
            worker.Start();
            try {ready.Task.GetAwaiter().GetResult();}
            catch {cancellation.Cancel();worker.Join(5000);throw;}
        }
        void Run(string pipe)
        {
            ManagedProviders? provider=null;
            OpenTabletDriver.Instance? instance=null;
            List<Exception> failures=[];
            try {
                if (pipe=="OpenTabletDriver.Daemon") {
                    instance=new OpenTabletDriver.Instance(pipe);
                    if (instance.AlreadyExists) throw new IOException("Original OpenTabletDriver daemon instance already exists.");
                }
                provider=new ManagedProviders();
                var host=new RpcHost<IDriverDaemon>(pipe) {HostedUpdateResponseFlushed=()=>provider.Call("FinishUpdate")};
                // Original Run binds its first listener before the initial await.
                var serving=host.Run(provider,cancellation.Token);
                if (serving.IsFaulted) serving.GetAwaiter().GetResult();
                ready.TrySetResult();
                serving.GetAwaiter().GetResult();
            } catch(Exception error) {ready.TrySetException(error);failures.Add(error);}
            finally {
                var batch=new ManagedRetirements.Batch();
                using(var owned=ManagedRetirements.Enter(batch)) {
                    try {provider?.Dispose();} catch(Exception error) {failures.Add(error);}
                }
                try {
                    var drained=batch.Drain();
                    if (!drained.Wait(TimeSpan.FromSeconds(15))) throw new TimeoutException("Hosted original RPC dependencies did not retire.");
                    drained.GetAwaiter().GetResult();
                } catch(Exception error) {failures.Add(error);}
                try {instance?.HostedDispose();} catch(Exception error) {failures.Add(error);}
                if (failures.Count==0) completion.TrySetResult();
                else completion.TrySetException(new AggregateException("Original RPC owner retirement failed.",failures));
            }
        }
        public void Dispose()
        {
            cancellation.Cancel();
            if (!worker.Join(20000)) throw new TimeoutException("Original RPC owner remains live; retirement did not complete.");
            completion.Task.GetAwaiter().GetResult();
            if (Interlocked.Exchange(ref disposed,1)==0) cancellation.Dispose();
        }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static nint StartHostedRpc(byte* pipe, uint length)
    {
        try {
            ServiceClient.RequireBlockingAllowed();
            if (!ServiceClient.Available) throw new NotSupportedException("A native service owner must be installed before hosting original RPC.");
            if (pipe == null || length is < 1 or > 512) throw new ArgumentException("Invalid hosted pipe name.");
            string name = new UTF8Encoding(false, true).GetString(new ReadOnlySpan<byte>(pipe, (int)length));
            if (name.IndexOf('\0') >= 0) throw new ArgumentException("Pipe name contains NUL.");
            lock (RpcGate) {
                if (RpcHosts.Count >= 4) throw new InvalidOperationException("At most four hosted original RPC owners are permitted.");
                long identity = checked(++rpcIdentity);
                nint token = checked((nint)identity);
                if (token <= 0) throw new InvalidOperationException("Hosted RPC identity exhausted.");
                var owner = new HostedRpc(name); RpcHosts.Add(token, owner); return token;
            }
        } catch (Exception error) { lastError = error.GetBaseException().Message; return 0; }
    }
    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    public static int StopHostedRpc(nint token)
    {
        try {
            ServiceClient.RequireBlockingAllowed();
            if (token <= 0) throw new ArgumentException("Invalid hosted RPC token.");
            HostedRpc? owner;
            lock (RpcGate) RpcHosts.TryGetValue(token,out owner);
            owner?.Dispose();
            lock (RpcGate) { if (RpcHosts.TryGetValue(token,out var current) && ReferenceEquals(current,owner)) RpcHosts.Remove(token); }
            return 0;
        } catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }
}

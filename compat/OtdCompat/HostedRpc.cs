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
        readonly ManagedProviders provider = new();
        readonly Task serving;
        internal HostedRpc(string pipe)
        {
            var host = new RpcHost<IDriverDaemon>(pipe);
            try {
                // Run binds its first listening pipe before its initial await.
                serving = host.Run(provider, cancellation.Token);
                if (serving.IsFaulted) serving.GetAwaiter().GetResult();
            } catch { cancellation.Cancel(); provider.Dispose(); cancellation.Dispose(); throw; }
        }
        public void Dispose()
        {
            cancellation.Cancel();
            try { serving.GetAwaiter().GetResult(); }
            finally { provider.Dispose(); cancellation.Dispose(); }
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
            lock (RpcGate) RpcHosts.Remove(token, out owner);
            owner?.Dispose(); return 0;
        } catch (Exception error) { lastError = error.GetBaseException().Message; return -1; }
    }
}

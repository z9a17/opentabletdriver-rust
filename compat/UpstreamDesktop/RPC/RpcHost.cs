using System;
using System.Collections.Generic;
using System.IO;
using System.IO.Pipes;
using System.Threading;
using System.Threading.Tasks;
using OpenTabletDriver.Plugin;
using StreamJsonRpc;

namespace OpenTabletDriver.Desktop.RPC
{
    public class RpcHost<T>(string pipeName)
        where T : class
    {
        public event EventHandler<bool> ConnectionStateChanged;

        public async Task Run(T host, CancellationToken ct)
        {
            // Native hosting must own every connected client's lifetime, not
            // return from shutdown while fire-and-forget dispatch is still live.
            var clients = new List<Task>();
            try
            {
                while (!ct.IsCancellationRequested)
                {
                    var stream = CreateStream();
                    try { await stream.WaitForConnectionAsync(ct).ConfigureAwait(false); }
                    catch { await stream.DisposeAsync().ConfigureAwait(false); throw; }
                    clients.RemoveAll(task => task.IsCompleted);
                    clients.Add(RespondToRpcRequestAsync(host, stream, ct));
                }
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested) { }
            finally { await Task.WhenAll(clients).ConfigureAwait(false); }
        }

        private async Task RespondToRpcRequestAsync(T host, NamedPipeServerStream stream, CancellationToken ct)
        {
            try
            {
                using var rpc = new JsonRpc(stream, stream, host);
                rpc.ExceptionStrategy = ExceptionProcessing.ISerializable;
                ConnectionStateChanged?.Invoke(this, true);
                rpc.StartListening();
                await rpc.Completion.WaitAsync(ct);
            }
            catch (TaskCanceledException) { } // ignore exceptions caused by daemon shutting down
            catch (Exception ex)
            {
                Log.Exception(ex);
            }

            try { ConnectionStateChanged?.Invoke(this, false); }
            finally { await stream.DisposeAsync().ConfigureAwait(false); }
        }

        private NamedPipeServerStream CreateStream()
        {
            return new NamedPipeServerStream(
                pipeName,
                PipeDirection.InOut,
                NamedPipeServerStream.MaxAllowedServerInstances,
                PipeTransmissionMode.Byte,
                PipeOptions.Asynchronous | PipeOptions.WriteThrough | PipeOptions.CurrentUserOnly
            );
        }
    }
}

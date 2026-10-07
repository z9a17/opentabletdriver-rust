using System;
using System.Collections.Generic;
using System.Collections.Concurrent;
using System.IO;
using System.IO.Pipes;
using System.Threading;
using System.Threading.Tasks;
using OpenTabletDriver.Plugin;
using StreamJsonRpc;
using StreamJsonRpc.Protocol;

namespace OpenTabletDriver.Desktop.RPC
{
    public class RpcHost<T>(string pipeName)
        where T : class
    {
        public event EventHandler<bool> ConnectionStateChanged;
        // Called only after the successful original InstallUpdate response has
        // been written AND flushed. Admission returns a task; shutdown may not
        // be awaited by the same response writer that it is going to retire.
        internal Func<Task> HostedUpdateResponseFlushed;

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
                using var handler = new FlushedResponseHandler(new HeaderDelimitedMessageHandler(stream, stream), HostedUpdateResponseFlushed);
                using var rpc = new JsonRpc(handler, host);
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

        private sealed class FlushedResponseHandler(IJsonRpcMessageHandler inner, Func<Task> updateFlushed)
            : IJsonRpcMessageHandler, IDisposable
        {
            private readonly ConcurrentDictionary<RequestId, byte> updates = new();
            public bool CanRead => inner.CanRead;
            public bool CanWrite => inner.CanWrite;
            public IJsonRpcMessageFormatter Formatter => inner.Formatter;
            public async ValueTask<JsonRpcMessage> ReadAsync(CancellationToken cancellationToken)
            {
                var message = await inner.ReadAsync(cancellationToken).ConfigureAwait(false);
                if (updateFlushed != null && message is JsonRpcRequest request && request.Method == "InstallUpdate" && !request.RequestId.IsEmpty)
                {
                    if (updates.Count >= 16) throw new InvalidOperationException("Too many outstanding update requests.");
                    if (!updates.TryAdd(request.RequestId, 0)) throw new InvalidOperationException("Duplicate outstanding update request identity.");
                }
                return message;
            }
            public async ValueTask WriteAsync(JsonRpcMessage message, CancellationToken cancellationToken)
            {
                // IJsonRpcMessageHandler.WriteAsync's exact pinned contract is
                // write+flush; an event raised before serialization is insufficient.
                await inner.WriteAsync(message, cancellationToken).ConfigureAwait(false);
                if (message is JsonRpcError packetError) updates.TryRemove(packetError.RequestId, out _);
                if (message is JsonRpcResult result && updates.TryRemove(result.RequestId, out _))
                {
                    try { Observe(updateFlushed()); } catch (Exception error) { Log.Exception(error); }
                }
            }
            private static async void Observe(Task completion)
            {
                try { await completion.ConfigureAwait(false); } catch (Exception error) { Log.Exception(error); }
            }
            public void Dispose() { updates.Clear(); (inner as IDisposable)?.Dispose(); }
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

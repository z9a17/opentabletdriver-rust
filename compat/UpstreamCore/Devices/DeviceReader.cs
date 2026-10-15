using System;
using System.IO;
using System.Threading;
using System.Threading.Tasks;
using OpenTabletDriver.Plugin;
using OpenTabletDriver.Plugin.Devices;
using OpenTabletDriver.Plugin.Tablet;

namespace OpenTabletDriver.Devices
{
    public class DeviceReader<T> : IDisposable where T : class, IDeviceReport
    {
        public DeviceReader(IDeviceEndpoint endpoint, IReportParser<T> reportParser)
        {
            Endpoint = endpoint;
            Parser = reportParser ?? throw new ArgumentNullException(nameof(reportParser));
            workerThread = new Thread(Main)
            {
                Name = "OpenTabletDriver Device Reader",
                IsBackground = true,
                Priority = ThreadPriority.AboveNormal
            };
        }

        internal Func<IDisposable> HostedReportScope;
        private readonly TaskCompletionSource hostedCompletion = new(TaskCreationOptions.RunContinuationsAsynchronously);
        internal Task HostedCompletion => (workerThread.ThreadState & ThreadState.Unstarted) != 0 ? Task.CompletedTask : hostedCompletion.Task;
        private readonly Thread workerThread;
        private bool initialized, connected;

        /// <summary>
        /// The device endpoint in which is reporting data in the <see cref="ReportStream"/>.
        /// </summary>
        public IDeviceEndpoint Endpoint { protected set; get; }

        /// <summary>
        /// The raw device endpoint report stream.
        /// </summary>
        public IDeviceEndpointStream ReportStream { protected set; get; }

        /// <summary>
        /// The <see cref="IReportParser{T}"/> in which the device reports will be parsed with.
        /// </summary>
        public IReportParser<T> Parser { private set; get; }

        /// <summary>
        /// Whether to deliver the parsed report to raw observers before processing.
        /// </summary>
        public bool RawClone { set; get; }

        /// <summary>
        /// Invoked when a new report comes in from the device.
        /// </summary>
        public event EventHandler<T> Report;

        /// <summary>
        /// Invoked when a new report comes in from the device.
        /// </summary>
        /// <remarks>
        /// This will only be invoked when <see cref="RawClone"/> is set to true.
        /// This report is not meant in any way to be modified, as it is supposed to represent the original data.
        /// </remarks>
        public event EventHandler<T> RawReport;

        /// <summary>
        /// Whether the device is actively emitting reports and being parsed.
        /// </summary>
        public bool Connected
        {
            protected set
            {
                connected = value;
                ConnectionStateChanged?.Invoke(this, Connected);
            }
            get => connected;
        }

        /// <summary>
        /// Invoked when <see cref="Connected"/> is changed.
        /// </summary>
        public event EventHandler<bool> ConnectionStateChanged;

        protected virtual bool Initialize()
        {
            try
            {
                ReportStream = Endpoint.Open();
                return true;
            }
            catch (Exception ex)
            {
                Log.Exception(ex);
                return false;
            }
        }

        protected virtual void Start()
        {
            if (!initialized)
                initialized = Initialize();

            if (initialized)
                workerThread.Start();
        }

        protected void Main()
        {
            byte[] data = null;
            try
            {
                Connected = true;
                while (Connected)
                {
                    data = ReportStream.Read();
                    using var hostedCallback = HostedReportScope?.Invoke();
                    T parsed;
                    try
                    {
                        parsed = Parser.Parse(data);
                    }
                    catch (IndexOutOfRangeException ex)
                    {
                        Log.Write("Device", $"Unable to parse tablet data {FormatReport(data)}", LogLevel.Warning);
                        Log.Exception(ex);
                        parsed = new DeviceReport(data) as T;
                    }

                    if (parsed is not { } report) continue;
                    // Stateful parsers must advance once, even with the debugger open.
                    // Raw observers snapshot before filters can mutate the report.
                    if (RawClone)
                        OnRawReport(report);
                    OnReport(report);
                }
            }
            catch (ObjectDisposedException dex)
            {
                Log.Debug("Device", $"{(string.IsNullOrWhiteSpace(dex.ObjectName) ? "A device stream" : dex.ObjectName)} was disposed.");
            }
            catch (IOException ioex) when (ioex.Message == "I/O disconnected." || ioex.Message == "Operation failed after some time.")
            {
                Log.Write("Device", "Device disconnected.");
            }
            catch (ArgumentOutOfRangeException)
            {
                Log.Write("Device", "Not enough report data returned by the device. Was it disconnected?");
            }
            catch (Exception ex)
            {
                Log.Exception(ex);
                Log.Write("Device", $"Last report read from tablet: {FormatReport(data)}");
            }
            finally
            {
                try { Connected = false; }
                finally { hostedCompletion.TrySetResult(); }
            }
        }

        private static string FormatReport(byte[] data) =>
            data == null ? "<null>" : "{ " + BitConverter.ToString(data).Replace('-', ' ') + " }";

        protected virtual void OnReport(T report) => Report?.Invoke(this, report);
        protected virtual void OnRawReport(T report) => RawReport?.Invoke(this, report);

        public void Dispose()
        {
            Dispose(true);
            GC.SuppressFinalize(this);
        }

        private bool _isDisposed;

        protected virtual void Dispose(bool disposing)
        {
            if (_isDisposed) return;

            if (disposing)
            {
                Connected = false;
                ReportStream?.Dispose();
            }

            _isDisposed = true;
        }
    }
}

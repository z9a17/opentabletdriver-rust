using System;
using System.Collections.Generic;
using System.Diagnostics.CodeAnalysis;
using System.Reflection;
using System.Runtime.Serialization;
using JetBrains.Annotations;
using Newtonsoft.Json;
using Newtonsoft.Json.Serialization;
using OpenTabletDriver.Plugin;
using OpenTabletDriver.Plugin.Attributes;
using OpenTabletDriver.Plugin.Devices;
using OpenTabletDriver.Plugin.Logging;

namespace OpenTabletDriver.Desktop.Diagnostics
{
    public class DiagnosticInfo
    {
        internal static Func<string> HostedAppVersion;
        internal static Func<string> HostedBuildDate;
        private static string GetBuildDate()
        {
            if (Assembly.GetEntryAssembly() == null && HostedBuildDate != null) return HostedBuildDate();
            var attribute = typeof(BuildDateAttribute).Assembly.GetCustomAttribute<BuildDateAttribute>();
            if (attribute != null) return attribute.BuildDate;
            return HostedBuildDate?.Invoke() ?? throw new InvalidOperationException("Build provenance is unavailable.");
        }
        public DiagnosticInfo(IEnumerable<LogMessage> log, IEnumerable<SerializedDeviceEndpoint> devices)
        {
            ConsoleLog = log;
            Devices = devices;
        }

        [JsonProperty("App Version")]
        public string AppVersion { private set; get; } = GetAppVersion();

        [JsonProperty("Build Date")]
        public string BuildDate { private set; get; } = GetBuildDate();

        [JsonProperty("Operating System")]
        public static OSInfo OperatingSystem => OSInfo.GetOSInfo();

        [JsonProperty("Environment Variables")]
        public IDictionary<string, string> EnvironmentVariables { private set; get; } = new EnvironmentDictionary();

        [JsonProperty("HID Devices")]
        public IEnumerable<SerializedDeviceEndpoint> Devices { private set; get; }

        [JsonProperty("Console Log")]
        public IEnumerable<LogMessage> ConsoleLog { private set; get; }

        private static string GetAppVersion()
        {
            var entry = Assembly.GetEntryAssembly();
            if (entry == null)
                return HostedAppVersion?.Invoke() ?? throw new InvalidOperationException("Native application provenance is unavailable.");
            string version = entry.GetCustomAttribute<AssemblyInformationalVersionAttribute>().InformationalVersion;
            return $"OpenTabletDriver v{version}";
        }

        [OnError, UsedImplicitly]
        [SuppressMessage("Performance", "CA1822:Mark members as static")] // unclear if [OnError] works when static, so let's err on the side of caution
        internal void OnError(StreamingContext _, ErrorContext errorContext)
        {
            errorContext.Handled = true;
            Log.Write("Diagnostics", $"Handled diagnostics serialization error", LogLevel.Error);
            Log.Exception(errorContext.Error);
        }

        public override string ToString()
        {
            return JsonConvert.SerializeObject(this, Formatting.Indented);
        }
    }
}

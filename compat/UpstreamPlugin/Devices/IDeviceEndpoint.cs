using System.Collections.Generic;

namespace OpenTabletDriver.Plugin.Devices
{
    public interface IDeviceEndpoint
    {
        int ProductID { get; }
        int VendorID { get; }
        int InputReportLength { get; }
        int OutputReportLength { get; }
        int FeatureReportLength { get; }

        string? Manufacturer { get; }
        string? ProductName { get; }
        string? FriendlyName { get; }
        string? SerialNumber { get; }

        string DevicePath { get; }
        bool CanOpen { get; }
        IDictionary<string, string> DeviceAttributes { get; }

        IDeviceEndpointStream Open();

        string? GetDeviceString(byte index);
        // Default bodies preserve type loading for unchanged 0.6.7 endpoint
        // implementations. New initialization requires an explicit implementation:
        // Open() alone does not establish ownership of a disposable stream.
        void OpenAndSendFeatureInit(byte[] initData) => throw new System.NotSupportedException(
            "This legacy plugin endpoint does not implement feature initialization. Update the endpoint plugin to support this operation.");
        void OpenAndSendOutputInit(byte[] initData) => throw new System.NotSupportedException(
            "This legacy plugin endpoint does not implement output initialization. Update the endpoint plugin to support this operation.");
    }
}

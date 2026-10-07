using OpenTabletDriver.Plugin.Devices;
namespace OpenTabletDriver
{
    // Internal marker only: the native reader owns initialization and physical I/O.
    internal interface IHostedDeviceEndpoint : IDeviceEndpoint { }
}

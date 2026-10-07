# Bluetooth HID transport source

The pinned original HidSharpEndpoint wraps every HidDevice without a USB-only
transport predicate. Linux hidraw and macOS IOHID already carry input, output,
feature, removal and cancellation operations independently of USB descriptors.
Native enumeration now accepts Bluetooth HID and keeps its actual transport.
Core matches Bluetooth HID with the same actual identifier constraints; a
missing required USB string/interface is still a mismatch, never approximated.

Apple defines Bluetooth and BluetoothLowEnergy transport values in
[IOHIDKeys.h](https://github.com/apple-oss-distributions/IOHIDFamily/blob/main/IOHIDFamily/IOHIDKeys.h).
The actual PhysicalDeviceUniqueID property is documented in
[IOHIDDeviceKeys.h](https://github.com/apple-oss-distributions/IOHIDFamily/blob/main/IOHIDFamily/IOHIDDeviceKeys.h).
Native macOS uses that physical identity across HID collections; when absent,
independent registry endpoint IDs prevent two same-model devices merging.
USB parents and indexed descriptor requests are used only for USB-backed
endpoints. macOS inventory preserves actual transport and HID product strings.

Linux BUS_BLUETOOTH is 0x0005. HID_PHYS/HID_UNIQ expose actual local/remote
Bluetooth addresses. A valid remote address and adapter path join collections;
input suffixes do not split one physical tablet, different addresses never
coalesce, and missing/invalid identity falls back to the concrete HID sysfs path.
Non-USB nodes no longer enter USB descriptor discovery. Inventory uses actual
HID_NAME/HID_UNIQ when a USB descriptor product/serial is absent.

Selection/identity fixtures were written but not executed. No suites, builds,
apps, drivers, daemons, plugins or hardware were run. Parent package compilation
and owner native hardware validation remain separate evidence requirements.

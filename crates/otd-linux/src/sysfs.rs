//! Parsing for what Linux exposes about a hidraw node in sysfs. The layout of
//! a USB tablet interface is
//! `/sys/devices/.../usb1/1-2/1-2:1.0/0003:056A:0357.0001/hidraw/hidraw0`:
//! the USB device, its interface, the HID device, then the hidraw node.

use std::path::{Path, PathBuf};

/// `BUS_USB` in `HID_ID`.
pub const BUS_USB: u16 = 0x0003;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HidId {
    pub bus: u16,
    pub vendor: u16,
    pub product: u16,
}

/// Reads `HID_ID=0003:0000056A:00000357` from a HID device's `uevent`.
pub fn hid_id(uevent: &str) -> Option<HidId> {
    let value = uevent
        .lines()
        .find_map(|line| line.strip_prefix("HID_ID="))?;
    let mut parts = value.trim().split(':');
    let mut next = || u32::from_str_radix(parts.next()?, 16).ok();
    let (bus, vendor, product) = (next()?, next()?, next()?);
    Some(HidId {
        bus: u16::try_from(bus).ok()?,
        vendor: u16::try_from(vendor).ok()?,
        product: u16::try_from(product).ok()?,
    })
}

/// The USB interface number of a canonical hidraw sysfs path, as upstream's
/// `HidSharpEndpoint.GetDeviceAttributesLinux` reads it: the digits after the
/// last dot of the interface directory (`1-2:1.0` gives `0`).
pub fn interface_number(hidraw: &Path) -> Option<String> {
    let components: Vec<&str> = hidraw.iter().filter_map(|part| part.to_str()).collect();
    let [.., interface, _hid, class, _node] = components.as_slice() else {
        return None;
    };
    if *class != "hidraw" || !interface.contains(':') {
        return None;
    }
    let digits = interface.rsplit_once('.')?.1;
    (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| digits.to_owned())
}

/// The USB device directory above a canonical hidraw sysfs path. Every
/// interface of one tablet shares it, so it identifies the physical device.
pub fn usb_device(hidraw: &Path) -> Option<PathBuf> {
    let device = hidraw.ancestors().nth(4)?;
    // A USB device directory is named like `1-2` or `1-2.4`, without a colon.
    let name = device.file_name()?.to_str()?;
    (!name.contains(':') && name.contains('-')).then(|| device.to_path_buf())
}

/// Decodes a USB string descriptor: length, type 3, then UTF-16LE text.
pub fn string_descriptor(bytes: &[u8]) -> Option<String> {
    let length = usize::from(*bytes.first()?).min(bytes.len());
    if length < 2 || bytes[1] != 3 {
        return None;
    }
    let units: Vec<u16> = bytes[2..length]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NODE: &str =
        "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-2/1-2:1.2/0003:056A:0357.0004/hidraw/hidraw3";

    #[test]
    fn uevent_gives_bus_vendor_and_product() {
        let uevent =
            "DRIVER=wacom\nHID_ID=0003:0000056A:00000357\nHID_NAME=Wacom Co.,Ltd. PTH-660\n";
        assert_eq!(
            hid_id(uevent),
            Some(HidId {
                bus: BUS_USB,
                vendor: 0x056a,
                product: 0x0357
            })
        );
        assert_eq!(hid_id("HID_NAME=x\n"), None);
    }

    #[test]
    fn interface_and_physical_device_come_from_the_path() {
        let node = Path::new(NODE);
        assert_eq!(interface_number(node).as_deref(), Some("2"));
        assert_eq!(
            usb_device(node),
            Some(PathBuf::from(
                "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-2"
            ))
        );
        // A Bluetooth or virtual HID device has no USB interface directory.
        let virtual_node =
            Path::new("/sys/devices/virtual/misc/uhid/0005:056A:0357.0009/hidraw/hidraw5");
        assert_eq!(interface_number(virtual_node), None);
        assert_eq!(usb_device(virtual_node), None);
    }

    #[test]
    fn string_descriptors_decode_utf16() {
        assert_eq!(
            string_descriptor(&[8, 3, b'P', 0, b'T', 0, b'H', 0]).as_deref(),
            Some("PTH")
        );
        assert_eq!(string_descriptor(&[4, 2, 0, 0]), None);
        assert_eq!(string_descriptor(&[]), None);
    }
}

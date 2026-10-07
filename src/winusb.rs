//! Native Windows WinUSB transport matching the pinned OpenTabletDriver hub.
//! Descriptor/setup requests are bounded and cancel-safe; steady input reuses
//! the session's fixed buffer and OVERLAPPED operation.

mod descriptor;

use std::{collections::BTreeMap, io, ptr, mem::size_of};

use otd_core::{endpoint_match::{Endpoint, Transport}, tablets::{Database, DeviceIdentifier, TabletConfiguration}};
use windows_sys::{core::GUID, Win32::{
    Devices::{DeviceAndDriverInstallation::{SetupDiGetClassDevsW, SetupDiEnumDeviceInterfaces,
        DIGCF_PRESENT, DIGCF_DEVICEINTERFACE, SP_DEVICE_INTERFACE_DATA},
        Usb::{WINUSB_INTERFACE_HANDLE, WINUSB_SETUP_PACKET, WINUSB_PIPE_INFORMATION,
            USB_INTERFACE_DESCRIPTOR, WinUsb_Initialize, WinUsb_Free, WinUsb_QueryInterfaceSettings,
            WinUsb_QueryPipe, WinUsb_ReadPipe, WinUsb_WritePipe, WinUsb_ControlTransfer,
            WinUsb_GetOverlappedResult, WinUsb_FlushPipe}},
    Foundation::{INVALID_HANDLE_VALUE, ERROR_NO_MORE_ITEMS, ERROR_IO_PENDING,
        WAIT_OBJECT_0, WAIT_TIMEOUT, DuplicateHandle, DUPLICATE_SAME_ACCESS},
    System::{IO::{OVERLAPPED, CancelIoEx}, Threading::{WaitForMultipleObjects, WaitForSingleObject, GetCurrentProcess}},
}};

use crate::hid::{self, Candidate, Event, OwnedHandle};

pub const INTERFACE_GUIDS: [GUID; 2] = [
    GUID { data1: 0xdee824ef, data2: 0x729b, data3: 0x4a0e, data4: [0x9c, 0x14, 0xb7, 0x11, 0x7d, 0x33, 0xa8, 0x17] },
    GUID { data1: 0x62f12d4c, data2: 0x3431, data3: 0x4efd, data4: [0x8d, 0xd7, 0x8e, 0x9a, 0xab, 0x18, 0xd3, 0x0c] },
];

/// Owns a duplicated file handle, so every interface keeps its backing file
/// alive. Drop frees WinUSB before Rust drops the owned handle.
pub struct Interface {
    raw: WINUSB_INTERFACE_HANDLE,
    file: OwnedHandle,
    number: u8,
    input: Option<(u8, u16)>,
    output: Option<(u8, u16)>,
}

impl Interface {
    pub fn open(file: &OwnedHandle) -> io::Result<Self> {
        let process = unsafe { GetCurrentProcess() };
        let mut duplicate = ptr::null_mut();
        if unsafe { DuplicateHandle(process, file.raw(), process, &mut duplicate, 0, 0,
            DUPLICATE_SAME_ACCESS) } == 0 { return Err(io::Error::last_os_error()); }
        let file = OwnedHandle::new(duplicate)?;
        let mut raw = ptr::null_mut();
        if unsafe { WinUsb_Initialize(file.raw(), &mut raw) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut this = Self { raw, file, number: 0, input: None, output: None };
        let mut interface = USB_INTERFACE_DESCRIPTOR::default();
        if unsafe { WinUsb_QueryInterfaceSettings(raw, 0, &mut interface) } == 0 {
            return Err(io::Error::last_os_error());
        }
        this.number = interface.bInterfaceNumber;
        for index in 0..interface.bNumEndpoints {
            let mut pipe = WINUSB_PIPE_INFORMATION::default();
            if unsafe { WinUsb_QueryPipe(raw, 0, index, &mut pipe) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let target = if pipe.PipeId & 0x80 != 0 { &mut this.input } else { &mut this.output };
            if target.is_some() || pipe.MaximumPacketSize == 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData,
                    "WinUSB HID interface has multiple pipes in one direction or a zero packet size"));
            }
            *target = Some((pipe.PipeId, pipe.MaximumPacketSize));
        }
        Ok(this)
    }

    pub fn input_length(&self) -> u16 { self.input.map_or(0, |(_, length)| length) }
    /// Drop cached input only after the old reader has drained its requests.
    pub fn flush_input(&self) -> io::Result<()> {
        let pipe = self.input.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "WinUSB interface has no input pipe"))?.0;
        if unsafe { WinUsb_FlushPipe(self.raw, pipe) } == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }
    fn output_length(&self) -> u16 { self.output.map_or(0, |(_, length)| length) }

    /// Callers retain operation and buffer storage until completion/cancellation.
    pub unsafe fn read(&self, buffer: &mut [u8], operation: &OVERLAPPED) -> io::Result<bool> {
        let pipe = self.input.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData,
            "WinUSB interface has no input pipe"))?.0;
        let started = unsafe { WinUsb_ReadPipe(self.raw, pipe, buffer.as_mut_ptr(), buffer.len() as u32,
            ptr::null_mut(), operation) };
        if started != 0 { return Ok(true); }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) { Ok(false) } else { Err(error) }
    }

    pub fn completed(&self, operation: &OVERLAPPED, wait: bool) -> io::Result<u32> {
        let mut length = 0;
        if unsafe { WinUsb_GetOverlappedResult(self.raw, operation, &mut length, i32::from(wait)) } == 0 {
            Err(io::Error::last_os_error())
        } else { Ok(length) }
    }

    pub fn cancel(&self, operation: &OVERLAPPED) {
        unsafe { CancelIoEx(self.file.raw(), operation) };
        // The event and buffers cannot be destroyed until cancellation completes.
        let _ = self.completed(operation, true);
    }

    fn transfer(&self, stop: Option<&Event>, timeout: u32,
        start: impl FnOnce(&OVERLAPPED) -> bool) -> io::Result<u32> {
        if let Some(stop) = stop {
            match unsafe { WaitForSingleObject(stop.raw(), 0) } {
                WAIT_OBJECT_0 => return Err(cancelled()),
                WAIT_TIMEOUT => {},
                _ => return Err(io::Error::last_os_error()),
            }
        }
        let event = Event::create(true)?;
        let operation = OVERLAPPED { hEvent: event.raw(), ..Default::default() };
        if start(&operation) { return self.completed(&operation, false); }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) { return Err(error); }
        let handles = [stop.map_or(event.raw(), Event::raw), event.raw()];
        let wait = unsafe { WaitForMultipleObjects(if stop.is_some() { 2 } else { 1 },
            if stop.is_some() { handles.as_ptr() } else { handles[1..].as_ptr() }, 0, timeout) };
        let completion = WAIT_OBJECT_0 + u32::from(stop.is_some());
        if wait == completion { return self.completed(&operation, false); }
        let error = if stop.is_some() && wait == WAIT_OBJECT_0 { cancelled() }
            else if wait == WAIT_TIMEOUT { io::Error::new(io::ErrorKind::TimedOut, "WinUSB request timed out") }
            else { io::Error::last_os_error() };
        self.cancel(&operation);
        Err(error)
    }

    fn control(&self, request_type: u8, request: u8, value: u16, index: u16,
        data: &mut [u8], stop: Option<&Event>, timeout: u32) -> io::Result<usize> {
        let length = u16::try_from(data.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput,
            "WinUSB control request exceeds 65535 bytes"))?;
        let packet = WINUSB_SETUP_PACKET { RequestType: request_type, Request: request,
            Value: value, Index: index, Length: length };
        let transferred = self.transfer(stop, timeout, |operation| unsafe {
            WinUsb_ControlTransfer(self.raw, packet, data.as_mut_ptr(), u32::from(length),
                ptr::null_mut(), operation) != 0
        })? as usize;
        if transferred > data.len() { return Err(io::Error::new(io::ErrorKind::InvalidData,
            "WinUSB control transfer exceeded its buffer")); }
        Ok(transferred)
    }

    fn device_descriptor(&self) -> io::Result<[u8; 18]> {
        let mut data = [0; 18];
        let count = self.control(0x80, 6, 0x0100, 0, &mut data, None, 1000)?;
        if count != data.len() || data[0] != 18 || data[1] != 1 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid USB device descriptor"));
        }
        Ok(data)
    }

    fn string_with_stop(&self, index: u8, stop: Option<&Event>) -> io::Result<String> {
        let mut data = [0; 255];
        let count = self.control(0x80, 6, 0x0300 | u16::from(index), 0, &mut data, stop, 1000)?;
        decode_string(&data[..count])
    }

    pub fn initialize(&self, _candidate: &Candidate, identifier: &DeviceIdentifier,
        configuration: &TabletConfiguration, stop: &Event) -> io::Result<()> {
        let delay = configuration.attributes.as_ref().and_then(|a| a.get("FeatureInitDelayMs"))
            .map(|text| text.parse::<u32>()).transpose()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid FeatureInitDelayMs"))?
            .unwrap_or(0);
        if delay == u32::MAX { return Err(io::Error::new(io::ErrorKind::InvalidData,
            "infinite feature initialization delay is unsupported")); }
        for index in identifier.initialization_strings.as_deref().unwrap_or_default() {
            self.string_with_stop(*index, Some(stop))?;
        }
        for report in identifier.feature_init_report.iter().flatten().filter(|r| !r.0.is_empty()) {
            match unsafe { WaitForSingleObject(stop.raw(), delay) } {
                WAIT_OBJECT_0 => return Err(cancelled()), WAIT_TIMEOUT => {},
                _ => return Err(io::Error::last_os_error()),
            }
            // WinUSB SET_REPORT uses the configured packet's exact length.
            // The descriptor maximum can cover a different report ID; HID
            // class padding must not change this control request's wLength.
            if report.0.len() > u16::MAX as usize {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "WinUSB feature initialization report exceeds 65535 bytes"));
            }
            let mut data = report.0.clone();
            let value = 0x0300 | u16::from(data[0]);
            let count = self.control(0x21, 9, value, u16::from(self.number), &mut data, Some(stop), 5000)?;
            if count != data.len() { return Err(io::Error::new(io::ErrorKind::WriteZero,
                "partial WinUSB feature initialization write")); }
        }
        for report in identifier.output_init_report.iter().flatten().filter(|r| !r.0.is_empty()) {
            let (pipe, length) = self.output.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData,
                "WinUSB interface has no output pipe"))?;
            let data = padded(&report.0, u32::from(length).max(report.0.len() as u32))?;
            let count = self.transfer(Some(stop), 5000, |operation| unsafe {
                WinUsb_WritePipe(self.raw, pipe, data.as_ptr(), data.len() as u32,
                    ptr::null_mut(), operation) != 0
            })?;
            if count as usize != data.len() { return Err(io::Error::new(io::ErrorKind::WriteZero,
                "partial WinUSB output initialization write")); }
        }
        Ok(())
    }
}

impl Drop for Interface {
    fn drop(&mut self) { unsafe { WinUsb_Free(self.raw) }; }
}

fn cancelled() -> io::Error { io::Error::new(io::ErrorKind::Interrupted, "WinUSB initialization cancelled") }

fn padded(data: &[u8], length: u32) -> io::Result<Vec<u8>> {
    if length == 0 || length > u16::MAX as u32 || data.len() > length as usize {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid WinUSB initialization report length"));
    }
    let mut output = vec![0; length as usize];
    output[..data.len()].copy_from_slice(data);
    Ok(output)
}

fn decode_string(data: &[u8]) -> io::Result<String> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid USB string descriptor");
    if data.len() < 2 || data[1] != 3 || data[0] < 2 || data[0] as usize > data.len()
        || data[0] % 2 != 0 { return Err(invalid()); }
    let words: Vec<u16> = data[2..data[0] as usize].chunks_exact(2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]])).collect();
    String::from_utf16(&words).map_err(|_| invalid())
}

fn usb_ids(instance: &str) -> Option<(u16, u16)> {
    let upper = instance.to_ascii_uppercase();
    let hardware = upper.strip_prefix("USB\\")?.split('\\').next()?;
    let mut vendor = None; let mut product = None;
    for component in hardware.split('&') {
        if let Some(value) = component.strip_prefix("VID_") {
            if value.len() != 4 { return None; } vendor = Some(u16::from_str_radix(value, 16).ok()?);
        } else if let Some(value) = component.strip_prefix("PID_") {
            if value.len() != 4 { return None; } product = Some(u16::from_str_radix(value, 16).ok()?);
        }
    }
    vendor.zip(product)
}

fn paths() -> io::Result<Vec<(Vec<u16>, u32)>> {
    let mut paths = BTreeMap::new();
    for guid in &INTERFACE_GUIDS {
        let set = unsafe { SetupDiGetClassDevsW(guid, ptr::null(), ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) };
        if set == INVALID_HANDLE_VALUE as isize { return Err(io::Error::last_os_error()); }
        let _guard = hid::DeviceInfoSet(set);
        for index in 0.. {
            let mut interface = SP_DEVICE_INTERFACE_DATA {
                cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default()
            };
            if unsafe { SetupDiEnumDeviceInterfaces(set, ptr::null(), guid, index, &mut interface) } == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_NO_MORE_ITEMS as i32) { break; }
                return Err(error);
            }
            if let Ok((path, instance)) = hid::detail_path(set, &interface) { paths.insert(path, instance); }
        }
    }
    Ok(paths.into_iter().collect())
}

fn inspect(path: &[u16], instance: u32, database: Option<&Database>) -> io::Result<(Candidate, serde_json::Value)> {
    if let Some((vendor, product)) = hid::instance_id(instance).as_deref().and_then(usb_ids) {
        if vendor == 0x2833 || database.is_some_and(|db| db.find(vendor, product).next().is_none()) {
            return Err(io::Error::new(io::ErrorKind::NotFound, "unrelated WinUSB device"));
        }
    }
    let path_text = String::from_utf16_lossy(&path[..path.len().saturating_sub(1)]);
    let handle = hid::open_path(path, true)?;
    let interface = Interface::open(&handle)?;
    let device = interface.device_descriptor()?;
    let vendor = u16::from_le_bytes([device[8], device[9]]);
    let product = u16::from_le_bytes([device[10], device[11]]);
    if vendor == 0x2833 || database.is_some_and(|db| db.find(vendor, product).next().is_none()) {
        return Err(io::Error::new(io::ErrorKind::NotFound, "unrelated WinUSB device"));
    }
    let mut report = [0; 256];
    let length = interface.control(0x81, 6, 0x2200, u16::from(interface.number), &mut report, None, 1000)?;
    let metadata = descriptor::parse(&report[..length]);
    let mut attributes = BTreeMap::new();
    attributes.insert("USB_INTERFACE_NUMBER".into(), interface.number.to_string());
    if let Some(metadata) = &metadata { attributes.insert("HID_REPORTS".into(), metadata.reports.clone()); }
    else { attributes.insert("HID_REPORTS_NON_RECONSTRUCTABLE".into(), "true".into()); }
    let mut endpoint = Endpoint { path: path_text.clone(), physical_id: hid::physical_id(instance),
        transport: Transport::WinUsb, vendor_id: vendor, product_id: product, can_open: true,
        input_length: u32::from(interface.input_length()), output_length: u32::from(interface.output_length()),
        feature_length: metadata.as_ref().map_or(0, |m| m.feature_length), strings: BTreeMap::new(),
        attributes: Some(attributes.clone()) };
    if let Some(database) = database {
        hid::read_matching_strings(&mut endpoint, database, |index| interface.string_with_stop(index, None));
    }
    let label = |index, fallback: &str| {
        if index == 0 { fallback.to_owned() } else {
            interface.string_with_stop(index, None).unwrap_or_else(|_| fallback.to_owned())
        }
    };
    // GetDevices supplies endpoint properties; normal detection avoids unrelated
    // manufacturer/serial queries and probes only configuration string indices.
    let rpc = if database.is_none() {
        let manufacturer = label(device[14], "Unknown Manufacturer");
        let product_name = label(device[15], "Unknown Product Name");
        let serial = label(device[16], "Unknown Serial Number");
        serde_json::json!({"DevicePath":path_text,"Manufacturer":manufacturer,"ProductName":product_name,
            "SerialNumber":serial,"FriendlyName":product_name,"VendorID":vendor,"ProductID":product,
            "InputReportLength":endpoint.input_length,"OutputReportLength":endpoint.output_length,
            "FeatureReportLength":endpoint.feature_length,"CanOpen":true,"DeviceAttributes":attributes})
    } else { serde_json::Value::Null };
    let candidate = Candidate { path: path.to_vec(), vendor, product, input_length: interface.input_length(),
        usage_page: metadata.as_ref().map_or(0, |m| m.usage_page), usage: metadata.as_ref().map_or(0, |m| m.usage), endpoint };
    Ok((candidate, rpc))
}

pub fn enumerate(database: &Database) -> io::Result<Vec<Candidate>> {
    Ok(paths()?.into_iter().filter_map(|(path, instance)| inspect(&path, instance, Some(database)).ok().map(|(c, _)| c)).collect())
}

pub fn rpc_devices() -> io::Result<Vec<serde_json::Value>> {
    Ok(paths()?.into_iter().filter_map(|(path, instance)| inspect(&path, instance, None).ok().map(|(_, dto)| dto)).collect())
}

pub fn read_strings(vendor: u16, product: u16, indices: &[u8]) -> io::Result<Vec<hid::DeviceStrings>> {
    let mut devices = Vec::new();
    for (path, instance) in paths()? {
        if let Some(ids) = hid::instance_id(instance).as_deref().and_then(usb_ids)
            && ids != (vendor, product) { continue; }
        if vendor == 0x2833 { continue; }
        let Ok(handle) = hid::open_path(&path, true) else { continue; };
        let Ok(interface) = Interface::open(&handle) else { continue; };
        let Ok(device) = interface.device_descriptor() else { continue; };
        if (u16::from_le_bytes([device[8], device[9]]), u16::from_le_bytes([device[10], device[11]])) != (vendor, product) { continue; }
        let values = indices.iter().map(|index| (*index,
            interface.string_with_stop(*index, None).map_err(|error| error.to_string()))).collect();
        devices.push((String::from_utf16_lossy(&path[..path.len().saturating_sub(1)]), values));
    }
    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn descriptor_strings_are_bounded_and_strict_utf16() {
        assert_eq!(decode_string(&[6, 3, 65, 0, 66, 0]).unwrap(), "AB");
        for data in [&[4, 3, 0][..], &[3, 3, 0], &[4, 1, 65, 0], &[4, 3, 0, 0xd8]] {
            assert!(decode_string(data).is_err());
        }
    }
    #[test]
    fn usb_prefilter_preserves_unknown_buses_and_padded_reports_are_bounded() {
        assert_eq!(usb_ids(r"USB\VID_256C&PID_006E&MI_01\7&123"), Some((0x256c, 0x006e)));
        assert_eq!(usb_ids(r"HID\VID_256C&PID_006E"), None);
        assert_eq!(padded(&[1, 2], 4).unwrap(), vec![1, 2, 0, 0]);
        assert!(padded(&[1, 2], 1).is_err());
        assert!(padded(&[1], 65536).is_err());
    }
}

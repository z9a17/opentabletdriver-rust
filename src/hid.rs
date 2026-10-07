//! Narrow ownership wrappers for Windows HID discovery and PnP notification.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;
use std::ptr;

use crate::dotnet::RuntimeDecoder;
use otd_core::endpoint_match::{self, Endpoint, Transport};
use otd_core::spec::TabletSpec;
use otd_core::tablets::{
    Database, DeviceIdentifier, ParserSupport, Role, TabletConfiguration, parser_support,
};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_IDW, CM_Get_Parent, CM_NOTIFY_ACTION, CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
    CM_NOTIFY_ACTION_DEVICEREMOVECOMPLETE, CM_NOTIFY_FILTER, CM_NOTIFY_FILTER_0,
    CM_NOTIFY_FILTER_0_0, CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE, CM_Register_Notification,
    CM_Unregister_Notification, CR_SUCCESS, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HCMNOTIFICATION,
    HDEVINFO, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA,
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW,
};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HIDD_ATTRIBUTES, HIDP_CAPS, HIDP_STATUS_SUCCESS, HidD_FreePreparsedData, HidD_GetAttributes,
    HidD_GetHidGuid, HidD_GetIndexedString, HidD_GetPreparsedData, HidD_SetFeature, HidP_GetCaps,
    HidD_GetManufacturerString, HidD_GetProductString, HidD_GetSerialNumberString,
    PHIDP_PREPARSED_DATA,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_NO_MORE_ITEMS, GENERIC_READ,
    GENERIC_WRITE, HANDLE, HWND, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DBT_DEVTYP_DEVICEINTERFACE, DEVICE_NOTIFY_WINDOW_HANDLE, DEV_BROADCAST_DEVICEINTERFACE_W,
    HDEVNOTIFY, RegisterDeviceNotificationW, UnregisterDeviceNotification,
};
use windows_sys::core::GUID;

pub const WACOM_VENDOR: u16 = 0x056a;
pub const PTH660_USB: u16 = 0x0357;
#[cfg(test)]
pub const PEN_REPORT_LENGTH: u16 = 192;
#[cfg(test)]
pub const AUX_REPORT_LENGTH: u16 = 44;

pub struct OwnedHandle(HANDLE);

impl OwnedHandle {
    pub fn new(raw: HANDLE) -> io::Result<Self> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(raw))
        }
    }

    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub struct Event(OwnedHandle);

// SAFETY: An owned Windows event handle can be moved between threads. The UI
// duplicates its event handle before starting the driver, preserving ownership.
unsafe impl Send for Event {}

impl Event {
    pub fn create(manual_reset: bool) -> io::Result<Self> {
        let raw = unsafe { CreateEventW(ptr::null(), i32::from(manual_reset), 0, ptr::null()) };
        Ok(Self(OwnedHandle::new(raw)?))
    }

    pub fn raw(&self) -> HANDLE {
        self.0.raw()
    }

    pub fn duplicate(&self) -> io::Result<Self> {
        let process = unsafe { GetCurrentProcess() };
        let mut duplicate = ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                process,
                self.raw(),
                process,
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(OwnedHandle::new(duplicate)?))
    }

    pub fn signal(&self) -> io::Result<()> {
        if unsafe { SetEvent(self.raw()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod event_tests {
    use super::*;
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    #[test]
    fn ui_stop_event_wakes_worker_and_survives_duplicate_handle_drop() {
        let event = Event::create(true).unwrap();
        let worker = event.duplicate().unwrap();
        let thread = std::thread::spawn(move || unsafe { WaitForSingleObject(worker.raw(), 1000) });
        event.signal().unwrap();
        drop(event);
        assert_eq!(thread.join().unwrap(), WAIT_OBJECT_0);
    }
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub path: Vec<u16>,
    pub vendor: u16,
    pub product: u16,
    pub input_length: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub endpoint: Endpoint,
    pub managed_endpoint: Option<u64>,
}

impl Candidate {
    pub fn path_text(&self) -> String {
        let length = self
            .path
            .iter()
            .position(|&x| x == 0)
            .unwrap_or(self.path.len());
        std::ffi::OsString::from_wide(&self.path[..length])
            .to_string_lossy()
            .into_owned()
    }

    /// Whether this collection's interface still exists. Opens it without
    /// access rights, which costs far less than enumerating every HID device.
    pub fn is_present(&self) -> bool {
        if let Some(endpoint)=self.managed_endpoint {
            return crate::dotnet::custom_devices::snapshot().is_ok_and(|devices|devices.iter().any(|device|device.endpoint==endpoint));
        }
        OwnedHandle::new(unsafe {
            CreateFileW(
                self.path.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        })
        .is_ok()
    }

    pub fn open_read(&self) -> io::Result<OwnedHandle> {
        self.open(false)
    }

    pub fn open(&self, write: bool) -> io::Result<OwnedHandle> {
        if self.managed_endpoint.is_some() { return Err(io::Error::new(io::ErrorKind::Unsupported,
            "Actual managed custom endpoints must use their sole managed reader owner, not a Windows file handle")); }
        open_path(&self.path, write || self.endpoint.transport == Transport::WinUsb)
    }
}

pub(crate) fn open_path(path: &[u16], write: bool) -> io::Result<OwnedHandle> {
        let raw = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ | if write { GENERIC_WRITE } else { 0 },
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        };
        OwnedHandle::new(raw)
}

pub(crate) struct DeviceInfoSet(pub(crate) HDEVINFO);

impl Drop for DeviceInfoSet {
    fn drop(&mut self) {
        unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

fn hid_guid() -> GUID {
    let mut guid: GUID = unsafe { std::mem::zeroed() };
    unsafe { HidD_GetHidGuid(&mut guid) };
    guid
}

pub(crate) fn detail_path(
    set: HDEVINFO,
    interface: &SP_DEVICE_INTERFACE_DATA,
) -> io::Result<(Vec<u16>, u32)> {
    let mut required = 0u32;
    unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            set,
            interface,
            ptr::null_mut(),
            0,
            &mut required,
            ptr::null_mut(),
        )
    };
    if required < size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 {
        return Err(io::Error::last_os_error());
    }
    // Vec<usize> provides the alignment needed by the detail-data structure.
    let words = (required as usize).div_ceil(size_of::<usize>());
    let mut storage = vec![0usize; words];
    let detail = storage
        .as_mut_ptr()
        .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
    unsafe { (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 };
    let mut device = SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    };
    if unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            set,
            interface,
            detail,
            required,
            &mut required,
            &mut device,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let path = unsafe { ptr::addr_of!((*detail).DevicePath).cast::<u16>() };
    let offset = path as usize - detail as usize;
    let max_chars = (storage.len() * size_of::<usize>() - offset) / size_of::<u16>();
    let raw = unsafe { std::slice::from_raw_parts(path, max_chars) };
    let Some(end) = raw.iter().position(|&ch| ch == 0) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "HID path has no terminator",
        ));
    };
    let mut out = raw[..end].to_vec();
    out.push(0);
    Ok((out, device.DevInst))
}

pub(crate) fn instance_id(instance: u32) -> Option<String> {
    let mut buffer = [0u16; 512];
    if unsafe { CM_Get_Device_IDW(instance, buffer.as_mut_ptr(), buffer.len() as u32, 0) }
        != CR_SUCCESS
    {
        return None;
    }
    let length = buffer.iter().position(|&c| c == 0)?;
    Some(String::from_utf16_lossy(&buffer[..length]))
}

/// Only use standard USB HID instance IDs as a negative prefilter. Unknown
/// formats still take the descriptor path; interface paths are opaque here.
fn usb_hid_ids(instance: &str) -> Option<(u16, u16)> {
    let instance = instance.to_ascii_uppercase();
    let hardware = instance.strip_prefix("HID\\")?.split('\\').next()?;
    let mut vendor = None;
    let mut product = None;
    for part in hardware.split('&') {
        if let Some(value) = part.strip_prefix("VID_") {
            if value.len() != 4 { return None; }
            vendor = Some(u16::from_str_radix(value, 16).ok()?);
        } else if let Some(value) = part.strip_prefix("PID_") {
            if value.len() != 4 { return None; }
            product = Some(u16::from_str_radix(value, 16).ok()?);
        }
    }
    vendor.zip(product)
}

fn should_inspect(instance: Option<&str>, database: &Database) -> bool {
    instance.and_then(usb_hid_ids).is_none_or(|(vendor, product)| {
        database.find(vendor, product).next().is_some()
    })
}

// Walk collection/interface ancestors to the physical USB device. Never pair
// unrelated devices using only VID/PID or a collection path with bits removed.
pub(crate) fn physical_id(mut instance: u32) -> String {
    for _ in 0..12 {
        let mut buffer = [0u16; 512];
        if unsafe { CM_Get_Device_IDW(instance, buffer.as_mut_ptr(), buffer.len() as u32, 0) }
            == CR_SUCCESS
        {
            let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
            let id = String::from_utf16_lossy(&buffer[..length]).to_uppercase();
            if id.starts_with("USB\\VID_") && !id.contains("&MI_") {
                return id;
            }
        }
        let mut parent = 0;
        if unsafe { CM_Get_Parent(&mut parent, instance, 0) } != CR_SUCCESS {
            break;
        }
        instance = parent;
    }
    String::new()
}

fn indexed_string(handle: HANDLE, index: u8) -> io::Result<String> {
    let mut buffer = [0u16; 256];
    if !unsafe {
        HidD_GetIndexedString(
            handle,
            u32::from(index),
            buffer.as_mut_ptr().cast(),
            size_of::<[u16; 256]>() as u32,
        )
    } {
        return Err(io::Error::last_os_error());
    }
    let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Ok(String::from_utf16_lossy(&buffer[..length]))
}

fn inspect(path: &[u16], instance: u32, database: Option<&Database>) -> Option<Candidate> {
    if database.is_some_and(|database| !should_inspect(instance_id(instance).as_deref(), database)) {
        return None;
    }
    let handle = OwnedHandle::new(unsafe {
        CreateFileW(
            path.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    })
    .ok()?;
    let mut attrs = HIDD_ATTRIBUTES {
        Size: size_of::<HIDD_ATTRIBUTES>() as u32,
        VendorID: 0,
        ProductID: 0,
        VersionNumber: 0,
    };
    if !unsafe { HidD_GetAttributes(handle.raw(), &mut attrs) } {
        return None;
    }
    if let Some(database) = database { database.find(attrs.VendorID, attrs.ProductID).next()?; }
    let mut preparsed: PHIDP_PREPARSED_DATA = 0;
    if !unsafe { HidD_GetPreparsedData(handle.raw(), &mut preparsed) } {
        return None;
    }
    let mut caps: HIDP_CAPS = unsafe { std::mem::zeroed() };
    let status = unsafe { HidP_GetCaps(preparsed, &mut caps) };
    unsafe { HidD_FreePreparsedData(preparsed) };
    if status != HIDP_STATUS_SUCCESS {
        return None;
    }
    let path_text = String::from_utf16_lossy(&path[..path.len().saturating_sub(1)]);
    let mut attributes = BTreeMap::new();
    if let Some((_, after)) = path_text.to_ascii_lowercase().split_once("&mi_")
        && let Some(value) = after
            .get(..2)
            .and_then(|value| u8::from_str_radix(value, 16).ok())
    {
        attributes.insert("USB_INTERFACE_NUMBER".into(), value.to_string());
    }
    let mut endpoint = Endpoint {
        path: path_text,
        physical_id: physical_id(instance),
        transport: Transport::UsbHid,
        vendor_id: attrs.VendorID,
        product_id: attrs.ProductID,
        can_open: true,
        input_length: u32::from(caps.InputReportByteLength),
        output_length: u32::from(caps.OutputReportByteLength),
        feature_length: u32::from(caps.FeatureReportByteLength),
        strings: BTreeMap::new(),
        attributes: Some(attributes),
    };
    if let Some(database) = database {
        read_matching_strings(&mut endpoint, database, |index| indexed_string(handle.raw(), index));
    }
    Some(Candidate {
        path: path.to_vec(),
        vendor: attrs.VendorID,
        product: attrs.ProductID,
        input_length: caps.InputReportByteLength,
        usage_page: caps.UsagePage,
        usage: caps.Usage,
        endpoint,
        managed_endpoint: None,
    })
}

/// Probe only strings used by identifiers whose report sizes fit this
/// collection. A failed string read is still a missing-string rejection.
pub(crate) fn read_matching_strings(
    endpoint: &mut Endpoint,
    database: &Database,
    mut read: impl FnMut(u8) -> io::Result<String>,
) {
    let indices: BTreeSet<u8> = database
        .find(endpoint.vendor_id, endpoint.product_id)
        .filter(|found| endpoint_match::matches_report_lengths(endpoint, found.identifier))
        .flat_map(|found| found.identifier.device_strings.iter().flat_map(|strings| strings.keys()))
        .filter_map(|index| index.parse().ok())
        .collect();
    endpoint.strings = indices.into_iter().filter_map(|index| {
        read(index).ok().map(|value| (index, value))
    }).collect();
}

pub fn enumerate() -> io::Result<Vec<Candidate>> {
    enumerate_with_database(Database::builtin())
}

pub fn enumerate_with_database(database: &Database) -> io::Result<Vec<Candidate>> {
    let guid = hid_guid();
    let set = unsafe {
        SetupDiGetClassDevsW(
            &guid,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if set == INVALID_HANDLE_VALUE as isize {
        return Err(io::Error::last_os_error());
    }
    let _guard = DeviceInfoSet(set);
    let mut found = Vec::new();
    let mut index = 0;
    loop {
        let mut interface = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        if unsafe { SetupDiEnumDeviceInterfaces(set, ptr::null(), &guid, index, &mut interface) }
            == 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_ITEMS as i32) {
                break;
            }
            return Err(error);
        }
        if let Ok((path, instance)) = detail_path(set, &interface)
            && let Some(candidate) = inspect(&path, instance, Some(database))
        {
            found.push(candidate);
        }
        index += 1;
    }
    found.extend(crate::winusb::enumerate(database)?);
    found.extend(enumerate_managed(database)?);
    found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(found)
}

/// Custom original hubs become candidates only after managed code is actually
/// loaded. Discovery never opens their stream or initializes CLR for native users.
fn enumerate_managed(database:&Database)->io::Result<Vec<Candidate>> {
    let mut found=Vec::new();
    for metadata in crate::dotnet::custom_devices::snapshot().map_err(io::Error::other)? {
        if database.find(metadata.vendor,metadata.product).next().is_none(){continue;}
        // Original custom endpoint contracts contain no parent-container ID.
        // Providers may publish an explicit physical identity as an attribute;
        // otherwise hub scope is the original matching domain for primary/aux.
        let physical=metadata.attributes.as_ref().and_then(|attributes|attributes.get("OTD_PHYSICAL_ID")).cloned()
            .unwrap_or_else(||format!("managed-hub:{}",metadata.scope));
        let mut endpoint=Endpoint {path:metadata.path.clone(),physical_id:physical,
            // Original Driver accepts custom endpoints with ordinary report IDs;
            // UsbHid here selects identifier semantics, not a HID file backend.
            transport:Transport::UsbHid,vendor_id:metadata.vendor,product_id:metadata.product,can_open:metadata.can_open,
            input_length:u32::from(metadata.input_length),output_length:u32::from(metadata.output_length),
            feature_length:u32::from(metadata.feature_length),strings:BTreeMap::new(),attributes:metadata.attributes};
        read_matching_strings(&mut endpoint,database,|index|crate::dotnet::custom_devices::device_string(metadata.endpoint,index).map_err(io::Error::other));
        let mut path:Vec<u16>=metadata.path.encode_utf16().collect();path.push(0);
        found.push(Candidate {path,vendor:metadata.vendor,product:metadata.product,input_length:metadata.input_length,
            usage_page:0,usage:0,endpoint,managed_endpoint:Some(metadata.endpoint)});
    }
    Ok(found)
}

/// Explicit compatibility inventory: all present HID collections, plus the
/// pinned WinUSB interfaces. It does not apply initialization reports or read
/// input. Failures remain missing/unavailable endpoint metadata.
pub fn enumerate_rpc_devices() -> io::Result<Vec<serde_json::Value>> {
    enumerate_device_metadata(true)
}
/// Managed background snapshots never open another input reader to probe
/// CanOpen. Unknown availability remains null until an explicit owner can prove it.
pub fn enumerate_service_devices() -> io::Result<Vec<serde_json::Value>> {
    enumerate_device_metadata(false)
}
fn enumerate_device_metadata(probe_open: bool) -> io::Result<Vec<serde_json::Value>> {
    let guid = hid_guid();
    let set = unsafe { SetupDiGetClassDevsW(&guid, ptr::null(), ptr::null_mut(),
        DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) };
    if set == INVALID_HANDLE_VALUE as isize { return Err(io::Error::last_os_error()); }
    let _guard = DeviceInfoSet(set);
    let mut devices = Vec::new();
    for index in 0.. {
        let mut interface = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default()
        };
        if unsafe { SetupDiEnumDeviceInterfaces(set, ptr::null(), &guid, index, &mut interface) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_ITEMS as i32) { break; }
            return Err(error);
        }
        let Ok((path, instance)) = detail_path(set, &interface) else { continue; };
        let Some(candidate) = inspect(&path, instance, None) else { continue; };
        let handle = OwnedHandle::new(unsafe { CreateFileW(path.as_ptr(), 0,
            FILE_SHARE_READ | FILE_SHARE_WRITE, ptr::null(), OPEN_EXISTING, 0, ptr::null_mut()) });
        let text = |getter: unsafe extern "system" fn(HANDLE, *mut c_void, u32) -> bool| {
            let Ok(handle) = &handle else { return String::new(); };
            let mut data = [0u16; 256];
            if !unsafe { getter(handle.raw(), data.as_mut_ptr().cast(), size_of::<[u16; 256]>() as u32) } {
                return String::new();
            }
            let end = data.iter().position(|word| *word == 0).unwrap_or(data.len());
            String::from_utf16_lossy(&data[..end])
        };
        let manufacturer = text(HidD_GetManufacturerString);
        let product = text(HidD_GetProductString);
        let serial = text(HidD_GetSerialNumberString);
        devices.push(serde_json::json!({"DevicePath":candidate.endpoint.path,
            "Manufacturer":manufacturer,"ProductName":product,"FriendlyName":product,
            "SerialNumber":serial,"VendorID":candidate.vendor,"ProductID":candidate.product,
            "InputReportLength":candidate.endpoint.input_length,
            "OutputReportLength":candidate.endpoint.output_length,
            "FeatureReportLength":candidate.endpoint.feature_length,
            "CanOpen":if probe_open { Some(candidate.open_read().is_ok()) } else { None },
            "DeviceAttributes":candidate.endpoint.attributes}));
    }
    devices.extend(if probe_open { crate::winusb::rpc_devices()? } else { crate::winusb::service_metadata()? });
    for endpoint in crate::dotnet::custom_devices::snapshot().map_err(io::Error::other)? {
        let mut metadata=serde_json::to_value(&endpoint.original).map_err(io::Error::other)?;
        if let Some(object)=metadata.as_object_mut() {object.extend(serde_json::json!({"endpoint":endpoint.endpoint,"scope":endpoint.scope,
            "DevicePath":endpoint.path,"VendorID":endpoint.vendor,"ProductID":endpoint.product,
            "InputReportLength":endpoint.input_length,"OutputReportLength":endpoint.output_length,
            "FeatureReportLength":endpoint.feature_length,"CanOpen":endpoint.can_open,"DeviceAttributes":endpoint.attributes}).as_object().unwrap().clone());}
        devices.push(metadata);
    }
    Ok(devices)
}

/// A collection's path and its strings by index.
pub type DeviceStrings = (String, Vec<(u8, Result<String, String>)>);

/// USB string descriptors of every HID collection with these IDs, as
/// OpenTabletDriver's device string reader shows them. Read-only: each
/// collection is opened without read or write access.
pub fn read_strings(vendor: u16, product: u16, indices: &[u8]) -> io::Result<Vec<DeviceStrings>> {
    let guid = hid_guid();
    let set = unsafe {
        SetupDiGetClassDevsW(
            &guid,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if set == INVALID_HANDLE_VALUE as isize {
        return Err(io::Error::last_os_error());
    }
    let _guard = DeviceInfoSet(set);
    let mut found = Vec::new();
    for index in 0.. {
        let mut interface = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        if unsafe { SetupDiEnumDeviceInterfaces(set, ptr::null(), &guid, index, &mut interface) }
            == 0
        {
            break;
        }
        let Ok((path, _)) = detail_path(set, &interface) else {
            continue;
        };
        let Ok(handle) = OwnedHandle::new(unsafe {
            CreateFileW(
                path.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        }) else {
            continue;
        };
        let mut attrs = HIDD_ATTRIBUTES {
            Size: size_of::<HIDD_ATTRIBUTES>() as u32,
            VendorID: 0,
            ProductID: 0,
            VersionNumber: 0,
        };
        if !unsafe { HidD_GetAttributes(handle.raw(), &mut attrs) }
            || attrs.VendorID != vendor
            || attrs.ProductID != product
        {
            continue;
        }
        let strings = indices
            .iter()
            .map(|&string| {
                (
                    string,
                    indexed_string(handle.raw(), string).map_err(|error| error.to_string()),
                )
            })
            .collect();
        found.push((
            String::from_utf16_lossy(&path[..path.len().saturating_sub(1)]),
            strings,
        ));
    }
    found.extend(crate::winusb::read_strings(vendor, product, indices)?);
    Ok(found)
}

pub struct SelectedDevice<'a> {
    pub pen: &'a Candidate,
    pub configuration: TabletConfiguration,
    pub identifier: DeviceIdentifier,
    pub auxiliary: Option<(&'a Candidate, DeviceIdentifier)>,
    /// The digitizer's ranges from the configuration.
    pub spec: TabletSpec,
}

impl SelectedDevice<'_> {
    /// The decoder for this endpoint's configured parser.
    pub fn decoder(&self) -> io::Result<RuntimeDecoder> { self.decoder_for_graph(false) }
    pub fn decoder_for_graph(&self, prefer_original: bool) -> io::Result<RuntimeDecoder> {
        RuntimeDecoder::for_graph(self.identifier.parser(), self.spec, prefer_original).map_err(io::Error::other)
    }

}

/// The configuration name and role of a discovered collection, if a usable
/// configuration matches it, and whether the driver can decode it.
pub fn identify(device: &Candidate, database: &Database) -> Option<(String, Role, bool)> {
    database
        .find(device.vendor, device.product)
        .find(|found| endpoint_match::matches(&device.endpoint, found).is_ok())
        .map(|found| {
            (
                found.configuration.name.clone(),
                found.role,
                parser_supported(found.identifier.parser()),
            )
        })
}

/// Configuration names of the connected tablets this driver can run, in
/// device-path order. Used to pick which OpenTabletDriver profile to import.
pub fn connected_tablets() -> Result<Vec<String>, String> {
    let database = crate::config::configured_tablets()?;
    let mut names = Vec::new();
    for device in enumerate_with_database(&database).map_err(|error| error.to_string())? {
        if let Some((name, Role::Digitizer, true)) = identify(&device, &database)
            && crate::config::runtime_tablet_in_with_parser_support(&name, &database, &crate::dotnet::installed_report_parser).is_ok()
            && !names.contains(&name)
        {
            names.push(name);
        }
    }
    Ok(names)
}

/// Explicit original-settings import also considers matched tablets whose
/// parser is supplied by an installed plugin. This lookup is metadata-only;
/// actual parser availability is checked at explicit Apply/Start.
pub fn connected_tablets_for_import() -> Result<Vec<String>, String> {
    let database = crate::config::configured_tablets()?;
    let mut names = Vec::new();
    for device in enumerate_with_database(&database).map_err(|error| error.to_string())? {
        if let Some((name, Role::Digitizer, _)) = identify(&device, &database)
            && crate::config::spec_for_tablet_in(&name, &database).is_ok()
            && !names.contains(&name)
        {
            names.push(name);
        }
    }
    Ok(names)
}

/// Pure cached support check; no CLR initialization or plugin construction.
pub fn parser_supported(name: &str) -> bool {
    parser_support(name) != ParserSupport::Missing || crate::dotnet::installed_report_parser(name)
}

/// Selects one tablet to drive. Every digitizer interface that matches a
/// usable configuration (IDs, report lengths and device strings) and whose
/// parser and specifications this driver supports is a candidate. A profile
/// that names a tablet selects only that tablet; `path` selects one endpoint.
/// Among several candidates the first by device path wins, so the choice is
/// stable; OpenTabletDriver would run all of them.
pub fn select_device<'a>(
    devices: &'a [Candidate],
    database: &Database,
    path: Option<&str>,
    tablet: Option<&str>,
) -> Result<Option<SelectedDevice<'a>>, String> {
    select_device_impl(devices, database, path, tablet, false)
}

/// Explicit output startup may load an installed parser for an actually matched
/// missing endpoint. Ordinary discovery uses select_device and never does so.
pub fn select_device_for_start<'a>(devices: &'a [Candidate], database: &Database,
    path: Option<&str>, tablet: Option<&str>) -> Result<Option<SelectedDevice<'a>>, String> {
    select_device_impl(devices, database, path, tablet, true)
}
fn select_device_impl<'a>(devices: &'a [Candidate], database: &Database,
    path: Option<&str>, tablet: Option<&str>, load_registry: bool) -> Result<Option<SelectedDevice<'a>>, String> {
    let mut registry_loaded = false;
    let mut unsupported = None;
    for device in devices
        .iter()
        .filter(|device| path.is_none_or(|path| device.path_text().eq_ignore_ascii_case(path)))
    {
        for found in database
            .find(device.vendor, device.product)
            .filter(|found| {
                found.role == Role::Digitizer
                    && tablet.is_none_or(|name| found.configuration.name == name)
            })
        {
            if endpoint_match::matches(&device.endpoint, &found).is_err() {
                continue;
            }
            if !parser_supported(found.identifier.parser()) && load_registry && !registry_loaded {
                crate::plugins::load_parser_registry()?;
                registry_loaded = true;
            }
            if !parser_supported(found.identifier.parser()) {
                unsupported = Some(format!(
                    "{} uses {}, which this driver cannot decode",
                    found.configuration.name,
                    found.identifier.parser()
                ));
                continue;
            }
            let spec = match TabletSpec::from_configuration(found.configuration) {
                Ok(spec) => spec,
                Err(error) => {
                    unsupported = Some(error);
                    continue;
                }
            };
            let auxiliary = found.configuration.auxiliary_identifiers().iter().find_map(|identifier| {
                let (vendor, product) = identifier.vendor_id().zip(identifier.product_id())?;
                let aux = database.find(vendor, product).find(|aux| {
                    aux.role == Role::Auxiliary
                        && std::ptr::eq(aux.configuration, found.configuration)
                        && std::ptr::eq(aux.identifier, identifier)
                })?;
                devices.iter().find(|other| {
                    other.path != device.path
                        && !other.endpoint.physical_id.is_empty()
                        && other.endpoint.physical_id == device.endpoint.physical_id
                        && endpoint_match::matches(&other.endpoint, &aux).is_ok()
                }).map(|endpoint| (endpoint, aux.identifier.clone()))
            });
            let mut configuration = found.configuration.clone();
            // The managed TabletReference constructor receives the actual selected identifier.
            configuration.digitizer_identifiers = vec![found.identifier.clone()];
            return Ok(Some(SelectedDevice {
                pen: device,
                configuration,
                identifier: found.identifier.clone(),
                auxiliary,
                spec,
            }));
        }
    }
    match unsupported {
        Some(reason) => Err(reason),
        None => Ok(None),
    }
}

/// Initialization follows pinned InputDevice.Initialize: strings, delayed
/// features, then output writes. Unlike upstream's warning-and-continue policy,
/// failures abort this session so partially initialized hardware never injects.
pub fn initialize(
    candidate: &Candidate,
    handle: &OwnedHandle,
    identifier: &DeviceIdentifier,
    configuration: &TabletConfiguration,
    stop: &Event,
) -> io::Result<()> {
    let cancelled = || {
        if unsafe { WaitForSingleObject(stop.raw(), 0) } == WAIT_OBJECT_0 {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "device initialization cancelled",
            ))
        } else {
            Ok(())
        }
    };
    let delay = configuration
        .attributes
        .as_ref()
        .and_then(|attributes| attributes.get("FeatureInitDelayMs"))
        .map(|text| {
            text.parse::<u32>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid FeatureInitDelayMs")
            })
        })
        .transpose()?
        .unwrap_or(0);
    if delay == u32::MAX {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "infinite feature initialization delay is unsupported",
        ));
    }
    for &index in identifier
        .initialization_strings
        .as_deref()
        .unwrap_or_default()
    {
        cancelled()?;
        indexed_string(handle.raw(), index)?;
    }
    for report in identifier
        .feature_init_report
        .iter()
        .flatten()
        .filter(|report| !report.0.is_empty())
    {
        cancelled()?;
        match unsafe { WaitForSingleObject(stop.raw(), delay) } {
            WAIT_OBJECT_0 => {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "device initialization cancelled",
                ));
            }
            WAIT_TIMEOUT => {}
            _ => return Err(io::Error::last_os_error()),
        }
        let mut data = padded_report(&report.0, candidate.endpoint.feature_length)?;
        if !unsafe { HidD_SetFeature(handle.raw(), data.as_mut_ptr().cast(), data.len() as u32) } {
            return Err(io::Error::last_os_error());
        }
    }
    for report in identifier
        .output_init_report
        .iter()
        .flatten()
        .filter(|report| !report.0.is_empty())
    {
        cancelled()?;
        let data = padded_report(&report.0, candidate.endpoint.output_length)?;
        let event = Event::create(true)?;
        let mut operation = OVERLAPPED {
            hEvent: event.raw(),
            ..Default::default()
        };
        let started = unsafe {
            WriteFile(
                handle.raw(),
                data.as_ptr(),
                data.len() as u32,
                ptr::null_mut(),
                &mut operation,
            )
        };
        if started == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(windows_sys::Win32::Foundation::ERROR_IO_PENDING as i32)
            {
                return Err(error);
            }
            let handles = [stop.raw(), event.raw()];
            let result = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, 5000) };
            if result != WAIT_OBJECT_0 + 1 {
                unsafe {
                    CancelIoEx(handle.raw(), &operation);
                }
                let mut ignored = 0;
                unsafe {
                    GetOverlappedResult(handle.raw(), &operation, &mut ignored, 1);
                }
                return Err(io::Error::new(
                    if result == WAIT_OBJECT_0 {
                        io::ErrorKind::Interrupted
                    } else {
                        io::ErrorKind::TimedOut
                    },
                    "device initialization write cancelled or timed out",
                ));
            }
        }
        let mut written = 0;
        if unsafe { GetOverlappedResult(handle.raw(), &operation, &mut written, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if written != data.len() as u32 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "partial device initialization write",
            ));
        }
    }
    cancelled()
}

fn padded_report(report: &[u8], length: u32) -> io::Result<Vec<u8>> {
    if length == 0 || report.len() > length as usize || length > u16::MAX as u32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "initialization report exceeds endpoint report length",
        ));
    }
    let mut data = vec![0; length as usize];
    data[..report.len()].copy_from_slice(report);
    Ok(data)
}

unsafe extern "system" fn notification_callback(
    _notification: HCMNOTIFICATION,
    context: *const c_void,
    action: CM_NOTIFY_ACTION,
    _data: *const windows_sys::Win32::Devices::DeviceAndDriverInstallation::CM_NOTIFY_EVENT_DATA,
    _size: u32,
) -> u32 {
    if action == CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL
        || action == CM_NOTIFY_ACTION_DEVICEREMOVECOMPLETE
    {
        unsafe { SetEvent(context as HANDLE) };
    }
    CR_SUCCESS
}

pub struct Notification {
    registrations: Vec<HCMNOTIFICATION>,
    event: Event,
}

/// HID interface arrival/removal messages for the panel, including late
/// collections created after the generic device-tree change broadcast.
pub struct WindowNotification(Vec<HDEVNOTIFY>);

impl WindowNotification {
    pub fn register(window: HWND) -> io::Result<Self> {
        let mut this = Self(Vec::with_capacity(3));
        for guid in [hid_guid(), crate::winusb::INTERFACE_GUIDS[0], crate::winusb::INTERFACE_GUIDS[1]] {
        let filter = DEV_BROADCAST_DEVICEINTERFACE_W {
            dbcc_size: size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32,
            dbcc_devicetype: DBT_DEVTYP_DEVICEINTERFACE,
            dbcc_classguid: guid,
            ..Default::default()
        };
        let registration = unsafe {
            RegisterDeviceNotificationW(
                window,
                ptr::addr_of!(filter).cast(),
                DEVICE_NOTIFY_WINDOW_HANDLE,
            )
        };
        if registration.is_null() {
            return Err(io::Error::last_os_error());
        }
        this.0.push(registration);
        }
        Ok(this)
    }
}

impl Drop for WindowNotification {
    fn drop(&mut self) {
        for registration in &self.0 { unsafe { UnregisterDeviceNotification(*registration) }; }
    }
}

impl Notification {
    pub fn register() -> io::Result<Self> {
        let mut this = Self { registrations: Vec::with_capacity(3), event: Event::create(false)? };
        for guid in [hid_guid(), crate::winusb::INTERFACE_GUIDS[0], crate::winusb::INTERFACE_GUIDS[1]] {
        let filter = CM_NOTIFY_FILTER {
            cbSize: size_of::<CM_NOTIFY_FILTER>() as u32,
            Flags: 0,
            FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
            Reserved: 0,
            u: CM_NOTIFY_FILTER_0 {
                DeviceInterface: CM_NOTIFY_FILTER_0_0 { ClassGuid: guid },
            },
        };
        let mut registration = ptr::null_mut();
        let result = unsafe {
            CM_Register_Notification(
                &filter,
                this.event.raw() as *const c_void,
                Some(notification_callback),
                &mut registration,
            )
        };
        if result != CR_SUCCESS {
            return Err(io::Error::other(format!(
                "CM_Register_Notification failed: {result}"
            )));
        }
        this.registrations.push(registration);
        }
        Ok(this)
    }

    pub fn event(&self) -> HANDLE {
        self.event.raw()
    }
}

impl Drop for Notification {
    fn drop(&mut self) {
        for registration in &self.registrations { unsafe { CM_Unregister_Notification(*registration) }; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use otd_core::tablets::{Database, Role};

    #[test]
    fn unrelated_usb_hid_devices_are_rejected_before_opening_descriptors() {
        let database = Database::builtin();
        assert!(!should_inspect(Some(r"HID\VID_FFFF&PID_FFFF&MI_00\7&123&0&0000"), database));
        assert!(should_inspect(Some(r"hid\vid_056a&pid_0357&mi_00&col01\7&123&0&0000"), database));
        assert_eq!(usb_hid_ids(r"HID\VID_056A&PID_0357&COL01\7&123"), Some((0x056a, 0x0357)));
        // Unrecognized buses, failed metadata and malformed IDs must fall
        // back to descriptor inspection instead of hiding a tablet.
        for unknown in [None, Some(r"BTHENUM\VID_056A&PID_0357"), Some(r"HID\VID_056A&PID_0357_EXTRA\1")] {
            assert!(should_inspect(unknown, database));
        }
    }

    #[test]
    fn discovery_does_not_query_strings_for_impossible_report_lengths() {
        let files = vec![("discovery-fixture.json".into(), serde_json::json!({
            "Name": "Discovery fixture",
            "Specifications": {"Digitizer": {"Width": 100, "Height": 100, "MaxX": 1000, "MaxY": 1000}, "Pen": {"MaxPressure": 1000, "ButtonCount": 2}},
            "DigitizerIdentifiers": [
                {"VendorID": 65534, "ProductID": 65534, "InputReportLength": 64, "DeviceStrings": {"201": "^slow$"}},
                {"VendorID": 65534, "ProductID": 65534, "OutputReportLength": 64, "DeviceStrings": {"202": "^slow$"}},
                {"VendorID": 65534, "ProductID": 65534, "FeatureReportLength": 64, "DeviceStrings": {"203": "^slow$"}},
                {"VendorID": 65534, "ProductID": 65534, "InputReportLength": 8, "DeviceStrings": {"1": "^tablet$"}},
                {"VendorID": 65534, "ProductID": 65534, "DeviceStrings": {"1": "^tablet$", "2": "^model$"}}
            ]
        }).to_string())];
        let database = Database::with_overrides(&files);
        // An unusable fixture would make every assertion below vacuous.
        assert_eq!(database.find(65534, 65534).count(), 5);
        let mut endpoint = Endpoint {
            path: "fixture".into(), physical_id: "physical-fixture".into(),
            transport: Transport::UsbHid, vendor_id: 65534, product_id: 65534,
            can_open: true, input_length: 8, output_length: 0, feature_length: 0,
            strings: BTreeMap::new(), attributes: None,
        };
        let mut requested = Vec::new();
        read_matching_strings(&mut endpoint, &database, |index| {
            requested.push(index);
            match index {
                1 => Ok("tablet".into()),
                2 => Err(io::Error::other("string unavailable")),
                _ => panic!("slow string probe on a collection that cannot match"),
            }
        });
        assert_eq!(requested, [1, 2]);
        assert_eq!(endpoint.strings.get(&1).map(String::as_str), Some("tablet"));
        assert!(!endpoint.strings.contains_key(&2));
        let candidate = database.find(65534, 65534)
            .find(|candidate| candidate.identifier.input_report_length == Some(8)).unwrap();
        assert_eq!(endpoint_match::matches(&endpoint, &candidate), Ok(()));
    }

    /// The interfaces this driver opens are the ones OpenTabletDriver's
    /// PTH-660 configuration declares.
    #[test]
    fn opened_interfaces_match_the_tablet_database() {
        let lengths = |role| {
            Database::builtin()
                .find(WACOM_VENDOR, PTH660_USB)
                .filter(|m| m.role == role)
                .map(|m| m.identifier.input_report_length)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            lengths(Role::Digitizer),
            [Some(u32::from(PEN_REPORT_LENGTH))]
        );
        assert_eq!(
            lengths(Role::Auxiliary),
            [Some(u32::from(AUX_REPORT_LENGTH))]
        );
    }
}

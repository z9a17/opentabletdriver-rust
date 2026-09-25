//! Narrow ownership wrappers for Windows HID discovery and PnP notification.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;
use std::ptr;

use otd_core::decoders::TabletDecoder;
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
    PHIDP_PREPARSED_DATA,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_NO_MORE_ITEMS, GENERIC_READ,
    GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows_sys::core::GUID;

pub const WACOM_VENDOR: u16 = 0x056a;
pub const PTH660_USB: u16 = 0x0357;
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
        let raw = unsafe {
            CreateFileW(
                self.path.as_ptr(),
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
}

struct DeviceInfoSet(HDEVINFO);

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

fn detail_path(
    set: HDEVINFO,
    interface: &SP_DEVICE_INTERFACE_DATA,
) -> io::Result<(Vec<u16>, String)> {
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
    Ok((out, physical_id(device.DevInst)))
}

// Walk collection/interface ancestors to the physical USB device. Never pair
// unrelated devices using only VID/PID or a collection path with bits removed.
fn physical_id(mut instance: u32) -> String {
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

fn inspect(path: &[u16], physical_id: String, database: &Database) -> Option<Candidate> {
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
    database.find(attrs.VendorID, attrs.ProductID).next()?;
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
    let indices: BTreeSet<u8> = database
        .find(attrs.VendorID, attrs.ProductID)
        .flat_map(|found| {
            found
                .identifier
                .device_strings
                .iter()
                .flat_map(|strings| strings.keys())
        })
        .filter_map(|index| index.parse().ok())
        .collect();
    let strings = indices
        .into_iter()
        .filter_map(|index| {
            indexed_string(handle.raw(), index)
                .ok()
                .map(|value| (index, value))
        })
        .collect();
    let mut attributes = BTreeMap::new();
    if let Some((_, after)) = path_text.to_ascii_lowercase().split_once("&mi_")
        && let Some(value) = after
            .get(..2)
            .and_then(|value| u8::from_str_radix(value, 16).ok())
    {
        attributes.insert("USB_INTERFACE_NUMBER".into(), value.to_string());
    }
    let endpoint = Endpoint {
        path: path_text,
        physical_id,
        transport: Transport::UsbHid,
        vendor_id: attrs.VendorID,
        product_id: attrs.ProductID,
        can_open: true,
        input_length: u32::from(caps.InputReportByteLength),
        output_length: u32::from(caps.OutputReportByteLength),
        feature_length: u32::from(caps.FeatureReportByteLength),
        strings,
        attributes: Some(attributes),
    };
    Some(Candidate {
        path: path.to_vec(),
        vendor: attrs.VendorID,
        product: attrs.ProductID,
        input_length: caps.InputReportByteLength,
        usage_page: caps.UsagePage,
        usage: caps.Usage,
        endpoint,
    })
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
        if let Ok((path, physical_id)) = detail_path(set, &interface)
            && let Some(candidate) = inspect(&path, physical_id, database)
        {
            found.push(candidate);
        }
        index += 1;
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(found)
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
    pub fn decoder(&self) -> io::Result<TabletDecoder> {
        TabletDecoder::for_parser(self.identifier.parser(), self.spec).ok_or_else(|| {
            io::Error::other(format!(
                "{} uses {}, which this driver cannot decode",
                self.configuration.name,
                self.identifier.parser()
            ))
        })
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
                parser_support(found.identifier.parser()) != ParserSupport::Missing,
            )
        })
}

/// Configuration names of the connected tablets this driver can run, in
/// device-path order. Used to pick which OpenTabletDriver profile to import.
pub fn connected_tablets() -> Vec<String> {
    let database = Database::builtin();
    let mut names = Vec::new();
    for device in enumerate().unwrap_or_default() {
        if let Some((name, Role::Digitizer, true)) = identify(&device, database)
            && !names.contains(&name)
        {
            names.push(name);
        }
    }
    names
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
            if parser_support(found.identifier.parser()) == ParserSupport::Missing {
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
            let auxiliary = database
                .find(device.vendor, device.product)
                .filter(|aux| {
                    aux.role == Role::Auxiliary
                        && std::ptr::eq(aux.configuration, found.configuration)
                })
                .find_map(|aux| {
                    devices
                        .iter()
                        .find(|other| {
                            other.path != device.path
                                && !other.endpoint.physical_id.is_empty()
                                && other.endpoint.physical_id == device.endpoint.physical_id
                                && endpoint_match::matches(&other.endpoint, &aux).is_ok()
                        })
                        .map(|endpoint| (endpoint, aux.identifier.clone()))
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
    registration: HCMNOTIFICATION,
    event: Event,
}

impl Notification {
    pub fn register() -> io::Result<Self> {
        let event = Event::create(false)?;
        let guid = hid_guid();
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
                event.raw() as *const c_void,
                Some(notification_callback),
                &mut registration,
            )
        };
        if result != CR_SUCCESS {
            return Err(io::Error::other(format!(
                "CM_Register_Notification failed: {result}"
            )));
        }
        Ok(Self {
            registration,
            event,
        })
    }

    pub fn event(&self) -> HANDLE {
        self.event.raw()
    }
}

impl Drop for Notification {
    fn drop(&mut self) {
        unsafe { CM_Unregister_Notification(self.registration) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use otd_core::tablets::{Database, Role};

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

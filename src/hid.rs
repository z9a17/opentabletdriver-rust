//! Narrow ownership wrappers for Windows HID discovery and PnP notification.

use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStringExt;
use std::ptr;

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_NOTIFY_ACTION, CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
    CM_NOTIFY_ACTION_DEVICEREMOVECOMPLETE, CM_NOTIFY_FILTER, CM_NOTIFY_FILTER_0,
    CM_NOTIFY_FILTER_0_0, CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE, CM_Register_Notification,
    CM_Unregister_Notification, CR_SUCCESS, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HCMNOTIFICATION,
    HDEVINFO, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW,
};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HIDD_ATTRIBUTES, HIDP_CAPS, HIDP_STATUS_SUCCESS, HidD_FreePreparsedData, HidD_GetAttributes,
    HidD_GetHidGuid, HidD_GetPreparsedData, HidP_GetCaps, PHIDP_PREPARSED_DATA,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_NO_MORE_ITEMS, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent};
use windows_sys::core::GUID;

pub const WACOM_VENDOR: u16 = 0x056a;
pub const PTH660_USB: u16 = 0x0357;
pub const PEN_REPORT_LENGTH: u16 = 192;
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

impl Event {
    pub fn create(manual_reset: bool) -> io::Result<Self> {
        let raw = unsafe { CreateEventW(ptr::null(), i32::from(manual_reset), 0, ptr::null()) };
        Ok(Self(OwnedHandle::new(raw)?))
    }

    pub fn raw(&self) -> HANDLE {
        self.0.raw()
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

    pub fn is_pen(&self) -> bool {
        self.vendor == WACOM_VENDOR
            && self.product == PTH660_USB
            && self.input_length == PEN_REPORT_LENGTH
    }

    pub fn open_read(&self) -> io::Result<OwnedHandle> {
        let raw = unsafe {
            CreateFileW(
                self.path.as_ptr(),
                GENERIC_READ,
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

fn detail_path(set: HDEVINFO, interface: &SP_DEVICE_INTERFACE_DATA) -> io::Result<Vec<u16>> {
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
    if unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            set,
            interface,
            detail,
            required,
            &mut required,
            ptr::null_mut(),
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
    Ok(out)
}

fn inspect(path: &[u16]) -> Option<Candidate> {
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
    if attrs.VendorID != WACOM_VENDOR || attrs.ProductID != PTH660_USB {
        return None;
    }
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
    Some(Candidate {
        path: path.to_vec(),
        vendor: attrs.VendorID,
        product: attrs.ProductID,
        input_length: caps.InputReportByteLength,
        usage_page: caps.UsagePage,
        usage: caps.Usage,
    })
}

pub fn enumerate() -> io::Result<Vec<Candidate>> {
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
        if let Ok(path) = detail_path(set, &interface)
            && let Some(candidate) = inspect(&path)
        {
            found.push(candidate);
        }
        index += 1;
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(found)
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

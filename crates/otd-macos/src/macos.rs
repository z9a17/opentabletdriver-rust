//! USB IOKit HID transport and CoreGraphics logical-coordinate mouse output.
//! The single-threaded CFRunLoop owns every callback and its fixed queue.

use std::cell::{Cell, RefCell, UnsafeCell};
use std::collections::BTreeMap;
use std::ffi::{CString, c_void};
use std::io;
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use otd_platform::shared_device_io::{ReaderServices,ServiceCall,RequestKind,SharedIo};
use otd_platform::shared_devices::{Registration,OutputGate};
use otd_platform::managed_services::Operation;
use std::time::{Duration, Instant};

use otd_core::display::{DisplayFingerprint, DisplaySnapshot};
use otd_core::actions::{Action, ActionTransition, MouseButton};
use otd_core::output::buttons::{ActionSink, LocalActions, ScrollAxis, ScrollPulse};
use otd_core::endpoint_match::{Endpoint, Transport};
use otd_core::mapping::Rect;
use otd_core::output::{MousePacket, flags};
use otd_core::session::{Displays, Read, ReportSource};
use otd_core::tablets::{Database, DeviceIdentifier, TabletConfiguration};

use crate::{descriptor, ffi};

const MAX_REPORT: usize = 4096;
const QUEUE: usize = 32;
const UTF8: u32 = 0x0800_0100;
const LISTEN_EVENT: u32 = 1;

struct Owned(ffi::Ref);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: each wrapper owns a Create/Copy/Retain reference.
            unsafe { ffi::CFRelease(self.0) };
        }
    }
}

struct IoObject(u32);
impl Drop for IoObject {
    fn drop(&mut self) {
        if self.0 != 0 {
            // SAFETY: this is an owned registry entry or iterator reference.
            unsafe { ffi::IOObjectRelease(self.0) };
        }
    }
}

fn key(text: &str) -> Owned {
    let text = CString::new(text).expect("internal property names contain no NUL");
    // SAFETY: the CString remains live during the copying constructor.
    Owned(unsafe { ffi::CFStringCreateWithCString(ptr::null(), text.as_ptr(), UTF8) })
}

fn number(value: ffi::Ref) -> Option<i64> {
    if value.is_null() || unsafe { ffi::CFGetTypeID(value) != ffi::CFNumberGetTypeID() } {
        return None;
    }
    let mut result = 0i64;
    // SAFETY: kCFNumberSInt64Type (4) writes an i64 to valid storage.
    (unsafe { ffi::CFNumberGetValue(value, 4, (&mut result as *mut i64).cast()) } != 0).then_some(result)
}

fn string(value: ffi::Ref) -> Option<String> {
    if value.is_null() || unsafe { ffi::CFGetTypeID(value) != ffi::CFStringGetTypeID() } {
        return None;
    }
    let mut buffer = [0u8; 1024];
    // SAFETY: the bounded buffer is writable and UTF-8 is the requested encoding.
    if unsafe { ffi::CFStringGetCString(value, buffer.as_mut_ptr().cast(), buffer.len() as isize, UTF8) } == 0 {
        return None;
    }
    let length = buffer.iter().position(|byte| *byte == 0)?;
    String::from_utf8(buffer[..length].to_vec()).ok()
}

fn property(device: ffi::Hid, name: &str) -> ffi::Ref {
    // SAFETY: device is retained, and returned property remains borrowed from it.
    unsafe { ffi::IOHIDDeviceGetProperty(device, key(name).0) }
}

fn registry_property(entry: u32, name: &str) -> Owned {
    // SAFETY: entry is live; Create returns a separately owned reference.
    Owned(unsafe { ffi::IORegistryEntryCreateCFProperty(entry, key(name).0, ptr::null(), 0) })
}

fn native_error(operation: &str, code: i32) -> io::Error {
    let kind = match code as u32 {
        0xe000_02c1 | 0xe000_02e2 => io::ErrorKind::PermissionDenied,
        0xe000_02c2 => io::ErrorKind::InvalidInput,
        0xe000_02c7 => io::ErrorKind::Unsupported,
        0xe000_02d6 => io::ErrorKind::TimedOut,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, format!("{operation} failed (IOReturn 0x{:08x}); check Input Monitoring permission, tablet connection and competing drivers", code as u32))
}

/// The order matches upstream PermissionHelper: Input Monitoring first,
/// Accessibility second. This CLI emits instructions instead of Cocoa alerts.
pub fn input_permission() -> io::Result<()> {
    // SAFETY: permission APIs do not require device or process state pointers.
    if unsafe { ffi::IOHIDCheckAccess(LISTEN_EVENT) } != 0 {
        unsafe { ffi::IOHIDRequestAccess(LISTEN_EVENT) };
        return Err(io::Error::new(io::ErrorKind::PermissionDenied,
            "Input Monitoring permission is required. Enable the terminal/responsible application in System Settings > Privacy & Security > Input Monitoring, then quit and restart it. This release requires macOS 11 or later."));
    }
    Ok(())
}

pub fn output_permission() -> io::Result<()> {
    // SAFETY: these are read-only process permission checks.
    if unsafe { ffi::AXIsProcessTrusted() } == 0 || !unsafe { ffi::CGPreflightPostEventAccess() } {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied,
            "Accessibility permission is required for mouse output. Enable the terminal/responsible application in System Settings > Privacy & Security > Accessibility, then quit and restart it."));
    }
    Ok(())
}

pub struct Device {
    pub endpoint: Endpoint,
    pub name: String,
    handle: Owned,
    uses_report_ids: bool,
    usb_parent: Option<IoObject>,
    pub string_error: Option<String>,
}

impl Device {
    pub fn indexed_string(&self, index: u8) -> io::Result<String> {
        let service = self.usb_parent.as_ref().ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported,
            "USB parent service is unavailable for indexed string requests"))?;
        crate::usb::Strings::open(service.0)?.read(index)
    }
    fn indexed_string_checked(&self, index: u8, check: impl FnMut() -> io::Result<()>) -> io::Result<String> {
        let service = self.usb_parent.as_ref().ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported,
            "USB parent service is unavailable for indexed string requests"))?;
        crate::usb::Strings::open(service.0)?.read_checked(index, check)
    }
}

fn check_discovery(stop: Option<&AtomicBool>, deadline: Option<Instant>) -> io::Result<()> {
    if stop.is_some_and(|stop| stop.load(Ordering::Acquire)) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "USB discovery/initialization cancelled"));
    }
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "capture deadline elapsed during USB discovery/initialization"));
    }
    Ok(())
}

/// Enumerates services without opening unrelated keyboards or pointing devices.
/// Registry IDs identify live endpoints; the USB parent identifies the tablet.
pub fn enumerate(database: &Database, stop: Option<&AtomicBool>, deadline: Option<Instant>) -> io::Result<Vec<Device>> {
    let mut iterator = 0;
    // SAFETY: IOServiceMatching returns a dictionary consumed by matching.
    let matching = unsafe { ffi::IOServiceMatching(c"IOHIDDevice".as_ptr()) };
    if matching.is_null() { return Err(io::Error::other("cannot create HID matching dictionary")); }
    let result = unsafe { ffi::IOServiceGetMatchingServices(0, matching, &mut iterator) };
    if result != 0 { return Err(native_error("HID enumeration", result)); }
    let iterator = IoObject(iterator);
    let mut devices = Vec::new();
    loop {
        check_discovery(stop, deadline)?;
        let service = IoObject(unsafe { ffi::IOIteratorNext(iterator.0) });
        if service.0 == 0 { break; }
        let handle = Owned(unsafe { ffi::IOHIDDeviceCreate(ptr::null(), service.0) }.cast_const());
        if handle.0.is_null() { continue; }
        let hid = handle.0.cast_mut();
        if string(property(hid, "Transport")).as_deref() != Some("USB") { continue; }
        let Some(vendor) = number(property(hid, "VendorID")).and_then(|id| u16::try_from(id).ok()) else { continue; };
        let Some(product) = number(property(hid, "ProductID")).and_then(|id| u16::try_from(id).ok()) else { continue; };
        let data = property(hid, "ReportDescriptor");
        if data.is_null() || unsafe { ffi::CFGetTypeID(data) != ffi::CFDataGetTypeID() } { continue; }
        let length = unsafe { ffi::CFDataGetLength(data) };
        if !(1..=65535).contains(&length) { continue; }
        // SAFETY: CFData owns this byte region for the retained device lifetime.
        let bytes = unsafe { std::slice::from_raw_parts(ffi::CFDataGetBytePtr(data), length as usize) };
        let Some(lengths) = descriptor::lengths(bytes) else { continue; };
        let mut registry_id = 0;
        if unsafe { ffi::IORegistryEntryGetRegistryEntryID(service.0, &mut registry_id) } != 0 { continue; }
        let mut attributes = BTreeMap::new();
        let mut physical_id = format!("hid:{registry_id}");
        let mut usb_parent = None;
        // Traverse only the retained parent chain, collecting actual interface
        // and USB-device identity; never assume descriptor string indices.
        let mut current = service;
        for _ in 0..16 {
            if let Some(interface) = number(registry_property(current.0, "bInterfaceNumber").0) {
                attributes.entry("USB_INTERFACE_NUMBER".into()).or_insert_with(|| interface.to_string());
            }
            if number(registry_property(current.0, "idVendor").0).is_some() {
                let mut parent_id = 0;
                if unsafe { ffi::IORegistryEntryGetRegistryEntryID(current.0, &mut parent_id) } == 0 {
                    physical_id = format!("usb:{parent_id}");
                }
                usb_parent = Some(current);
                break;
            }
            let mut parent = 0;
            if unsafe { ffi::IORegistryEntryGetParentEntry(current.0, c"IOService".as_ptr(), &mut parent) } != 0 { break; }
            current = IoObject(parent);
        }
        let mut device = Device {
            endpoint: Endpoint {
                path: format!("IOHID:{registry_id}"), physical_id,
                transport: Transport::UsbHid, vendor_id: vendor, product_id: product,
                // Access is checked again immediately before opening. Unknown
                // TCC status must not prevent discovery or permission requests.
                can_open: unsafe { ffi::IOHIDCheckAccess(LISTEN_EVENT) } != 1,
                input_length: lengths.input, output_length: lengths.output,
                feature_length: lengths.feature, strings: BTreeMap::new(),
                attributes: Some(attributes),
            },
            name: string(property(hid, "Product")).unwrap_or_else(|| "USB HID device".into()),
            handle, uses_report_ids: lengths.uses_report_ids, usb_parent, string_error: None,
        };
        // Request only indices declared by plausible configured tablet endpoints.
        // Never open unrelated keyboards/mice or guess descriptor indices.
        let indices: std::collections::BTreeSet<u8> = database.find(vendor, product)
            .filter(|candidate| otd_core::endpoint_match::matches_report_lengths(&device.endpoint, candidate.identifier))
            .flat_map(|candidate| candidate.identifier.device_strings.iter().flat_map(BTreeMap::keys))
            .filter_map(|index| index.parse::<u8>().ok()).collect();
        if !indices.is_empty() {
            let result = (|| -> io::Result<()> {
                let parent = device.usb_parent.as_ref().ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported,
                    "USB parent service is unavailable for indexed string matching"))?;
                let mut strings = crate::usb::Strings::open(parent.0)?;
                for index in indices {
                    check_discovery(stop, deadline)?;
                    match strings.read_checked(index, || check_discovery(stop, deadline)) {
                        Ok(value) => { device.endpoint.strings.insert(index, value); }
                        Err(error) if matches!(error.kind(), io::ErrorKind::Interrupted | io::ErrorKind::TimedOut)
                            && (stop.is_some_and(|stop| stop.load(Ordering::Acquire)) || deadline.is_some_and(|deadline| Instant::now() >= deadline)) => return Err(error),
                        Err(error) => { device.string_error.get_or_insert_with(|| error.to_string()); }
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                if matches!(error.kind(), io::ErrorKind::Interrupted | io::ErrorKind::TimedOut) { return Err(error); }
                device.string_error = Some(error.to_string());
            }
        }
        devices.push(device);
    }
    devices.sort_by(|a, b| a.endpoint.path.cmp(&b.endpoint.path));
    Ok(devices)
}

struct Slot { bytes: [u8; MAX_REPORT], length: usize, ready: Instant }
struct Callbacks {
    slots: [Slot; QUEUE], head: usize, count: usize,
    uses_report_ids: bool, disconnected: bool, error: i32, overflow: bool,
    write_done: bool, write_result: i32,
}

unsafe extern "C" fn report_callback(context: *mut c_void, result: i32, _: *mut c_void, kind: u32, _: u32, bytes: *mut u8, length: isize) {
    // SAFETY: this stable box is owned by HidSource until unscheduled/closed.
    // All callbacks execute on its current CFRunLoop thread, synchronously.
    let state = unsafe { &mut *context.cast::<Callbacks>() };
    if result != 0 { state.error = result; return; }
    if kind != 0 || length <= 0 { return; }
    let offset = usize::from(!state.uses_report_ids);
    if bytes.is_null() || length as usize > MAX_REPORT - offset || state.count == QUEUE {
        state.overflow = true; return;
    }
    let index = (state.head + state.count) % QUEUE;
    let slot = &mut state.slots[index];
    slot.bytes[0] = 0;
    // SAFETY: native callback supplies length valid bytes; bounds checked above.
    unsafe { ptr::copy_nonoverlapping(bytes, slot.bytes.as_mut_ptr().add(offset), length as usize) };
    slot.length = length as usize + offset;
    slot.ready = Instant::now();
    state.count += 1;
}

unsafe extern "C" fn removed(context: *mut c_void, _: i32, _: *mut c_void) {
    unsafe { (*context.cast::<Callbacks>()).disconnected = true };
}

unsafe extern "C" fn write_completed(context: *mut c_void, result: i32, _: *mut c_void, _: u32, _: u32, _: *mut u8, _: isize) {
    let state = unsafe { &mut *context.cast::<Callbacks>() };
    state.write_result = result;
    state.write_done = true;
}

pub struct HidSource<'a> {
    device: &'a Device, stop: &'a AtomicBool, label: String,
    callbacks: Box<UnsafeCell<Callbacks>>, input: Box<UnsafeCell<[u8; MAX_REPORT]>>,
    delivery: [u8; MAX_REPORT], run_loop: ffi::Ref,
    // Buffers passed to asynchronous init calls stay alive through close even
    // on a timeout/stop. No subsequent write is issued before completion.
    pending_writes: Vec<Vec<u8>>,
    services:Option<ReaderServices>, registration:Option<Registration>,
    service:Option<Box<PendingService>>,
}
struct PendingService {call:ServiceCall,length:isize,offset:usize,done:bool,result:i32}
unsafe extern "C" fn service_completed(context:*mut c_void,result:i32,_:*mut c_void,_:u32,_:u32,_:*mut u8,length:isize){
    let pending=unsafe{&mut *context.cast::<PendingService>()};pending.result=result;pending.length=length;pending.done=true;
}

impl<'a> HidSource<'a> {
    pub fn open(device: &'a Device, label: String, stop: &'a AtomicBool) -> io::Result<Self> {
        input_permission()?;
        if device.endpoint.input_length == 0 || device.endpoint.input_length as usize > MAX_REPORT {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "endpoint input report length exceeds the 4096-byte macOS transport bound"));
        }
        let hid = device.handle.0.cast_mut();
        // Match upstream HidSharp's shared open. Seizing may require elevated
        // access and is not silently enabled for all HID devices.
        let result = unsafe { ffi::IOHIDDeviceOpen(hid, 0) };
        if result != 0 { return Err(native_error("opening USB HID endpoint", result)); }
        let now = Instant::now();
        let source = Self {
            device, stop, label,
            callbacks: Box::new(UnsafeCell::new(Callbacks {
                slots: std::array::from_fn(|_| Slot { bytes: [0; MAX_REPORT], length: 0, ready: now }),
                head: 0, count: 0, uses_report_ids: device.uses_report_ids,
                disconnected: false, error: 0, overflow: false,
                write_done: false, write_result: 0,
            })),
            input: Box::new(UnsafeCell::new([0; MAX_REPORT])), delivery: [0; MAX_REPORT],
            run_loop: unsafe { ffi::CFRunLoopGetCurrent() }, pending_writes: Vec::new(),services:None,registration:None,service:None,
        };
        let context = source.callbacks.get().cast();
        // SAFETY: callbacks and input are stable heap allocations, retained
        // until this source unregisters callbacks and closes the endpoint.
        unsafe {
            ffi::IOHIDDeviceRegisterInputReportCallback(hid, source.input.get().cast::<u8>(), MAX_REPORT as isize, Some(report_callback), context);
            ffi::IOHIDDeviceRegisterRemovalCallback(hid, Some(removed), context);
            ffi::IOHIDDeviceScheduleWithRunLoop(hid, source.run_loop, ffi::kCFRunLoopDefaultMode);
        }
        Ok(source)
    }

    pub fn attach(&mut self,configuration:&TabletConfiguration,identifier:&DeviceIdentifier,auxiliary:bool,gate:Arc<OutputGate>,epoch:Option<u64>)->io::Result<()> {
        let(io,services)=SharedIo::new(self.device.endpoint.output_length,self.device.endpoint.feature_length)?;
        let registration=Registration::new(self.device.endpoint.path.clone(),identifier.parser().into(),auxiliary,io,gate,
            self.device.endpoint.input_length as usize,serde_json::json!({"Properties":configuration,"Identifiers":[identifier]}),
            serde_json::json!(identifier),serde_json::json!(configuration),epoch).map_err(io::Error::other)?;
        self.services=Some(services);self.registration=Some(registration);Ok(())
    }
    pub fn initialized(&self){if let Some(registration)=&self.registration{registration.endpoint.initialized.store(true,Ordering::Release);}}
    pub fn tablet(&self,tablet:serde_json::Value){if let Some(registration)=&self.registration{registration.tablet(tablet);}}
    pub fn wait_pair(primary:&mut Self,_auxiliary:Option<&mut Self>,timeout:Duration)->io::Result<()>{
        // Both endpoints are scheduled on this same CFRunLoop. One pump services
        // their independent fixed queues and callback contexts.
        primary.pump(timeout.min(Duration::from_millis(50)));Ok(())
    }
    fn service_requests(&mut self)->io::Result<()> {
        if let Some(pending)=&self.service {
            if pending.done {
                let mut pending=self.service.take().expect("checked service");
                let result=if pending.result!=0{Err(native_error("HID service callback",pending.result))}else if pending.length<0||pending.length as usize+pending.offset>pending.call.data.len(){Err(io::Error::new(io::ErrorKind::InvalidData,"HID service returned an invalid length"))}else{Ok(())};
                if matches!(pending.call.kind,RequestKind::Report(Operation::GetFeature)){pending.call.data.truncate(pending.length.max(0) as usize+pending.offset);}
                pending.call.finish(result);
            }else if Instant::now()>=pending.call.deadline {
                // Close/unschedule in Drop happens before the retained call's
                // buffer is freed. Never reuse a timed-out callback context.
                return Err(io::Error::new(io::ErrorKind::TimedOut,"HID service callback timed out; reader retired safely"));
            }else{return Ok(());}
        }
        let Some(call)=self.services.as_mut().and_then(ReaderServices::next)else{return Ok(());};
        if let RequestKind::String(index)=call.kind {
            let mut call=call;let result=self.device.indexed_string_checked(index,||check_discovery(Some(self.stop),Some(call.deadline)))
                .map(|value|{call.data=value.into_bytes();});call.finish(result);return Ok(());
        }
        let operation=match call.kind{RequestKind::Report(operation)=>operation,RequestKind::String(_)=>unreachable!()};
        if !matches!(operation,Operation::WriteStream|Operation::GetFeature|Operation::SetFeature){call.finish(Err(io::Error::new(io::ErrorKind::InvalidInput,"Invalid HID service operation")));return Ok(());}
        let offset=usize::from(!self.device.uses_report_ids);
        if call.data.is_empty()||offset>call.data.len()||(!self.device.uses_report_ids&&call.data[0]!=0){call.finish(Err(io::Error::new(io::ErrorKind::InvalidInput,"Invalid unnumbered HID report")));return Ok(());}
        let mut pending=Box::new(PendingService{length:(call.data.len()-offset) as isize,call,offset,done:false,result:0});
        let context=(&mut *pending as *mut PendingService).cast();let hid=self.device.handle.0.cast_mut();
        let result=unsafe{if operation==Operation::GetFeature {
            ffi::IOHIDDeviceGetReportWithCallback(hid,2,isize::from(pending.call.data[0]),pending.call.data.as_mut_ptr().add(offset),&mut pending.length,1000.0,Some(service_completed),context)
        }else{ffi::IOHIDDeviceSetReportWithCallback(hid,if operation==Operation::SetFeature{2}else{1},isize::from(pending.call.data[0]),pending.call.data.as_ptr().add(offset),pending.length,1000.0,Some(service_completed),context)}};
        if result!=0{pending.call.finish(Err(native_error("submitting HID service",result)));}else{self.service=Some(pending);}Ok(())
    }

    // Callbacks run only while pump()/unschedule is inside native code. Never
    // retain a Rust state reference across either operation. UnsafeCell keeps
    // native writes valid when the source itself is borrowed by the session.
    fn state(&self) -> &Callbacks { unsafe { &*self.callbacks.get() } }
    fn state_mut(&mut self) -> &mut Callbacks { unsafe { &mut *self.callbacks.get() } }

    fn stopped(&self) -> bool { self.stop.load(Ordering::Acquire) || self.state().disconnected }

    fn pump(&mut self, duration: Duration) {
        // SAFETY: called only on the same thread which scheduled the HID device.
        unsafe { ffi::CFRunLoopRunInMode(ffi::kCFRunLoopDefaultMode, duration.as_secs_f64(), 1) };
    }

    pub fn initialize(&mut self, identifier: &DeviceIdentifier, configuration: &TabletConfiguration, capture_deadline: Option<Instant>) -> io::Result<()> {
        for index in identifier.initialization_strings.iter().flatten() {
            if self.stopped() { return Err(io::Error::new(io::ErrorKind::Interrupted, "initialization cancelled")); }
            if capture_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "capture deadline elapsed during string initialization"));
            }
            self.device.indexed_string_checked(*index, || check_discovery(Some(self.stop), capture_deadline))?;
        }
        let delay = configuration.attributes.as_ref().and_then(|a| a.get("FeatureInitDelayMs"))
            .map(|text| text.parse::<u32>().map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid FeatureInitDelayMs")))
            .transpose()?.unwrap_or(0);
        if delay == u32::MAX { return Err(io::Error::new(io::ErrorKind::InvalidInput, "infinite feature initialization delay is unsupported")); }
        for (kind, length, reports) in [
            (2u32, self.device.endpoint.feature_length, identifier.feature_init_report.as_deref()),
            (1u32, self.device.endpoint.output_length, identifier.output_init_report.as_deref()),
        ] {
            for report in reports.unwrap_or_default().iter().filter(|report| !report.0.is_empty()) {
                if length == 0 || length as usize > MAX_REPORT || report.0.len() > length as usize {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "initialization report exceeds endpoint report length or 4096-byte bound"));
                }
                if kind == 2 {
                    let deadline = Instant::now() + Duration::from_millis(u64::from(delay));
                    while Instant::now() < deadline {
                        if self.stopped() { return Err(io::Error::new(io::ErrorKind::Interrupted, "initialization cancelled")); }
                        if capture_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                            return Err(io::Error::new(io::ErrorKind::TimedOut, "capture deadline elapsed during device initialization"));
                        }
                        self.pump(deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(50)));
                    }
                }
                if self.stopped() { return Err(io::Error::new(io::ErrorKind::Interrupted, "initialization cancelled")); }
                if capture_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "capture deadline elapsed during device initialization"));
                }
                let mut data = vec![0u8; length as usize];
                data[..report.0.len()].copy_from_slice(&report.0);
                if !self.device.uses_report_ids && data[0] != 0 {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "nonzero initialization report ID on an unnumbered endpoint"));
                }
                self.pending_writes.push(data);
                let bytes = self.pending_writes.last().expect("just inserted initialization buffer");
                let offset = usize::from(!self.device.uses_report_ids);
                // SAFETY: no state reference survives the native call.
                unsafe { (*self.callbacks.get()).write_done = false };
                let context = self.callbacks.get().cast();
                // SAFETY: pending_writes retains bytes through callback/close.
                // Apple's timeout parameter is in milliseconds (IOHIDDevice.h).
                let result = unsafe { ffi::IOHIDDeviceSetReportWithCallback(self.device.handle.0.cast_mut(), kind,
                    isize::from(bytes[0]), bytes.as_ptr().add(offset), (bytes.len() - offset) as isize,
                    1000.0, Some(write_completed), context) };
                if result != 0 { return Err(native_error("submitting HID initialization report", result)); }
                let deadline = Instant::now() + Duration::from_secs(2);
                while !self.state().write_done {
                    if self.stopped() { return Err(io::Error::new(io::ErrorKind::Interrupted, "initialization cancelled")); }
                    if capture_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "capture deadline elapsed during device initialization"));
                    }
                    if Instant::now() >= deadline { return Err(io::Error::new(io::ErrorKind::TimedOut, "HID initialization report callback timed out")); }
                    self.pump(Duration::from_millis(50));
                }
                if self.state().write_result != 0 { return Err(native_error("HID initialization report", self.state().write_result)); }
                // Reports from before initialization are not valid session input.
                let state = self.state_mut();
                state.count = 0;
                state.head = 0;
                state.overflow = false;
            }
        }
        Ok(())
    }
}

impl ReportSource for HidSource<'_> {
    fn shared_output(&self)->bool{true}
    fn label(&self) -> &str { &self.label }
    fn now(&self) -> Instant { Instant::now() }
    fn native_output_enabled(&self)->bool{self.registration.as_ref().is_none_or(|registration|registration.endpoint.output.native_enabled())}
    fn output_started(&self){if let Some(registration)=&self.registration{registration.endpoint.output.start();}}
    fn output_acknowledged(&self,enabled:bool){if let Some(registration)=&self.registration{registration.endpoint.output.acknowledge(enabled);}}
    fn next(&mut self, timeout: Duration) -> io::Result<Read<'_>> {
        let queued = self.state().count > 0;
        let deadline = Instant::now() + timeout;
        loop {
            if self.stopped() { return Ok(Read::Ended); }
            self.service_requests()?;
            if let Some(registration)=&self.registration{if registration.endpoint.output.reader_wake().take_signal(){return Ok(Read::Idle);}}
            if self.state().error != 0 { return Err(native_error("HID report callback", self.state().error)); }
            if self.state().overflow { return Err(io::Error::new(io::ErrorKind::InvalidData, "macOS HID callback queue overflow or oversized report; session stopped instead of silently losing reports")); }
            if self.state().count > 0 {
                // SAFETY: no callback is running and this reference ends before
                // any pump; delivery and state are disjoint owned allocations.
                let state = unsafe { &mut *self.callbacks.get() };
                let slot = &state.slots[state.head];
                let length = slot.length;
                let ready = slot.ready;
                self.delivery[..length].copy_from_slice(&slot.bytes[..length]);
                state.head = (state.head + 1) % QUEUE;
                state.count -= 1;
                if let Some(registration)=&self.registration{registration.publish(&self.delivery[..length]);}
                return Ok(Read::Report { bytes: &self.delivery[..length], ready, queued });
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { return Ok(Read::Idle); }
            self.pump(remaining.min(Duration::from_millis(50)));
        }
    }
}

impl Drop for HidSource<'_> {
    fn drop(&mut self) {
        let hid = self.device.handle.0.cast_mut();
        // SAFETY: close cancels active I/O; unscheduling prevents subsequent
        // callback dispatch on this runloop before storage is dropped.
        unsafe {
            ffi::IOHIDDeviceClose(hid, 0);
            ffi::IOHIDDeviceUnscheduleFromRunLoop(hid, self.run_loop, ffi::kCFRunLoopDefaultMode);
            ffi::IOHIDDeviceRegisterInputReportCallback(hid, self.input.get().cast::<u8>(), MAX_REPORT as isize, None, ptr::null_mut());
            ffi::IOHIDDeviceRegisterRemovalCallback(hid, None, ptr::null_mut());
        }
    }
}

pub struct NativeDisplays {
    explicit: Option<Rect>,
    pub geometry: Rc<Cell<Rect>>,
    last: DisplayFingerprint,
    layout: [Rect; 32],
    changed: bool,
}

fn active_displays() -> io::Result<([Rect; 32], usize, Rect)> {
    let mut ids = [0u32; 32];
    let mut count = 0;
    // SAFETY: display IDs are written into fixed-size storage.
    let result = unsafe { ffi::CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) };
    if result != 0 { return Err(io::Error::other(format!("CoreGraphics display enumeration failed ({result})"))); }
    if count == 0 || count as usize >= ids.len() {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "need between 1 and 31 active displays"));
    }
    let mut rectangles = [Rect { left: 0, top: 0, right: 0, bottom: 0 }; 32];
    for (index, &id) in ids[..count as usize].iter().enumerate() {
        let bounds = unsafe { ffi::CGDisplayBounds(id) };
        let coordinates = [bounds.origin.x, bounds.origin.y,
            bounds.origin.x + bounds.size.width, bounds.origin.y + bounds.size.height];
        if coordinates.iter().any(|coordinate| !coordinate.is_finite() || *coordinate < -1_000_000.0 || *coordinate > 1_000_000.0) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid CoreGraphics display bounds"));
        }
        rectangles[index] = Rect { left: coordinates[0].round() as i32, top: coordinates[1].round() as i32,
            right: coordinates[2].round() as i32, bottom: coordinates[3].round() as i32 };
        if !rectangles[index].valid() { return Err(io::Error::new(io::ErrorKind::InvalidData, "empty display bounds")); }
    }
    rectangles[..count as usize].sort_unstable_by_key(|r| (r.left, r.top, r.right, r.bottom));
    let mut virtual_screen = rectangles[0];
    for rectangle in &rectangles[1..count as usize] {
        virtual_screen.left = virtual_screen.left.min(rectangle.left);
        virtual_screen.top = virtual_screen.top.min(rectangle.top);
        virtual_screen.right = virtual_screen.right.max(rectangle.right);
        virtual_screen.bottom = virtual_screen.bottom.max(rectangle.bottom);
    }
    Ok((rectangles, count as usize, virtual_screen))
}

impl NativeDisplays {
    pub fn new(explicit: Option<Rect>) -> io::Result<Self> {
        let (layout, monitors, screen) = if let Some(rectangle) = explicit {
            let mut layout = [rectangle; 32];
            layout[1..].fill(Rect { left: 0, top: 0, right: 0, bottom: 0 });
            (layout, 1, rectangle)
        } else { let (layout, count, screen) = active_displays()?; (layout, count as i32, screen) };
        Ok(Self { explicit, geometry: Rc::new(Cell::new(screen)),
            last: DisplayFingerprint { virtual_screen: screen, monitors }, layout, changed: false })
    }
}

impl Displays for NativeDisplays {
    fn fingerprint(&mut self) -> DisplayFingerprint {
        if self.explicit.is_none() {
            if let Ok((layout, count, screen)) = active_displays() {
                self.changed |= layout != self.layout;
                self.layout = layout;
                self.last = DisplayFingerprint { virtual_screen: screen, monitors: count as i32 };
                self.geometry.set(screen);
            }
        }
        self.last
    }
    fn topology_changed(&mut self) -> bool { self.changed }
    fn snapshot(&mut self) -> Result<DisplaySnapshot, String> {
        if let Some(screen) = self.explicit { return Ok(DisplaySnapshot { virtual_screen: screen, monitors: vec![screen] }); }
        let (rectangles, count, screen) = active_displays().map_err(|error| error.to_string())?;
        self.geometry.set(screen);
        self.last = DisplayFingerprint { virtual_screen: screen, monitors: count as i32 };
        self.layout = rectangles;
        self.changed = false;
        Ok(DisplaySnapshot { virtual_screen: screen, monitors: rectangles[..count].to_vec() })
    }
}

pub struct Mouse {
    _source: Owned, moved: Owned, dragged: Owned, down: Owned, up: Owned,
    contact_owner:otd_platform::input_owner::Native,
    geometry: Rc<Cell<Rect>>, contact: bool, last_absolute: Option<ffi::Point>,
    last_position: Option<ffi::Point>, buttons: u8, modifiers: u8, pending_clear_flags: u64,
    clear_flags_deadline: Option<Instant>,
}

impl Mouse {
    pub fn send_scroll(&mut self, pulse: ScrollPulse) -> io::Result<()> {
        // Match MacOSVirtualMouse: negate the binding pulse and use pixel units.
        // Separate scroll events preserve the reusable mouse event objects.
        let amount = pulse.delta.wrapping_neg();
        let (vertical, horizontal) = match pulse.axis {
            ScrollAxis::Vertical => (amount, 0), ScrollAxis::Horizontal => (0, amount),
        };
        let event = Owned(unsafe { ffi::CGEventCreateScrollWheelEvent2(ptr::null(), 0, 2, vertical, horizontal, 0) });
        if event.0.is_null() { return Err(io::Error::other("cannot create CoreGraphics scroll event")); }
        let flags = self.event_flags();
        unsafe {
            ffi::CGEventSetFlags(event.0, flags);
            ffi::CGEventSetTimestamp(event.0, event_timestamp()?);
            ffi::CGEventPost(0, event.0);
        }
        Ok(())
    }

    pub fn new(geometry: Rc<Cell<Rect>>) -> io::Result<Self> {
        output_permission()?;
        ensure_shared_inputs()?;
        let contact_owner=otd_platform::input_owner::Native::new()?;
        // SAFETY: private CGEvent source and distinct reusable mouse event
        // objects avoid changing a union between incompatible event families.
        let source = Owned(unsafe { ffi::CGEventSourceCreate(-1) });
        if source.0.is_null() { return Err(io::Error::other("cannot create CoreGraphics event source")); }
        let create = |kind| Owned(unsafe { ffi::CGEventCreateMouseEvent(source.0, kind, ffi::Point::default(), 0) });
        let moved = create(5);
        let dragged = create(6);
        let down = create(1);
        let up = create(2);
        if [moved.0, dragged.0, down.0, up.0].iter().any(|event| event.is_null()) {
            return Err(io::Error::other("cannot create CoreGraphics mouse events"));
        }
        Ok(Self { _source: source, moved, dragged, down, up, contact_owner, geometry, contact: false, last_absolute: None,
            last_position: None, buttons: 0, modifiers: 0, pending_clear_flags: 0, clear_flags_deadline: None })
    }

    pub fn send(&mut self, packet: MousePacket) -> io::Result<()> {
        let moving = packet.flags & flags::MOVE != 0;
        let absolute = packet.flags & flags::ABSOLUTE != 0;
        let mut delta = ffi::Point::default();
        let position = if moving && absolute {
            let screen = self.geometry.get();
            let position = ffi::Point { x: f64::from(screen.left) + f64::from(packet.dx) / 65535.0 * f64::from(screen.width() - 1),
                y: f64::from(screen.top) + f64::from(packet.dy) / 65535.0 * f64::from(screen.height() - 1) };
            if let Some(previous) = self.last_absolute {
                delta = ffi::Point { x: position.x - previous.x, y: position.y - previous.y };
            }
            position
        } else {
            // Relative motion must start at the current system cursor, as in
            // pinned MacOSRelativePointer. CGEventCreate allocates natively;
            // there is no Rust heap allocation on the successful report path.
            let query = Owned(unsafe { ffi::CGEventCreate(ptr::null()) });
            if query.0.is_null() { return Err(io::Error::other("cannot query current cursor position")); }
            let mut position = unsafe { ffi::CGEventGetLocation(query.0) };
            if moving {
                delta = ffi::Point { x: f64::from(packet.dx), y: f64::from(packet.dy) };
                position.x += delta.x;
                position.y += delta.y;
            }
            position
        };
        let contact = if packet.flags & flags::LEFTDOWN != 0 { true }
            else if packet.flags & flags::LEFTUP != 0 { false } else { self.contact };
        if packet.flags&(flags::LEFTDOWN|flags::LEFTUP)!=0{self.contact_owner.hold_at(otd_platform::input_owner::Code::Button(0),contact,Some((position.x,position.y)))?;}
        self.buttons=otd_platform::input_owner::buttons();let left=self.buttons&1!=0;
        if !moving {
            self.contact = contact;
            self.last_position=Some(position);if moving{self.last_absolute=absolute.then_some(position);}
            return Ok(());
        }
        // Shared ownership emits contact edges. Every moving packet still
        // updates the cursor even if another input scope changes its buttons.
        let (event, kind, button) = if left { (self.dragged.0, 6, 0) }
            else if self.buttons & 2 != 0 { (self.dragged.0, 7, 1) }
            else if self.buttons & 0x1c != 0 { (self.dragged.0, 27, (self.buttons & 0x1c).trailing_zeros()) }
            else { (self.moved.0, 5, 0) };
        let mut clock = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: writable timespec, nanoseconds since boot as required by
        // CGEventTimestamp. Refresh timestamps on the reusable event objects.
        if unsafe { libc::clock_gettime(libc::CLOCK_UPTIME_RAW, &mut clock) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let timestamp = (clock.tv_sec as u64).saturating_mul(1_000_000_000).saturating_add(clock.tv_nsec as u64);
        // SAFETY: valid reusable event; public fields 4/5 are delta X/Y,
        // 1 is click state, 3 is button number.
        let event_flags = self.event_flags();
        unsafe {
            ffi::CGEventSetType(event, kind);
            ffi::CGEventSetLocation(event, position);
            ffi::CGEventSetDoubleValueField(event, 4, delta.x);
            ffi::CGEventSetDoubleValueField(event, 5, delta.y);
            ffi::CGEventSetIntegerValueField(event, 3, i64::from(button));
            ffi::CGEventSetIntegerValueField(event, 1, i64::from(kind == 1 || kind == 2));
            ffi::CGEventSetFlags(event, event_flags);
            ffi::CGEventSetTimestamp(event, timestamp);
            ffi::CGEventPost(0, event);
        }
        self.contact = contact;
        self.last_position = Some(position);
        if moving { self.last_absolute = absolute.then_some(position); }
        // CGEventPost has no return value; TCC acceptance/application delivery
        // remains a physical macOS validation gate, not an acknowledged write.
        Ok(())
    }

    fn event_flags(&mut self) -> u64 {
        let modifiers=otd_platform::input_owner::key_mask([59,56,58,55,62,60,61,54]);
        let released=crate::keymap::modifier_flags(self.modifiers)&!crate::keymap::modifier_flags(modifiers);self.modifiers=modifiers;
        if released!=0{self.pending_clear_flags|=released;self.clear_flags_deadline=Some(Instant::now()+Duration::from_millis(50));}
        // CGEventSourceFlagsState can lag behind a posted release. Suppress
        // released synthetic flags while its snapshot catches up. Bound this
        // to 50 ms so a newly pressed physical modifier cannot stay masked.
        let observed = unsafe { ffi::CGEventSourceFlagsState(1) };
        self.pending_clear_flags &= observed;
        if self.pending_clear_flags == 0 || self.clear_flags_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            self.pending_clear_flags = 0;
            self.clear_flags_deadline = None;
        }
        (observed & !self.pending_clear_flags) | crate::keymap::modifier_flags(self.modifiers)
    }

    fn send_action(&mut self, transition: ActionTransition) -> io::Result<()> {
        match transition.action {
            Action::Key(key) => {
                let code = crate::keymap::key_code(key).ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "unsupported macOS keyboard usage"))?;
                // Fresh native keyboard events translate the current key code;
                // successful Rust-owned processing still allocates no heap.
                let event = Owned(unsafe { ffi::CGEventCreateKeyboardEvent(self._source.0, code, transition.pressed) });
                if event.0.is_null() { return Err(io::Error::other("cannot create CoreGraphics keyboard event")); }
                let timestamp = event_timestamp()?;
                let old_flags = crate::keymap::modifier_flags(self.modifiers);
                if key.is_modifier() {
                    let bit = 1 << (key.usage() - 0xe0);
                    if transition.pressed { self.modifiers |= bit; } else { self.modifiers &= !bit; }
                }
                let released_flags = old_flags & !crate::keymap::modifier_flags(self.modifiers);
                self.pending_clear_flags |= released_flags;
                if released_flags != 0 { self.clear_flags_deadline = Some(Instant::now() + Duration::from_millis(50)); }
                let event_flags = self.event_flags();
                unsafe {
                    ffi::CGEventSetIntegerValueField(event.0, 8, 0); // no autorepeat
                    ffi::CGEventSetFlags(event.0, event_flags);
                    ffi::CGEventSetTimestamp(event.0, timestamp);
                    ffi::CGEventPost(0, event.0);
                }
            }
            Action::Mouse(button) => {
                let number = match button { MouseButton::Left => 0, MouseButton::Right => 1,
                    MouseButton::Middle => 2, MouseButton::Backward => 3, MouseButton::Forward => 4 };
                let next = if transition.pressed { self.buttons | (1 << number) } else { self.buttons & !(1 << number) };
                let was_pressed = self.buttons & (1 << number) != 0 || number == 0 && self.contact;
                let pressed = next & (1 << number) != 0 || number == 0 && self.contact;
                if was_pressed == pressed { self.buttons = next; return Ok(()); }
                let position = match self.last_position {
                    Some(position) => position,
                    None => {
                        let query = Owned(unsafe { ffi::CGEventCreate(ptr::null()) });
                        if query.0.is_null() { return Err(io::Error::other("cannot query current cursor position")); }
                        unsafe { ffi::CGEventGetLocation(query.0) }
                    }
                };
                let kind = match (number, pressed) { (0, true) => 1, (0, false) => 2,
                    (1, true) => 3, (1, false) => 4, (_, true) => 25, (_, false) => 26 };
                let event = if pressed { self.down.0 } else { self.up.0 };
                let timestamp = event_timestamp()?;
                let event_flags = self.event_flags();
                unsafe {
                    ffi::CGEventSetType(event, kind);
                    ffi::CGEventSetLocation(event, position);
                    ffi::CGEventSetIntegerValueField(event, 3, i64::from(number));
                    ffi::CGEventSetIntegerValueField(event, 1, 1);
                    ffi::CGEventSetDoubleValueField(event, 4, 0.0);
                    ffi::CGEventSetDoubleValueField(event, 5, 0.0);
                    ffi::CGEventSetFlags(event, event_flags);
                    ffi::CGEventSetTimestamp(event, timestamp);
                    ffi::CGEventPost(0, event);
                }
                self.buttons = next;
            }
        }
        Ok(())
    }
}

fn event_timestamp() -> io::Result<u64> {
    let mut clock = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_UPTIME_RAW, &mut clock) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((clock.tv_sec as u64).saturating_mul(1_000_000_000).saturating_add(clock.tv_nsec as u64))
}

/// Shares native pointer state with tip output so one hold cannot release the
/// other's left button, and drag events see the held side buttons/modifiers.
pub fn action_sink(mouse: Rc<RefCell<Mouse>>) -> io::Result<Box<dyn ActionSink>> {
    ensure_shared_inputs()?;let owner=otd_platform::input_owner::Native::new()?;
    let scroll = Rc::clone(&mouse);
    Ok(Box::new(LocalActions::new(move |transition:ActionTransition|{
        let code=match transition.action{Action::Key(key)=>otd_platform::input_owner::Code::Key(crate::keymap::key_code(key).ok_or_else(||io::Error::new(io::ErrorKind::Unsupported,"Unsupported macOS keyboard usage"))?),
            Action::Mouse(button)=>otd_platform::input_owner::Code::Button(match button{MouseButton::Left=>0,MouseButton::Right=>1,MouseButton::Middle=>2,MouseButton::Backward=>3,MouseButton::Forward=>4})};
        let action=match transition.action{Action::Key(key)=>key.usage() as u32,Action::Mouse(_)=>0x10000+match code{otd_platform::input_owner::Code::Button(button)=>button as u32,_=>unreachable!()}};
        owner.hold_action(action,code,transition.pressed,None)
    },
        |action| match action { Action::Mouse(_) => true, Action::Key(key) => crate::keymap::key_code(key).is_some() })
        .with_scroll(move |pulse| scroll.borrow_mut().send_scroll(pulse))))
}
impl otd_platform::managed_source::NativeSource for HidSource<'_>{
    fn tablet(&self,value:serde_json::Value){HidSource::tablet(self,value)}
    fn initialized(&self){HidSource::initialized(self)}
    fn waits(&self)->[libc::pollfd;3]{[libc::pollfd{fd:-1,events:0,revents:0},
        libc::pollfd{fd:self.services.as_ref().map_or(-1,|services|services.wake().raw()),events:libc::POLLIN,revents:0},
        libc::pollfd{fd:self.registration.as_ref().map_or(-1,|registration|registration.endpoint.output.reader_wake().raw()),events:libc::POLLIN,revents:0}]}
    fn pump_native(&mut self,timeout:Duration)->io::Result<()>{self.pump(timeout.min(Duration::from_millis(50)));Ok(())}
}
impl otd_platform::paired_source::Retire for HidSource<'_>{}

// CGEvent objects can be posted from any thread. The shared owner mutex is the
// sole accessor; no CFRunLoop or tablet-thread resource is borrowed here.
struct SharedInput {source:Owned,down:Owned,up:Owned,modifiers:u8,clear:u64,clear_until:Option<Instant>}
unsafe impl Send for SharedInput {}
impl SharedInput {
    fn flags(&mut self)->u64{let observed=unsafe{ffi::CGEventSourceFlagsState(1)};self.clear&=observed;if self.clear_until.is_some_and(|until|Instant::now()>=until){self.clear=0;self.clear_until=None;}(observed&!self.clear)|crate::keymap::modifier_flags(self.modifiers)}
    fn send(&mut self,code:otd_platform::input_owner::Code,held:bool,position:Option<(f64,f64)>)->io::Result<()>{
        let timestamp=event_timestamp()?;
        match code{
            otd_platform::input_owner::Code::Key(code)=>{
                let event=Owned(unsafe{ffi::CGEventCreateKeyboardEvent(self.source.0,code,held)});if event.0.is_null(){return Err(io::Error::other("Cannot create original keyboard event"));}
                let old=crate::keymap::modifier_flags(self.modifiers);
                if let Some(bit)=[59,56,58,55,62,60,61,54].iter().position(|modifier|*modifier==code){if held{self.modifiers|=1<<bit;}else{self.modifiers&=!(1<<bit);}}
                let released=old&!crate::keymap::modifier_flags(self.modifiers);if released!=0{self.clear|=released;self.clear_until=Some(Instant::now()+Duration::from_millis(50));}
                let flags=self.flags();unsafe{ffi::CGEventSetIntegerValueField(event.0,8,0);ffi::CGEventSetFlags(event.0,flags);ffi::CGEventSetTimestamp(event.0,timestamp);ffi::CGEventPost(0,event.0);}
            },
            otd_platform::input_owner::Code::Button(button)=>{
                let point=match position{Some((x,y))=>ffi::Point{x,y},None=>{let query=Owned(unsafe{ffi::CGEventCreate(ptr::null())});if query.0.is_null(){return Err(io::Error::other("Cannot query cursor for original mouse binding"));}unsafe{ffi::CGEventGetLocation(query.0)}}};
                let kind=match(button,held){(0,true)=>1,(0,false)=>2,(1,true)=>3,(1,false)=>4,(_,true)=>25,(_,false)=>26};let event=if held{self.down.0}else{self.up.0};let flags=self.flags();
                unsafe{ffi::CGEventSetType(event,kind);ffi::CGEventSetLocation(event,point);ffi::CGEventSetIntegerValueField(event,3,button.into());ffi::CGEventSetIntegerValueField(event,1,1);ffi::CGEventSetDoubleValueField(event,4,0.0);ffi::CGEventSetDoubleValueField(event,5,0.0);ffi::CGEventSetFlags(event,flags);ffi::CGEventSetTimestamp(event,timestamp);ffi::CGEventPost(0,event);}
            }
        }Ok(())
    }
}
pub fn ensure_shared_inputs()->io::Result<()>{otd_platform::input_owner::ensure(||{
    output_permission()?;let source=Owned(unsafe{ffi::CGEventSourceCreate(-1)});if source.0.is_null(){return Err(io::Error::other("Cannot create shared event source"));}
    let down=Owned(unsafe{ffi::CGEventCreateMouseEvent(source.0,1,ffi::Point::default(),0)});let up=Owned(unsafe{ffi::CGEventCreateMouseEvent(source.0,2,ffi::Point::default(),0)});
    if down.0.is_null()||up.0.is_null(){return Err(io::Error::other("Cannot create shared mouse events"));}
    let mut writer=SharedInput{source,down,up,modifiers:0,clear:0,clear_until:None};Ok(Box::new(move|code,held,position|writer.send(code,held,position)))
})}

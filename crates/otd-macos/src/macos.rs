//! USB IOKit HID transport and CoreGraphics logical-coordinate mouse output.
//! The single-threaded CFRunLoop owns every callback and its fixed queue.

use std::cell::{Cell, UnsafeCell};
use std::collections::BTreeMap;
use std::ffi::{CString, c_void};
use std::io;
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use otd_core::display::{DisplayFingerprint, DisplaySnapshot};
use otd_core::endpoint_match::{Endpoint, Transport};
use otd_core::mapping::Rect;
use otd_core::output::{MousePacket, flags};
use otd_core::session::{Displays, Read, ReportSource};
use otd_core::tablets::{DeviceIdentifier, TabletConfiguration};

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
}

/// Enumerates services without opening unrelated keyboards or pointing devices.
/// Registry IDs identify live endpoints; the USB parent identifies the tablet.
pub fn enumerate() -> io::Result<Vec<Device>> {
    let mut iterator = 0;
    // SAFETY: IOServiceMatching returns a dictionary consumed by matching.
    let matching = unsafe { ffi::IOServiceMatching(c"IOHIDDevice".as_ptr()) };
    if matching.is_null() { return Err(io::Error::other("cannot create HID matching dictionary")); }
    let result = unsafe { ffi::IOServiceGetMatchingServices(0, matching, &mut iterator) };
    if result != 0 { return Err(native_error("HID enumeration", result)); }
    let iterator = IoObject(iterator);
    let mut devices = Vec::new();
    loop {
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
                break;
            }
            let mut parent = 0;
            if unsafe { ffi::IORegistryEntryGetParentEntry(current.0, c"IOService".as_ptr(), &mut parent) } != 0 { break; }
            current = IoObject(parent);
        }
        devices.push(Device {
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
            handle, uses_report_ids: lengths.uses_report_ids,
        });
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
            run_loop: unsafe { ffi::CFRunLoopGetCurrent() }, pending_writes: Vec::new(),
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
        if identifier.initialization_strings.as_ref().is_some_and(|strings| !strings.is_empty()) {
            return Err(io::Error::new(io::ErrorKind::Unsupported,
                "this tablet requires USB initialization string requests, which the macOS CLI backend does not implement; refusing partial initialization"));
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
    fn label(&self) -> &str { &self.label }
    fn now(&self) -> Instant { Instant::now() }
    fn next(&mut self, timeout: Duration) -> io::Result<Read<'_>> {
        let queued = self.state().count > 0;
        let deadline = Instant::now() + timeout;
        loop {
            if self.stopped() { return Ok(Read::Ended); }
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
        let (screen, monitors) = if let Some(rectangle) = explicit { (rectangle, 1) }
            else { let (_, count, screen) = active_displays()?; (screen, count as i32) };
        Ok(Self { explicit, geometry: Rc::new(Cell::new(screen)),
            last: DisplayFingerprint { virtual_screen: screen, monitors } })
    }
}

impl Displays for NativeDisplays {
    fn fingerprint(&mut self) -> DisplayFingerprint {
        if self.explicit.is_none() {
            if let Ok((_, count, screen)) = active_displays() {
                self.last = DisplayFingerprint { virtual_screen: screen, monitors: count as i32 };
                self.geometry.set(screen);
            }
        }
        self.last
    }
    fn snapshot(&mut self) -> Result<DisplaySnapshot, String> {
        if let Some(screen) = self.explicit { return Ok(DisplaySnapshot { virtual_screen: screen, monitors: vec![screen] }); }
        let (rectangles, count, screen) = active_displays().map_err(|error| error.to_string())?;
        self.geometry.set(screen);
        self.last = DisplayFingerprint { virtual_screen: screen, monitors: count as i32 };
        Ok(DisplaySnapshot { virtual_screen: screen, monitors: rectangles[..count].to_vec() })
    }
}

pub struct Mouse {
    _source: Owned, moved: Owned, dragged: Owned, down: Owned, up: Owned,
    geometry: Rc<Cell<Rect>>, contact: bool, last_absolute: Option<ffi::Point>,
}

impl Mouse {
    pub fn new(geometry: Rc<Cell<Rect>>) -> io::Result<Self> {
        output_permission()?;
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
        Ok(Self { _source: source, moved, dragged, down, up, geometry, contact: false, last_absolute: None })
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
        let (event, kind, contact) = if packet.flags & flags::LEFTDOWN != 0 { (self.down.0, 1, true) }
            else if packet.flags & flags::LEFTUP != 0 { (self.up.0, 2, false) }
            else if self.contact { (self.dragged.0, 6, true) }
            else { (self.moved.0, 5, false) };
        let mut clock = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: writable timespec, nanoseconds since boot as required by
        // CGEventTimestamp. Refresh timestamps on the reusable event objects.
        if unsafe { libc::clock_gettime(libc::CLOCK_UPTIME_RAW, &mut clock) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let timestamp = (clock.tv_sec as u64).saturating_mul(1_000_000_000).saturating_add(clock.tv_nsec as u64);
        // SAFETY: valid reusable event; public fields 4/5 are delta X/Y,
        // 1 is click state, 3 is button number.
        unsafe {
            ffi::CGEventSetType(event, kind);
            ffi::CGEventSetLocation(event, position);
            ffi::CGEventSetDoubleValueField(event, 4, delta.x);
            ffi::CGEventSetDoubleValueField(event, 5, delta.y);
            ffi::CGEventSetIntegerValueField(event, 3, 0);
            ffi::CGEventSetIntegerValueField(event, 1, i64::from(kind == 1 || kind == 2));
            ffi::CGEventSetFlags(event, ffi::CGEventSourceFlagsState(1));
            ffi::CGEventSetTimestamp(event, timestamp);
            ffi::CGEventPost(0, event);
        }
        self.contact = contact;
        if moving { self.last_absolute = absolute.then_some(position); }
        // CGEventPost has no return value; TCC acceptance/application delivery
        // remains a physical macOS validation gate, not an acknowledged write.
        Ok(())
    }
}

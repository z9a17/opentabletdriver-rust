//! Narrow declarations from Apple's IOKit/CoreFoundation/CoreGraphics headers.
//! All owning references are released by the backend; callbacks use the
//! current thread's CFRunLoop, never a dispatch queue or background thread.

use std::ffi::{c_char, c_void};

pub type Ref = *const c_void;
pub type Hid = *mut c_void;
pub type ReportCallback = unsafe extern "C" fn(*mut c_void, i32, *mut c_void, u32, u32, *mut u8, isize);
pub type RemovalCallback = unsafe extern "C" fn(*mut c_void, i32, *mut c_void);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Point { pub x: f64, pub y: f64 }

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Size { pub width: f64, pub height: f64 }

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Rect { pub origin: Point, pub size: Size }

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub static kCFRunLoopDefaultMode: Ref;
    pub fn CFRelease(value: Ref);
    pub fn CFGetTypeID(value: Ref) -> usize;
    pub fn CFStringGetTypeID() -> usize;
    pub fn CFNumberGetTypeID() -> usize;
    pub fn CFDataGetTypeID() -> usize;
    pub fn CFStringCreateWithCString(allocator: Ref, text: *const c_char, encoding: u32) -> Ref;
    pub fn CFStringGetCString(value: Ref, buffer: *mut c_char, length: isize, encoding: u32) -> u8;
    pub fn CFNumberGetValue(value: Ref, kind: isize, output: *mut c_void) -> u8;
    pub fn CFDataGetBytePtr(value: Ref) -> *const u8;
    pub fn CFDataGetLength(value: Ref) -> isize;
    pub fn CFRunLoopGetCurrent() -> Ref;
    pub fn CFRunLoopRunInMode(mode: Ref, seconds: f64, return_after_source: u8) -> i32;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    pub fn IOServiceMatching(name: *const c_char) -> Ref;
    pub fn IOServiceGetMatchingServices(port: u32, matching: Ref, iterator: *mut u32) -> i32;
    pub fn IOIteratorNext(iterator: u32) -> u32;
    pub fn IOObjectRelease(object: u32) -> i32;
    pub fn IORegistryEntryGetRegistryEntryID(entry: u32, identifier: *mut u64) -> i32;
    pub fn IORegistryEntryGetParentEntry(entry: u32, plane: *const c_char, parent: *mut u32) -> i32;
    pub fn IORegistryEntryCreateCFProperty(entry: u32, key: Ref, allocator: Ref, options: u32) -> Ref;
    pub fn IOHIDDeviceCreate(allocator: Ref, service: u32) -> Hid;
    pub fn IOHIDDeviceGetProperty(device: Hid, key: Ref) -> Ref;
    pub fn IOHIDDeviceOpen(device: Hid, options: u32) -> i32;
    pub fn IOHIDDeviceClose(device: Hid, options: u32) -> i32;
    pub fn IOHIDDeviceRegisterInputReportCallback(device: Hid, report: *mut u8, length: isize, callback: Option<ReportCallback>, context: *mut c_void);
    pub fn IOHIDDeviceRegisterRemovalCallback(device: Hid, callback: Option<RemovalCallback>, context: *mut c_void);
    pub fn IOHIDDeviceScheduleWithRunLoop(device: Hid, run_loop: Ref, mode: Ref);
    pub fn IOHIDDeviceUnscheduleFromRunLoop(device: Hid, run_loop: Ref, mode: Ref);
    pub fn IOHIDDeviceSetReportWithCallback(device: Hid, kind: u32, id: isize, report: *const u8, length: isize, timeout: f64, callback: Option<ReportCallback>, context: *mut c_void) -> i32;
    pub fn IOHIDCheckAccess(request: u32) -> u32;
    pub fn IOHIDRequestAccess(request: u32) -> bool;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub fn AXIsProcessTrusted() -> u8;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub fn CGPreflightPostEventAccess() -> bool;
    pub fn CGGetActiveDisplayList(maximum: u32, displays: *mut u32, count: *mut u32) -> i32;
    pub fn CGDisplayBounds(display: u32) -> Rect;
    pub fn CGEventSourceCreate(state: i32) -> Ref;
    pub fn CGEventCreate(source: Ref) -> Ref;
    pub fn CGEventCreateMouseEvent(source: Ref, kind: u32, position: Point, button: u32) -> Ref;
    pub fn CGEventCreateKeyboardEvent(source: Ref, key: u16, down: bool) -> Ref;
    pub fn CGEventCreateScrollWheelEvent2(source: Ref, units: u32, wheels: u32, wheel1: i32, wheel2: i32, wheel3: i32) -> Ref;
    pub fn CGEventGetLocation(event: Ref) -> Point;
    pub fn CGEventSetLocation(event: Ref, position: Point);
    pub fn CGEventSetType(event: Ref, kind: u32);
    pub fn CGEventSetIntegerValueField(event: Ref, field: u32, value: i64);
    pub fn CGEventSetDoubleValueField(event: Ref, field: u32, value: f64);
    pub fn CGEventSetFlags(event: Ref, flags: u64);
    pub fn CGEventSourceFlagsState(state: i32) -> u64;
    pub fn CGEventSetTimestamp(event: Ref, timestamp: u64);
    pub fn CGEventPost(tap: u32, event: Ref);
}

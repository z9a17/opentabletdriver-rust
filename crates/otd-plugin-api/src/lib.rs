//! Native position-filter ABI. All calls use the C ABI on one driver thread.
//! No Rust-owned values or allocations may cross the DLL boundary. Callbacks
//! must not unwind, block, allocate per report, or retain the sample pointer.
use core::ffi::c_void;

pub const ABI_VERSION: u32 = 1;
pub const PROXIMITY: u32 = 1;
pub const ERASER: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    /// Raw PTH-660 units. Only x and y are used from the returned sample.
    pub x: f32,
    pub y: f32,
    /// Monotonic nanoseconds since this filter chain was loaded.
    pub time_ns: u64,
    pub pressure: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Header {
    pub abi_version: u32,
    pub struct_size: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FilterApi {
    pub header: Header,
    /// NUL-terminated UTF-8, stored inline to avoid borrowed string ownership.
    pub name: [u8; 64],
    /// UTF-8 JSON configuration is valid for this call only. Null means failure.
    pub create: Option<unsafe extern "C" fn(json: *const u8, len: usize) -> *mut c_void>,
    /// Return zero on success. Nonzero disables this instance for the run.
    pub process: Option<unsafe extern "C" fn(context: *mut c_void, sample: *mut Sample) -> i32>,
    pub reset: Option<unsafe extern "C" fn(context: *mut c_void)>,
    pub destroy: Option<unsafe extern "C" fn(context: *mut c_void)>,
}

impl Header {
    pub const V1: Self = Self {
        abi_version: ABI_VERSION,
        struct_size: core::mem::size_of::<FilterApi>() as u32,
    };
}

pub const fn plugin_name(name: &str) -> [u8; 64] {
    let bytes = name.as_bytes();
    assert!(bytes.len() < 64);
    let mut result = [0; 64];
    let mut index = 0;
    while index < bytes.len() {
        result[index] = bytes[index];
        index += 1;
    }
    result
}

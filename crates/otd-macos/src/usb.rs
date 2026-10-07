//! Indexed USB strings, using Apple's IOUSBDeviceInterface245 ABI.
//! Descriptor parsing stays portable; device requests run only during setup.

use std::io;

pub fn descriptor_payload(bytes: &[u8]) -> io::Result<&[u8]> {
    let length = bytes.first().copied().unwrap_or(0) as usize;
    if length < 2 || length > bytes.len() || length % 2 != 0 || bytes.get(1) != Some(&3) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid USB string descriptor"));
    }
    Ok(&bytes[2..length])
}

pub fn language(bytes: &[u8]) -> io::Result<u16> {
    let payload = descriptor_payload(bytes)?;
    let mut languages = payload.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
    let first = languages.next().filter(|value| *value != 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "USB device supplies no string languages"))?;
    Ok(if first == 0x0409 || languages.any(|value| value == 0x0409) { 0x0409 } else { first })
}

pub fn text(bytes: &[u8]) -> io::Result<String> {
    let words: Vec<_> = descriptor_payload(bytes)?.chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
    String::from_utf16(&words).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "USB string is not valid UTF-16"))
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use crate::ffi;
    use std::ffi::c_void;
    use std::ptr;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Uuid { bytes: [u8; 16] }

    #[repr(C)]
    struct Unknown {
        reserved: *mut c_void,
        query: unsafe extern "C" fn(*mut c_void, Uuid, *mut *mut c_void) -> i32,
        add_ref: unsafe extern "C" fn(*mut c_void) -> u32,
        release: unsafe extern "C" fn(*mut c_void) -> u32,
    }

    // Field order is IOUSBDeviceStruct182 in Apple's IOUSBLib.h.
    // Uncalled slots retain their pointer size and alignment.
    #[repr(C)]
    struct DeviceInterface {
        unknown: Unknown,
        async_slots: [*const c_void; 4],
        open: unsafe extern "C" fn(*mut c_void) -> i32,
        close: unsafe extern "C" fn(*mut c_void) -> i32,
        device_slots: [*const c_void; 20],
        request: unsafe extern "C" fn(*mut c_void, *mut Request) -> i32,
    }

    #[repr(C)]
    struct Request {
        kind: u8, request: u8, value: u16, index: u16, length: u16,
        data: *mut c_void, transferred: u32, idle_timeout: u32, completion_timeout: u32,
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFUUIDCreateFromUUIDBytes(allocator: ffi::Ref, bytes: Uuid) -> ffi::Ref;
    }
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOCreatePlugInInterfaceForService(service: u32, kind: ffi::Ref, interface: ffi::Ref,
            plugin: *mut *mut *const Unknown, score: *mut i32) -> i32;
        fn IODestroyPlugInInterface(plugin: *mut *const Unknown) -> i32;
    }

    const DEVICE: Uuid = Uuid { bytes: [0x9d,0xc7,0xb7,0x80,0x9e,0xc0,0x11,0xd4,0xa5,0x4f,0x00,0x0a,0x27,0x05,0x28,0x61] };
    const PLUGIN: Uuid = Uuid { bytes: [0xc2,0x44,0xe8,0x58,0x10,0x9c,0x11,0xd4,0x91,0xd4,0x00,0x50,0xe4,0xc6,0x42,0x6f] };
    // 245 retains its IOService; older UUIDs have Apple's documented
    // over-release bug. The prefix through DeviceRequestTO is unchanged.
    const INTERFACE: Uuid = Uuid { bytes: [0xfe,0x2f,0xd5,0x2f,0x3b,0x5a,0x47,0x3b,0x97,0x7b,0xad,0x99,0x00,0x1e,0xb3,0xed] };

    fn error(operation: &str, result: i32) -> io::Error {
        io::Error::other(format!("{operation} failed (IOReturn 0x{:08x}); check USB access and competing drivers", result as u32))
    }

    struct Plugin(*mut *const Unknown);
    impl Drop for Plugin {
        fn drop(&mut self) { unsafe { IODestroyPlugInInterface(self.0) }; }
    }

    pub struct Strings { interface: *mut *const DeviceInterface, language: Option<u16> }
    impl Strings {
        pub fn open(service: u32) -> io::Result<Self> {
            // These are owned Create references, released after plugin creation.
            let device = unsafe { CFUUIDCreateFromUUIDBytes(ptr::null(), DEVICE) };
            let plugin_type = unsafe { CFUUIDCreateFromUUIDBytes(ptr::null(), PLUGIN) };
            if device.is_null() || plugin_type.is_null() {
                if !device.is_null() { unsafe { ffi::CFRelease(device) }; }
                if !plugin_type.is_null() { unsafe { ffi::CFRelease(plugin_type) }; }
                return Err(io::Error::other("cannot create USB interface identifiers"));
            }
            let mut plugin = ptr::null_mut();
            let mut score = 0;
            let result = unsafe { IOCreatePlugInInterfaceForService(service, device, plugin_type, &mut plugin, &mut score) };
            unsafe { ffi::CFRelease(device); ffi::CFRelease(plugin_type); }
            if result != 0 || plugin.is_null() {
                if !plugin.is_null() { drop(Plugin(plugin)); }
                return Err(error("USB plugin creation", result));
            }
            let plugin = Plugin(plugin);
            let mut interface = ptr::null_mut();
            let result = unsafe { ((**plugin.0).query)(plugin.0.cast(), INTERFACE, &mut interface) };
            if result != 0 || interface.is_null() { return Err(error("USB interface query", result)); }
            let interface = interface.cast::<*const DeviceInterface>();
            let result = unsafe { ((**interface).open)(interface.cast()) };
            if result != 0 {
                unsafe { ((**interface).unknown.release)(interface.cast()) };
                return Err(error("USB string access", result));
            }
            Ok(Self { interface, language: None })
        }

        fn descriptor(&self, index: u8, language: u16) -> io::Result<Vec<u8>> {
            let mut bytes = [0u8; 255];
            let mut request = Request { kind: 0x80, request: 6, value: 0x0300 | u16::from(index),
                index: language, length: bytes.len() as u16, data: bytes.as_mut_ptr().cast(),
                transferred: 0, idle_timeout: 1000, completion_timeout: 1000 };
            // The data stays alive for the bounded synchronous GET_DESCRIPTOR.
            let result = unsafe { ((**self.interface).request)(self.interface.cast(), &mut request) };
            if result != 0 { return Err(error(&format!("USB string {index}"), result)); }
            let length = usize::try_from(request.transferred).unwrap_or(usize::MAX);
            let bytes = bytes.get(..length).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "USB descriptor length exceeds request"))?;
            super::descriptor_payload(bytes)?;
            Ok(bytes.to_vec())
        }

        pub fn read(&mut self, index: u8) -> io::Result<String> {
            self.read_checked(index, || Ok(()))
        }

        pub fn read_checked(&mut self, index: u8, mut check: impl FnMut() -> io::Result<()>) -> io::Result<String> {
            check()?;
            if index == 0 { return super::text(&self.descriptor(0, 0)?); }
            let language = match self.language {
                Some(language) => language,
                None => { let language = super::language(&self.descriptor(0, 0)?)?; self.language = Some(language); language }
            };
            check()?;
            super::text(&self.descriptor(index, language)?)
        }
    }
    impl Drop for Strings {
        fn drop(&mut self) {
            unsafe { ((**self.interface).close)(self.interface.cast()); ((**self.interface).unknown.release)(self.interface.cast()); }
        }
    }
}

#[cfg(target_os = "macos")]
pub use native::Strings;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn languages_and_utf16_follow_descriptor_lengths() {
        assert_eq!(language(&[6,3,0x11,0x04,9,4]).unwrap(), 0x0409);
        assert_eq!(language(&[4,3,0x11,0x04]).unwrap(), 0x0411);
        assert_eq!(text(&[6,3,b'A',0,b'B',0,255]).unwrap(), "AB");
        assert_eq!(text(&[6,3,0x3d,0xd8,0,0xde]).unwrap(), "😀");
    }
    #[test]
    fn malformed_descriptors_do_not_produce_matching_strings() {
        for bytes in [&[][..], &[1,3], &[4,3], &[3,3,0], &[4,2,0,0]] {
            assert!(text(bytes).is_err());
        }
        assert!(language(&[2,3]).is_err());
        assert!(language(&[4,3,0,0]).is_err());
        assert!(text(&[4,3,0,0xd8]).is_err());
    }
}

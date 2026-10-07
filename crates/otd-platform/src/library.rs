//! Unix trusted-library loader. No library opens occur on report dispatch.
use std::ffi::{CStr, CString, OsStr, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub type HostChar = u8;
pub fn native_string(value: &OsStr) -> Result<Vec<HostChar>, String> {
    let mut bytes = value.as_bytes().to_vec();
    if bytes.contains(&0) { return Err("library/host string contains a NUL byte".into()); }
    bytes.push(0); Ok(bytes)
}

pub struct Library(*mut c_void);
impl Library {
    /// Trusted code only. RTLD_LOCAL keeps plugin symbols from changing the
    /// resolution of unrelated libraries; RTLD_NOW rejects missing imports
    /// before the input loop. The owned handle outlives every callback pointer.
    pub fn load(path: &Path) -> Result<Self, String> {
        if cfg!(all(target_env = "musl", target_feature = "crt-static")) {
            return Err("Shared plugins and CoreCLR require a dynamically linked Linux runtime; this static musl executable cannot load .so libraries".into());
        }
        let path = path.canonicalize().map_err(|error| format!("{}: {error}", path.display()))?;
        let bytes = CString::new(path.as_os_str().as_bytes()).map_err(|_| "library path contains NUL")?;
        unsafe { libc::dlerror(); }
        let handle = unsafe { libc::dlopen(bytes.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() { return Err(format!("cannot load {}: {}", path.display(), last_error())); }
        Ok(Self(handle))
    }
    pub fn symbol(&self, name: &[u8]) -> Result<*mut c_void, String> {
        let name = CStr::from_bytes_with_nul(name).map_err(|_| "symbol name must contain exactly one terminal NUL")?;
        unsafe { libc::dlerror(); }
        let symbol = unsafe { libc::dlsym(self.0, name.as_ptr()) };
        let error = unsafe { libc::dlerror() };
        if !error.is_null() { return Err(unsafe { CStr::from_ptr(error) }.to_string_lossy().into_owned()); }
        if symbol.is_null() { return Err(format!("{} resolved to a null callback", name.to_string_lossy())); }
        Ok(symbol)
    }
}
fn last_error() -> String {
    let value = unsafe { libc::dlerror() };
    if value.is_null() { "dynamic loader returned no diagnostic".into() }
    else { unsafe { CStr::from_ptr(value) }.to_string_lossy().into_owned() }
}
impl Drop for Library { fn drop(&mut self) { unsafe { libc::dlclose(self.0); } } }

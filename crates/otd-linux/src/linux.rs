//! hidraw discovery and input, USB string descriptors through usbfs, and a
//! uinput pointer for output. Needs read/write access to the tablet's
//! `/dev/hidraw*` node and to `/dev/uinput`; see the udev rule in the README.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use otd_core::endpoint_match::{Endpoint, Transport};
use otd_core::output::{MousePacket, flags};
use otd_core::session::{Read, ReportSource};
use otd_core::tablets::{Database, DeviceIdentifier, TabletConfiguration};

use crate::descriptor;
use crate::sysfs;

/// One hidraw node and what matching needs to know about it.
pub struct Device {
    pub endpoint: Endpoint,
    pub node: PathBuf,
    pub usb: Option<PathBuf>,
    pub uses_report_ids: bool,
}

/// Every hidraw node, by sysfs path. Device strings are read only for the
/// indices a configuration for the same IDs asks for.
pub fn enumerate(database: &Database) -> io::Result<Vec<Device>> {
    let mut devices = Vec::new();
    for entry in fs::read_dir("/sys/class/hidraw")? {
        let entry = entry?;
        let Ok(sys) = fs::canonicalize(entry.path()) else {
            continue;
        };
        let Some(hid) = sys.parent().and_then(Path::parent) else {
            continue;
        };
        let Some(id) = fs::read_to_string(hid.join("uevent"))
            .ok()
            .and_then(|uevent| sysfs::hid_id(&uevent))
        else {
            continue;
        };
        let Some(lengths) = fs::read(hid.join("report_descriptor"))
            .ok()
            .and_then(|descriptor| descriptor::lengths(&descriptor))
        else {
            continue;
        };
        let node = Path::new("/dev").join(entry.file_name());
        let usb = sysfs::usb_device(&sys);
        let mut strings = BTreeMap::new();
        if let Some(usb) = &usb {
            let indices: std::collections::BTreeSet<u8> = database
                .find(id.vendor, id.product)
                .filter_map(|found| found.identifier.device_strings.as_ref())
                .flat_map(|strings| strings.keys())
                .filter_map(|index| index.parse().ok())
                .collect();
            for index in indices {
                if let Ok(text) = usb_string(usb, index) {
                    strings.insert(index, text);
                }
            }
        }
        let mut attributes = BTreeMap::new();
        if let Some(interface) = sysfs::interface_number(&sys) {
            attributes.insert("USB_INTERFACE_NUMBER".to_owned(), interface);
        }
        devices.push(Device {
            endpoint: Endpoint {
                path: sys.to_string_lossy().into_owned(),
                physical_id: usb
                    .as_ref()
                    .map(|usb| usb.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                transport: if id.bus == sysfs::BUS_USB {
                    Transport::UsbHid
                } else {
                    Transport::Other
                },
                vendor_id: id.vendor,
                product_id: id.product,
                can_open: accessible(&node),
                input_length: lengths.input,
                output_length: lengths.output,
                feature_length: lengths.feature,
                strings,
                attributes: Some(attributes),
            },
            node,
            usb,
            uses_report_ids: lengths.uses_report_ids,
        });
    }
    devices.sort_by(|a, b| a.endpoint.path.cmp(&b.endpoint.path));
    Ok(devices)
}

fn accessible(node: &Path) -> bool {
    let Ok(path) = CString::new(node.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `path` is a valid NUL-terminated string.
    unsafe { libc::access(path.as_ptr(), libc::R_OK | libc::W_OK) == 0 }
}

const fn ioc(direction: u32, kind: u8, number: u8, size: usize) -> u32 {
    (direction << 30) | ((size as u32) << 16) | ((kind as u32) << 8) | number as u32
}
const WRITE: u32 = 1;
const READ_WRITE: u32 = 3;

#[repr(C)]
struct ControlTransfer {
    request_type: u8,
    request: u8,
    value: u16,
    index: u16,
    length: u16,
    timeout: u32,
    data: *mut libc::c_void,
}
const USBDEVFS_CONTROL: u32 = ioc(READ_WRITE, b'U', 0, size_of::<ControlTransfer>());

/// Reads a USB string descriptor (US English) through usbfs, which needs
/// access to the device's `/dev/bus/usb` node.
pub fn usb_string(usb: &Path, index: u8) -> io::Result<String> {
    let number = |name: &str| -> io::Result<u32> {
        fs::read_to_string(usb.join(name))?
            .trim()
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("invalid {name}")))
    };
    let node = format!(
        "/dev/bus/usb/{:03}/{:03}",
        number("busnum")?,
        number("devnum")?
    );
    let file = OpenOptions::new().read(true).write(true).open(&node)?;
    let mut buffer = [0u8; 255];
    let mut transfer = ControlTransfer {
        request_type: 0x80,
        request: 6, // GET_DESCRIPTOR
        value: 0x0300 | u16::from(index),
        index: 0x0409,
        length: buffer.len() as u16,
        timeout: 1000,
        data: buffer.as_mut_ptr().cast(),
    };
    // SAFETY: `transfer` points at `buffer`, which outlives the call.
    let read = unsafe { libc::ioctl(file.as_raw_fd(), USBDEVFS_CONTROL as _, &mut transfer) };
    if read < 0 {
        return Err(io::Error::last_os_error());
    }
    sysfs::string_descriptor(&buffer[..read as usize]).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("string {index} is not a string descriptor"),
        )
    })
}

/// Initialization follows pinned InputDevice.Initialize, as on Windows:
/// strings, delayed features, then output writes. A failure aborts the
/// session so partially initialized hardware never drives the pointer.
pub fn initialize(
    device: &Device,
    file: &File,
    identifier: &DeviceIdentifier,
    configuration: &TabletConfiguration,
    stop: &AtomicBool,
) -> io::Result<()> {
    let cancelled = || {
        if stop.load(Ordering::Acquire) {
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
        return Err(io::Error::other(
            "infinite feature initialization delay is unsupported",
        ));
    }
    for &index in identifier
        .initialization_strings
        .as_deref()
        .unwrap_or_default()
    {
        cancelled()?;
        let usb = device
            .usb
            .as_deref()
            .ok_or_else(|| io::Error::other("initialization strings need a USB device"))?;
        usb_string(usb, index).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("cannot read initialization string {index}: {error}"),
            )
        })?;
    }
    for report in identifier
        .feature_init_report
        .iter()
        .flatten()
        .filter(|report| !report.0.is_empty())
    {
        cancelled()?;
        let mut remaining = Duration::from_millis(u64::from(delay));
        while !remaining.is_zero() {
            cancelled()?;
            let step = remaining.min(Duration::from_millis(10));
            std::thread::sleep(step);
            remaining -= step;
        }
        cancelled()?;
        let mut data = padded(&report.0, device.endpoint.feature_length)?;
        let request = ioc(READ_WRITE, b'H', 0x06, data.len()); // HIDIOCSFEATURE
        // SAFETY: `data` is valid for the length encoded in the request.
        if unsafe { libc::ioctl(file.as_raw_fd(), request as _, data.as_mut_ptr()) } < 0 {
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
        // hidraw drops a leading zero report ID itself, as Windows does.
        (&*file).write_all(&padded(&report.0, device.endpoint.output_length)?)?;
    }
    Ok(())
}

fn padded(report: &[u8], length: u32) -> io::Result<Vec<u8>> {
    if length == 0 || report.len() > length as usize || length > u32::from(u16::MAX) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "initialization report exceeds endpoint report length",
        ));
    }
    let mut data = vec![0; length as usize];
    data[..report.len()].copy_from_slice(report);
    Ok(data)
}

/// Reads input reports from a hidraw node. Reads never allocate.
pub struct Hidraw<'a> {
    file: File,
    label: String,
    buffer: Box<[u8]>,
    /// Reads go after a zero report ID byte when the device uses no IDs,
    /// so parsers see the Windows layout.
    offset: usize,
    stop: &'a AtomicBool,
}

impl<'a> Hidraw<'a> {
    pub fn open(device: &Device, label: String, stop: &'a AtomicBool) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&device.node)?;
        Ok(Self {
            file,
            label,
            buffer: vec![0; (device.endpoint.input_length as usize).max(64) + 1].into(),
            offset: usize::from(!device.uses_report_ids),
            stop,
        })
    }

    pub fn file(&self) -> &File {
        &self.file
    }
}

impl ReportSource for Hidraw<'_> {
    fn label(&self) -> &str {
        &self.label
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    fn next(&mut self, timeout: Duration) -> io::Result<Read<'_>> {
        if self.stop.load(Ordering::Acquire) {
            return Ok(Read::Ended);
        }
        let mut poll = libc::pollfd {
            fd: self.file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        let queued = unsafe { libc::poll(&mut poll, 1, 0) } > 0;
        if !queued {
            // Wake at least every 100 ms to notice a stop request.
            // Round up: a sub-millisecond filter timer deadline must not spin.
            let wait = timeout
                .min(Duration::from_millis(100))
                .as_micros()
                .div_ceil(1000) as i32;
            // SAFETY: one valid pollfd.
            match unsafe { libc::poll(&mut poll, 1, wait) } {
                0 => return Ok(Read::Idle),
                n if n < 0 => {
                    let error = io::Error::last_os_error();
                    return if error.kind() == io::ErrorKind::Interrupted {
                        Ok(Read::Idle)
                    } else {
                        Err(error)
                    };
                }
                _ => {}
            }
        }
        if poll.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            return Ok(Read::Ended);
        }
        let target = &mut self.buffer[self.offset..];
        // SAFETY: `target` is valid for writes of its length.
        let read = unsafe { libc::read(poll.fd, target.as_mut_ptr().cast(), target.len()) };
        let ready = Instant::now();
        if read < 0 {
            let error = io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(libc::EAGAIN | libc::EINTR) => Ok(Read::Idle),
                Some(libc::ENODEV) => Ok(Read::Ended),
                _ => Err(error),
            };
        }
        if read == 0 {
            return Ok(Read::Ended);
        }
        if self.offset == 1 {
            self.buffer[0] = 0;
        }
        Ok(Read::Report {
            bytes: &self.buffer[..self.offset + read as usize],
            ready,
            queued,
        })
    }
}

const UI_SET_EVBIT: u32 = ioc(WRITE, b'U', 100, size_of::<libc::c_int>());
const UI_SET_KEYBIT: u32 = ioc(WRITE, b'U', 101, size_of::<libc::c_int>());
const UI_SET_RELBIT: u32 = ioc(WRITE, b'U', 102, size_of::<libc::c_int>());
const UI_SET_ABSBIT: u32 = ioc(WRITE, b'U', 103, size_of::<libc::c_int>());
const UI_DEV_SETUP: u32 = ioc(WRITE, b'U', 3, size_of::<libc::uinput_setup>());
const UI_ABS_SETUP: u32 = ioc(WRITE, b'U', 4, size_of::<libc::uinput_abs_setup>());
const UI_DEV_CREATE: u32 = ioc(0, b'U', 1, 0);
const UI_DEV_DESTROY: u32 = ioc(0, b'U', 2, 0);
const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const EV_ABS: u16 = 3;
const BTN_LEFT: u16 = 0x110;
const BUS_VIRTUAL: u16 = 0x06;

/// A virtual pointer: absolute axes spanning the virtual screen in
/// `SendInput`'s 0..65535 units, or relative axes, plus the left button. An
/// absolute pointer is what QEMU's USB tablet presents; desktops map it over
/// all screens.
pub struct Uinput {
    file: File,
}

impl Uinput {
    pub fn create(relative: bool) -> io::Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open("/dev/uinput")?;
        let fd = file.as_raw_fd();
        let set = |request: u32, value: u16| -> io::Result<()> {
            // SAFETY: these requests take an int argument by value.
            if unsafe { libc::ioctl(fd, request as _, libc::c_int::from(value)) } < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        };
        set(UI_SET_EVBIT, EV_KEY)?;
        set(UI_SET_KEYBIT, BTN_LEFT)?;
        if relative {
            set(UI_SET_EVBIT, EV_REL)?;
            set(UI_SET_RELBIT, 0)?; // REL_X
            set(UI_SET_RELBIT, 1)?; // REL_Y
        } else {
            set(UI_SET_EVBIT, EV_ABS)?;
            for code in [0, 1] {
                // ABS_X, ABS_Y
                set(UI_SET_ABSBIT, code)?;
                // SAFETY: plain-data struct; zero is a valid value.
                let mut axis: libc::uinput_abs_setup = unsafe { std::mem::zeroed() };
                axis.code = code;
                axis.absinfo.maximum = 65_535;
                // SAFETY: `axis` is a valid uinput_abs_setup.
                if unsafe { libc::ioctl(fd, UI_ABS_SETUP as _, &axis) } < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
        }
        // SAFETY: plain-data struct; zero is a valid value.
        let mut setup: libc::uinput_setup = unsafe { std::mem::zeroed() };
        setup.id.bustype = BUS_VIRTUAL;
        setup.id.version = 1;
        for (slot, byte) in setup.name.iter_mut().zip(b"OpenTabletDriver Rust pointer") {
            *slot = *byte as libc::c_char;
        }
        // SAFETY: `setup` is a valid uinput_setup.
        if unsafe { libc::ioctl(fd, UI_DEV_SETUP as _, &setup) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: no argument.
        if unsafe { libc::ioctl(fd, UI_DEV_CREATE as _) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { file })
    }

    /// Sends one packet as a single evdev frame, without allocating.
    pub fn send(&self, packet: MousePacket) -> io::Result<()> {
        // SAFETY: plain-data struct; zero is a valid value.
        let blank: libc::input_event = unsafe { std::mem::zeroed() };
        let mut events = [blank; 4];
        let mut count = 0;
        let mut push = |kind: u16, code: u16, value: i32| {
            events[count].type_ = kind;
            events[count].code = code;
            events[count].value = value;
            count += 1;
        };
        if packet.flags & flags::MOVE != 0 {
            let kind = if packet.flags & flags::ABSOLUTE != 0 {
                EV_ABS
            } else {
                EV_REL
            };
            push(kind, 0, packet.dx);
            push(kind, 1, packet.dy);
        }
        if packet.flags & flags::LEFTDOWN != 0 {
            push(EV_KEY, BTN_LEFT, 1);
        } else if packet.flags & flags::LEFTUP != 0 {
            push(EV_KEY, BTN_LEFT, 0);
        }
        push(EV_SYN, 0, 0);
        // SAFETY: `events[..count]` is initialized plain data.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                events.as_ptr().cast::<u8>(),
                count * size_of::<libc::input_event>(),
            )
        };
        (&self.file).write_all(bytes)
    }
}

impl Drop for Uinput {
    fn drop(&mut self) {
        // SAFETY: no argument; the device was created by this handle.
        unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY as _) };
    }
}

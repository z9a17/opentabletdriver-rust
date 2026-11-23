//! Optional Windows scheduling settings, kept away from tablet profiles and
//! report callbacks. Empty CPU lists retain the Windows scheduler defaults.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use windows_sys::Win32::System::Threading::{
    GetActiveProcessorGroupCount, GetCurrentProcess, GetCurrentThread, GetProcessAffinityMask,
    SetProcessAffinityMask, SetThreadAffinityMask,
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub ui_cpus: Vec<u16>,
    pub driver_cpus: Vec<u16>,
    pub mmcss: bool,
}

// New sessions read this once; applying settings never interrupts input.
static DRIVER_MMCSS: AtomicBool = AtomicBool::new(false);
// Target logical-CPU mask for the report-reader thread(s) only (0 = All CPUs).
// Kept separate from the daemon's process affinity mask so background threads
// and the .NET GC are never forced onto the reader's isolated core(s).
static DRIVER_AFFINITY_MASK: AtomicUsize = AtomicUsize::new(0);
static DRIVER_AFFINITY_EPOCH: AtomicU64 = AtomicU64::new(0);

pub fn mmcss_enabled() -> bool {
    DRIVER_MMCSS.load(Ordering::Relaxed)
}

#[cfg(test)]
pub fn driver_affinity_mask() -> usize {
    DRIVER_AFFINITY_MASK.load(Ordering::Relaxed)
}

/// Applies the configured `driver_cpus` affinity mask to the calling report
/// thread only, restoring the previous thread affinity on drop.
pub struct ThreadAffinity {
    previous: Option<usize>,
    epoch: u64,
}

impl ThreadAffinity {
    pub fn apply_driver_thread() -> Self {
        let mut this = Self {
            previous: None,
            epoch: u64::MAX,
        };
        this.refresh();
        this
    }

    /// Re-applies the thread affinity mask if `experimental.toml` changed while
    /// this session was running. Fast path is a single relaxed atomic load.
    #[inline]
    pub fn refresh(&mut self) {
        let epoch = DRIVER_AFFINITY_EPOCH.load(Ordering::Acquire);
        if epoch == self.epoch {
            return;
        }
        self.epoch = epoch;
        if unsafe { GetActiveProcessorGroupCount() } > 1 {
            return;
        }
        let target = DRIVER_AFFINITY_MASK.load(Ordering::Relaxed);
        let mask = if target != 0 {
            target
        } else if let Some(previous) = self.previous.take() {
            unsafe {
                let _ = SetThreadAffinityMask(GetCurrentThread(), previous);
            }
            return;
        } else {
            return;
        };
        let prev = unsafe { SetThreadAffinityMask(GetCurrentThread(), mask) };
        if prev != 0 && self.previous.is_none() {
            self.previous = Some(prev);
        }
    }
}

impl Drop for ThreadAffinity {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            unsafe {
                let _ = SetThreadAffinityMask(GetCurrentThread(), previous);
            }
        }
    }
}

pub fn path() -> Result<PathBuf, String> {
    Ok(otd_core::storage::data_directory()?.join("experimental.toml"))
}

pub fn load_at(path: &Path) -> Result<Settings, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

pub fn load() -> Result<Settings, String> {
    load_at(&path()?)
}

/// Parse a logical-CPU list without accepting duplicates, reversed ranges,
/// empty items or numbers that cannot be represented by a Windows x64 mask.
pub fn parse_cpus(text: &str) -> Result<Vec<u16>, String> {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("all") {
        return Ok(Vec::new());
    }
    let number = |value: &str| -> Result<u16, String> {
        let value = value.trim();
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("Use logical CPU numbers, for example 0,2,4-7, or All.".into());
        }
        value
            .parse::<u16>()
            .ok()
            .filter(|cpu| *cpu < 64)
            .ok_or_else(|| "Logical CPU numbers must be between 0 and 63.".into())
    };
    let mut cpus = Vec::new();
    for item in text.split(',') {
        let (first, last) = match item.split_once('-') {
            Some((first, last)) => (number(first)?, number(last)?),
            None => {
                let cpu = number(item)?;
                (cpu, cpu)
            }
        };
        if first > last {
            return Err("CPU ranges must run from a smaller number to a larger number.".into());
        }
        for cpu in first..=last {
            if cpus.contains(&cpu) {
                return Err(format!("Logical CPU {cpu} was selected more than once."));
            }
            cpus.push(cpu);
        }
    }
    cpus.sort_unstable();
    Ok(cpus)
}

pub fn format_cpus(cpus: &[u16]) -> String {
    if cpus.is_empty() {
        "All".into()
    } else {
        cpus.iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
}

pub fn masks() -> Result<(usize, usize), String> {
    let (mut process, mut system) = (0, 0);
    if unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) } == 0 {
        return Err(format!(
            "Cannot read CPU affinity: {}",
            std::io::Error::last_os_error()
        ));
    }
    if process == 0 || system == 0 {
        return Err("CPU affinity is unavailable for a process spanning processor groups.".into());
    }
    Ok((process, system))
}

fn selected_mask(cpus: &[u16], available: usize) -> Result<usize, String> {
    if cpus.is_empty() {
        return Ok(available);
    }
    let mut mask = 0;
    for &cpu in cpus {
        let bit = 1usize
            .checked_shl(u32::from(cpu))
            .filter(|bit| available & bit != 0)
            .ok_or_else(|| format!("Logical CPU {cpu} is unavailable on this computer."))?;
        if mask & bit != 0 {
            return Err(format!("Logical CPU {cpu} was selected more than once."));
        }
        mask |= bit;
    }
    Ok(mask)
}

pub fn validate(settings: &Settings) -> Result<(), String> {
    // Do not mistake a group-local mask for the CPU numbers of a large system.
    if unsafe { GetActiveProcessorGroupCount() } > 1 {
        if !settings.ui_cpus.is_empty() || !settings.driver_cpus.is_empty() {
            return Err("CPU pinning currently supports one Windows processor group, up to 64 logical CPUs. Keep both fields set to All on this computer.".into());
        }
        return Ok(());
    }
    let (_, system) = masks()?;
    selected_mask(&settings.ui_cpus, system)?;
    selected_mask(&settings.driver_cpus, system)?;
    Ok(())
}

pub fn set_mask(mask: usize) -> Result<(), String> {
    if unsafe { SetProcessAffinityMask(GetCurrentProcess(), mask) } == 0 {
        return Err(format!(
            "Cannot apply CPU affinity: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

pub fn apply(cpus: &[u16]) -> Result<(), String> {
    if unsafe { GetActiveProcessorGroupCount() } > 1 {
        return if cpus.is_empty() {
            Ok(())
        } else {
            Err("CPU pinning requires a single Windows processor group.".into())
        };
    }
    let (_, system) = masks()?;
    set_mask(selected_mask(cpus, system)?)
}

fn resolve_driver_mask(cpus: &[u16]) -> Result<usize, String> {
    if cpus.is_empty() {
        return Ok(0);
    }
    if unsafe { GetActiveProcessorGroupCount() } > 1 {
        return Err("CPU pinning requires a single Windows processor group.".into());
    }
    let (_, system) = masks()?;
    selected_mask(cpus, system)
}

fn publish_driver_mask(mask: usize) {
    DRIVER_AFFINITY_MASK.store(mask, Ordering::Relaxed);
    DRIVER_AFFINITY_EPOCH.fetch_add(1, Ordering::Release);
}

/// Persist first, then publish the reader-thread CPU mask and MMCSS flag.
/// A failed save leaves the running driver's affinity untouched.
pub fn save_driver_at(path: &Path, settings: &Settings) -> Result<(), String> {
    validate(settings)?;
    let next_mask = resolve_driver_mask(&settings.driver_cpus)?;
    let snapshot = otd_core::storage::capture(path)?;
    let text = toml::to_string_pretty(settings).map_err(|error| error.to_string())?;
    otd_core::storage::save(
        path,
        text.as_bytes(),
        otd_core::storage::SaveMode::Replace(&snapshot),
    )?;
    publish_driver_mask(next_mask);
    DRIVER_MMCSS.store(settings.mmcss, Ordering::Relaxed);
    Ok(())
}

pub fn apply_saved(ui: bool) -> Result<(), String> {
    // A child daemon inherits its launching GUI's mask. Reset that inheritance
    // before reading its independent choice, including when the file is bad.
    if !ui {
        apply(&[])?;
    }
    let settings = load()?;
    if !ui {
        DRIVER_MMCSS.store(settings.mmcss, Ordering::Relaxed);
        let mask = resolve_driver_mask(&settings.driver_cpus)?;
        publish_driver_mask(mask);
        return Ok(());
    }
    // Default GUI settings retain external launcher affinity. An explicit All
    // in the dialog resets a prior app selection.
    if settings.ui_cpus.is_empty() {
        Ok(())
    } else {
        apply(&settings.ui_cpus)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_lists_accept_all_and_ranges_and_reject_invalid_selections() {
        assert_eq!(parse_cpus(" ALL ").unwrap(), Vec::<u16>::new());
        assert_eq!(parse_cpus("").unwrap(), Vec::<u16>::new());
        assert_eq!(parse_cpus("7, 0,2-4").unwrap(), [0, 2, 3, 4, 7]);
        for invalid in ["0,0", "2-4,3", "4-2", "64", "-1", "0,", "1--3", "+1", "a"] {
            assert!(parse_cpus(invalid).is_err(), "{invalid}");
        }
        assert_eq!(selected_mask(&[0, 2], 0b111).unwrap(), 0b101);
        assert_eq!(selected_mask(&[], 0b111).unwrap(), 0b111);
        assert!(selected_mask(&[3], 0b111).is_err());
        assert!(selected_mask(&[64], usize::MAX).is_err());
        assert!(selected_mask(&[1, 1], 0b111).is_err());
    }

    #[test]
    fn settings_round_trip_and_reject_unknown_keys() {
        let settings = Settings {
            ui_cpus: vec![0, 2],
            driver_cpus: vec![1, 3],
            mmcss: true,
        };
        assert_eq!(
            toml::from_str::<Settings>(&toml::to_string(&settings).unwrap()).unwrap(),
            settings
        );
        assert_eq!(toml::from_str::<Settings>("").unwrap(), Settings::default());
        let legacy = toml::from_str::<Settings>("ui_cpus = [0]\ndriver_cpus = [1]").unwrap();
        assert_eq!(
            legacy,
            Settings {
                ui_cpus: vec![0],
                driver_cpus: vec![1],
                mmcss: false
            }
        );
        assert!(toml::from_str::<Settings>("driver_core = 1").is_err());
    }

    // Run in an isolated test executable after publication. Only that process
    // changes affinity; no daemon endpoint, device, output or GUI is started.
    #[test]
    #[ignore = "changes the isolated test process CPU affinity"]
    fn applies_persists_resets_and_rolls_back_on_save_failure() {
        let original = masks().unwrap().0;
        struct Restore(usize);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = set_mask(self.0);
            }
        }
        let _restore = Restore(original);
        let system = masks().unwrap().1;
        let cpu = system.trailing_zeros() as u16;
        let directory = std::env::temp_dir().join(format!("otd-affinity-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("experimental.toml");
        let settings = Settings {
            ui_cpus: vec![cpu],
            driver_cpus: vec![cpu],
            mmcss: true,
        };
        save_driver_at(&path, &settings).unwrap();
        assert_eq!(
            masks().unwrap().0,
            original,
            "driver CPU pinning must not restrict the daemon process mask"
        );
        assert_eq!(driver_affinity_mask(), 1usize << cpu);
        assert_eq!(load_at(&path).unwrap(), settings);
        assert!(mmcss_enabled());
        std::thread::spawn(move || {
            let guard = ThreadAffinity::apply_driver_thread();
            assert!(guard.previous.is_some());
            drop(guard);
        })
        .join()
        .unwrap();
        let invalid = Settings {
            ui_cpus: vec![64],
            driver_cpus: Vec::new(),
            mmcss: false,
        };
        assert!(save_driver_at(&path, &invalid).is_err());
        assert_eq!(driver_affinity_mask(), 1usize << cpu);
        assert_eq!(load_at(&path).unwrap(), settings);
        assert!(
            mmcss_enabled(),
            "a rejected save must retain the prior scheduling choice"
        );
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        assert!(save_driver_at(&path, &settings).is_err());
        assert_eq!(driver_affinity_mask(), 1usize << cpu);
        assert_eq!(load_at(&path).unwrap(), settings);
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&path, permissions).unwrap();
        save_driver_at(&path, &Settings::default()).unwrap();
        assert_eq!(driver_affinity_mask(), 0);
        assert_eq!(load_at(&path).unwrap(), Settings::default());
        assert!(!mmcss_enabled());
        std::fs::remove_dir_all(directory).unwrap();
    }
}

//! Same-volume staging and crash recovery for one installation. The OS owns
//! the lock lifetime, so a process crash cannot leave a stale lock owner.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject},
};

const JOURNAL: &str = ".otd-update";

pub(crate) struct InstallLock {
    #[cfg(windows)]
    handle: crate::hid::OwnedHandle,
    #[cfg(unix)]
    handle:File,
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl InstallLock {
    pub fn acquire(root: &Path, name: &str) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(|error| error.to_string())?;
        Self::try_acquire(root, name)?.ok_or_else(|| "another process is updating or recovering this installation".into())
    }

    #[cfg(windows)]
    pub fn try_acquire(root: &Path, name: &str) -> Result<Option<Self>, String> {
        let canonical = root.canonicalize().map_err(|error| error.to_string())?;
        let identity = format!("{}:{name}", canonical.to_string_lossy().to_lowercase());
        let hash = identity.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
        let name: Vec<u16> = format!("Local\\OpenTabletDriverRustInstall-{hash:016x}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let handle = crate::hid::OwnedHandle::new(unsafe {
            CreateMutexW(std::ptr::null(), 0, name.as_ptr())
        })
        .map_err(|error| error.to_string())?;
        match unsafe { WaitForSingleObject(handle.raw(), 0) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Some(Self {
                handle,
                _thread_bound: std::marker::PhantomData,
            })),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(format!("cannot wait for installation lock: {}", std::io::Error::last_os_error())),
        }
    }
}
#[cfg(unix)]
impl InstallLock {
    pub fn try_acquire(root:&Path,name:&str)->Result<Option<Self>,String>{
        use std::os::{fd::AsRawFd,unix::fs::{OpenOptionsExt,MetadataExt}};
        if name.is_empty()||name.contains(['/', '\\'])||name=="."||name==".."{return Err("Invalid transaction lock name".into());}
        let root=root.canonicalize().map_err(|error|error.to_string())?;
        let handle=OpenOptions::new().create(true).read(true).write(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC)
            .open(root.join(name)).map_err(|error|error.to_string())?;
        let metadata=handle.metadata().map_err(|error|error.to_string())?;
        if !metadata.is_file()||metadata.uid()!=unsafe{libc::geteuid()}{return Err("Installation lock must be owned by current user".into());}
        if unsafe{libc::flock(handle.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)}!=0{
            let error=std::io::Error::last_os_error();return if error.kind()==std::io::ErrorKind::WouldBlock{Ok(None)}else{Err(error.to_string())};
        }Ok(Some(Self{handle,_thread_bound:std::marker::PhantomData}))
    }
}
#[cfg(windows)]
impl Drop for InstallLock {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.handle.raw());
        }
    }
}

#[cfg(unix)]
impl Drop for InstallLock {fn drop(&mut self){use std::os::fd::AsRawFd;let _=unsafe{libc::flock(self.handle.as_raw_fd(),libc::LOCK_UN)};}}

#[derive(Serialize, Deserialize)]
struct Entry {
    path: PathBuf,
    existed: bool,
}
#[derive(Serialize, Deserialize)]
struct Plan {
    version: u32,
    entries: Vec<Entry>,
}

fn valid(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && !path.components().next().is_some_and(|part| {
            part.as_os_str()
                .to_string_lossy()
                .to_lowercase()
                .starts_with(".otd-")
        })
}
fn sync_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = File::create(path).map_err(|error| error.to_string())?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())
}

/// Call only with the install lock. A committed generation may keep loaded
/// old binaries until a later startup can delete them; it is already usable.
pub(super) fn recover(install: &Path) -> Result<bool, String> {
    let journal = install.join(JOURNAL);
    if !journal.exists() {
        return Ok(false);
    }
    if journal.join("committed").exists() || journal.join("rolled-back").exists() {
        let _ = clean_terminal(&journal);
        return Ok(false);
    }
    let plan_path = journal.join("plan.json");
    if !plan_path.exists() {
        // Staging cannot mutate installed files until plan.json is published.
        return fs::remove_dir_all(&journal)
            .map(|()| false)
            .map_err(|error| error.to_string());
    }
    let plan: Plan = serde_json::from_slice(
        &fs::read(&plan_path).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("update recovery plan is invalid; backups preserved: {error}"))?;
    if plan.version != 1 || plan.entries.iter().any(|entry| !valid(&entry.path)) {
        return Err("unsupported or invalid update recovery plan; backups preserved".into());
    }
    let mut restored = false;
    for entry in plan.entries.iter().rev() {
        let target = install.join(&entry.path);
        let backup = journal.join("backup").join(&entry.path);
        if entry.existed && backup.exists() {
            if target.exists() {
                discard(&journal, &entry.path, &target)?;
            }
            fs::rename(&backup, &target)
                .map_err(|error| format!("cannot restore {}: {error}", target.display()))?;
            restored = true;
        } else if !entry.existed && target.exists() {
            discard(&journal, &entry.path, &target)?;
            restored = true;
        }
    }
    sync_write(&journal.join("rolled-back"), b"1\n")?;
    let _ = clean_terminal(&journal);
    Ok(restored)
}

/// Renaming is allowed for a mapped Windows executable; deletion is not.
fn discard(journal: &Path, relative: &Path, target: &Path) -> Result<(), String> {
    let discarded = journal.join("discarded").join(relative);
    if let Some(parent) = discarded.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::rename(target, discarded).map_err(|error| {
        format!(
            "cannot set aside {} during recovery: {error}",
            target.display()
        )
    })
}

/// Keep the terminal marker and plan until all potentially locked payloads
/// have gone. Partial recursive cleanup must never turn a commit into rollback.
fn clean_terminal(journal: &Path) -> Result<(), String> {
    for name in ["staged", "backup", "discarded"] {
        let path = journal.join(name);
        if path.exists() {
            fs::remove_dir_all(path).map_err(|error| error.to_string())?;
        }
    }
    for name in ["plan.next", "plan.json", "committed", "rolled-back"] {
        let path = journal.join(name);
        if path.exists() {
            fs::remove_file(path).map_err(|error| error.to_string())?;
        }
    }
    fs::remove_dir(journal).map_err(|error| error.to_string())
}

fn prepare(source: &Path, install: &Path, relative: &[PathBuf]) -> Result<Plan, String> {
    let journal = install.join(JOURNAL);
    if journal.exists() {
        return Err("a completed update still has loaded backups; restart the running driver and panel before updating again".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for path in relative {
        if !valid(path) || !seen.insert(path.to_string_lossy().to_lowercase()) {
            return Err("invalid or duplicate package path".into());
        }
    }
    fs::create_dir(&journal).map_err(|error| error.to_string())?;
    let mut entries = Vec::new();
    for path in relative {
        let staged = journal.join("staged").join(path);
        if let Some(parent) = staged.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::copy(source.join(path), &staged)
            .map_err(|error| format!("cannot stage {}: {error}", path.display()))?;
        OpenOptions::new()
            .write(true)
            .open(&staged)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        entries.push(Entry {
            path: path.clone(),
            existed: install.join(path).exists(),
        });
    }
    let plan = Plan {
        version: 1,
        entries,
    };
    let bytes = serde_json::to_vec(&plan).map_err(|error| error.to_string())?;
    sync_write(&journal.join("plan.next"), &bytes)?;
    fs::rename(journal.join("plan.next"), journal.join("plan.json"))
        .map_err(|error| error.to_string())?;
    Ok(plan)
}

fn apply(install: &Path, entry: &Entry) -> Result<(), String> {
    let journal = install.join(JOURNAL);
    let target = install.join(&entry.path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    if entry.existed {
        let backup = journal.join("backup").join(&entry.path);
        if let Some(parent) = backup.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::rename(&target, backup)
            .map_err(|error| format!("cannot back up {}: {error}", target.display()))?;
    }
    fs::rename(journal.join("staged").join(&entry.path), &target)
        .map_err(|error| format!("cannot install {}: {error}", target.display()))
}

pub(super) fn replace(source: &Path, install: &Path, relative: &[PathBuf]) -> Result<(), String> {
    let _lock = InstallLock::acquire(install, ".otd-update.lock")?;
    if recover(install)? {
        return Err("an interrupted update was restored; restart the driver and panel before updating again".into());
    }
    let result = (|| {
        let plan = prepare(source, install, relative)?;
        for entry in &plan.entries {
            apply(install, entry)?;
        }
        sync_write(&install.join(JOURNAL).join("committed"), b"1\n")
    })();
    if let Err(error) = result {
        return match recover(install) {
            Ok(_) => Err(error),
            Err(recovery) => Err(format!(
                "{error}; recovery incomplete: {recovery}. Backups were preserved."
            )),
        };
    }
    Ok(())
}

pub(super) fn startup(install: &Path) -> Result<bool, String> {
    let _lock = InstallLock::acquire(install, ".otd-update.lock")?;
    recover(install)
}

#[cfg(all(test,windows))]
mod tests {
    use super::*;

    struct MappedImage(windows_sys::Win32::Foundation::HMODULE);
    impl MappedImage {
        fn open(path: &Path) -> Self {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::System::LibraryLoader::{
                LOAD_LIBRARY_AS_IMAGE_RESOURCE, LoadLibraryExW,
            };
            let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            // Map the test executable as image data: no entry point, imports,
            // driver code, or UI runs. Windows still locks the image file.
            let module = unsafe {
                LoadLibraryExW(
                    name.as_ptr(),
                    std::ptr::null_mut(),
                    LOAD_LIBRARY_AS_IMAGE_RESOURCE,
                )
            };
            assert!(!module.is_null(), "{}", std::io::Error::last_os_error());
            Self(module)
        }
    }
    impl Drop for MappedImage {
        fn drop(&mut self) {
            unsafe {
                windows_sys::Win32::Foundation::FreeLibrary(self.0);
            }
        }
    }

    #[test]
    fn recovery_renames_mapped_images_and_keeps_terminal_markers() {
        let root = crate::update::temporary_work("otd-mapped-recovery").unwrap();
        let source = root.join("source");
        let install = root.join("install");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&install).unwrap();
        let executable = std::env::current_exe().unwrap();
        fs::copy(&executable, source.join("driver.exe")).unwrap();
        fs::copy(&executable, install.join("driver.exe")).unwrap();
        fs::write(source.join("version.txt"), b"new").unwrap();
        fs::write(install.join("version.txt"), b"old").unwrap();
        let paths = vec![PathBuf::from("driver.exe"), PathBuf::from("version.txt")];
        let lock = InstallLock::acquire(&install, ".otd-update.lock").unwrap();
        let plan = prepare(&source, &install, &paths).unwrap();
        for entry in &plan.entries {
            apply(&install, entry).unwrap();
        }
        let image = MappedImage::open(&install.join("driver.exe"));
        assert!(
            fs::remove_file(install.join("driver.exe")).is_err(),
            "mapped image must exercise Windows delete protection"
        );
        drop(lock);
        assert!(
            startup(&install).unwrap(),
            "rollback requires a fresh process"
        );
        assert_eq!(fs::read(install.join("version.txt")).unwrap(), b"old");
        assert!(install.join(JOURNAL).join("rolled-back").exists());
        assert!(!startup(&install).unwrap());
        drop(image);
        startup(&install).unwrap();
        assert!(!install.join(JOURNAL).exists());

        // A completed update whose old executable remains mapped must stay
        // committed across repeated, partially successful cleanup attempts.
        let old = MappedImage::open(&install.join("driver.exe"));
        replace(&source, &install, &paths).unwrap();
        for _ in 0..2 {
            startup(&install).unwrap();
            assert!(install.join(JOURNAL).join("committed").exists());
            assert_eq!(fs::read(install.join("version.txt")).unwrap(), b"new");
        }
        drop(old);
        startup(&install).unwrap();
        assert!(!install.join(JOURNAL).exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn interrupted_replacements_restore_old_files_and_remove_new_ones() {
        let root = std::env::temp_dir().join(format!("otd-recovery-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let source = root.join("source");
        let install = root.join("install");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&install).unwrap();
        fs::write(source.join("old.exe"), b"new exe").unwrap();
        fs::write(source.join("new.dll"), b"new dll").unwrap();
        let paths = vec![PathBuf::from("old.exe"), PathBuf::from("new.dll")];
        for applied in 0..=2 {
            fs::write(install.join("old.exe"), b"old exe").unwrap();
            let lock = InstallLock::acquire(&install, ".otd-update.lock").unwrap();
            let other = install.clone();
            assert!(
                std::thread::spawn(move || startup(&other))
                    .join()
                    .unwrap()
                    .is_err(),
                "startup must not clean an active transaction"
            );
            let plan = prepare(&source, &install, &paths).unwrap();
            for entry in plan.entries.iter().take(applied) {
                apply(&install, entry).unwrap();
            }
            drop(lock); // Simulate termination before the commit marker.
            startup(&install).unwrap();
            assert_eq!(fs::read(install.join("old.exe")).unwrap(), b"old exe");
            assert!(!install.join("new.dll").exists());
            startup(&install).unwrap();
        }
        replace(&source, &install, &paths).unwrap();
        startup(&install).unwrap();
        assert_eq!(fs::read(install.join("old.exe")).unwrap(), b"new exe");
        assert_eq!(fs::read(install.join("new.dll")).unwrap(), b"new dll");
        fs::remove_dir_all(root).unwrap();
    }
}

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
    handle: File,
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl InstallLock {
    pub fn acquire(root: &Path, name: &str) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(|error| error.to_string())?;
        Self::try_acquire(root, name)?
            .ok_or_else(|| "another process is updating or recovering this installation".into())
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
            _ => Err(format!(
                "cannot wait for installation lock: {}",
                std::io::Error::last_os_error()
            )),
        }
    }
}
#[cfg(unix)]
impl InstallLock {
    pub fn try_acquire(root: &Path, name: &str) -> Result<Option<Self>, String> {
        use std::os::{
            fd::AsRawFd,
            unix::fs::{MetadataExt, OpenOptionsExt},
        };
        if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
            return Err("Invalid transaction lock name".into());
        }
        let root = root.canonicalize().map_err(|error| error.to_string())?;
        let handle = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(root.join(name))
            .map_err(|error| error.to_string())?;
        let metadata = handle.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err("Installation lock must be owned by current user".into());
        }
        if unsafe { libc::flock(handle.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(error.to_string())
            };
        }
        Ok(Some(Self {
            handle,
            _thread_bound: std::marker::PhantomData,
        }))
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
impl Drop for InstallLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        let _ = unsafe { libc::flock(self.handle.as_raw_fd(), libc::LOCK_UN) };
    }
}

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
        && path.components().all(|part| match part {
            Component::Normal(name) => {
                // Win32 strips trailing dots/spaces and interprets colons
                // as streams. Such aliases cannot own independent files.
                #[cfg(windows)]
                {
                    let name = name.to_string_lossy();
                    !name.ends_with(['.', ' ']) && !name.contains(':')
                }
                #[cfg(not(windows))]
                {
                    let _ = name;
                    true
                }
            }
            _ => false,
        })
        && !path.components().next().is_some_and(|part| {
            part.as_os_str()
                .to_string_lossy()
                .to_lowercase()
                .starts_with(".otd-")
        })
}
fn valid_paths<'a>(paths: impl IntoIterator<Item = &'a Path>) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    for path in paths {
        if !valid(path) {
            return false;
        }
        // Normalize separators as well as case: Windows treats a/b and a\b
        // as the same file. A file entry may not also own another entry's parent.
        let key = path
            .components()
            .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
            .collect::<Vec<_>>()
            .join("/");
        if !seen.insert(key) {
            return false;
        }
    }
    seen.iter().all(|key| {
        key.match_indices('/')
            .all(|(index, _)| !seen.contains(&key[..index]))
    })
}
/// Inspect relative parents without following links. The installation root may
/// itself be a junction; only entries below the caller-selected root are owned.
/// This preflight does not arbitrate concurrent changes by unrelated processes.
fn relative_metadata(root: &Path, relative: &Path) -> Result<Option<fs::Metadata>, String> {
    let mut current = root.to_path_buf();
    let mut parts = relative.components().peekable();
    while let Some(part) = parts.next() {
        if !matches!(part, Component::Normal(_)) {
            return Err("invalid relative update entry".into());
        }
        current.push(part);
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("cannot inspect {}: {error}", current.display())),
        };
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes()
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                != 0
        };
        #[cfg(not(windows))]
        let reparse = false;
        if metadata.file_type().is_symlink() || reparse {
            return Err(format!(
                "update entry is a link or reparse point; files preserved: {}",
                current.display()
            ));
        }
        if parts.peek().is_none() {
            return Ok(Some(metadata));
        }
        if !metadata.is_dir() {
            return Err(format!(
                "update parent is not a directory: {}",
                current.display()
            ));
        }
    }
    Err("empty relative update entry".into())
}
fn regular_file_or_absent(root: &Path, relative: &Path) -> Result<bool, String> {
    match relative_metadata(root, relative)? {
        Some(metadata) if !metadata.is_file() => Err(format!(
            "cannot replace non-file installation entry {}",
            root.join(relative).display()
        )),
        Some(_) => Ok(true),
        None => Ok(false),
    }
}
fn directory_or_absent(root: &Path, relative: &Path) -> Result<bool, String> {
    match relative_metadata(root, relative)? {
        Some(metadata) if !metadata.is_dir() => Err(format!(
            "update journal entry is not a directory: {}",
            root.join(relative).display()
        )),
        Some(_) => Ok(true),
        None => Ok(false),
    }
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
    if !directory_or_absent(install, Path::new(JOURNAL))? {
        return Ok(false);
    }
    for name in ["staged", "backup", "discarded"] {
        directory_or_absent(install, &Path::new(JOURNAL).join(name))?;
    }
    for name in ["plan.json", "plan.next", "committed", "rolled-back"] {
        regular_file_or_absent(install, &Path::new(JOURNAL).join(name))?;
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
    if plan.version != 1 || !valid_paths(plan.entries.iter().map(|entry| entry.path.as_path())) {
        return Err("unsupported or invalid update recovery plan; backups preserved".into());
    }
    // Validate the entire rollback before moving any installed file. A local
    // directory collision must not become discarded data recursively removed
    // with the terminal journal, nor may a linked backup parent escape it.
    for entry in &plan.entries {
        regular_file_or_absent(install, &entry.path)?;
        regular_file_or_absent(
            install,
            &Path::new(JOURNAL).join("backup").join(&entry.path),
        )?;
        regular_file_or_absent(
            install,
            &Path::new(JOURNAL).join("discarded").join(&entry.path),
        )?;
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
    if directory_or_absent(install, Path::new(JOURNAL))? {
        return Err("a completed update still has loaded backups; restart the running driver and panel before updating again".into());
    }
    if !valid_paths(relative.iter().map(PathBuf::as_path)) {
        return Err("invalid, duplicate or overlapping package path".into());
    }
    for path in relative {
        regular_file_or_absent(install, path)?;
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

#[cfg(all(test, windows))]
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
    fn recovery_retries_interrupted_backup_and_discard_steps() {
        let root = crate::update::temporary_work("otd-recovery-steps").unwrap();
        let source = root.join("source");
        let install = root.join("install");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&install).unwrap();
        fs::write(source.join("driver.exe"), b"new").unwrap();
        let paths = [PathBuf::from("driver.exe")];
        for phase in 0..3 {
            fs::write(install.join("driver.exe"), b"old").unwrap();
            let lock = InstallLock::acquire(&install, ".otd-update.lock").unwrap();
            let plan = prepare(&source, &install, &paths).unwrap();
            let journal = install.join(JOURNAL);
            if phase == 0 {
                // Crash after moving the old file but before publishing the new one.
                fs::create_dir_all(journal.join("backup")).unwrap();
                fs::rename(
                    install.join("driver.exe"),
                    journal.join("backup/driver.exe"),
                )
                .unwrap();
            } else {
                apply(&install, &plan.entries[0]).unwrap();
                // Crash after setting the new file aside during rollback.
                discard(&journal, &paths[0], &install.join(&paths[0])).unwrap();
                if phase == 2 {
                    // A locked backup makes rollback fail. Its journal must survive
                    // for the next attempt rather than losing the only old copy.
                    use std::os::windows::fs::OpenOptionsExt;
                    let locked = OpenOptions::new()
                        .read(true)
                        .share_mode(1)
                        .open(journal.join("backup/driver.exe"))
                        .unwrap();
                    assert!(recover(&install).is_err());
                    assert!(journal.join("plan.json").exists());
                    assert!(!journal.join("rolled-back").exists());
                    drop(locked);
                }
            }
            drop(lock);
            assert!(startup(&install).unwrap());
            assert_eq!(fs::read(install.join("driver.exe")).unwrap(), b"old");
            assert!(!startup(&install).unwrap());
            assert!(!journal.exists());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_recovery_plans_preserve_installed_files_and_backups() {
        let root = crate::update::temporary_work("otd-invalid-recovery").unwrap();
        let journal = root.join(JOURNAL);
        fs::create_dir_all(journal.join("backup")).unwrap();
        fs::write(root.join("driver.exe"), b"current").unwrap();
        fs::write(journal.join("backup/driver.exe"), b"old").unwrap();
        for conflicting in [
            "driver.exe",
            "DRIVER.EXE",
            "driver.exe/child.dll",
            "../outside",
            "driver.exe.",
            "driver.exe ",
            "driver.exe:payload",
        ] {
            let plan = Plan {
                version: 1,
                entries: vec![
                    Entry {
                        path: "driver.exe".into(),
                        existed: false,
                    },
                    Entry {
                        path: conflicting.into(),
                        existed: true,
                    },
                ],
            };
            fs::write(
                journal.join("plan.json"),
                serde_json::to_vec(&plan).unwrap(),
            )
            .unwrap();
            assert!(startup(&root).is_err());
            assert_eq!(fs::read(root.join("driver.exe")).unwrap(), b"current");
            assert_eq!(fs::read(journal.join("backup/driver.exe")).unwrap(), b"old");
            assert!(!journal.join("rolled-back").exists());
        }
        assert!(!valid_paths([
            Path::new("lib/a.dll"),
            Path::new("lib\\a.dll")
        ]));
        for alias in [
            "driver.exe.",
            "driver.exe ",
            "lib./driver.exe",
            "lib /driver.exe",
            "driver.exe:payload",
        ] {
            assert!(
                !valid_paths([Path::new(alias)]),
                "Win32 alias accepted: {alias}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_preflights_all_directory_collisions_before_moving_any_file() {
        let root = crate::update::temporary_work("otd-recovery-collision").unwrap();
        let journal = root.join(JOURNAL);
        fs::create_dir_all(journal.join("backup")).unwrap();
        fs::create_dir_all(root.join("plugin.dll")).unwrap();
        fs::write(root.join("plugin.dll/local-settings.json"), b"local").unwrap();
        fs::write(root.join("driver.exe"), b"current").unwrap();
        fs::write(journal.join("backup/plugin.dll"), b"old").unwrap();
        let plan = Plan {
            version: 1,
            entries: vec![
                Entry {
                    path: "plugin.dll".into(),
                    existed: true,
                },
                Entry {
                    path: "driver.exe".into(),
                    existed: false,
                },
            ],
        };
        fs::write(
            journal.join("plan.json"),
            serde_json::to_vec(&plan).unwrap(),
        )
        .unwrap();
        assert!(
            startup(&root)
                .unwrap_err()
                .contains("non-file installation entry")
        );
        assert_eq!(fs::read(root.join("driver.exe")).unwrap(), b"current");
        assert_eq!(
            fs::read(root.join("plugin.dll/local-settings.json")).unwrap(),
            b"local"
        );
        assert_eq!(fs::read(journal.join("backup/plugin.dll")).unwrap(), b"old");
        assert!(!journal.join("discarded").exists());
        // A malformed non-file backup must also leave the current target and
        // journal intact, even when another rollback entry is otherwise valid.
        fs::remove_dir_all(root.join("plugin.dll")).unwrap();
        fs::write(root.join("plugin.dll"), b"current plugin").unwrap();
        fs::remove_file(journal.join("backup/plugin.dll")).unwrap();
        fs::create_dir(journal.join("backup/plugin.dll")).unwrap();
        fs::write(
            journal.join("backup/plugin.dll/local.json"),
            b"backup local",
        )
        .unwrap();
        assert!(startup(&root).is_err());
        assert_eq!(fs::read(root.join("driver.exe")).unwrap(), b"current");
        assert_eq!(
            fs::read(root.join("plugin.dll")).unwrap(),
            b"current plugin"
        );
        assert_eq!(
            fs::read(journal.join("backup/plugin.dll/local.json")).unwrap(),
            b"backup local"
        );
        assert!(journal.join("plan.json").exists());
        assert!(!journal.join("rolled-back").exists());
        fs::remove_dir_all(root).unwrap();
    }

    // Junction creation needs no symbolic-link privilege. Run the built-in in
    // a hidden, noninteractive child; remove only the temporary junction itself.
    fn junction(link: &Path, target: &Path) {
        use std::{
            ffi::OsString,
            os::windows::{
                ffi::{OsStrExt, OsStringExt},
                process::CommandExt,
            },
        };
        let windows_path = |path: &Path| {
            let units: Vec<_> = path
                .as_os_str()
                .encode_wide()
                .map(|unit| {
                    if unit == b'/' as u16 {
                        b'\\' as u16
                    } else {
                        unit
                    }
                })
                .collect();
            OsString::from_wide(&units)
        };
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(windows_path(link))
            .arg(windows_path(target))
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "junction fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn relative_junctions_preserve_external_files_but_root_junctions_remain_supported() {
        let root = crate::update::temporary_work("otd-relative-junction").unwrap();
        let source = root.join("source");
        let install = root.join("install");
        let external = root.join("external");
        fs::create_dir_all(source.join("lib")).unwrap();
        fs::create_dir_all(&install).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(source.join("lib/plugin.dll"), b"package").unwrap();
        fs::write(external.join("plugin.dll"), b"local external").unwrap();
        let paths = [PathBuf::from("lib/plugin.dll")];
        junction(&install.join("lib"), &external);
        assert!(
            replace(&source, &install, &paths)
                .unwrap_err()
                .contains("reparse point")
        );
        assert_eq!(
            fs::read(external.join("plugin.dll")).unwrap(),
            b"local external"
        );
        assert!(!install.join(JOURNAL).exists());
        fs::remove_dir(install.join("lib")).unwrap();
        // The journal itself is relative to the install root and cannot be a
        // junction either, including an empty/uncommitted journal.
        junction(&install.join(JOURNAL), &external);
        assert!(startup(&install).unwrap_err().contains("reparse point"));
        assert_eq!(
            fs::read(external.join("plugin.dll")).unwrap(),
            b"local external"
        );
        fs::remove_dir(install.join(JOURNAL)).unwrap();
        let journal = install.join(JOURNAL);
        fs::create_dir_all(journal.join("backup")).unwrap();
        fs::create_dir_all(install.join("lib")).unwrap();
        fs::write(install.join(&paths[0]), b"current").unwrap();
        junction(&journal.join("backup/lib"), &external);
        let plan = Plan {
            version: 1,
            entries: vec![Entry {
                path: paths[0].clone(),
                existed: true,
            }],
        };
        fs::write(
            journal.join("plan.json"),
            serde_json::to_vec(&plan).unwrap(),
        )
        .unwrap();
        assert!(startup(&install).unwrap_err().contains("reparse point"));
        assert_eq!(fs::read(install.join(&paths[0])).unwrap(), b"current");
        assert_eq!(
            fs::read(external.join("plugin.dll")).unwrap(),
            b"local external"
        );
        assert!(journal.join("plan.json").exists());
        fs::remove_dir(journal.join("backup/lib")).unwrap();
        fs::remove_dir_all(&journal).unwrap();
        // The explicitly selected installation root may legitimately be a
        // junction. Relative package entries are still inspected beneath it.
        let linked_root = root.join("linked-install");
        junction(&linked_root, &install);
        replace(&source, &linked_root, &paths).unwrap();
        assert_eq!(fs::read(install.join(&paths[0])).unwrap(), b"package");
        assert!(!startup(&linked_root).unwrap());
        fs::remove_dir(linked_root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replacement_rejects_directory_collision_without_moving_local_files() {
        let root = crate::update::temporary_work("otd-directory-collision").unwrap();
        let source = root.join("source");
        let install = root.join("install");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(install.join("plugin.dll")).unwrap();
        fs::write(source.join("plugin.dll"), b"package").unwrap();
        fs::write(install.join("plugin.dll/local-settings.json"), b"preserve").unwrap();
        assert!(
            replace(&source, &install, &[PathBuf::from("plugin.dll")])
                .unwrap_err()
                .contains("non-file installation entry")
        );
        assert_eq!(
            fs::read(install.join("plugin.dll/local-settings.json")).unwrap(),
            b"preserve"
        );
        assert!(!install.join(JOURNAL).exists());
        fs::remove_dir_all(root).unwrap();
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

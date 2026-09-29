//! Durable configuration files, separate from runtime configuration transactions.
//! A sidecar lock coordinates this implementation's writers. Exact-byte snapshot
//! checks detect external edits, but are not an atomic compare-and-swap against
//! applications which ignore that lock and write between the final check/rename.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct FileSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

impl FileSnapshot {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn exists(&self) -> bool {
        self.contents.is_some()
    }
    pub fn matches_path(&self, path: &Path) -> Result<bool, String> {
        Ok(self.path == absolute_path(path)?)
    }
}

pub struct LoadedFile {
    pub text: String,
    pub snapshot: FileSnapshot,
}

#[derive(Clone, Copy)]
pub enum SaveMode<'a> {
    /// Publish a new path without replacing any existing file.
    CreateNew,
    /// Replace only if the bytes still match the file that was loaded.
    Replace(&'a FileSnapshot),
}

pub fn capture(path: &Path) -> Result<FileSnapshot, String> {
    let path = absolute_path(path)?;
    let contents = match fs::read(&path) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    Ok(FileSnapshot { path, contents })
}

pub fn read_utf8(path: &Path) -> Result<LoadedFile, String> {
    let snapshot = capture(path)?;
    let bytes = snapshot
        .contents
        .as_ref()
        .ok_or_else(|| format!("{} does not exist", path.display()))?;
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("{} is not UTF-8: {error}", path.display()))?
        .to_owned();
    Ok(LoadedFile { text, snapshot })
}

pub fn backup_path(path: &Path) -> Result<PathBuf, String> {
    sibling(&absolute_path(path)?, ".bak")
}

pub fn read_backup(path: &Path) -> Result<LoadedFile, String> {
    read_utf8(&backup_path(path)?)
}

/// Synchronize the new file before publication, retaining the previous bytes
/// at `<name>.bak`. A failed new-path publish leaves the destination untouched.
/// Windows uses a write-through rename; Unix new-path publication uses a hard
/// link and therefore needs filesystem support for that operation.
pub fn save(path: &Path, bytes: &[u8], mode: SaveMode<'_>) -> Result<FileSnapshot, String> {
    save_inner(path, bytes, mode, true)
}

/// Explicit recovery keeps the known backup intact, including when the current
/// primary file is corrupt. The caller validates its own document format first.
pub fn recover_backup(
    path: &Path,
    expected: &FileSnapshot,
    validate: impl FnOnce(&str) -> Result<(), String>,
) -> Result<FileSnapshot, String> {
    let recovered = read_backup(path)?;
    validate(&recovered.text)?;
    save_inner(
        path,
        recovered.text.as_bytes(),
        SaveMode::Replace(expected),
        false,
    )
}

fn save_inner(
    path: &Path,
    bytes: &[u8],
    mode: SaveMode<'_>,
    rotate_backup: bool,
) -> Result<FileSnapshot, String> {
    let path = absolute_path(path)?;
    let parent = path.parent().ok_or("output has no parent directory")?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    let _lock = WriterLock::acquire(&sibling(&path, ".lock")?)?;
    let previous = verify(&path, mode)?;
    if let Ok(metadata) = fs::metadata(&path)
        && metadata.permissions().readonly()
    {
        return Err(format!(
            "{} is read-only; choose a new Save As path",
            path.display()
        ));
    }
    let temporary = Temporary::write(&path, bytes)?;
    if let Ok(metadata) = fs::metadata(&path) {
        fs::set_permissions(&temporary.path, metadata.permissions())
            .map_err(|error| format!("cannot preserve profile file permissions: {error}"))?;
    }
    if rotate_backup && let Some(previous) = previous.as_ref() {
        let backup = backup_path(&path)?;
        let backup_temporary = Temporary::write(&backup, previous)?;
        publish(&backup_temporary.path, &backup, true).map_err(|error| {
            format!(
                "cannot update backup {}: {error}; original profile was not replaced",
                backup.display()
            )
        })?;
        sync_directory(parent)?;
    }
    // Recheck after staging/backup work, keeping the remaining external-writer
    // race limited to publication itself. Cooperating saves hold the same lock.
    verify(&path, mode)?;
    publish(&temporary.path, &path, previous.is_some()).map_err(|error| {
        format!(
            "cannot publish {}: {error}; previous destination was not replaced",
            path.display()
        )
    })?;
    sync_directory(parent)?;
    Ok(FileSnapshot {
        path,
        contents: Some(bytes.to_vec()),
    })
}

fn verify(path: &Path, mode: SaveMode<'_>) -> Result<Option<Vec<u8>>, String> {
    // Do not replace a symlink itself while comparing bytes from its target.
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        return Err(format!(
            "refusing to replace symbolic link {}; select its real destination",
            path.display()
        ));
    }
    let current = capture(path)?;
    match mode {
        SaveMode::CreateNew if current.exists() => Err(format!(
            "{} already exists; Save As requires a new file name",
            path.display()
        )),
        SaveMode::CreateNew => Ok(None),
        SaveMode::Replace(expected) if expected.path != current.path => Err(
            "saved-file snapshot belongs to another path; use Save As for a new destination".into(),
        ),
        SaveMode::Replace(expected) if expected.contents != current.contents => Err(format!(
            "{} changed outside this editor; reload it or use Save As to keep both versions",
            path.display()
        )),
        SaveMode::Replace(_) => Ok(current.contents),
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    absolute
        .file_name()
        .ok_or("profile path must name a file")?;
    Ok(absolute)
}

fn sibling(path: &Path, suffix: &str) -> Result<PathBuf, String> {
    let mut name = path
        .file_name()
        .ok_or("profile path must name a file")?
        .to_os_string();
    name.push(suffix);
    Ok(path.with_file_name(name))
}

/// An immutable marker is published atomically, then the OS owns the lock.
/// Never remove this file: unlinking it lets two writers lock different files.
/// Old versions using create_new fail closed on the persistent marker. Their
/// pid-only sentinels cannot prove an idle writer and require explicit recovery.
pub(crate) struct WriterLock {
    _file: File,
}

impl WriterLock {
    pub(crate) fn acquire(path: &Path) -> Result<Self, String> {
        const MARKER: &[u8] = b"OpenTabletDriver Rust OS writer lock v2\n";
        if !path.try_exists().map_err(|error| error.to_string())? {
            let marker = Temporary::write(path, MARKER)?;
            if let Err(error) = publish(&marker.path, path, false) {
                if !path.try_exists().map_err(|error| error.to_string())? {
                    return Err(format!("cannot publish writer lock {}: {error}", path.display()));
                }
            }
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Permit competing handles to observe the OS lock, but prohibit
            // unlink/rename while one writer could still own this file.
            options.share_mode(0x0000_0001 | 0x0000_0002); // FILE_SHARE_READ | FILE_SHARE_WRITE
        }
        let mut file = options.open(path)
            .map_err(|error| format!("cannot open writer lock {}: {error}", path.display()))?;
        file.try_lock().map_err(|error| format!("cannot acquire writer lock {}: {error}", path.display()))?;
        let mut marker = Vec::with_capacity(MARKER.len() + 1);
        (&mut file).take((MARKER.len() + 1) as u64).read_to_end(&mut marker)
            .map_err(|error| error.to_string())?;
        if marker != MARKER {
            return Err(format!("legacy or unrecognized writer lock {}: close all older driver/panel versions, confirm no save is running, then remove this legacy lock once. Modern v2 lock files stay in place and recover automatically after a crash.", path.display()));
        }
        Ok(Self { _file: file })
    }
}

struct Temporary {
    path: PathBuf,
}

impl Temporary {
    fn write(destination: &Path, bytes: &[u8]) -> Result<Self, String> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
        let path = sibling(
            destination,
            &format!(".{}-{stamp}-{sequence}.tmp", std::process::id()),
        )?;
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&path)
            .map_err(|error| format!("cannot stage {}: {error}", destination.display()))?;
        // The guard exists only after create_new succeeded, so cleanup can never
        // delete a file that made exclusive creation fail.
        let temporary = Self { path };
        let outcome = file.write_all(bytes).and_then(|_| file.sync_all());
        drop(file);
        outcome.map_err(|error| format!("cannot flush staged profile: {error}"))?;
        Ok(temporary)
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn sync_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("could not flush directory {}: {error}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn publish(from: &Path, to: &Path, replace: bool) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            #[link_name = "MoveFileExW"]
            fn move_file_ex(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
        }
        let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        // MOVEFILE_WRITE_THROUGH | optional MOVEFILE_REPLACE_EXISTING. Both
        // paths are siblings, so no copy/delete fallback crosses filesystems.
        if unsafe { move_file_ex(from.as_ptr(), to.as_ptr(), 8 | u32::from(replace)) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        if replace {
            fs::rename(from, to)
        } else {
            fs::hard_link(from, to)
        }
    }
}

/// An explicit portable directory takes priority. This function does not create
/// directories; callers choose when persistence is requested.
pub fn data_directory() -> Result<PathBuf, String> {
    if let Some(directory) = std::env::var_os("OTD_RUST_PORTABLE_DIR") {
        let path = PathBuf::from(directory);
        if !path.is_absolute() {
            return Err("OTD_RUST_PORTABLE_DIR must be an absolute directory".into());
        }
        return Ok(path);
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(|path| PathBuf::from(path).join("OpenTabletDriverRust"))
            .ok_or_else(|| {
                "LOCALAPPDATA is unavailable; set OTD_RUST_PORTABLE_DIR explicitly".into()
            })
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|path| {
                PathBuf::from(path).join("Library/Application Support/OpenTabletDriverRust")
            })
            .ok_or_else(|| "HOME is unavailable; set OTD_RUST_PORTABLE_DIR explicitly".into())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or("no user config directory; set OTD_RUST_PORTABLE_DIR explicitly")?;
        Ok(base.join("opentabletdriver-rust"))
    }
}

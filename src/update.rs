//! Updates from this project's GitHub releases. The panel and the `update`
//! command check the latest release, download its Windows package and its
//! SHA-256 file over HTTPS (`crate::download`), verify the hash with the
//! Windows crypto API, extract with tar.exe and replace the installed files.
//! Replacement is serialized and journaled: incomplete updates roll back on
//! the next startup. Recovery failures preserve backups and stop startup.
//! Committed backups are removed once running processes release their DLLs.
//!
//! A private repository needs a GitHub token: `GH_TOKEN`, `GITHUB_TOKEN` or
//! the logged-in GitHub CLI's (`gh auth token`). It is sent only to GitHub's
//! API, never to the storage host a download redirects to.

use std::fs;
use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
pub(crate) mod transaction;

pub const REPOSITORY: &str = "z9a17/opentabletdriver-rust";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: (u64, u64, u64),
    pub page: String,
    pub package_url: String,
    pub checksum_url: String,
    /// The same files through the API, for authenticated downloads.
    pub package_api_url: String,
    pub checksum_api_url: String,
    pub package_name: String,
}

/// A GitHub token for a private repository, if one is available.
fn token(cancel: Option<&AtomicBool>) -> Option<String> {
    ["GH_TOKEN", "GITHUB_TOKEN"]
        .iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|token| !token.trim().is_empty())
        })
        .or_else(|| {
            let output = crate::download::run(Command::new("gh")
                .creation_flags(CREATE_NO_WINDOW)
                .args(["auth", "token"]), cancel)
                .ok()?;
            let token = String::from_utf8(output.stdout).ok()?.trim().to_owned();
            (output.status.success() && !token.is_empty()).then_some(token)
        })
}

/// `v1.2.3` or `1.2.3` as numbers; anything else is not a release version.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.strip_prefix('v').unwrap_or(text).split('.');
    let version = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(version)
}

pub fn current_version() -> (u64, u64, u64) {
    parse_version(env!("CARGO_PKG_VERSION")).expect("the package version is numeric")
}

pub(crate) fn system_tool(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    PathBuf::from(root).join("System32").join(name)
}

/// The latest published release, from the GitHub API.
pub fn latest() -> Result<Release, String> {
    latest_with_cancel(None)
}
pub(crate) fn latest_with_cancel(cancel: Option<&AtomicBool>) -> Result<Release, String> {
    crate::download::cancelled(cancel)?;
    let token = token(cancel);
    let body = crate::download::to_memory_with_cancel(
        &format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"),
        Some("application/vnd.github+json"),
        token.as_deref(),
        cancel,
    )
    .map_err(|error| {
        if token.is_none() {
            format!("{error} (a private repository needs GH_TOKEN or a logged-in GitHub CLI)")
        } else {
            error
        }
    })?;
    parse_release(&body)
}

/// Downloads a release file, through the API when a token is available.
fn download(public: &str, api: &str, output: Option<&Path>, cancel: Option<&AtomicBool>) -> Result<Vec<u8>, String> {
    crate::download::cancelled(cancel)?;
    let token = token(cancel);
    let (url, accept) = match &token {
        Some(_) => (api, Some("application/octet-stream")),
        None => (public, None),
    };
    match output {
        Some(path) => {
            crate::download::to_file_with_cancel(url, accept, token.as_deref(), path, cancel).map(|()| Vec::new())
        }
        None => crate::download::to_memory_with_cancel(url, accept, token.as_deref(), cancel),
    }
}

fn parse_release(body: &[u8]) -> Result<Release, String> {
    let json: serde_json::Value = serde_json::from_slice(body)
        .map_err(|error| format!("unexpected release data: {error}"))?;
    let tag = json["tag_name"]
        .as_str()
        .ok_or("the latest release has no tag")?
        .to_owned();
    let version = parse_version(&tag).ok_or_else(|| format!("{tag} is not a version"))?;
    let assets = json["assets"]
        .as_array()
        .ok_or("the latest release has no files")?;
    let asset = |suffix: &str| {
        assets.iter().find_map(|asset| {
            let name = asset["name"].as_str()?;
            (name.starts_with("opentabletdriver-rust-") && name.ends_with(suffix)).then(|| {
                (
                    name.to_owned(),
                    asset["browser_download_url"].as_str().map(str::to_owned),
                    asset["url"].as_str().map(str::to_owned),
                )
            })
        })
    };
    let (package_name, package_url, package_api_url) =
        asset("-win-x64.zip").ok_or_else(|| format!("{tag} has no Windows package"))?;
    let (_, checksum_url, checksum_api_url) =
        asset("-win-x64.zip.sha256").ok_or_else(|| format!("{tag} has no package checksum"))?;
    let within = |url: Option<String>, prefix: &str| {
        url.filter(|url| url.starts_with(prefix))
            .ok_or_else(|| format!("{tag} has an unexpected download location"))
    };
    let public = format!("https://github.com/{REPOSITORY}/releases/download/");
    let api = format!("https://api.github.com/repos/{REPOSITORY}/releases/assets/");
    Ok(Release {
        page: json["html_url"].as_str().unwrap_or_default().to_owned(),
        package_url: within(package_url, &public)?,
        checksum_url: within(checksum_url, &public)?,
        package_api_url: within(package_api_url, &api)?,
        checksum_api_url: within(checksum_api_url, &api)?,
        package_name,
        tag,
        version,
    })
}

/// SHA-256 of a file, as lowercase hex, with the Windows CNG provider.
pub fn sha256(path: &Path) -> Result<String, String> {
    use std::io::Read;
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_HASH_HANDLE, BCRYPT_SHA256_ALG_HANDLE, BCryptCreateHash,
        BCryptDestroyHash, BCryptFinishHash, BCryptHashData,
    };
    struct Hash(BCRYPT_HASH_HANDLE);
    impl Drop for Hash {
        fn drop(&mut self) { unsafe { BCryptDestroyHash(self.0) }; }
    }
    let mut file = fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut handle = std::ptr::null_mut();
    // CNG owns the hash object buffer and releases it with BCryptDestroyHash.
    let status = unsafe {
        BCryptCreateHash(
            BCRYPT_SHA256_ALG_HANDLE, &mut handle, std::ptr::null_mut(), 0,
            std::ptr::null(), 0, 0,
        )
    };
    if status != 0 {
        return Err(format!("SHA-256 failed with status {status:#x}"));
    }
    let hash = Hash(handle);
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let length = match file.read(&mut buffer) {
            Ok(length) => length,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        if length == 0 { break; }
        let status = unsafe { BCryptHashData(hash.0, buffer.as_ptr(), length as u32, 0) };
        if status != 0 { return Err(format!("SHA-256 failed with status {status:#x}")); }
    }
    let mut digest = [0u8; 32];
    let status = unsafe { BCryptFinishHash(hash.0, digest.as_mut_ptr(), digest.len() as u32, 0) };
    if status != 0 { return Err(format!("SHA-256 failed with status {status:#x}")); }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(64);
    for byte in digest {
        text.push(HEX[usize::from(byte >> 4)] as char);
        text.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    Ok(text)
}

/// Every file under `root`, relative to it.
fn files(root: &Path, directory: &Path, found: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            files(root, &path, found)?;
        } else {
            found.push(
                path.strip_prefix(root)
                    .expect("walked under root")
                    .to_path_buf(),
            );
        }
    }
    Ok(())
}

/// Downloads, verifies and installs `release` into `install`, reporting each
/// step. Failures attempt rollback; blocked recovery preserves the journal
/// and its backups for the next startup.
#[cfg(windows)]
pub fn install(release: &Release, install: &Path, progress: &dyn Fn(&str)) -> Result<(), String> {
    // The global managed tool owner outlives tablet sessions. Every native
    // installer must reserve the same daemon drain used by the original RPC
    // updater before touching DLLs; a missing daemon never starts implicitly.
    use crate::control::{self, Command as ControlCommand, Reply, Request};
    use std::time::{Duration, Instant};
    let endpoint = control::endpoint_name().map_err(|error| error.to_string())?;
    let process = control::pipe::CompatPipe::server_process_id(&endpoint)
        .map_err(|error| format!("Update installation requires the existing native daemon: {error}. Check for updates remains available offline."))?;
    let call = |command| -> Result<Reply, String> {
        let response = control::request_owned(&Request::new(1, command), Duration::from_secs(5), process)
            .map_err(|error| error.to_string())?;
        match response.reply {
            Reply::Error { error } => Err(error.message),
            reply => Ok(reply),
        }
    };
    let expected = match call(ControlCommand::Status)? {
        Reply::Status { status } => status.identity(),
        _ => return Err("unexpected daemon status before update".into()),
    };
    let token = match call(ControlCommand::BeginUpdate { expected })? {
        Reply::UpdateAccepted { token } => token,
        _ => return Err("unexpected update reservation reply".into()),
    };
    let mut exit = false;
    let result = (|| -> Result<(), String> {
        progress("Waiting for tablet sessions and global tools to release their resources...");
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            match call(ControlCommand::UpdateStatus { token: token.clone() })? {
                Reply::UpdateState { token: actual, ready, error } if actual == token => {
                    if let Some(error) = error { return Err(error); }
                    if ready { break; }
                }
                _ => return Err("unexpected update reservation state".into()),
            }
            if Instant::now() >= deadline { return Err("update resource drain timed out".into()); }
            std::thread::sleep(Duration::from_millis(25));
        }
        match install_with_cancel(release, install, progress, None) {
            Ok(()) => { exit = true; Ok(()) }
            Err(error) => match recover_for_daemon(install) {
                // A download/preflight failure never replaced installed files.
                // Cancellation releases the reservation and restores tools.
                Ok(false) => Err(error),
                Ok(true) => {
                    exit = true;
                    Err(format!("{error}; interrupted update recovered; daemon will exit before restored files are used"))
                }
                Err(recovery) => {
                    exit = true;
                    Err(format!("{error}; update recovery failed: {recovery}; daemon will exit and preserve recovery files"))
                }
            },
        }
    })();
    let cleanup = (|| -> Result<(), String> {
        // Cancellation completes only after the daemon's tool dispatcher has
        // restored the exact pre-update configuration. Busy is an ownership
        // receipt still pending, not permission to repeat any installation.
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            let response = control::request_owned(&Request::new(1, ControlCommand::FinishUpdate {
                token: token.clone(), success: exit,
            }), Duration::from_secs(5), process).map_err(|error| error.to_string())?;
            match response.reply {
                Reply::ShutdownAccepted if exit => return Ok(()),
                Reply::UpdateCancelled if !exit => return Ok(()),
                Reply::Error { error } if !exit && matches!(error.code, control::ErrorCode::Busy) => {
                    if Instant::now() >= deadline { return Err(format!("global tool restoration timed out: {}", error.message)); }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Reply::Error { error } => return Err(error.message),
                _ => return Err("unexpected update ownership completion reply".to_owned()),
            }
        }
    })().map_err(|error|format!("update ownership cleanup failed: {error}; the daemon may still be reserved; inspect its status before restarting"));
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(format!("{error}; {cleanup}")),
    }
}
// Portable services own their reservation around this shared installer.
#[cfg(unix)]
pub fn install(release: &Release, install: &Path, progress: &dyn Fn(&str)) -> Result<(), String> {
    install_with_cancel(release, install, progress, None)
}
pub(crate) fn install_with_cancel(release: &Release, install: &Path, progress: &dyn Fn(&str), cancel: Option<&AtomicBool>) -> Result<(), String> {
    crate::download::cancelled(cancel)?;
    let work = temporary_work("otd-update")?;
    let result = (|| {
        let package = work.join(&release.package_name);
        progress(&format!("Downloading {}...", release.package_name));
        download(
            &release.package_url,
            &release.package_api_url,
            Some(&package),
            cancel,
        )?;
        let expected = String::from_utf8(download(
            &release.checksum_url,
            &release.checksum_api_url,
            None,
            cancel,
        )?)
        .map_err(|_| "the checksum file is not text")?;
        let expected = expected
            .split_whitespace()
            .next()
            .ok_or("the checksum file is empty")?
            .to_ascii_lowercase();
        let actual = sha256(&package)?;
        if actual != expected {
            return Err(format!(
                "the download's SHA-256 {actual} does not match the published {expected}; nothing was installed"
            ));
        }
        progress("Checksum verified. Extracting...");
        let extracted = work.join("extracted");
        fs::create_dir_all(&extracted).map_err(|error| error.to_string())?;
        let status = crate::download::run(Command::new(system_tool("tar.exe"))
            .creation_flags(CREATE_NO_WINDOW)
            .arg("-xf")
            .arg(&package)
            .arg("-C")
            .arg(&extracted), cancel)?.status;
        if !status.success() {
            return Err("the package could not be extracted".into());
        }
        let root = extracted.join(release.package_name.trim_end_matches(".zip"));
        let root = if root.is_dir() { root } else { extracted };
        if !root.join("opentabletdriver-rust.exe").is_file() {
            return Err("the package does not contain opentabletdriver-rust.exe".into());
        }
        let mut new_files = Vec::new();
        files(&root, &root, &mut new_files).map_err(|error| error.to_string())?;
        progress(&format!("Installing {} files...", new_files.len()));
        // Cancellation is safe while preparing scratch files. Once replacement
        // starts, complete its journal/rollback before releasing daemon ownership.
        crate::download::cancelled(cancel)?;
        replace(&root, install, &new_files)?;
        progress(&format!(
            "{} is installed. Restart the driver and panel to use it.",
            release.tag
        ));
        Ok(())
    })();
    let _ = fs::remove_dir_all(&work);
    result
}

fn replace(source: &Path, install: &Path, relative: &[PathBuf]) -> Result<(), String> {
    transaction::replace(source, install, relative)
}

/// Recovers an interrupted transaction before loading any installed bridge.
/// Active transactions are locked; unjournaled legacy backups are preserved.
pub fn remove_leftovers() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let directory = exe.parent().ok_or("cannot find install directory")?;
    if transaction::startup(directory)? {
        return Err("An interrupted update was recovered. Start the driver or panel again to use the restored files.".into());
    }
    Ok(())
}
/// Called only under a drained daemon update reservation after a failed install.
/// A recovered transaction requires exiting the old process before reuse.
pub(crate) fn recover_for_daemon(install: &Path) -> Result<bool, String> {
    transaction::startup(install)
}

pub(crate) fn temporary_work(prefix: &str) -> Result<PathBuf, String> {
    unique_directory(&std::env::temp_dir(), prefix)
}

pub(crate) fn unique_directory(root: &Path, prefix: &str) -> Result<PathBuf, String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = root.join(format!("{prefix}-{}-{stamp}-{id}", std::process::id()));
    fs::create_dir(&path).map_err(|error| error.to_string())?;
    Ok(path)
}
/// `update [--check]`: prints whether a newer release exists and installs it.
pub fn run(check_only: bool) -> Result<(), String> {
    let release = latest()?;
    let current = current_version();
    if release.version <= current {
        println!(
            "opentabletdriver-rust {} is up to date (latest release {}).",
            env!("CARGO_PKG_VERSION"),
            release.tag
        );
        return Ok(());
    }
    println!("{} is available: {}", release.tag, release.page);
    if check_only {
        return Ok(());
    }
    let folder = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .parent()
        .ok_or("cannot find the install folder")?
        .to_path_buf();
    install(&release, &folder, &|line| println!("{line}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_compare() {
        assert_eq!(parse_version("v0.11.0"), Some((0, 11, 0)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v1.2"), None);
        assert_eq!(parse_version("v1.2.3-beta"), None);
        assert!(parse_version("v0.10.0").unwrap() < parse_version("v0.11.0").unwrap());
    }

    #[test]
    fn release_data_is_read_and_checked() {
        let body = serde_json::json!({
            "tag_name": "v9.8.7",
            "html_url": "https://github.com/z9a17/opentabletdriver-rust/releases/tag/v9.8.7",
            "assets": [
                {"name": "opentabletdriver-rust-v9.8.7-win-x64.zip",
                 "url": "https://api.github.com/repos/z9a17/opentabletdriver-rust/releases/assets/1",
                 "browser_download_url": "https://github.com/z9a17/opentabletdriver-rust/releases/download/v9.8.7/opentabletdriver-rust-v9.8.7-win-x64.zip"},
                {"name": "opentabletdriver-rust-v9.8.7-win-x64.zip.sha256",
                 "url": "https://api.github.com/repos/z9a17/opentabletdriver-rust/releases/assets/2",
                 "browser_download_url": "https://github.com/z9a17/opentabletdriver-rust/releases/download/v9.8.7/opentabletdriver-rust-v9.8.7-win-x64.zip.sha256"}
            ]
        });
        let release = parse_release(body.to_string().as_bytes()).unwrap();
        assert_eq!(release.version, (9, 8, 7));
        assert_eq!(
            release.package_name,
            "opentabletdriver-rust-v9.8.7-win-x64.zip"
        );
        let mut elsewhere = body.clone();
        elsewhere["assets"][0]["browser_download_url"] = "https://example.com/x.zip".into();
        assert!(parse_release(elsewhere.to_string().as_bytes()).is_err());
    }

    #[test]
    fn sha256_matches_a_known_digest() {
        let path = std::env::temp_dir().join(format!("otd-sha-{}", std::process::id()));
        fs::write(&path, b"abc").unwrap();
        let digest = sha256(&path);
        let _ = fs::remove_file(&path);
        assert_eq!(
            digest.unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_failed_replacement_restores_every_file() {
        let root = std::env::temp_dir().join(format!("otd-update-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (source, install) = (root.join("new"), root.join("installed"));
        fs::create_dir_all(source.join("compat")).unwrap();
        fs::create_dir_all(&install).unwrap();
        fs::write(source.join("a.exe"), b"new a").unwrap();
        fs::write(source.join("compat/b.dll"), b"new b").unwrap();
        fs::write(install.join("a.exe"), b"old a").unwrap();
        // The second file is missing from the package, so copying it fails.
        let broken = [PathBuf::from("a.exe"), PathBuf::from("compat/missing.dll")];
        assert!(replace(&source, &install, &broken).is_err());
        assert_eq!(fs::read(install.join("a.exe")).unwrap(), b"old a");
        assert!(!install.join(".otd-update/backup/a.exe").exists());
        let files = [PathBuf::from("a.exe"), PathBuf::from("compat/b.dll")];
        replace(&source, &install, &files).unwrap();
        assert_eq!(fs::read(install.join("compat/b.dll")).unwrap(), b"new b");
        assert_eq!(
            fs::read(install.join(".otd-update/backup/a.exe")).unwrap(),
            b"old a"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// Downloads and installs the latest release into a temporary folder.
    #[test]
    #[ignore = "downloads the latest release from GitHub"]
    fn installs_the_latest_release_into_a_folder() {
        let folder = std::env::temp_dir().join(format!("otd-install-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("opentabletdriver-rust.exe"), b"previous").unwrap();
        let release = latest().unwrap();
        install(&release, &folder, &|line| println!("{line}")).unwrap();
        assert!(
            fs::metadata(folder.join("opentabletdriver-rust.exe"))
                .unwrap()
                .len()
                > 100_000
        );
        assert!(folder.join("data/compat").is_dir());
        assert_eq!(
            fs::read(folder.join(".otd-update/backup/opentabletdriver-rust.exe")).unwrap(),
            b"previous"
        );
        fs::remove_dir_all(&folder).unwrap();
    }
}

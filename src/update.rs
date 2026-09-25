//! Updates from this project's GitHub releases. The panel and the `update`
//! command check the latest release, download its Windows package and its
//! SHA-256 file over HTTPS with Windows' curl.exe, verify the hash with the
//! Windows crypto API, extract with tar.exe and replace the installed files.
//! Running executables and loaded DLLs are renamed aside, not overwritten, so
//! a failed update restores every file and the running driver keeps working.
//! The renamed files are removed the next time either executable starts.
//!
//! A private repository needs a GitHub token: `GH_TOKEN`, `GITHUB_TOKEN` or
//! the logged-in GitHub CLI's (`gh auth token`). curl reads it from a
//! temporary header file, so it never appears on a command line, and does not
//! send it on to the storage host a download redirects to.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const REPOSITORY: &str = "z9a17/opentabletdriver-rust";
const OLD_SUFFIX: &str = ".old-update";

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
fn token() -> Option<String> {
    ["GH_TOKEN", "GITHUB_TOKEN"]
        .iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|token| !token.trim().is_empty())
        })
        .or_else(|| {
            let output = Command::new("gh").args(["auth", "token"]).output().ok()?;
            let token = String::from_utf8(output.stdout).ok()?.trim().to_owned();
            (output.status.success() && !token.is_empty()).then_some(token)
        })
}

/// Deletes the header file holding a token when the request is done.
struct HeaderFile(PathBuf);

impl Drop for HeaderFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
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

fn system_tool(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    PathBuf::from(root).join("System32").join(name)
}

fn curl(arguments: &[&str], token: Option<&str>) -> Result<Vec<u8>, String> {
    let mut command = Command::new(system_tool("curl.exe"));
    let _header = match token {
        Some(token) => {
            let path = std::env::temp_dir().join(format!(
                "otd-update-auth-{}-{:?}.txt",
                std::process::id(),
                std::thread::current().id()
            ));
            fs::write(&path, format!("Authorization: Bearer {token}\n"))
                .map_err(|error| format!("cannot prepare the request: {error}"))?;
            command.arg("--header").arg(format!("@{}", path.display()));
            Some(HeaderFile(path))
        }
        None => None,
    };
    let output = command
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--location",
            "--max-time",
            "120",
        ])
        .args([
            "--user-agent",
            concat!("opentabletdriver-rust/", env!("CARGO_PKG_VERSION")),
        ])
        .args(arguments)
        .output()
        .map_err(|error| format!("cannot run curl.exe: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "download failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

/// The latest published release, from the GitHub API.
pub fn latest() -> Result<Release, String> {
    let token = token();
    let body = curl(
        &[
            "--header",
            "Accept: application/vnd.github+json",
            &format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"),
        ],
        token.as_deref(),
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
fn download(public: &str, api: &str, output: Option<&Path>) -> Result<Vec<u8>, String> {
    let token = token();
    let path = output.map(|path| path.to_string_lossy().into_owned());
    let mut arguments = Vec::new();
    if let Some(path) = &path {
        arguments.extend(["--output", path.as_str()]);
    }
    match &token {
        Some(_) => arguments.extend(["--header", "Accept: application/octet-stream", api]),
        None => arguments.push(public),
    }
    curl(&arguments, token.as_deref())
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
    use windows_sys::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
    let data = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut digest = [0u8; 32];
    let status = unsafe {
        BCryptHash(
            BCRYPT_SHA256_ALG_HANDLE,
            std::ptr::null(),
            0,
            data.as_ptr(),
            u32::try_from(data.len()).map_err(|_| "package is too large")?,
            digest.as_mut_ptr(),
            digest.len() as u32,
        )
    };
    if status != 0 {
        return Err(format!("SHA-256 failed with status {status:#x}"));
    }
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
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

fn aside(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(OLD_SUFFIX);
    PathBuf::from(name)
}

/// Downloads, verifies and installs `release` into `install`, reporting each
/// step. On failure every replaced file is restored.
pub fn install(release: &Release, install: &Path, progress: &dyn Fn(&str)) -> Result<(), String> {
    let work = std::env::temp_dir().join(format!("opentabletdriver-rust-update-{}", release.tag));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|error| format!("{}: {error}", work.display()))?;
    let package = work.join(&release.package_name);
    progress(&format!("Downloading {}...", release.package_name));
    download(
        &release.package_url,
        &release.package_api_url,
        Some(&package),
    )?;
    let expected = String::from_utf8(download(
        &release.checksum_url,
        &release.checksum_api_url,
        None,
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
    let status = Command::new(system_tool("tar.exe"))
        .arg("-xf")
        .arg(&package)
        .arg("-C")
        .arg(&extracted)
        .status()
        .map_err(|error| format!("cannot run tar.exe: {error}"))?;
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
    replace(&root, install, &new_files)?;
    let _ = fs::remove_dir_all(&work);
    progress(&format!(
        "{} is installed. Restart the driver and panel to use it.",
        release.tag
    ));
    Ok(())
}

/// Replaces files one by one, renaming each existing file aside first.
fn replace(source: &Path, install: &Path, relative: &[PathBuf]) -> Result<(), String> {
    let mut done: Vec<(PathBuf, bool)> = Vec::new();
    let result = (|| -> Result<(), String> {
        for file in relative {
            let target = install.join(file);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("{}: {error}", parent.display()))?;
            }
            let existed = target.exists();
            if existed {
                let old = aside(&target);
                let _ = fs::remove_file(&old);
                fs::rename(&target, &old)
                    .map_err(|error| format!("cannot move {} aside: {error}", target.display()))?;
            }
            done.push((target.clone(), existed));
            fs::copy(source.join(file), &target)
                .map_err(|error| format!("cannot write {}: {error}", target.display()))?;
        }
        Ok(())
    })();
    if result.is_err() {
        for (target, existed) in done.iter().rev() {
            let _ = fs::remove_file(target);
            if *existed {
                let _ = fs::rename(aside(target), target);
            }
        }
    }
    result
}

/// Deletes files a previous update renamed aside. Files still in use stay
/// until a later start.
pub fn remove_leftovers() {
    let Some(directory) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return;
    };
    let mut found = Vec::new();
    if files(&directory, &directory, &mut found).is_ok() {
        for file in found {
            if file.to_string_lossy().ends_with(OLD_SUFFIX) {
                let _ = fs::remove_file(directory.join(file));
            }
        }
    }
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
        assert!(!aside(&install.join("a.exe")).exists());
        let files = [PathBuf::from("a.exe"), PathBuf::from("compat/b.dll")];
        replace(&source, &install, &files).unwrap();
        assert_eq!(fs::read(install.join("compat/b.dll")).unwrap(), b"new b");
        assert_eq!(fs::read(aside(&install.join("a.exe"))).unwrap(), b"old a");
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
        assert!(folder.join("compat").is_dir());
        assert_eq!(
            fs::read(aside(&folder.join("opentabletdriver-rust.exe"))).unwrap(),
            b"previous"
        );
        fs::remove_dir_all(&folder).unwrap();
    }
}

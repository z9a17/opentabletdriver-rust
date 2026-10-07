//! HTTPS downloads for updates and plugins, with Windows' curl.exe.
//!
//! OpenTabletDriver downloads with .NET's HttpClient, which uses the Windows
//! proxy settings and does not check certificate revocation. curl.exe reads
//! only proxy environment variables, and fails when Windows cannot reach a
//! certificate's revocation list. GitHub serves release files from another
//! host than its API, with a certificate from another authority, so either
//! difference can break plugin and update downloads while the catalog loads.
//!
//! So curl follows redirects here one request at a time, each through the
//! proxy Windows picks for that address, and skips the revocation check like
//! HttpClient. Certificates and host names are still verified, and plugins
//! and update packages are still checked against their published SHA-256.

use std::fs;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(unix)]
use crate::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
#[cfg(windows)]
use std::ptr::null;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{GetLastError, GlobalFree};
#[cfg(windows)]
use windows_sys::Win32::Networking::WinHttp::{
    ERROR_WINHTTP_AUTO_PROXY_SERVICE_ERROR, WINHTTP_ACCESS_TYPE_NAMED_PROXY,
    WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_AUTO_DETECT_TYPE_DHCP, WINHTTP_AUTO_DETECT_TYPE_DNS_A,
    WINHTTP_AUTOPROXY_AUTO_DETECT, WINHTTP_AUTOPROXY_CONFIG_URL, WINHTTP_AUTOPROXY_OPTIONS,
    WINHTTP_AUTOPROXY_RUN_INPROCESS, WINHTTP_CURRENT_USER_IE_PROXY_CONFIG, WINHTTP_PROXY_INFO,
    WinHttpCloseHandle, WinHttpGetIEProxyConfigForCurrentUser, WinHttpGetProxyForUrl, WinHttpOpen,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
#[cfg(windows)]
use windows_sys::core::PWSTR;

const USER_AGENT: &str = concat!("opentabletdriver-rust/", env!("CARGO_PKG_VERSION"));
#[cfg(unix)]
const CREATE_NO_WINDOW:u32=0;
const MAX_REDIRECTS: usize = 10;

/// Holds a GitHub token in a temporary header file, so it never appears on a
/// command line, and deletes it when the download is done.
struct TokenFile(PathBuf);

impl TokenFile {
    fn new(token: &str) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!(
            "otd-update-auth-{}-{:?}.txt",
            std::process::id(),
            std::thread::current().id()
        ));
        #[cfg(windows)]
        fs::write(&path, format!("Authorization: Bearer {token}\n"))
            .map_err(|error| format!("cannot prepare the request: {error}"))?;
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            use std::io::Write;
            let mut file=fs::OpenOptions::new().create_new(true).write(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC)
                .open(&path).map_err(|error|format!("cannot prepare private request header: {error}"))?;
            let guard=Self(path);
            file.write_all(format!("Authorization: Bearer {token}\n").as_bytes()).map_err(|error|error.to_string())?;
            return Ok(guard);
        }
        Ok(Self(path))
    }
}

impl Drop for TokenFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Downloads `url` into `output`. `accept` goes to every host; `token` only
/// to the first URL's host, never to the storage host it redirects to.
pub(crate) fn to_file(
    url: &str,
    accept: Option<&str>,
    token: Option<&str>,
    output: &Path,
) -> Result<(), String> {
    to_file_with_cancel(url, accept, token, output, None)
}

pub(crate) fn cancelled(cancel: Option<&AtomicBool>) -> Result<(), String> {
    if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
        Err("operation cancelled because the daemon is shutting down".into())
    } else { Ok(()) }
}

/// Only background-owned helpers use this. Drain both pipes concurrently with
/// bounded retained output, and reap the exact owned child before returning.
/// Cancellation never kills another process or interrupts a replacement journal.
pub(crate) fn run(command: &mut Command, cancel: Option<&AtomicBool>) -> Result<Output, String> {
    use std::io::Read;
    cancelled(cancel)?;
    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .map_err(|error| format!("cannot run download/install helper: {error}"))?;
    let stdout = child.stdout.take().ok_or("helper stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("helper stderr unavailable")?;
    let drain = |mut pipe: Box<dyn Read + Send>| -> Result<Vec<u8>, String> {
        let mut output = Vec::new();
        let mut buffer = [0; 8192];
        let mut exceeded = false;
        loop {
            let length = pipe.read(&mut buffer).map_err(|error| error.to_string())?;
            if length == 0 { break; }
            let keep = length.min((64 * 1024usize).saturating_sub(output.len()));
            output.extend_from_slice(&buffer[..keep]);
            exceeded |= keep != length;
        }
        if exceeded { Err("helper output exceeds 64 KiB".into()) } else { Ok(output) }
    };
    std::thread::scope(|scope| {
        let stdout = scope.spawn(|| drain(Box::new(stdout)));
        let stderr = scope.spawn(|| drain(Box::new(stderr)));
        let deadline = Instant::now() + Duration::from_secs(180);
        let status = loop {
            let failure = cancelled(cancel).err().or_else(||
                (Instant::now() >= deadline).then(|| "download/install helper timed out".to_owned()));
            if let Some(error) = failure {
                let _ = child.kill();
                let _ = child.wait();
                break Err(error);
            }
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(error) => {
                    let _ = child.kill(); let _ = child.wait();
                    break Err(error.to_string());
                }
            }
        };
        let stdout = stdout.join().map_err(|_| "helper stdout reader panicked".to_owned());
        let stderr = stderr.join().map_err(|_| "helper stderr reader panicked".to_owned());
        Ok(Output { status: status?, stdout: stdout??, stderr: stderr?? })
    })
}

pub(crate) fn to_file_with_cancel(
    url: &str, accept: Option<&str>, token: Option<&str>, output: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<(), String> {
    cancelled(cancel)?;
    let origin = host(url).ok_or_else(|| format!("{url} is not an HTTPS address"))?;
    let token = token.map(TokenFile::new).transpose()?;
    let mut current = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        cancelled(cancel)?;
        let here = host(&current)
            .ok_or_else(|| format!("{url} redirects to {current}, which is not HTTPS"))?;
        let proxy = system_proxy(&current, &here);
        let mut command = Command::new(crate::update::system_tool("curl.exe"));
        command.creation_flags(CREATE_NO_WINDOW).args([
            "--silent",
            "--show-error",
            "--fail",
            "--globoff",
            "--max-time",
            "180",
            "--user-agent",
            USER_AGENT,
        ]);
        #[cfg(windows)]
        command.arg("--ssl-no-revoke");
        if let Some(proxy) = &proxy {
            command.arg("--proxy").arg(proxy);
        }
        if let Some(accept) = accept {
            command.arg("--header").arg(format!("Accept: {accept}"));
        }
        if let Some(token) = token.as_ref().filter(|_| here == origin) {
            command
                .arg("--header")
                .arg(format!("@{}", token.0.display()));
        }
        command
            .arg("--output")
            .arg(output)
            .args(["--write-out", "%{http_code} %{redirect_url}"])
            .arg(&current);
        let result = run(&mut command, cancel)
            .map_err(|error| format!("download from {here} failed: {error} ({url})"))?;
        if !result.status.success() {
            let reason = String::from_utf8_lossy(&result.stderr)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let reason = match reason.strip_prefix("curl: ").unwrap_or(&reason) {
                "" => format!("curl.exe exited with {}", result.status),
                reason => reason.to_owned(),
            };
            let through = proxy
                .map(|proxy| format!(" through proxy {proxy}"))
                .unwrap_or_default();
            return Err(format!(
                "download from {here}{through} failed: {reason} ({url})"
            ));
        }
        let written = String::from_utf8_lossy(&result.stdout);
        match written.trim().split_once(' ') {
            Some((status, next)) if status.starts_with('3') && !next.trim().is_empty() => {
                current = next.trim().to_owned();
            }
            _ => return Ok(()),
        }
    }
    Err(format!("{url} redirects more than {MAX_REDIRECTS} times"))
}

/// Downloads `url` into memory, as `to_file` does.
pub(crate) fn to_memory(
    url: &str,
    accept: Option<&str>,
    token: Option<&str>,
) -> Result<Vec<u8>, String> {
    to_memory_with_cancel(url, accept, token, None)
}

pub(crate) fn to_memory_with_cancel(
    url: &str, accept: Option<&str>, token: Option<&str>, cancel: Option<&AtomicBool>,
) -> Result<Vec<u8>, String> {
    cancelled(cancel)?;
    let work = crate::update::temporary_work("otd-download")?;
    let body = work.join("body");
    let result = to_file_with_cancel(url, accept, token, &body, cancel)
        .and_then(|()| fs::read(&body).map_err(|error| format!("{url}: {error}")));
    let _ = fs::remove_dir_all(&work);
    result
}

/// The lowercase host of an `https://` URL; None for anything else.
fn host(url: &str) -> Option<String> {
    let rest = url
        .get(..8)
        .filter(|scheme| scheme.eq_ignore_ascii_case("https://"))
        .map(|_| &url[8..])?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = match authority.strip_prefix('[') {
        Some(address) => address.split(']').next()?,
        None => authority.split(':').next()?,
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// The proxy the current user's Windows settings give `url`, found as .NET's
/// HttpClient finds it: automatic detection or a setup script first, then
/// the manual proxy unless its exceptions cover the host. None is direct.
#[cfg(windows)]
fn system_proxy(url: &str, host: &str) -> Option<String> {
    let mut config = WINHTTP_CURRENT_USER_IE_PROXY_CONFIG::default();
    if unsafe { WinHttpGetIEProxyConfigForCurrentUser(&mut config) } == 0 {
        return None;
    }
    let (script, manual, exceptions) = unsafe {
        (
            take(config.lpszAutoConfigUrl),
            take(config.lpszProxy),
            take(config.lpszProxyBypass),
        )
    };
    let detect = config.fAutoDetect != 0;
    if (detect || script.is_some())
        && let Some(found) = automatic_proxy(url, detect, script.as_deref())
    {
        return found;
    }
    if bypassed(host, exceptions.as_deref().unwrap_or_default()) {
        return None;
    }
    https_proxy(&manual?)
}

/// The proxy from automatic detection or a setup script: `Some(None)` to
/// connect directly, None when neither gave an answer.
#[cfg(windows)]
fn automatic_proxy(url: &str, detect: bool, script: Option<&str>) -> Option<Option<String>> {
    let agent = wide(USER_AGENT);
    let session = unsafe {
        WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_NO_PROXY,
            null(),
            null(),
            0,
        )
    };
    if session.is_null() {
        return None;
    }
    let url = wide(url);
    let script = script.map(wide);
    let mut options = WINHTTP_AUTOPROXY_OPTIONS {
        dwFlags: if detect {
            WINHTTP_AUTOPROXY_AUTO_DETECT
        } else {
            0
        } | if script.is_some() {
            WINHTTP_AUTOPROXY_CONFIG_URL
        } else {
            0
        },
        dwAutoDetectFlags: if detect {
            WINHTTP_AUTO_DETECT_TYPE_DHCP | WINHTTP_AUTO_DETECT_TYPE_DNS_A
        } else {
            0
        },
        lpszAutoConfigUrl: script.as_ref().map_or(null(), |script| script.as_ptr()),
        fAutoLogonIfChallenged: 1,
        ..Default::default()
    };
    let mut info = WINHTTP_PROXY_INFO::default();
    let mut found =
        unsafe { WinHttpGetProxyForUrl(session, url.as_ptr(), &mut options, &mut info) } != 0;
    // Some tuned systems disable the WinHTTP auto-proxy service.
    if !found && unsafe { GetLastError() } == ERROR_WINHTTP_AUTO_PROXY_SERVICE_ERROR {
        options.dwFlags |= WINHTTP_AUTOPROXY_RUN_INPROCESS;
        found =
            unsafe { WinHttpGetProxyForUrl(session, url.as_ptr(), &mut options, &mut info) } != 0;
    }
    unsafe { WinHttpCloseHandle(session) };
    if !found {
        return None;
    }
    let (proxy, _) = unsafe { (take(info.lpszProxy), take(info.lpszProxyBypass)) };
    Some(
        proxy
            .filter(|_| info.dwAccessType == WINHTTP_ACCESS_TYPE_NAMED_PROXY)
            .and_then(|proxy| https_proxy(&proxy)),
    )
}

/// Copies and frees a string WinHTTP allocated; empty strings are None.
unsafe fn take(text: PWSTR) -> Option<String> {
    if text.is_null() {
        return None;
    }
    let mut length = 0;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    unsafe { GlobalFree(text.cast()) };
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// Entries of a Windows proxy or exception list.
#[cfg(unix)]
fn system_proxy(_url:&str,_host:&str)->Option<String>{None}

fn entries(list: &str) -> impl Iterator<Item = &str> {
    list.split([';', ' ', '\t', '\r', '\n'])
        .filter(|entry| !entry.is_empty())
}

/// The entry of a proxy list for HTTPS: `https=host:port`, otherwise the
/// first entry without a scheme, which serves every scheme.
fn https_proxy(list: &str) -> Option<String> {
    let mut any = None;
    for entry in entries(list) {
        match entry.split_once('=') {
            Some((scheme, proxy)) if scheme.eq_ignore_ascii_case("https") => {
                return Some(proxy.to_owned());
            }
            Some(_) => {}
            None => {
                any.get_or_insert(entry);
            }
        }
    }
    any.map(str::to_owned)
}

/// Whether the manual proxy's exception list covers `host`: `<local>`
/// matches names without a dot, and `*` in an entry matches any text.
fn bypassed(host: &str, list: &str) -> bool {
    entries(list).any(|entry| {
        if entry.eq_ignore_ascii_case("<local>") {
            return !host.contains('.');
        }
        let entry = entry.rsplit_once("://").map_or(entry, |(_, rest)| rest);
        wildcard(&entry.to_ascii_lowercase(), host)
    })
}

fn wildcard(pattern: &str, text: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == text,
        Some((prefix, rest)) => text.strip_prefix(prefix).is_some_and(|text| {
            (0..=text.len()).any(|at| text.is_char_boundary(at) && wildcard(rest, &text[at..]))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_cancelled_operations_do_not_spawn_or_create_downloads() {
        let stop = AtomicBool::new(true);
        let missing = Path::new("does-not-exist/cancelled-download");
        assert!(to_file_with_cancel("not an address", None, None, missing, Some(&stop))
            .unwrap_err().contains("cancelled"));
        assert!(run(&mut Command::new("nonexistent-otd-helper"), Some(&stop))
            .unwrap_err().contains("cancelled"));
        assert!(to_memory_with_cancel("not an address", None, None, Some(&stop))
            .unwrap_err().contains("cancelled"));
    }

    #[test]
    fn hosts_come_only_from_https_urls() {
        assert_eq!(
            host("https://Release-Assets.GitHubUserContent.com/a?b=c").as_deref(),
            Some("release-assets.githubusercontent.com")
        );
        assert_eq!(
            host("https://user@example.com:8443/x").as_deref(),
            Some("example.com")
        );
        assert_eq!(host("https://[::1]:8443/x").as_deref(), Some("::1"));
        assert_eq!(host("http://github.com/x"), None);
        assert_eq!(host("https:///x"), None);
    }

    #[test]
    fn https_uses_its_own_proxy_or_the_shared_one() {
        assert_eq!(
            https_proxy("127.0.0.1:7890").as_deref(),
            Some("127.0.0.1:7890")
        );
        assert_eq!(
            https_proxy("http=10.0.0.1:80;https=10.0.0.2:443;ftp=10.0.0.3:21").as_deref(),
            Some("10.0.0.2:443")
        );
        assert_eq!(https_proxy("http=10.0.0.1:80;socks=10.0.0.3:1080"), None);
        assert_eq!(
            https_proxy("http://proxy:8080 backup:3128").as_deref(),
            Some("http://proxy:8080")
        );
    }

    #[test]
    fn exceptions_match_like_windows_settings() {
        let list = "localhost;127.*;*.corp.example;<local>";
        assert!(bypassed("intranet", list));
        assert!(bypassed("127.0.0.1", list));
        assert!(bypassed("files.corp.example", list));
        assert!(!bypassed("corp.example", list));
        assert!(!bypassed("release-assets.githubusercontent.com", list));
        assert!(bypassed("github.com", "https://*github.com"));
        assert!(!bypassed("github.com", ""));
    }

    /// A setup script can send GitHub's storage host through a proxy and the
    /// API directly; each redirect is asked separately.
    #[test]
    fn a_setup_script_picks_the_proxy_per_host() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let script = format!(
            "http://127.0.0.1:{}/proxy.pac",
            listener.local_addr().unwrap().port()
        );
        std::thread::spawn(move || {
            let body = r#"function FindProxyForURL(url, host) {
                return shExpMatch(host, "*.githubusercontent.com") ? "PROXY 127.0.0.1:7890; DIRECT" : "DIRECT";
            }"#;
            for mut stream in listener.incoming().flatten() {
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                while !request.ends_with(b"\r\n\r\n") {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(length) => request.extend_from_slice(&buffer[..length]),
                    }
                }
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/x-ns-proxy-autoconfig\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        assert_eq!(
            automatic_proxy(
                "https://release-assets.githubusercontent.com/a",
                false,
                Some(&script)
            ),
            Some(Some("127.0.0.1:7890".to_owned()))
        );
        assert_eq!(
            automatic_proxy("https://github.com/a", false, Some(&script)),
            Some(None)
        );
    }
}

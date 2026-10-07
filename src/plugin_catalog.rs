//! OpenTabletDriver's plugin catalog and installed plugins, as its plugin
//! manager handles them: the catalog is the Plugin-Repository archive of
//! metadata files; a download is installed only when its SHA-256 matches the
//! catalog's; each plugin lives in its own folder with a copy of its metadata.
//! Plugins install under this driver's data folder, not OpenTabletDriver's.
//!
//! Upstream: OpenTabletDriver.Desktop/Reflection/Metadata/{PluginMetadata,
//! PluginMetadataCollection}.cs and DesktopPluginManager.cs at 736003e.

use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use serde::{Deserialize, Serialize};

const CATALOG: &str = "https://api.github.com/repos/OpenTabletDriver/Plugin-Repository/tarball";
/// The OpenTabletDriver version this driver's plugin support follows.
pub const DRIVER_VERSION: [u64; 4] = [0, 6, 7, 0];

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PluginMetadata {
    pub name: String,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub description: String,
    pub plugin_version: String,
    #[serde(default)]
    pub supported_driver_version: Option<String>,
    #[serde(default)]
    pub max_supported_driver_version: Option<String>,
    #[serde(default)]
    pub repository_url: Option<String>,
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default)]
    pub compression_format: Option<String>,
    #[serde(default, rename = "SHA256")]
    pub sha256: Option<String>,
    #[serde(default)]
    pub wiki_url: Option<String>,
    #[serde(default)]
    pub license_identifier: Option<String>,
}

/// A .NET `Version` as four numbers; missing parts are zero.
pub fn version(text: &str) -> Option<[u64; 4]> {
    let mut parts = [0u64; 4];
    let mut count = 0;
    for (slot, part) in parts.iter_mut().zip(text.trim().split('.')) {
        *slot = part.parse().ok()?;
        if *slot > i32::MAX as u64 { return None; }
        count += 1;
    }
    (count >= 2 && text.trim().split('.').count() <= 4).then_some(parts)
}

// System.Version uses -1 for unspecified build/revision. This distinction
// matters for upper bounds: 0.6.7 is less than the baseline 0.6.7.0.
fn clr_version(text: &str) -> Option<[i64; 4]> {
    version(text)?;
    let mut parts = [-1; 4];
    for (slot, part) in parts.iter_mut().zip(text.trim().split('.')) {
        *slot = part.parse().ok()?;
    }
    Some(parts)
}

impl PluginMetadata {
    pub fn same_identity(&self, other: &Self) -> bool {
        self.name == other.name
            && self.owner == other.owner
            && self.repository_url == other.repository_url
    }

    pub fn version(&self) -> [u64; 4] {
        version(&self.plugin_version).unwrap_or_default()
    }

    /// Whether the plugin declares support for OpenTabletDriver 0.6.7.
    pub fn supports_driver(&self) -> bool {
        let Some(minimum) = self.supported_driver_version.as_deref().and_then(clr_version) else {
            return false;
        };
        let baseline = DRIVER_VERSION.map(|part| part as i64);
        // IsSupportedBy requires the same major/minor and a supported build,
        // while intentionally ignoring the minimum version's revision.
        if minimum[..2] != baseline[..2] || minimum[2] > baseline[2] { return false; }
        match self.max_supported_driver_version.as_deref() {
            None => true,
            Some(maximum) => clr_version(maximum).is_some_and(|maximum| maximum >= baseline),
        }
    }

    /// Plugin folder name: the plugin's name without path characters.
    pub fn folder(&self) -> String {
        self.name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || " -_.()".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
            .trim_matches(['.', ' '])
            .to_owned()
    }
}

fn system_tool(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    PathBuf::from(root).join("System32").join(name)
}

fn extract(archive: &Path, into: &Path) -> Result<(), String> {
    fs::create_dir_all(into).map_err(|error| error.to_string())?;
    let status = Command::new(system_tool("tar.exe"))
        .creation_flags(CREATE_NO_WINDOW)
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .status()
        .map_err(|error| format!("cannot run tar.exe: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cannot extract {}", archive.display()))
    }
}

/// Plugin ZIPs are untrusted archives even when the catalog hash matches.
/// The embedded extractor validates all paths and bounded sizes before writes.
/// Environment arguments preserve arbitrary local paths without shell quoting.
fn extract_plugin(archive: &Path, into: &Path) -> Result<(), String> {
    extract_plugin_with_cancel(archive, into, None)
}
fn extract_plugin_with_cancel(archive: &Path, into: &Path, cancel: Option<&std::sync::atomic::AtomicBool>) -> Result<(), String> {
    let output = crate::download::run(Command::new(system_tool("WindowsPowerShell/v1.0/powershell.exe"))
        .creation_flags(CREATE_NO_WINDOW)
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
        .arg(include_str!("../compat/ExtractPluginArchive.ps1"))
        .env("OTD_PLUGIN_ARCHIVE", archive)
        .env("OTD_PLUGIN_STAGE", into), cancel)
        .map_err(|error| format!("cannot validate plugin ZIP: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("cannot extract plugin ZIP {}: {}", archive.display(),
            String::from_utf8_lossy(&output.stderr).trim()))
    }
}

fn json_files(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            json_files(&path, found);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        {
            found.push(path);
        }
    }
}

/// Every catalog entry that supports OpenTabletDriver 0.6.7, newest version
/// per name, owner and repository identity, sorted by name.
pub fn fetch() -> Result<Vec<PluginMetadata>, String> {
    let work = crate::update::temporary_work("otd-rust-catalog")?;
    let archive = work.join("catalog.tar.gz");
    let result = crate::download::to_file(CATALOG, None, None, &archive)
        .and_then(|()| extract(&archive, &work.join("catalog")))
        .map(|()| read_catalog(&work.join("catalog")));
    let _ = fs::remove_dir_all(&work);
    result
}

fn read_catalog(directory: &Path) -> Vec<PluginMetadata> {
    let mut files = Vec::new();
    json_files(directory, &mut files);
    let mut entries: Vec<PluginMetadata> = files
        .iter()
        .filter_map(|file| fs::read(file).ok())
        .filter_map(|bytes| serde_json::from_slice::<PluginMetadata>(&bytes).ok())
        .filter(|entry| entry.supports_driver() && version(&entry.plugin_version).is_some())
        .collect();
    entries.sort_by(|a, b| {
        (a.name.to_lowercase(), &a.name, &a.owner, &a.repository_url, b.version(), clr_version(&b.plugin_version)).cmp(&(
            b.name.to_lowercase(),
            &b.name,
            &b.owner,
            &b.repository_url,
            a.version(),
            clr_version(&a.plugin_version),
        ))
    });
    entries.dedup_by(|later, first| later.same_identity(first));
    entries
}

/// Runs .NET filters that have a verified native port on that port instead
/// (`Profile::use_native_ports`), so their reports skip the .NET bridge and
/// the runtime is not loaded for them. Set `OTD_DISABLE_NATIVE_PORTS=1` to
/// keep running the DLLs, for example to compare the two.
pub fn use_native_ports(profile: &mut otd_core::config::Profile, log: impl Fn(&str)) {
    if std::env::var_os("OTD_DISABLE_NATIVE_PORTS").is_some_and(|value| value != "0") {
        return;
    }
    let moved = profile.use_native_ports(|path| {
        crate::update::sha256(path).is_ok_and(|hash| hash == otd_core::radial_follow::DLL_SHA256)
    });
    if moved > 0 {
        log(&format!(
            "Running {moved} RadialFollow 0.3.0 filter(s) on the built-in port instead of the .NET bridge."
        ));
    }
}

/// Where plugins are installed: `Plugins` in this driver's data folder.
pub fn plugins_directory() -> Result<PathBuf, String> {
    Ok(otd_core::storage::data_directory()?.join("Plugins"))
}

fn internal_folder(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with(".otd-") || name.ends_with(".old-update")
}

/// Complete interrupted promotion; false means another installer owns it.
/// Callers loading DLLs must defer on false. Inventory itself stays read-only.
pub fn recover_installations() -> Result<bool, String> {
    let root = plugins_directory()?;
    if !root.try_exists().map_err(|error| error.to_string())? { return Ok(true); }
    let Some(_lock) = crate::update::transaction::InstallLock::try_acquire(&root, ".otd-plugins.lock")? else {
        eprintln!("Plugin recovery deferred while another installer owns the directory.");
        return Ok(false);
    };
    recover_locked(&root).map(|()| true)
}

fn recover_locked(root: &Path) -> Result<(), String> {
    for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry.file_type().map_err(|error| error.to_string())?.is_dir() { continue; }
        let name = entry.file_name().to_string_lossy().into_owned();
        // place() creates and consumes root staging only while holding this
        // same install lock. Network download/extraction work lives elsewhere,
        // so a root stage observed here belongs to an interrupted installation.
        if name.starts_with(".otd-plugin-stage-") || name.starts_with(".otd-plugin-removed-") {
            let _ = fs::remove_dir_all(entry.path());
            continue;
        }
        let Some(target_name) = name.strip_suffix(".old-update") else { continue; };
        if target_name.is_empty() || internal_folder(target_name) { continue; }
        let target = root.join(target_name);
        let validated = (|| {
            // Only metadata proves that this is our interrupted promotion.
            let bytes = fs::read(entry.path().join("metadata.json")).map_err(|error| error.to_string())?;
            let metadata: PluginMetadata = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            if !metadata.folder().eq_ignore_ascii_case(target_name) {
                return Err("recovery folder does not match metadata".to_owned());
            }
            Ok(())
        })();
        if let Err(error) = validated {
            // Retain hidden backups for manual repair without preventing
            // unrelated plugins from recovering or being inventoried.
            eprintln!("Plugin {target_name} recovery failed; backup kept at {}: {error}", entry.path().display());
            continue;
        }
        if !target.try_exists().map_err(|error| error.to_string())? {
            fs::rename(entry.path(), &target).map_err(|error| format!("cannot restore plugin {target_name}: {error}"))?;
        } else {
            // A loaded old DLL may keep the backup alive. It stays hidden and
            // another startup retries its cleanup after handles are released.
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    Ok(())
}

/// Installed plugins, from each folder's `metadata.json`.
pub fn installed() -> Result<Vec<(PathBuf, PluginMetadata)>, String> {
    let directory = plugins_directory()?;
    if !directory.try_exists().map_err(|error| error.to_string())? { return Ok(Vec::new()); }
    // Serialize the read with directory promotion, without creating files or
    // doing recovery. A transient rename gap is not an empty installation.
    let _lock = crate::update::transaction::InstallLock::try_acquire(&directory, ".otd-plugins.lock")?
        .ok_or("Plugin inventory is busy while an installation is being changed.")?;
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot list plugins in {}: {error}", directory.display())),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read plugin directory entry: {error}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if internal_folder(&name) || !entry.file_type().map_err(|error| error.to_string())?.is_dir() { continue; }
        let folder = entry.path();
        let bytes = match fs::read(folder.join("metadata.json")) {
            Ok(bytes) => bytes,
            // Manually copied plugin folders need not be catalog installs.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("cannot read plugin metadata in {}: {error}", folder.display())),
        };
        let metadata: PluginMetadata = match serde_json::from_slice(&bytes) {
            Ok(metadata) => metadata,
            Err(error) => {
                eprintln!("Skipping invalid plugin metadata in {}: {error}", folder.display());
                continue;
            }
        };
        if metadata.folder().eq_ignore_ascii_case(&name) { found.push((folder, metadata)); }
    }
    found.sort_by_key(|(_, plugin)| plugin.name.to_lowercase());
    Ok(found)
}

/// Downloads, verifies and installs a catalog entry, replacing an installed
/// version. Returns the plugin folder.
pub fn install(entry: &PluginMetadata) -> Result<PathBuf, String> {
    install_with_cancel(entry, None)
}
pub(crate) fn install_with_cancel(entry: &PluginMetadata, cancel: Option<&std::sync::atomic::AtomicBool>) -> Result<PathBuf, String> {
    crate::download::cancelled(cancel)?;
    let url = entry
        .download_url
        .as_deref()
        .filter(|url| url.starts_with("https://"))
        .ok_or_else(|| format!("{} has no HTTPS download", entry.name))?;
    let work = crate::update::temporary_work("otd-plugin")?;
    fs::create_dir_all(&work).map_err(|error| error.to_string())?;
    let archive = work.join("download");
    let result = crate::download::to_file_with_cancel(url, None, None, &archive, cancel)
        .and_then(|()| {
            crate::download::cancelled(cancel)?;
            install_archive_with_cancel(entry, &archive, &plugins_directory()?, &work, cancel)
        });
    let _ = fs::remove_dir_all(&work);
    result
}

/// Verifies a downloaded archive against the catalog's SHA-256 and installs
/// it as `root/<plugin name>`, moving an installed version aside first: a
/// loaded DLL cannot be deleted, but its folder can be renamed.
fn install_archive(
    entry: &PluginMetadata,
    archive: &Path,
    root: &Path,
    work: &Path,
) -> Result<PathBuf, String> {
    install_archive_with_cancel(entry, archive, root, work, None)
}
fn install_archive_with_cancel(
    entry: &PluginMetadata, archive: &Path, root: &Path, work: &Path,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<PathBuf, String> {
    crate::download::cancelled(cancel)?;
    if !entry.supports_driver() {
        return Err(format!("{} does not support OpenTabletDriver 0.6.7", entry.name));
    }
    let expected = entry
        .sha256
        .as_deref()
        .ok_or_else(|| {
            format!(
                "{} declares no SHA-256, so it cannot be verified",
                entry.name
            )
        })?
        .to_ascii_lowercase();
    let folder_name = entry.folder();
    if folder_name.is_empty() {
        return Err("the plugin has no usable name".into());
    }
    let actual = crate::update::sha256(archive)?;
    if actual != expected {
        return Err(format!(
            "the download's SHA-256 {actual} does not match the catalog's {expected}; nothing was installed"
        ));
    }
    let staged = work.join("plugin");
    let _ = fs::remove_dir_all(&staged);
    match entry
        .compression_format
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("zip") | None => extract_plugin_with_cancel(archive, &staged, cancel)?,
        Some(other) => return Err(format!("unsupported plugin format {other}")),
    }
    if dlls(&staged).is_empty() {
        return Err(format!("{} contains no DLL", archive.display()));
    }
    crate::download::cancelled(cancel)?;
    place(entry, &staged, root)
}

/// Installs a plugin the user picked: a zip archive, as OpenTabletDriver's
/// plugin manager accepts, or a single DLL. There is no catalog hash to
/// check, so the user vouches for the file.
pub fn install_file(file: &Path) -> Result<PathBuf, String> {
    let work = crate::update::temporary_work("otd-plugin")?;
    let result = install_file_into(file, &plugins_directory()?, &work);
    let _ = fs::remove_dir_all(&work);
    result
}

fn install_file_into(file: &Path, root: &Path, work: &Path) -> Result<PathBuf, String> {
    let name = file
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let entry = PluginMetadata {
        name,
        owner: "Local file".into(),
        description: format!("Installed from {}", file.display()),
        plugin_version: "0.0.0.0".into(),
        supported_driver_version: None,
        max_supported_driver_version: None,
        repository_url: None,
        download_url: None,
        compression_format: None,
        sha256: None,
        wiki_url: None,
        license_identifier: None,
    };
    if entry.folder().is_empty() {
        return Err("the file has no usable name".into());
    }
    let staged = work.join("plugin");
    let _ = fs::remove_dir_all(&staged);
    let extension = file
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase());
    match extension.as_deref() {
        Some("zip") => extract_plugin(file, &staged)?,
        Some("dll") => {
            fs::create_dir_all(&staged).map_err(|error| error.to_string())?;
            let copied = staged.join(file.file_name().unwrap_or_default());
            fs::copy(file, copied)
                .map_err(|error| format!("cannot copy {}: {error}", file.display()))?;
        }
        _ => return Err("choose a .zip plugin archive or a .dll".into()),
    }
    if dlls(&staged).is_empty() {
        return Err(format!("{} contains no DLL", file.display()));
    }
    place(&entry, &staged, root)
}

/// Moves a staged plugin folder to `root/<plugin name>` with its metadata.
fn place(entry: &PluginMetadata, staged: &Path, root: &Path) -> Result<PathBuf, String> {
    if entry.folder().is_empty() || internal_folder(&entry.folder()) {
        return Err("plugin name conflicts with a reserved installation folder".into());
    }
    let _lock = crate::update::transaction::InstallLock::acquire(root, ".otd-plugins.lock")?;
    recover_locked(root)?;
    let local = crate::update::unique_directory(root, ".otd-plugin-stage")?;
    let result = (|| {
        copy_tree(staged, &local)?;
        fs::write(
            local.join("metadata.json"),
            serde_json::to_vec_pretty(entry).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let target = root.join(entry.folder());
        let previous = root.join(format!("{}.old-update", entry.folder()));
        if target.exists() {
            let installed: PluginMetadata = serde_json::from_slice(
                &fs::read(target.join("metadata.json")).map_err(|error|
                    format!("cannot establish plugin identity at {}: {error}", target.display()))?
            ).map_err(|error| format!("invalid installed plugin identity: {error}"))?;
            if !entry.same_identity(&installed) {
                return Err(format!("{} is owned by a different plugin identity ({} / {}); nothing was replaced",
                    target.display(), installed.name, installed.owner));
            }
        }
        if previous.exists() {
            return Err(format!("plugin backup remains at {}; close users of the old DLL or repair the backup before reinstalling", previous.display()));
        }
        if target.exists() {
            fs::rename(&target, &previous)
                .map_err(|error| format!("cannot replace {}: {error}", target.display()))?;
        }
        if let Err(error) = fs::rename(&local, &target) {
            if previous.exists() {
                fs::rename(&previous, &target).map_err(|restore| {
                    format!(
                        "install failed: {error}; restoring {} failed: {restore}; backup preserved",
                        target.display()
                    )
                })?;
            }
            return Err(format!("cannot install into {}: {error}", target.display()));
        }
        let _ = fs::remove_dir_all(&previous);
        Ok(target)
    })();
    let _ = fs::remove_dir_all(&local);
    result
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target).map_err(|error| error.to_string())?;
        } else {
            return Err("plugin packages cannot contain filesystem links".into());
        }
    }
    Ok(())
}
/// Removes an installed plugin's folder.
pub fn uninstall(folder: &Path) -> Result<(), String> {
    let root = plugins_directory()?;
    let _lock = crate::update::transaction::InstallLock::acquire(&root, ".otd-plugins.lock")?;
    if folder.parent() != Some(root.as_path())
        || folder.file_name().is_none_or(|name| internal_folder(&name.to_string_lossy())) {
        return Err("only plugins in the plugin folder can be removed".into());
    }
    recover_locked(&root)?;
    let name = folder.file_name().ok_or("plugin folder has no name")?.to_string_lossy();
    let backup = root.join(format!("{name}.old-update"));
    if backup.exists() {
        return Err(format!("plugin backup remains at {}; close users of the old DLL or repair the backup before removal", backup.display()));
    }
    let retired = crate::update::unique_directory(&root, ".otd-plugin-removed")?;
    if let Err(error) = fs::rename(folder, retired.join("payload")) {
        let _ = fs::remove_dir(&retired);
        return Err(format!("cannot remove {}: {error}", folder.display()));
    }
    let _ = fs::remove_dir_all(retired);
    Ok(())
}

/// The DLLs in a plugin folder, for adding its filters to settings.
pub fn dlls(folder: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    fn walk(directory: &Path, found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dll"))
            {
                found.push(path);
            }
        }
    }
    walk(folder, &mut found);
    found.sort();
    found
}

/// `plugins catalog|installed|install NAME|install-file PATH|remove NAME`.
pub fn run(arguments: Vec<String>) -> Result<(), String> {
    let usage = "Usage: plugins catalog | plugins installed | plugins install NAME | plugins install-file PATH | plugins remove NAME";
    let mut arguments = arguments.into_iter();
    let command = arguments.next().ok_or(usage)?;
    let name = arguments.next();
    if arguments.next().is_some() {
        return Err(usage.into());
    }
    let matches = |plugin: &PluginMetadata, name: &str| plugin.name.eq_ignore_ascii_case(name)
        || format!("{}/{}", plugin.owner, plugin.name).eq_ignore_ascii_case(name)
        || plugin.repository_url.as_deref() == Some(name);
    match (command.as_str(), name) {
        ("catalog", None) => {
            let installed = installed()?;
            for plugin in fetch()? {
                let state = installed
                    .iter()
                    .find(|(_, local)| local.same_identity(&plugin))
                    .map_or(String::new(), |(_, local)| {
                        format!(" [installed {}]", local.plugin_version)
                    });
                println!(
                    "{} {} by {}{state}: {}",
                    plugin.name,
                    plugin.plugin_version,
                    plugin.owner,
                    plugin.description.lines().next().unwrap_or_default()
                );
            }
            Ok(())
        }
        ("installed", None) => {
            for (folder, plugin) in installed()? {
                println!(
                    "{} {} ({})",
                    plugin.name,
                    plugin.plugin_version,
                    folder.display()
                );
            }
            Ok(())
        }
        ("install", Some(name)) => {
            let candidates: Vec<_> = fetch()?.into_iter().filter(|plugin| matches(plugin, &name)).collect();
            if candidates.len() > 1 {
                return Err(format!("{name} matches multiple catalog identities; select a unique owner/name or repository URL"));
            }
            let plugin = candidates.into_iter().next().ok_or_else(|| {
                format!("no catalog plugin named {name} supports OpenTabletDriver 0.6.7")
            })?;
            let folder = install(&plugin)?;
            println!(
                "Installed {} {} in {}",
                plugin.name,
                plugin.plugin_version,
                folder.display()
            );
            for dll in dlls(&folder) {
                println!("  {}", dll.display());
            }
            Ok(())
        }
        ("install-file", Some(path)) => {
            let folder = install_file(Path::new(&path))?;
            println!("Installed {path} in {}", folder.display());
            for dll in dlls(&folder) {
                println!("  {}", dll.display());
            }
            Ok(())
        }
        ("remove", Some(name)) => {
            let candidates: Vec<_> = installed()?.into_iter()
                .filter(|(_, plugin)| matches(plugin, &name)).collect();
            if candidates.len() > 1 {
                return Err(format!("{name} matches multiple installed identities; select a unique owner/name or repository URL"));
            }
            let (folder, plugin) = candidates.into_iter().next()
                .ok_or_else(|| format!("no installed plugin is named {name}"))?;
            uninstall(&folder)?;
            println!("Removed {}", plugin.name);
            Ok(())
        }
        _ => Err(usage.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(version: &str, minimum: &str, maximum: Option<&str>) -> PluginMetadata {
        PluginMetadata {
            name: "Radial Follow".into(),
            owner: "AbstractQbit".into(),
            description: String::new(),
            plugin_version: version.into(),
            supported_driver_version: Some(minimum.into()),
            max_supported_driver_version: maximum.map(Into::into),
            repository_url: None,
            download_url: None,
            compression_format: None,
            sha256: None,
            wiki_url: None,
            license_identifier: None,
        }
    }

    #[test]
    fn driver_support_follows_the_declared_range() {
        assert!(entry("0.3.0.0", "0.6.0.0", None).supports_driver());
        assert!(!entry("0.3.0.0", "0.7.0.0", None).supports_driver());
        assert!(!entry("0.3.0.0", "0.5.0.0", Some("0.6.6.0")).supports_driver());
        assert!(!entry("0.3.0.0", "0.5.0.0", Some("0.6.7.0")).supports_driver());
        assert!(entry("0.3.0.0", "0.6.7.99", None).supports_driver());
        assert!(!entry("0.3.0.0", "0.6.0.0", Some("0.6.7")).supports_driver());
        assert!(!entry("0.3.0.0", "bad", None).supports_driver());
        assert!(!entry("0.3.0.0", "0.6.0.0", Some("bad")).supports_driver());
        assert_eq!(version("0.6.7"), Some([0, 6, 7, 0]));
        assert_eq!(version("x"), None);
    }

    #[test]
    fn the_catalog_keeps_the_newest_supported_version() {
        let root = std::env::temp_dir().join(format!("otd-catalog-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("a/b")).unwrap();
        for (file, value) in [
            ("a/one.json", entry("0.2.0.0", "0.6.0.0", None)),
            ("a/b/two.json", entry("0.3.0.0", "0.6.0.0", None)),
            ("a/three.json", entry("0.4.0.0", "0.7.0.0", None)),
        ] {
            fs::write(root.join(file), serde_json::to_vec(&value).unwrap()).unwrap();
        }
        let entries = read_catalog(&root);
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].plugin_version, "0.3.0.0");
    }

    #[test]
    fn archives_install_only_with_the_catalog_hash() {
        let root = std::env::temp_dir().join(format!("otd-plugin-install-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (source, work, plugins) =
            (root.join("source"), root.join("work"), root.join("Plugins"));
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&work).unwrap();
        fs::write(source.join("Filter.dll"), b"dll").unwrap();
        let archive = root.join("plugin.zip");
        let status = Command::new(system_tool("tar.exe"))
            .creation_flags(CREATE_NO_WINDOW)
            .arg("-a")
            .arg("-cf")
            .arg(&archive)
            .arg("-C")
            .arg(&source)
            .arg("Filter.dll")
            .status()
            .unwrap();
        assert!(status.success());
        let mut plugin = entry("1.0.0.0", "0.6.0.0", None);
        plugin.compression_format = Some("zip".into());
        plugin.sha256 = Some("00".repeat(32));
        assert!(install_archive(&plugin, &archive, &plugins, &work).is_err());
        assert!(!plugins.join(plugin.folder()).exists());
        plugin.sha256 = Some(crate::update::sha256(&archive).unwrap());
        let folder = install_archive(&plugin, &archive, &plugins, &work).unwrap();
        assert_eq!(dlls(&folder), [folder.join("Filter.dll")]);
        let saved: PluginMetadata =
            serde_json::from_slice(&fs::read(folder.join("metadata.json")).unwrap()).unwrap();
        assert_eq!(saved, plugin);
        // Installing again replaces the folder.
        install_archive(&plugin, &archive, &plugins, &work).unwrap();
        assert!(
            !plugins
                .join(format!("{}.old-update", plugin.folder()))
                .exists()
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn local_files_install_as_zip_or_dll() {
        let root = std::env::temp_dir().join(format!("otd-plugin-file-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (work, plugins) = (root.join("work"), root.join("Plugins"));
        fs::create_dir_all(&root).unwrap();
        let dll = root.join("MyFilter.dll");
        fs::write(&dll, b"dll").unwrap();
        let folder = install_file_into(&dll, &plugins, &work).unwrap();
        assert_eq!(folder, plugins.join("MyFilter"));
        assert_eq!(dlls(&folder), [folder.join("MyFilter.dll")]);
        let saved: PluginMetadata =
            serde_json::from_slice(&fs::read(folder.join("metadata.json")).unwrap()).unwrap();
        assert_eq!(saved.owner, "Local file");
        let archive = root.join("Packed.zip");
        let status = Command::new(system_tool("tar.exe"))
            .creation_flags(CREATE_NO_WINDOW)
            .arg("-a")
            .arg("-cf")
            .arg(&archive)
            .arg("-C")
            .arg(&root)
            .arg("MyFilter.dll")
            .status()
            .unwrap();
        assert!(status.success());
        let folder = install_file_into(&archive, &plugins, &work).unwrap();
        assert_eq!(dlls(&folder), [plugins.join("Packed").join("MyFilter.dll")]);
        let text = root.join("notes.txt");
        fs::write(&text, b"x").unwrap();
        assert!(install_file_into(&text, &plugins, &work).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn plugin_staging_can_live_on_a_different_volume() {
        // Set this to a scratch directory on a second drive for the actual
        // cross-volume check. Without it, still check copy/replace behavior.
        let source_root = std::env::var_os("OTD_TEST_PLUGIN_SOURCE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let source = crate::update::unique_directory(&source_root, "otd-cross-source").unwrap();
        let destination = crate::update::temporary_work("otd-cross-destination").unwrap();
        fs::create_dir(source.join("nested")).unwrap();
        fs::write(source.join("nested/Filter.dll"), b"fixture only").unwrap();
        let plugin = entry("1.0.0.0", "0.6.0.0", None);
        let installed = place(&plugin, &source, &destination).unwrap();
        assert_eq!(
            fs::read(installed.join("nested/Filter.dll")).unwrap(),
            b"fixture only"
        );
        assert_eq!(
            fs::read(source.join("nested/Filter.dll")).unwrap(),
            b"fixture only"
        );
        fs::remove_dir_all(&source).unwrap();
        fs::remove_dir_all(&destination).unwrap();
    }

    #[test]
    fn folder_names_have_no_path_characters() {
        let mut plugin = entry("1.0.0.0", "0.6.0.0", None);
        plugin.name = "..\\Evil/Plugin:1".into();
        assert_eq!(plugin.folder(), "_Evil_Plugin_1");
    }

    fn zip_entries(path: &Path, names: &[&str]) {
        let script = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::Open($env:OTD_TEST_ZIP, [IO.Compression.ZipArchiveMode]::Create)
try {
    foreach ($name in (ConvertFrom-Json $env:OTD_TEST_NAMES)) {
        $entry = $zip.CreateEntry($name)
        if (-not $name.EndsWith('/')) {
            $stream = $entry.Open()
            try { $bytes = [Text.Encoding]::UTF8.GetBytes('fixture'); $stream.Write($bytes, 0, $bytes.Length) }
            finally { $stream.Dispose() }
        }
    }
} finally { $zip.Dispose() }
"#;
        let output = Command::new(system_tool("WindowsPowerShell/v1.0/powershell.exe"))
            .creation_flags(CREATE_NO_WINDOW)
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script])
            .env("OTD_TEST_ZIP", path)
            .env("OTD_TEST_NAMES", serde_json::to_string(names).unwrap())
            .output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }

    #[test]
    fn plugin_zip_validation_precedes_all_extraction() {
        let root = crate::update::unique_directory(Path::new("E:/AgentWork/tmp"), "otd-plugin-zip-contract").unwrap();
        let cases: &[&[&str]] = &[
            &["Filter.dll", "../outside.dll"],
            &["Filter.dll", "/absolute.dll"],
            &["Filter.dll", "C:/drive.dll"],
            &["Filter.dll", "Filter.dll:stream"],
            &["Filter.dll", "filter.DLL"],
            &["Filter.dll", "CON.dll"],
            &["Filter.dll", "folder./bad.dll"],
            &["Filter.dll", "Filter.dll/child.dll"],
        ];
        for (index, names) in cases.iter().enumerate() {
            let archive = root.join(format!("invalid-{index}.zip"));
            let stage = root.join(format!("stage-{index}"));
            zip_entries(&archive, names);
            assert!(extract_plugin(&archive, &stage).is_err(), "accepted {names:?}");
            assert!(!stage.exists(), "wrote an entry before complete validation");
        }
        let archive = root.join("valid.zip");
        zip_entries(&archive, &["nested/", "nested/Filter.dll"]);
        let stage = root.join("valid");
        extract_plugin(&archive, &stage).unwrap();
        assert_eq!(fs::read(stage.join("nested/Filter.dll")).unwrap(), b"fixture");
        let oversized = root.join("oversized.zip");
        zip_entries(&oversized, &["Filter.dll"]);
        let mut bytes = fs::read(&oversized).unwrap();
        let central = bytes.windows(4).position(|bytes| bytes == [0x50, 0x4b, 0x01, 0x02]).unwrap();
        bytes[central + 24..central + 28].copy_from_slice(&67_108_865u32.to_le_bytes());
        fs::write(&oversized, bytes).unwrap();
        let rejected = root.join("oversized-stage");
        assert!(extract_plugin(&oversized, &rejected).is_err());
        assert!(!rejected.exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn distinct_plugin_identities_cannot_replace_each_other() {
        let root = crate::update::unique_directory(Path::new("E:/AgentWork/tmp"), "otd-plugin-identity-contract").unwrap();
        let staged = root.join("source");
        let plugins = root.join("plugins");
        fs::create_dir_all(&staged).unwrap();
        fs::write(staged.join("Filter.dll"), b"original").unwrap();
        let first = entry("1.0.0.0", "0.6.0.0", None);
        let target = place(&first, &staged, &plugins).unwrap();
        fs::write(staged.join("Filter.dll"), b"replacement").unwrap();
        let mut other = first.clone();
        other.owner = "Different owner".into();
        assert!(place(&other, &staged, &plugins).is_err());
        assert_eq!(fs::read(target.join("Filter.dll")).unwrap(), b"original");
        other = first.clone();
        other.repository_url = Some("https://example.test/different-repository".into());
        assert!(place(&other, &staged, &plugins).is_err());
        let mut upgrade = first.clone();
        upgrade.plugin_version = "1.1.0.0".into();
        place(&upgrade, &staged, &plugins).unwrap();
        assert_eq!(fs::read(target.join("Filter.dll")).unwrap(), b"replacement");
        fs::remove_dir_all(root).unwrap();
    }
}

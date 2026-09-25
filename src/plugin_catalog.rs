//! OpenTabletDriver's plugin catalog and installed plugins, as its plugin
//! manager handles them: the catalog is the Plugin-Repository archive of
//! metadata files; a download is installed only when its SHA-256 matches the
//! catalog's; each plugin lives in its own folder with a copy of its metadata.
//! Plugins install under this driver's data folder, not OpenTabletDriver's.
//!
//! Upstream: OpenTabletDriver.Desktop/Reflection/Metadata/{PluginMetadata,
//! PluginMetadataCollection}.cs and DesktopPluginManager.cs at 736003e.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
        count += 1;
    }
    (count >= 2 && text.trim().split('.').count() <= 4).then_some(parts)
}

impl PluginMetadata {
    pub fn version(&self) -> [u64; 4] {
        version(&self.plugin_version).unwrap_or_default()
    }

    /// Whether the plugin declares support for OpenTabletDriver 0.6.7.
    pub fn supports_driver(&self) -> bool {
        let minimum = self
            .supported_driver_version
            .as_deref()
            .and_then(version)
            .unwrap_or_default();
        let maximum = self
            .max_supported_driver_version
            .as_deref()
            .and_then(version);
        minimum <= DRIVER_VERSION && maximum.is_none_or(|maximum| maximum >= DRIVER_VERSION)
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

fn curl(url: &str, output: &Path) -> Result<(), String> {
    let status = Command::new(system_tool("curl.exe"))
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--location",
            "--max-time",
            "180",
        ])
        .args(["--user-agent", "OpenTabletDriver"])
        .arg("--output")
        .arg(output)
        .arg(url)
        .status()
        .map_err(|error| format!("cannot run curl.exe: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("download failed: {url}"))
    }
}

fn extract(archive: &Path, into: &Path) -> Result<(), String> {
    fs::create_dir_all(into).map_err(|error| error.to_string())?;
    let status = Command::new(system_tool("tar.exe"))
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
/// per name and owner, sorted by name.
pub fn fetch() -> Result<Vec<PluginMetadata>, String> {
    let work = std::env::temp_dir().join(format!("otd-rust-catalog-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|error| error.to_string())?;
    let archive = work.join("catalog.tar.gz");
    let result = curl(CATALOG, &archive)
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
        (a.name.to_lowercase(), &a.owner, b.version()).cmp(&(
            b.name.to_lowercase(),
            &b.owner,
            a.version(),
        ))
    });
    entries.dedup_by(|later, first| later.name == first.name && later.owner == first.owner);
    entries
}

/// Where plugins are installed: `Plugins` in this driver's data folder.
pub fn plugins_directory() -> Result<PathBuf, String> {
    Ok(otd_core::storage::data_directory()?.join("Plugins"))
}

/// Installed plugins, from each folder's `metadata.json`.
pub fn installed() -> Vec<(PathBuf, PluginMetadata)> {
    let Ok(directory) = plugins_directory() else {
        return Vec::new();
    };
    let mut found: Vec<(PathBuf, PluginMetadata)> = fs::read_dir(&directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let folder = entry.path();
            let bytes = fs::read(folder.join("metadata.json")).ok()?;
            Some((folder, serde_json::from_slice(&bytes).ok()?))
        })
        .collect();
    found.sort_by_key(|(_, plugin)| plugin.name.to_lowercase());
    found
}

/// Downloads, verifies and installs a catalog entry, replacing an installed
/// version. Returns the plugin folder.
pub fn install(entry: &PluginMetadata) -> Result<PathBuf, String> {
    let url = entry
        .download_url
        .as_deref()
        .filter(|url| url.starts_with("https://"))
        .ok_or_else(|| format!("{} has no HTTPS download", entry.name))?;
    let work = std::env::temp_dir().join(format!("otd-rust-plugin-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|error| error.to_string())?;
    let archive = work.join("download");
    let result = curl(url, &archive)
        .and_then(|()| install_archive(entry, &archive, &plugins_directory()?, &work));
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
        Some("zip") | None => extract(archive, &staged)?,
        Some(other) => return Err(format!("unsupported plugin format {other}")),
    }
    fs::write(
        staged.join("metadata.json"),
        serde_json::to_vec_pretty(entry).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let target = root.join(&folder_name);
    let previous = root.join(format!("{folder_name}.old-update"));
    let _ = fs::remove_dir_all(&previous);
    if target.exists() {
        fs::rename(&target, &previous)
            .map_err(|error| format!("cannot replace {}: {error}", target.display()))?;
    }
    if let Err(error) = fs::rename(&staged, &target) {
        if previous.exists() {
            let _ = fs::rename(&previous, &target);
        }
        return Err(format!("cannot install into {}: {error}", target.display()));
    }
    let _ = fs::remove_dir_all(&previous);
    Ok(target)
}

/// Removes an installed plugin's folder.
pub fn uninstall(folder: &Path) -> Result<(), String> {
    let root = plugins_directory()?;
    if folder.parent() != Some(root.as_path()) {
        return Err("only plugins in the plugin folder can be removed".into());
    }
    fs::remove_dir_all(folder).map_err(|error| {
        format!(
            "cannot remove {}: {error}. Stop the driver if it uses this plugin, then try again.",
            folder.display()
        )
    })
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

/// `plugins catalog|installed|install NAME|remove NAME`.
pub fn run(arguments: Vec<String>) -> Result<(), String> {
    let usage =
        "Usage: plugins catalog | plugins installed | plugins install NAME | plugins remove NAME";
    let mut arguments = arguments.into_iter();
    let command = arguments.next().ok_or(usage)?;
    let name = arguments.next();
    if arguments.next().is_some() {
        return Err(usage.into());
    }
    let matches = |plugin: &PluginMetadata, name: &str| plugin.name.eq_ignore_ascii_case(name);
    match (command.as_str(), name) {
        ("catalog", None) => {
            let installed = installed();
            for plugin in fetch()? {
                let state = installed
                    .iter()
                    .find(|(_, local)| local.name == plugin.name && local.owner == plugin.owner)
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
            for (folder, plugin) in installed() {
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
            let plugin = fetch()?
                .into_iter()
                .find(|plugin| matches(plugin, &name))
                .ok_or_else(|| {
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
        ("remove", Some(name)) => {
            let (folder, plugin) = installed()
                .into_iter()
                .find(|(_, plugin)| matches(plugin, &name))
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
        assert!(entry("0.3.0.0", "0.5.0.0", Some("0.6.7.0")).supports_driver());
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
    fn folder_names_have_no_path_characters() {
        let mut plugin = entry("1.0.0.0", "0.6.0.0", None);
        plugin.name = "..\\Evil/Plugin:1".into();
        assert_eq!(plugin.folder(), "_Evil_Plugin_1");
    }
}

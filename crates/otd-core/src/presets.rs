//! Named native profiles with explicit creation/replacement and no activation.
//! Names keep their spelling. Case-only aliases are rejected on every platform.
use crate::config::Profile;
use crate::storage::{self, FileSnapshot, SaveMode};
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresetName(String);

impl PresetName {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value.is_empty()
            || value.len() > 64
            || value.trim() != value
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'-' | b'_' | b'(' | b')')
            })
        {
            return Err("preset names require 1-64 ASCII letters/digits, internal spaces, hyphens, underscores or parentheses; no leading/trailing spaces or path characters".into());
        }
        let upper = value.to_ascii_uppercase();
        if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ((upper.starts_with("COM") || upper.starts_with("LPT"))
                && upper.len() == 4
                && matches!(upper.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(format!("{value:?} is a reserved Windows device name"));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub struct LoadedPreset {
    name: PresetName,
    profile: Profile,
    snapshot: FileSnapshot,
}

impl LoadedPreset {
    pub fn name(&self) -> &str {
        self.name.as_str()
    }
    pub fn profile(&self) -> &Profile {
        &self.profile
    }
    pub fn path(&self) -> &Path {
        self.snapshot.path()
    }
}

#[derive(Serialize)]
pub struct PresetSummary {
    pub name: String,
    pub settings_revision: Option<u64>,
    pub error: Option<String>,
}

#[derive(Default, Serialize)]
pub struct PresetListing {
    pub presets: Vec<PresetSummary>,
    pub warnings: Vec<String>,
}

struct Entry {
    name: PresetName,
    path: PathBuf,
}

pub struct PresetStore {
    directory: PathBuf,
}

impl PresetStore {
    /// Select storage without creating directories or reading any preset.
    pub fn user() -> Result<Self, String> {
        Self::at(storage::data_directory()?.join("presets"))
    }

    pub fn at(directory: PathBuf) -> Result<Self, String> {
        Ok(Self {
            directory: std::path::absolute(directory).map_err(|error| error.to_string())?,
        })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn inventory(&self) -> Result<(Vec<Entry>, Vec<String>), String> {
        let files = match fs::read_dir(&self.directory) {
            Ok(files) => files,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Vec::new(), Vec::new()));
            }
            Err(error) => return Err(format!("cannot list presets: {error}")),
        };
        let mut entries = Vec::new();
        let mut warnings = Vec::new();
        for file in files {
            let file = file.map_err(|error| format!("cannot inspect preset directory: {error}"))?;
            let path = file.path();
            if !path
                .extension()
                .and_then(|part| part.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
            {
                continue; // Reserved backups, locks and temporary files are not presets.
            }
            let Some(stem) = path.file_stem().and_then(|part| part.to_str()) else {
                warnings.push(format!(
                    "ignored non-UTF-8 preset filename: {}",
                    path.display()
                ));
                continue;
            };
            match PresetName::parse(stem) {
                Ok(name) => entries.push(Entry { name, path }),
                Err(error) => warnings.push(format!("ignored {}: {error}", path.display())),
            }
        }
        entries.sort_by_cached_key(|entry| (entry.name.0.to_ascii_lowercase(), entry.path.clone()));
        warnings.sort();
        Ok((entries, warnings))
    }

    fn resolve(&self, name: &PresetName) -> Result<Option<PathBuf>, String> {
        let (entries, _) = self.inventory()?;
        let matches: Vec<_> = entries
            .iter()
            .filter(|entry| entry.name.0.eq_ignore_ascii_case(name.as_str()))
            .collect();
        match matches.as_slice() {
            [] => Ok(None),
            [entry] if entry.name == *name => Ok(Some(entry.path.clone())),
            [entry] => Err(format!(
                "preset name differs only by case; use the stored spelling {:?}",
                entry.name.as_str()
            )),
            _ => Err(format!(
                "preset name {:?} is ambiguous: multiple files differ only by case; resolve the conflicting files before continuing",
                name.as_str()
            )),
        }
    }

    fn read_at(name: PresetName, path: &Path) -> Result<LoadedPreset, String> {
        let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        if !metadata.file_type().is_file() {
            return Err("preset must be a regular file, not a directory or symbolic link".into());
        }
        let loaded = storage::read_utf8(path)?;
        let profile = Profile::from_toml_text(&loaded.text, loaded.snapshot.path())?;
        Ok(LoadedPreset {
            name,
            profile,
            snapshot: loaded.snapshot,
        })
    }

    pub fn list(&self) -> Result<PresetListing, String> {
        let (entries, warnings) = self.inventory()?;
        let mut listing = PresetListing {
            presets: Vec::new(),
            warnings,
        };
        for (index, entry) in entries.iter().enumerate() {
            // inventory groups case-equivalent names, so neighboring entries
            // detect ambiguity without rescanning every profile for each row.
            let same_name = |other: &Entry| other.name.0.eq_ignore_ascii_case(entry.name.as_str());
            let ambiguous = index
                .checked_sub(1)
                .and_then(|index| entries.get(index))
                .is_some_and(same_name)
                || entries.get(index + 1).is_some_and(same_name);
            let result = if ambiguous {
                Err("multiple preset files differ only by case".into())
            } else {
                Self::read_at(entry.name.clone(), &entry.path)
            };
            let (settings_revision, error) = match result {
                Ok(preset) => (Some(preset.profile.settings_revision), None),
                Err(error) => (None, Some(error)),
            };
            listing.presets.push(PresetSummary {
                name: entry.name.0.clone(),
                settings_revision,
                error,
            });
        }
        Ok(listing)
    }

    pub fn load(&self, name: &PresetName) -> Result<LoadedPreset, String> {
        let path = self
            .resolve(name)?
            .ok_or_else(|| format!("no preset named {:?}", name.as_str()))?;
        Self::read_at(name.clone(), &path)
    }

    /// `None` creates a new name. Replacement requires the exact loaded preset
    /// snapshot; edits since that load fail rather than overwriting the file.
    /// Profile plugin paths must already resolve against their source location.
    pub fn save(
        &self,
        name: &PresetName,
        profile: &Profile,
        previous: Option<&LoadedPreset>,
    ) -> Result<LoadedPreset, String> {
        if profile
            .plugins
            .iter()
            .any(|plugin| !plugin.path.is_absolute())
        {
            return Err("preset save requires resolved plugin paths; load the source profile using its absolute path first".into());
        }
        fs::create_dir_all(&self.directory)
            .map_err(|error| format!("cannot create preset directory: {error}"))?;
        let _lock = DirectoryLock::acquire(&self.directory)?;
        let existing = self.resolve(name)?;
        let (path, mode) = match (existing, previous) {
            (Some(_), None) => {
                return Err(format!(
                    "preset {:?} already exists; replacement requires an explicit loaded snapshot",
                    name.as_str()
                ));
            }
            (None, Some(_)) => {
                return Err(
                    "the preset was removed since it was loaded; no replacement was written".into(),
                );
            }
            (None, None) => (
                self.directory.join(format!("{}.toml", name.as_str())),
                SaveMode::CreateNew,
            ),
            (Some(path), Some(previous)) => {
                if previous.name != *name || !previous.snapshot.matches_path(&path)? {
                    return Err(
                        "replacement snapshot belongs to another preset or directory".into(),
                    );
                }
                (path, SaveMode::Replace(&previous.snapshot))
            }
        };
        let mut profile = profile.clone();
        if let Some(previous) = previous {
            profile.settings_revision = profile
                .settings_revision
                .max(previous.profile.settings_revision);
        }
        profile.advance_revision()?;
        let text = profile.to_toml_at(&path)?;
        // Validate the stored form before touching the existing primary/backup.
        let profile = Profile::from_toml_text(&text, &path)?;
        let snapshot = storage::save(&path, text.as_bytes(), mode)?;
        Ok(LoadedPreset {
            name: name.clone(),
            profile,
            snapshot,
        })
    }

    pub fn export(&self, name: &PresetName, output: &Path) -> Result<FileSnapshot, String> {
        let loaded = self.load(name)?;
        let output = std::path::absolute(output).map_err(|error| error.to_string())?;
        let directory = self
            .directory
            .canonicalize()
            .map_err(|error| error.to_string())?;
        if let Some(parent) = output.parent() {
            match parent.canonicalize() {
                Ok(parent) if parent == directory => return Err(
                    "export cannot create files in the preset directory; use presets save to enforce name and collision rules".into(),
                ),
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("cannot resolve export directory: {error}")),
            }
        }
        storage::save(
            &output,
            loaded.profile.to_toml_at(&output)?.as_bytes(),
            SaveMode::CreateNew,
        )
    }
}

/// A shared lock across preset names makes case-collision checks consistent
/// between cooperating creators even on case-sensitive filesystems.
struct DirectoryLock {
    path: PathBuf,
    file: Option<File>,
}

impl DirectoryLock {
    fn acquire(directory: &Path) -> Result<Self, String> {
        let path = directory.join(".presets.lock");
        let file = OpenOptions::new().write(true).create_new(true).open(&path)
            .map_err(|error| format!("cannot acquire preset lock {}: {error}; remove a stale lock only after confirming no preset save is running", path.display()))?;
        let mut guard = Self {
            path,
            file: Some(file),
        };
        writeln!(
            guard.file.as_mut().expect("lock file exists"),
            "pid={}",
            std::process::id()
        )
        .map_err(|error| format!("cannot initialize preset lock: {error}"))?;
        Ok(guard)
    }
}

impl Drop for DirectoryLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

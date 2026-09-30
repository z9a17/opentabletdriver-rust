//! Background inspection and device discovery, with UI-owned completions.
use super::*;

impl App {
    pub(super) fn background(
        &mut self,
        name: &str,
        work: impl FnOnce() -> BackgroundResult + Send + 'static,
    ) -> bool {
        let sender = self.background_tx.clone();
        let window = self.hwnd as isize;
        match std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                if sender.send(work()).is_ok() {
                    unsafe {
                        PostMessageW(window as HWND, WM_BACKGROUND, 0, 0);
                    }
                }
            }) {
            Ok(_) => true,
            Err(error) => {
                self.log(Level::Error, "Background work", error.to_string());
                false
            }
        }
    }

    pub(super) fn inspect_plugin(&mut self, path: PathBuf, add: bool) {
        if let Some(pending_add) = self.metadata_pending.get_mut(&path) {
            *pending_add |= add;
            return;
        }
        self.metadata_pending.insert(path.clone(), add);
        let revision = self.metadata_versions.entry(path.clone()).or_default();
        *revision = revision.wrapping_add(1);
        let revision = *revision;
        let generation = self.metadata_generation;
        let inspection_path = path.clone();
        let mut paths: Vec<_> = if add {
            self.editor.profile.plugins.iter().map(|plugin| plugin.path.clone()).collect()
        } else {
            vec![path.clone()]
        };
        if add { paths.push(path.clone()); }
        if !self.background("plugin-inspection", move || BackgroundResult::Metadata {
            generation,
            revision,
            result: crate::dotnet::inspect_details(&inspection_path),
            path: inspection_path,
            aliases: path_aliases(paths),
        }) {
            self.metadata_pending.remove(&path);
        }
    }

    pub(super) fn background_results(&mut self) -> Option<String> {
        // A confirmed update restart freezes the editor until it succeeds or
        // fails; keep profile-changing completions queued during that interval.
        if self.closing || self.update_restart_pending { return None; }
        let mut dialog = None;
        while let Ok(event) = self.background_rx.try_recv() {
            match event {
                BackgroundResult::Metadata {
                    generation,
                    revision,
                    path,
                    result,
                    aliases,
                } => {
                    if generation != self.metadata_generation || self.metadata_versions.get(&path) != Some(&revision) {
                        continue;
                    }
                    let add = self.metadata_pending.remove(&path).unwrap_or(false);
                    match result {
                        Ok(entries) => {
                            let metadata = Ok(entries.iter().map(|entry| entry.metadata.clone()).collect());
                            self.plugin_metadata.insert(
                                path.clone(),
                                metadata.clone(),
                            );
                            let inspected = canonical_alias(&path, &aliases);
                            for (original, canonical) in &aliases {
                                if canonical == inspected {
                                    self.plugin_metadata.insert(original.clone(), metadata.clone());
                                }
                            }
                            if add {
                                if entries.is_empty() {
                                    self.metadata_changed();
                                    self.log(
                                        Level::Warning,
                                        "Plugins",
                                        "This assembly exports no supported filters or tools.",
                                    );
                                } else {
                                    self.complete_plugin_add(
                                        entries
                                            .into_iter()
                                            .map(|entry| PluginConfig { path: path.clone(), ..entry.config })
                                            .collect(),
                                        &aliases,
                                        true,
                                    );
                                }
                            } else {
                                let selected = self.selected_target().is_some_and(|target|
                                    matches!(target, FilterRef::Plugin(index) if self.editor.profile.plugins.get(index).is_some_and(|plugin| plugin.path == path)));
                                if selected {
                                    self.metadata_changed();
                                } else {
                                    self.refresh_filter_list();
                                }
                            }
                        }
                        Err(error) => {
                            self.plugin_metadata
                                .insert(path.clone(), Err(error.clone()));
                            self.metadata_changed();
                            self.log(
                                Level::Error,
                                "Plugins",
                                format!("Could not inspect {}: {error}", path.display()),
                            );
                        }
                    }
                }
                BackgroundResult::PluginFolder {
                    generation,
                    revision,
                    select_added,
                    folder,
                    name,
                    entries,
                    errors,
                    aliases,
                } => {
                    if generation != self.metadata_generation || self.metadata_versions.get(&folder) != Some(&revision) {
                        continue;
                    }
                    self.metadata_pending.remove(&folder);
                    if !errors.is_empty() {
                        self.log(
                            Level::Warning,
                            "Plugins",
                            format!(
                                "Some DLLs in {name} could not be inspected:\n{}",
                                errors.join("\n")
                            ),
                        );
                    }
                    if entries.is_empty() && errors.is_empty() {
                        self.log(Level::Warning, "Plugins", format!("{name} exports no supported filters or tools; it may provide bindings or output modes."));
                    }
                    let mut configs = Vec::new();
                    if !entries.is_empty() {
                        let mut metadata_by_path: HashMap<PathBuf, Vec<FilterMetadata>> = HashMap::new();
                        for entry in entries {
                            metadata_by_path.entry(entry.config.path.clone()).or_default().push(entry.metadata);
                            configs.push(entry.config);
                        }
                        for (path, metadata) in metadata_by_path {
                            let inspected = canonical_alias(&path, &aliases);
                            for (original, canonical) in &aliases {
                                if canonical == inspected {
                                    self.plugin_metadata.insert(original.clone(), Ok(metadata.clone()));
                                }
                            }
                            self.plugin_metadata.insert(path, Ok(metadata));
                        }
                    }
                    // A removed DLL or failed inspection must not leave the old
                    // property controls visible or trigger another scan each refresh.
                    for plugin in &self.editor.profile.plugins {
                        if within_folder(&plugin.path, &folder) && !self.plugin_metadata.contains_key(&plugin.path) {
                            let error = if errors.is_empty() {
                                "The installed plugin no longer exports this assembly's filters or tools.".to_owned()
                            } else {
                                format!("Could not inspect the updated plugin:\n{}", errors.join("\n"))
                            };
                            self.plugin_metadata.insert(plugin.path.clone(), Err(error));
                        }
                    }
                    if configs.is_empty() {
                        self.metadata_changed();
                    } else {
                        self.complete_plugin_add(configs, &aliases, select_added);
                    }
                }
                BackgroundResult::Devices { announce, result } => {
                    self.device_scan_pending = false;
                    match result {
                        Ok(pens) => {
                            self.tablet_present = Some(!pens.is_empty());
                            if announce && pens.is_empty() {
                                self.log(
                                    Level::Warning,
                                    "Tablet",
                                    "No supported tablet was found.",
                                );
                            } else if announce {
                                self.log(
                                    Level::Info,
                                    "Tablet",
                                    format!("Found {}.", pens.join(", ")),
                                );
                            }
                            self.connected_tablets = pens;
                            // Detection updates labels immediately, but never
                            // replaces unsaved or invalid editor values.
                            if !self.dirty && self.invalid.is_empty() && !self.editing_controls() {
                                match self.editor.update_detected_tablet(&self.connected_tablets) {
                                    Ok(true) => {
                                        self.sync_areas(None);
                                        self.sync_pen(None);
                                        self.layout();
                                    }
                                    Ok(false) => {}
                                    Err(error) => self.log(Level::Warning, "Tablet", format!("Could not use the detected tablet's dimensions: {error}")),
                                }
                            }
                            self.update_title();
                            self.set_driver_state(self.driver);
                        }
                        Err(error) => self.log(
                            Level::Error,
                            "Tablet",
                            format!("HID discovery failed: {error}"),
                        ),
                    }
                }
                BackgroundResult::Import {
                    generation,
                    edit_revision,
                    result,
                } => {
                    if generation != self.metadata_generation {
                        continue;
                    }
                    self.import_pending = false;
                    if edit_revision != self.edit_revision {
                        self.log(Level::Warning, "Settings", "Settings changed while importing. Your edits were kept; import again to replace them.");
                        continue;
                    }
                    match result {
                        Ok(profile) => {
                            let skipped = profile.ignored_filters;
                            self.replace_profile(*profile, None, true);
                            self.log(Level::Info,"Settings","Imported OpenTabletDriver settings. Its own files were not changed.");
                            if skipped > 0 {
                                self.log(Level::Warning,"Settings",format!("Skipped {skipped} enabled filters. Add their DLLs using Plugins > Add .NET plugin."));
                            }
                        }
                        Err(error) => self.log(Level::Error, "Settings", error),
                    }
                }
                BackgroundResult::Strings(text) => {
                    self.device_strings_pending = false;
                    self.log(Level::Info, "Device strings", text.clone());
                    dialog = Some(text);
                }
                BackgroundResult::Presets(result) => {
                    self.preset_scan_pending = false;
                    self.presets_loaded = true;
                    match result {
                        Ok(names) => self.preset_names = names,
                        Err(error) => self.log(Level::Warning, "Presets", error),
                    }
                }
                BackgroundResult::Diagnostics(result) => {
                    self.diagnostics_pending = false;
                    match result {
                        Ok(DiagnosticExport::Clipboard(text)) if copy_to_clipboard(self.hwnd, &text) => self.log(Level::Info, "UI", "Copied diagnostics to the clipboard. Paths, plugin settings and log text are left out."),
                        Ok(DiagnosticExport::Clipboard(_)) => self.log(Level::Error, "UI", "Cannot open the clipboard."),
                        Ok(DiagnosticExport::Saved(path)) => self.log(Level::Info, "UI", format!("Saved diagnostics to {}. Paths, plugin settings and log text are left out.", path.display())),
                        Err(error) => self.log(Level::Error, "UI", format!("Cannot export diagnostics: {error}")),
                    }
                }
            }
        }
        dialog
    }

    pub(super) fn read_device_strings(&mut self) {
        if self.device_strings_pending {
            return;
        }
        self.device_strings_pending = self.background("device-strings", || {
            BackgroundResult::Strings(commands::device_string_report())
        });
    }

    pub(super) fn add_plugin_folder(&mut self, folder: PathBuf, name: String) {
        self.discover_plugin_folder(folder, name, true);
    }

    fn discover_plugin_folder(&mut self, folder: PathBuf, name: String, select_added: bool) {
        if self.metadata_pending.contains_key(&folder) {
            return;
        }
        self.forget_plugin_folder(&folder);
        self.metadata_pending.insert(folder.clone(), true);
        let revision = self.metadata_versions.entry(folder.clone()).or_default();
        *revision = revision.wrapping_add(1);
        let revision = *revision;
        let generation = self.metadata_generation;
        let work_folder = folder.clone();
        let mut paths: Vec<_> = self.editor.profile.plugins.iter().map(|plugin| plugin.path.clone()).collect();
        if !self.background("plugin-folder-inspection", move || {
            let mut entries = Vec::new();
            let mut errors = Vec::new();
            for path in crate::plugin_catalog::dlls(&work_folder) {
                paths.push(path.clone());
                match crate::dotnet::inspect_details(&path) {
                    Ok(found) => entries.extend(found.into_iter().map(|entry| crate::dotnet::InspectedFilter {
                        config: PluginConfig { path: path.clone(), ..entry.config },
                        metadata: entry.metadata,
                    })),
                    Err(error) => errors.push(format!("{}: {error}", path.display())),
                }
            }
            BackgroundResult::PluginFolder {
                generation,
                revision,
                select_added,
                folder: work_folder,
                name,
                entries,
                errors,
                aliases: path_aliases(paths),
            }
        }) {
            self.metadata_pending.remove(&folder);
        }
    }

    pub(super) fn add_plugin(&mut self, path: PathBuf, dotnet: bool) {
        if dotnet {
            self.inspect_plugin(path, true);
            return;
        }
        self.complete_plugin_add(vec![PluginConfig {
            path,
            kind: PluginKind::Native,
            enabled: false,
            type_name: String::new(),
            settings_json: "{}".into(),
        }], &[], true);
    }

    fn complete_plugin_add(&mut self, mut entries: Vec<PluginConfig>, aliases: &[(PathBuf, PathBuf)], select_added: bool) {
        entries.retain(|entry| !self.editor.profile.plugins.iter().any(|existing| {
            canonical_alias(&existing.path, aliases) == canonical_alias(&entry.path, aliases)
                && existing.kind == entry.kind && existing.type_name == entry.type_name
        }));
        let mut identities = HashSet::new();
        entries.retain(|entry| identities.insert((canonical_alias(&entry.path, aliases).clone(), match entry.kind {
            PluginKind::Native => 0u8, PluginKind::Dotnet => 1, PluginKind::DotnetTool => 2,
        }, entry.type_name.clone())));
        if entries.is_empty() {
            self.metadata_changed();
            self.log(Level::Info, "Plugins", "Updated plugin information; existing filters and settings were kept.");
            return;
        }
        if self.editor.profile.plugins.len() + entries.len() > 32 {
            self.metadata_changed();
            self.log(
                Level::Error,
                "Plugins",
                "At most 32 plugin entries are supported.",
            );
            return;
        }
        let first = self.editor.filters().len();
        let count = entries.len();
        self.editor.profile.plugins.extend(entries);
        self.mark_dirty();
        if self.editing_controls() || !select_added {
            self.metadata_changed();
        } else {
            self.selected_filter = first;
            self.refresh_filters();
            if self.tab == Tab::Filters {
                self.layout();
            } else {
                self.select_tab(Tab::Filters);
            }
        }
        self.log(Level::Info, "Plugins", format!("Added {count} filter entr{} disabled. Select one, check its settings, then enable it.", if count == 1 { "y" } else { "ies" }));
    }

    pub(super) fn detect_tablet(&mut self) {
        self.refresh_tablets(true);
    }

    pub(super) fn installed_plugin_changed(&mut self, folder: PathBuf, name: String) {
        // A reinstall supersedes an inspection of the previous DLL contents.
        self.metadata_pending.remove(&folder);
        self.discover_plugin_folder(folder, name, false);
    }

    pub(super) fn forget_plugin_folder(&mut self, folder: &Path) {
        self.plugin_metadata.retain(|path, _| !within_folder(path, folder));
        for (path, version) in &mut self.metadata_versions {
            if within_folder(path, folder) { *version = version.wrapping_add(1); }
        }
        self.metadata_pending.retain(|path, _| !within_folder(path, folder));
    }

    pub(super) fn plugin_removed(&mut self, folder: &Path) {
        self.forget_plugin_folder(folder);
        for plugin in &self.editor.profile.plugins {
            if within_folder(&plugin.path, folder) {
                self.plugin_metadata.insert(plugin.path.clone(), Err("This plugin was removed. Reinstall it or remove its saved entries.".into()));
            }
        }
        self.metadata_changed();
    }

    pub(super) fn export_diagnostics(&mut self, path: Option<PathBuf>) {
        if self.diagnostics_pending {
            self.log(Level::Info, "UI", "A diagnostic export is already running.");
            return;
        }
        let profile = self.editor.profile.clone();
        self.diagnostics_pending = self.background("diagnostic-export", move || {
            let result = crate::diagnostics::bundle(Some(&profile), false)
                .and_then(|bundle| serde_json::to_string_pretty(&bundle).map_err(|error| error.to_string()))
                .and_then(|text| match path {
                    Some(path) => std::fs::write(&path, text).map(|()| DiagnosticExport::Saved(path)).map_err(|error| error.to_string()),
                    None => Ok(DiagnosticExport::Clipboard(text)),
                });
            BackgroundResult::Diagnostics(result)
        });
    }

    pub(super) fn refresh_presets(&mut self) {
        if self.preset_scan_pending { return; }
        self.preset_scan_pending = self.background("preset-listing", || BackgroundResult::Presets(presets::list_names()));
    }

    pub(super) fn refresh_tablets(&mut self, announce: bool) {
        if self.device_scan_pending {
            return;
        }
        self.device_scan_pending =
            self.background("tablet-discovery", move || BackgroundResult::Devices {
                announce,
                result: crate::hid::connected_tablets()
                    .map(|mut names| {
                        names.sort();
                        names
                    }),
            });
    }
}

/// Resolve DLL identities on the inspection worker, never in a window callback.
fn path_aliases(paths: Vec<PathBuf>) -> Vec<(PathBuf, PathBuf)> {
    paths.into_iter().map(|path| {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        (path, canonical)
    }).collect()
}

fn canonical_alias<'a>(path: &'a PathBuf, aliases: &'a [(PathBuf, PathBuf)]) -> &'a PathBuf {
    aliases.iter().find(|(original, _)| original == path).map_or(path, |(_, canonical)| canonical)
}

/// Canonical Windows paths can carry a verbatim prefix while stored paths do not.
pub(super) fn within_folder(path: &Path, folder: &Path) -> bool {
    let normalize = |path: &Path| {
        let path = path.to_string_lossy().replace('/', "\\").to_lowercase();
        if let Some(path) = path.strip_prefix("\\\\?\\unc\\") {
            format!("\\\\{path}")
        } else {
            path.strip_prefix("\\\\?\\").unwrap_or(&path).to_owned()
        }
    };
    let path = normalize(path);
    let folder = normalize(folder);
    path == folder || path.starts_with(&(folder.trim_end_matches('\\').to_owned() + "\\"))
}
